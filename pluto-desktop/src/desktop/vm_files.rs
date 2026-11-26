use super::{Lookup, egui, vm_client::vm_helper_command};
use anyhow::{Context, Result, bail};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{process::Command, runtime::Runtime};

const WORKSPACE: &str = "/var/lib/pluto-workspace";

#[derive(Clone)]
struct Entry {
    name: String,
    directory: bool,
}

enum Transfer {
    Cancelled,
    Imported,
    Exported(PathBuf),
}

#[derive(Default)]
pub(super) struct VmFiles {
    folder: PathBuf,
    entries: Lookup<Vec<Entry>>,
    selected: Option<usize>,
    transfer: Lookup<Transfer>,
    notice: Option<String>,
}

impl VmFiles {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, runtime: &Runtime, instance: &str) {
        self.entries.poll();
        self.transfer.poll();
        if let Lookup::Complete(_) = &self.transfer
            && let Lookup::Complete(result) = std::mem::take(&mut self.transfer)
        {
            match result {
                Ok(Transfer::Cancelled) => {}
                Ok(Transfer::Imported) => {
                    self.notice = Some("Import complete.".into());
                    self.entries = Lookup::Idle;
                }
                Ok(Transfer::Exported(path)) => {
                    self.notice = Some(format!("Saved to {}", path.display()));
                }
                Err(error) => self.notice = Some(format!("{error:#}")),
            }
        }
        if matches!(self.entries, Lookup::Idle) {
            self.reload(runtime, ui.ctx(), instance);
        }
        let transferring = matches!(self.transfer, Lookup::Loading(_));
        let can_transfer = !transferring && matches!(self.entries, Lookup::Complete(Ok(_)));
        let selected = match (&self.entries, self.selected) {
            (Lookup::Complete(Ok(entries)), Some(index)) => entries.get(index).cloned(),
            _ => None,
        };

        ui.heading("Files");
        ui.label(
            Path::new(WORKSPACE)
                .join(&self.folder)
                .display()
                .to_string(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !transferring && !self.folder.as_os_str().is_empty(),
                    egui::Button::new("Up"),
                )
                .clicked()
            {
                self.folder.pop();
                self.reload(runtime, ui.ctx(), instance);
            }
            if ui
                .add_enabled(!transferring, egui::Button::new("Refresh"))
                .clicked()
            {
                self.reload(runtime, ui.ctx(), instance);
            }
            for (label, directory) in [("Import File", false), ("Import Folder", true)] {
                if ui
                    .add_enabled(can_transfer, egui::Button::new(label))
                    .clicked()
                {
                    self.notice = None;
                    self.transfer.start(
                        runtime,
                        ui.ctx(),
                        import(instance.to_owned(), self.folder.clone(), directory),
                    );
                }
            }
            if ui
                .add_enabled(
                    can_transfer && selected.is_some(),
                    egui::Button::new("Export Selected"),
                )
                .clicked()
                && let Some(entry) = selected
            {
                self.notice = None;
                self.transfer.start(
                    runtime,
                    ui.ctx(),
                    export(instance.to_owned(), self.folder.clone(), entry),
                );
            }
        });

        if transferring {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("File operation in progress…");
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        ui.separator();
        match &self.entries {
            Lookup::Complete(Ok(entries)) if entries.is_empty() => {
                ui.weak("This folder is empty.");
            }
            Lookup::Complete(Ok(entries)) => {
                let mut open = None;
                ui.add_enabled_ui(!transferring, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (index, entry) in entries.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let row =
                                    ui.selectable_label(self.selected == Some(index), &entry.name);
                                if row.clicked() {
                                    self.selected = Some(index);
                                }
                                if entry.directory {
                                    ui.weak("Folder");
                                    if ui.small_button("Open").clicked() || row.double_clicked() {
                                        open = Some(entry.name.clone());
                                    }
                                }
                            });
                        }
                    });
                });
                if let Some(folder) = open {
                    self.folder.push(folder);
                    self.reload(runtime, ui.ctx(), instance);
                }
            }
            Lookup::Complete(Err(error)) => {
                ui.colored_label(ui.visuals().warn_fg_color, format!("{error:#}"));
            }
            _ => {
                ui.spinner();
            }
        }
    }

    fn reload(&mut self, runtime: &Runtime, context: &egui::Context, instance: &str) {
        self.selected = None;
        let folder = self.folder.clone();
        let instance = instance.to_owned();
        self.entries.start(runtime, context, async move {
            let mut command = vm_helper_command(&instance)?;
            command.arg("list-files");
            if !folder.as_os_str().is_empty() {
                command.arg(folder);
            }
            let bytes = vm_output(command, Stdio::piped()).await?;
            let mut fields = bytes.split_inclusive(|byte| *byte == 0);
            let mut entries = Vec::new();
            while let Some(kind) = fields.next() {
                let name = fields
                    .next()
                    .and_then(|field| field.strip_suffix(b"\0"))
                    .context("Incomplete VM directory entry")?;
                let Ok(name) = std::str::from_utf8(name) else {
                    continue;
                };
                if name.is_empty()
                    || name.contains('/')
                    || matches!(name, "." | "..")
                    || !matches!(kind, b"d\0" | b"f\0")
                {
                    bail!("Invalid VM directory entry");
                }
                entries.push(Entry {
                    name: name.to_owned(),
                    directory: kind == b"d\0",
                });
            }
            entries.sort_by(|left, right| {
                right
                    .directory
                    .cmp(&left.directory)
                    .then_with(|| left.name.cmp(&right.name))
            });
            Ok(entries)
        });
    }
}

