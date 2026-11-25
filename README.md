# Pluto

<img src="pluto-desktop/assets/pluto-icon.png" alt="Pluto icon" width="112" height="112">

**Pluto** is a free, local-first assistant for everyday work with open models. Use it to draft a message, summarize notes, plan a project, or work on code. **Pluto Desktop** is the Mac app for chat and tool approvals.

## Get started

Install [Pluto Desktop](#install-on-macos-from-a-source-archive), then choose where Pluto runs.

| Mode | Where Pluto runs | Where your files and chat live |
| --- | --- | --- |
| **Standalone** | On your Mac. Terminal commands and local MCP servers run with your account’s permissions. | On your Mac |
| **Pluto VM** | In an optional Linux sandbox. Terminal commands and local MCP servers run inside the VM. | On the VM’s persistent disk |

### Run Pluto Desktop in standalone mode

Open **Pluto Desktop**, open **Settings → Connection** and click **Connect** next to **Standalone** if you’re switching from a VM, and [connect a model](#connect-a-model). From the repository root, you can also launch directly with Rust 1.100.0:

```sh
cargo run --locked --manifest-path pluto-desktop/Cargo.toml -- --standalone
```

The standalone Linux build needs a desktop and OpenGL drivers.

### Run in Pluto VM

On an Apple Silicon Mac with Lima 2.3 or later, open **Settings → Connection → New VM** in Pluto Desktop. Enter an instance name, then click **Create VM**. When it appears in the list, click **Connect** and open the VM’s **Settings → Model**. Desktop remembers your connection and starts the VM when needed. [VM model setup](pluto/vm/README.md#2-connect-a-model)

To install the default VM from Terminal, run this from the repository root:

```sh
pluto/vm/vm install
```

Run the same install command to apply the source in your checkout. Use `pluto/vm/vm install --latest` from a Git checkout with an upstream to install that branch’s latest source. [VM updates](pluto/vm/README.md#update-pluto-vm) · [Manual builds](pluto/vm/README.md#build-manually-from-source) · [Use Pluto from Terminal](pluto/vm/README.md#use-pluto-without-desktop) · [VM API](pluto/vm/README.md#api)

You can also [use Pluto from Terminal](pluto/vm/README.md#use-pluto-without-desktop) or [rename an existing instance to `plutoVM`](pluto/vm/README.md#name-or-rename-a-vm).

Once connected, **Settings → VM runtime** shows the VM’s status, allocated resources, and live CPU and RAM usage. Start, stop, or restart that VM from the same panel. [VM settings and runtime](pluto/vm/README.md#configure-a-vm-in-desktop)

## Install on macOS from a source archive

1. Install the Xcode Command Line Tools with `xcode-select --install`. Install Rustup if needed, then open a new Terminal window. [Apple Command Line Tools](https://developer.apple.com/documentation/xcode/installing-the-command-line-tools) · [Rustup](https://rustup.rs/)
2. Download and unzip the source for the branch or release you want. In Terminal, open the folder containing `pluto/` and `pluto-desktop/`. [Pluto on GitHub](https://github.com/BlackRabbitt/pluto)
3. Run the installer and open **Pluto Desktop** from Spotlight or Applications.

   ```sh
   sh pluto-desktop/install-macos.sh
   ```

To launch with your shell’s API keys and command search path, run `~/.cargo/bin/pluto-desktop`. [MCP environment variables on macOS](#mcp-environment-variables-on-macos)

### Update on macOS

Quit Pluto Desktop and run the same installer from the updated checkout or a newly unpacked source archive. It replaces the app and Terminal executable even when the version number is unchanged. Your settings and chats stay in place. [Local files](#local-files-and-troubleshooting)

## Connect a model

Both modes use the same **Model**, **Skills**, **MCP servers**, and **Tool approvals** tabs. The heading shows which environment you’re editing. The steps below are for standalone mode. [VM model setup](pluto/vm/README.md#2-connect-a-model)

1. Open **Settings** and configure **Local model**, **OpenAI-compatible model**, or both. Enter the server URL, then click **Load models** or enter the model ID.
2. Choose **Local only** to use llama.cpp or Ollama. **OpenAI-compatible** tries the primary model, enabled backups, and then the saved local model if configured.
3. Set the **Terminal working directory** if you plan to run commands. Click **Save settings** and open **Chat**.

| Provider | Server URL |
| --- | --- |
| **llama.cpp** | The server base URL, for example `http://localhost:8080` |
| **Ollama** | The server base URL, for example `http://localhost:11434` |
| **OpenAI-compatible** | The API base URL for a server offering a free, open model, including its required path such as `/v1` |

For an authenticated model or MCP server, export the key before launching Pluto Desktop. In Settings, enter the **variable name**, such as `MODEL_API_KEY`. If Pluto can load the model list but chat returns a 404, check that your model ID supports text chat on that endpoint.

### Tested local setup

Pluto has been tested with **Qwen3.5-27B Q4_K_M** on llama.cpp build **b12426**, with Jinja templates enabled and a **32,768-token** server context. The tested GGUF is about **16.7 GB**. Qwen publishes the base model under Apache 2.0. [Qwen model card](https://huggingface.co/Qwen/Qwen3.5-27B) · [Unsloth GGUF](https://huggingface.co/unsloth/Qwen3.5-27B-GGUF/blob/3221f178a6b842d04f1fb42f1c413534adcc0a6a/Qwen3.5-27B-Q4_K_M.gguf)

| Pluto setting | Tested value |
| --- | --- |
| **Runtime** | llama.cpp |
| **Server URL** | `http://localhost:8080` on the machine running llama.cpp |
| **Model name** | `qwen3.5-27b` when launched with `--alias qwen3.5-27b` |
| **Token environment variable** | Blank for an unauthenticated local server |

With the GGUF already downloaded, run a same-machine server with:

```sh
llama-server -m /path/to/Qwen3.5-27B-Q4_K_M.gguf --jinja --ctx-size 32768 \
  --alias qwen3.5-27b --host 127.0.0.1 --port 8080
```

### Find free cloud models and configure fallbacks

The [FreeLLM registry](https://freellm.net/models/) lists free tiers across providers. Pick a free, openly licensed model and copy its provider's API base URL and exact model ID. Provider quotas and available models can change.

Under **OpenAI-compatible model**, enable **Try other free models automatically** to add up to four backups in preference order. Use **Check model metadata** to see the provider's reported context limit and tool support. Supply verified values when the provider omits them. Confirm each backup is free and openly licensed, then save.

Pluto honors `Retry-After` when an inference server returns HTTP 429. It checks estimated context fit and tool support before picking a cloud backup. Each switched model receives the current chat and tool results so Pluto can continue without repeating completed tools. Chat shows the selected destination.

### Connect to a model on your local network from macOS

Open the installed **Pluto Desktop** app from Spotlight. In Settings, enter the model server’s LAN address and port, then click **Load models**. Use `localhost` when the model server runs on the same Mac. For a server on another device, make sure it listens on its LAN interface and accepts connections on that port.

If macOS asks to allow local network access, choose **Allow**. macOS lists Pluto Desktop under **System Settings → Privacy & Security → Local Network** after it attempts a connection that requires permission. If Pluto Desktop is already listed, make sure its switch is on. [Apple’s local network privacy guide](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)

Source installs use ad hoc signing by default, so macOS can ask again after a rebuild. If you have an Apple-issued code-signing identity in your keychain, set `PLUTO_SIGNING_IDENTITY` to its name when running the installer. To compare whether the issue is specific to the app’s local network permission, try the same server from `~/.cargo/bin/pluto-desktop` in Terminal. macOS grants local network access to command-line tools launched from Terminal. [Apple’s macOS signing and local network guidance](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy#Build-time-considerations)

## Chat

Both modes use the same chat controls. Click **New chat** to start fresh, or use **Recent chats** and **Archived chats** to reopen a conversation. Each VM keeps its own chats, approvals, and workspace. Update Pluto Desktop and the Pluto service to get the full set of VM chat controls. [VM updates](pluto/vm/README.md#update-pluto-vm)

- Use the **Thinking** menu to select a level or toggle when llama.cpp or Ollama reports support. **Server default** uses the server’s own behavior. The choice applies to subsequent messages with that model.
- While waiting for a model response, Chat shows a spinner and the current phase: **Thinking…**, **Reviewing tool results…**, or **Writing a partial answer…**. When a model includes a leading `<think>` block in its reply, expand **Model thinking** to see it.
- Chat shows token usage for the last model request. With llama.cpp, the bar shows input and output tokens against the server's configured context size. With an OpenAI-compatible server that reports a context limit, the bar shows input tokens against that limit. You can also set **Context limit (input tokens)** in Settings for Ollama or OpenAI-compatible servers.
- Pluto sends the full transcript of the open chat and the computer’s local date, time, and time zone to the configured model. New messages show a timestamp below the sender name.
- Pluto opens the most recently updated active chat on launch. Use **Archive chat** to move it out of **Recent chats** and **Restore chat** to continue it. Use **Copy chat ID** to find its diagnostic events.

## Use skills and tools

The walkthrough below uses standalone mode. In VM mode, connect to the instance you want to edit and open **Settings**. You can register a skill from a VM path, import a portable `SKILL.md` from the Mac, and edit local or remote MCP servers. [Configure a VM in Desktop](pluto/vm/README.md#configure-a-vm-in-desktop) · [Try the notes skill and MCP fixture in a VM](pluto/vm/README.md#try-the-notes-skill-and-mcp-fixture)

Type `/` at the start of the message box to pick a skill or MCP tool. Filter by typing, then click a result or use **↑**, **↓**, and **Enter**. Type `/` again to add more skills or MCP tools before writing your request. Use **×** to remove a selection. Selections apply to the next message you send.

### Add or update a skill

1. Install the skill’s CLI separately and make sure Pluto can find it on your PATH.
2. In **Settings → Skills**, enter the full path to a local skill directory or its `SKILL.md`. Choose **Add skill**, then **Save settings**. For GitHub skills, check out the files locally first.
3. Ask Pluto for something the skill can do. When the skill loads, Chat shows its name. Terminal commands run noninteractively from the configured working directory with your account’s privileges.

Pluto passes arguments directly to the executable. Pipes and redirects require an explicit shell command. The model receives the exit code and captured output. Pluto keeps up to 5,500 bytes from each terminal output stream.

| What changed | What to do |
| --- | --- |
| Instructions in `SKILL.md` | Save the file in place. Pluto reads it the next time it loads the skill, even in the same chat. |
| Description | Add the same file again and click **Save settings** to refresh what the model sees in the skill catalog. |
| Name or path | Remove the old entry, add the updated one, and save. |
| Files in a GitHub repo | Update your local copy, then follow the row for what changed. |

To try the test notes skill, install Python 3, set Pluto’s terminal working directory to the `pluto/` project folder, and register [the test notes skill](pluto/tests/test-notes/SKILL.md). Save settings and ask “Create a test note titled Shopping with the body Buy milk.” Try the CLI directly from the repository root.

```sh
python3 pluto/tests/test-notes/notes.py add -- "Shopping" "Buy milk"
python3 pluto/tests/test-notes/notes.py search -- "milk"
```

The CLI uses `pluto/tests/test-notes/.notes.sqlite3` for sample data.

### Add an MCP server

In **Settings → MCP servers**, select a transport:

- **Local process (stdio)**. Enter the installed command and one argument per line.
- **Remote HTTP (Streamable HTTP)**. Enter the MCP endpoint and the bearer token variable name if the server requires it.

Click **Test connection** to see which tools the server exposes. Click **Add MCP server**, then **Save settings**. To recheck an existing server, click **Test** next to it. Standalone tests run on your Mac. VM tests run inside the connected VM.

Small catalogs are available directly. For larger catalogs, Pluto lets the model use `find_tools` to load the best match as needed. The search can target a specific server.

For Slack, register a local MCP server that handles its own credentials, or supply a bearer token obtained through Slack’s OAuth setup for its hosted endpoint. Pluto expects the credentials to be ready before connecting. [Slack MCP setup](https://docs.slack.dev/ai/slack-mcp-server/)

For web search, connect a web-search MCP server using the same steps.

#### MCP environment variables on macOS

When you open Pluto Desktop from Spotlight or Finder, use `launchctl setenv` to make variables available to apps launched in your current macOS login session. For example, if `MCP_TOKEN` is already exported in Terminal, run:

```sh
launchctl setenv MCP_TOKEN "$MCP_TOKEN"
```

Quit and reopen Pluto Desktop. For a remote MCP server, enter `MCP_TOKEN` in **Bearer token environment variable**. For a local MCP server, use the variable name that server expects. Other apps launched in the same session can also inherit the value. Run the command again after signing out and back in, or use `launchctl unsetenv MCP_TOKEN` to remove it for future launches.

### Approvals and live activity

In both modes, Pluto asks before running a terminal command or calling an MCP tool. Chat shows the tool name, action, elapsed time, and approval status below your latest message.

| Approval | What it allows |
| --- | --- |
| **Run once** | Approves the current call. |
| **Run always** for a terminal command | Approves the same command and arguments in the same working directory. For commands using `--` to separate fixed arguments from operands, you can optionally let those operand values vary while keeping their count fixed. |
| **Run always** for MCP | Approves future calls to that tool on the same server configuration, including calls with different arguments. |
| **Auto-approve tool calls (at your own risk)** | Allows terminal and MCP calls without prompting. It is off by default and takes effect for new messages after **Save settings**. |

Manage these options under **Settings → Tool approvals**. Use **Saved approvals** to forget individual rules for the current connection. Automatic approvals appear as **Auto-approved** in Chat and in the debug log.

## Local files and troubleshooting

In standalone mode, Desktop stores the files below in `~/Library/Application Support/pluto/` on macOS or under `pluto/` in the OS user config directory. Set `PLUTO_CONFIG` to choose another settings file. Chats and logs sit beside it.

In VM mode, Pluto keeps the workspace, conversation, model settings, and logs inside Pluto VM. [Pluto VM files and diagnostics](pluto/vm/README.md#files-and-diagnostics)

| File or folder | Contents |
| --- | --- |
| `tools.toml` | Model, workspace, registered skills and MCP servers, and approval settings |
| `chats/` and `chats/archive/` | Recent and archived chat transcripts |
| `agent-debug.jsonl` | Background diagnostics for model requests, tool calls, approvals, timings, and outcomes |

The first launch migrates the older default data folder if the new folder is absent.

If a batch would exceed **128 tool calls** in a message, Pluto skips that batch and asks the model to summarize what it completed, where it stopped, and what to do next. The reply stays in the chat so you can ask Pluto to continue.

### Check diagnostics

Send a chat message to create the log. On macOS, watch the default log with the command below. Press **Ctrl+C** to stop.

```sh
tail -f "$HOME/Library/Application Support/pluto/agent-debug.jsonl"
```

If you have `jq`, use this to show one line per event for the turn that most recently wrote to the current log. The command groups events by `process` and `turn`.

```sh
jq -rs '
  last as $latest
  | .[]
  | select(.process == $latest.process and .turn == $latest.turn)
  | [.event, (.details | tojson)]
  | @tsv
' "$HOME/Library/Application Support/pluto/agent-debug.jsonl"
```

If you set `PLUTO_CONFIG`, use the `agent-debug.jsonl` beside that settings file. The log records metadata and names and omits prompts, raw arguments, credentials, and tool output. Identical argument text shares an `arguments_id` within a turn. Pluto Desktop keeps the current log and `agent-debug.previous.jsonl`, up to 2 MiB each.

For a saved chat, click **Copy chat ID** beside the Chat heading. The ID matches the filename stem in `chats/` or `chats/archive/` and appears as `chat_id` on each new event. Filter by that ID to see all turns still in the logs.

```sh
jq --arg chat_id 'PASTE_CHAT_ID_HERE' 'select(.chat_id == $chat_id)' \
  "$HOME/Library/Application Support/pluto/agent-debug.jsonl"
```

- For model switching, `model_fallback` records the source, destination, and trigger, and `model_fallback_unavailable` records when no eligible backup was found. Later model request events show whether the selected model responded.
- To follow tool discovery, look for `tool_requested` with `"tool":"find_tools"`. The `mcp_tools_matched` event lists matched aliases for that batch when there are any. The next `model_request` reports `tools_offered`.
- For a background job, `tool_finished` with `"outcome":"returned"` marks when the tool returned its response. A status check, if the model requests one, appears as a separate tool call.

## Desktop benchmark

In a baseline on Apple Silicon, the Pluto Desktop release executable was **18.6 MiB** and peaked at **56 MiB RAM** in a 10-second sandboxed launch with fresh settings and no model connected. This was a Rust 1.100.0 release build on macOS 26.7.3, before the package split. [Sandboxing research and VM measurements](research/SANDBOXING.md#4-what-the-csv-smoke-test-verified)

## Packages

[Pluto](pluto/) and [Pluto Desktop](pluto-desktop/) each have their own Cargo manifest and lockfile. Pluto builds on its own for the VM. Pluto Desktop uses the sibling Pluto library for standalone mode and the shared API types. [Project map and build commands](DEVELOPMENT.md#package-boundaries)

## Contribute

Tried Pluto with an open model or a workflow you use regularly? Share the model, runtime, OS, and steps you used in a GitHub issue. Reproducible setups and bug reports help others get started. For code contributions, the development guide maps features to their source files and focused tests. [GitHub issues](https://github.com/BlackRabbitt/pluto/issues) · [Development guide](DEVELOPMENT.md)
