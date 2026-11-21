use crate::agent::{ApprovalDecision, Event, TokenUsage, chat};
use crate::providers::{Provider, ThinkingSetting};
use crate::tools::{Config, Mcp, ToolSet};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::time::Duration;
use std::{fs, path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::process::Command;

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
        let result = chat(config.clone(), Vec::new(), Some(level.setting), |_| {}).await?;
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
            workspace: workspace.clone(),
            ..Config::default()
        };
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
        Ok(())
    }
    .await;
    fs::remove_dir_all(&workspace)?;
    result
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

    let approvals = std::cell::RefCell::new(Vec::new());
    let statuses = std::cell::RefCell::new(Vec::new());
    let result = chat(
        config,
        vec![("user".into(), "Run the fixture".into())],
        None,
        |event| match event {
            Event::Status(status) => statuses.borrow_mut().push(status),
            Event::Approve(request, reply) => {
                let decision = if request.description.contains("should be declined") {
                    ApprovalDecision::Decline
                } else if request.terminal.is_some() {
                    ApprovalDecision::AutoApproved
                } else {
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
    assert_eq!(approvals.into_inner().len(), 4);
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
    Ok(())
}
