# Development guide

Pluto is a Rust desktop chatbot. A turn flows from `Desktop::send` to `agent::chat` to `ToolSet::call`. Settings live in `tools.toml`, and each saved chat has its own JSON file.

## Start a task

1. Start with the file or symbol named in the request. Otherwise, use the matching row in the code map. For an existing change, start with `git status --short`, `git diff --stat`, and the diff for the relevant paths.
2. Use `rg -n` to locate the symbols. Read the smallest line-numbered ranges that explain the behavior. Follow callers, tests, or dependencies only when they answer an open question.
3. For dependency questions, read `Cargo.toml` and use `cargo tree` or the relevant `Cargo.lock` entries. Keep the full lockfile, generated files, and `target/` out of routine reads.
4. For code changes, run `cargo fmt --check` and `cargo test --locked --quiet`. Use a focused test first when it can confirm the change quickly. Keep successful output to a summary. If a check fails, read the diagnostics for the specific failure.

## Code map

| Task | First source to read |
| --- | --- |
| Chat UI, message composer, or `/` picker | [src/main.rs](src/main.rs) → `Desktop::update`, `Desktop::composer_ui`, `Desktop::picker_ui` |
| Model requests, prompt, turn selection, or tool loop | [src/agent.rs](src/agent.rs) → `chat` |
| Model providers or thinking controls | [src/providers/mod.rs](src/providers/mod.rs) → `Connection`, then the provider module |
| Saved chats | [src/history.rs](src/history.rs) → `ChatStore`, then `Desktop::persist_chat` or `Desktop::open_chat` |
| Settings and persistence | [src/main.rs](src/main.rs) → `Desktop::settings_ui`, `Desktop::save`, then [src/tools.rs](src/tools.rs) → `Config` |
| Skill registration or loading | [src/tools.rs](src/tools.rs) → `Config::add_skill`, `Config::skill_catalog`, or the `load_skill` branch of `ToolSet::call` |
| Terminal execution | [src/tools.rs](src/tools.rs) → `run_process`, `read_output` |
| MCP setup, connection test, or discovery | [src/main.rs](src/main.rs) → `Desktop::mcp_draft`, `Desktop::start_mcp_test`, or [src/tools.rs](src/tools.rs) → `Mcp::discover`, `ToolSet::connect` |
| MCP tool selection or schemas | [src/tools.rs](src/tools.rs) → `ToolSet::find_tools`, `ToolSet::definitions`, `ToolSet::mcp_alias` |
| Approval and in-chat activity | [src/main.rs](src/main.rs) → `Desktop::approval_ui`, then [src/agent.rs](src/agent.rs) → `chat` or [src/tools.rs](src/tools.rs) → `Config::permits` |
| End-to-end tests | [tests/support/chat.rs](tests/support/chat.rs) → matching test, then the associated scenario in [tests/fixture.py](tests/fixture.py) |
| User setup | [README.md](README.md) → relevant section |

## Review a change

Start with the diff and verify affected behavior through its callers and tests. For a project-wide review, start with `Desktop::send`, `agent::chat`, `ToolSet::call`, and the end-to-end tests, then expand based on findings. Report findings in impact order with file and line references, and state which checks ran.
