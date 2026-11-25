"""Run against a disposable, provisioned Pluto Lima VM."""
import json
import signal
import subprocess
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path

from fixture import Handler, tool_call

VM = Path(__file__).resolve().parents[1] / "vm" / "vm"
REPORT = "category,total\nbooks,25\n"
PROMPT = "Summarize vm-smoke.csv and save vm-smoke-report.csv"
ANSWER = "Saved vm-smoke-report.csv with a books total of 25."
FOLLOWUP = "Which report did you save?"
RECALL = "I saved vm-smoke-report.csv."
NEW_PROMPT = "Start a separate conversation."
NEW_ANSWER = "This chat started with an empty history."

MCP = """#!/bin/sh
while IFS= read -r request; do
    id=$(printf '%s' "$request" | sed -n 's/.*"id":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p')
    [ -n "$id" ] || continue
    case "$request" in
        *'"initialize"'*)
            version=$(printf '%s' "$request" | sed -n 's/.*"protocolVersion":[[:space:]]*"\\([^"]*\\)".*/\\1/p')
            result=$(printf '{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"smoke","version":"1"}}' "$version")
            ;;
        *'"tools/list"'*)
            result='{"tools":[{"name":"vm_user","description":"Read the VM tool account","inputSchema":{"type":"object"}}]}'
            ;;
        *'"tools/call"'*)
            result=$(printf '{"content":[{"type":"text","text":"%s in %s"}]}' "$(id -un)" "$(pwd)")
            ;;
        *) exit 1 ;;
    esac
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\\n' "$id" "$result"
done
"""


def response(content):
    return {"choices": [{"message": {"role": "assistant", "content": content}}]}


class Model(Handler):
    def do_GET(self):
        assert self.path == "/v1/models"
        self.respond({"data": [{"id": "updated-vm-fixture", "context_length": 32768},
                               {"id": "another-model"}, {"id": "another-model"}]})

    def do_POST(self):
        assert self.path == "/v1/chat/completions"
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert request["model"] == "updated-vm-fixture"
        messages = request["messages"]
        latest = [message["content"] for message in messages if message["role"] == "user"][-1]
        if latest == NEW_PROMPT:
            assert [(message["role"], message["content"]) for message in messages
                    if message["role"] in ("user", "assistant")] == [("user", NEW_PROMPT)]
            self.respond(response(NEW_ANSWER))
            return
        if latest == FOLLOWUP:
            assert [message["content"] for message in messages[-3:]] == [PROMPT, ANSWER, FOLLOWUP]
            self.respond(response(RECALL))
            return
        assert latest == PROMPT
        pos = max(index for index, message in enumerate(messages) if message["role"] == "user")
        results = [message["content"] for message in messages[pos:] if message["role"] == "tool"]
        if not results:
            self.respond(tool_call(0, "terminal", {
                "command": "/bin/sh",
                "args": ["-ec", """
                    test "$(uname -s)" = Linux
                    test "$(id -un)" = pluto-tool
                    test ! -r /var/lib/pluto/tools.toml
                    test ! -x /run/pluto
                    awk -F, 'NR > 1 { total += $2 } END { printf "category,total\\nbooks,%d\\n", total }' vm-smoke.csv > vm-smoke-report.csv
                    cat vm-smoke-report.csv
                """],
            }))
            return
        terminal = json.loads(results[0])
        assert terminal["exit_code"] == 0 and terminal["stdout"] == REPORT, terminal
        if len(results) == 1:
            alias = next(tool["function"]["name"] for tool in request["tools"]
                         if tool["function"]["name"].endswith("_smoke_vm_user"))
            self.respond(tool_call(1, alias, {}))
            return
        assert "pluto-tool in /var/lib/pluto-workspace" in results[1], results[1]
        if len(results) == 2:
            self.respond(tool_call(2, "terminal", {
                "command": "touch", "args": ["vm-smoke-declined"],
            }))
            return
        assert len(results) == 3 and "user declined" in results[2], results
        self.respond(response(ANSWER))


def vm(*args, **kwargs):
    return subprocess.run([VM, *args], check=True, **kwargs)