async fn vm_output(mut command: Command, stdout: Stdio) -> Result<Vec<u8>> {
    let output = command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start the Pluto VM helper")?
        .wait_with_output()
        .await
        .context("Could not read the Pluto VM helper result")?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        bail!("VM file operation failed. {}", error.trim());
    }
    Ok(output.stdout)
}

const PICK_FILE: &str =
    "return POSIX path of (choose file with prompt \"Choose a file to import\")";
const PICK_FOLDER: &str =
    "return POSIX path of (choose folder with prompt \"Choose a folder to import\")";
const SAVE_FILE: &str = "return POSIX path of (choose file name with prompt \"Save from Pluto VM\" default name (item 1 of argv))";

async fn picker(action: &str, suggested_name: Option<&str>) -> Result<Option<PathBuf>> {
    let script =
        format!("on run argv\ntry\n{action}\non error number -128\nreturn \"\"\nend try\nend run");
    let mut command = Command::new("/usr/bin/osascript");
    command
        .args(["-e", &script, "--"])
        .args(suggested_name)
        .kill_on_drop(true);
    let output = command
        .output()
        .await
        .context("Could not open the Mac file picker")?;
    if !output.status.success() {
        bail!(
            "Mac file picker failed. {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let path = String::from_utf8(output.stdout)?;
    let path = path.strip_suffix('\n').unwrap_or(&path);
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

async fn import(instance: String, folder: PathBuf, directory: bool) -> Result<Transfer> {
    let action = if directory { PICK_FOLDER } else { PICK_FILE };
    let Some(source) = picker(action, None).await? else {
        return Ok(Transfer::Cancelled);
    };
    let name = source
        .file_name()
        .context("Choose a file or folder with a name")?;
    let mut command = vm_helper_command(&instance)?;
    let option = if directory { "--folder" } else { "--path" };
    command
        .args(["import", option])
        .arg(&source)
        .arg(folder.join(name));
    vm_output(command, Stdio::null()).await?;
    Ok(Transfer::Imported)
}

async fn export(instance: String, folder: PathBuf, entry: Entry) -> Result<Transfer> {
    let suggested_name = if entry.directory {
        format!("{}.zip", entry.name)
    } else {
        entry.name.clone()
    };
    let Some(destination) = picker(SAVE_FILE, Some(&suggested_name)).await? else {
        return Ok(Transfer::Cancelled);
    };
    let (staging, file) = ExportFile::create(
        destination
            .parent()
            .context("Choose a destination folder")?,
    )?;
    let mut command = vm_helper_command(&instance)?;
    let option = if entry.directory {
        "--folder"
    } else {
        "--path"
    };
    command
        .args(["export", option])
        .arg(folder.join(entry.name));
    vm_output(command, Stdio::from(file)).await?;
    fs::hard_link(&staging.0, &destination).with_context(|| {
        format!(
            "Could not save {}. Choose a new filename if it already exists.",
            destination.display()
        )
    })?;
    Ok(Transfer::Exported(destination))
}

struct ExportFile(PathBuf);

impl ExportFile {
    fn create(parent: &Path) -> Result<(Self, File)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".pluto-export-{}-{id}", std::process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => return Ok((Self(path), file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .context("Could not stage the download in the selected folder");
                }
            }
        }
    }
}

impl Drop for ExportFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vm_export_streams_into_the_local_file() -> Result<()> {
        let (staging, file) = ExportFile::create(&std::env::temp_dir())?;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'file contents'"]);
        assert!(vm_output(command, Stdio::from(file)).await?.is_empty());
        assert_eq!(fs::read(&staging.0)?, b"file contents");
        Ok(())
    }
}
