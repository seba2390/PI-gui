// Pistachio's model relay: runs in a hidden pi on this computer and sends model requests from
// pi on SSH hosts to the real providers, with this computer's credentials.
//
// Pistachio starts `pi --mode rpc --no-session --no-extensions -e <this file>` with:
//   PISTACHIO_RELAY_SOCKET  Unix socket to listen on (in a folder only this user can open)
//   PISTACHIO_RELAY_KEY     hex secret: authorises GET /_pistachio/models and signs host tokens
// Each host reaches the socket through a reverse forward on its SSH connection.
//
// A host's pi sends requests to http://127.0.0.1:<port>/p/<provider>/<scheme>/<host[:port]>/<path>
// with a placeholder API key `pistachio-<nonce>-<hmac(key, nonce)>` (prefixed `sk-ant-oat01-`
// for an Anthropic login). A request is relayed only when the placeholder is valid and the
// origin is one of that provider's model endpoints here; the placeholder is replaced by the
// real key, wherever pi put it (header or query), and the provider's headers are added.

import { createHmac, timingSafeEqual } from "node:crypto";
import { chmodSync, unlinkSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { Readable } from "node:stream";

const SOCKET = process.env.PISTACHIO_RELAY_SOCKET;
const KEY = process.env.PISTACHIO_RELAY_KEY ?? "";
// Nothing else in this pi needs it.
delete process.env.PISTACHIO_RELAY_KEY;

const PLACEHOLDER = /(?:sk-ant-oat01-)?pistachio-([0-9a-f]{16})-([0-9a-f]{64})/g;
const HOP_BY_HOP = new Set([
	"host",
	"connection",
	"keep-alive",
	"proxy-authorization",
	"proxy-connection",
	"te",
	"trailer",
	"transfer-encoding",
	"upgrade",
	"content-length",
	// fetch negotiates (and undoes) its own compression.
	"accept-encoding",
]);
const RESPONSE_DROP = new Set(["connection", "keep-alive", "transfer-encoding", "content-encoding", "content-length"]);
// Where pi sends a key. The placeholder is swapped only here: anywhere else (a header or query
// value the server might echo back) it could carry the real key to the host.
const KEY_HEADERS = new Set(["authorization", "x-api-key", "api-key", "x-goog-api-key"]);
const KEY_PARAMS = new Set(["key"]);

function same(a: string, b: string): boolean {
	const x = Buffer.from(a);
	const y = Buffer.from(b);
	return x.length === y.length && timingSafeEqual(x, y);
}

function validPlaceholder(text: string): boolean {
	for (const match of text.matchAll(PLACEHOLDER)) {
		const mac = createHmac("sha256", Buffer.from(KEY, "hex")).update(match[1]).digest("hex");
		if (same(mac, match[2])) return true;
	}
	return false;
}

function reply(res: ServerResponse, status: number, message: string) {
	if (res.headersSent) {
		res.destroy();
		return;
	}
	res.writeHead(status, { "content-type": "application/json" });
	res.end(JSON.stringify({ error: { type: "pistachio_relay", message } }));
}

async function readBody(req: IncomingMessage): Promise<Buffer> {
	const chunks: Buffer[] = [];
	for await (const chunk of req) chunks.push(chunk as Buffer);
	return Buffer.concat(chunks);
}

function origin(url: string | undefined): string | undefined {
	try {
		return url ? new URL(url).origin : undefined;
	} catch {
		return undefined;
	}
}

// biome-ignore lint/suspicious/noExplicitAny: pi's extension context
async function handle(ctx: any, req: IncomingMessage, res: ServerResponse) {
	const url = new URL(req.url ?? "/", "http://relay");

	if (url.pathname === "/_pistachio/models") {
		if (!same(String(req.headers["x-pistachio-relay-key"] ?? ""), KEY)) return reply(res, 403, "forbidden");
		await ctx.modelRegistry.refresh({ allowNetwork: false });
		// biome-ignore lint/suspicious/noExplicitAny: pi models
		const models = ctx.modelRegistry.getAvailable().map((model: any) => ({
			...model,
			pistachioOAuth: ctx.modelRegistry.isUsingOAuth(model),
		}));
		res.writeHead(200, { "content-type": "application/json" });
		res.end(JSON.stringify({ models }));
		return;
	}

	const route = /^\/p\/([A-Za-z0-9._-]+)\/(https?)\/([^/]+)(\/.*)?$/.exec(url.pathname);
	if (!route) return reply(res, 404, "Unknown relay route.");
	const [, provider, scheme, authority, rest] = route;

	const headerText = Object.values(req.headers).flat().join("\n");
	if (!validPlaceholder(`${headerText}\n${url.search}`)) {
		return reply(res, 401, "This request did not come from a Pistachio session on this host.");
	}

	// biome-ignore lint/suspicious/noExplicitAny: pi models
	const models = ctx.modelRegistry.getAvailable().filter((model: any) => model.provider === provider);
	const target = origin(`${scheme}://${authority}`);
	// biome-ignore lint/suspicious/noExplicitAny: pi models
	if (!target || !models.some((model: any) => origin(model.baseUrl) === target)) {
		return reply(res, 403, `${target} is not an endpoint of ${provider} on the computer running Pistachio.`);
	}

	const body = req.method === "GET" || req.method === "HEAD" ? undefined : await readBody(req);
	// The request names its model (OpenAI and Anthropic APIs); its own headers then apply too.
	let model = models[0];
	if (body?.length) {
		try {
			const id = JSON.parse(body.toString("utf8"))?.model;
			// biome-ignore lint/suspicious/noExplicitAny: pi models
			model = models.find((candidate: any) => candidate.id === id) ?? model;
		} catch {}
	}
	const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
	if (!auth.ok) return reply(res, 401, `${provider} on the computer running Pistachio: ${auth.error}`);
	const key: string = auth.apiKey ?? "";

	// A function, so a `$` in the key is never read as a replacement pattern.
	const swap = (text: string) => text.replace(PLACEHOLDER, () => key);
	const carries = (text: string) => {
		PLACEHOLDER.lastIndex = 0;
		return PLACEHOLDER.test(text);
	};
	const headers = new Headers();
	for (const [name, value] of Object.entries(req.headers)) {
		if (value === undefined || HOP_BY_HOP.has(name)) continue;
		for (const one of Array.isArray(value) ? value : [value]) {
			if (!carries(one)) {
				headers.append(name, one);
				continue;
			}
			if (!KEY_HEADERS.has(name)) continue;
			const swapped = swap(one).trim();
			// Without a key here, a header that only carried the placeholder goes.
			if (swapped === "" || /^Bearer$/i.test(swapped)) continue;
			headers.append(name, swapped);
		}
	}
	const secrets = [key];
	for (const [name, value] of Object.entries(auth.headers ?? {})) {
		if (value === null || value === undefined) headers.delete(name);
		else {
			headers.set(name, String(value));
			secrets.push(String(value));
		}
	}

	const forward = new URL(`${target}${(rest ?? "/").replace(/\/{2,}/g, "/")}`);
	for (const [name, value] of url.searchParams) {
		if (!carries(value)) forward.searchParams.append(name, value);
		else if (KEY_PARAMS.has(name) && key) forward.searchParams.append(name, swap(value));
	}
	// Nothing in the response headers (a redirect's Location, say) may hand the key back.
	const redact = (text: string) =>
		secrets.reduce((out, secret) => (secret.length >= 8 ? out.split(secret).join("[redacted]") : out), text);

	const abort = new AbortController();
	res.on("close", () => {
		if (!res.writableFinished) abort.abort();
	});
	let response: Response;
	try {
		response = await fetch(forward, {
			method: req.method,
			headers,
			body,
			redirect: "manual",
			signal: abort.signal,
		});
	} catch (error) {
		return reply(res, 502, `Could not reach ${target} from the computer running Pistachio: ${error}`);
	}
	const out: Record<string, string> = {};
	response.headers.forEach((value, name) => {
		if (!RESPONSE_DROP.has(name)) out[name] = redact(value);
	});
	res.writeHead(response.status, out);
	if (!response.body) {
		res.end();
		return;
	}
	// biome-ignore lint/suspicious/noExplicitAny: web stream to node stream
	Readable.fromWeb(response.body as any)
		.on("error", () => res.destroy())
		.pipe(res);
}

// biome-ignore lint/suspicious/noExplicitAny: pi's extension API
export default function (pi: any) {
	let server: Server | undefined;
	// biome-ignore lint/suspicious/noExplicitAny: pi's extension context
	pi.on("session_start", (_event: unknown, ctx: any) => {
		if (server || !SOCKET || !KEY) return;
		server = createServer((req, res) => {
			handle(ctx, req, res).catch((error) => reply(res, 502, String(error)));
		});
		// WebSockets (Codex) are not relayed yet.
		server.on("upgrade", (_req, socket) => socket.destroy());
		try {
			unlinkSync(SOCKET);
		} catch {}
		server.listen(SOCKET, () => chmodSync(SOCKET, 0o600));
	});
	pi.on("session_shutdown", () => {
		server?.close();
		server = undefined;
	});
}
