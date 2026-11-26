use anyhow::{Context, Result, bail, ensure};
use pluto::{
    agent::{self, ApprovalDecision, Event, ModelPhase, ToolPhase, TurnSelection},
    api::{ApprovalScope, ChatSummary, ChatUpdate, Request, Response, read_message, write_message},
    diagnostics::{self, LogSender, Trace},
    history::{ChatEntry, ChatHistory, ChatStore},
    model_fallback::ModelCooldowns,
    providers::{Provider, ThinkingSetting},
    tools::{ApprovalRequest, ApprovalRule, Config, ToolSet},
};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File},
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
    time::timeout,
};

const MAX_REQUEST_BYTES: u64 = 256 * 1024;
const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const VM_WORKSPACE: &str = "/var/lib/pluto-workspace";

pub fn run_tool(executable: &OsStr, args: &[OsString]) -> Result<()> {
    let error = Command::new(executable)
        .args(args)
        .env_clear()
        .env("HOME", VM_WORKSPACE)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .exec();
    Err(error).with_context(|| format!("Could not run {executable:?} in Pluto VM"))
}

pub fn connect(socket: PathBuf) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let socket = UnixStream::connect(socket)
            .await
            .context("Could not reach Pluto’s API")?;
        let (mut from_service, mut to_service) = socket.into_split();
        let mut input = tokio::io::stdin();
        let mut output = tokio::io::stdout();
        let upstream = async {
            tokio::io::copy(&mut input, &mut to_service).await?;
            to_service.shutdown().await
        };
        let downstream = async {
            tokio::io::copy(&mut from_service, &mut output).await?;
            output.flush().await
        };
        tokio::pin!(upstream, downstream);
        tokio::select! {
            result = &mut upstream => {
                result?;
                downstream.await?;
            }
            result = &mut downstream => result?,
        }
        Ok(())
    });
    // The async stdin adapter can still be blocked waiting for the SSH client.
    runtime.shutdown_background();
    result
}

pub fn serve(state: PathBuf, socket: PathBuf) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(serve_api(state, socket))
}

async fn serve_api(state: PathBuf, socket: PathBuf) -> Result<()> {
    fs::create_dir_all(&state)?;
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.join("service.lock"))?;
    lock.try_lock()
        .context("This Pluto state directory is already in use")?;
    let listener = UnixListener::bind(&socket).or_else(|error| {
        if error.kind() != io::ErrorKind::AddrInUse {
            return Err(error);
        }
        if std::os::unix::net::UnixStream::connect(&socket)
            .is_err_and(|error| error.kind() == io::ErrorKind::ConnectionRefused)
        {
            fs::remove_file(&socket)?;
            return UnixListener::bind(&socket);
        }
        Err(error)
    })?;
    let log = diagnostics::start(state.join("diagnostics.jsonl"))?;
    let service = Arc::new(Service {
        settings: state.join("tools.toml"),
        cooldowns: ModelCooldowns::default(),
        discovery: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?,
        conversation: Mutex::new(()),
        log,
    });
    let connections = Arc::new(tokio::sync::Semaphore::new(8));
    loop {
        let (stream, _) = listener.accept().await?;
        let service = Arc::clone(&service);
        let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
            continue;
        };
        tokio::spawn(async move {
            let _permit = permit;
            service.handle(stream).await;
        });
    }
}

struct Service {
    settings: PathBuf,
    cooldowns: ModelCooldowns,
    discovery: reqwest::Client,
    conversation: Mutex<()>,
    log: LogSender,
}

fn chat_summaries(entries: Vec<ChatEntry>) -> Vec<ChatSummary> {
    entries
        .into_iter()
        .filter_map(|entry| {
            Some(ChatSummary {
                id: ChatStore::chat_id(&entry.path)?.to_owned(),
                title: entry.title,
            })
        })
        .collect()
}

impl Service {
    async fn handle(&self, stream: UnixStream) {
        let (input, mut output) = stream.into_split();
        let mut input = BufReader::new(input);
        let result = self.session(&mut input, &mut output).await;
        let response = match result {
            Ok(()) => Response::Complete,
            Err(error) => Response::Error {
                message: format!("{error:#}"),
            },
        };
        let _ = write_message(&mut output, &response).await;
    }

