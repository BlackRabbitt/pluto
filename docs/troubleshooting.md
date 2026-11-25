# Local files and troubleshooting

[Home](../README.md) · [Pluto VM diagnostics](../pluto/vm/README.md#files-and-diagnostics)

| What you’re checking | Where to look |
| --- | --- |
| Settings, chat files, or logs | [Local files](#local-files) |
| Model server isn’t reachable from macOS | [Local network access](models.md#connect-to-a-model-on-your-local-network-from-macos) |
| MCP server can’t read an environment variable | [MCP environment variables on macOS](tools.md#mcp-environment-variables-on-macos) |
| A tool call or model switch | [Check diagnostics](#check-diagnostics) |

## Local files

In standalone mode, Desktop stores the files below in `~/Library/Application Support/pluto/` on macOS or under `pluto/` in the OS user config directory. Set `PLUTO_CONFIG` to choose another settings file. Chats and logs sit beside it.

In VM mode, Pluto keeps the workspace, conversation, model settings, and logs inside Pluto VM. [Pluto VM files and diagnostics](../pluto/vm/README.md#files-and-diagnostics)

| File or folder | Contents |
| --- | --- |
| `tools.toml` | Model, workspace, registered skills and MCP servers, and approval settings |
| `chats/` and `chats/archive/` | Recent and archived chat transcripts |
| `agent-debug.jsonl` | Background diagnostics for model requests, tool calls, approvals, timings, and outcomes |

The first launch migrates the older default data folder if the new folder is absent.

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
