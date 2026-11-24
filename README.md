# Pluto

<img src="assets/pluto-icon.png" alt="Pluto Evergreen icon" width="112" height="112">

Pluto is a free, local-first Rust desktop chatbot for free and open models. Run a model with llama.cpp or Ollama, or connect to an OpenAI-compatible server hosting an open model. Pluto can load skills, run terminal commands, and call MCP tools. Chats are saved as local JSON files.

## Run Pluto

Use **Rust 1.100.0** and a model that supports tool calls. From the Pluto folder, run:

```sh
cargo run --locked
```

On Linux, Pluto needs a graphical desktop and working OpenGL drivers. On macOS, launch it from a terminal if Pluto needs your shell’s API keys or access to CLI commands on your PATH.

### Install on macOS from a source archive

1. If the Xcode Command Line Tools are missing, run `xcode-select --install` in Terminal and finish the installer. If Rustup is missing, install it and open a new Terminal window. [Apple Command Line Tools](https://developer.apple.com/documentation/xcode/installing-the-command-line-tools) · [Rustup](https://rustup.rs/)
2. Download and unzip the [Pluto source ZIP](https://github.com/BlackRabbitt/pluto/archive/refs/heads/main.zip), or use a copy of the source archive you already have. In Terminal, enter the extracted folder that contains `Cargo.toml`.
3. Run the installer. It downloads the required Rust toolchain and build dependencies as needed.

   ```sh
   sh install-macos.sh
   ```

Search for **Pluto** in Spotlight, or open `~/Applications/Pluto.app` in Finder. If an MCP server needs environment variables, see [MCP environment variables on macOS](#mcp-environment-variables-on-macos). You can also run `~/.cargo/bin/pluto` from Terminal to use your shell environment.

### Update on macOS

Quit Pluto, download a fresh source ZIP, and unzip it into a new folder. Run the installer from that folder.

```sh
sh install-macos.sh
```

The installer updates the app in your Applications folder and the Terminal executable even if Pluto’s version number is unchanged. It also replaces the AppleScript launcher created by earlier installers. Settings and saved chats stay in `~/Library/Application Support/pluto/`, or at the location selected through `PLUTO_CONFIG`.

### Connect a model

1. Open **Settings**. Configure **Local model**, **OpenAI-compatible model**, or both. In each setup, enter the server URL and any required token environment variable. Click **Load models** or enter the model ID directly.
2. Select **Local only** to use the saved llama.cpp or Ollama server for inference. Select **OpenAI-compatible** to try the saved online model first. Pluto tries any enabled free-model fallbacks and uses the saved local model if the cloud options fail.
3. Set **Terminal working directory** if you plan to run commands. Click **Save settings** and open **Chat**.

| Provider | Server URL |
| --- | --- |
| **llama.cpp** | The server base URL, for example `http://localhost:8080` |
| **Ollama** | The server base URL, for example `http://localhost:11434` |
| **OpenAI-compatible** | The API base URL for a server offering a free, open model, including its required path such as `/v1` |

For an authenticated model or MCP server, export the key before launching Pluto. In Settings, enter the **variable name**, such as `MODEL_API_KEY`. If Pluto can load the model list but chat returns a 404, check that your model ID supports text chat on that endpoint.

#### Tested local setup

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

#### Find free cloud models and configure fallbacks

The [FreeLLM registry](https://freellm.net/models/) lists free tiers across providers. Pick a free, openly licensed model and copy its provider's API base URL and exact model ID. Provider quotas and available models can change.

Under **OpenAI-compatible model**, enable **Try other free models automatically** to add up to four backups in preference order. Use **Check model metadata** to see the provider's reported context limit and tool support. Supply verified values when the provider omits them. Confirm each backup is free and openly licensed, then save.

Pluto honors `Retry-After` when an inference server returns HTTP 429. It checks estimated context fit and tool support before picking a cloud backup. Each switched model receives the current chat and tool results so Pluto can continue without repeating completed tools. Chat shows the selected destination.

#### Connect to a model on your local network from macOS

Open the installed **Pluto** app from Spotlight. In Settings, enter the model server’s LAN address and port, then click **Load models**. Use `localhost` when the model server runs on the same Mac. For a server on another device, make sure it listens on its LAN interface and accepts connections on that port.

If macOS asks to allow local network access, choose **Allow**. macOS lists Pluto under **System Settings → Privacy & Security → Local Network** after it attempts a connection that requires permission. If Pluto is already listed, make sure its switch is on. [Apple’s local network privacy guide](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)

Source installs use ad hoc signing by default, so macOS can ask again after a rebuild. If you have an Apple-issued code-signing identity in your keychain, set `PLUTO_SIGNING_IDENTITY` to its name when running the installer. To compare whether the issue is specific to the app’s local network permission, try the same server from `~/.cargo/bin/pluto` in Terminal. macOS grants local network access to command-line tools launched from Terminal. [Apple’s macOS signing and local network guidance](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy#Build-time-considerations)

### Use Rust 1.100.0 only for Pluto

Run these commands from the Pluto folder with Rustup installed:

```sh
rustup toolchain install 1.100.0
rustup override set 1.100.0
rustc --version
```

`rustc --version` should report `rustc 1.100.0`. This override applies to the folder and its subfolders. Run `rustup override unset` there to remove it.

To share the pin with everyone who clones a project, commit a `rust-toolchain.toml` at the project root:

```toml
[toolchain]
channel = "1.100.0"
```

## Chat

- Use the **Thinking** menu to select a level or toggle when llama.cpp or Ollama reports support. **Server default** uses the server’s own behavior. The choice applies to subsequent messages with that model.
- While waiting for a model response, Chat shows a spinner and the current phase: **Thinking…**, **Reviewing tool results…**, or **Writing a partial answer…**. When a model includes a leading `<think>` block in its reply, expand **Model thinking** to see it.
- Chat shows token usage for the last model request. With llama.cpp, the bar shows input and output tokens against the server's configured context size. With an OpenAI-compatible server that reports a context limit, the bar shows input tokens against that limit. You can also set **Context limit (input tokens)** in Settings for Ollama or OpenAI-compatible servers.
- Pluto sends the full transcript of the open chat and the computer’s local date, time, and time zone to the configured model. New messages show a timestamp below the sender name.
- Reopen conversations under **Recent chats**. Pluto opens the most recently updated chat on launch. Use **Archive chat** to move a conversation out of the recent list, and **Restore chat** to continue it.

## Use skills and tools

Type `/` at the start of the message box to pick a skill or MCP tool. Filter by typing, then click a result or use **↑**, **↓**, and **Enter**. Type `/` again to add more skills or MCP tools before writing your request. Use **×** to remove a selection. Selections apply to the next message you send.

### Add or update a skill

1. Install the skill’s CLI separately and make sure Pluto can find it on your PATH.
2. In **Settings → Skills**, enter the full path to a local skill directory or its `SKILL.md`. Choose **Add skill**, then **Save settings**. For GitHub skills, check out the files locally first.
3. Ask Pluto for something the skill can do. When the skill loads, Chat shows its name. Terminal commands run noninteractively from the configured working directory with your account’s privileges.

Pluto passes arguments directly to the executable. Pipes and redirects require an explicit shell command. The model receives the exit code and captured output.

| What changed | What to do |
| --- | --- |
| Instructions in `SKILL.md` | Save the file in place. Pluto reads it the next time it loads the skill, even in the same chat. |
| Description | Add the same file again and click **Save settings** to refresh what the model sees in the skill catalog. |
| Name or path | Remove the old entry, add the updated one, and save. |
| Files in a GitHub repo | Update your local copy, then follow the row for what changed. |

To try the test notes skill, install Python 3, set the terminal working directory to the Pluto folder, and register [tests/test-notes/SKILL.md](tests/test-notes/SKILL.md). Save settings and ask “Create a test note titled Shopping with the body Buy milk.” To try the CLI directly:

```sh
python3 tests/test-notes/notes.py add -- "Shopping" "Buy milk"
python3 tests/test-notes/notes.py search -- "milk"
```

The CLI uses `tests/test-notes/.notes.sqlite3` for sample data.

### Add an MCP server

In **Settings → MCP servers**, select a transport:

- **Local process (stdio)**. Enter the installed command and one argument per line.
- **Remote HTTP (Streamable HTTP)**. Enter the MCP endpoint and the bearer token variable name if the server requires it.

Click **Test connection** to see which tools the server exposes. Click **Add MCP server**, then **Save settings**. To recheck an existing server, click **Test** next to it.

Small catalogs are available directly. For larger catalogs, Pluto lets the model use `find_tools` to load the best match as needed. The search can target a specific server.

For Slack, register a local MCP server that handles its own credentials, or supply a bearer token obtained through Slack’s OAuth setup for its hosted endpoint. Pluto expects the credentials to be ready before connecting. [Slack MCP setup](https://docs.slack.dev/ai/slack-mcp-server/)

For web search, connect a web-search MCP server using the same steps.

#### MCP environment variables on macOS

When you open Pluto from Spotlight or Finder, use `launchctl setenv` to make variables available to apps launched in your current macOS login session. For example, if `MCP_TOKEN` is already exported in Terminal, run:

```sh
launchctl setenv MCP_TOKEN "$MCP_TOKEN"
```

Quit and reopen Pluto. For a remote MCP server, enter `MCP_TOKEN` in **Bearer token environment variable**. For a local MCP server, use the variable name that server expects. Other apps launched in the same session can also inherit the value. Run the command again after signing out and back in, or use `launchctl unsetenv MCP_TOKEN` to remove it for future launches.

### Approvals and live activity

By default, Pluto asks before running a terminal command or calling an MCP tool. Chat shows the tool name, action, elapsed time, and approval status below your latest message.

| Approval | What it allows |
| --- | --- |
| **Run once** | Approves the current call. |
| **Run always** for a terminal command | Approves the same command and arguments in the same working directory. For commands using `--` to separate fixed arguments from operands, you can optionally let those operand values vary while keeping their count fixed. |
| **Run always** for MCP | Approves future calls to that tool on the same server configuration, including calls with different arguments. |
| **Auto-approve tool calls (at your own risk)** | Allows terminal and MCP calls without prompting. It is off by default and takes effect for new messages after **Save settings**. |

Manage these options under **Settings → Tool approvals**. Use **Saved approvals** to forget individual rules. Automatic approvals appear as **Auto-approved** in Chat and in the debug log.

## Local files and troubleshooting

| File or folder | Contents |
| --- | --- |
| `tools.toml` | Model, workspace, registered skills and MCP servers, and approval settings |
| `chats/` and `chats/archive/` | Recent and archived chat transcripts |
| `agent-debug.jsonl` | Background diagnostics for model requests, tool calls, approvals, timings, and outcomes |

These live together under `~/Library/Application Support/pluto/` on macOS, or under `pluto/` in the OS user config directory. Set `PLUTO_CONFIG` to choose a different settings file. Chats and logs sit beside it. On first launch, Pluto moves the previous default data folder if the new one does not yet exist.

If a batch would exceed **128 tool calls** in a message, Pluto skips that batch and asks the model to summarize what it completed, where it stopped, and what to do next. The reply stays in the chat so you can ask Pluto to continue.

### Check diagnostics

Send a chat message to create the log. On macOS, run this in a terminal to watch events as Pluto writes them. Press **Ctrl+C** to stop.

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

If you set `PLUTO_CONFIG`, use the `agent-debug.jsonl` beside that settings file. The log records metadata and names and omits prompts, raw arguments, credentials, and tool output. Identical argument text shares an `arguments_id` within a turn. Pluto keeps the current log and `agent-debug.previous.jsonl`, up to 2 MiB each.

- To follow tool discovery, look for `tool_requested` with `"tool":"find_tools"`. The `mcp_tools_matched` event lists matched aliases for that batch when there are any. The next `model_request` reports `tools_offered`.
- For a background job, `tool_finished` with `"outcome":"returned"` marks when the tool returned its response. A status check, if the model requests one, appears as a separate tool call.

For code changes, see the [development guide](DEVELOPMENT.md).
