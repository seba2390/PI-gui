// Runs the agent's tools in Pistachio's sandbox in local Code sessions (Settings → Sandboxing & Security).
//
// - `bash` commands start through Pistachio's helper (`<exe> --sandbox-tool run`), which puts
//   them in the sandbox this computer has (Seatbelt on macOS, bubblewrap or Landlock on Linux).
//   What the call shows and pi's handling of output, time limits and Esc stay pi's own.
// - `read`, `write`, `edit`, `grep`, `find` and `ls` run inside pi, so their paths are checked
//   against the same rules here: in the home folder only the project, allowed folders and tool
//   folders can be read, keys and logins never, and changes only where they are allowed.
// - pi itself, its logins and model requests, MCP servers, other extensions and your own `!`
//   commands are not sandboxed.
// - The gate: in the sandbox, `git`, `gh`, `gpg` and `ssh-keygen` are wrappers. What needs the
//   user's keys (pushing, fetching private repositories, GitHub, signing) comes to a socket this
//   extension listens on; the helper says what it would do (`gate decide`), the user is asked
//   (once, or always for this project and remote), and the helper does it (`gate run`): git with
//   the user's keys works only in the helper's own copy of the remote, never in the agent's
//   repository. Only what the command prints goes back.
//
// The rules are read again whenever the settings change, so changes apply to the next tool call.
// Pistachio writes this file on start; edits here are overwritten.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, realpathSync, statSync, unlinkSync } from "node:fs";
import { createServer } from "node:net";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";

// What the user wrote for the model in place of a tool's own text (Settings → Tools).
function edited(name: string): { description?: string; guidelines?: string[] } {
  try {
    const file = process.env.PISTACHIO_TOOL_TEXTS;
    const text = file ? JSON.parse(readFileSync(file, "utf8"))?.[name] : undefined;
    const lines = text?.guidelines;
    return {
      description: typeof text?.description === "string" ? text.description : undefined,
      guidelines: Array.isArray(lines) && lines.every((line: unknown) => typeof line === "string") ? lines : undefined,
    };
  } catch {
    return {};
  }
}

const BIN = process.env.PISTACHIO_SANDBOX_BIN;
const DIR = process.env.PISTACHIO_SANDBOX_DIR;

type Policy = {
  sandboxed: boolean;
  enforced: boolean;
  backend: { kind: string } | null;
  project: string | null;
  private: string[];
  readable: string[];
  writable: string[];
  hidden: string[];
  network: "on" | "off" | "custom";
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

function mayRead(policy: Policy, path: string): boolean {
  if (policy.hidden.some((h) => under(path, h))) return false;
  const inside = policy.private.find((p) => under(path, p));
  if (!inside) return true;
  // Only what is open inside the private folder counts, not a folder around it.
  return [...policy.readable, ...policy.writable].some((root) => under(root, inside) && under(path, root));
}

function where(policy: Policy): string {
  const others = policy.writable.filter((path) => path !== policy.project).length;
  return `${policy.project ?? "the project"}${others ? ` (and ${others} more folders such as temporary files and tool caches)` : ""}`;
}

// The helper's answer to `gate <command>` with `input` on stdin, as JSON (or null).
function gateHelper(command: string, input: unknown): any {
  try {
    const out = spawnSync(BIN!, ["--sandbox-tool", "gate", command], {
      encoding: "utf8",
      input: JSON.stringify(input),
      timeout: 30000,
    });
    return JSON.parse(out.stdout.trim().split("\n").pop() ?? "null");
  } catch {
    return null;
  }
}

// `gateHelper` without blocking pi: deciding on a push packs its commits, which can take a while.
function gateHelperLater(command: string, input: unknown): Promise<any> {
  return new Promise((done) => {
    let out = "";
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(BIN!, ["--sandbox-tool", "gate", command], { stdio: ["pipe", "pipe", "ignore"] });
    } catch {
      return done(null);
    }
    child.stdout!.setEncoding("utf8").on("data", (text: string) => (out += text));
    child.on("error", () => done(null));
    child.on("close", () => {
      try {
        done(JSON.parse(out.trim().split("\n").pop() ?? "null"));
      } catch {
        done(null);
      }
    });
    child.stdin!.on("error", () => {});
    child.stdin!.end(JSON.stringify(input));
  });
}

// Added to the system prompt in sandboxed sessions.
const GATE_NOTE = [
  "Git and GitHub in Pistachio's sandbox:",
  "- Local git (commit, branch, merge, rebase, log) works as usual, with the user's own git settings and identity.",
  "- `git push`, `pull`, `fetch`, `clone` and `ls-remote`, and `gh` for pull requests, issues, runs, releases and `gh repo create` or `delete`, run outside the sandbox with the user's logins. Pistachio shows the user a question for each; just run the command and wait for its result. Don't ask the user first yourself, and don't retry a command the user declined: ask them instead.",
  "- `gh auth status` says which GitHub accounts are logged in and the git identity commits use.",
  "- Don't set GIT_CONFIG_* or other git variables to get around an error, and don't invent a commit author: if git has no identity, ask the user.",
].join("\n");

