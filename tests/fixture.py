"""An MCP server and scripted model for the end-to-end cargo test."""
import json
import sys
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


def model(request):
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
    return {"choices": [{"message": {"role": "assistant", "content": None, "tool_calls": [{
        "id": f"call_{len(messages)}", "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)},
    }]}}]}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlsplit(self.path)
        if parsed.path == "/v1/models":
            body = {"data": [{"id": "llama fixture"}]}
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
            assert self.path == "/v1/chat/completions"
            body = model(payload)
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


if sys.argv[1] == "stdio":
    for line in sys.stdin:
        response = mcp(json.loads(line))
        if response is not None:
            print(json.dumps(response), flush=True)
else:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()
