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
    Removed,
}

#[derive(Default)]
pub(super) struct VmFiles {
    folder: PathBuf,
    entries: Lookup<Vec<Entry>>,
    selected: Option<usize>,
    transfer: Lookup<Transfer>,
    notice: Option<String>,
    pending_removal: Option<(PathBuf, bool)>,
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
                Ok(Transfer::Removed) => {
                    self.notice = Some("Item removed.".into());
                    self.entries = Lookup::Idle;
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
                && let Some(entry) = &selected
            {
                self.notice = None;
                self.transfer.start(
                    runtime,
                    ui.ctx(),
                    export(instance.to_owned(), self.folder.clone(), entry.clone()),
                );
            }
            if ui
                .add_enabled(
                    can_transfer && selected.is_some(),
                    egui::Button::new("Remove Selected"),
                )
                .clicked()
                && let Some(entry) = selected
            {
                self.pending_removal = Some((self.folder.join(entry.name), entry.directory));
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
        if let Some((path, directory)) = &self.pending_removal {
            let dialog = egui::Modal::new(egui::Id::new("remove_vm_item")).show(ui.ctx(), |ui| {
                ui.set_max_width(420.0);
                ui.heading(if *directory {
                    "Remove folder?"
                } else {
                    "Remove file?"
                });
                ui.label(format!("Permanently remove this item from {instance}?"));
                ui.label(Path::new(WORKSPACE).join(path).display().to_string());
                if *directory {
                    ui.label("All files and folders inside it will also be removed.");
                }
                ui.horizontal(|ui| {
                    let cancel = ui.button("Cancel").clicked();
                    let remove = ui.button("Remove permanently").clicked();
                    (cancel, remove)
                })
                .inner
            });
            let (cancel, confirmed) = dialog.inner;
            if confirmed {
                let (path, directory) = self.pending_removal.take().expect("removal was selected");
                self.notice = None;
                self.transfer.start(
                    runtime,
                    ui.ctx(),
                    remove(instance.to_owned(), path, directory),
                );
            } else if cancel || dialog.should_close() {
                self.pending_removal = None;
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
    export_to(
        &instance,
        &folder.join(entry.name),
        entry.directory,
        &destination,
    )
    .await?;
    Ok(Transfer::Exported(destination))
}

async fn export_to(instance: &str, path: &Path, directory: bool, destination: &Path) -> Result<()> {
    let mut command = vm_helper_command(instance)?;
    let option = if directory { "--folder" } else { "--path" };
    command.args(["export", option]).arg(path);
    save_export(command, destination).await
}

async fn save_export(command: Command, destination: &Path) -> Result<()> {
    let (staging, file) = ExportFile::create(destination)?;
    vm_output(command, Stdio::from(file)).await?;
    // The Mac save dialog confirms replacement before returning an existing file path.
    fs::rename(&staging.0, destination)
        .with_context(|| format!("Could not save {}", destination.display()))?;
    Ok(())
}

async fn remove(instance: String, path: PathBuf, directory: bool) -> Result<Transfer> {
    let mut command = vm_helper_command(&instance)?;
    let option = if directory { "--folder" } else { "--file" };
    command.args(["remove", option]).arg(path);
    vm_output(command, Stdio::null()).await?;
    Ok(Transfer::Removed)
}

struct ExportFile(PathBuf);

impl ExportFile {
    fn create(destination: &Path) -> Result<(Self, File)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = destination
            .parent()
            .context("Choose a destination folder")?;
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".pluto-export-{}-{id}", std::process::id()));
            if path == destination {
                continue;
            }
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
    use std::io::Write;

    #[tokio::test]
    async fn vm_export_replaces_existing_download_only_on_success() -> Result<()> {
        let (destination, mut file) =
            ExportFile::create(&std::env::temp_dir().join("pluto-export-test"))?;
        file.write_all(b"previous download")?;
        drop(file);

        let mut failed = Command::new("/bin/sh");
        failed.args(["-c", "printf 'partial download'; exit 1"]);
        assert!(save_export(failed, &destination.0).await.is_err());
        assert_eq!(fs::read(&destination.0)?, b"previous download");

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'file contents'"]);
        save_export(command, &destination.0).await?;
        assert_eq!(fs::read(&destination.0)?, b"file contents");
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires an isolated fixture in a running Pluto VM"]
    async fn desktop_file_actions_round_trip_on_vm() -> Result<()> {
        let instance = std::env::var("PLUTO_FILES_TEST_VM")?;
        let fixture = PathBuf::from(
            std::env::var_os("PLUTO_FILES_TEST_FIXTURE")
                .context("Set the workspace fixture path")?,
        );
        let fixture_name = fixture.to_str().context("Use a UTF-8 fixture name")?;
        anyhow::ensure!(
            fixture.components().count() == 1 && fixture_name.starts_with("Desktop Transfer Test "),
            "Use a disposable top-level Desktop Transfer Test folder"
        );
        let output = PathBuf::from(
            std::env::var_os("PLUTO_FILES_TEST_OUTPUT")
                .context("Set the local output directory")?,
        );
        let file = fixture.join("summary.txt");
        let archive = output.join("folder.zip");
        let download = output.join("summary.txt");
        export_to(&instance, &fixture, true, &archive).await?;
        export_to(&instance, &file, false, &download).await?;
        assert_eq!(fs::read(download)?, b"Pluto desktop file export fixture\n");
        let member = fixture.join("Inside/result.txt");
        let member = member.to_str().context("ZIP member name must be UTF-8")?;
        let contents = Command::new("/usr/bin/unzip")
            .arg("-p")
            .arg(&archive)
            .arg(member)
            .output()
            .await?;
        assert!(contents.status.success());
        assert_eq!(contents.stdout, b"Pluto desktop folder export fixture\n");

        let mut contents = vm_helper_command(&instance)?;
        contents.arg("list-files").arg(&fixture);
        remove(instance.clone(), file, false).await?;
        let entries = vm_output(contents, Stdio::piped()).await?;
        assert_eq!(entries, b"d\0Inside\0");
        remove(instance.clone(), fixture.clone(), true).await?;
        let mut contents = vm_helper_command(&instance)?;
        contents.arg("list-files").arg(fixture);
        assert!(vm_output(contents, Stdio::piped()).await.is_err());
        Ok(())
    }
}
