# Development guide

## Start a task

1. If the request names a file or symbol, start there. Otherwise, choose one row in the [code map](#code-map).
2. For existing work, check `git status --short` and `git diff --stat`, then read the diff for the affected paths.
3. Find the entry point with `rg -n` and read its relevant lines with `sed -n`. Follow one caller, dependency, or test at a time when it answers a specific question.
4. Verify the normal entry point with the focused check below. Expand to the full suite for code changes.

Use [README.md](README.md) for setup and user-facing behavior. For dependency work, start with [Cargo.toml](Cargo.toml), `cargo tree -i <crate>`, or one lockfile entry. Skip whole-file reads of `Cargo.lock`, `src/main.rs`, and `src/tools.rs`. Build output, assets, live settings, history, and logs are outside the usual reading path.

## Code map

| Task | Read first | Focused check |
| --- | --- | --- |
| Sidebar, message layout, Markdown, timestamps, or live tool status | [src/main.rs](src/main.rs) · `Desktop::show_ui`, `chat_message`, `tool_progress_ui` | `chat_layout_tests` |
| Composer or `/` picker | [src/main.rs](src/main.rs) · `Desktop::composer_ui`, `Desktop::picker_ui` | `chat_layout_tests` |
| Prompt, local clock, tool loop, or partial answer | [src/agent.rs](src/agent.rs) · `chat` | `chat_allows_52_tool_calls` |
| Provider URLs, model catalog, thinking, or context meter | [src/providers/mod.rs](src/providers/mod.rs) · `Connection`, then that provider’s file. For display, see `Desktop::chat_heading` and `Desktop::context_usage_ui`. | `provider_metadata_controls_chat_reasoning` or `llama_template_can_expose` |
| Saved chats and archive | [src/history.rs](src/history.rs) · `ChatHistory`, `ChatStore`. For UI, see `Desktop::persist_chat`, `Desktop::open_chat`, `Desktop::move_active_chat`. | `saves_reopens_and_updates_separate_chats` |
| Settings and manual saves | [src/main.rs](src/main.rs) · `Desktop::settings_ui`, `Desktop::save`. For TOML, see `Config` in [src/tools.rs](src/tools.rs). | `mcp_changes_require_an_explicit_save` |
| Skills and terminal execution | [src/tools.rs](src/tools.rs) · `Config::add_skill`, `Config::skill_catalog`, `ToolSet::call`, `run_process` | `pluto_loads_and_runs_the_test_notes_skill` |
| MCP server setup and connection testing | [src/main.rs](src/main.rs) · `Desktop::mcp_draft`, `Desktop::start_mcp_test`. For transport, see `Mcp::discover` and `ToolSet::connect`. | `chat_loads_skill_runs_cli_and_calls_both_mcp_transports` |
| MCP tool discovery and schemas | [src/tools.rs](src/tools.rs) · `ToolSet::initial_mcp_tools`, `ToolSet::find_tools`, `ToolSet::definitions` | `find_tools_exposes_fixture_schemas_on_demand` |
| Tool approval | [src/agent.rs](src/agent.rs) · approval branch of `chat`. For prompts and saved rules, see `Desktop::approval_ui` and `Config::permits`. | `global_auto_approval_runs_and_tracks` or `remembered_cli_approvals` |
| Background diagnostics | [src/diagnostics.rs](src/diagnostics.rs) · `Trace::record`, `start`. For event fields, find the event name in `src/agent.rs`. | `background_writer_records_events` |

## Boundaries to check when changing behavior

- **Chat turn** · `Desktop::send` snapshots the config and transcript. `agent::chat` handles model and tool calls, then sends events to `Desktop::update`. The clock is added to the outgoing prompt. `ChatStore` saves message timestamps alongside the transcript.
- **Widget state** · Scope a reply’s Markdown widgets and nested scroll areas to the chat and message. Keep the composer outside the scrolling history. The layout fixtures check for ID collisions and obscured controls.
- **Settings and approvals** · MCP add and remove actions stage settings. **Save settings** writes the draft. **Run always** saves a matching rule from the approval dialog. Global auto-approval uses the saved setting at the start of each turn.
- **Diagnostics** · Queue metadata with `Trace::record`. The background writer serializes and rotates the log. Record tool and model identifiers, decisions, and timings while keeping raw content and credentials out of events.
- **Persisted files** · Check older TOML and chat JSON when adding fields. History tests cover save, reopen, archive, and restore.

## Verify a change

Use **Rust 1.100.0**. From the project root, run:

```sh
cargo test --locked --quiet chat_layout_tests
cargo fmt --check
cargo test --locked --quiet
```

Replace `chat_layout_tests` with the filter for your task. Run `cargo build --locked --quiet` when the change affects startup or packaging. Tests use Python 3 fixtures on a local ephemeral port. Start with the matching test in [tests/support/chat.rs](tests/support/chat.rs), then find only its corresponding model or MCP case in [tests/fixture.py](tests/fixture.py). Chat layout tests run without opening the desktop window. For a manual launch with isolated settings, set `PLUTO_CONFIG` to a scratch file before running Pluto.

For a review, follow the changed path from its normal caller to the outcome and check the applicable persistence or failure cases. Report findings in impact order with file and line references. Summarize successful checks and include only relevant failure output.
