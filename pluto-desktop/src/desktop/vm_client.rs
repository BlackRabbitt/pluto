use super::{
    ApprovalDecision, ApprovalRequest, ChatComposer, CommonMarkCache, Desktop, Lookup,
    PickerCatalog, chat_list, chat_title, chat_transcript, context_usage, egui, field,
    thinking_picker, tool_approval_dialog, vm_files::VmFiles, vm_settings::VmSettingsForm,
};
use anyhow::{Context, Result, bail, ensure};
use pluto::{
    agent::{TokenUsage, ToolProgress, TurnSelection},
    api::{ApprovalScope, ChatSummary, ChatUpdate, Request, Response, read_message, write_message},
    history::ChatHistory,
    providers::{Provider, ThinkingLevel, ThinkingSetting},
    tools::{ApprovalRule, Config, ModelMode, valid_name},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::{io::BufReader, process::Command, runtime::Runtime, sync::oneshot};

pub(super) struct DesktopApp {
    standalone: Option<Desktop>,
    vm: Option<VmClient>,
    logo: egui::TextureHandle,
}

impl DesktopApp {
    pub(super) fn new(
        instance: Option<String>,
        use_saved: bool,
        context: &egui::Context,
        logo: egui::TextureHandle,
    ) -> Result<Self> {
        let instance = if use_saved {
            Self::saved_instance()
        } else {
            instance
        };
        let mut app = Self {
            standalone: None,
            vm: None,
            logo,
        };
        app.switch(instance, context)?;
        Ok(app)
    }

    fn saved_instance() -> Option<String> {
        let path = Config::path().ok()?.with_file_name("desktop-vm");
        let saved = fs::read_to_string(path).ok()?;
        let saved = saved.trim();
        valid_instance(saved).then(|| saved.to_owned())
    }

    fn switch(&mut self, instance: Option<String>, context: &egui::Context) -> Result<()> {
        if let Some(instance) = instance {
            self.vm = Some(VmClient::new(instance, context.clone(), self.logo.clone())?);
        } else {
            if self.standalone.is_none() {
                let mut desktop = Desktop::new()?;
                desktop.logo = Some(self.logo.clone());
                self.standalone = Some(desktop);
            }
            self.vm = None;
        }
        Ok(())
    }

    fn save_selection(instance: Option<&str>) -> Result<()> {
        let path = Config::path()?.with_file_name("desktop-vm");
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, instance.unwrap_or_default())?;
        fs::rename(&temporary, path)?;
        Ok(())
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        let selection = if let Some(vm) = &mut self.vm {
            vm.update(context, frame);
            vm.picker.selection.take()
        } else if let Some(desktop) = &mut self.standalone {
            desktop.update(context, frame);
            desktop.vm_picker.selection.take()
        } else {
            None
        };
        if let Some(selection) = selection {
            let result = self
                .switch(selection.clone(), context)
                .and_then(|()| Self::save_selection(selection.as_deref()));
            if let Err(error) = result {
                let message = format!("Could not switch or remember the connection: {error:#}");
                if let Some(vm) = &mut self.vm {
                    vm.warning = Some(message);
                } else if let Some(desktop) = &mut self.standalone {
                    desktop.vm_picker.notice = Some(message);
                }
            }
            context.request_repaint();
        }
    }
}

#[derive(Default)]
pub(super) struct VmPicker {
    instances: Lookup<Vec<String>>,
    selection: Option<Option<String>>,
    notice: Option<String>,
    new_vm: Option<String>,
    creation: Lookup<()>,
}

impl VmPicker {
    pub(super) fn refresh(&mut self) {
        self.instances = Lookup::Idle;
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &Runtime,
        busy: bool,
        current: Option<&str>,
    ) {
        self.creation.poll();
        if matches!(self.creation, Lookup::Complete(Ok(()))) && self.new_vm.take().is_some() {
            self.refresh();
        }
        let creating = matches!(self.creation, Lookup::Loading(_));
        self.instances.poll();
        if matches!(self.instances, Lookup::Idle) {
            self.instances.start(runtime, ui.ctx(), list_instances());
        }

        ui.horizontal_wrapped(|ui| {
            ui.heading("Connection");
            if cfg!(all(target_os = "macos", target_arch = "aarch64"))
                && self.new_vm.is_none()
                && ui.add_enabled(!busy, egui::Button::new("New VM")).clicked()
            {
                self.creation = Lookup::Idle;
                self.new_vm = Some(String::new());
            }
        });
        let instances = match &self.instances {
            Lookup::Complete(Ok(instances)) => instances.as_slice(),
            _ => &[],
        };
        egui::Grid::new("pluto connections").show(ui, |ui| {
            if Self::connection_row(
                ui,
                "Standalone · Unsandboxed",
                current.is_none(),
                busy || creating,
            ) {
                self.selection = Some(None);
            }
            for name in instances {
                if Self::connection_row(ui, name, current == Some(name.as_str()), busy || creating)
                {
                    self.selection = Some(Some(name.clone()));
                }
            }
            if let Some(name) = current
                && !instances.iter().any(|instance| instance == name)
            {
                Self::connection_row(ui, name, true, busy);
            }
        });

        if let Some(name) = &mut self.new_vm {
            ui.add_enabled_ui(!creating, |ui| {
                field(ui, "VM name", name, "pluto-work");
            });
            if creating {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Creating {name}…"));
                });
            } else {
                let target = name.trim().to_owned();
                let duplicate = instances
                    .iter()
                    .any(|instance| instance.eq_ignore_ascii_case(&target))
                    || current.is_some_and(|instance| instance.eq_ignore_ascii_case(&target));
                let valid = valid_instance(&target);
                if duplicate {
                    ui.label("A VM with that name already exists.");
                } else if !target.is_empty() && !valid {
                    ui.label("Use up to 64 letters, numbers, hyphens, or underscores. Start with a letter, number, or underscore.");
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy && valid && !duplicate, egui::Button::new("Create VM"))
                        .clicked()
                    {
                        self.creation.start(runtime, ui.ctx(), create_vm(target));
                    }
                    if ui.button("Cancel").clicked() {
                        self.new_vm = None;
                        self.creation = Lookup::Idle;
                    }
                });
            }
        }
        match &self.creation {
            Lookup::Complete(Ok(())) => {
                ui.label("VM created. Choose Connect to configure its model.");
            }
            Lookup::Complete(Err(error)) => {
                ui.colored_label(ui.visuals().warn_fg_color, format!("{error:#}"));
            }
            _ => {}
        }

        match &self.instances {
            Lookup::Idle | Lookup::Loading(_) => {
                ui.weak("Looking for Pluto VMs…");
            }
            Lookup::Complete(result) => {
                if let Err(error) = result {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{error:#}"));
                } else if instances.is_empty() {
                    ui.label("No Pluto VMs found.");
                }
                if instances.is_empty() {
                    ui.hyperlink_to(
                        "Set up Pluto VM",
                        "https://github.com/BlackRabbitt/pluto/blob/pluto-as-a-machine/pluto/vm/README.md#quick-start",
                    );
                }
            }
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(ui.visuals().warn_fg_color, notice);
        }
        ui.separator();
    }

    fn connection_row(ui: &mut egui::Ui, name: &str, current: bool, busy: bool) -> bool {
        ui.label(name);
        let connect = if current {
            ui.weak("Current");
            false
        } else {
            ui.add_enabled(!busy, egui::Button::new("Connect"))
                .clicked()
        };
        ui.end_row();
        connect
    }
}

