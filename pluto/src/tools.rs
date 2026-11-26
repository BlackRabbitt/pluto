use crate::providers::{Connection, Provider};
use anyhow::{Context, Result, bail, ensure};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParam, Tool},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub models: ModelSettings,
    pub model_fallbacks: crate::model_fallback::FallbackSettings,
    pub workspace: PathBuf,
    pub auto_approve_tools: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminal_approvals: Vec<TerminalApproval>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_approvals: Vec<McpApproval>,
    pub skills: BTreeMap<String, Skill>,
    pub mcp: BTreeMap<String, Mcp>,
    #[serde(skip)]
    pub vm_tools: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMode {
    #[default]
    LocalOnly,
    OpenAiCompatible,
}

impl ModelMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalOnly => "Local only",
            Self::OpenAiCompatible => "OpenAI-compatible",
        }
    }
}

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelSettings {
    pub mode: ModelMode,
    pub local: ModelProfile,
    pub openai: ModelProfile,
}

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelProfile {
    pub provider: Option<Provider>,
    pub url: String,
    pub name: String,
    pub token_env: String,
    pub context_limit: u32,
}

impl ModelSettings {
    pub fn profile(&self, mode: ModelMode) -> &ModelProfile {
        match mode {
            ModelMode::LocalOnly => &self.local,
            ModelMode::OpenAiCompatible => &self.openai,
        }
    }

    pub fn connection(&self, mode: ModelMode) -> Result<Connection> {
        self.profile(mode).connection(mode)
    }
}