    async fn session(
        &self,
        input: &mut (impl AsyncBufRead + Unpin),
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let Some(request) = timeout(
            Duration::from_secs(10),
            read_message(input, MAX_REQUEST_BYTES),
        )
        .await??
        else {
            return Ok(());
        };
        let _guard = match &request {
            Request::ListModels { .. }
            | Request::CheckFallbackModel { .. }
            | Request::TestMcp { .. }
            | Request::ListSkillsAndMcpTools
            | Request::PickerCatalog
            | Request::ModelCapabilities
            | Request::SavedApprovals => None,
            _ => Some(
                self.conversation
                    .try_lock()
                    .context("A chat turn is already active")?,
            ),
        };
        let mut selection = None;
        let mut thinking = None;
        let store = ChatStore::beside(&self.settings);
        let (prompt, chat_id, use_latest) = match request {
            Request::History => (None, None, true),
            Request::OpenChat { chat_id } => (None, Some(chat_id), false),
            Request::Send { prompt } => (Some(prompt), None, true),
            Request::SendChat {
                chat_id,
                prompt,
                selection: tools,
                thinking: setting,
            } => {
                selection = tools;
                thinking = setting;
                (Some(prompt), chat_id, false)
            }
            Request::MoveChat { chat_id, archive } => {
                let (entries, _) = if archive {
                    store.entries()?
                } else {
                    store.archived_entries()?
                };
                let entry = entries
                    .iter()
                    .find(|entry| ChatStore::chat_id(&entry.path) == Some(&chat_id))
                    .context("Saved VM chat was not found")?;
                if archive {
                    store.archive(&entry.path)?;
                } else {
                    store.restore(&entry.path)?;
                }
                (None, Some(chat_id), false)
            }
            Request::ListSkillsAndMcpTools | Request::PickerCatalog => {
                let config = self.runtime_config()?;
                let mut warnings = Vec::new();
                let tools = ToolSet::connect(&config, |warning| warnings.push(warning))
                    .await
                    .mcp_tool_choices();
                let skills = config.skills.into_keys().collect();
                // Clients using the previous request name expect its original response tag.
                let response = if matches!(request, Request::PickerCatalog) {
                    Response::PickerCatalog {
                        skills,
                        tools,
                        warnings,
                    }
                } else {
                    Response::SkillsAndMcpTools {
                        skills,
                        tools,
                        warnings,
                    }
                };
                return write_message(output, &response).await;
            }
            Request::ModelCapabilities => {
                let config = Config::load_from(&self.settings)?;
                let mode = config.models.mode;
                let profile = config.models.profile(mode);
                let model = profile.name.trim();
                let connection = config.model_connection().ok();
                let (provider, mut context_window, thinking) =
                    match connection.filter(|_| !model.is_empty()) {
                        Some(connection) => {
                            let provider = Some(connection.provider);
                            match connection.capabilities(&self.discovery, model).await {
                                Ok(caps) => (
                                    provider,
                                    caps.context_window,
                                    caps.thinking.map_err(|error| format!("{error:#}")),
                                ),
                                Err(error) => (provider, None, Err(format!("{error:#}"))),
                            }
                        }
                        None => (None, None, Ok(Vec::new())),
                    };
                if provider != Some(Provider::LlamaCpp) && profile.context_limit > 0 {
                    context_window = Some(u64::from(profile.context_limit));
                }
                return write_message(
                    output,
                    &Response::ModelCapabilities {
                        mode,
                        provider,
                        context_window,
                        thinking,
                    },
                )
                .await;
            }
            Request::SavedApprovals | Request::ForgetApproval { .. } => {
                if let Request::ForgetApproval { rule } = request {
                    Config::update_saved_approvals(&self.settings, |config| {
                        config.forget_approval(&rule)
                    })?;
                }
                let config = Config::load_from(&self.settings)?;
                let rules = config
                    .terminal_approvals
                    .into_iter()
                    .map(ApprovalRule::Terminal)
                    .chain(config.mcp_approvals.into_iter().map(ApprovalRule::Mcp))
                    .collect();
                return write_message(output, &Response::SavedApprovals { rules }).await;
            }
            Request::Decision { .. } => bail!("No action is awaiting a decision"),
            Request::ModelSettings => {
                let models = Config::load_from(&self.settings)?.models;
                return write_message(output, &Response::ModelSettings { models }).await;
            }
            Request::SaveModelSettings { previous, models } => {
                Config::save_vm_models(&self.settings, &previous, &models)?;
                return write_message(output, &Response::ModelSettings { models: *models }).await;
            }
            Request::VmSettings => {
                let settings = Config::load_from(&self.settings)?.into();
                return write_message(
                    output,
                    &Response::VmSettings {
                        settings: Box::new(settings),
                    },
                )
                .await;
            }
            Request::SaveVmSettings { previous, settings } => {
                Config::save_vm_settings(&self.settings, &previous, &settings)?;
                return write_message(output, &Response::VmSettings { settings }).await;
            }
            Request::ListModels { mode, profile } => {
                let names = profile.connection(mode)?.models(&self.discovery).await?;
                return write_message(output, &Response::Models { names }).await;
            }
            Request::CheckFallbackModel { model } => {
                let metadata = model
                    .connection()?
                    .fallback_metadata(&self.discovery, model.model.trim())
                    .await?;
                return write_message(output, &Response::FallbackModelMetadata { metadata }).await;
            }
            Request::TestMcp { server } => {
                let names = server.test_vm_connection(Path::new(VM_WORKSPACE)).await?;
                return write_message(output, &Response::McpTools { names }).await;
            }
            Request::InspectSkill { path } => {
                let path = PathBuf::from(path);
                ensure!(path.is_absolute(), "Enter an absolute path inside Pluto VM");
                let (name, skill) = Config::inspect_skill(&path)?;
                return write_message(output, &Response::Skill { name, skill }).await;
            }
            Request::ImportSkill { text } => {
                let directory = self.settings.with_file_name("skills");
                let (name, skill) = Config::import_vm_skill(&directory, &text)?;
                return write_message(output, &Response::Skill { name, skill }).await;
            }
        };
        let (mut entries, _) = store.entries()?;
        let (archived, _) = store.archived_entries()?;
        let path = if let Some(id) = chat_id.as_deref() {
            Some(
                &entries
                    .iter()
                    .chain(&archived)
                    .find(|entry| ChatStore::chat_id(&entry.path) == Some(id))
                    .context("Saved VM chat was not found")?
                    .path,
            )
        } else if use_latest {
            entries.first().map(|entry| &entry.path)
        } else {
            None
        };
        let mut history = match path {
            Some(path) => store.read(path)?,
            None => ChatHistory::default(),
        };
        let Some(prompt) = prompt else {
            let chat_id = path
                .and_then(|path| ChatStore::chat_id(path))
                .map(str::to_owned);
            let chats = Some(chat_summaries(entries));
            return write_message(
                output,
                &Response::History {
                    history,
                    chat_id,
                    chats,
                    archived: Some(chat_summaries(archived)),
                },
            )
            .await;
        };
        ensure!(!prompt.trim().is_empty(), "Enter a message");
        ensure!(
            !path.is_some_and(|path| store.is_archived(path)),
            "Restore this chat before sending a message"
        );
        let config = self.runtime_config()?;
        config.model_connection()?;
        ensure!(
            !config
                .models
                .profile(config.models.mode)
                .name
                .trim()
                .is_empty(),
            "Set a model name in Pluto VM settings"
        );

        history.push("user", prompt);
        let entry = store.save(path.map(PathBuf::as_path), &history)?;
        let path = entry.path.clone();
        let chat_id = ChatStore::chat_id(&path).map(str::to_owned);
        entries.retain(|existing| existing.path != path);
        entries.insert(0, entry);
        let chats = Some(chat_summaries(entries));
        let messages = history.messages().to_vec();
        write_message(
            output,
            &Response::History {
                history,
                chat_id: chat_id.clone(),
                chats,
                archived: Some(chat_summaries(archived)),
            },
        )
        .await?;
        let trace = Trace::new(Some(self.log.clone()), ChatStore::chat_id(&path));
        let answer = timeout(
            TURN_TIMEOUT,
            self.run_turn(
                config,
                messages,
                selection.unwrap_or_default(),
                thinking,
                &trace,
                input,
                output,
            ),
        )
        .await
        .context("The chat turn exceeded 15 minutes")?;
        trace.record(
            "turn_finished",
            || serde_json::json!({"reply_returned": answer.is_ok()}),
        );
        let mut history = store.read(&path)?;
        history.push("assistant", answer?);
        store.save(Some(&path), &history)?;
        write_message(
            output,
            &Response::History {
                history,
                chat_id,
                chats: None,
                archived: None,
            },
        )
        .await
    }

