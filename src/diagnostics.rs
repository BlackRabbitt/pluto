use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
static NEXT_TURN: AtomicU64 = AtomicU64::new(1);

pub type LogSender = SyncSender<Entry>;

#[derive(Serialize)]
pub struct Entry {
    unix_ms: u128,
    process: u32,
    turn: u64,
    event: &'static str,
    details: Value,
}

#[derive(Default)]
pub struct Trace {
    sender: Option<LogSender>,
    turn: u64,
    arguments: RandomState,
}

impl Trace {
    pub fn new(sender: Option<LogSender>) -> Self {
        Self {
            sender,
            turn: NEXT_TURN.fetch_add(1, Ordering::Relaxed),
            arguments: RandomState::new(),
        }
    }

    pub fn arguments_id(&self, raw: &str) -> u64 {
        self.arguments.hash_one(raw)
    }

    pub fn record(&self, event: &'static str, details: impl FnOnce() -> Value) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(Entry {
                unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                process: std::process::id(),
                turn: self.turn,
                event,
                details: details(),
            });
        }
    }
}

pub fn start(path: PathBuf) -> io::Result<LogSender> {
    let (sender, receiver) = mpsc::sync_channel::<Entry>(256);
    std::thread::Builder::new()
        .name("pluto-diagnostics".into())
        .spawn(move || {
            let mut log = None;
            for entry in receiver {
                if let Err(error) = append(&path, &mut log, &entry) {
                    eprintln!("Pluto could not write diagnostics: {error}");
                    break;
                }
            }
        })?;
    Ok(sender)
}

fn append(path: &Path, log: &mut Option<(File, u64)>, entry: &Entry) -> io::Result<()> {
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    if log.is_none() {
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
        let file = open(path)?;
        let bytes = file.metadata()?.len();
        *log = Some((file, bytes));
    }
    let (file, bytes) = log.as_mut().expect("log is open");
    if *bytes + line.len() as u64 > MAX_LOG_BYTES {
        fs::rename(path, path.with_extension("previous.jsonl"))?;
        *file = open(path)?;
        *bytes = 0;
    }
    file.write_all(&line)?;
    *bytes += line.len() as u64;
    Ok(())
}

fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn background_writer_records_events_and_rotates_a_full_log() -> anyhow::Result<()> {
        let directory = std::env::temp_dir().join(format!("pluto-log-test-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let path = directory.join("agent-debug.jsonl");
        let result = (|| {
            File::create(&path)?.set_len(MAX_LOG_BYTES)?;
            let trace = Trace::new(Some(start(path.clone())?));
            trace.record("tool_finished", || json!({"call": 1}));
            drop(trace);

            let deadline = Instant::now() + Duration::from_secs(3);
            let line = loop {
                if let Ok(line) = fs::read_to_string(&path)
                    && line.ends_with('\n')
                {
                    break line;
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "Diagnostic event was not written"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let entry: Value = serde_json::from_str(&line)?;
            assert_eq!(entry["event"], "tool_finished");
            assert_eq!(entry["details"]["call"], 1);
            assert_eq!(
                fs::metadata(path.with_extension("previous.jsonl"))?.len(),
                MAX_LOG_BYTES
            );
            anyhow::Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }
}