impl ModelProfile {
    pub fn connection(&self, mode: ModelMode) -> Result<Connection> {
        let provider = match mode {
            ModelMode::LocalOnly => match self.provider {
                Some(provider @ (Provider::LlamaCpp | Provider::Ollama)) => provider,
                _ => anyhow::bail!("Choose llama.cpp or Ollama for the local model"),
            },
            ModelMode::OpenAiCompatible => Provider::OpenAiCompatible,
        };
        Connection::new(provider, &self.url, &self.token_env)
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Skill {
    pub path: PathBuf,
    pub description: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct VmSettings {
    pub models: ModelSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_fallbacks: Option<crate::model_fallback::FallbackSettings>,
    pub skills: BTreeMap<String, Skill>,
    pub mcp: BTreeMap<String, Mcp>,
    pub auto_approve_tools: bool,
}

impl From<Config> for VmSettings {
    fn from(config: Config) -> Self {
        Self {
            models: config.models,
            model_fallbacks: Some(config.model_fallbacks),
            skills: config.skills,
            mcp: config.mcp,
            auto_approve_tools: config.auto_approve_tools,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalApproval {
    workspace: PathBuf,
    command: String,
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operand_count: Option<usize>,
}

#[derive(Deserialize)]
struct TerminalInvocation {
    command: String,
    args: Vec<String>,
}

pub struct ApprovalRequest {
    pub description: String,
    pub rule: Option<ApprovalRule>,
}

#[derive(Clone, Deserialize, Serialize)]
pub enum ApprovalRule {
    Terminal(TerminalApproval),
    Mcp(McpApproval),
}

impl ApprovalRule {
    pub fn label(&self) -> String {
        match self {
            Self::Terminal(rule) => rule.label(),
            Self::Mcp(rule) => format!("MCP · {} / {}", rule.server, rule.tool),
        }
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpApproval {
    server: String,
    tool: String,
    connection: Mcp,
}

impl TerminalApproval {
    fn for_invocation(workspace: &Path, invocation: TerminalInvocation) -> Option<Self> {
        Some(Self {
            workspace: workspace.canonicalize().ok()?,
            command: invocation.command,
            args: invocation.args,
            operand_count: None,
        })
    }

    pub fn allow_other_operands(&self) -> Option<Self> {
        let separator = self.args.iter().position(|arg| arg == "--")?;
        let prefix_len = separator + 1;
        let operand_count = self.args.len() - prefix_len;
        if separator == 0 || operand_count == 0 {
            return None;
        }
        Some(Self {
            args: self.args[..prefix_len].to_vec(),
            operand_count: Some(operand_count),
            ..self.clone()
        })
    }

    pub fn matches(&self, request: &Self) -> bool {
        request.operand_count.is_none()
            && self.workspace == request.workspace
            && self.command == request.command
            && match self.operand_count {
                Some(count) => {
                    self.args.len() > 1
                        && self.args.last().is_some_and(|arg| arg == "--")
                        && count > 0
                        && request.args.starts_with(&self.args)
                        && request.args.len() - self.args.len() == count
                }
                None => self.args == request.args,
            }
    }

    pub fn label(&self) -> String {
        let args = serde_json::to_string(&self.args).unwrap_or_default();
        let scope = match self.operand_count {
            Some(count) => format!("{args} followed by {count} argument(s)"),
            None => args,
        };
        format!("{} {scope}\n{}", self.command, self.workspace.display())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum Mcp {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
        #[serde(default)]
        token_env: String,
    },
}

impl Default for Config {
    fn default() -> Self {
        Self {
            models: ModelSettings::default(),
            model_fallbacks: Default::default(),
            workspace: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            auto_approve_tools: false,
            terminal_approvals: Vec::new(),
            mcp_approvals: Vec::new(),
            skills: BTreeMap::new(),
            mcp: BTreeMap::new(),
            vm_tools: false,
        }
    }
}

#[derive(Deserialize)]
struct SkillHeader {
    name: String,
    description: String,
}

impl Config {
    pub fn permits(&self, rule: &ApprovalRule) -> bool {
        match rule {
            ApprovalRule::Terminal(request) => self
                .terminal_approvals
                .iter()
                .any(|saved| saved.matches(request)),
            ApprovalRule::Mcp(request) => {
                self.mcp.get(&request.server) == Some(&request.connection)
                    && self.mcp_approvals.contains(request)
            }
        }
    }

    pub fn remember_approval(&mut self, rule: ApprovalRule) {
        match rule {
            ApprovalRule::Terminal(rule) => {
                if !self.terminal_approvals.contains(&rule) {
                    self.terminal_approvals.push(rule);
                }
            }
            ApprovalRule::Mcp(rule) => {
                if !self.mcp_approvals.contains(&rule) {
                    self.mcp_approvals.push(rule);
                }
            }
        }
    }

    pub fn forget_approval(&mut self, rule: &ApprovalRule) {
        match rule {
            ApprovalRule::Terminal(rule) => self.terminal_approvals.retain(|saved| saved != rule),
            ApprovalRule::Mcp(rule) => self.mcp_approvals.retain(|saved| saved != rule),
        }
    }

    pub fn set_mcp_server(&mut self, name: &str, server: Option<Mcp>) -> Result<()> {
        if let Some(server) = server {
            ensure!(
                valid_name(name),
                "Use letters, numbers, hyphens, or underscores for the server name"
            );
            server.validate()?;
            self.mcp.insert(name.to_owned(), server);
        } else {
            self.mcp.remove(name);
        }
        self.mcp_approvals
            .retain(|rule| rule.server != name || self.mcp.get(name) == Some(&rule.connection));
        Ok(())
    }

    pub fn model_connection(&self) -> Result<Connection> {
        self.models.connection(self.models.mode)
    }

    pub fn path() -> Result<PathBuf> {
        if let Some(path) = env::var_os("PLUTO_CONFIG").or_else(|| env::var_os("HARNESS_CONFIG")) {
            return Ok(path.into());
        }
        let root = dirs::config_dir().context("Could not find the user config directory")?;
        Ok(Self::app_data_dir(&root)?.join("tools.toml"))
    }

    fn app_data_dir(root: &Path) -> Result<PathBuf> {
        let directory = root.join(env!("CARGO_PKG_NAME"));
        if !directory.try_exists()? {
            let legacy = root.join("personal-harness");
            match fs::rename(legacy, &directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error)
                    if directory.is_dir() && error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).context("Could not move app data to Pluto"),
            }
        }
        Ok(directory)
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        Self::read_settings(path)?
            .try_into()
            .with_context(|| format!("Invalid {}", path.display()))
    }

    fn read_settings(path: &Path) -> Result<toml::Table> {
        let mut table = match fs::read_to_string(path) {
            Ok(text) => toml::from_str::<toml::Table>(&text)
                .with_context(|| format!("Invalid {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(toml::Table::new());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        };
        if !table.contains_key("models") {
            let mut profile: ModelProfile = [
                ("model_provider", "provider"),
                ("model_url", "url"),
                ("model_name", "name"),
                ("model_token_env", "token_env"),
                ("model_context_limit", "context_limit"),
            ]
            .into_iter()
            .filter_map(|(old, new)| table.remove(old).map(|value| (new.into(), value)))
            .collect::<toml::Table>()
            .try_into()?;
            let mut models = ModelSettings::default();
            if profile.provider == Some(Provider::OpenAiCompatible)
                || (profile.provider.is_none() && !profile.url.trim().is_empty())
            {
                profile.provider = None;
                models.mode = ModelMode::OpenAiCompatible;
                models.openai = profile;
            } else {
                models.local = profile;
            }
            table.insert("models".into(), toml::Value::try_from(models)?);
        }
        Ok(table)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path()?)
    }

    pub fn save_vm_models(
        path: &Path,
        previous: &ModelSettings,
        models: &ModelSettings,
    ) -> Result<()> {
        let mut table = Self::read_settings(path)?;
        let current: Self = table.clone().try_into()?;
        let current = current.models;
        ensure!(
            &current == previous,
            "Model settings changed. Reload them before saving."
        );
        models.connection(models.mode)?;
        ensure!(
            !models.profile(models.mode).name.trim().is_empty(),
            "Enter a model name"
        );
        table.insert("models".into(), toml::Value::try_from(models)?);
        Self::write_settings(path, &toml::to_string_pretty(&table)?)
    }

    pub fn save_vm_settings(
        path: &Path,
        previous: &VmSettings,
        settings: &VmSettings,
    ) -> Result<()> {
        let mut table = Self::read_settings(path)?;
        let current: Self = table.clone().try_into()?;
        let mut current = VmSettings::from(current);
        if previous.model_fallbacks.is_none() && settings.model_fallbacks.is_none() {
            current.model_fallbacks = None;
        }
        ensure!(
            current == *previous,
            "VM settings changed. Reload them before saving."
        );
        if settings.models != previous.models {
            settings.models.connection(settings.models.mode)?;
            ensure!(
                !settings
                    .models
                    .profile(settings.models.mode)
                    .name
                    .trim()
                    .is_empty(),
                "Enter a model name"
            );
            table.insert("models".into(), toml::Value::try_from(&settings.models)?);
        }
        if settings.model_fallbacks != previous.model_fallbacks {
            let fallbacks = settings
                .model_fallbacks
                .as_ref()
                .context("Missing model fallbacks")?;
            fallbacks.validate()?;
            table.insert("model_fallbacks".into(), toml::Value::try_from(fallbacks)?);
        }
        if settings.skills != previous.skills {
            for (name, skill) in &settings.skills {
                if previous.skills.get(name) != Some(skill) {
                    let (registered, details) = Self::inspect_skill(&skill.path)?;
                    ensure!(
                        &registered == name && &details == skill,
                        "Skill {name} changed. Add it again before saving."
                    );
                }
            }
            table.insert("skills".into(), toml::Value::try_from(&settings.skills)?);
        }
        if settings.mcp != previous.mcp {
            for (name, server) in &settings.mcp {
                if previous.mcp.get(name) != Some(server) {
                    ensure!(valid_name(name), "Enter a valid MCP server name");
                    match server {
                        Mcp::Stdio { command, .. } => {
                            ensure!(!command.trim().is_empty(), "Enter a command for {name}")
                        }
                        Mcp::Http { url, .. } => {
                            let url = reqwest::Url::parse(url)
                                .with_context(|| format!("Enter a valid URL for {name}"))?;
                            ensure!(
                                matches!(url.scheme(), "http" | "https")
                                    && url.host_str().is_some(),
                                "Use an HTTP or HTTPS URL for {name}"
                            );
                        }
                    }
                }
            }
            table.insert("mcp".into(), toml::Value::try_from(&settings.mcp)?);
        }
        if settings.auto_approve_tools != previous.auto_approve_tools {
            table.insert(
                "auto_approve_tools".into(),
                settings.auto_approve_tools.into(),
            );
        }
        Self::write_settings(path, &toml::to_string_pretty(&table)?)
    }

    pub fn update_saved_approvals(path: &Path, edit: impl FnOnce(&mut Self)) -> Result<()> {
        let mut table = Self::read_settings(path)?;
        let mut config: Self = table.clone().try_into()?;
        edit(&mut config);
        table.insert(
            "terminal_approvals".into(),
            toml::Value::try_from(&config.terminal_approvals)?,
        );
        table.insert(
            "mcp_approvals".into(),
            toml::Value::try_from(&config.mcp_approvals)?,
        );
        Self::write_settings(path, &toml::to_string_pretty(&table)?)
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        Self::write_settings(path, &toml::to_string_pretty(self)?)
    }

    fn write_settings(path: &Path, text: &str) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    pub fn add_skill(&mut self, path: &Path) -> Result<String> {
        let (name, skill) = Self::inspect_skill(path)?;
        self.skills.insert(name.clone(), skill);
        Ok(name)
    }

    pub fn inspect_skill(path: &Path) -> Result<(String, Skill)> {
        let path = if path.is_dir() {
            path.join("SKILL.md")
        } else {
            path.to_owned()
        };
        let path = path
            .canonicalize()
            .context("Could not find the skill file")?;
        ensure!(
            path.file_name().is_some_and(|name| name == "SKILL.md"),
            "Select a skill folder or SKILL.md"
        );
        ensure!(path.is_file(), "SKILL.md must be a regular file");
        let text = fs::read_to_string(&path)?;
        let header = Self::skill_header(&text)?;
        Ok((
            header.name,
            Skill {
                path,
                description: header.description,
            },
        ))
    }

    fn skill_header(text: &str) -> Result<SkillHeader> {
        let mut lines = text.lines();
        ensure!(
            lines.next() == Some("---"),
            "SKILL.md needs YAML front matter"
        );
        let front_matter = lines
            .by_ref()
            .take_while(|line| *line != "---")
            .collect::<Vec<_>>()
            .join("\n");
        ensure!(
            lines.next().is_some(),
            "SKILL.md needs a closing front matter delimiter and instructions"
        );
        let header: SkillHeader =
            yaml_serde::from_str(&front_matter).context("SKILL.md needs name and description")?;
        ensure!(
            valid_name(&header.name) && !header.description.trim().is_empty(),
            "Use a letter, number, hyphen or underscore for the skill name and provide a description"
        );
        Ok(header)
    }

    pub fn import_vm_skill(directory: &Path, text: &str) -> Result<(String, Skill)> {
        let header = Self::skill_header(text)?;
        let directory = directory.join(&header.name);
        fs::create_dir_all(&directory)?;
        let path = directory.join("SKILL.md");
        match fs::read_to_string(&path) {
            Ok(existing) => ensure!(
                existing == text,
                "A different imported skill named {} already exists",
                header.name
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::write_settings(&path, text)?
            }
            Err(error) => return Err(error.into()),
        }
        Self::inspect_skill(&path)
    }

    pub fn skill_catalog(&self) -> String {
        self.skills
            .iter()
            .map(|(name, skill)| format!("{name} — {}", skill.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

impl Mcp {
    async fn discover(
        &self,
        workspace: Option<&Path>,
    ) -> Result<(RunningService<RoleClient, ()>, Vec<Tool>)> {
        timeout(Duration::from_secs(12), async {
            let client = self.connect(workspace).await?;
            let tools = client.list_all_tools().await?;
            Ok((client, tools))
        })
        .await
        .context("MCP connection or tool discovery timed out")?
    }

    pub async fn test_connection(&self) -> Result<Vec<String>> {
        self.test_connection_at(None).await
    }

    pub async fn test_vm_connection(&self, workspace: &Path) -> Result<Vec<String>> {
        self.test_connection_at(Some(workspace)).await
    }

    async fn test_connection_at(&self, workspace: Option<&Path>) -> Result<Vec<String>> {
        let (_client, tools) = self.discover(workspace).await?;
        Ok(tools
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect())
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Stdio { command, .. } => {
                ensure!(!command.trim().is_empty(), "Enter a launch command")
            }
            Self::Http { url, token_env } => {
                let parsed = reqwest::Url::parse(url).context("Enter a valid MCP URL")?;
                ensure!(
                    parsed.scheme() == "https"
                        || (parsed.scheme() == "http"
                            && matches!(
                                parsed.host_str(),
                                Some("localhost" | "127.0.0.1" | "[::1]")
                            )),
                    "Use HTTPS for a remote MCP server or HTTP on localhost"
                );
                ensure!(
                    parsed.username().is_empty() && parsed.password().is_none(),
                    "Use a token environment variable for credentials"
                );
                ensure!(
                    token_env.is_empty()
                        || token_env
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_'),
                    "Enter an environment variable name"
                );
            }
        }
        Ok(())
    }

    async fn connect(&self, workspace: Option<&Path>) -> Result<RunningService<RoleClient, ()>> {
        self.validate()?;
        match self {
            Self::Stdio { command, args } => {
                let mut process = match workspace {
                    Some(workspace) => vm_command(command, workspace),
                    None => Command::new(command),
                };
                process.args(args);
                let transport = TokioChildProcess::new(process).context(
                    "Could not start the local MCP process. Check its launch command and PATH",
                )?;
                Ok(().serve(transport).await?)
            }
            Self::Http { url, token_env } => {
                let mut options = StreamableHttpClientTransportConfig::with_uri(url.as_str());
                if !token_env.is_empty() {
                    options = options.auth_header(
                        env::var(token_env)
                            .with_context(|| format!("Set {token_env} before launching Pluto"))?,
                    );
                }
                let transport = StreamableHttpClientTransport::from_config(options);
                Ok(().serve(transport).await?)
            }
        }
    }
}

struct RemoteTool {
    server: usize,
    server_name: String,
    name: String,
    target: String,
    activity: ToolActivity,
    definition: Value,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ToolActivity {
    pub name: String,
    pub action: Option<String>,
}

impl std::fmt::Display for ToolActivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        if let Some(action) = &self.action {
            write!(f, " · {action}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct McpToolId {
    pub server: String,
    pub name: String,
}

pub struct ToolSet<'a> {
    config: &'a Config,
    servers: Vec<RunningService<RoleClient, ()>>,
    remote: BTreeMap<String, RemoteTool>,
}

const EAGER_MCP_TOOL_LIMIT: usize = 3;
const EAGER_MCP_SCHEMA_BYTES: usize = 8_192;

fn search_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn function(name: &str, description: &str, parameters: Value) -> Value {
    json!({"type":"function","function":{
        "name":name,"description":description,"parameters":parameters
    }})
}

fn activity_text(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(100) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

fn terminal_activity(args: &Value) -> ToolActivity {
    let command = args["command"]
        .as_str()
        .and_then(|path| Path::new(path).file_name()?.to_str())
        .unwrap_or("Terminal command");
    let mut arguments = args["args"]
        .as_array()
        .into_iter()
        .flatten()
        .map(Value::as_str);
    let first = arguments.next().flatten();
    let script = match command.split('.').next() {
        Some("python" | "python3" | "node" | "ruby" | "perl") => first
            .filter(|arg| !arg.starts_with('-'))
            .and_then(|arg| Path::new(arg).file_name()?.to_str()),
        _ => None,
    };
    // Show familiar subcommands without copying arbitrary argument values.
    let operation = if script.is_some() {
        arguments.next().flatten()
    } else {
        first
    }
    .filter(|arg| {
        matches!(
            *arg,
            "add"
                | "check"
                | "create"
                | "edit"
                | "fetch"
                | "get"
                | "list"
                | "log"
                | "lookup"
                | "read"
                | "search"
                | "show"
                | "status"
                | "update"
        )
    });
    let name = script.map_or_else(
        || command.to_owned(),
        |script| format!("{command} {script}"),
    );
    ToolActivity {
        name: activity_text(&name),
        action: Some(operation.map_or_else(
            || "Run terminal command".into(),
            |operation| format!("Run {operation}"),
        )),
    }
}

impl<'a> ToolSet<'a> {
    pub async fn connect(config: &'a Config, mut warning: impl FnMut(String)) -> Self {
        let mut set = Self {
            config,
            servers: Vec::new(),
            remote: BTreeMap::new(),
        };
        let mut requests = tokio::task::JoinSet::new();
        let workspace = config.vm_tools.then_some(&config.workspace);
        for (name, entry) in &config.mcp {
            let (name, entry) = (name.clone(), entry.clone());
            let workspace = workspace.cloned();
            requests.spawn(async move { (name, entry.discover(workspace.as_deref()).await) });
        }
        let mut discovered = BTreeMap::new();
        while let Some(result) = requests.join_next().await {
            if let Ok((name, result)) = result {
                discovered.insert(name, result);
            }
        }
        for (name, result) in discovered {
            match result {
                Ok((client, tools)) => {
                    let server = set.servers.len();
                    set.servers.push(client);
                    for tool in tools {
                        let target = format!("{name} / {}", tool.name);
                        let purpose = [
                            tool.title.as_deref(),
                            tool.annotations
                                .as_ref()
                                .and_then(|hint| hint.title.as_deref()),
                            tool.description.as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .map(activity_text)
                        .find(|text| !text.is_empty());
                        let activity = ToolActivity {
                            name: activity_text(&target),
                            action: purpose,
                        };
                        let slug = format!("{name}_{}", tool.name)
                            .chars()
                            .map(|c| {
                                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                    c
                                } else {
                                    '_'
                                }
                            })
                            .take(50)
                            .collect::<String>();
                        let alias = format!("mcp_{}_{slug}", set.remote.len());
                        let description = format!(
                            "{target}: {}",
                            tool.description.as_deref().unwrap_or_default()
                        );
                        let definition = function(&alias, &description, json!(tool.input_schema));
                        set.remote.insert(
                            alias,
                            RemoteTool {
                                server,
                                server_name: name.clone(),
                                name: tool.name.into_owned(),
                                target,
                                activity,
                                definition,
                            },
                        );
                    }
                }
                Err(error) => warning(format!("{name}: {error:#}")),
            }
        }
        set
    }

    pub fn initial_mcp_tools(&self) -> BTreeSet<String> {
        if self.remote.len() <= EAGER_MCP_TOOL_LIMIT
            && self
                .remote
                .values()
                .map(|tool| tool.definition.to_string().len())
                .sum::<usize>()
                <= EAGER_MCP_SCHEMA_BYTES
        {
            self.remote.keys().cloned().collect()
        } else {
            BTreeSet::new()
        }
    }

    pub fn mcp_tool_choices(&self) -> Vec<McpToolId> {
        self.remote
            .values()
            .map(|tool| McpToolId {
                server: tool.server_name.clone(),
                name: tool.name.clone(),
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn mcp_alias(&self, selected: &McpToolId) -> Option<String> {
        self.remote.iter().find_map(|(alias, tool)| {
            (tool.server_name == selected.server && tool.name == selected.name)
                .then(|| alias.clone())
        })
    }

    pub fn definitions(&self, active_mcp_tools: &BTreeSet<String>) -> Vec<Value> {
        let mut tools = vec![function(
            "terminal",
            "Run an installed executable in the user's configured workspace after desktop approval. Pass CLI arguments separately. For shell pipelines, explicitly request a shell executable and pass its script as an argument.",
            json!({
                "type":"object","properties":{"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},
                "required":["command","args"],"additionalProperties":false
            }),
        )];
        if !self.config.skills.is_empty() {
            tools.push(function(
                "load_skill",
                "Read the instructions of an installed skill by its catalog name.",
                json!({
                    "type":"object","properties":{"name":{"type":"string"}},
                    "required":["name"],"additionalProperties":false
                }),
            ));
        }
        if !self.remote.is_empty() {
            tools.push(function(
                "find_tools",
                "Find the best MCP tool for one action or name. Optionally limit the search by configured server name. The best match becomes callable on the next model request. Search again if you need a follow-up tool.",
                json!({
                    "type":"object",
                    "properties":{
                        "query":{"type":"string","description":"The tool name or the action you need"},
                        "server":{"type":"string","description":"Optional MCP server name as configured in Pluto"}
                    },
                    "required":["query"],
                    "additionalProperties":false
                }),
            ));
        }
        tools.extend(
            active_mcp_tools
                .iter()
                .filter_map(|name| self.remote.get(name))
                .map(|tool| tool.definition.clone()),
        );
        tools
    }

    pub fn needs_discovery(&self, name: &str, active_mcp_tools: &BTreeSet<String>) -> bool {
        self.remote.contains_key(name) && !active_mcp_tools.contains(name)
    }

    pub fn find_tools(&self, args: Value, found: &mut BTreeSet<String>) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Query {
            query: String,
            server: Option<String>,
        }

        let query: Query = serde_json::from_value(args)?;
        let terms = query.query.to_lowercase();
        let terms: BTreeSet<_> = search_words(&terms).collect();
        ensure!(!terms.is_empty(), "Enter a tool name or an action to find");
        let server = query.server.as_deref().map(str::trim);
        let mut best = None;
        for (alias, tool) in &self.remote {
            if server.is_some_and(|name| !name.eq_ignore_ascii_case(&tool.server_name)) {
                continue;
            }
            let target = tool.target.to_lowercase();
            let description = tool.definition["function"]["description"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            let score = terms
                .iter()
                .map(|term| {
                    if search_words(&target).any(|word| word == *term) {
                        3
                    } else if search_words(&description).any(|word| word == *term) {
                        1
                    } else {
                        0
                    }
                })
                .sum::<usize>();
            if score > 0
                && best
                    .as_ref()
                    .is_none_or(|(best_score, _, _)| score > *best_score)
            {
                best = Some((score, alias, tool));
            }
        }
        let matches: Vec<_> = best
            .into_iter()
            .map(|(_, alias, tool)| {
                found.insert(alias.clone());
                json!({"name": alias, "server": tool.server_name, "summary": tool.activity.to_string()})
            })
            .collect();
        Ok(json!({"matches": matches}).to_string())
    }

    pub fn approval(&self, name: &str, args: &Value) -> Option<ApprovalRequest> {
        if name == "terminal" {
            let terminal = serde_json::from_value(args.clone())
                .ok()
                .and_then(|invocation| {
                    TerminalApproval::for_invocation(&self.config.workspace, invocation)
                });
            return Some(ApprovalRequest {
                description: format!(
                    "Run a terminal command in {}\n\n{}",
                    self.config.workspace.display(),
                    serde_json::to_string_pretty(args).unwrap_or_default()
                ),
                rule: terminal.map(ApprovalRule::Terminal),
            });
        }
        self.remote.get(name).map(|tool| ApprovalRequest {
            description: format!(
                "Call {}\n\n{}",
                tool.target,
                serde_json::to_string_pretty(args).unwrap_or_default()
            ),
            rule: self.config.mcp.get(&tool.server_name).map(|connection| {
                ApprovalRule::Mcp(McpApproval {
                    server: tool.server_name.clone(),
                    tool: tool.name.clone(),
                    connection: connection.clone(),
                })
            }),
        })
    }

    pub fn activity(&self, name: &str, args: &Value) -> ToolActivity {
        match name {
            "terminal" => terminal_activity(args),
            "find_tools" => ToolActivity {
                name: "MCP tool discovery".into(),
                action: Some("Find a matching tool".into()),
            },
            "load_skill" => ToolActivity {
                name: args["name"]
                    .as_str()
                    .filter(|name| valid_name(name))
                    .map_or_else(|| "Skill".into(), activity_text),
                action: Some("Load skill instructions".into()),
            },
            _ => self.remote.get(name).map_or_else(
                || ToolActivity {
                    name: activity_text(name),
                    action: None,
                },
                |tool| tool.activity.clone(),
            ),
        }
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<String> {
        match name {
            "terminal" => {
                let invocation: TerminalInvocation = serde_json::from_value(args)?;
                run_process(
                    &self.config.workspace,
                    self.config.vm_tools,
                    &invocation.command,
                    &invocation.args,
                )
                .await
            }
            "load_skill" => {
                let name = args["name"].as_str().context("Expected a skill name")?;
                let skill = self
                    .config
                    .skills
                    .get(name)
                    .context("Skill is not registered")?;
                fs::read_to_string(&skill.path).context("Could not read the skill file")
            }
            _ => {
                let Some(tool) = self.remote.get(name) else {
                    bail!("Unknown tool: {name}")
                };
                let request = CallToolRequestParam {
                    name: tool.name.clone().into(),
                    arguments: args.as_object().cloned(),
                };
                let result = timeout(
                    Duration::from_secs(60),
                    self.servers[tool.server].call_tool(request),
                )
                .await??;
                Ok(serde_json::to_string(&result)?)
            }
        }
    }
}

async fn read_output(mut stream: impl AsyncRead + Unpin) -> std::io::Result<String> {
    let mut chunk = [0; 4096];
    let mut output = Vec::new();
    let mut truncated = false;
    loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let keep = count.min(5_500usize.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
        truncated |= keep < count;
    }
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        text.push_str("\n[Output truncated]");
    }
    Ok(text)
}

fn vm_command(executable: &str, workspace: &Path) -> Command {
    let mut command = Command::new("/usr/bin/sudo");
    command.env_clear().current_dir(workspace).args([
        "-n",
        "-u",
        "pluto-tool",
        "--",
        "/usr/local/bin/pluto",
        "--vm-run-tool",
        executable,
    ]);
    command
}

async fn run_process(
    workspace: &Path,
    vm_tools: bool,
    executable: &str,
    args: &[String],
) -> Result<String> {
    let mut command = if vm_tools {
        vm_command(executable, workspace)
    } else {
        Command::new(executable)
    };
    command
        .args(args)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().with_context(|| format!("Could not run {executable:?} in {}. Check that the command is installed on PATH and the workspace exists", workspace.display()))?;
    let stdout = child.stdout.take().context("Could not capture stdout")?;
    let stderr = child.stderr.take().context("Could not capture stderr")?;
    let (status, stdout, stderr) = timeout(Duration::from_secs(30), async {
        tokio::try_join!(child.wait(), read_output(stdout), read_output(stderr))
    })
    .await
    .context("Terminal command timed out after 30 seconds")??;
    Ok(json!({"exit_code":status.code(),"stdout":stdout,"stderr":stderr}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_header_accepts_yaml_aliases_and_folded_descriptions() -> Result<()> {
        let header = Config::skill_header(
            r#"---
name: sample-skill
metadata:
  summary: &summary >-
    Format a report:
    preserve the source details.
description: *summary
---
Skill instructions.
"#,
        )?;
        assert_eq!(header.name, "sample-skill");
        assert_eq!(
            header.description,
            "Format a report: preserve the source details."
        );
        Ok(())
    }

    #[test]
    fn vm_settings_preserve_other_fields_and_reject_stale_edits() -> Result<()> {
        let directory =
            env::temp_dir().join(format!("pluto-vm-settings-test-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| -> Result<()> {
            let path = directory.join("tools.toml");
            fs::write(&path, "workspace = '/custom'\ncustom_value = 'preserve'\n")?;
            let initial = VmSettings::from(Config::load_from(&path)?);
            let mut edited = initial.clone();
            edited.auto_approve_tools = true;
            edited.mcp.insert(
                "local".into(),
                Mcp::Stdio {
                    command: "/bin/true".into(),
                    args: Vec::new(),
                },
            );
            Config::save_vm_settings(&path, &initial, &edited)?;
            let config = Config::load_from(&path)?;
            assert_eq!(config.workspace, Path::new("/custom"));
            assert!(VmSettings::from(config) == edited);
            let document = Config::read_settings(&path)?;
            assert_eq!(document["custom_value"].as_str(), Some("preserve"));
            assert!(Config::save_vm_settings(&path, &initial, &initial).is_err());
            let mut legacy = edited.clone();
            legacy.model_fallbacks = None;
            let mut legacy_update = legacy.clone();
            legacy_update.auto_approve_tools = false;
            Config::save_vm_settings(&path, &legacy, &legacy_update)?;
            assert!(
                VmSettings::from(Config::load_from(&path)?)
                    .model_fallbacks
                    .is_some()
            );
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn global_auto_approval_defaults_off_and_round_trips() -> Result<()> {
        assert!(!toml::from_str::<Config>("")?.auto_approve_tools);
        for enabled in [true, false] {
            let config = Config {
                auto_approve_tools: enabled,
                ..Config::default()
            };
            assert_eq!(
                toml::from_str::<Config>(&toml::to_string(&config)?)?.auto_approve_tools,
                enabled
            );
        }
        Ok(())
    }

    #[test]
    fn model_settings_migrate_legacy_profiles() -> Result<()> {
        let directory =
            env::temp_dir().join(format!("pluto-model-settings-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let path = directory.join("tools.toml");
        let result = (|| -> Result<()> {
            for (legacy_provider, mode) in [
                ("model_provider = 'llama_cpp'\n", ModelMode::LocalOnly),
                (
                    "model_provider = 'open_ai_compatible'\n",
                    ModelMode::OpenAiCompatible,
                ),
                ("", ModelMode::OpenAiCompatible),
            ] {
                fs::write(
                    &path,
                    format!(
                        "{legacy_provider}model_url = 'http://localhost:8080/v1'\nmodel_name = 'existing'\nmodel_context_limit = 32768\n"
                    ),
                )?;
                let mut config = Config::load_from(&path)?;
                assert_eq!(config.models.mode, mode);
                assert_eq!(config.models.profile(mode).name, "existing");
                assert_eq!(config.models.profile(mode).context_limit, 32768);
                if mode == ModelMode::OpenAiCompatible {
                    config.models.local.provider = Some(Provider::Ollama);
                    config.models.local.url = "http://localhost:11434".into();
                    config.models.local.name = "local".into();
                    config.models.mode = ModelMode::LocalOnly;
                    config.save_to(&path)?;
                    let saved = Config::load_from(&path)?;
                    assert_eq!(saved.models.mode, ModelMode::LocalOnly);
                    assert_eq!(saved.model_connection()?.provider, Provider::Ollama);
                    assert_eq!(saved.models.openai.name, "existing");
                }
            }
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn mcp_changes_require_an_explicit_save() -> Result<()> {
        let directory =
            env::temp_dir().join(format!("pluto-mcp-settings-test-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let path = directory.join("tools.toml");
        let result = (|| -> Result<()> {
            let mut config = Config::default();
            config.models.local.name = "Existing model".into();
            config.save_to(&path)?;
            let server = Mcp::Stdio {
                command: "python3".into(),
                args: vec!["fixture.py".into(), "stdio".into()],
            };
            config.set_mcp_server("fixture", Some(server.clone()))?;
            assert!(Config::load_from(&path)?.mcp.is_empty());
            config.save_to(&path)?;
            let mut reopened = Config::load_from(&path)?;
            assert_eq!(reopened.models.local.name, "Existing model");
            assert!(reopened.mcp.get("fixture") == Some(&server));

            let approval = ApprovalRule::Mcp(McpApproval {
                server: "fixture".into(),
                tool: "lookup".into(),
                connection: server,
            });
            reopened.remember_approval(approval.clone());
            reopened.save_to(&path)?;
            assert!(Config::load_from(&path)?.permits(&approval));

            reopened.set_mcp_server("fixture", None)?;
            assert!(Config::load_from(&path)?.permits(&approval));
            reopened.save_to(&path)?;
            let reopened = Config::load_from(&path)?;
            assert!(reopened.mcp.is_empty());
            assert!(reopened.mcp_approvals.is_empty());
            assert_eq!(reopened.models.local.name, "Existing model");
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn saved_mcp_approval_is_specific_to_the_tool_and_server() -> Result<()> {
        let server = Mcp::Http {
            url: "http://127.0.0.1:8080/mcp".into(),
            token_env: String::new(),
        };
        let approval = McpApproval {
            server: "first".into(),
            tool: "lookup".into(),
            connection: server.clone(),
        };
        let mut config = Config::default();
        config.mcp.insert("first".into(), server.clone());
        config.mcp.insert("second".into(), server);
        let approved = ApprovalRule::Mcp(approval.clone());
        config.remember_approval(approved.clone());
        let mut reopened: Config = toml::from_str(&toml::to_string(&config)?)?;
        assert!(reopened.permits(&approved));
        assert!(!reopened.permits(&ApprovalRule::Mcp(McpApproval {
            server: "second".into(),
            ..approval.clone()
        })));
        assert!(!reopened.permits(&ApprovalRule::Mcp(McpApproval {
            tool: "write".into(),
            ..approval
        })));
        reopened.forget_approval(&approved);
        assert!(!reopened.permits(&approved));

        config.mcp.insert(
            "first".into(),
            Mcp::Http {
                url: "http://127.0.0.1:9000/mcp".into(),
                token_env: String::new(),
            },
        );
        assert!(!config.permits(&approved));
        Ok(())
    }

    #[test]
    fn remembered_cli_approvals_match_only_the_saved_scope() -> Result<()> {
        let first = TerminalApproval {
            workspace: "/workspace".into(),
            command: "python3".into(),
            args: ["notes.py", "search", "--", "milk"]
                .map(str::to_owned)
                .to_vec(),
            operand_count: None,
        };
        let mut next = first.clone();
        next.args[3] = "coffee".into();
        let similar = first
            .allow_other_operands()
            .context("Missing operand scope")?;
        let config = Config {
            terminal_approvals: vec![first.clone(), similar],
            ..Config::default()
        };
        let reopened: Config = toml::from_str(&toml::to_string(&config)?)?;
        let [exact, similar] = reopened.terminal_approvals.as_slice() else {
            bail!("Expected both approval rules");
        };
        assert!(exact.matches(&first));
        assert!(!exact.matches(&next));
        assert!(similar.matches(&next));

        next.args.insert(2, "--extra-option".into());
        assert!(!similar.matches(&next));
        next.args.remove(2);
        next.args.push("another operand".into());
        assert!(!similar.matches(&next));
        next.args.pop();
        next.workspace = "/other-workspace".into();
        assert!(!similar.matches(&next));
        next.workspace = first.workspace.clone();
        next.command = "other-executable".into();
        assert!(!similar.matches(&next));
        Ok(())
    }

    #[test]
    fn migrates_settings_and_history_without_overwriting_pluto_data() -> Result<()> {
        let root = env::temp_dir().join(format!("pluto-migration-test-{}", std::process::id()));
        let legacy = root.join("personal-harness");
        fs::create_dir_all(legacy.join("chats"))?;
        let result = (|| -> Result<()> {
            fs::write(legacy.join("tools.toml"), "model_name = \"test\"")?;
            fs::write(legacy.join("chats/chat.json"), "{}")?;

            let pluto = Config::app_data_dir(&root)?;
            assert_eq!(pluto, root.join("pluto"));
            assert_eq!(
                fs::read_to_string(pluto.join("tools.toml"))?,
                "model_name = \"test\""
            );
            assert_eq!(fs::read_to_string(pluto.join("chats/chat.json"))?, "{}");
            assert!(!legacy.exists());

            fs::create_dir(&legacy)?;
            fs::write(legacy.join("tools.toml"), "model_name = \"old\"")?;
            assert_eq!(Config::app_data_dir(&root)?, pluto);
            assert_eq!(
                fs::read_to_string(pluto.join("tools.toml"))?,
                "model_name = \"test\""
            );
            assert!(legacy.join("tools.toml").is_file());
            Ok(())
        })();
        fs::remove_dir_all(root)?;
        result
    }
}