    fn runtime_config(&self) -> Result<Config> {
        let mut config = Config::load_from(&self.settings)?;
        config.vm_tools = true;
        config.workspace = VM_WORKSPACE.into();
        Ok(config)
    }

    async fn run_turn(
        &self,
        config: Config,
        messages: Vec<(String, String)>,
        selection: TurnSelection,
        thinking: Option<ThinkingSetting>,
        trace: &Trace,
        input: &mut (impl AsyncBufRead + Unpin),
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<String> {
        let (events, mut incoming) = mpsc::unbounded_channel();
        let emit = |event| {
            let _ = events.send(event);
        };
        let turn = agent::chat(
            config.clone(),
            messages,
            thinking,
            selection,
            &self.cooldowns,
            trace,
            emit,
        );
        tokio::pin!(turn);
        let mut approvals = config;
        let mut decision = None;
        let mut next_id = 1;
        let mut finished = None;
        loop {
            let request = read_message::<Request>(input, MAX_REQUEST_BYTES);
            tokio::pin!(request);
            let request = loop {
                let event = if let Some(result) = finished.take() {
                    match incoming.try_recv() {
                        Ok(event) => {
                            finished = Some(result);
                            event
                        }
                        Err(_) => return result,
                    }
                } else {
                    tokio::select! {
                        request_result = &mut request => break request_result?,
                        event = incoming.recv() => event.context("Turn events closed unexpectedly")?,
                        result = &mut turn => {
                            finished = Some(result);
                            continue;
                        }
                    }
                };
                let response = match event {
                    Event::Approve(request, reply) => {
                        if request
                            .rule
                            .as_ref()
                            .is_some_and(|rule| approvals.permits(rule))
                        {
                            let _ = reply.send(ApprovalDecision::AutoApproved);
                            continue;
                        }
                        let id = next_id;
                        next_id += 1;
                        let response = approval_response(id, &request, None);
                        decision = Some((id, Pending::Tool(request, reply)));
                        response
                    }
                    Event::ModelRequestFailed(description, reply) => {
                        let id = next_id;
                        next_id += 1;
                        decision = Some((id, Pending::Retry(reply)));
                        Response::Retry { id, description }
                    }
                    Event::ModelRequest(phase) => {
                        let message = match phase {
                            ModelPhase::Thinking => "Thinking…",
                            ModelPhase::ReviewingToolResults => "Reviewing tool results…",
                            ModelPhase::WritingPartialAnswer => "Writing a partial answer…",
                        };
                        Response::Status {
                            message: message.into(),
                            update: None,
                        }
                    }
                    Event::Tool(tool) => {
                        let phase = match tool.phase {
                            ToolPhase::AwaitingApproval => "Awaiting approval",
                            ToolPhase::Running => "Running",
                            ToolPhase::AutoApproved | ToolPhase::GloballyAutoApproved => {
                                "Approved and running"
                            }
                        };
                        Response::Status {
                            message: format!("{phase} · {}", tool.activity),
                            update: Some(ChatUpdate::Tool {
                                activity: tool.activity,
                                phase: tool.phase,
                            }),
                        }
                    }
                    Event::SkillLoaded(name) => Response::Status {
                        message: format!("Loaded skill · {name}"),
                        update: Some(ChatUpdate::Skill { name }),
                    },
                    Event::Usage { tokens, .. } => Response::Status {
                        message: String::new(),
                        update: Some(ChatUpdate::Usage { tokens }),
                    },
                    Event::ModelSwitched {
                        model,
                        provider,
                        origin,
                        context_window,
                    } => Response::Warning {
                        message: format!(
                            "Switched to {model} via {} at {origin}",
                            provider.label()
                        ),
                        update: Some(ChatUpdate::ModelSwitch {
                            provider,
                            context_window,
                        }),
                    },
                    Event::Warning(message) => Response::Warning {
                        message,
                        update: None,
                    },
                    _ => continue,
                };
                write_message(output, &response).await?;
            };
            let Some(Request::Decision {
                id,
                accept,
                remember,
            }) = request
            else {
                bail!("Expected a decision from the connected client");
            };
            let Some((pending_id, pending)) = decision.take() else {
                bail!("No action is awaiting a decision");
            };
            ensure!(
                id == pending_id,
                "Decision does not match the pending action"
            );
            match pending {
                Pending::Tool(request, reply) => {
                    if let Some(scope) = remember.filter(|_| accept) {
                        let rule = match (scope, request.rule.as_ref()) {
                            (ApprovalScope::Exact, Some(rule)) => Some(rule.clone()),
                            (ApprovalScope::OtherOperands, Some(ApprovalRule::Terminal(rule))) => {
                                rule.allow_other_operands().map(ApprovalRule::Terminal)
                            }
                            _ => None,
                        };
                        let result = rule
                            .context("This tool call cannot be saved with that scope")
                            .and_then(|rule| {
                                Config::update_saved_approvals(&self.settings, |saved| {
                                    saved.remember_approval(rule.clone())
                                })?;
                                approvals.remember_approval(rule);
                                Ok(())
                            });
                        if let Err(error) = result {
                            write_message(
                                output,
                                &approval_response(
                                    id,
                                    &request,
                                    Some(format!("Could not save approval: {error:#}")),
                                ),
                            )
                            .await?;
                            decision = Some((id, Pending::Tool(request, reply)));
                            continue;
                        }
                    }
                    let _ = reply.send(if accept {
                        ApprovalDecision::RunOnce
                    } else {
                        ApprovalDecision::Decline
                    });
                }
                Pending::Retry(reply) if accept => {
                    let _ = reply.send(());
                }
                Pending::Retry(_) => {}
            }
        }
    }
}

fn approval_response(id: u64, request: &ApprovalRequest, error: Option<String>) -> Response {
    Response::Approval {
        id,
        description: request.description.clone(),
        rule: request.rule.clone(),
        error,
    }
}

enum Pending {
    Tool(ApprovalRequest, oneshot::Sender<ApprovalDecision>),
    Retry(oneshot::Sender<()>),
}
