#![cfg(unix)]

use anyhow::{Context, Result};
use pluto::agent::{self, ApprovalDecision, Event, TokenUsage, ToolPhase, TurnSelection};
use pluto::diagnostics::Trace;
use pluto::model_fallback::{FallbackModel, FallbackSettings, ModelCooldowns};
use pluto::providers::{Connection, Provider, ThinkingSetting};
use pluto::tools::{
    ApprovalRule, Config, Mcp, McpToolId, ModelMode, ModelProfile, ModelSettings, ToolSet,
};
use serde_json::{Value, json};
use std::time::Duration;
use std::{fs, path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::Command;

fn model_settings(provider: Provider, url: String, name: String) -> ModelSettings {
    let profile = ModelProfile {
        provider: Some(provider),
        url,
        name,
        ..Default::default()
    };
    if provider == Provider::OpenAiCompatible {
        ModelSettings {
            mode: ModelMode::OpenAiCompatible,
            openai: profile,
            ..Default::default()
        }
    } else {
        ModelSettings {
            local: profile,
            ..Default::default()
        }
    }
}

async fn chat(
    config: Config,
    history: Vec<(String, String)>,
    thinking: Option<ThinkingSetting>,
    selection: TurnSelection,
    emit: impl Fn(Event),
) -> Result<String> {
    agent::chat(
        config,
        history,
        thinking,
        selection,
        &ModelCooldowns::default(),
        &Trace::default(),
        emit,
    )
    .await
}

async fn http_fixture(script: &Path) -> Result<(Child, u16)> {
    let mut http = Command::new("python3")
        .arg(script)
        .arg("http")
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let port_line = http.stdout.take().context("Fixture must report a port")?;
    let mut port = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(port_line).read_line(&mut port),
    )
    .await??;
    let port: u16 = port
        .trim()
        .parse()
        .context("Fixture did not report a port")?;
    Ok((http, port))
}

