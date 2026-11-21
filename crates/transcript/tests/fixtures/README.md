# RPC fixtures

Raw stdout of `pi --mode rpc --no-session --thinking low` (pi 1.5.2), recorded against
local models and sanitized (paths, hostnames; system prompts stripped from `get_messages`).

| File | Model | Scenario |
|---|---|---|
| `tool_run.jsonl` | qwen3.8-27b | read → edit → bash, then `get_session_stats`, `get_messages` |
| `abort_run.jsonl` | qwen3.8-27b | long prompt aborted after 6 s |
| `markdown_run.jsonl` | inkling-2-mini | markdown answer (heading, list, code, table) |

Re-record with a small script that writes commands to pi's stdin and copies stdout
verbatim; keep fixtures small and scrub anything machine-specific.