fn valid_instance(name: &str) -> bool {
    valid_name(name) && !name.starts_with('-')
}

fn lima_executable() -> Result<PathBuf> {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("limactl"))
        .chain(["/opt/homebrew/bin/limactl", "/usr/local/bin/limactl"].map(PathBuf::from))
        .find(|path| path.is_file())
        .context("Could not find limactl. Install Lima and set up Pluto VM.")
}

fn lima_command() -> Result<Command> {
    let home = std::env::var_os("PLUTO_LIMA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".pluto-lima")))
        .context("Set PLUTO_LIMA_HOME to the Pluto VM storage directory")?;
    let mut command = Command::new(lima_executable()?);
    command.env("LIMA_HOME", home).kill_on_drop(true);
    Ok(command)
}

pub(super) fn vm_helper_command(instance: &str) -> Result<Command> {
    let lima = lima_executable()?;
    let resources = "Contents/Resources/pluto";
    let executable = std::env::current_exe()?;
    let packaged = executable
        .parent()
        .and_then(|dir| dir.parent())
        .map(|contents| contents.join("Resources/pluto"));
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pluto");
    let installed =
        dirs::home_dir().map(|home| home.join("Applications/Pluto.app").join(resources));
    let helper = packaged
        .into_iter()
        .chain([source])
        .chain(installed)
        .filter(|directory| directory.join("Cargo.lock").is_file())
        .map(|directory| directory.join("vm/vm"))
        .find(|helper| helper.is_file())
        .context(
            "VM installer files are missing. Reinstall Pluto Desktop from its macOS installer.",
        )?;
    let path = std::env::join_paths(
        lima.parent().into_iter().map(PathBuf::from).chain(
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths),
        ),
    )?;
    let mut command = Command::new("/bin/sh");
    command
        .arg(helper)
        .env("PLUTO_INSTANCE", instance)
        .env("PATH", path)
        .kill_on_drop(true);
    Ok(command)
}

async fn create_vm(instance: String) -> Result<()> {
    let output = vm_helper_command(&instance)?
        .args(["create", &instance])
        .output()
        .await
        .context("Could not start the Pluto VM installer")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = stderr.lines().collect::<Vec<_>>();
        let details = details[details.len().saturating_sub(8)..].join("\n");
        bail!(
            "Could not create {instance} ({}).\n{details}",
            output.status
        );
    }
    Ok(())
}

async fn list_instances() -> Result<Vec<String>> {
    let output = lima_command()?
        .args(["list", "--format", "{{.Name}}"])
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "Could not list Lima instances: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let builder = std::env::var("PLUTO_BUILDER").unwrap_or_else(|_| "pluto-builder".into());
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|name| valid_instance(name) && *name != builder)
        .map(str::to_owned)
        .collect())
}

struct VmResources {
    status: String,
    cpus: u32,
    memory_bytes: u64,
}

impl VmResources {
    fn parse(line: &str) -> Result<Self> {
        let mut fields = line.split_whitespace();
        let status = fields.next().context("The VM was not found")?.to_owned();
        let cpus = fields
            .next()
            .context("Lima did not report the VM CPU allocation")?
            .parse()?;
        let memory_bytes = fields
            .next()
            .context("Lima did not report the VM memory allocation")?
            .parse()?;
        Ok(Self {
            status,
            cpus,
            memory_bytes,
        })
    }
}

async fn vm_resources(instance: String) -> Result<VmResources> {
    let output = lima_command()?
        .args([
            "list",
            "--format",
            "{{.Status}} {{.CPUs}} {{.Memory}}",
            &instance,
        ])
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "Could not read VM status: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    VmResources::parse(&String::from_utf8(output.stdout)?)
}

const VM_USAGE_INTERVAL: Duration = Duration::from_secs(3);
const VM_USAGE_COMMAND: &str = r#"
    awk '/^cpu / {print $2,$3,$4,$5,$6,$7,$8,$9; exit}' /proc/stat
    sleep 1
    awk '/^cpu / {print $2,$3,$4,$5,$6,$7,$8,$9; exit}' /proc/stat
    awk '/^MemTotal:/ { total=$2 } /^MemAvailable:/ { available=$2 }
         END { if (total == "" || available == "") exit 1; print total, available }' /proc/meminfo
"#;

struct VmUsage {
    cpu_percent: u64,
    used_kib: u64,
    total_kib: u64,
}

impl VmUsage {
    fn parse(output: &str) -> Result<Self> {
        let samples = output
            .split_whitespace()
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let [before @ .., total_kib, available_kib] = samples.as_slice() else {
            bail!("Missing VM usage counters");
        };
        ensure!(
            before.len() == 16 && *total_kib > 0 && available_kib <= total_kib,
            "Invalid VM usage counters"
        );
        let (first, second) = before.split_at(8);
        let total = second
            .iter()
            .sum::<u64>()
            .saturating_sub(first.iter().sum());
        let idle = (second[3] + second[4]).saturating_sub(first[3] + first[4]);
        ensure!(total > 0, "No CPU time elapsed during the sample");
        let busy = total.saturating_sub(idle);
        Ok(Self {
            cpu_percent: (100 * busy + total / 2) / total,
            used_kib: total_kib - available_kib,
            total_kib: *total_kib,
        })
    }
}

