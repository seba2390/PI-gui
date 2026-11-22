// Web search for Pistachio chats: off, search (a quick lookup) or deep (a thorough investigation),
// set with `/websearch off|search|deep`. This file only connects pi to the Rust helper that does
// the work (Pistachio's own executable, run as `<exe> --deepsearch-tool <command>` with JSON on
// stdin). Pistachio writes this file on start; edits here are overwritten. Settings and each mode's
// instructions live next to it in config.json, prompt-search.md and prompt-deep.md.
import { spawn } from "node:child_process";
import { Type } from "typebox";

const BIN = process.env.PISTACHIO_DEEPSEARCH_BIN;
const KEY = "pistachio-deepsearch";
const TOOLS = ["web_search", "web_fetch"];
const MODES = ["off", "search", "deep"] as const;
type Mode = (typeof MODES)[number];
type Budget = { max_searches: number; max_fetches: number };

type Reply = {
  ok: boolean;
  text?: string;
  details?: unknown;
  error?: string;
  config?: { default_mode?: Mode; modes?: { search: Budget; deep: Budget } };
  prompts?: { search: string; deep: string };
};

function helper(command: string, input: unknown, signal?: AbortSignal): Promise<Reply> {
  return new Promise((resolve) => {
    if (!BIN) {
      resolve({ ok: false, error: "Deep search only works in sessions started by Pistachio." });
      return;
    }
    const child = spawn(BIN, ["--deepsearch-tool", command], { stdio: ["pipe", "pipe", "pipe"], signal });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("error", (error) => resolve({ ok: false, error: signal?.aborted ? "Cancelled." : String(error) }));
    child.on("close", () => {
      try {
        resolve(JSON.parse(out));
      } catch {
        resolve({ ok: false, error: (err || out || "The deep search helper failed.").trim().slice(0, 2000) });
      }
    });
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(input ?? {}));
  });
}

function fill(prompt: string, budget: Budget): string {
  const today = new Date().toLocaleDateString("en-CA", { weekday: "long", year: "numeric", month: "long", day: "numeric" });
  return prompt
    .replaceAll("{{date}}", today)
    .replaceAll("{{max_searches}}", String(budget.max_searches))
    .replaceAll("{{max_fetches}}", String(budget.max_fetches))
    .trim();
}

export default function (pi: any) {
  let mode: Mode = "off";
  let budget: Budget = { max_searches: 3, max_fetches: 3 };
  let used = { web_search: 0, web_fetch: 0 };

  const apply = (ctx: any) => {
    const others = pi.getActiveTools().filter((name: string) => !TOOLS.includes(name));
    pi.setActiveTools(mode === "off" ? others : [...others, ...TOOLS]);
    ctx?.ui?.setStatus?.(KEY, mode === "off" ? undefined : mode);
  };

  const run = async (command: string, params: unknown, signal?: AbortSignal) => {
    const reply = await helper(command, params, signal);
    if (!reply.ok) throw new Error(reply.error ?? "Deep search failed.");
    return { content: [{ type: "text", text: reply.text ?? "" }], details: reply.details ?? {} };
  };

  pi.registerTool({
    name: "web_search",
    label: "Web search",
    description:
      "Search the web. Returns result titles, URLs and short snippets. Snippets are previews, not evidence: read promising pages with web_fetch.",
    promptSnippet: "Search the web for current or external information",
    defaultActive: false,
    parameters: Type.Object({
      query: Type.String({ description: "A short, specific search-engine query (keywords, not a sentence)" }),
      max_results: Type.Optional(Type.Integer({ minimum: 1, maximum: 20, description: "Number of results (default from settings)" })),
    }),
    async execute(_id: string, params: unknown, signal?: AbortSignal) {
      return run("search", params, signal);
    },
  });

  pi.registerTool({
    name: "web_fetch",
    label: "Read web page",
    description:
      "Read a web page (HTML or text) and return its main text as Markdown. Long pages are cut down to the sections most relevant to `focus`.",
    promptSnippet: "Read a web page found with web_search",
    defaultActive: false,
    parameters: Type.Object({
      url: Type.String({ description: "The http(s) URL to read, e.g. from web_search results" }),
      focus: Type.Optional(Type.String({ description: "What you are looking for on this page, used to pick the relevant sections" })),
    }),
    async execute(_id: string, params: unknown, signal?: AbortSignal) {
      return run("fetch", params, signal);
    },
  });

  pi.registerCommand("websearch", {
    description: "Web search for this chat: off, search (quick lookup) or deep (thorough investigation)",
    handler: async (args: string, ctx: any) => {
      const word = String(args ?? "").trim().toLowerCase();
      if (!MODES.includes(word as Mode)) {
        ctx?.ui?.notify?.(`Web search is ${mode}. Use /websearch off, search or deep.`, "info");
        return;
      }
      mode = word as Mode;
      pi.appendEntry(KEY, { mode });
      apply(ctx);
    },
  });

  pi.on("session_start", async (_event: unknown, ctx: any) => {
    const reply = await helper("config", {});
    mode = MODES.includes(reply.config?.default_mode as Mode) ? (reply.config!.default_mode as Mode) : "off";
    for (const entry of ctx.sessionManager.getBranch()) {
      if (entry.type !== "custom" || entry.customType !== KEY) continue;
      if (MODES.includes(entry.data?.mode)) mode = entry.data.mode;
      // Chats from before the modes existed stored `enabled`.
      else if (typeof entry.data?.enabled === "boolean") mode = entry.data.enabled ? "deep" : "off";
    }
    apply(ctx);
  });

  pi.on("before_agent_start", async (event: any, ctx: any) => {
    used = { web_search: 0, web_fetch: 0 };
    const sections = event.systemPromptOptions?.sections;
    const set = (text?: string) => {
      if (!sections) return;
      if (sections instanceof Map) text ? sections.set("web_search", text) : sections.delete("web_search");
      else if (text) sections.web_search = text;
      else delete sections.web_search;
    };
    if (mode === "off") {
      set(undefined);
      return;
    }
    // Read on every message, so edits to config.json and the prompts apply right away.
    const reply = await helper("config", {});
    if (!reply.ok) {
      ctx?.ui?.notify?.(`Web search settings: ${reply.error}`, "error");
      return;
    }
    budget = reply.config?.modes?.[mode] ?? budget;
    set(fill(reply.prompts?.[mode] ?? "", budget));
  });

  pi.on("tool_call", async (event: any) => {
    if (!TOOLS.includes(event.toolName)) return;
    const name = event.toolName as "web_search" | "web_fetch";
    const limit = name === "web_search" ? budget.max_searches : budget.max_fetches;
    used[name] += 1;
    if (used[name] > limit) {
      const what = name === "web_search" ? "searches" : "page reads";
      return {
        block: true,
        reason: `The limit of ${limit} ${what} for this message is reached. Answer now with the sources you have read, and say what is still uncertain.`,
      };
    }
  });
}
