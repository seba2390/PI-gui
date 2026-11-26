// The `python` tool of Pistachio chats: the model runs Python to work numbers out instead of
// guessing them. The code runs in Monty, a Python interpreter written in Rust for code a model
// writes, inside Pistachio's own executable (`<exe> --python-tool`, the code as JSON on stdin):
// no files, network, environment or programs, and limits on time, memory and recursion.
// Settings → General → Chat mode turns it off (settings.json next to this file, read before each
// message). Pistachio writes this file on start; edits here are overwritten.
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Type } from "typebox";

const BIN = process.env.PISTACHIO_PYTHON_BIN;
const DIR = process.env.PISTACHIO_PYTHON_DIR;
const TOOL = "python";
// The helper stops code after 5 s of computing; this ends a helper that hangs anyway.
const KILL_AFTER_MS = 15_000;
// What the model gets back at most.
const MAX_TEXT = 20_000;

type Reply = { output?: string; value?: string | null; error?: string | null };

function helper(code: string, signal?: AbortSignal): Promise<Reply> {
  return new Promise((resolve) => {
    if (!BIN) {
      resolve({ error: "Python only works in chats started by Pistachio." });
      return;
    }
    if (signal?.aborted) {
      resolve({ error: "Cancelled." });
      return;
    }
    const child = spawn(BIN, ["--python-tool"], { stdio: ["pipe", "pipe", "pipe"], signal });
    const timer = setTimeout(() => child.kill("SIGKILL"), KILL_AFTER_MS);
    let out = "";
    let err = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (out += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (err += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ error: signal?.aborted ? "Cancelled." : String(error) });
    });
    child.on("close", () => {
      clearTimeout(timer);
      try {
        // The reply is the last line.
        resolve(JSON.parse(out.trim().split("\n").pop() ?? ""));
      } catch {
        resolve({ error: (err || "The Python helper stopped without an answer (too much memory or time?).").trim().slice(0, 2000) });
      }
    });
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify({ code }));
  });
}

// On unless the settings say otherwise.
function enabled(): boolean {
  if (!DIR) return true;
  try {
    return JSON.parse(readFileSync(join(DIR, "settings.json"), "utf8"))?.enabled !== false;
  } catch {
    return true;
  }
}

function cut(text: string): string {
  return text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT)}\n… (cut: ${text.length - MAX_TEXT} more characters)` : text;
}

export default function (pi: any) {
  // Offers the tool, or takes it away, as the settings say (other tools stay as they are).
  const apply = () => {
    const others = pi.getActiveTools().filter((name: string) => name !== TOOL);
    pi.setActiveTools(enabled() ? [...others, TOOL] : others);
  };
  pi.on("session_start", async () => apply());
  pi.on("before_agent_start", async () => apply());
  // A call made anyway while it is off (the model still had it) is refused.
  pi.on("tool_call", async (event: any) => {
    if (event.toolName === TOOL && !enabled()) {
      return { block: true, reason: "Python is turned off in Pistachio's settings. Answer without it." };
    }
  });

  pi.registerTool({
    name: TOOL,
    label: "Python",
    description:
      "Run Python code and get what it prints plus the value of its last expression. Use it for any calculation " +
      "beyond trivial mental arithmetic (sums, products, powers, logarithms, trigonometry, series, statistics, " +
      "unit conversions, dates), so numbers are computed, not estimated. Write the expression explicitly, with " +
      "the order of operations made clear. A sandboxed subset of Python 3: math, decimal, statistics, random, " +
      "datetime, itertools, functools, collections, re and json are there; no files, network, environment, " +
      "third-party packages (no numpy or sympy) or classes beyond simple ones. Integers are exact at any size; " +
      "use decimal.Decimal for exact decimal arithmetic. Runs stop after 5 seconds.",
    promptSnippet: "Compute numbers with Python instead of estimating them",
    promptGuidelines: [
      "When a question needs a numerical result (arithmetic, a formula, a series, statistics), compute it with the python tool and give the computed value; say how the expression was read when it is ambiguous.",
    ],
    parameters: Type.Object({
      code: Type.String({ description: "Python code. The value of the last line, if it is an expression, is returned; print() output too." }),
    }),
    async execute(_id: string, params: any, signal?: AbortSignal) {
      const reply = await helper(String(params.code ?? ""), signal);
      const parts: string[] = [];
      if (reply.output) parts.push(reply.output.trimEnd());
      if (reply.value != null) parts.push(reply.value);
      if (reply.error) throw new Error(cut([...parts, reply.error].join("\n")));
      const text = parts.join("\n") || "(no output: end with an expression or print() to see a result)";
      return { content: [{ type: "text", text: cut(text) }], details: {} };
    },
  });
}