async fn vm_usage(instance: String) -> Result<VmUsage> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        lima_command()?
            .args([
                "shell",
                "--tty=false",
                "--workdir=/",
                &instance,
                "sh",
                "-ec",
                VM_USAGE_COMMAND,
            ])
            .output(),
    )
    .await
    .context("Timed out reading VM usage")??;
    ensure!(output.status.success(), "Could not read usage from the VM");
    VmUsage::parse(std::str::from_utf8(&output.stdout)?)
}

#[derive(Default)]
struct VmUsageMonitor {
    sample: Lookup<VmUsage>,
    latest: Option<Result<VmUsage>>,
    next_sample: Option<Instant>,
}

impl VmUsageMonitor {
    fn show(&mut self, ui: &mut egui::Ui, runtime: &Runtime, instance: &str) {
        self.sample.poll();
        if matches!(self.sample, Lookup::Complete(_))
            && let Lookup::Complete(result) = std::mem::take(&mut self.sample)
        {
            self.latest = Some(result);
        }
        let now = Instant::now();
        if matches!(self.sample, Lookup::Idle) && self.next_sample.is_none_or(|next| next <= now) {
            self.next_sample = Some(now + VM_USAGE_INTERVAL);
            self.sample
                .start(runtime, ui.ctx(), vm_usage(instance.to_owned()));
        }
        if matches!(self.sample, Lookup::Idle)
            && let Some(next) = self.next_sample
        {
            ui.ctx()
                .request_repaint_after(next.saturating_duration_since(now));
        }
        match &self.latest {
            Some(Ok(usage)) => {
                ui.label(format!(
                    "CPU {}% · RAM {} / {} MiB used",
                    usage.cpu_percent,
                    usage.used_kib / 1024,
                    usage.total_kib / 1024
                ));
            }
            Some(Err(error)) => {
                ui.weak("Live usage unavailable")
                    .on_hover_text(format!("{error:#}"));
            }
            None => {
                ui.weak("Sampling live usage…");
            }
        }
    }
}

#[derive(Clone, Copy)]
enum VmPower {
    Start,
    Stop,
    Restart,
}