// Added too while commands reach websites only through Pistachio's proxy.
const SITES_NOTE =
  "- Websites: commands (curl, pip, npm, cargo, git over HTTPS, …) reach the network through Pistachio's proxy. The first time a site is used, Pistachio asks the user; just run the command and wait. A site the user declined answers 403: don't try other sites or ways around it, ask the user.";

const ALLOW = "Allow";
const ALWAYS = "Always for this project";
const DENY = "Don't allow";

// Listens for the wrappers in the sandbox. Returns the socket's path, or undefined.
function startGate(project: () => string, ui: () => any): string | undefined {
  // A socket's path has room for about 100 bytes (104 on macOS); a short folder of this user's
  // otherwise.
  let dir = join(DIR!, "gate");
  if (Buffer.byteLength(join(dir, `${process.pid}.sock`)) > 100) dir = join("/tmp", `pistachio-gate-${process.getuid?.() ?? "user"}`);
  const path = join(dir, `${process.pid}.sock`);
  try {
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    try {
      unlinkSync(path);
    } catch {}
  } catch {
    return undefined;
  }
  const server = createServer((conn) => {
    let buffer = "";
    const send = (frame: unknown) => conn.write(JSON.stringify(frame) + "\n");
    const finish = (code: number, text?: string) => {
      if (text) send({ stderr: text.endsWith("\n") ? text : text + "\n" });
      send({ exit: code });
      conn.end();
    };
    const onData = async (chunk: string) => {
      buffer += chunk;
      const end = buffer.indexOf("\n");
      if (end < 0) return;
      conn.off("data", onData);
      let request: any;
      try {
        request = JSON.parse(buffer.slice(0, end));
      } catch {
        return finish(2, "[Pistachio] The wrapper's request couldn't be read.");
      }
      request.project = project();
      const decision = await gateHelperLater("decide", request);
      if (!decision) return finish(1, "[Pistachio] The gate couldn't read the request.");
      if (decision.kind === "local") {
        send({ local: true });
        return conn.end();
      }
      if (decision.kind === "refused") return finish(1, `[Pistachio] ${decision.reason}`);
      if (decision.kind === "answer") {
        send({ stdout: decision.text });
        return finish(0);
      }
      const plan = decision.plan;
      if (!decision.approved) {
        const dialog = ui();
        if (!dialog?.select) {
          return finish(1, `[Pistachio] This needs the user's OK, and no one can be asked: ${plan.title}.`);
        }
        const text = [plan.title, ...plan.details.map((line: string) => `· ${line}`), ...plan.warnings.map((line: string) => `⚠ ${line}`)].join("\n");
        // With several GitHub accounts, each answer says which one it acts as.
        const accounts: string[] = plan.accounts ?? [];
        const several = accounts.length > 1;
        const choices: { label: string; always: boolean; account?: string }[] = several
          ? [
              ...accounts.map((account) => ({ label: `${ALLOW} as ${account}`, always: false, account })),
              ...(plan.rememberable ? accounts.map((account) => ({ label: `${ALWAYS} as ${account}`, always: true, account })) : []),
            ]
          : [
              { label: ALLOW, always: false, account: accounts[0] },
              ...(plan.rememberable ? [{ label: ALWAYS, always: true, account: accounts[0] }] : []),
            ];
        let answer: string | undefined;
        try {
          answer = await dialog.select(text, [...choices.map((choice) => choice.label), DENY]);
        } catch {
          answer = undefined;
        }
        const chosen = choices.find((choice) => choice.label === answer);
        if (!chosen) {
          return finish(1, `[Pistachio] The user didn't allow this: ${plan.title}. Don't try another way; ask them if it is needed.`);
        }
        request.account = chosen.account;
        if (chosen.always) {
          gateHelper("allow", { project: request.project, target: plan.target, class: plan.class, account: chosen.account });
        }
      } else if (plan.account) {
        request.account = plan.account;
      }
      // A question only (a website for the proxy): nothing runs, the answer was the point.
      if (!plan.program) return finish(0);
      // What runs must still be what was allowed.
      request.approved = plan;
      const child = spawn(BIN!, ["--sandbox-tool", "gate", "run"], { stdio: ["pipe", "pipe", "pipe"] });
      child.stdout.setEncoding("utf8").on("data", (text: string) => send({ stdout: text }));
      child.stderr.setEncoding("utf8").on("data", (text: string) => send({ stderr: text }));
      child.on("error", (error) => finish(127, `[Pistachio] ${error}`));
      child.on("close", (code) => finish(code ?? 1));
      conn.on("close", () => child.kill());
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(request));
    };
    conn.setEncoding("utf8");
    conn.on("data", onData);
    conn.on("error", () => {});
  });
  server.on("error", () => {});
  server.listen(path);
  server.unref();
  process.on("exit", () => {
    try {
      unlinkSync(path);
    } catch {}
  });
  return path;
}

