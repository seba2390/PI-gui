// Runs the agent's tools in Pistachio's sandbox in local Code sessions (Settings → Security).
//
// - `bash` commands start through Pistachio's helper (`<exe> --sandbox-tool run`), which puts
//   them in the sandbox this computer has (Seatbelt on macOS, bubblewrap or Landlock on Linux).
//   What the call shows and pi's handling of output, time limits and Esc stay pi's own.
// - `read`, `write`, `edit`, `grep`, `find` and `ls` run inside pi, so their paths are checked
//   against the same rules here: no writes outside the writable folders, nothing hidden read.
// - pi itself, its logins and model requests, MCP servers, other extensions and your own `!`
//   commands are not sandboxed.
//
// The rules are read again whenever the settings change, so changes apply to the next tool call.
// Pistachio writes this file on start; edits here are overwritten.
import { spawnSync } from "node:child_process";
import { realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";

const BIN = process.env.PISTACHIO_SANDBOX_BIN;
const DIR = process.env.PISTACHIO_SANDBOX_DIR;

type Policy = {
  sandboxed: boolean;
  enforced: boolean;
  backend: { kind: string } | null;
  project: string | null;
  writable: string[];
  hidden: string[];
  network: "on" | "off";
};

let cached: { key: string; policy: Policy | null } | undefined;

// The rules for `cwd`, from the helper; `null` when it could not say.
function policyFor(cwd: string): Policy | null {
  let stamp = "none";
  try {
    stamp = String(statSync(join(DIR!, "settings.json")).mtimeMs);
  } catch {}
  const key = `${cwd}\0${stamp}`;
  if (cached?.key === key) return cached.policy;
  let policy: Policy | null = null;
  try {
    const out = spawnSync(BIN!, ["--sandbox-tool", "policy", "--cwd", cwd], {
      encoding: "utf8",
      timeout: 15000,
    });
    policy = JSON.parse(out.stdout.trim().split("\n").pop() ?? "");
  } catch {}
  cached = { key, policy };
  return policy;
}

function quote(text: string): string {
  return `'${text.replaceAll("'", `'\\''`)}'`;
}

// A tool's path as the kernel sees it: `@` and `~` handled, relative to `cwd`, links resolved
// for the part that exists.
function realPath(path: string, cwd: string): string {
  let p = path.trim().replace(/^@/, "");
  if (p === "~") p = homedir();
  else if (p.startsWith("~/")) p = join(homedir(), p.slice(2));
  p = isAbsolute(p) ? resolve(p) : resolve(cwd, p);
  const rest: string[] = [];
  for (;;) {
    try {
      return join(realpathSync(p), ...rest.reverse());
    } catch {
      const parent = dirname(p);
      if (parent === p) return join(p, ...rest.reverse());
      rest.push(basename(p));
      p = parent;
    }
  }
}

const under = (path: string, root: string) => path === root || path.startsWith(root.endsWith(sep) ? root : root + sep);

const READS = new Set(["read", "grep", "find", "ls"]);
const WRITES = new Set(["write", "edit"]);

function where(policy: Policy): string {
  const others = policy.writable.filter((path) => path !== policy.project).length;
  return `${policy.project ?? "the project"}${others ? ` (and ${others} more folders such as temporary files and tool caches)` : ""}`;
}

export default async function (pi: any) {
  if (!BIN || !DIR) return;
  const agent = await import("@earendil-works/pi-coding-agent");
  const shell = (() => {
    try {
      return agent.getShellConfig?.().shell ?? "/bin/bash";
    } catch {
      return "/bin/bash";
    }
  })();

  if (typeof agent.createBashToolDefinition === "function") {
    const bash = agent.createBashToolDefinition(process.cwd(), {
      // pi runs this in its shell, which becomes the helper, which becomes the command.
      spawnHook: ({ command, cwd, env }: { command: string; cwd: string; env: Record<string, string> }) => ({
        command: `exec ${quote(BIN)} --sandbox-tool run --cwd ${quote(cwd)} -- ${quote(shell)} -c ${quote(command)}`,
        cwd,
        env: { ...env, PISTACHIO_SANDBOX_DIR: DIR },
      }),
    });
    pi.registerTool(bash);
  }

  pi.on("tool_call", async (event: any, ctx: any) => {
    const tool = event.toolName;
    if (!READS.has(tool) && !WRITES.has(tool)) return;
    const cwd = ctx?.cwd ?? process.cwd();
    const policy = policyFor(cwd);
    if (!policy) {
      return { block: true, reason: "Pistachio's sandbox could not read its rules, so file tools are stopped. Tell the user." };
    }
    if (!policy.enforced) return;
    const given = typeof event.input?.path === "string" && event.input.path.trim() ? event.input.path : ".";
    const path = realPath(given, cwd);
    const hidden = policy.hidden.find((h) => under(path, h));
    if (hidden) {
      return {
        block: true,
        reason: `${given} is hidden by Pistachio's sandbox (${hidden}). Don't try to get at it another way; ask the user if it is needed (Settings → Security).`,
      };
    }
    // Searching a folder that holds a hidden one would read it too; listing names is fine.
    if (tool === "grep") {
      const inside = policy.hidden.find((h) => under(h, path));
      if (inside) {
        return {
          block: true,
          reason: `Searching ${given} would read ${inside}, which Pistachio's sandbox hides. Search a narrower folder.`,
        };
      }
    }
    if (WRITES.has(tool) && !policy.writable.some((root) => under(path, root))) {
      return {
        block: true,
        reason: `Pistachio's sandbox only allows changes in ${where(policy)}; ${given} is outside. Ask the user if it should change (they can allow the folder in Settings → Security).`,
      };
    }
  });

  // A command that failed on what the sandbox blocks gets a note, so the model doesn't guess.
  pi.on("tool_result", async (event: any, ctx: any) => {
    if (event.toolName !== "bash" || !event.isError) return;
    const text = (event.content ?? []).map((part: any) => (part.type === "text" ? part.text : "")).join("\n");
    if (!/Operation not permitted|Read-only file system|Permission denied|Network is unreachable|Could not resolve host|Temporary failure in name resolution/i.test(text)) return;
    const policy = policyFor(ctx?.cwd ?? process.cwd());
    if (!policy?.sandboxed) return;
    const network = policy.network === "off" ? " It has no network access." : "";
    const note =
      `\n\n[Pistachio] This command ran in a sandbox: it can write only in ${where(policy)} and can't read ` +
      `${policy.hidden.length} hidden paths such as ${policy.hidden.slice(0, 3).join(", ")}.${network} If the sandbox blocked ` +
      `what the task needs, say so and ask the user instead of working around it (Settings → Security).`;
    return { content: [...(event.content ?? []), { type: "text", text: note }] };
  });
}
