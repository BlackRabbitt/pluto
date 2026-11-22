use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ChatHistory {
    messages: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    timestamps: Vec<Option<DateTime<Utc>>>,
}

impl ChatHistory {
    pub fn push(&mut self, role: &str, content: String) {
        self.timestamps.resize(self.messages.len(), None);
        self.messages.push((role.into(), content));
        self.timestamps.push(Some(Utc::now()));
    }

    pub fn messages(&self) -> &[(String, String)] {
        &self.messages
    }

    pub fn timestamp(&self, index: usize) -> Option<DateTime<Utc>> {
        self.timestamps.get(index).copied().flatten()
    }
}

pub struct ChatStore {
    directory: PathBuf,
}

pub struct ChatEntry {
    pub path: PathBuf,
    pub title: String,
}

#[derive(Serialize)]
struct SavedChat<'a> {
    title: &'a str,
    #[serde(flatten)]
    history: &'a ChatHistory,
}

#[derive(Deserialize)]
struct ChatTitle {
    title: String,
}

impl ChatStore {
    pub fn beside(config_path: &Path) -> Self {
        Self {
            directory: config_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join("chats"),
        }
    }

    pub fn entries(&self) -> Result<(Vec<ChatEntry>, usize)> {
        Self::entries_in(&self.directory)
    }

    fn archive_directory(&self) -> PathBuf {
        self.directory.join("archive")
    }

    pub fn archived_entries(&self) -> Result<(Vec<ChatEntry>, usize)> {
        Self::entries_in(&self.archive_directory())
    }

    pub fn is_archived(&self, path: &Path) -> bool {
        path.parent() == Some(self.archive_directory().as_path())
    }

    pub fn archive(&self, path: &Path) -> Result<PathBuf> {
        Self::move_chat(path, &self.directory, &self.archive_directory())
            .context("Could not archive chat")
    }

    pub fn restore(&self, path: &Path) -> Result<PathBuf> {
        Self::move_chat(path, &self.archive_directory(), &self.directory)
            .context("Could not restore chat")
    }

