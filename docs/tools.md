# Use skills and tools

[Home](../README.md) · [Chat](chat.md)

- [Select a skill or tool in chat](#select-a-skill-or-tool-in-chat)
- [Add or update a skill](#add-or-update-a-skill)
- [Add an MCP server](#add-an-mcp-server)
- [Manage approvals](#approvals-and-live-activity)

The walkthrough below uses standalone mode. In VM mode, connect to the instance you want to edit and open **Settings**. You can register a skill from a VM path, import a portable `SKILL.md` from the Mac, and edit local or remote MCP servers. [Configure a VM in Desktop](../pluto/vm/README.md#configure-a-vm-in-desktop) · [Try the notes skill and MCP fixture in a VM](../pluto/vm/README.md#try-the-notes-skill-and-mcp-fixture)

## Select a skill or tool in chat

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

To try the test notes skill, install Python 3, set Pluto’s terminal working directory to the `pluto/` project folder, and register [the test notes skill](../pluto/tests/test-notes/SKILL.md). Save settings and ask “Create a test note titled Shopping with the body Buy milk.” Try the CLI directly from the repository root.

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

### MCP environment variables on macOS

For a VM connection, follow [VM environment variables](../pluto/vm/README.md#environment-variables-in-pluto-vm).

When you open Pluto Desktop from Spotlight or Finder, use `launchctl setenv` to make variables available to apps launched in your current macOS login session. For example, if `MCP_TOKEN` is already exported in Terminal, run:

```sh
launchctl setenv MCP_TOKEN "$MCP_TOKEN"
```

Quit and reopen Pluto Desktop. For a remote MCP server, enter `MCP_TOKEN` in **Bearer token environment variable**. For a local MCP server, use the variable name that server expects. Other apps launched in the same session can also inherit the value. Run the command again after signing out and back in, or use `launchctl unsetenv MCP_TOKEN` to remove it for future launches.

### Approvals and live activity

In both modes, Pluto asks before running a terminal command or calling an MCP tool. Chat shows the tool name, action, elapsed time, and approval status below your latest message.

Activity rows show the skills Pluto loaded and the tool results the model is reviewing. Click a skill or result row to expand the exact text sent to the model, including any truncation notice. The expanded output scrolls horizontally for long lines. Activity stays available while the chat is open.

| Approval | What it allows |
| --- | --- |
| **Run once** | Approves the current call. |
| **Run always** for a terminal command | Approves the same command and arguments in the same working directory. For commands using `--` to separate fixed arguments from operands, you can optionally let those operand values vary while keeping their count fixed. |
| **Run always** for MCP | Approves future calls to that tool on the same server configuration, including calls with different arguments. |
| **Auto-approve tool calls (at your own risk)** | Allows terminal and MCP calls without prompting. It is off by default and takes effect for new messages after **Save settings**. |

Manage these options under **Settings → Tool approvals**. Use **Saved approvals** to forget individual rules for the current connection. Automatic approvals appear as **Auto-approved** in Chat and in the debug log.