#[tokio::test]
async fn global_auto_approval_runs_and_tracks_terminal_and_mcp_calls() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let mut config = Config {
        models: model_settings(
            Provider::OpenAiCompatible,
            format!("http://127.0.0.1:{port}/v1"),
            "global-approval-fixture".into(),
        ),
        auto_approve_tools: true,
        ..Config::default()
    };
    config.set_mcp_server(
        "fixture",
        Some(Mcp::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token_env: String::new(),
        }),
    )?;
    let (sender, entries) = std::sync::mpsc::sync_channel(64);
    let trace = Trace::new(Some(sender), None);
    let activity = std::cell::RefCell::new(Vec::new());
    let answer = agent::chat(
        config,
        vec![("user".into(), "Run the fixture".into())],
        None,
        TurnSelection::default(),
        &ModelCooldowns::default(),
        &trace,
        |event| match event {
            Event::Approve(..) => panic!("Global auto-approval must not open a confirmation"),
            Event::Tool(tool) => activity.borrow_mut().push(tool),
            _ => {}
        },
    )
    .await?;
    assert_eq!(answer, "Global approval fixture passed");
    assert_eq!(
        activity
            .into_inner()
            .iter()
            .filter(|tool| tool.phase == ToolPhase::GloballyAutoApproved)
            .count(),
        2
    );
    let entries: Vec<Value> = entries
        .try_iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["event"] == "tool_approval_decided"
                && entry["details"]["source"] == "global_setting")
            .count(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn retry_model_call_after_503_reuses_tool_result() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let config = Config {
        models: model_settings(
            Provider::OpenAiCompatible,
            format!("http://127.0.0.1:{port}/v1"),
            "model-retry-fixture".into(),
        ),
        ..Config::default()
    };
    let (log, entries) = std::sync::mpsc::sync_channel(64);
    let trace = Trace::new(Some(log), None);
    let retries = std::cell::Cell::new(0);
    let answer = agent::chat(
        config,
        vec![("user".into(), "Run the fixture".into())],
        None,
        TurnSelection::default(),
        &ModelCooldowns::default(),
        &trace,
        |event| match event {
            Event::Approve(_, decision) => {
                assert!(decision.send(ApprovalDecision::RunOnce).is_ok());
            }
            Event::ModelRequestFailed(error, retry) => {
                assert!(error.contains("503"), "{error}");
                assert_eq!(retries.replace(1), 0, "Expected one retry prompt");
                retry.send(()).unwrap();
            }
            _ => {}
        },
    )
    .await?;
    assert_eq!(retries.get(), 1);
    assert_eq!(answer, "Reused the tool result");
    let entries: Vec<Value> = entries
        .try_iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["event"] == "tool_finished")
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn rate_limit_fallback_matches_metadata_and_reuses_tool_results() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_primary, primary_port) = http_fixture(&script).await?;
    let (_fallback, fallback_port) = http_fixture(&script).await?;
    let mut config = Config {
        models: model_settings(
            Provider::OpenAiCompatible,
            format!("http://127.0.0.1:{primary_port}/v1"),
            "rate-limited-fixture".into(),
        ),
        model_fallbacks: FallbackSettings {
            enabled: false,
            models: [
                "fallback-paid",
                "fallback-small",
                "fallback-no-tools",
                "fallback-ready",
            ]
            .into_iter()
            .map(|model| FallbackModel {
                url: format!("http://127.0.0.1:{fallback_port}/fallback/v1"),
                model: model.into(),
                free_open: true,
                ..Default::default()
            })
            .collect(),
        },
        ..Config::default()
    };
    for (enabled, eligible) in [(false, true), (true, false), (true, true)] {
        config.model_fallbacks.enabled = enabled;
        for model in &mut config.model_fallbacks.models {
            model.free_open = eligible;
        }
        let cooldowns = ModelCooldowns::default();
        let (log, entries) = std::sync::mpsc::sync_channel(128);
        let trace = Trace::new(Some(log), Some("1234-7-1"));
        let approvals = std::cell::Cell::new(0);
        let switches = std::cell::Cell::new(0);
        let result = agent::chat(
            config.clone(),
            vec![("user".into(), "Run the fixture".into())],
            Some(ThinkingSetting::Effort("high".into())),
            TurnSelection::default(),
            &cooldowns,
            &trace,
            |event| match event {
                Event::Approve(_, reply) => {
                    approvals.set(approvals.get() + 1);
                    let _ = reply.send(ApprovalDecision::RunOnce);
                }
                Event::ModelSwitched {
                    model,
                    context_window,
                    ..
                } => {
                    assert_eq!(model, "fallback-ready");
                    assert_eq!(context_window, Some(32768));
                    switches.set(switches.get() + 1);
                }
                _ => {}
            },
        )
        .await;
        assert_eq!(approvals.get(), 1);
        assert_eq!(switches.get(), usize::from(enabled && eligible));
        if enabled && eligible {
            assert_eq!(result?, "Reused the tool result");
        } else {
            assert!(format!("{:#}", result.unwrap_err()).contains("429"));
        }
        assert_eq!(cooldowns.active(&config.model_connection()?), enabled);
        let events: Vec<Value> = entries
            .try_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        assert!(events.iter().all(|event| event["chat_id"] == "1234-7-1"));
        let fallback_events: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event["event"].as_str(),
                    Some("model_fallback" | "model_fallback_unavailable")
                )
            })
            .collect();
        assert_eq!(fallback_events.len(), usize::from(enabled));
        if let Some(event) = fallback_events.first() {
            let details = &event["details"];
            assert_eq!(details["http_status"], 429);
            assert_eq!(details["retry_after_seconds"], 120);
            if eligible {
                assert_eq!(event["event"], "model_fallback");
                assert_eq!(details["from_model"], "rate-limited-fixture");
                assert_eq!(details["model"], "fallback-ready");
            } else {
                assert_eq!(event["event"], "model_fallback_unavailable");
                assert_eq!(details["model"], "rate-limited-fixture");
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn model_modes_use_the_expected_fallback_order() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_primary, primary_port) = http_fixture(&script).await?;
    let (_backup, backup_port) = http_fixture(&script).await?;
    let (_local, local_port) = http_fixture(&script).await?;
    let mut config = Config {
        models: ModelSettings {
            openai: ModelProfile {
                url: format!("http://127.0.0.1:{primary_port}/v1"),
                name: "rate-limited-fixture".into(),
                ..Default::default()
            },
            local: ModelProfile {
                provider: Some(Provider::LlamaCpp),
                url: format!("http://127.0.0.1:{local_port}"),
                name: "local-rate-limited".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model_fallbacks: FallbackSettings {
            enabled: true,
            models: vec![FallbackModel {
                url: format!("http://127.0.0.1:{backup_port}/fallback/v1"),
                model: "fallback-quota".into(),
                free_open: true,
                ..Default::default()
            }],
        },
        ..Default::default()
    };
    for (mode, backup_enabled) in [
        (ModelMode::LocalOnly, true),
        (ModelMode::OpenAiCompatible, true),
        (ModelMode::OpenAiCompatible, false),
    ] {
        config.models.mode = mode;
        config.model_fallbacks.enabled = backup_enabled;
        if mode == ModelMode::OpenAiCompatible {
            config.models.local.name = "local-resume".into();
        }
        let approvals = std::cell::Cell::new(0);
        let switches = std::cell::RefCell::new(Vec::new());
        let result = chat(
            config.clone(),
            vec![("user".into(), "Run the fixture".into())],
            None,
            TurnSelection::default(),
            |event| match event {
                Event::Approve(_, reply) => {
                    approvals.set(approvals.get() + 1);
                    let _ = reply.send(ApprovalDecision::RunOnce);
                }
                Event::ModelSwitched { model, .. } => switches.borrow_mut().push(model),
                _ => {}
            },
        )
        .await;
        assert_eq!(approvals.get(), 1);
        let switches = switches.into_inner();
        match (mode, backup_enabled) {
            (ModelMode::LocalOnly, _) => {
                assert!(result.unwrap_err().to_string().contains("429"));
                assert!(switches.is_empty());
            }
            (ModelMode::OpenAiCompatible, true) => {
                assert_eq!(result?, "Reused the tool result");
                assert_eq!(switches, ["fallback-quota", "local-resume"]);
            }
            (ModelMode::OpenAiCompatible, false) => {
                assert_eq!(result?, "Reused the tool result");
                assert_eq!(switches, ["local-resume"]);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn chat_enforces_the_tool_call_limit() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    for (model, expected_answer) in [
        ("tool-limit-fixture", "128 tool calls completed"),
        (
            "tool-limit-overflow-fixture",
            "Summary of the 128 completed requests",
        ),
        (
            "tool-limit-no-summary-fixture",
            "The model did not return a summary",
        ),
    ] {
        let config = Config {
            models: model_settings(
                Provider::OpenAiCompatible,
                format!("http://127.0.0.1:{port}/v1"),
                model.into(),
            ),
            ..Config::default()
        };
        let calls = std::cell::Cell::new(0);
        let (log, entries) = std::sync::mpsc::sync_channel(1024);
        let trace = Trace::new(Some(log), None);
        let result = agent::chat(
            config.clone(),
            vec![("user".into(), "Run the fixture".into())],
            None,
            TurnSelection::default(),
            &ModelCooldowns::default(),
            &trace,
            |event| match event {
                Event::Tool(tool)
                    if tool.activity.name == "MCP tool discovery"
                        && tool.phase == ToolPhase::Running =>
                {
                    calls.set(calls.get() + 1);
                }
                Event::Approve(..) => panic!("The skipped batch must not request approval"),
                _ => {}
            },
        )
        .await;
        assert_eq!(calls.get(), 128);
        let entries: Vec<Value> = entries
            .try_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["event"] == "tool_finished")
                .count(),
            128
        );
        assert!(
            entries
                .iter()
                .all(|entry| !entry.to_string().contains("Run the fixture"))
        );
        let answer = result?;
        assert!(answer.contains(expected_answer));
        if model != "tool-limit-fixture" {
            assert!(answer.contains("skipped the entire pending batch of 2 calls"));
            assert!(answer.contains("`find_tools`, `terminal`"));
            assert!(answer.contains("**Next step**"));
        }
        if model == "tool-limit-no-summary-fixture" {
            assert!(!answer.contains("pending-marker"));
        }
        if model == "tool-limit-overflow-fixture" {
            let follow_up = chat(
                config,
                vec![
                    ("user".into(), "Run the fixture".into()),
                    ("assistant".into(), answer),
                    ("user".into(), "Continue".into()),
                ],
                None,
                TurnSelection::default(),
                |event| {
                    if let Event::Approve(request, reply) = event {
                        assert!(request.description.contains("pending-marker"));
                        let _ = reply.send(ApprovalDecision::RunOnce);
                    }
                },
            )
            .await?;
            assert_eq!(follow_up, "Resumed from the handoff");
        }
        let final_requests: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry["event"] == "model_request" && entry["details"]["partial"] == true
            })
            .collect();
        assert_eq!(
            final_requests.len(),
            usize::from(model != "tool-limit-fixture")
        );
        assert!(
            final_requests
                .iter()
                .all(|entry| entry["details"]["tools_offered"] == 0)
        );
    }
    Ok(())
}

#[tokio::test]
async fn provider_metadata_controls_chat_reasoning() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    for (provider, model, expected_levels, selected) in [
        (
            Provider::LlamaCpp,
            "llama fixture",
            vec!["low", "high"],
            "high",
        ),
        (
            Provider::Ollama,
            "ollama-fixture",
            vec!["none", "low"],
            "low",
        ),
    ] {
        let config = Config {
            models: model_settings(
                provider,
                format!("http://127.0.0.1:{port}/v1/"),
                model.into(),
            ),
            ..Config::default()
        };
        let connection = config.model_connection()?;
        assert_eq!(connection.models(&client).await?, [model]);
        let capabilities = connection.capabilities(&client, model).await?;
        assert_eq!(
            capabilities.context_window,
            (provider == Provider::LlamaCpp).then_some(8192)
        );
        let levels = capabilities.thinking?;
        assert_eq!(
            levels
                .into_iter()
                .map(|level| level.setting)
                .collect::<Vec<_>>(),
            expected_levels
                .into_iter()
                .map(|level| ThinkingSetting::Effort(level.into()))
                .collect::<Vec<_>>(),
        );
        let last_usage = std::cell::Cell::new(None);
        let result = chat(
            config,
            Vec::new(),
            Some(ThinkingSetting::Effort(selected.into())),
            TurnSelection::default(),
            |event| {
                if let Event::Usage {
                    model: reported_model,
                    tokens,
                    ..
                } = event
                {
                    assert_eq!(reported_model, model);
                    last_usage.set(tokens);
                }
            },
        )
        .await?;
        assert_eq!(result, "Thinking fixture passed");
        assert_eq!(
            last_usage.get(),
            Some(TokenUsage {
                input: 120,
                output: 8
            })
        );
        assert_eq!(last_usage.get().map(TokenUsage::total), Some(128));
    }
    Ok(())
}

