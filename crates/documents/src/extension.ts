// Has pi's `read` return PDF files as Markdown in Pistachio's local Code sessions. Everything else
// about `read` stays pi's own: it decides whether the file may be read, and this only replaces what
// a PDF's bytes say with its text, which Pistachio's own executable reads (run as
// `<exe> --documents-tool parse` with JSON on stdin). `offset` and `limit` are lines of the
// Markdown. Pistachio writes this file on start; edits here are overwritten.
import { spawn } from "node:child_process";
import { constants } from "node:fs";
import { access, open } from "node:fs/promises";
import { basename } from "node:path";

const BIN = process.env.PISTACHIO_DOCUMENTS_BIN;

type Reply = { ok: true; markdown: string; parser: string; note: string | null } | { ok: false; error: string };

function parse(bin: string, path: string, signal?: AbortSignal): Promise<Reply> {
  return new Promise((resolve) => {
    if (signal?.aborted) {
      resolve({ ok: false, error: "cancelled" });
      return;
    }
    const child = spawn(bin, ["--documents-tool", "parse"], { stdio: ["pipe", "pipe", "ignore"], signal });
    let out = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (out += chunk));
    child.on("error", (error) => resolve({ ok: false, error: signal?.aborted ? "cancelled" : String(error) }));
    child.on("close", () => {
      try {
        // The reply is the last line: a library may print something before it.
        resolve(JSON.parse(out.trim().split("\n").pop() ?? ""));
      } catch {
        resolve({ ok: false, error: "its reader stopped" });
      }
    });
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify({ path }));
  });
}

// Whether the file starts like a PDF: something else with that name is left to pi.
async function isPdf(path: string): Promise<boolean> {
  const file = await open(path, "r");
  try {
    const start = Buffer.alloc(1024);
    const { bytesRead } = await file.read(start, 0, start.length, 0);
    return start.subarray(0, bytesRead).includes("%PDF-");
  } finally {
    await file.close();
  }
}

export default function (pi: any) {
  if (!BIN) return;
  pi.on("tool_result", async (event: any, ctx: any) => {
    if (event.toolName !== "read") return;
    const path = event.input?.path;
    if (typeof path !== "string" || !/\.pdf$/i.test(path.trim())) return;
    let reply: Reply | undefined;
    try {
      // pi's own `read`, of the Markdown instead of the file: it finds the file and cuts the text
      // like any other.
      const { createReadTool } = await import("@earendil-works/pi-coding-agent");
      const tool = createReadTool(ctx.cwd, {
        operations: {
          access: (file: string) => access(file, constants.R_OK),
          detectImageMimeType: async () => undefined,
          readFile: async (file: string) => {
            if (!(await isPdf(file))) throw new Error("not a PDF");
            reply = await parse(BIN, file, ctx.signal);
            if (!reply.ok) throw new Error(reply.error);
            const note = reply.note ? ` ${reply.note}` : "";
            const heading = `[The text of ${basename(file)}, as Markdown.${note}]`;
            return Buffer.from(`${heading}\n\n${reply.markdown}`);
          },
        },
      });
      const result: any = await tool.execute(event.toolCallId, event.input, ctx.signal, undefined, ctx);
      const text = result.content?.find((part: any) => part.type === "text")?.text;
      const structured = event.structuredContent === undefined ? {} : { structuredContent: text };
      return { content: result.content, details: result.details ?? {}, isError: false, ...structured };
    } catch (error) {
      // Not found, not allowed, not a PDF or cancelled: what pi said about it stays.
      if (!reply || ctx.signal?.aborted) return;
      const reason = error instanceof Error ? error.message : String(error);
      const text = reply.ok ? reason : `This PDF could not be read as text: ${reason}.`;
      return { content: [{ type: "text", text }], details: {}, isError: true };
    }
  });
}
