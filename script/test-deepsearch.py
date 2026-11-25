#!/usr/bin/env python3
"""Runs deep search's pi extension end to end: real pi, the app's helper, and a scripted model,
search service and web pages on this computer. Nothing leaves it.

  cargo build && script/test-deepsearch.py            # every case
  script/test-deepsearch.py revise esc-in-check       # some of them

Needs pi on PATH. Each case prints what pi's session holds in the end.
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
import time
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("PISTACHIO_BIN", os.path.join(ROOT, "target/debug/pistachio"))
CHECK = "pistachio-deepsearch-check"


def page(number):
    return (
        f"<html><head><title>Shop {number}</title></head><body><article><h1>Shop {number}</h1>"
        + f"<p>Shop {number} sells the console for {5000 + number} kr. including VAT, and has it in stock.</p>"
        + "<p>Delivery takes two days, and it can be returned for a month after that.</p>" * 4
        + "</article></body></html>"
    )


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    """One of several, so that there are several sites."""

    daemon_threads = True
    allow_reuse_address = True

    def handle_error(self, *_):
        pass


class Case:
    """What the scripted model does. `reply` answers the conversation, `ask` the questions besides."""

    effort = "standard"
    # Config of the effort level, so that a case is short.
    limits = {
        "max_questions": 2, "searches_per_question": 1, "pages_per_question": 1, "min_sites": 2,
        "max_rounds": 2, "max_revisions": 1, "max_claims": 20, "claims_per_check": 1,
    }
    check = {"timeout_secs": 3, "total_secs": 60, "max_tokens": 2000}
    # Stops the run when a status with this text shows up.
    stop_at = None
    verdict = "supported"

    def __init__(self):
        self.calls = 0
        self.asked = []
        self.forced = 0
        self.cut = 0

    def links(self):
        return [f"http://127.0.0.1:{port}/page/{n}" for n, port in enumerate(self.ports)]

    def report(self, pages, extra=""):
        lines = [f"- Shop {n} sells it for {5000 + n} kr. [Shop {n}]({self.links()[n]})" for n in pages]
        return "# Prices\n\n" + "\n".join(lines) + extra

    def tools(self, *calls):
        return ("tools", list(calls))

    def search(self, query="console price"):
        return ("web_search", {"query": query})

    def fetch(self, n):
        return ("web_fetch", {"url": self.links()[n]})

    def reply(self, n, body):
        raise NotImplementedError

    def ask(self, kind, prompt):
        if kind == "plan":
            return '["What does it cost?", "Where is it in stock?"]'
        if kind == "judge":
            ids = [line.split(":")[0].split()[1] for line in prompt.splitlines() if line.startswith("CLAIM ")]
            quote = "sells the console for"
            number = [word for word in prompt.split() if word.startswith("50")][0]
            return json.dumps([
                {"id": int(i), "verdict": self.verdict, "quote": f"{quote} {number} kr. including VAT", "why": "it says less"}
                for i in ids
            ])
        return "[]"

    def expect(self, result):
        pass


class Stops(Case):
    """Reports after one search: sent back to its research, then final."""

    def reply(self, n, body):
        # Where the API can, it has to call a tool while its research is short.
        if body.get("tool_choice") == "required":
            return self.tools(self.search(f"more {n}"), self.fetch(min(n, 3)))
        return [self.tools(self.search()), self.report([])][n] if n < 2 else self.report([2, 3])

    def expect(self, result):
        assert result.forced > 0, "no tool call was forced"
        assert result.finals == [True], result.finals


class NoToolChoice(Case):
    """A server that refuses `tool_choice`, and a model that stops early."""

    def reply(self, n, body):
        if body.get("tool_choice"):
            return ("error", 400)
        return [
            self.tools(self.search()),
            self.report([]),
            self.tools(self.search("second"), self.fetch(0), self.fetch(1)),
        ][n] if n < 3 else self.report([0, 1])

    def expect(self, result):
        assert result.finals == [True], result.finals
        assert any(title == "Continuing research" for title in result.steps), result.steps


class Refuses(Case):
    """Never does any research beyond the first search: it still ends."""

    def reply(self, n, body):
        if body.get("tool_choice"):
            return ("error", 400)
        return self.tools(self.search()) if n == 0 else "The console costs 4999 kr. in most shops."

    def expect(self, result):
        assert result.finals == [True], result.finals
        assert result.about["claims"]["unsourced"] == 1, result.about


class Revise(Case):
    """The first report has a claim without a source, the second is fine."""

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search(), self.search("stock"), self.fetch(0), self.fetch(1))
        return self.report([0, 1], "\n- Shop 9 sells it for 6789 kr.\n" if n == 1 else "")

    def expect(self, result):
        assert result.finals == [True], result.finals
        assert "Revising" in result.steps, result.steps
        assert result.about["problems"] == [], result.about["problems"]
        assert result.about["claims"]["backed"] == 2, result.about


class Wrong(Case):
    """Claims its sources do not back, which the model does not correct."""

    verdict = "unsupported"

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search(), self.search("stock"), self.fetch(0), self.fetch(1))
        return self.report([0, 1])

    def expect(self, result):
        assert result.finals == [True], result.finals
        assert result.about["claims"]["unbacked"] == 2, result.about
        assert len(result.about["problems"]) == 2


class Stub(Case):
    """Writes only what changed instead of the whole report again."""

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search(), self.search("stock"), self.fetch(0), self.fetch(1))
        return self.report([0, 1], "\n- Shop 9 sells it for 6789 kr.\n" * 3) if n == 1 else "Done, it is fixed."

    def expect(self, result):
        assert result.finals == [False] and result.back == 1, (result.finals, result.back)


class Silent(Case):
    """The model never answers when a claim is checked."""

    limits = {**Case.limits, "max_rounds": 0}

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search(), self.fetch(0), self.fetch(1))
        return self.report([0, 1])

    def ask(self, kind, prompt):
        return ("hang",) if kind == "judge" else super().ask(kind, prompt)

    def expect(self, result):
        assert result.finals == [False], result.finals
        assert "did not answer" in result.notes[-1], result.notes
        assert result.seconds < 30, result.seconds


class Garbled(Case):
    """Answers the check with something that is no JSON."""

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search(), self.search("stock"), self.fetch(0), self.fetch(1))
        return self.report([0, 1])

    def ask(self, kind, prompt):
        return "I think [they are fine" if kind == "judge" else super().ask(kind, prompt)

    def expect(self, result):
        assert result.finals == [False], result.finals


class EscInCheck(Silent):
    """Esc while the report is checked."""

    check = {**Case.check, "timeout_secs": 60}
    stop_at = "Checking the report: 0 of"

    def expect(self, result):
        assert result.finals == [False], result.finals
        assert result.aborted
        assert result.stopped_in < 2, result.stopped_in


class EscInResearch(Case):
    """Esc after a draft was sent back, while the model is writing again."""

    stop_at = "Researching further"

    def reply(self, n, body):
        if body.get("tool_choice"):
            return ("error", 400)
        if n == 0:
            return self.tools(self.search())
        return self.report([0]) if n == 1 else ("hang",)

    def expect(self, result):
        assert result.finals == [False], result.finals
        assert result.aborted


class Rambles(Case):
    """Ignores `tool_choice` and writes a long report each time: it is cut off, until the last."""

    def reply(self, n, body):
        if n == 0:
            return self.tools(self.search())
        if body["messages"][-1]["role"] == "tool" or n % 2:
            return self.report([0, 1], "\n\nThe shops are nice to visit on most days." * 400)
        return self.tools(self.search(f"more {n}"), self.fetch(n % 4))

    def expect(self, result):
        assert result.finals == [True], result.finals
        assert result.cut == 2, result.cut
        assert result.steps.count("Continuing research") == 2, result.steps
        assert "Writing it again" not in result.steps, result.steps


class NoResearch(Case):
    """A reply without any research is only that."""

    def reply(self, n, body):
        return "Hello!"

    def expect(self, result):
        assert result.finals == [] and result.steps == []


class Gone(Case):
    """The server fails for good after a draft."""

    def reply(self, n, body):
        if body.get("tool_choice") or n > 1:
            return ("error", 400)
        return self.tools(self.search()) if n == 0 else self.report([0])

    def expect(self, result):
        assert result.finals == [False], result.finals


CASES = {
    "stops": Stops, "no-tool-choice": NoToolChoice, "refuses": Refuses, "revise": Revise,
    "wrong": Wrong, "stub": Stub, "silent": Silent, "garbled": Garbled,
    "esc-in-check": EscInCheck, "esc-in-research": EscInResearch, "rambles": Rambles,
    "no-research": NoResearch, "gone": Gone,
}


def handler(case, stopping):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def send(self, status, kind, body):
            body = body.encode()
            self.send_response(status)
            self.send_header("content-type", kind)
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path = urllib.parse.urlparse(self.path).path
            if path == "/search":
                results = [
                    {"url": url, "title": f"Shop {n}", "content": "What it costs"}
                    for n, url in enumerate(case.links())
                ]
                return self.send(200, "application/json", json.dumps({"results": results}))
            if path.startswith("/page/"):
                return self.send(200, "text/html", page(int(path.split("/")[-1])))
            self.send(404, "text/plain", "gone")

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["content-length"])))
            last = body["messages"][-1]["content"]
            text = last if isinstance(last, str) else "\n".join(part.get("text", "") for part in last or [])
            kind = "chat"
            for name, sign in [
                ("plan", "The end of a conversation:"), ("judge", "Check whether each claim"),
                ("open", "Which sub-questions does it not answer"), ("maybe", "cite no source:"),
            ]:
                if sign in text and "tools" not in body:
                    kind = name
            if kind == "chat":
                if body.get("tool_choice") == "required":
                    case.forced += 1
                answer = case.reply(case.calls, body)
                if not (isinstance(answer, tuple) and answer[0] == "error"):
                    case.calls += 1
            else:
                case.asked.append((kind, body.get("max_tokens"), body.get("reasoning_effort")))
                answer = case.ask(kind, text)
            if isinstance(answer, tuple) and answer[0] == "error":
                return self.send(answer[1], "application/json", '{"error": {"message": "unknown parameter"}}')
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("connection", "close")
            self.end_headers()
            self.close_connection = True

            def chunk(delta, finish=None):
                data = {"id": "x", "object": "chat.completion.chunk", "model": "m",
                        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                self.wfile.write(b"data: " + json.dumps(data).encode() + b"\n\n")
                self.wfile.flush()

            try:
                if isinstance(answer, tuple) and answer[0] == "hang":
                    stopping.wait(120)
                    return
                if isinstance(answer, tuple):
                    for index, (name, args) in enumerate(answer[1]):
                        call = {"index": index, "id": f"call-{case.calls}-{index}", "type": "function",
                                "function": {"name": name, "arguments": json.dumps(args)}}
                        chunk({"tool_calls": [call]})
                    chunk({}, "tool_calls")
                else:
                    # About four characters to a token.
                    limit = body.get("max_tokens", 65536) * 4
                    case.cut += len(answer) > limit
                    for at in range(0, min(len(answer), limit), 400):
                        chunk({"content": answer[at:at + 400]})
                    chunk({}, "length" if len(answer) > limit else "stop")
                self.wfile.write(b"data: [DONE]\n\n")
            except OSError:
                pass

    return Handler


class Result:
    pass


def run(case):
    stopping = threading.Event()
    servers = [Server(("127.0.0.1", 0), handler(case, stopping)) for _ in range(4)]
    case.ports = [server.server_port for server in servers]
    for server in servers:
        threading.Thread(target=server.serve_forever, daemon=True).start()
    folder = os.environ.get("KEEP") or tempfile.mkdtemp(prefix="deepsearch-test-")
    if os.environ.get("KEEP"):
        shutil.rmtree(folder, ignore_errors=True)
    settings = os.path.join(folder, "settings")
    chat = os.path.join(folder, "pistachio", "chat")
    os.makedirs(settings)
    os.makedirs(chat)
    url = f"http://127.0.0.1:{case.ports[0]}"
    with open(os.path.join(folder, "models.json"), "w") as file:
        json.dump({"providers": {"fake": {
            "baseUrl": url + "/v1", "api": "openai-completions", "apiKey": "x",
            "compat": {"thinkingFormat": "openai", "supportsReasoningEffort": True, "maxTokensField": "max_tokens"},
            "models": [{"id": "m", "name": "m", "reasoning": True, "contextWindow": 200000, "maxTokens": 65536}],
        }}}, file)
    with open(os.path.join(settings, "config.json"), "w") as file:
        json.dump({
            "backend": "searxng", "searxng": {"url": url}, "check": case.check,
            "fetch": {"allow_private_networks": True}, "deep": {case.effort: case.limits},
        }, file)
    shutil.copy(os.path.join(ROOT, "crates/deepsearch/src/extension.ts"), settings)
    env = dict(os.environ, PI_CODING_AGENT_DIR=folder, PISTACHIO_DEEPSEARCH_DIR=settings,
               PISTACHIO_DEEPSEARCH_BIN=BIN)
    pi = subprocess.Popen(
        ["pi", "--mode", "rpc", "--model", "fake/m", "--thinking", "minimal", "--tools", "web_search,web_fetch",
         "--no-mcp", "--extension", os.path.join(settings, "extension.ts")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, cwd=chat, env=env, text=True)
    events = queue.Queue()

    def read():
        for line in pi.stdout:
            events.put(json.loads(line))
        events.put(None)

    threading.Thread(target=read, daemon=True).start()

    def send(command):
        pi.stdin.write(json.dumps(command) + "\n")
        pi.stdin.flush()

    result = Result()
    result.finals, result.steps, result.notes, result.statuses = [], [], [], []
    result.aborted, result.back, result.about, result.stopped_in = False, 0, {}, 0
    send({"type": "prompt", "message": "/websearch deep"})
    send({"type": "prompt", "message": f"/websearch effort {case.effort}"})
    time.sleep(1.5)
    start = time.time()
    stopped = None
    send({"type": "prompt", "message": "What does the console cost?"})
    try:
        while True:
            event = events.get(timeout=90)
            assert event is not None, "pi ended"
            kind = event["type"]
            if kind == "extension_ui_request" and event.get("statusKey") == "pistachio-deepsearch-progress":
                text = event.get("statusText")
                result.statuses.append(text)
                if case.stop_at and text and text.startswith(case.stop_at) and not stopped:
                    stopped = time.time()
                    send({"type": "abort"})
            elif kind == "extension_error":
                raise AssertionError(f"extension error: {event.get('error')}")
            elif kind == "entry_appended" and event["entry"].get("customType") == CHECK:
                details = event["entry"]["details"]
                result.steps += [step["title"] for step in details.get("steps", [])]
                if details["stage"] == "final":
                    result.finals.append(details["checked"])
                    result.back = details["back"]
                    result.about = details.get("about", {})
                    result.notes.append(event["entry"]["content"])
            elif kind == "agent_settled":
                result.aborted = event["aborted"]
                break
    finally:
        stopping.set()
        pi.kill()
        for server in servers:
            server.shutdown()
        if not os.environ.get("KEEP"):
            shutil.rmtree(folder, ignore_errors=True)
    result.seconds = time.time() - start
    result.stopped_in = time.time() - stopped if stopped else 0
    result.forced, result.cut = case.forced, case.cut
    # Nothing is asked without limits, or thinks more than the chat.
    for kind, tokens, effort in case.asked:
        assert tokens == case.check["max_tokens"] and effort == "minimal", (kind, tokens, effort)
    # The status never outlives the run.
    assert not result.statuses or result.statuses[-1] is None, result.statuses[-3:]
    return result


def main():
    names = sys.argv[1:] or list(CASES)
    failed = 0
    for name in names:
        case = CASES[name]()
        result = None
        try:
            result = run(case)
            case.expect(result)
            print(f"ok    {name:16} {result.seconds:5.1f} s  {' → '.join(result.steps)}")
        except (AssertionError, queue.Empty) as error:
            failed += 1
            print(f"FAIL  {name:16} {type(error).__name__}: {error}")
            if result:
                print("      steps", result.steps, "finals", result.finals, "notes", result.notes)
        if result and os.environ.get("VERBOSE"):
            print("     ", "\n      ".join(str(status) for status in result.statuses))
            print("     ", "\n      ".join(result.notes))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
