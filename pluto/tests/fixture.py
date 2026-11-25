"""An MCP server and scripted model for the end-to-end cargo test."""
import json
import sys
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit


def mcp(request):
    method = request["method"]
    if "id" not in request:
        return None
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "lookup",
            "description": "Look up the fixture",
            "inputSchema": {"type": "object"},
        }]}
    elif method == "tools/call":
        assert request["params"]["name"] == "lookup"
        result = {"content": [{"type": "text", "text": "fixture lookup succeeded"}]}
    else:
        raise AssertionError(method)
    return {"jsonrpc": "2.0", "id": request["id"], "result": result}


def tool_call(step, name, arguments):
    return {"choices": [{"message": {"role": "assistant", "content": None, "tool_calls": [{
        "id": f"call_{step}", "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)},
    }]}}]}


def tool_discovery_model(request):
    names = [tool["function"]["name"] for tool in request["tools"]]
    visible = [name for name in names if name.startswith("mcp_")]
    results = [item["content"] for item in request["messages"] if item["role"] == "tool"]
    assert "find_tools" in names
    if not results:
        assert not visible
        return tool_call(0, "find_tools", {"query": "lookup", "server": "fixture"})
    if len(results) == 1:
        matches = json.loads(results[0])["matches"]
        assert [match["server"] for match in matches] == ["fixture"]
        assert visible == [matches[0]["name"]]
        return tool_call(1, "find_tools", {"query": "secondary lookup"})
    if len(results) == 2:
        fixture_tool = json.loads(results[0])["matches"][0]["name"]
        matches = json.loads(results[1])["matches"]
        assert [match["server"] for match in matches] == ["secondary"]
        assert set(visible) == {fixture_tool, matches[0]["name"]}
        return tool_call(2, fixture_tool, {})
    assert len(results) == 3 and "fixture lookup succeeded" in results[-1]
    return {"choices": [{"message": {"content": "MCP discovery fixture passed"}}]}


