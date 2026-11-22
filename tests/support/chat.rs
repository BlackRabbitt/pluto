use crate::agent::{self, ApprovalDecision, Event, TokenUsage, TurnSelection};
use crate::diagnostics::Trace;
use crate::providers::{Provider, ThinkingSetting};
use crate::tools::{ApprovalRule, Config, Mcp, McpToolId, ToolSet};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::time::Duration;
use std::{fs, path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::Command;

async fn chat(
    config: Config,
    history: Vec<(String, String)>,
    thinking: Option<ThinkingSetting>,
    selection: Option<TurnSelection>,
    emit: impl Fn(Event),
) -> Result<String> {
    agent::chat(
        config,
        history,
        thinking,
        selection,
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
async fn chat_allows_52_tool_calls_and_caps_additional_calls() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
    let (_http, port) = http_fixture(&script).await?;
    for (model, expected_answer) in [
        ("tool-limit-fixture", "52 tool calls completed"),
        (
            "tool-limit-overflow-fixture",
            "Summary of the 52 completed requests",
        ),
        (
            "tool-limit-no-summary-fixture",
            "The model did not return a summary",
        ),
    ] {
        let config = Config {
            model_provider: Some(Provider::OpenAiCompatible),
            model_url: format!("http://127.0.0.1:{port}/v1"),
            model_name: model.into(),
            ..Config::default()
        };
        let calls = std::cell::Cell::new(0);
        let (log, entries) = std::sync::mpsc::sync_channel(512);
        let trace = Trace::new(Some(log));
        let result = agent::chat(
            config,
            vec![("user".into(), "Run the fixture".into())],
            None,
            None,
            &trace,
            |event| {
                if matches!(event, Event::Status(status) if status == "Running · Find MCP tools") {
                    calls.set(calls.get() + 1);
                }
            },
        )
        .await;
        assert_eq!(calls.get(), 52);
        let entries: Vec<Value> = entries
            .try_iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry["event"] == "tool_finished")
                .count(),
            52
        );
        assert!(
            entries
                .iter()
                .all(|entry| !entry.to_string().contains("Run the fixture"))
        );
        assert!(result?.contains(expected_answer));
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
            model_provider: Some(provider),
            model_url: format!("http://127.0.0.1:{port}/v1/"),
            model_name: model.into(),
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
            None,
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
        model_provider: Some(Provider::LlamaCpp),
        model_url: format!("http://127.0.0.1:{port}"),
        model_name: "toggle fixture".into(),
        ..Config::default()
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let connection = config.model_connection()?;
    let levels = connection
        .capabilities(&client, &config.model_name)
        .await?
        .thinking?;
    assert_eq!(levels.len(), 2);
    for (level, enabled) in levels.into_iter().zip([true, false]) {
        assert_eq!(level.setting, ThinkingSetting::Toggle(enabled));
        let result = chat(
            config.clone(),
            Vec::new(),
            Some(level.setting),
            None,
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
            model_provider: Some(Provider::OpenAiCompatible),
            model_name: "picked-skill-fixture".into(),
            workspace: workspace.clone(),
            ..Config::default()
        };
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.py");
        let (_http, port) = http_fixture(&fixture).await?;
        config.model_url = format!("http://127.0.0.1:{port}/v1");
        let name = config.add_skill(&example)?;
        assert_eq!(name, "test-notes");
        let tools = ToolSet::connect(&config, |_| {}).await;
        let skill = tools.call("load_skill", json!({ "name": name })).await?;
        assert!(skill.contains("tests/test-notes/notes.py"));

        let args = json!({
            "command": "python3",
            "args": [relative_cli, "add", "--", "Shopping", "Buy milk"]
        });
        assert_eq!(tools.activity("terminal", &args), "python3 notes.py add");
        assert!(tools.approval("terminal", &args).is_some());
        let output: Value = serde_json::from_str(&tools.call("terminal", args).await?)?;
        assert_eq!(output["exit_code"], 0, "{output}");
        let note: Value =
            serde_json::from_str(output["stdout"].as_str().context("Missing stdout")?)?;
        assert_eq!(note, json!({"id": 1, "title": "Shopping"}));
        drop(tools);
        let approved = std::cell::Cell::new(false);
        let skill_loaded = std::cell::Cell::new(false);
        let answer = chat(
            config,
            vec![("user".into(), "List my notes".into())],
            None,
            Some(TurnSelection::Skill(name)),
            |event| match event {
                Event::SkillLoaded(name) => {
                    assert_eq!(name, "test-notes");
                    skill_loaded.set(true);
                }
                Event::Approve(request, reply) => {
                    assert!(skill_loaded.get());
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
        model_provider: Some(Provider::OpenAiCompatible),
        model_url: format!("http://127.0.0.1:{port}/v1"),
        model_name: "find-tools-fixture".into(),
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
        None,
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

    let expected = McpToolId {
        server: "fixture".into(),
        name: "lookup".into(),
    };
    let tools = ToolSet::connect(&config, |warning| {
        panic!("Unexpected discovery warning: {warning}")
    })
    .await;
    let selected = tools
        .mcp_tool_choices()
        .into_iter()
        .find(|item| *item == expected)
        .context("Fixture lookup missing in picker catalog")?;
    drop(tools);
    config.model_name = "picked-mcp-fixture".into();
    let result = chat(
        config,
        vec![("user".into(), "Run the selected lookup".into())],
        None,
        Some(TurnSelection::Mcp(selected)),
        |event| {
            if let Event::Approve(request, reply) = event {
                assert!(request.description.starts_with("Call fixture / lookup"));
                approvals.set(approvals.get() + 1);
                let _ = reply.send(ApprovalDecision::RunOnce);
            }
        },
    )
    .await?;
    assert_eq!(result, "Picked MCP fixture passed");
    assert_eq!(approvals.get(), 2);
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
        model_provider: Some(Provider::OpenAiCompatible),
        model_url: format!("http://127.0.0.1:{port}/v1"),
        model_name: "fixture".into(),
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
    let statuses = std::cell::RefCell::new(Vec::new());
    let loaded_skills = std::cell::RefCell::new(Vec::new());
    let remembered = std::cell::RefCell::new(config.clone());
    let result = chat(
        config,
        vec![("user".into(), "Run the fixture".into())],
        None,
        None,
        |event| match event {
            Event::Status(status) => statuses.borrow_mut().push(status),
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
    let statuses = statuses.into_inner();
    assert!(
        statuses
            .iter()
            .any(|status| status == "Running · Auto-approved · printf")
    );
    assert!(statuses.windows(2).any(|steps| steps
        == [
            "Waiting for approval · local / lookup · Look up the fixture",
            "Running · local / lookup · Look up the fixture",
        ]));
    assert!(
        statuses.iter().any(
            |status| status == "Running · Auto-approved · local / lookup · Look up the fixture"
        )
    );
    assert!(
        statuses
            .iter()
            .any(|status| status == "Running · remote / lookup · Look up the fixture")
    );
    Ok(())
}
