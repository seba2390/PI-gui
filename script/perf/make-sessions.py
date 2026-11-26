#!/usr/bin/env python3
"""Writes generated pi sessions for the navigation benchmark (script/perf.sh).

Usage: make-sessions.py <agent dir> <project dir>

Creates chats (in <agent dir>/pistachio/chat) and Code sessions in a few folders under
<project dir>: mostly short ones, a few long ones with markdown, code blocks and tool calls,
and one chat titled "delete me" for the benchmark's delete step. Deterministic.
"""

import json
import os
import sys
import uuid
from datetime import datetime, timedelta, timezone

AGENT, PROJECTS = sys.argv[1], sys.argv[2]
NOW = datetime(2026, 11, 23, 9, 0, tzinfo=timezone.utc)

PARAGRAPH = (
    "Here is how the pieces fit together. The **reducer** turns pi's events into items, "
    "the view lays them out, and `cached` child views only re-render when they notify. "
    "That keeps streaming cheap: one turn changes, the rest of the window is reused."
)
CODE = """```rust
fn fibonacci(n: u64) -> u64 {
    let (mut a, mut b) = (0, 1);
    for _ in 0..n {
        (a, b) = (b, a + b);
    }
    a
}

#[test]
fn small_numbers() {
    assert_eq!(fibonacci(10), 55);
}
```"""
TABLE = """| Step | Before | After |
|------|-------:|------:|
| Open | 120 ms | 18 ms |
| Hover | 9 ms | 2 ms |
| Delete | 640 ms | 12 ms |"""


def encode(cwd):
    return "--" + cwd.strip("/").replace("/", "-") + "--"


def stamp(t):
    return t.strftime("%Y-%m-%dT%H:%M:%S.") + f"{t.microsecond // 1000:03d}Z"


def write_session(cwd, title, turns, when, rng):
    sid = str(uuid.UUID(int=rng))
    folder = os.path.join(AGENT, "sessions", encode(cwd))
    os.makedirs(folder, exist_ok=True)
    path = os.path.join(folder, stamp(when).replace(":", "-").replace(".", "-", 1) + f"_{sid}.jsonl")
    lines = [{"type": "session", "version": 3, "id": sid, "timestamp": stamp(when), "cwd": cwd}]
    parent = None
    counter = 0

    def entry(message, t):
        nonlocal parent, counter
        counter += 1
        eid = f"{rng % 0xFFFFFF:06x}{counter:02x}"
        lines.append({"type": "message", "id": eid, "parentId": parent, "timestamp": stamp(t), "message": message})
        parent = eid

    t = when
    for turn in range(turns):
        t += timedelta(seconds=20)
        prompt = title if turn == 0 else f"Follow-up {turn}: can you explain step {turn} in more detail?"
        entry({"role": "user", "content": [{"type": "text", "text": prompt}], "timestamp": int(t.timestamp() * 1000)}, t)
        call = f"call-{rng}-{turn}"
        if turn % 2 == 1:
            entry({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Let me look at the files first.", "thinkingSignature": "reasoning"},
                {"type": "toolCall", "id": call, "name": "bash", "arguments": {"command": "ls -la && wc -l src/*.rs"}},
            ], "api": "openai-completions", "provider": "bench", "model": "bench-model",
                "usage": {"input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 120,
                          "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
                "stopReason": "toolUse", "timestamp": int(t.timestamp() * 1000)}, t)
            entry({"role": "toolResult", "toolCallId": call, "toolName": "bash",
                   "content": [{"type": "text", "text": "\n".join(f"{i:>5} src/file_{i}.rs" for i in range(1, 30))}],
                   "isError": False, "timestamp": int(t.timestamp() * 1000)}, t)
        body = [f"### Step {turn}", PARAGRAPH, "- first point\n- second point with `code`\n- third point", CODE, PARAGRAPH]
        if turn % 3 == 0:
            body.append(TABLE)
        entry({"role": "assistant", "content": [{"type": "text", "text": "\n\n".join(body)}],
               "api": "openai-completions", "provider": "bench", "model": "bench-model",
               "usage": {"input": 100, "output": 300, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 400,
                         "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
               "stopReason": "stop", "timestamp": int(t.timestamp() * 1000)}, t)
    with open(path, "w") as handle:
        for line in lines:
            handle.write(json.dumps(line, separators=(",", ":")) + "\n")
    mtime = when.timestamp()
    os.utime(path, (mtime, mtime))


chat = os.path.join(AGENT, "pistachio", "chat")
os.makedirs(chat, exist_ok=True)
topics = ["Plan a weekend in Copenhagen", "Explain Rust lifetimes", "Recipe for sourdough bread",
          "Compare SQLite and Postgres", "Write a haiku about autumn", "How do vaccines work",
          "Summarise the French revolution", "Help me name a cat", "What is a monad",
          "Tips for running a 10k", "Explain quantum tunnelling", "Draft a birthday message"]
rng = 1000
for index, topic in enumerate(topics * 3):
    rng += 1
    long = index in (2, 7)
    write_session(chat, f"{topic} ({index})", 40 if long else 3, NOW - timedelta(hours=5 * index), rng)
rng += 1
write_session(chat, "delete me: a chat for the benchmark", 2, NOW - timedelta(minutes=30), rng)

for project in ["webshop", "compiler", "dotfiles", "ml-notebooks"]:
    folder = os.path.join(PROJECTS, project)
    os.makedirs(os.path.join(folder, "src"), exist_ok=True)
    for i in range(6):
        with open(os.path.join(folder, "src", f"file_{i}.rs"), "w") as handle:
            handle.write("fn main() {}\n" * (i + 1))
    for index in range(6):
        rng += 1
        write_session(folder, f"Refactor the {project} module {index}", 30 if index == 0 else 4,
                      NOW - timedelta(hours=7 * index + len(project)), rng)

# The model the sessions used, so pi restores it and the composer shows it (pi never calls
# it: the benchmark sends nothing).
with open(os.path.join(AGENT, "models.json"), "w") as handle:
    json.dump({"providers": {"bench": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions", "apiKey": "bench",
        "models": [{"id": "bench-model", "name": "Bench Model", "reasoning": True, "input": ["text"],
                    "contextWindow": 128000, "maxTokens": 8192}],
    }}}, handle, indent=2)
with open(os.path.join(AGENT, "settings.json"), "w") as handle:
    json.dump({"defaultProvider": "bench", "defaultModel": "bench-model",
               "defaultThinkingLevel": "medium"}, handle, indent=2)