impl VmPower {
    fn command(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    async fn run(self, instance: &str) -> Result<()> {
        let output = lima_command()?
            .args([self.command(), "--tty=false", instance])
            .output()
            .await?;
        if !output.status.success() {
            bail!(
                "Could not {} Pluto VM: {}",
                self.command(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

pub(super) struct VmClient {
    instance: String,
    logo: Option<egui::TextureHandle>,
    runtime: Runtime,
    events: mpsc::Receiver<ClientEvent>,
    sender: mpsc::Sender<ClientEvent>,
    history: ChatHistory,
    chat_id: Option<String>,
    chats: Option<Vec<ChatSummary>>,
    // Older services omit archived summaries and lack the expanded chat API.
    archived: Option<Vec<ChatSummary>>,
    capabilities: Lookup<VmCapabilities>,
    thinking: Option<ThinkingSetting>,
    context_usage: Option<TokenUsage>,
    tool_progress: Option<ToolProgress>,
    switched_model: Option<(Provider, Option<u64>)>,
    loaded_skills: BTreeMap<usize, BTreeSet<String>>,
    chat_warnings: Vec<String>,
    saved_approvals: Lookup<Vec<ApprovalRule>>,
    pending_send: Option<(String, TurnSelection)>,
    markdown: CommonMarkCache,
    composer: ChatComposer,
    status: String,
    warning: Option<String>,
    decision: Option<Decision>,
    ready: bool,
    busy: bool,
    page: VmPage,
    files: VmFiles,
    picker: VmPicker,
    settings_form: Option<VmSettingsForm>,
    settings_load_attempted: bool,
    settings_status: Option<String>,
    resources: Lookup<VmResources>,
    usage: VmUsageMonitor,
}

#[derive(PartialEq)]
enum VmPage {
    Chat,
    Files,
    Settings,
}

enum ClientEvent {
    Response(Response),
    Decision(Decision),
    Finished(Result<()>),
    PowerFinished(VmPower, Result<()>),
}

struct Decision {
    request: ApprovalRequest,
    retry: bool,
    error: Option<String>,
    allow_other_operands: bool,
    reply: oneshot::Sender<(bool, Option<ApprovalScope>)>,
}

struct VmCapabilities {
    mode: ModelMode,
    provider: Option<Provider>,
    context_window: Option<u64>,
    thinking: std::result::Result<Vec<ThinkingLevel>, String>,
}

impl VmClient {
    fn new(instance: String, context: egui::Context, logo: egui::TextureHandle) -> Result<Self> {
        let mut client = Self::disconnected(instance, Some(logo))?;
        client.request(Request::History, &context);
        Ok(client)
    }

    fn disconnected(instance: String, logo: Option<egui::TextureHandle>) -> Result<Self> {
        let (sender, events) = mpsc::channel();
        Ok(Self {
            instance,
            logo,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()?,
            events,
            sender,
            history: ChatHistory::default(),
            chat_id: None,
            chats: None,
            markdown: CommonMarkCache::default(),
            composer: ChatComposer::default(),
            archived: None,
            capabilities: Lookup::Idle,
            thinking: None,
            context_usage: None,
            tool_progress: None,
            switched_model: None,
            loaded_skills: BTreeMap::new(),
            chat_warnings: Vec::new(),
            saved_approvals: Lookup::Idle,
            pending_send: None,
            status: String::new(),
            warning: None,
            decision: None,
            ready: false,
            busy: false,
            page: VmPage::Chat,
            files: VmFiles::default(),
            picker: VmPicker::default(),
            settings_form: None,
            settings_load_attempted: false,
            settings_status: None,
            resources: Lookup::Idle,
            usage: VmUsageMonitor::default(),
        })
    }

    fn request(&mut self, request: Request, context: &egui::Context) {
        self.busy = true;
        if matches!(request, Request::VmSettings) {
            self.settings_load_attempted = true;
        }
        self.warning = None;
        self.tool_progress = None;
        self.status = match &request {
            Request::History => "Connecting to Pluto VM…",
            Request::OpenChat { .. } => "Opening saved chat…",
            Request::MoveChat { archive: true, .. } => "Archiving chat…",
            Request::MoveChat { archive: false, .. } => "Restoring chat…",
            Request::ForgetApproval { .. } => "Removing saved approval…",
            Request::VmSettings => "Loading VM settings…",
            Request::SaveVmSettings { .. } => "Saving VM settings…",
            Request::InspectSkill { .. } | Request::ImportSkill { .. } => "Adding skill…",
            _ => "Preparing turn…",
        }
        .into();
        let context = context.clone();
        let instance = self.instance.clone();
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let emit = |event| {
                let result = sender.send(event);
                context.request_repaint();
                result
            };
            let result = exchange(&instance, request, &emit).await;
            let _ = emit(ClientEvent::Finished(result));
        });
    }

    fn reconnect(&mut self, context: &egui::Context) {
        let request = match self.chat_id.clone() {
            Some(chat_id) => Request::OpenChat { chat_id },
            None => Request::History,
        };
        self.request(request, context);
    }

    fn power(&mut self, action: VmPower, context: &egui::Context) {
        self.ready = false;
        self.busy = true;
        self.warning = None;
        self.status = match action {
            VmPower::Start => "Starting Pluto VM…",
            VmPower::Stop => "Stopping Pluto VM…",
            VmPower::Restart => "Restarting Pluto VM…",
        }
        .into();
        let (context, instance, sender) =
            (context.clone(), self.instance.clone(), self.sender.clone());
        self.runtime.spawn(async move {
            let result = action.run(&instance).await;
            let _ = sender.send(ClientEvent::PowerFinished(action, result));
            context.request_repaint();
        });
    }

    fn new_chat(&mut self) {
        self.page = VmPage::Chat;
        self.chat_id = None;
        self.history = ChatHistory::default();
        self.reset_chat();
        self.warning = None;
    }

    fn reset_chat(&mut self) {
        self.markdown = CommonMarkCache::default();
        self.composer.selection = TurnSelection::default();
        self.composer.picker_catalog = Lookup::Idle;
        self.context_usage = None;
        self.switched_model = None;
        self.loaded_skills.clear();
        self.chat_warnings.clear();
    }

    fn archived_chat(&self) -> bool {
        self.archived
            .iter()
            .flatten()
            .any(|chat| Some(&chat.id) == self.chat_id.as_ref())
    }

    fn chat_navigation(&mut self, ui: &mut egui::Ui) {
        let enabled = self.ready && !self.busy;
        if ui
            .add_enabled(
                enabled && self.chats.is_some(),
                egui::Button::new("New chat"),
            )
            .on_disabled_hover_text(if self.busy {
                "Wait for the current request to finish."
            } else {
                "Connect to an updated Pluto VM to start a new chat."
            })
            .clicked()
        {
            self.new_chat();
        }
        if self.archived.is_some()
            && self.page == VmPage::Chat
            && let Some(chat_id) = &self.chat_id
        {
            let archive = !self.archived_chat();
            let label = if archive {
                "Archive chat"
            } else {
                "Restore chat"
            };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                self.request(
                    Request::MoveChat {
                        chat_id: chat_id.clone(),
                        archive,
                    },
                    ui.ctx(),
                );
            }
        }
        let selected = chat_list(
            ui,
            self.chats
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|chat| (&chat.id, chat.title.as_str())),
            self.archived
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|chat| (&chat.id, chat.title.as_str())),
            self.chat_id.as_ref().filter(|_| self.page == VmPage::Chat),
            enabled,
        )
        .cloned();
        if let Some(chat_id) = selected {
            self.page = VmPage::Chat;
            if self.chat_id.as_ref() != Some(&chat_id) {
                self.request(Request::OpenChat { chat_id }, ui.ctx());
            }
        }
    }

    fn approval(&mut self, context: &egui::Context) {
        let Some(pending) = self.decision.as_mut().filter(|pending| !pending.retry) else {
            return;
        };
        let (decision, remember) = tool_approval_dialog(
            context,
            &pending.request,
            &mut pending.allow_other_operands,
            pending.error.as_deref(),
        );
        let answer = if remember.is_some() {
            let scope = if pending.allow_other_operands {
                ApprovalScope::OtherOperands
            } else {
                ApprovalScope::Exact
            };
            Some((true, Some(scope)))
        } else {
            decision.map(|decision| (!matches!(decision, ApprovalDecision::Decline), None))
        };
        if let Some(answer) = answer
            && let Some(decision) = self.decision.take()
        {
            if answer.1.is_some() {
                self.saved_approvals = Lookup::Idle;
            }
            let _ = decision.reply.send(answer);
        }
    }

    fn send(&mut self, context: &egui::Context) {
        if !self.ready
            || self.busy
            || self.composer.prompt.trim().is_empty()
            || self.archived_chat()
        {
            return;
        }
        if matches!(&self.capabilities, Lookup::Complete(Ok(caps)) if caps.provider.is_none()) {
            self.page = VmPage::Settings;
            self.settings_status =
                Some("Choose a provider, enter its server URL, and select a model.".into());
            return;
        }
        let prompt = std::mem::take(&mut self.composer.prompt);
        let selection = std::mem::take(&mut self.composer.selection);
        let request = if self.chats.is_some() {
            Request::SendChat {
                chat_id: self.chat_id.clone(),
                prompt: prompt.clone(),
                selection: self.archived.is_some().then(|| selection.clone()),
                thinking: self.thinking.clone(),
            }
        } else {
            Request::Send {
                prompt: prompt.clone(),
            }
        };
        self.pending_send = Some((prompt, selection));
        self.composer.picker_catalog = Lookup::Idle;
        self.chat_warnings.clear();
        if self.switched_model.take().is_some() {
            self.context_usage = None;
        }
        self.request(request, context);
    }

    fn load_capabilities(&mut self, context: &egui::Context) {
        let instance = self.instance.clone();
        self.capabilities.start(&self.runtime, context, async move {
            match query_vm_api(instance, Request::ModelCapabilities).await? {
                Response::ModelCapabilities {
                    mode,
                    provider,
                    context_window,
                    thinking,
                } => Ok(VmCapabilities {
                    mode,
                    provider,
                    context_window,
                    thinking,
                }),
                _ => bail!("Unexpected VM model capabilities response"),
            }
        });
    }

    fn chat_ui(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        self.chat_heading(ui);
        let enabled = self.ready && !self.busy;
        let archived = self.archived_chat();
        let picker_enabled = self.archived.is_some();
        if picker_enabled
            && self.composer.picker_query(enabled && !archived).is_some()
            && matches!(self.composer.picker_catalog, Lookup::Idle)
        {
            let instance = self.instance.clone();
            self.composer
                .picker_catalog
                .start(&self.runtime, &context, async move {
                    let response = match query_vm_api(
                        instance.clone(),
                        Request::ListSkillsAndMcpTools,
                    )
                    .await
                    {
                        Err(error)
                            if error
                                .to_string()
                                .contains("unknown variant `list_skills_and_mcp_tools`") =>
                        {
                            query_vm_api(instance, Request::PickerCatalog).await?
                        }
                        result => result?,
                    };
                    match response {
                        Response::SkillsAndMcpTools {
                            skills,
                            tools,
                            warnings,
                        }
                        | Response::PickerCatalog {
                            skills,
                            tools,
                            warnings,
                        } => Ok(PickerCatalog {
                            skills,
                            tools,
                            warnings,
                        }),
                        _ => bail!("Unexpected VM skill and tool catalog response"),
                    }
                });
        }
        if self
            .composer
            .panel(ui, &[], enabled, archived, picker_enabled)
        {
            self.send(&context);
        }
        let decision = &mut self.decision;
        let warning = self.warning.as_deref();
        let status = &self.status;
        let busy = self.busy;
        chat_transcript(
            ui,
            &self.chat_id,
            &self.history,
            &mut self.markdown,
            &self.chat_warnings,
            &self.loaded_skills,
            self.ready,
            |ui| {
                if let Some(warning) = warning {
                    ui.colored_label(ui.visuals().warn_fg_color, warning);
                }
                if let Some(retry) = decision.as_ref().filter(|decision| decision.retry) {
                    if let Some(accept) = super::model_retry_ui(ui, &retry.request.description)
                        && let Some(decision) = decision.take()
                    {
                        let _ = decision.reply.send((accept, None));
                    }
                } else if let Some(tool) = &self.tool_progress {
                    super::tool_progress_ui(ui, tool);
                } else if busy && !status.is_empty() {
                    super::chat_progress(ui, status);
                }
            },
        );
    }

    fn chat_heading(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            chat_title(ui, self.chat_id.as_deref());
            let mut refresh = false;
            match &self.capabilities {
                Lookup::Loading(_) => {
                    ui.weak("Checking model capabilities…");
                }
                Lookup::Complete(Ok(caps)) => {
                    ui.weak(caps.mode.label());
                    if caps.provider.is_some() && caps.provider != Some(Provider::OpenAiCompatible)
                    {
                        match &caps.thinking {
                            Ok(levels) if !levels.is_empty() => {
                                thinking_picker(ui, levels, &mut self.thinking, !self.busy)
                            }
                            result => {
                                let explanation = match result {
                                    Ok(_) => "The server did not report thinking levels.",
                                    Err(error) => error,
                                };
                                refresh = ui
                                    .small_button("Refresh thinking levels")
                                    .on_hover_text(explanation)
                                    .clicked();
                            }
                        }
                    }
                }
                Lookup::Complete(Err(error)) => {
                    refresh = ui
                        .small_button("Refresh model capabilities")
                        .on_hover_text(format!("{error:#}"))
                        .clicked();
                }
                Lookup::Idle => {}
            }
            if refresh && self.ready && !self.busy {
                self.load_capabilities(ui.ctx());
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Reconnect"))
                .on_hover_text(&self.instance)
                .clicked()
            {
                self.resources = Lookup::Idle;
                self.reconnect(ui.ctx());
            }
        });
        let capabilities = match &self.capabilities {
            Lookup::Complete(Ok(caps)) => (caps.provider, caps.context_window),
            _ => (None, None),
        };
        let (provider, window) = self
            .switched_model
            .map(|(provider, window)| (Some(provider), window))
            .unwrap_or(capabilities);
        context_usage(
            ui,
            provider != Some(Provider::LlamaCpp),
            window,
            self.context_usage,
        );
    }

