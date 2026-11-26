# Use chat

[Home](../README.md) · [Connect a model](models.md) · [Skills and tools](tools.md)

Both modes use the same chat controls. Click **New chat** to start fresh, or use **Recent chats** and **Archived chats** to reopen a conversation. Each VM keeps its own chats, approvals, and workspace. Update Pluto Desktop and the Pluto service to get the full set of VM chat controls. [VM updates](../pluto/vm/README.md#update-pluto-vm)

- Use the **Thinking** menu to select a level or toggle when llama.cpp or Ollama reports support. **Server default** uses the server’s own behavior. The choice applies to subsequent messages with that model.
- While waiting for a model response, Chat shows a spinner and the current phase: **Thinking…**, **Reviewing tool results…**, or **Writing a partial answer…**. When a model includes a leading `<think>` block in its reply, expand **Model thinking** to see it.
- Chat shows token usage for the last model request. With llama.cpp, the bar shows input and output tokens against the server's configured context size. With an OpenAI-compatible server that reports a context limit, the bar shows input tokens against that limit. You can also set **Context limit (input tokens)** in Settings for Ollama or OpenAI-compatible servers.
- Pluto sends the full transcript of the open chat and the computer’s local date, time, and time zone to the configured model. New messages show a timestamp below the sender name.
- Pluto opens the most recently updated active chat on launch. Use **Archive chat** to move it out of **Recent chats** and **Restore chat** to continue it. Use **Copy chat ID** to find its diagnostic events.

## Long tool runs

If a batch would exceed **128 tool calls** in a message, Pluto skips that batch and asks the model to summarize what it completed, where it stopped, and what to do next. The reply stays in the chat so you can ask Pluto to continue.

See [approvals and live activity](tools.md#approvals-and-live-activity) to follow a tool call or [diagnostics](troubleshooting.md#check-diagnostics) to inspect a saved turn.
