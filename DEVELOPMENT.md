# Development guide

Pluto is a Rust desktop chatbot. It talks to a tool-capable model through an OpenAI-compatible endpoint, loads local skill instructions on demand, runs installed CLIs, and calls tools from local or remote MCP servers. Config lives in `tools.toml`. Chat history is stored as one JSON file per conversation.

A chat turn starts at `Desktop::send`, enters `agent::chat`, and uses `ToolSet::call` to get results for the model. Before a terminal command or MCP call runs, `agent::chat` emits `Event::Approve` and waits for the desktop to reply. The desktop displays the model's final answer.

## Where to start

| Task or question | File and symbols to read first |
| --- | --- |
| Model request, system prompt, chat context, tool loop, or output limits | [src/agent.rs](src/agent.rs) → `chat` |
| Model providers, discovery, or thinking controls | [src/providers/mod.rs](src/providers/mod.rs) → `Connection`, then the selected provider module and `Desktop::model_settings_ui` or `Desktop::chat_heading` |
| Chat UI, history in the window, or approval dialog | [src/main.rs](src/main.rs) → `Desktop::send`, `Desktop::update` |
| Saved chat files and restoring conversations | [src/history.rs](src/history.rs) → `ChatStore`, and [src/main.rs](src/main.rs) → `Desktop::persist_chat`, `Desktop::open_chat` |
| Settings screen or adding an integration from the UI | [src/main.rs](src/main.rs) → `Desktop::settings_ui`, `Desktop::save`, `Desktop::add_mcp` |
| Config format, file location, or persistence | [src/tools.rs](src/tools.rs) → `Config`, `Config::path`, `Config::load`, `Config::save` |
| Skill registration, catalog, or loading instructions | [src/tools.rs](src/tools.rs) → `Config::add_skill`, `Config::skill_catalog`, the `load_skill` branch of `ToolSet::call` |
| Terminal arguments, working directory, timeout, or stdout and stderr | [src/tools.rs](src/tools.rs) → the `terminal` branch of `ToolSet::call`, `run_process`, `read_output` |
| MCP transport, authentication, discovery, or tool calls | [src/tools.rs](src/tools.rs) → `Mcp::connect`, `ToolSet::connect`, the MCP branch of `ToolSet::call` |
| Tool schemas or the approval prompt text | [src/tools.rs](src/tools.rs) → `ToolSet::definitions`, `ToolSet::approval` |
| End-to-end behavior or regressions | [tests/support/chat.rs](tests/support/chat.rs) and [tests/fixture.py](tests/fixture.py) |
| Build dependencies or user setup | [Cargo.toml](Cargo.toml) or [README.md](README.md), respectively |

## Working with a small context window

1. Read the relevant symbols first. Use `rg` to find callers or definitions when the task crosses a module boundary. For a full review, include the tests.
2. Keep `Cargo.lock` in the project for reproducible builds. For dependency questions, start with `Cargo.toml` and use `cargo tree` or read the matching lockfile entries. Exclude the full lockfile and `target/` from ordinary prompts.
3. For code changes, run `cargo fmt --check` and `cargo test --locked`. The end-to-end test uses `python3` on Linux or macOS and covers skill loading, CLI arguments, both MCP transports, and declined commands.

## Reviewing code

1. For a change review, start with the diff and trace the affected callers and tests. For a project-wide review, start with `Desktop::send`, `agent::chat`, `ToolSet::call`, and the end-to-end test.
2. Read small, line-numbered ranges around relevant symbols. Open more code as needed to trace behavior or verify a finding.
3. Keep successful build and test output to the final summary. Inspect the relevant diagnostics when a check fails.
4. Give an overall assessment and prioritize findings by impact. Cite the file and line for each finding and state which checks ran.