    fn chat_update(&mut self, update: ChatUpdate) {
        match update {
            ChatUpdate::Tool { activity, phase } => {
                self.tool_progress = Some(ToolProgress {
                    activity,
                    phase,
                    started: Instant::now(),
                });
            }
            ChatUpdate::Skill { name } => {
                if let Some(index) = self.history.messages().len().checked_sub(1) {
                    self.loaded_skills.entry(index).or_default().insert(name);
                }
            }
            ChatUpdate::Usage { tokens } => self.context_usage = tokens,
            ChatUpdate::ModelSwitch {
                provider,
                context_window,
            } => {
                self.context_usage = None;
                self.switched_model = Some((provider, context_window));
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        self.picker
            .show(ui, &self.runtime, self.busy, Some(&self.instance));
        ui.heading(format!("{} settings", self.instance));
        self.runtime_ui(ui);
        let action = ui
            .add_enabled_ui(self.ready && !self.busy, |ui| {
                if let Some(form) = &mut self.settings_form {
                    form.show(
                        ui,
                        &self.runtime,
                        &self.instance,
                        &mut self.saved_approvals,
                        self.archived.is_some(),
                    )
                } else {
                    Ok(ui
                        .button("Reload settings")
                        .clicked()
                        .then_some(Request::VmSettings))
                }
            })
            .inner;
        match action {
            Ok(Some(request)) => {
                self.settings_status = None;
                self.request(request, ui.ctx());
            }
            Err(error) => self.warning = Some(format!("{error:#}")),
            Ok(None) => {}
        }
        if let Some(status) = &self.settings_status {
            ui.label(status);
        }
    }

    fn runtime_ui(&mut self, ui: &mut egui::Ui) {
        self.resources.poll();
        if !self.busy
            && matches!(&self.resources, Lookup::Complete(Ok(info)) if info.status == "Stopped")
        {
            self.ready = false;
        }
        if matches!(self.resources, Lookup::Idle) {
            self.resources
                .start(&self.runtime, ui.ctx(), vm_resources(self.instance.clone()));
        }
        ui.heading("VM runtime");
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            match &self.resources {
                Lookup::Complete(Ok(resources)) => {
                    ui.label(format!(
                        "{} · {} vCPUs · {} MiB allocated",
                        resources.status,
                        resources.cpus,
                        resources.memory_bytes / (1024 * 1024)
                    ));
                    for (status, label, power) in [
                        ("Stopped", "Start VM", VmPower::Start),
                        ("Running", "Stop VM", VmPower::Stop),
                        ("Running", "Restart VM", VmPower::Restart),
                    ] {
                        if resources.status == status
                            && ui
                                .add_enabled(!self.busy, egui::Button::new(label))
                                .clicked()
                        {
                            action = Some(power);
                        }
                    }
                }
                Lookup::Complete(Err(error)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("{error:#}"));
                }
                _ => {
                    ui.weak("Checking VM status…");
                }
            }
            if ui
                .add_enabled(
                    !self.busy && !matches!(self.resources, Lookup::Loading(_)),
                    egui::Button::new("Refresh status"),
                )
                .clicked()
            {
                self.resources = Lookup::Idle;
                ui.ctx().request_repaint();
            }
        });
        if let Some(action) = action {
            self.power(action, ui.ctx());
        }
        if (!self.busy || self.ready)
            && matches!(&self.resources, Lookup::Complete(Ok(info)) if info.status == "Running")
        {
            self.usage.show(ui, &self.runtime, &self.instance);
        } else {
            self.usage = VmUsageMonitor::default();
        }
        if matches!(&self.resources, Lookup::Complete(Ok(info)) if info.status == "Stopped") {
            ui.weak("Start the VM to edit its settings or chat.");
        }
        ui.separator();
    }

    fn show_status(&self, ui: &mut egui::Ui) {
        if let Some(warning) = &self.warning {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
        }
        if self.busy {
            ui.label(&self.status);
        }
    }
}

