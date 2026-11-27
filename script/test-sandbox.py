#!/usr/bin/env python3
"""Runs the sandbox's pi extension end to end: real pi, the app's helper and a scripted model on
this computer, in a made-up home folder. Nothing leaves it.

  cargo build && script/test-sandbox.py

Needs pi on PATH. The model calls tools that the sandbox should allow or block, and the test
checks what each call returned. The gate's steps need the network (they list a public GitHub
repository's branches outside the sandbox); the test answers the gate's questions itself.
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
        # The home folder's names can be listed on macOS (getcwd needs it), not what is in them.
        ("bash", {"command": f"ls {home}/.ssh"}, False, ""),
        ("bash", {"command": "git --version && pwd"}, True, "git version"),
        ("ls", {"path": "."}, True, "inside.txt"),
        # The gate: git's local work stays in the sandbox; what needs the network or keys is
        # asked about (the answers are in ANSWERS, in order) and done outside it.
        ("bash", {"command": "git init -q -b main && git -c user.name=a -c user.email=a@b commit -q --allow-empty -m one && git log --format=%s"}, True, "one"),
        ("bash", {"command": f"git ls-remote --heads {PUBLIC}"}, True, "refs/heads/"),
        ("bash", {"command": f"git ls-remote --heads {PUBLIC}"}, False, "didn't allow"),
        ("bash", {"command": f"git ls-remote --heads {PUBLIC}"}, True, "refs/heads/"),
        # Remembered: not asked again.
        ("bash", {"command": f"git ls-remote --heads {PUBLIC}"}, True, "refs/heads/"),
        ("bash", {"command": "git push --receive-pack=evil origin main"}, False, "Pistachio"),
        # A setting the agent plants runs only in the sandbox (it can't write the home folder):
        # the gate's git never reads the repository's settings.
        ("bash", {"command": f"git config core.fsmonitor 'touch {home}/planted-ran; false' && "
                             f"git fetch -q {PUBLIC}; git rev-parse FETCH_HEAD"}, True, "7fd1a60"),
        # request_user_help: offered while the sandbox is on; the pill shows the command blocked
        # last (HELP_ANSWERS answers it and checks that).
        ("bash", {"command": f"touch {home}/needs-help"}, False, ""),
        ("request_user_help", {"title": "Create needs-help", "reason": "The sandbox blocks the home folder.",
                               "suggested_command": "touch ~/needs-help"}, True, "marked this done"),
        ("request_user_help", {"title": "Again", "reason": "r"}, True, "dismissed"),
    ]


PUBLIC = "https://github.com/octocat/Hello-World.git"
# The answers to the gate's questions, in order.
ANSWERS = ["Allow", "Don't allow", "Always for this project"]
HELP_MARK = "pistachio-help:"
# The answers to request_user_help, in order, and the command each request must show as blocked
# (cleared once shown).
HELP_ANSWERS = [("Done", "needs-help"), ("Dismiss", None)]


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
        ["pi", "--mode", "rpc", "--model", "fake/m", "--no-session", "--tools", "+grep,+find,+ls",
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
    asked = 0
    helped = 0
    try:
        while True:
            event = events.get(timeout=60)
            if event["type"] == "extension_error":
                raise AssertionError(f"extension error: {event.get('error')}")
            if event["type"] == "extension_ui_request" and event.get("method") == "select" \
                    and event["title"].startswith(HELP_MARK):
                request = json.loads(event["title"][len(HELP_MARK):])
                answer, command = HELP_ANSWERS[min(helped, len(HELP_ANSWERS) - 1)]
                blocked = (request.get("blocked") or {}).get("command")
                if (command is None) != (blocked is None) or (command and command not in blocked):
                    failed += 1
                    print(f"FAIL  help request {helped + 1} showed {blocked!r} as blocked")
                helped += 1
                print(f"      help: {request['title']} → {answer}")
                pi.stdin.write(json.dumps({"type": "extension_ui_response", "id": event["id"], "value": answer}) + "\n")
                pi.stdin.flush()
            elif event["type"] == "extension_ui_request" and event.get("method") == "select":
                answer = ANSWERS[asked] if asked < len(ANSWERS) else "Don't allow"
                asked += 1
                print(f"      asked: {event['title'].splitlines()[0]} → {answer}")
                pi.stdin.write(json.dumps({"type": "extension_ui_response", "id": event["id"], "value": answer}) + "\n")
                pi.stdin.flush()
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
    if helped != len(HELP_ANSWERS):
        failed += 1
        print(f"FAIL  {helped} help requests, not {len(HELP_ANSWERS)}")
    if asked != len(ANSWERS):
        failed += 1
        print(f"FAIL  the gate asked {asked} times, not {len(ANSWERS)}")
    if len(results) != len(plan["steps"]):
        failed += 1
        print(f"FAIL  {len(results)} results for {len(plan['steps'])} calls")
    if os.path.exists(os.path.join(home, "planted-ran")):
        failed += 1
        print("FAIL  a setting of the repository ran outside the sandbox")
    if os.path.exists(os.path.join(home, "outside.txt")) or os.path.exists(os.path.join(home, "notes.txt")):
        failed += 1
        print("FAIL  a file was written outside the project")
    if not os.environ.get("KEEP"):
        shutil.rmtree(folder, ignore_errors=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
