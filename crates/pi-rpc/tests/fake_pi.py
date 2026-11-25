#!/usr/bin/env python3
"""A scripted stand-in for `pi --mode rpc`, used by the process tests.

Answers the commands the GUI sends at startup and streams a tiny run for `prompt`.
Writes strict JSONL to stdout and exits when stdin closes, like pi.

Prompts that act differently: "refuse ..." is rejected, "die" makes pi exit instead of
answering, "start and die" makes it exit after the run started, and "hold" runs until
`abort`. Prompts during that run are queued ("/skill:x" expanded) until `clear_queue`, or the
shell command "deliver" for the first one.

`bash` ends right away, except "hang", which prints a line and runs until `abort_bash`;
"refuse" is rejected, and "die" makes pi exit.

`compact` fails right away, except with the instructions "hang": it runs until `abort`, or
until the shell command "finish compaction", and prompts are refused meanwhile.
"""
import json
import sys


def emit(record):
    sys.stdout.write(json.dumps(record) + "\n")
    sys.stdout.flush()


assert sys.argv[1:3] == ["--mode", "rpc"], sys.argv
model = {"id": "fake-1", "name": "Fake", "provider": "test", "reasoning": True,
         "input": ["text"], "contextWindow": 1000, "maxTokens": 100,
         "thinkingLevelMap": {"minimal": "low", "low": "low", "high": "xhigh", "xhigh": "xhigh"}}

# Request ids of the `bash` commands still running.
hanging = []
# A "hold" run is in progress.
running = False
# The request id of the `compact` command in progress.
compacting = None
queue = {"steering": [], "followUp": []}


def end_compaction(**outcome):
    """Ends the compaction in progress like pi: an event first, then the answer."""
    failed = "result" not in outcome
    emit({"type": "compaction_end", "reason": "manual", "aborted": False, "willRetry": False, **outcome})
    answer = {"error": "Compaction cancelled"} if failed else {"data": {}}
    emit({"id": compacting, "type": "response", "command": "compact", "success": not failed, **answer})


for line in sys.stdin:
    command = json.loads(line)
    kind, rid = command["type"], command.get("id")
    reply = {"id": rid, "type": "response", "command": kind, "success": True}
    if kind == "get_state":
        reply["data"] = {"model": model, "thinkingLevel": "low", "sessionId": "s1"}
    elif kind == "get_available_models":
        reply["data"] = {"models": [model]}
    elif kind == "get_available_thinking_levels":
        reply["data"] = {"levels": ["off", "minimal", "low", "medium", "high", "xhigh"]}
    elif kind == "prompt" and compacting:
        reply.update(success=False, error="Cannot submit a prompt while compaction is in progress.")
    elif kind == "prompt" and command["message"].startswith("refuse"):
        reply.update(success=False, error="No API key found for the selected model.")
    elif kind == "prompt" and command["message"] == "die":
        break
    elif kind == "prompt" and command["message"] == "start and die":
        emit({"type": "agent_start"})
        break
    elif kind == "prompt" and running:
        text = command["message"].replace("/skill:x", "<skill>x</skill>")
        queue["followUp" if command.get("streamingBehavior") == "followUp" else "steering"].append(text)
        emit({"type": "queue_update", **queue})
        reply["data"] = {"disposition": "queued"}
    elif kind == "clear_queue":
        reply["data"] = queue
        queue = {"steering": [], "followUp": []}
        emit({"type": "queue_update", **queue})
    elif kind == "bash" and command["command"] == "deliver":
        text = (queue["steering"] or queue["followUp"]).pop(0)
        emit({"type": "queue_update", **queue})
        emit({"type": "message_start", "message": {"role": "user", "content": text, "timestamp": 2}})
        reply["data"] = {}
    elif kind == "prompt" and command["message"] == "hold":
        reply["data"] = {"disposition": "started"}
        emit(reply)
        emit({"type": "agent_start"})
        emit({"type": "message_start", "message": {"role": "user", "content": "hold", "timestamp": 1}})
        running = True
        continue
    elif kind == "abort":
        if running:
            emit({"type": "agent_settled", "aborted": True})
        if compacting:
            end_compaction(aborted=True)
        running, compacting = False, None
    elif kind == "compact":
        emit({"type": "compaction_start", "reason": "manual"})
        compacting = rid
        if command.get("customInstructions") != "hang":
            end_compaction(errorMessage="Compaction failed: Nothing to compact")
            compacting = None
        continue
    elif kind == "bash" and command["command"] == "finish compaction":
        end_compaction(result={"tokensBefore": 2000, "estimatedTokensAfter": 1000})
        compacting = None
        reply["data"] = {}
    elif kind == "prompt":
        reply["data"] = {"disposition": "started"}
        emit(reply)
        emit({"type": "agent_start"})
        user = {"role": "user", "content": command["message"], "timestamp": 1}
        emit({"type": "message_start", "message": user})
        emit({"type": "message_end", "message": user})
        emit({"type": "message_start", "message": {"role": "assistant", "content": [], "stopReason": "pending"}})
        for chunk in ["Hel", "lo   world"]:
            emit({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": chunk}})
        final = {"role": "assistant", "content": [{"type": "text", "text": "Hello   world"}], "stopReason": "stop"}
        emit({"type": "message_end", "message": final})
        emit({"type": "agent_end", "messages": [final], "willRetry": False})
        emit({"type": "agent_settled", "aborted": False})
        continue
    elif kind == "bash" and command["command"] == "hang":
        emit({"type": "bash_execution_update", "id": rid, "delta": "working\n"})
        hanging.append(rid)
        continue
    elif kind == "bash" and command["command"] == "refuse":
        reply.update(success=False, error="spawn failed")
    elif kind == "bash" and command["command"] == "die":
        emit({"type": "bash_execution_update", "id": rid, "delta": "working\n"})
        break
    elif kind == "bash":
        emit({"type": "bash_execution_update", "id": rid, "delta": "par"})
        reply["data"] = {"output": "part two", "exitCode": 3, "cancelled": False, "truncated": False}
    elif kind == "abort_bash":
        emit(reply)
        for running in hanging:
            emit({"id": running, "type": "response", "command": "bash", "success": True,
                  "data": {"output": "working\n", "cancelled": True, "truncated": False}})
        hanging.clear()
        continue
    elif kind == "set_model":
        reply = {"id": rid, "type": "response", "command": kind, "success": False, "error": "Model not found: x/y"}
    emit(reply)
