#!/usr/bin/env python3
"""Runs the sandbox's pi extension end to end: real pi, the app's helper and a scripted model on
this computer, in a made-up home folder. Nothing leaves it.

  cargo build && script/test-sandbox.py

Needs pi on PATH. The model calls tools that the sandbox should allow or block, and the test
checks what each call returned.
"""
import http.server
import json
import os
import queue
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("PISTACHIO_BIN", os.path.join(ROOT, "target/debug/pistachio"))


def steps(home, project):
    """(tool, arguments, whether it should succeed, text its result must contain)."""
    return [
        ("bash", {"command": "echo inside > inside.txt && cat inside.txt"}, True, "inside"),
        ("bash", {"command": f"echo outside > {home}/outside.txt"}, False, ""),
        ("bash", {"command": f"cat {home}/.ssh/id_test"}, False, ""),
        ("bash", {"command": "echo key=${ANTHROPIC_API_KEY:-none}"}, True, "key=none"),
        ("bash", {"command": "echo home=$HOME"}, True, "home="),
        ("read", {"path": f"{home}/.ssh/id_test"}, False, "outside what"),
        ("read", {"path": "~/.ssh/id_test"}, False, "outside what"),
        ("write", {"path": f"{home}/notes.txt", "content": "x"}, False, "outside what"),
        ("write", {"path": "/usr/local/pistachio-test.txt", "content": "x"}, False, "only allows changes"),
        ("write", {"path": "written.txt", "content": "ok"}, True, ""),
        ("grep", {"pattern": "PRIVATE", "path": home}, False, "outside what"),
        ("grep", {"pattern": "PRIVATE", "path": os.path.dirname(home)}, False, "narrower"),
        ("read", {"path": f"{home}/.bashrc"}, False, "outside what"),
        ("bash", {"command": f"cat {home}/.bashrc"}, False, ""),
        ("bash", {"command": f"ls {home}"}, False, ""),
        ("bash", {"command": "git --version && pwd"}, True, "git version"),
        ("ls", {"path": "."}, True, "inside.txt"),
    ]


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def handler(plan):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            self.rfile.read(int(self.headers["content-length"]))
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("connection", "close")
            self.end_headers()
            self.close_connection = True

            def chunk(delta, finish=None):
                data = {"id": "x", "object": "chat.completion.chunk", "model": "m",
                        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                self.wfile.write(b"data: " + json.dumps(data).encode() + b"\n\n")

            n = plan["calls"]
            plan["calls"] += 1
            if n < len(plan["steps"]):
                name, args = plan["steps"][n][:2]
                call = {"index": 0, "id": f"call-{n}", "type": "function",
                        "function": {"name": name, "arguments": json.dumps(args)}}
                chunk({"tool_calls": [call]})
                chunk({}, "tool_calls")
            else:
                chunk({"content": "done"})
                chunk({}, "stop")
            self.wfile.write(b"data: [DONE]\n\n")

    return Handler


def main():
    # Not in the temporary folder: commands may write there.
    os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
    folder = os.path.realpath(tempfile.mkdtemp(prefix="sandbox-test-", dir=os.path.join(ROOT, "target")))
    home, agent, project = (os.path.join(folder, name) for name in ("home", "agent", "project"))
    sandbox_dir = os.path.join(agent, "pistachio", "sandbox")
    for path in (os.path.join(home, ".ssh"), project, sandbox_dir):
        os.makedirs(path)
    with open(os.path.join(home, ".ssh", "id_test"), "w") as file:
        file.write("PRIVATE KEY\n")
    with open(os.path.join(home, ".bashrc"), "w") as file:
        file.write("export EDITOR=vi\n")
    plan = {"calls": 0, "steps": steps(home, project)}
    server = Server(("127.0.0.1", 0), handler(plan))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with open(os.path.join(agent, "models.json"), "w") as file:
        json.dump({"providers": {"fake": {
            "baseUrl": f"http://127.0.0.1:{server.server_port}/v1", "api": "openai-completions",
            "apiKey": "x", "models": [{"id": "m", "name": "m", "contextWindow": 200000, "maxTokens": 8000}],
        }}}, file)
    with open(os.path.join(ROOT, "crates/sandbox/src/extension.ts")) as source:
        extension = os.path.join(sandbox_dir, "extension.ts")
        with open(extension, "w") as target:
            target.write(source.read())
    env = dict(os.environ, HOME=home, PI_CODING_AGENT_DIR=agent, PISTACHIO_SANDBOX_DIR=sandbox_dir,
               PISTACHIO_SANDBOX_BIN=BIN, ANTHROPIC_API_KEY="sk-secret")
    pi = subprocess.Popen(
        ["pi", "--mode", "rpc", "--model", "fake/m", "--no-session", "--tools", "read,bash,edit,write,grep,find,ls",
         "--extension", extension],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, cwd=project, env=env,
        text=True)
    events = queue.Queue()
    threading.Thread(target=lambda: [events.put(json.loads(line)) for line in pi.stdout] and None,
                     daemon=True).start()
    pi.stdin.write(json.dumps({"type": "prompt", "message": "go"}) + "\n")
    pi.stdin.flush()
    results = []
    failed = 0
    try:
        while True:
            event = events.get(timeout=60)
            if event["type"] == "extension_error":
                raise AssertionError(f"extension error: {event.get('error')}")
            if event["type"] == "tool_execution_end":
                result = event.get("result") or {}
                text = "\n".join(part.get("text", "") for part in result.get("content", []))
                results.append((not event.get("isError"), text))
            if event["type"] == "agent_settled":
                break
    finally:
        pi.kill()
        server.shutdown()
    for (name, args, ok, needle), (succeeded, text) in zip(plan["steps"], results):
        good = succeeded == ok and needle in text
        failed += not good
        line = text.strip().splitlines()[0] if text.strip() else ""
        print(f"{'ok  ' if good else 'FAIL'}  {name:5} {json.dumps(args)[:70]:72} → {line[:90]}")
    if len(results) != len(plan["steps"]):
        failed += 1
        print(f"FAIL  {len(results)} results for {len(plan['steps'])} calls")
    if os.path.exists(os.path.join(home, "outside.txt")) or os.path.exists(os.path.join(home, "notes.txt")):
        failed += 1
        print("FAIL  a file was written outside the project")
    if not os.environ.get("KEEP"):
        shutil.rmtree(folder, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