def api(request, approvals=()):
    approvals = iter(approvals)
    history = None
    chats = None
    with subprocess.Popen([VM, "api"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True) as child:
        def send(message):
            child.stdin.write(json.dumps(message) + "\n")
            child.stdin.flush()

        send(request)
        for line in child.stdout:
            event = json.loads(line)
            kind = event["type"]
            if kind == "approval":
                send({"type": "decision", "id": event["id"], "accept": next(approvals)})
            elif kind == "history":
                history = event
                chats = event.get("chats", chats)
            elif kind == "error":
                raise AssertionError(event["message"])
            elif kind == "complete":
                assert next(approvals, None) is None
                assert history is not None
                assert child.wait(timeout=10) == 0
                return history.get("chat_id"), history["history"]["messages"], chats
        raise AssertionError("API disconnected before completing the request")


def first_response(request, expected="model_settings"):
    response = vm("api", input=json.dumps(request) + "\n", capture_output=True, text=True)
    events = [json.loads(line) for line in response.stdout.splitlines()]
    assert events[0]["type"] == expected, events
    return events[0]


def smoke():
    with tempfile.TemporaryDirectory() as directory, ThreadingHTTPServer(("127.0.0.1", 0), Model) as model:
        threading.Thread(target=model.serve_forever, daemon=True).start()
        port = str(model.server_port)
        directory = Path(directory)
        csv = directory / "input.csv"
        csv.write_text("category,amount\nbooks,10\nbooks,15\n")
        mcp = directory / "fixture.sh"
        mcp.write_text(MCP)
        config = directory / "tools.toml"
        config.write_text(f"""
[models]
mode = "open_ai_compatible"
[models.openai]
url = "http://127.0.0.1:{port}/v1"
name = "vm-fixture"
""")
        vm("start")
        vm("configure", config)
        previous = first_response({"type": "model_settings"})["models"]
        models = {**previous, "openai": {**previous["openai"], "name": "updated-vm-fixture"}}
        update = {"type": "save_model_settings", "previous": previous, "models": models}
        assert first_response(update)["models"] == models
        assert "Reload" in first_response(update, expected="error")["message"]
        skill = first_response({"type": "import_skill", "text": "---\nname: vm-smoke-skill\ndescription: A portable test skill\n---\nSummarize the report.\n"}, expected="skill")
        assert first_response({"type": "inspect_skill", "path": skill["skill"]["path"]}, expected="skill") == skill
        before = first_response({"type": "vm_settings"}, expected="vm_settings")["settings"]
        fallback = {"url": models["openai"]["url"], "model": "updated-vm-fixture",
                    "token_env": "", "context_limit": 0, "verified_tools": False, "free_open": True}
        settings = {**before,
                    "model_fallbacks": {"enabled": False, "models": [fallback]},
                    "skills": {skill["name"]: skill["skill"]},
                    "mcp": {"smoke": {"transport": "stdio", "command": "/bin/sh", "args": ["vm-smoke-mcp.sh"]}}}
        save = {"type": "save_vm_settings", "previous": before, "settings": settings}
        assert first_response(save, expected="vm_settings")["settings"] == settings
        assert "Reload" in first_response(save, expected="error")["message"]
        vm("import", csv, "vm-smoke.csv")
        vm("import", mcp, "vm-smoke-mcp.sh")
        with subprocess.Popen([VM, "forward-model", port]) as tunnel:
            try:
                time.sleep(1)
                assert tunnel.poll() is None, "Model tunnel exited"
                catalog = first_response({"type": "list_models", "mode": "open_ai_compatible", "profile": models["openai"]}, expected="models")
                assert catalog["names"] == ["another-model", "updated-vm-fixture"]
                metadata = first_response({"type": "check_fallback_model", "model": fallback}, expected="fallback_model_metadata")
                assert metadata["metadata"]["context_window"] == 32768
                original_id, saved, _ = api({"type": "send_chat", "chat_id": None, "prompt": PROMPT}, (True, True, False))
                new_id, new_messages, chats = api({"type": "send_chat", "chat_id": None, "prompt": NEW_PROMPT})
            finally:
                if tunnel.poll() is None:
                    tunnel.terminate()

        assert saved[-2:] == [["user", PROMPT], ["assistant", ANSWER]], saved
        assert original_id and new_id and original_id != new_id
        assert new_messages == [["user", NEW_PROMPT], ["assistant", NEW_ANSWER]]
        assert [chat["id"] for chat in chats[:2]] == [new_id, original_id]
        vm("stop")
        started = time.monotonic()
        vm("start")
        latest_id, latest, _ = api({"type": "history"})
        print(f"Cold restart through API response: {time.monotonic() - started:.2f} seconds", flush=True)
        assert (latest_id, latest) == (new_id, new_messages)
        reopened_id, reopened, chats = api({"type": "open_chat", "chat_id": original_id})
        assert (reopened_id, reopened) == (original_id, saved)
        assert [chat["id"] for chat in chats[:2]] == [new_id, original_id]
        assert "not found" in first_response({"type": "open_chat", "chat_id": "../tools"}, expected="error")["message"]
        assert first_response({"type": "model_settings"})["models"] == models
        assert first_response({"type": "vm_settings"}, expected="vm_settings")["settings"] == settings
        report = vm("export", "vm-smoke-report.csv", capture_output=True, text=True)
        assert report.stdout == REPORT, report.stdout
        declined = subprocess.run([VM, "export", "vm-smoke-declined"], capture_output=True)
        assert declined.returncode != 0
        with subprocess.Popen([VM, "forward-model", port]) as tunnel:
            try:
                time.sleep(1)
                assert tunnel.poll() is None, "Model tunnel exited"
                continued_id, continued, _ = api({"type": "send_chat", "chat_id": original_id, "prompt": FOLLOWUP})
            finally:
                if tunnel.poll() is None:
                    tunnel.terminate()
        assert continued_id == original_id and continued == saved + [["user", FOLLOWUP], ["assistant", RECALL]]
        assert api({"type": "open_chat", "chat_id": new_id})[:2] == (new_id, new_messages)
        print("CSV report, approvals, VM tool accounts, and separate chat persistence passed.", flush=True)


if __name__ == "__main__":
    signal.alarm(300)
    smoke()