export default async function (pi: any) {
  if (!BIN || !DIR) return;
  // The latest context, for asking the user and knowing the session's folder.
  let latest: any;
  pi.on("session_start", async (_event: unknown, ctx: any) => {
    latest = ctx;
  });
  const gate = startGate(
    () => latest?.cwd ?? process.cwd(),
    () => (latest?.hasUI === false ? undefined : latest?.ui),
  );
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
        env: { ...env, PISTACHIO_SANDBOX_DIR: DIR, ...(gate ? { PISTACHIO_GATE_SOCKET: gate } : {}) },
      }),
    });
    const text = edited("bash");
    if (text.description !== undefined) bash.description = text.description;
    if (text.guidelines !== undefined) bash.promptGuidelines = text.guidelines;
    pi.registerTool(bash);
  }

  pi.on("tool_call", async (event: any, ctx: any) => {
    if (ctx) latest = ctx;
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
    if (!mayRead(policy, path)) {
      return {
        block: true,
        reason:
          `${given} is outside what Pistachio's sandbox lets this session see (the project, folders the user ` +
          `allowed and tools' folders; keys and logins never). Don't try to get at it another way; ask the user ` +
          `if it is needed (Settings → Sandboxing & Security).`,
      };
    }
    // Searching or listing a folder that holds hidden ones would show them too.
    if (tool === "grep" || tool === "find") {
      const inside = [...policy.hidden, ...policy.private].find((h) => under(h, path) && h !== path && !mayRead(policy, h));
      if (inside) {
        return {
          block: true,
          reason: `Searching ${given} would look into ${inside}, which Pistachio's sandbox hides. Search a narrower folder.`,
        };
      }
    }
    if (WRITES.has(tool) && !policy.writable.some((root) => under(path, root))) {
      return {
        block: true,
        reason: `Pistachio's sandbox only allows changes in ${where(policy)}; ${given} is outside. Ask the user if it should change (they can allow the folder in Settings → Sandboxing & Security).`,
      };
    }
  });

  // The model learns how git and GitHub work here before it tries, instead of from refusals.
  pi.on("before_agent_start", async (event: any, ctx: any) => {
    if (!gate) return;
    const policy = policyFor(ctx?.cwd ?? process.cwd());
    if (!policy?.sandboxed) return;
    const sites = policy.network === "on" ? "" : `\n${SITES_NOTE}`;
    return { systemPrompt: `${event.systemPrompt}\n\n${GATE_NOTE}${sites}` };
  });

  // A command that ran into what the sandbox blocks gets a note, so the model doesn't guess. Also
  // when it exited 0: a chain (`a && b || echo …`) hides the failure but not the message.
  pi.on("tool_result", async (event: any, ctx: any) => {
    if (event.toolName !== "bash") return;
    const text = (event.content ?? []).map((part: any) => (part.type === "text" ? part.text : "")).join("\n");
    const blocked = event.isError
      ? /Operation not permitted|Read-only file system|Permission denied|Network is unreachable|Could not resolve host|Temporary failure in name resolution|not an allowed site|CONNECT tunnel failed|403 Forbidden/i.test(
          text,
        )
      : /: Operation not permitted|: Read-only file system|not an allowed site|CONNECT tunnel failed/i.test(text);
    if (!blocked) return;
    const policy = policyFor(ctx?.cwd ?? process.cwd());
    if (!policy?.sandboxed) return;
    const network =
      policy.network === "on"
        ? ""
        : " It reaches websites through Pistachio's proxy (HTTPS_PROXY), which asks the user about each new site; a site the user declined, or one reached without the proxy, fails."
    const git = /unable to access '[^']*(\.gitconfig|\/git\/[^']*)': Operation not permitted/.test(text)
      ? " git's settings file was blocked: tell the user, and don't work around it (no GIT_CONFIG_GLOBAL, no made-up user.name or user.email)."
      : "";
    const note =
      `\n\n[Pistachio] This command ran in a sandbox: it can write only in ${where(policy)}, and of the home folder ` +
      `it sees only the project, folders the user allowed and tools' folders (keys and logins never).${network} If the sandbox blocked ` +
      `what the task needs, say so and ask the user instead of working around it (Settings → Sandboxing & Security).${git}`;
    return { content: [...(event.content ?? []), { type: "text", text: note }] };
  });
}