def model(request):
    system_prompt = request["messages"][0]["content"]
    clock_lines = [line for line in system_prompt.splitlines() if line.startswith("Current local date and time: ")]
    assert len(clock_lines) == 1
    local_time = datetime.fromisoformat(clock_lines[0].removeprefix("Current local date and time: "))
    assert local_time.utcoffset() is not None
    assert abs((datetime.now(timezone.utc) - local_time).total_seconds()) < 60
    assert any(line.startswith("Local time zone: ") and line.removeprefix("Local time zone: ").strip()
               for line in system_prompt.splitlines())
    if request["model"] in ("model-retry-fixture", "rate-limited-fixture", "fallback-ready",
                            "local-rate-limited", "local-resume"):
        messages = request["messages"][1:]
        assert messages[0] == {"role": "user", "content": "Run the fixture"}
        if len(messages) == 1:
            return tool_call(0, "terminal", {"command": "printf", "args": ["%s", "retry fixture output"]})
        assert len(messages) == 3
        call, result = messages[1:]
        assert call["role"] == "assistant"
        assert result["role"] == "tool"
        assert result["tool_call_id"] == call["tool_calls"][0]["id"] == "call_0"
        assert json.loads(result["content"])["stdout"] == "retry fixture output"
        if request["model"] == "fallback-ready":
            assert "reasoning_effort" not in request
        return {"choices": [{"message": {"content": "Reused the tool result"}}]}
    if request["model"] == "global-approval-fixture":
        results = [item["content"] for item in request["messages"] if item["role"] == "tool"]
        if not results:
            return tool_call(0, "terminal", {"command": "printf", "args": ["%s", "global approval"]})
        assert json.loads(results[0])["stdout"] == "global approval"
        if len(results) == 1:
            lookup = next(tool["function"]["name"] for tool in request["tools"]
                          if tool["function"]["name"].endswith("_fixture_lookup"))
            return tool_call(1, lookup, {})
        assert len(results) == 2 and "fixture lookup succeeded" in results[1]
        return {"choices": [{"message": {"content": "Global approval fixture passed"}}]}
    if request["model"].startswith("tool-limit-"):
        limit = 128
        if len(request["messages"]) >= 4 and request["messages"][3]["content"] == "Continue":
            assert request["model"] == "tool-limit-overflow-fixture"
            assert request["messages"][1]["content"] == "Run the fixture"
            handoff = request["messages"][2]
            assert handoff["role"] == "assistant"
            assert "skipped the entire pending batch of 2 calls" in handoff["content"]
            assert "**Next step** · Print `pending-marker`." in handoff["content"]
            if len(request["messages"]) == 4:
                return tool_call(0, "terminal", {"command": "printf", "args": ["%s", "pending-marker"]})
            assert len(request["messages"]) == 6
            assert json.loads(request["messages"][-1]["content"])["stdout"] == "pending-marker"
            return {"choices": [{"message": {"content": "Resumed from the handoff"}}]}
        count = sum(item["role"] == "tool" for item in request["messages"])
        if "tools" not in request:
            assert request["model"] != "tool-limit-fixture" and count == limit
            assert "Give a partial answer" in system_prompt
            assert all(heading in system_prompt for heading in ("**Completed**", "**Stopped at**", "**Next step**"))
            assert "pending-marker" in system_prompt
            if request["model"] == "tool-limit-no-summary-fixture":
                return tool_call(count, "find_tools", {"query": "ignored"})
            summary = (f"**Completed** · Summary of the {limit} completed requests.\n\n"
                       "**Stopped at** · The marker has not been printed.\n\n"
                       "**Next step** · Print `pending-marker`.")
            return {"choices": [{"message": {"content": summary}}]}
        if count == limit and request["model"] == "tool-limit-fixture":
            return {"choices": [{"message": {"content": f"{limit} tool calls completed"}}]}
        reply = tool_call(count, "find_tools", {"query": "fixture"})
        second = (tool_call(count + 1, "terminal", {"command": "printf", "args": ["%s", "pending-marker"]})
                  if count == limit else tool_call(count + 1, "find_tools", {"query": "fixture"}))
        reply["choices"][0]["message"]["tool_calls"].extend(second["choices"][0]["message"]["tool_calls"])
        return reply
    if request["model"] == "picked-mcp-fixture":
        names = [tool["function"]["name"] for tool in request["tools"]]
        visible = [name for name in names if name.startswith("mcp_")]
        assert len(visible) == 2
        selected = [next(name for name in visible if name.endswith(f"_{server}_lookup"))
                    for server in ("fixture", "secondary")]
        assert all(name in system_prompt for name in selected)
        results = [item["content"] for item in request["messages"] if item["role"] == "tool"]
        assert all("fixture lookup succeeded" in result for result in results)
        if len(results) < len(selected):
            return tool_call(len(results), selected[len(results)], {})
        assert len(results) == len(selected)
        return {"choices": [{"message": {"content": "Picked MCP fixture passed"}}]}
    if request["model"] == "picked-skill-fixture":
        messages = request["messages"]
        call_ids = set()
        for index, (name, instructions) in enumerate([
            ("test-notes", "tests/test-notes/notes.py"),
            ("summary", "Use a bullet list."),
        ]):
            call, result = messages[2 + index * 2:4 + index * 2]
            preload = call["tool_calls"][0]
            assert result["role"] == "tool"
            assert preload["id"] == result["tool_call_id"]
            assert preload["id"] not in call_ids
            call_ids.add(preload["id"])
            assert preload["function"]["name"] == "load_skill"
            assert json.loads(preload["function"]["arguments"]) == {"name": name}
            assert instructions in result["content"]
        if len(messages) == 6:
            return tool_call(0, "terminal", {"command": "python3", "args": ["tests/test-notes/notes.py", "list"]})
        assert len(messages) == 8
        output = json.loads(messages[-1]["content"])
        assert output["exit_code"] == 0
        assert json.loads(output["stdout"]) == [{"id": 1, "title": "Shopping"}]
        return {"choices": [{"message": {"content": "Picked skill fixture passed"}}]}
    if request["model"] == "find-tools-fixture":
        return tool_discovery_model(request)
    if request["model"] == "toggle fixture":
        assert "reasoning_effort" not in request
        assert request["reasoning_format"] == "auto"
        enabled = request["chat_template_kwargs"]["enable_thinking"]
        assert isinstance(enabled, bool)
        content = "Thinking on" if enabled else "Thinking off"
        return {"choices": [{"message": {"content": content}}]}
    if request["model"] in ("llama fixture", "ollama-fixture"):
        assert request["reasoning_effort"] == {
            "llama fixture": "high",
            "ollama-fixture": "low",
        }[request["model"]]
        return {"choices": [{"message": {"content": "Thinking fixture passed"}}],
                "usage": {"prompt_tokens": 120, "completion_tokens": 8, "total_tokens": 128}}
    assert "reasoning_effort" not in request
    assert "You are Pluto" in request["messages"][0]["content"]
    names = [tool["function"]["name"] for tool in request["tools"]]
    messages = [item for item in request["messages"] if item["role"] == "tool"]
    assert "Notes for the fixture" in request["messages"][0]["content"]
    if not messages:
        name, arguments = "load_skill", {"name": "notes"}
    elif len(messages) == 1:
        assert "Use printf" in messages[-1]["content"]
        name, arguments = "terminal", {"command": "printf", "args": ["%s", "cli fixture ; |"]}
    elif len(messages) == 2:
        output = json.loads(messages[-1]["content"])
        assert output["stdout"] == "cli fixture ; |" and output["exit_code"] == 0
        name, arguments = next(n for n in names if n.endswith("_local_lookup")), {}
    elif len(messages) == 3:
        assert "fixture lookup succeeded" in messages[-1]["content"]
        name, arguments = next(n for n in names if n.endswith("_local_lookup")), {"query": "another lookup"}
    elif len(messages) == 4:
        assert "fixture lookup succeeded" in messages[-1]["content"]
        name, arguments = next(n for n in names if n.endswith("_remote_lookup")), {}
    elif len(messages) == 5:
        assert "fixture lookup succeeded" in messages[-1]["content"]
        name, arguments = "terminal", {"command": "printf", "args": ["should be declined"]}
    else:
        assert len(messages) == 6 and "declined" in messages[-1]["content"]
        return {"choices": [{"message": {"role": "assistant", "content": "Pluto fixture passed"}}]}
    return tool_call(len(messages), name, arguments)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlsplit(self.path)
        if parsed.path == "/v1/models":
            body = {"data": [{"id": "llama fixture"}]}
        elif parsed.path == "/compatible/v1/models":
            body = {"data": [
                {"id": "fixture/model", "context_length": 131072},
                {"id": "fixture/zero", "context_length": 0},
                {"id": "fixture/unknown"},
            ]}
        elif parsed.path == "/fallback/v1/models":
            assert not getattr(self.server, "fallback_catalog_read", False)
            self.server.fallback_catalog_read = True
            body = {"data": [
                {"id": "fallback-paid", "context_length": 32768, "supported_parameters": ["tools"],
                 "pricing": {"prompt": "0.001"}},
                {"id": "fallback-small", "context_length": 1024, "supported_parameters": ["tools"]},
                {"id": "fallback-no-tools", "context_length": 32768, "supported_parameters": ["temperature"]},
                {"id": "fallback-ready", "context_length": 32768, "supported_parameters": ["tools"],
                 "pricing": {"prompt": "0", "completion": "0"}},
                {"id": "fallback-quota", "context_length": 32768, "supported_parameters": ["tools"]},
            ]}
        elif parsed.path == "/props":
            selected = parse_qs(parsed.query)["model"]
            if selected in (["toggle fixture"], ["fixed fixture"]):
                body = {"chat_template": "{% if enable_thinking %}...{% endif %}",
                        "chat_template_caps": {"reasoning_effort_levels": []}}
            else:
                assert selected == ["llama fixture"]
                body = {"chat_template_caps": {
                    "reasoning_effort_levels": ["low", "medium", "high"],
                    "reasoning_effort_rejected": ["medium"],
                }, "default_generation_settings": {"n_ctx": 8192}}
        else:
            assert parsed.path == "/api/tags"
            body = {"models": [{"name": "ollama-fixture", "model": "ollama-fixture"}]}
        self.respond(body)

    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/mcp":
            body = mcp(payload)
        elif self.path == "/api/show":
            assert payload == {"model": "ollama-fixture"}
            body = {"thinking": {"values": [False, True], "default": True}}
        elif self.path == "/apply-template":
            assert payload["model"] in ("toggle fixture", "fixed fixture")
            enabled = payload["chat_template_kwargs"]["enable_thinking"]
            assert isinstance(enabled, bool)
            assert payload["reasoning_format"] == "auto"
            prompt = "thinking" if enabled and payload["model"] == "toggle fixture" else "answer"
            body = {"prompt": prompt}
        else:
            assert self.path in ("/v1/chat/completions", "/fallback/v1/chat/completions")
            if self.path == "/fallback/v1/chat/completions":
                assert payload["model"] in ("fallback-ready", "fallback-quota")
            if (payload["model"] in ("rate-limited-fixture", "fallback-quota", "local-rate-limited")
                    and payload["messages"][-1]["role"] == "tool"):
                self.send_response(429)
                self.send_header("Retry-After", "0" if payload["model"] == "fallback-quota" else "120")
                self.end_headers()
                return
            body = model(payload)
            if payload["model"] == "model-retry-fixture" and payload["messages"][-1]["role"] == "tool":
                retry_request = {**payload, "messages": payload["messages"][1:]}
                if self.server.failed_model_request is None:
                    self.server.failed_model_request = retry_request
                    self.send_error(503)
                    return
                assert retry_request == self.server.failed_model_request
        self.respond(body)

    def respond(self, body):
        self.send_response(202 if body is None else 200)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        self.end_headers()
        if body is not None:
            self.wfile.write(json.dumps(body).encode())

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    if sys.argv[1] == "stdio":
        for line in sys.stdin:
            response = mcp(json.loads(line))
            if response is not None:
                print(json.dumps(response), flush=True)
    else:
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.failed_model_request = None
        print(server.server_port, flush=True)
        server.serve_forever()
