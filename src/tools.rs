use crate::providers::{Connection, Provider};
use anyhow::{Context, Result, bail, ensure};
use rmcp::{
    RoleClient, ServiceExt,
    model::CallToolRequestParam,
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
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
    pub model_provider: Option<Provider>,
    pub model_url: String,
    pub model_name: String,
    pub model_token_env: String,
    pub workspace: PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terminal_approvals: Vec<TerminalApproval>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_approvals: Vec<McpApproval>,
    pub skills: BTreeMap<String, Skill>,
    pub mcp: BTreeMap<String, Mcp>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Skill {
    pub path: PathBuf,
    pub description: String,
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

#[derive(Clone)]
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
            model_provider: None,
            model_url: String::new(),
            model_name: String::new(),
            model_token_env: String::new(),
            workspace: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            terminal_approvals: Vec::new(),
            mcp_approvals: Vec::new(),
            skills: BTreeMap::new(),
            mcp: BTreeMap::new(),
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

    pub fn model_connection(&self) -> Result<Connection> {
        Connection::new(
            self.model_provider.context("Choose a model provider")?,
            &self.model_url,
            &self.model_token_env,
        )
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
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => {
                let mut config: Self =
                    toml::from_str(&text).with_context(|| format!("Invalid {}", path.display()))?;
                if config.model_provider.is_none() && !config.model_url.trim().is_empty() {
                    config.model_provider = Some(Provider::OpenAiCompatible);
                }
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Could not read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    pub fn add_skill(&mut self, path: &Path) -> Result<String> {
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
        let text = fs::read_to_string(&path)?;
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
            serde_yaml::from_str(&front_matter).context("SKILL.md needs name and description")?;
        ensure!(
            valid_name(&header.name) && !header.description.trim().is_empty(),
            "Use a letter, number, hyphen or underscore for the skill name and provide a description"
        );
        self.skills.insert(
            header.name.clone(),
            Skill {
                path,
                description: header.description,
            },
        );
        Ok(header.name)
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

    async fn connect(&self) -> Result<RunningService<RoleClient, ()>> {
        self.validate()?;
        match self {
            Self::Stdio { command, args } => {
                let mut process = Command::new(command);
                process.args(args);
                Ok(().serve(TokioChildProcess::new(process)?).await?)
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
    activity: String,
    definition: Value,
}

pub struct ToolSet<'a> {
    config: &'a Config,
    servers: Vec<RunningService<RoleClient, ()>>,
    remote: BTreeMap<String, RemoteTool>,
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

fn terminal_activity(args: &Value) -> String {
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
    activity_text(
        &[Some(command), script, operation]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

impl<'a> ToolSet<'a> {
    pub async fn connect(config: &'a Config, warning: impl Fn(String)) -> Self {
        let mut set = Self {
            config,
            servers: Vec::new(),
            remote: BTreeMap::new(),
        };
        let mut requests = tokio::task::JoinSet::new();
        for (name, entry) in &config.mcp {
            let (name, entry) = (name.clone(), entry.clone());
            requests.spawn(async move {
                let result = timeout(Duration::from_secs(12), async {
                    let client = entry.connect().await?;
                    let tools = client.list_all_tools().await?;
                    Ok::<_, anyhow::Error>((client, tools))
                })
                .await
                .context("Discovery timed out")
                .and_then(|result| result);
                (name, result)
            });
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
                        let activity = match purpose {
                            Some(purpose) => format!("{} · {purpose}", activity_text(&target)),
                            None => activity_text(&target),
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

    pub fn definitions(&self) -> Vec<Value> {
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
        tools.extend(self.remote.values().map(|tool| tool.definition.clone()));
        tools
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

    pub fn activity(&self, name: &str, args: &Value) -> String {
        match name {
            "terminal" => terminal_activity(args),
            "load_skill" => match args["name"].as_str().filter(|name| valid_name(name)) {
                Some(name) => format!("Load skill {name}"),
                None => "Load skill".into(),
            },
            _ => self
                .remote
                .get(name)
                .map_or_else(|| activity_text(name), |tool| tool.activity.clone()),
        }
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<String> {
        match name {
            "terminal" => {
                let invocation: TerminalInvocation = serde_json::from_value(args)?;
                run_process(
                    &self.config.workspace,
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

async fn run_process(workspace: &Path, executable: &str, args: &[String]) -> Result<String> {
    let mut command = Command::new(executable);
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