impl eframe::App for VmClient {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.composer.picker_catalog.poll();
        self.capabilities.poll();
        self.saved_approvals.poll();
        while let Ok(event) = self.events.try_recv() {
            match event {
                ClientEvent::Response(Response::History {
                    history,
                    chat_id,
                    chats,
                    archived,
                }) => {
                    if let Some(chats) = chats {
                        self.chats = Some(chats);
                    }
                    if let Some(archived) = archived {
                        self.archived = Some(archived);
                    }
                    self.pending_send = None;
                    if self.chat_id != chat_id {
                        self.reset_chat();
                    }
                    self.chat_id = chat_id;
                    self.history = history;
                    self.ready = true;
                }
                ClientEvent::Response(Response::VmSettings { settings }) => {
                    if let Some(form) = &mut self.settings_form {
                        form.replace_settings(*settings);
                    } else {
                        self.settings_form = Some(VmSettingsForm::new(*settings));
                    }
                    self.capabilities = Lookup::Idle;
                    self.thinking = None;
                    self.context_usage = None;
                    self.switched_model = None;
                    self.composer.picker_catalog = Lookup::Idle;
                    self.settings_status = Some("Settings are up to date.".into());
                }
                ClientEvent::Response(Response::Skill { name, skill }) => {
                    if let Some(form) = &mut self.settings_form {
                        form.add_skill(name, skill);
                    }
                }
                ClientEvent::Response(Response::SavedApprovals { rules }) => {
                    self.saved_approvals = Lookup::Complete(Ok(rules));
                    self.settings_status = Some("Approval removed".into());
                }
                ClientEvent::Response(Response::Status { message, update }) => {
                    if !message.is_empty() {
                        self.status = message;
                        self.tool_progress = None;
                    }
                    if let Some(update) = update {
                        self.chat_update(update);
                    }
                }
                ClientEvent::Response(Response::Warning { message, update }) => {
                    self.chat_warnings.push(message);
                    if let Some(update) = update {
                        self.chat_update(update);
                    }
                }
                ClientEvent::Response(_) => {}
                ClientEvent::Decision(decision) => {
                    if decision.retry {
                        self.page = VmPage::Chat;
                        self.tool_progress = None;
                    }
                    self.decision = Some(decision);
                }
                ClientEvent::Finished(result) => {
                    self.busy = false;
                    self.decision = None;
                    self.tool_progress = None;
                    self.status.clear();
                    if let Err(error) = result {
                        if let Some((prompt, selection)) = self.pending_send.take() {
                            self.composer.prompt = prompt;
                            self.composer.selection = selection;
                        }
                        self.warning = Some(format!("{error:#}"));
                    }
                }
                ClientEvent::PowerFinished(action, result) => {
                    self.busy = false;
                    self.status.clear();
                    self.resources = Lookup::Idle;
                    match result {
                        Ok(()) if matches!(action, VmPower::Stop) => {}
                        Ok(()) => self.reconnect(context),
                        Err(error) => self.warning = Some(format!("{error:#}")),
                    }
                }
            }
        }
        super::navigation_panel(context).show(context, |ui| {
            super::desktop_brand(ui, self.logo.as_ref());
            ui.weak(format!("Pluto VM · {}", self.instance));
            ui.add_space(20.0);
            if ui
                .selectable_label(self.page == VmPage::Chat, "Chat")
                .clicked()
            {
                self.page = VmPage::Chat;
            }
            if ui
                .selectable_label(self.page == VmPage::Files, "Files")
                .clicked()
            {
                self.page = VmPage::Files;
            }
            if ui
                .selectable_label(self.page == VmPage::Settings, "Settings")
                .clicked()
                && self.page != VmPage::Settings
            {
                self.picker.refresh();
                self.resources = Lookup::Idle;
                self.page = VmPage::Settings;
            }
            self.chat_navigation(ui);
            if let Some(form) = &self.settings_form {
                let (skills, servers) = form.tool_counts();
                super::tool_counts_ui(ui, skills, servers);
            }
        });
        if self.page == VmPage::Settings {
            if self.ready && !self.busy && !self.settings_load_attempted {
                self.request(Request::VmSettings, context);
            }
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::central_panel(&context.style())
                        .inner_margin(egui::Margin::same(24)),
                )
                .show(context, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.show_status(ui);
                        self.settings_ui(ui);
                    });
                });
            self.approval(context);
            return;
        }
        self.usage = VmUsageMonitor::default();
        if self.page == VmPage::Files {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::central_panel(&context.style())
                        .inner_margin(egui::Margin::same(24)),
                )
                .show(context, |ui| {
                    self.files.show(ui, &self.runtime, &self.instance)
                });
            self.approval(context);
            return;
        }
        if self.ready
            && !self.busy
            && self.archived.is_some()
            && matches!(self.capabilities, Lookup::Idle)
        {
            self.load_capabilities(context);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&context.style()).inner_margin(egui::Margin::same(24)),
            )
            .show(context, |ui| {
                self.chat_ui(ui);
            });
        self.approval(context);
    }
}