#[tokio::test]
async fn llama_template_can_expose_a_thinking_switch_without_named_levels() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let config = Config {
        models: model_settings(
            Provider::LlamaCpp,
            format!("http://127.0.0.1:{port}"),
            "toggle fixture".into(),
        ),
        ..Config::default()
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let connection = config.model_connection()?;
    let levels = connection
        .capabilities(&client, &config.models.profile(config.models.mode).name)
        .await?
        .thinking?;
    assert_eq!(levels.len(), 2);
    for (level, enabled) in levels.into_iter().zip([true, false]) {
        assert_eq!(level.setting, ThinkingSetting::Toggle(enabled));
        let result = chat(
            config.clone(),
            Vec::new(),
            Some(level.setting),
            TurnSelection::default(),
            |_| {},
        )
        .await?;
        assert_eq!(
            result,
            if enabled {
                "Thinking on"
            } else {
                "Thinking off"
            }
        );
    }
    assert!(
        connection
            .capabilities(&client, "fixed fixture")
            .await?
            .thinking?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn compatible_context_limits_come_from_the_selected_model() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let openai = Connection::new(
        Provider::OpenAiCompatible,
        &format!("{base_url}/compatible/v1"),
        "",
    )?;
    for (model, limit) in [
        ("fixture/model", Some(131_072)),
        ("fixture/zero", None),
        ("fixture/unknown", None),
        ("absent", None),
    ] {
        assert_eq!(
            openai.capabilities(&client, model).await?.context_window,
            limit
        );
    }
    Ok(())
}

#[tokio::test]
async fn pluto_loads_and_runs_the_test_notes_skill() -> Result<()> {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test-notes");
    let workspace = std::env::temp_dir().join(format!("pluto-notes-test-{}", std::process::id()));
    fs::create_dir(&workspace)?;
    let result: Result<()> = async {
        let relative_cli = Path::new("tests/test-notes/notes.py");
        let scratch_cli = workspace.join(relative_cli);
        fs::create_dir_all(workspace.join("tests/test-notes"))?;
        fs::copy(example.join("notes.py"), scratch_cli)?;
        let mut config = Config {
            models: model_settings(
                Provider::OpenAiCompatible,
                String::new(),
                "picked-skill-fixture".into(),
            ),
            workspace: workspace.clone(),
            ..Config::default()
        };
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
        let (_http, port) = http_fixture(&fixture).await?;
        config.models.openai.url = format!("http://127.0.0.1:{port}/v1");
        let name = config.add_skill(&example)?;
        assert_eq!(name, "test-notes");
        let summary_skill = workspace.join("summary");
        fs::create_dir(&summary_skill)?;
        fs::write(
            summary_skill.join("SKILL.md"),
            "---\nname: summary\ndescription: Summarize notes\n---\nUse a bullet list.",
        )?;
        let summary = config.add_skill(&summary_skill)?;
        let tools = ToolSet::connect(&config, |_| {}).await;
        let skill = tools.call("load_skill", json!({ "name": name })).await?;
        assert!(skill.contains("tests/test-notes/notes.py"));

        let args = json!({
            "command": "python3",
            "args": [relative_cli, "add", "--", "Shopping", "Buy milk"]
        });
        let activity = tools.activity("terminal", &args);
        assert_eq!(activity.name, "python3 notes.py");
        assert_eq!(activity.action.as_deref(), Some("Run add"));
        assert!(tools.approval("terminal", &args).is_some());
        let output: Value = serde_json::from_str(&tools.call("terminal", args).await?)?;
        assert_eq!(output["exit_code"], 0, "{output}");
        let note: Value =
            serde_json::from_str(output["stdout"].as_str().context("Missing stdout")?)?;
        assert_eq!(note, json!({"id": 1, "title": "Shopping"}));
        drop(tools);
        let approved = std::cell::Cell::new(false);
        let loaded_skills = std::cell::RefCell::new(Vec::new());
        let answer = chat(
            config,
            vec![("user".into(), "List my notes".into())],
            None,
            TurnSelection {
                skills: vec![name, summary],
                ..Default::default()
            },
            |event| match event {
                Event::SkillLoaded(name) => loaded_skills.borrow_mut().push(name),
                Event::Approve(request, reply) => {
                    assert_eq!(*loaded_skills.borrow(), ["test-notes", "summary"]);
                    assert!(request.description.contains("notes.py"));
                    approved.set(true);
                    let _ = reply.send(ApprovalDecision::RunOnce);
                }
                _ => {}
            },
        )
        .await?;
        assert!(approved.get());
        assert_eq!(answer, "Picked skill fixture passed");
        Ok(())
    }
    .await;
    fs::remove_dir_all(&workspace)?;
    result
}

#[tokio::test]
async fn find_tools_exposes_fixture_schemas_on_demand() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let other_server = Mcp::Http {
        url: format!("http://127.0.0.1:{port}/mcp"),
        token_env: String::new(),
    };
    let mut config = Config {
        models: model_settings(
            Provider::OpenAiCompatible,
            format!("http://127.0.0.1:{port}/v1"),
            "find-tools-fixture".into(),
        ),
        mcp: ["secondary", "extra-a", "extra-b"]
            .into_iter()
            .map(|name| (name.to_owned(), other_server.clone()))
            .collect(),
        ..Config::default()
    };
    config.mcp.insert(
        "fixture".into(),
        Mcp::Stdio {
            command: "python3".into(),
            args: vec![script.display().to_string(), "stdio".into()],
        },
    );
    let approvals = std::cell::Cell::new(0);
    let result = chat(
        config.clone(),
        vec![("user".into(), "Find and use the fixture lookup tool".into())],
        None,
        TurnSelection::default(),
        |event| match event {
            Event::Approve(request, reply) => {
                assert!(request.description.starts_with("Call fixture / lookup"));
                approvals.set(approvals.get() + 1);
                let _ = reply.send(ApprovalDecision::RunOnce);
            }
            Event::Warning(warning) => panic!("Unexpected discovery warning: {warning}"),
            _ => {}
        },
    )
    .await?;
    assert_eq!(result, "MCP discovery fixture passed");
    assert_eq!(approvals.get(), 1);

    let tools = ToolSet::connect(&config, |warning| {
        panic!("Unexpected discovery warning: {warning}")
    })
    .await;
    let choices = tools.mcp_tool_choices();
    let selected = ["fixture", "secondary"].map(|server| McpToolId {
        server: server.into(),
        name: "lookup".into(),
    });
    assert!(selected.iter().all(|tool| choices.contains(tool)));
    drop(tools);
    config.models.openai.name = "picked-mcp-fixture".into();
    let selected_approvals = std::cell::RefCell::new(Vec::new());
    let result = chat(
        config,
        vec![("user".into(), "Run the selected lookups".into())],
        None,
        TurnSelection {
            mcp: selected.into(),
            ..Default::default()
        },
        |event| {
            if let Event::Approve(request, reply) = event {
                selected_approvals.borrow_mut().push(
                    request
                        .description
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                );
                let _ = reply.send(ApprovalDecision::RunOnce);
            }
        },
    )
    .await?;
    assert_eq!(result, "Picked MCP fixture passed");
    assert_eq!(
        selected_approvals.into_inner(),
        ["Call fixture / lookup", "Call secondary / lookup"]
    );
    Ok(())
}