    fn move_chat(path: &Path, source: &Path, destination: &Path) -> Result<PathBuf> {
        ensure!(
            path.parent() == Some(source)
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                && fs::symlink_metadata(path)?.is_file(),
            "Expected a saved conversation file"
        );
        fs::create_dir_all(destination)?;
        let target = destination.join(path.file_name().context("Missing chat filename")?);
        match fs::symlink_metadata(&target) {
            Ok(_) => anyhow::bail!("A chat with this filename already exists"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::rename(path, &target)?;
        Ok(target)
    }

    fn entries_in(directory: &Path) -> Result<(Vec<ChatEntry>, usize)> {
        let paths = match fs::read_dir(directory) {
            Ok(paths) => paths,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(error) => return Err(error).context("Could not open the chat history folder"),
        };
        let mut entries = Vec::new();
        let mut skipped = 0;
        for entry in paths {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json")
                || !entry.file_type()?.is_file()
            {
                continue;
            }
            let title = File::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(|file| {
                    Ok(serde_json::from_reader::<_, ChatTitle>(BufReader::new(
                        file,
                    ))?)
                });
            match title {
                Ok(ChatTitle { title }) => {
                    let modified = entry
                        .metadata()
                        .and_then(|meta| meta.modified())
                        .unwrap_or(UNIX_EPOCH);
                    entries.push((modified, ChatEntry { path, title }));
                }
                Err(_) => skipped += 1,
            }
        }
        entries.sort_unstable_by(|(a_time, a), (b_time, b)| {
            b_time.cmp(a_time).then_with(|| b.path.cmp(&a.path))
        });
        Ok((
            entries.into_iter().map(|(_, entry)| entry).collect(),
            skipped,
        ))
    }

    pub fn read(&self, path: &Path) -> Result<ChatHistory> {
        let file = File::open(path).context("Could not open the saved chat")?;
        let chat: ChatHistory = serde_json::from_reader(BufReader::new(file))
            .context("Could not read the saved chat")?;
        ensure!(
            chat.messages
                .iter()
                .all(|(role, _)| matches!(role.as_str(), "user" | "assistant")),
            "Saved chat contains an invalid message role"
        );
        Ok(chat)
    }

    pub fn save(&self, path: Option<&Path>, history: &ChatHistory) -> Result<ChatEntry> {
        fs::create_dir_all(&self.directory).context("Could not create the chat history folder")?;
        let path = match path {
            Some(path) => {
                ensure!(
                    path.parent() == Some(self.directory.as_path()),
                    "Restore the chat before editing it"
                );
                path.to_owned()
            }
            None => self.directory.join(format!("{}.json", unique_id()?)),
        };
        let title = history
            .messages()
            .iter()
            .find(|(role, _)| role == "user")
            .map(|(_, text)| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "Untitled chat".into());
        let title = match title.char_indices().nth(60) {
            Some((end, _)) => format!("{}…", &title[..end]),
            None => title,
        };
        let temporary = path.with_extension(format!("{}.tmp", unique_id()?));
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(&temporary)?;
        let result = (|| -> Result<()> {
            let mut file = BufWriter::new(file);
            serde_json::to_writer_pretty(
                &mut file,
                &SavedChat {
                    title: &title,
                    history,
                },
            )?;
            file.flush()?;
            file.get_ref().sync_all()?;
            drop(file);
            fs::rename(&temporary, &path).context("Could not replace the saved chat")
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result?;
        Ok(ChatEntry { path, title })
    }
}

fn unique_id() -> Result<String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{timestamp}-{}-{sequence}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_reopens_and_updates_separate_chats() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("pluto-history-test-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = check_history(&directory);
        fs::remove_dir_all(&directory)?;
        result
    }

    fn check_history(directory: &Path) -> Result<()> {
        let config = directory.join("tools.toml");
        let store = ChatStore::beside(&config);
        assert!(store.entries()?.0.is_empty());

        let mut first: ChatHistory = serde_json::from_str(
            r#"{"title":"First chat","messages":[["user","First chat\nwith two lines"]]}"#,
        )?;
        let first_entry = store.save(None, &first)?;
        assert_eq!(first_entry.title, "First chat with two lines");
        let mut second = ChatHistory::default();
        second.push("user", "Second chat".into());
        let second_entry = store.save(None, &second)?;
        assert_ne!(first_entry.path, second_entry.path);
        first.push("assistant", "Saved reply".into());
        let updated = store.save(Some(&first_entry.path), &first)?;
        assert_eq!(updated.path, first_entry.path);
        fs::write(store.directory.join("partial.tmp"), "{")?;
        fs::write(store.directory.join("bad.json"), "{")?;

        let reopened = ChatStore::beside(&config);
        let (entries, skipped) = reopened.entries()?;
        assert_eq!(entries.len(), 2);
        assert_eq!(skipped, 1);
        assert_eq!(reopened.read(&first_entry.path)?, first);
        assert_eq!(reopened.read(&second_entry.path)?, second);
        assert!(first.timestamp(0).is_none());
        assert!(first.timestamp(1).is_some());
        assert!(second.timestamp(0).is_some());

        let original = fs::read(&first_entry.path)?;
        let archive = reopened.archive(&first_entry.path)?;
        let reopened = ChatStore::beside(&config);
        assert!(reopened.is_archived(&archive));
        assert_eq!(reopened.entries()?.0.len(), 1);
        assert_eq!(reopened.archived_entries()?.0[0].path, archive);
        assert_eq!(fs::read(&archive)?, original);
        assert!(reopened.save(Some(&archive), &first).is_err());

        fs::write(&first_entry.path, b"existing file")?;
        assert!(reopened.restore(&archive).is_err());
        assert_eq!(fs::read(&archive)?, original);
        assert_eq!(fs::read(&first_entry.path)?, b"existing file");
        fs::remove_file(&first_entry.path)?;

        let restored = reopened.restore(&archive)?;
        assert_eq!(restored, first_entry.path);
        assert_eq!(fs::read(&restored)?, original);
        assert!(reopened.archived_entries()?.0.is_empty());
        assert_eq!(reopened.entries()?.0.len(), 2);
        Ok(())
    }
}
