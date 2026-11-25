# Development guide

[Home](README.md) · [Installation](docs/installation.md) · [Sandbox architecture](research/SANDBOXING.md)

## Start a task

1. Start with the file or symbol named in the request. Otherwise, choose a task in the [code map](#code-map).
2. Run `git status --short` and `git diff --stat`, then read the existing diff for the files you’ll touch.
3. Find the entry point with `rg -n` and read a focused, line-numbered range. Follow the caller and any dependencies needed to understand the outcome.
4. Before editing shared behavior, check the [behavior boundaries](#boundaries-to-check-when-changing-behavior).
5. Run the focused check from the code map. For code changes, run both projects’ test suites under [Verify a change](#verify-a-change).

For dependencies, start with the relevant manifest or `cargo tree -i <crate>`. Read targeted ranges in the lockfiles, `desktop.rs`, and `tools.rs`. Leave local settings, chats, logs, and build output outside routine source inspection.

## Package boundaries

The repository contains two Rust projects, each with its own manifest and lockfile. Use Rust 1.100.0 and run these commands from the repository root.

| Project | Owns | Build |
| --- | --- | --- |
| [Pluto](pluto/Cargo.toml) | Harness, model routing, tools, storage, and headless service | `cargo build --locked --manifest-path pluto/Cargo.toml` |
| [Pluto Desktop](pluto-desktop/Cargo.toml) | Desktop UI, saved connection, and SSH transport | `cargo build --locked --manifest-path pluto-desktop/Cargo.toml` |

Standalone Desktop runs the Pluto library in-process. In VM mode, Desktop uses the shared API types and sends newline-delimited JSON over SSH to the service’s private Unix socket. The Linux build uses the `pluto/` project alone. Building Desktop requires both directories. [VM setup](pluto/vm/README.md) · [Mac installer](pluto-desktop/install-macos.sh)

To pin Rust for this checkout, run `rustup toolchain install 1.100.0` and `rustup override set 1.100.0` from the repository root. Remove the override with `rustup override unset`.

## Code map

[Desktop UI](#desktop-ui) · [Harness, models, and tools](#harness-models-and-tools) · [Pluto VM](#pluto-vm) · [Documentation](#documentation)

### Desktop UI

| Task | Read first | Focused check |
| --- | --- | --- |
| Sidebar, chat layout, Markdown, timestamps, or live activity | [Desktop UI](pluto-desktop/src/desktop.rs) · `Desktop::show_ui`, `chat_message`, `tool_progress_ui` | `chat_layout_tests` |
| Composer or `/` picker | [Shared chat UI](pluto-desktop/src/desktop/chat_ui.rs) · `ChatComposer`, `chat_transcript` | `chat_layout_tests` |
| Saved chats and archive | [History](pluto/src/history.rs) · `ChatHistory`, `ChatStore`. Desktop uses `persist_chat`, `open_chat`, and `move_active_chat`. | `saves_reopens_and_updates_separate_chats` |
| Settings tabs and manual saves | [Shared tabs](pluto-desktop/src/desktop/settings_tabs.rs), [Desktop UI](pluto-desktop/src/desktop.rs) · `settings_ui`, `save`, `reload_settings`, and [Config](pluto/src/tools.rs). | `standalone_settings_tabs_keep_their_existing_controls` and `mcp_changes_require_an_explicit_save` |
| MCP setup and connection testing | [Shared MCP editor](pluto-desktop/src/desktop/mcp_settings.rs) · `McpEditor`, [tools](pluto/src/tools.rs) · `Mcp::discover` | `chat_loads_skill_runs_cli_and_calls_both_mcp_transports` and the [VM smoke test](pluto/vm/README.md#verify) |

### Harness, models, and tools

| Task | Read first | Focused check |
| --- | --- | --- |
| Prompt, local clock, tool loop, or partial answer | [Harness](pluto/src/agent.rs) · `chat` | `chat_enforces_the_tool_call_limit` |
| Model URLs, catalog, thinking, or context meter | [Providers](pluto/src/providers/mod.rs) · `Connection`, then the provider file. [Desktop UI](pluto-desktop/src/desktop.rs) · `chat_heading`, `context_usage_ui`. | `provider_metadata_controls_chat_reasoning` or `llama_template_can_expose` |
| Model profiles and fallbacks | [Tools](pluto/src/tools.rs) · `ModelSettings`, `Config::load_from`. [Fallbacks](pluto/src/model_fallback.rs) · `FallbackSettings::pick`. [Desktop UI](pluto-desktop/src/desktop.rs) · `model_settings_ui`, `fallback_settings_ui`. | `model_settings_migrate_legacy_profiles`, `rate_limit_fallback_matches_metadata_and_reuses_tool_results`, or `model_modes_use_the_expected_fallback_order` |
| Shared model form and discovery | [Model settings UI](pluto-desktop/src/desktop/model_settings.rs) · `ModelCatalog`, `fallback_form`. The [VM service](pluto/src/vm.rs) handles VM catalog and metadata requests. | `model_form_loads_catalog` and the [VM smoke test](pluto/vm/README.md#verify) |
| Skills and terminal execution | [Tools](pluto/src/tools.rs) · `Config::add_skill`, `Config::skill_catalog`, `ToolSet::call`, `run_process` | `pluto_loads_and_runs_the_test_notes_skill` |
| MCP tool discovery and schemas | [Tools](pluto/src/tools.rs) · `ToolSet::initial_mcp_tools`, `find_tools`, `definitions` | `find_tools_exposes_fixture_schemas_on_demand` |
| Tool approval | [Harness](pluto/src/agent.rs) · approval branch in `chat`. [Desktop UI](pluto-desktop/src/desktop.rs) · `approval_ui`. [Tools](pluto/src/tools.rs) · `Config::permits`. | `global_auto_approval_runs_and_tracks` or `remembered_cli_approvals` |
| Background diagnostics | [Diagnostics](pluto/src/diagnostics.rs) · `Trace::record`, `start`. Event fields are set at the call site in [the harness](pluto/src/agent.rs). | `background_writer_records_events` |

### Pluto VM

| Task | Read first | Focused check |
| --- | --- | --- |
| Install, update, or manage the VM | [VM CLI](pluto/vm/vm), [provisioning](pluto/vm/setup-vm.sh), [Lima defaults](pluto/vm/lima.yaml) | [VM smoke test](pluto/vm/README.md#verify) |
| List VMs, switch modes, or create a VM | [Desktop VM client](pluto-desktop/src/desktop/vm_client.rs) · `DesktopApp`, `VmPicker`. The [Mac installer](pluto-desktop/install-macos.sh) bundles the Pluto build inputs. | `vm_picker` |
| Start, reopen, archive, or restore a VM chat | [Desktop VM client](pluto-desktop/src/desktop/vm_client.rs) · `chat_navigation`, [service](pluto/src/vm.rs) · `Service::session` | `vm_new_chat_clears_the_view_and_keeps_recent_chats` and the [VM smoke test](pluto/vm/README.md#verify) |
| VM API, model settings, and approvals | [API types](pluto/src/api.rs), [service](pluto/src/vm.rs) · `Service::session`, [Desktop transport](pluto-desktop/src/desktop/vm_client.rs) · `exchange` | `vm_approval_dialog` and the [VM smoke test](pluto/vm/README.md#verify) |
| Edit per-VM settings in Desktop | [VM Settings form](pluto-desktop/src/desktop/vm_settings.rs), [configuration](pluto/src/tools.rs) · `VmSettings`, `Config::save_vm_settings` | [VM smoke test](pluto/vm/README.md#verify) |
| Start, stop, restart, or view VM resource usage from Desktop | [Desktop VM client](pluto-desktop/src/desktop/vm_client.rs) · `VmPower`, `runtime_ui`, `VmUsage` | `vm_runtime` and `vm_usage` |

### Documentation

| Task | Read first | Focused check |
| --- | --- | --- |
| Install, update, run, or configure Desktop | [Installation](docs/installation.md) or the relevant guide in the [README](README.md#find-a-guide) | Match setup steps to the installer, manifest, or settings UI. Check changed links and anchors. |
| Set up, operate, or integrate with Pluto VM | [VM guide](pluto/vm/README.md) | Match commands to the VM CLI and API fields to the shared types. |
| Explain the sandbox design or cite benchmark results | [Sandboxing research](research/SANDBOXING.md) | Check implementation claims against linked source. Check benchmark scope in the [measurement data](research/sandbox-measurements.csv). |

## Boundaries to check when changing behavior

- **Chat turns.** Both modes share the composer, transcript, history list, and approval controls. `Desktop::send` snapshots standalone settings and history. The VM service loads them in `Service::session`. Both use `agent::chat` and `ChatStore`.
- **Model routing.** `models.mode` selects `models.local` or `models.openai`. Keep older single-model TOML readable. The OpenAI-compatible route tries its primary, eligible backups, then the saved local profile. Switch within the model loop so tool results, approvals, and the tool-call count carry forward.
- **Fallback metadata.** Match exact IDs against `/models`. Bound catalog reads, read each provider catalog once per selection, and retain metadata for configured models. Scope cooldowns to provider origins and calculate context fit from borrowed messages and tool definitions.
- **Settings.** Both modes share the tab and model forms. Standalone settings use a local TOML file. In VM mode, the service reads and saves that instance’s TOML over the API. Model discovery and fallback checks use the VM’s network and token environment. VM saves preserve unrelated fields and reject stale edits. The `vm configure` command replaces the full VM settings file.
- **VM lifecycle.** Desktop creates instances using the bundled VM CLI. Start, stop, and restart run through Lima. Usage sampling reads Linux CPU and memory counters over SSH while VM Settings is open. Status and usage checks leave stopped instances stopped.
- **Widget state.** Scope Markdown widgets and nested scroll areas to the chat and message. Keep the composer outside the scrolling history.
- **Diagnostics and storage.** Use the saved chat filename stem for `chat_id`, including through archive and restore. Standalone rotates `agent-debug.jsonl`. The VM rotates `diagnostics.jsonl`. Record identifiers, decisions, and timings. Keep raw content and credentials out of events. Check older TOML and chat JSON when adding fields.

## Verify a change

Pass the test name from the code map as a filter to `cargo test` with the corresponding project manifest. For code changes, use the following commands for the full checks.

```sh
cargo fmt --manifest-path pluto/Cargo.toml --check
cargo fmt --manifest-path pluto-desktop/Cargo.toml --check
cargo test --locked --quiet --manifest-path pluto/Cargo.toml
cargo test --locked --quiet --manifest-path pluto-desktop/Cargo.toml
```

- Model and MCP integration tests use Python 3 and ephemeral loopback ports. See the [Rust tests](pluto/tests/chat.rs) and [Python fixture](pluto/tests/fixture.py). Desktop layout tests run without opening a window.
- To check skills and MCP with your own model, follow the [manual fixture walkthrough](pluto/vm/README.md#try-the-notes-skill-and-mcp-fixture).
- For startup or packaging changes, build both projects. For VM setup, transport, or persistence changes, run the [smoke test on a disposable VM](pluto/vm/README.md#verify).
- For a manual standalone launch with isolated state, set `PLUTO_CONFIG` to a scratch `tools.toml` and launch Desktop with `--standalone`.

In a review, trace the changed path from its normal caller through the outcome. Check relevant failure and persistence cases. Report findings in impact order with file and line references, and summarize passing checks.