#[tokio::test]
async fn chat_loads_skill_runs_cli_and_calls_both_mcp_transports() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    let directory = std::env::temp_dir().join(format!("pluto-test-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let skill_file = directory.join("SKILL.md");
    fs::write(
        &skill_file,
        "---\nname: notes\ndescription: Notes for the fixture\n---\nUse printf to test the terminal.\n",
    )?;

    let mut config = Config {
        models: model_settings(
            Provider::OpenAiCompatible,
            format!("http://127.0.0.1:{port}/v1"),
            "fixture".into(),
        ),
        ..Config::default()
    };
    config.add_skill(&skill_file)?;
    config.mcp.insert(
        "local".into(),
        Mcp::Stdio {
            command: "python3".into(),
            args: vec![script.display().to_string(), "stdio".into()],
        },
    );
    config.mcp.insert(
        "remote".into(),
        Mcp::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            token_env: String::new(),
        },
    );
    for server in config.mcp.values() {
        assert_eq!(server.test_connection().await?, ["lookup"]);
    }

    let approvals = std::cell::RefCell::new(Vec::new());
    let activity = std::cell::RefCell::new(Vec::new());
    let loaded_skills = std::cell::RefCell::new(Vec::new());
    let remembered = std::cell::RefCell::new(config.clone());
    let result = chat(
        config,
        vec![("user".into(), "Run the fixture".into())],
        None,
        TurnSelection::default(),
        |event| match event {
            Event::Tool(tool) => activity.borrow_mut().push(tool),
            Event::SkillLoaded(name) => loaded_skills.borrow_mut().push(name),
            Event::Approve(request, reply) => {
                let decision = if request.description.contains("should be declined") {
                    ApprovalDecision::Decline
                } else if matches!(request.rule, Some(ApprovalRule::Terminal(_)))
                    || request
                        .rule
                        .as_ref()
                        .is_some_and(|rule| remembered.borrow().permits(rule))
                {
                    ApprovalDecision::AutoApproved
                } else {
                    if request.description.starts_with("Call local / lookup")
                        && let Some(rule @ ApprovalRule::Mcp(_)) = request.rule
                    {
                        remembered.borrow_mut().remember_approval(rule);
                    }
                    ApprovalDecision::RunOnce
                };
                let _ = reply.send(decision);
                approvals.borrow_mut().push(request.description);
            }
            Event::Warning(warning) => panic!("Unexpected discovery warning: {warning}"),
            _ => {}
        },
    )
    .await;
    fs::remove_dir_all(&directory)?;
    assert_eq!(result?, "Pluto fixture passed");
    assert_eq!(loaded_skills.into_inner(), ["notes"]);
    assert_eq!(approvals.into_inner().len(), 5);
    let activity = activity.into_inner();
    let saw = |phase, name, action| {
        activity.iter().any(|tool| {
            tool.phase == phase
                && tool.activity.name == name
                && tool.activity.action.as_deref() == Some(action)
        })
    };
    assert!(saw(
        ToolPhase::AutoApproved,
        "printf",
        "Run terminal command"
    ));
    assert!(saw(
        ToolPhase::AwaitingApproval,
        "local / lookup",
        "Look up the fixture"
    ));
    assert!(saw(
        ToolPhase::Running,
        "local / lookup",
        "Look up the fixture"
    ));
    assert!(saw(
        ToolPhase::AutoApproved,
        "local / lookup",
        "Look up the fixture"
    ));
    assert!(saw(
        ToolPhase::Running,
        "remote / lookup",
        "Look up the fixture"
    ));
    Ok(())
}