async fn exchange(
    instance: &str,
    request: Request,
    emit: &impl Fn(ClientEvent) -> std::result::Result<(), mpsc::SendError<ClientEvent>>,
) -> Result<()> {
    let mut command = lima_command()?;
    command.arg("shell");
    if !matches!(
        &request,
        Request::ListModels { .. }
            | Request::CheckFallbackModel { .. }
            | Request::TestMcp { .. }
            | Request::ListSkillsAndMcpTools
            | Request::PickerCatalog
            | Request::ModelCapabilities
            | Request::SavedApprovals
    ) {
        command.arg("--start");
    }
    let mut child = command
        .args([
            "--tty=false",
            "--workdir=/",
            instance,
            "sudo",
            "-n",
            "-u",
            "pluto-api",
            "/usr/local/bin/pluto",
            "--vm-connect",
            "/run/pluto/api.sock",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("Could not launch limactl. Install Lima and set up Pluto VM.")?;
    let mut input = child.stdin.take().context("Missing VM input pipe")?;
    let mut output = BufReader::new(child.stdout.take().context("Missing VM output pipe")?);
    write_message(&mut input, &request).await?;
    while let Some(response) = read_message::<Response>(&mut output, 64 * 1024 * 1024).await? {
        let (id, description, rule, error, retry) = match response {
            Response::Approval {
                id,
                description,
                rule,
                error,
            } => (id, description, rule, error, false),
            Response::Retry { id, description } => (id, description, None, None, true),
            Response::Complete => return Ok(()),
            Response::Error { message } => {
                bail!("{}", vm_request_error(instance, &request, message))
            }
            response => {
                emit(ClientEvent::Response(response))
                    .map_err(|_| anyhow::anyhow!("Client closed"))?;
                continue;
            }
        };
        let (reply, decision) = oneshot::channel();
        emit(ClientEvent::Decision(Decision {
            request: ApprovalRequest { description, rule },
            retry,
            error,
            allow_other_operands: false,
            reply,
        }))
        .map_err(|_| anyhow::anyhow!("Client closed"))?;
        tokio::select! {
            decision = decision => {
                let (accept, remember) = decision?;
                write_message(&mut input, &Request::Decision { id, accept, remember }).await?;
            }
            status = child.wait() => bail!("Connection closed while waiting for a decision ({})", status?),
        }
    }
    bail!("Disconnected from Pluto VM. Check its status and reconnect.")
}

pub(super) async fn query_vm_api(instance: String, request: Request) -> Result<Response> {
    let (sender, events) = mpsc::channel();
    exchange(&instance, request, &|event| sender.send(event)).await?;
    match events
        .try_recv()
        .context("The VM did not return a response")?
    {
        ClientEvent::Response(response) => Ok(response),
        _ => bail!("Unexpected response to a VM API query"),
    }
}

fn vm_request_error(instance: &str, request: &Request, message: String) -> String {
    if matches!(request, Request::VmSettings) && message.contains("unknown variant `vm_settings`") {
        return format!(
            "{instance} needs a Pluto service update for Skills and MCP settings. From the updated source directory, run: PLUTO_INSTANCE={instance} pluto/vm/vm install. Then click Reload settings."
        );
    }
    if matches!(request, Request::TestMcp { .. }) && message.contains("unknown variant `test_mcp`")
    {
        return format!(
            "Update the Pluto service in {instance} to test MCP connections. From the updated source directory, run: PLUTO_INSTANCE={instance} pluto/vm/vm install."
        );
    }
    let unknown_query = match request {
        Request::ListModels { .. } => Some("unknown variant `list_models`"),
        Request::CheckFallbackModel { .. } => Some("unknown variant `check_fallback_model`"),
        _ => None,
    };
    if unknown_query.is_some_and(|variant| message.contains(variant)) {
        return format!(
            "Update the Pluto service in {instance} to use model discovery. From the updated source directory, run: PLUTO_INSTANCE={instance} pluto/vm/vm install."
        );
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_vm_settings_error_names_the_instance_and_update_command() {
        let message = vm_request_error(
            "plutoVM",
            &Request::VmSettings,
            "Invalid API message: unknown variant `vm_settings`, expected `model_settings`".into(),
        );
        assert!(message.contains("PLUTO_INSTANCE=plutoVM pluto/vm/vm install"));
        assert_eq!(
            vm_request_error(
                "plutoVM",
                &Request::VmSettings,
                "A chat turn is already active".into()
            ),
            "A chat turn is already active"
        );
    }

    #[test]
    fn vm_usage_uses_cpu_deltas_and_available_guest_memory() -> Result<()> {
        let usage = VmUsage::parse("10 0 10 60 20 0 0 0\n20 0 20 130 30 0 0 0\n524288 393216\n")?;
        assert_eq!(usage.cpu_percent, 20);
        assert_eq!(usage.total_kib, 524288);
        assert_eq!(usage.used_kib, 131072);
        assert!(VmUsage::parse("1 2 3").is_err());
        Ok(())
    }

    #[test]
    fn vm_runtime_shows_allocations_and_power_actions_for_its_status() -> Result<()> {
        let context = egui::Context::default();
        let mut client = VmClient::disconnected("test".into(), None)?;
        client.busy = true;
        for (status, actions) in [
            ("Running", &["Stop VM", "Restart VM"][..]),
            ("Stopped", &["Start VM"][..]),
        ] {
            client.resources =
                Lookup::Complete(Ok(VmResources::parse(&format!("{status} 2 536870912\n"))?));
            let output = context.run(Default::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| client.runtime_ui(ui));
            });
            let shown = |label: &str| {
                output.shapes.iter().any(|shape| {
                    matches!(
                        &shape.shape, egui::Shape::Text(text) if text.galley.text() == label
                    )
                })
            };
            assert!(shown(&format!("{status} · 2 vCPUs · 512 MiB allocated")));
            for label in ["Start VM", "Stop VM", "Restart VM"] {
                assert_eq!(shown(label), actions.contains(&label), "{label}");
            }
        }
        Ok(())
    }

    #[test]
    fn vm_chat_renders_shared_composer_and_archived_state() -> Result<()> {
        let context = egui::Context::default();
        let mut desktop = VmClient::disconnected("test".into(), None)?;
        desktop.ready = true;
        desktop.chat_id = Some("saved".into());
        desktop.archived = Some(Vec::new());
        desktop.composer.prompt = "/".into();
        desktop.composer.picker_catalog = Lookup::Complete(Ok(PickerCatalog {
            skills: vec!["report-skill".into()],
            tools: Vec::new(),
            warnings: Vec::new(),
        }));
        let render = |desktop: &mut VmClient| {
            context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| desktop.chat_ui(ui));
            })
        };
        let visible = |output: &egui::FullOutput, label| {
            output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
        })
        };
        let output = render(&mut desktop);
        assert!(visible(&output, "Copy chat ID"));
        assert!(visible(&output, "report-skill"));
        desktop.archived = Some(vec![ChatSummary {
            id: "saved".into(),
            title: "Earlier chat".into(),
        }]);
        let output = render(&mut desktop);
        assert!(visible(&output, "Archived chat · Restore it to continue"));
        assert!(!visible(&output, "Send"));
        Ok(())
    }

    #[test]
    fn vm_new_chat_clears_the_view_and_keeps_recent_chats() -> Result<()> {
        let context = egui::Context::default();
        let mut desktop = VmClient::disconnected("test".into(), None)?;
        desktop.ready = true;
        desktop.chat_id = Some("saved".into());
        desktop.chats = Some(vec![ChatSummary {
            id: "saved".into(),
            title: "Earlier chat".into(),
        }]);
        desktop.history.push("user", "Earlier chat".into());

        let mut render = |events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| desktop.chat_navigation(ui));
                },
            )
        };
        let output = render(Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "New chat" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .context("New chat button is missing")?;
        let button = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![
            egui::Event::PointerMoved(position),
            button(true),
            button(false),
        ]);
        assert!(desktop.chat_id.is_none());
        assert!(desktop.history.messages().is_empty());
        assert_eq!(
            desktop.chats.as_ref().context("Recent chats disappeared")?[0].id,
            "saved"
        );
        Ok(())
    }

    #[test]
    fn vm_picker_switches_connection_in_settings() -> Result<()> {
        let context = egui::Context::default();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let mut picker = VmPicker {
            instances: Lookup::Complete(Ok(vec!["pluto-work".into(), "pluto-personal".into()])),
            ..Default::default()
        };

        for (button_index, current, expected) in [
            (1, None, Some("pluto-personal".into())),
            (0, Some("pluto-personal"), None),
        ] {
            let mut render = |events| {
                context.run(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            picker.show(ui, &runtime, false, current);
                        });
                    },
                )
            };
            render(Vec::new());
            let output = render(Vec::new());
            assert!(output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "Current"
            )));
            let position = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Connect" => {
                        Some(text.pos + text.galley.size() / 2.0)
                    }
                    _ => None,
                })
                .nth(button_index)
                .context("Connection button is missing")?;
            let button = |pressed| egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(vec![
                egui::Event::PointerMoved(position),
                button(true),
                button(false),
            ]);
            assert_eq!(picker.selection.take(), Some(expected));
        }
        Ok(())
    }

    #[test]
    fn vm_picker_loads_automatically_and_links_to_setup_when_empty() -> Result<()> {
        let context = egui::Context::default();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let mut picker = VmPicker::default();
        let frame = |picker: &mut VmPicker| {
            context.run(Default::default(), |context| {
                egui::CentralPanel::default()
                    .show(context, |ui| picker.show(ui, &runtime, false, None));
            })
        };

        frame(&mut picker);
        assert!(matches!(picker.instances, Lookup::Loading(_)));
        picker.instances = Lookup::Complete(Ok(Vec::new()));
        let output = frame(&mut picker);
        for label in [
            "Standalone · Unsandboxed",
            "Current",
            "No Pluto VMs found.",
            "Set up Pluto VM",
        ] {
            assert!(
                output.shapes.iter().any(|shape| matches!(
                    &shape.shape, egui::Shape::Text(text) if text.galley.text() == label
                )),
                "Missing {label}"
            );
        }
        Ok(())
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn vm_picker_opens_creation_and_refreshes_after_success() -> Result<()> {
        let context = egui::Context::default();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let mut picker = VmPicker {
            instances: Lookup::Complete(Ok(vec!["pluto-work".into()])),
            ..Default::default()
        };
        let render = |picker: &mut VmPicker, events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        picker.show(ui, &runtime, false, None);
                    });
                },
            )
        };
        let button_position = |frame: &egui::FullOutput, label: &str| {
            frame.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
        };
        let click = |picker: &mut VmPicker, position| {
            let button = |pressed| egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(
                picker,
                vec![
                    egui::Event::PointerMoved(position),
                    button(true),
                    button(false),
                ],
            );
        };

        render(&mut picker, Vec::new());
        let position = button_position(&render(&mut picker, Vec::new()), "New VM")
            .context("New VM button is missing")?;
        click(&mut picker, position);
        assert_eq!(picker.new_vm.as_deref(), Some(""));

        picker.new_vm = Some("Pluto-Work".into());
        render(&mut picker, Vec::new());
        let output = render(&mut picker, Vec::new());
        assert!(button_position(&output, "A VM with that name already exists.").is_some());
        let position =
            button_position(&output, "Create VM").context("Create VM button is missing")?;
        click(&mut picker, position);
        assert!(matches!(picker.creation, Lookup::Idle));

        picker.new_vm = Some("pluto-personal".into());
        picker.creation = Lookup::Complete(Ok(()));
        render(&mut picker, Vec::new());
        assert!(picker.new_vm.is_none());
        assert!(matches!(picker.instances, Lookup::Loading(_)));
        Ok(())
    }

    #[test]
    fn vm_approval_dialog_sends_the_clicked_decision() -> Result<()> {
        for (label, accept) in [("Run once", true), ("Decline", false)] {
            let context = egui::Context::default();
            let (reply, mut decision) = oneshot::channel();
            let mut desktop = VmClient::disconnected("test".into(), None)?;
            desktop.busy = true;
            desktop.decision = Some(Decision {
                request: ApprovalRequest {
                    description: "Run awk on the imported CSV".into(),
                    rule: None,
                },
                retry: false,
                error: None,
                allow_other_operands: false,
                reply,
            });
            let mut render = |events| {
                context.run(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |context| {
                        desktop.approval(context);
                    },
                )
            };
            render(Vec::new());
            let output = render(Vec::new());
            let position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + text.galley.size() / 2.0)
                    }
                    _ => None,
                })
                .context("Approval button is missing")?;
            let button = |pressed| egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(vec![
                egui::Event::PointerMoved(position),
                button(true),
                button(false),
            ]);
            assert_eq!(decision.try_recv()?.0, accept);
            assert!(desktop.decision.is_none());
        }
        Ok(())
    }
}
