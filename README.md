# Pluto

A Rust desktop chatbot for a local or remote tool-capable model. The model can read installed skills, run commands found on the system PATH, and use local or remote MCP tools. Chats are saved locally.

## Run

Pluto is built and tested with **Rust 1.100.0**. Install that version and start a tool-capable model server. Then run from this directory.

```sh
cargo run --locked
```

In **Settings**, select **llama.cpp**, **Ollama**, or **OpenAI-compatible**. Enter your server URL, click **Load models**, and select a model. You can also enter its name directly. For llama.cpp and Ollama, use the server base URL. For other servers, use their OpenAI-compatible base URL, including any required path such as `/v1`. If the server requires authentication, enter the name of the environment variable containing its token.

Set the working directory for terminal commands, save settings, and switch to **Chat**. The **Thinking** menu shows the selected model’s supported levels or an **On/Off** switch when the model offers only a toggle. **Server default** uses the server’s own behavior. The selection applies to subsequent messages with that model.

On Linux, the app needs a graphical desktop and working OpenGL drivers. On macOS, launch it from a terminal if your CLI or model credentials depend on your shell environment.

### Use Rust 1.100.0 only for Pluto

With Rustup installed, run these commands from the Pluto folder.

```sh
rustup toolchain install 1.100.0
rustup override set 1.100.0
rustc --version
```

`rustc --version` should report `rustc 1.100.0`. The override applies to this folder and its subfolders. To remove it, run `rustup override unset` from the Pluto folder.

To share the pin with everyone who clones the repository, create and commit a `rust-toolchain.toml` file at the project root with these contents.

```toml
[toolchain]
channel = "1.100.0"
```

## Chat history

Open a saved conversation from **Recent chats** in the sidebar or select **New chat** to start another one. The app reopens the most recently updated chat on launch. Sent messages and completed replies are saved as plain JSON in a `chats` folder beside `tools.toml`.

## Add a skill and use a CLI

1. Install the app's CLI independently and make sure the command is on the PATH inherited by Pluto.
2. In **Settings → Skills**, enter the local skill directory or its `SKILL.md`. Select **Add skill** and save.
3. Ask the assistant to do something described by the skill. Pluto shows the executable and arguments before running it, then returns the exit code, stdout, and stderr to the model.

Pluto registers a path to the skill and reads its instructions when requested. Keep the skill folder available. For a skill from a repository, check it out locally and register that folder. The terminal tool runs an executable with separate arguments in the configured working directory. It is noninteractive and runs with your account's privileges. An explicit shell command can be approved if you need pipes or redirects.

For repeat CLI calls, choose **Run always** to save approval for the exact command in that working directory. For commands that use `--` after their fixed arguments, you can also allow the values after `--` to vary while keeping their count fixed. Matching calls show **Auto-approved** in Chat. You can remove a rule under **Settings → Saved CLI approvals**.

### Update a skill

| What changed | What to do |
| --- | --- |
| Instructions in `SKILL.md` | Save the file in place. Pluto reads the updated instructions the next time the model loads the skill, even in the same chat. |
| Skill description | In **Settings → Skills**, add the same file again and click **Save settings**. This refreshes the description the model sees when choosing a skill. |
| Skill name or file location | Remove the old entry in **Settings → Skills**, add the updated file, and click **Save settings**. |
| Skill files in a GitHub repository | Update your local copy first. Then follow the row above for what changed. |

To try an update right away, ask Pluto to use the skill by name in your next message.

### Try the test notes app

With Python 3 installed, run these commands from the project root:

```sh
python3 tests/test-notes/notes.py add -- "Shopping" "Buy milk"
python3 tests/test-notes/notes.py search -- "milk"
```

To try it in Chat, set **Terminal working directory** to the project root. Register [tests/test-notes/SKILL.md](tests/test-notes/SKILL.md) using its full local path, save settings, and ask “Create a test note titled Shopping with the body Buy milk.” The CLI stores sample data in `tests/test-notes/.notes.sqlite3`.

## Add an MCP server

In **Settings → MCP servers**, select a transport and enter the server details.

- **Local process** uses stdio. Enter the launch command and one argument per line. The command must be installed separately.
- **Remote HTTP** uses Streamable HTTP. Enter the MCP endpoint URL and, when required, the name of an environment variable holding its bearer token. Set the variable before launching the app.

Select **Add MCP server** and save. Chat discovers each server's tools and shows any connection errors. The app asks for approval before calling a tool. Install or authorize the server using its own instructions.

For Slack, register a configured Slack MCP server using either transport. Slack's [official hosted endpoint](https://docs.slack.dev/ai/slack-mcp-server/) requires a registered Slack app and OAuth. This build accepts a caller-supplied bearer token for compatible HTTP servers. It does not run the Slack authorization flow or refresh credentials. A Slack MCP server that handles its own credentials can be registered as a local process.

A web-search MCP server can be registered in the same way. There is no separate built-in browser or web-search implementation.

The configuration is stored as `pluto/tools.toml` inside the OS user config directory. You can edit it by hand while the app is closed or set `PLUTO_CONFIG` to load a different file. On first launch, Pluto moves the previous default settings and chats if the new data folder does not already exist.
