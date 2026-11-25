// Pistachio's model relay: runs in a hidden pi on this computer and sends model requests from
// pi on SSH hosts to the real providers, with this computer's credentials.
//
// Pistachio starts `pi --mode rpc --no-session --no-extensions -e <this file>` with:
//   PISTACHIO_RELAY_SOCKET       Unix socket to listen on (in a folder only this user can open), or
//   PISTACHIO_RELAY_PORT_FILE    where to write the loopback port it listens on (Windows)
//   PISTACHIO_RELAY_KEY          hex secret that signs host tokens and proofs
//   PISTACHIO_RELAY_CONTROL_KEY  hex secret that signs the app's own requests (a new one each start)
// Each host reaches it through a reverse forward on its SSH connection. On Windows, every
// program and user of this computer can reach the port too, so nothing is answered without a
// token or a signature, and no secret is ever sent as it is.
//
// A host's pi sends requests with a placeholder API key `pistachio-<nonce>-<hmac(key, nonce)>`
// (prefixed `sk-ant-oat01-` for an Anthropic login). Only nonces the app allows are accepted: a
// host gets a new one with each connection.
//
//   /p/<provider>/<scheme>/<host[:port]>/<path>   Relayed as it is to an origin that is one of
//       that provider's model endpoints here; the placeholder is replaced by the real key,
//       wherever pi put it (header or query), and the provider's headers are added.
//   /m/<provider>/messages   For logins and cloud credentials that take more than a key: pi's
//       own protocol (`pi-messages`). The request names one of this computer's models and has
//       the conversation; the pi here sends it as it would its own.
//   /_pistachio/prove/<challenge>   Proves to a host that this relay answers (see
//       `LAUNCH_SCRIPT` in the remote crate).
//   /_pistachio/models, /_pistachio/tokens   For the app.

import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import { chmodSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { Readable } from "node:stream";

const SOCKET = process.env.PISTACHIO_RELAY_SOCKET;
const PORT_FILE = process.env.PISTACHIO_RELAY_PORT_FILE;
const KEY = process.env.PISTACHIO_RELAY_KEY ?? "";
const CONTROL_KEY = process.env.PISTACHIO_RELAY_CONTROL_KEY ?? "";
// Nothing else in this pi needs them.
delete process.env.PISTACHIO_RELAY_KEY;
delete process.env.PISTACHIO_RELAY_CONTROL_KEY;

// The largest request: a long conversation with pictures.
const MAX_BODY = 64 * 1024 * 1024;
// A comment sent while a model thinks, so the host's pi does not take the silence for a lost
// connection (the provider's own keep-alives end here).
const KEEP_ALIVE_MS = 30_000;

// The nonces of the hosts' current connections.
let allowed = new Set<string>();
// The app's requests are accepted once.
const seen = new Set<string>();

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
// Answers that arrive as a stream of events (server-sent events, newline-delimited JSON).
const STREAM_TYPES = /^\s*(text\/event-stream|application\/(x-)?ndjson|application\/jsonl)\b/i;

function same(a: string, b: string): boolean {
	const x = Buffer.from(a);
	const y = Buffer.from(b);
	return x.length === y.length && timingSafeEqual(x, y);
}

function hmac(key: string, text: string | Buffer): string {
	return createHmac("sha256", Buffer.from(key, "hex")).update(text).digest("hex");
}

function validPlaceholder(text: string): boolean {
	for (const match of text.matchAll(PLACEHOLDER)) {
		if (same(hmac(KEY, match[1]), match[2]) && allowed.has(match[1])) return true;
	}
	return false;
}

/** The nonce of a request the app signed for this path and body, accepted once. */
function appNonce(req: IncomingMessage, path: string, body: Buffer): string | undefined {
	const match = /^([0-9a-f]{32})-([0-9a-f]{64})$/.exec(String(req.headers["x-pistachio-relay-auth"] ?? ""));
	if (!match) return undefined;
	const digest = createHash("sha256").update(body).digest("hex");
	if (!same(hmac(CONTROL_KEY, `request ${match[1]} ${req.method} ${path} ${digest}`), match[2])) return undefined;
	if (seen.has(match[1])) return undefined;
	seen.add(match[1]);
	return match[1];
}

/** `bytes` with every secret of 8 or more characters replaced by `[redacted]`. */
function redactBytes(bytes: Buffer, secrets: string[]): Buffer {
	const mark = Buffer.from("[redacted]");
	let out = bytes;
	for (const secret of secrets) {
		if (secret.length < 8) continue;
		const needle = Buffer.from(secret);
		const parts: Buffer[] = [];
		let start = 0;
		for (let at = out.indexOf(needle); at !== -1; at = out.indexOf(needle, start)) {
			parts.push(out.subarray(start, at), mark);
			start = at + needle.length;
		}
		if (parts.length) {
			parts.push(out.subarray(start));
			out = Buffer.concat(parts);
		}
	}
	return out;
}

function reply(res: ServerResponse, status: number, message: string) {
	if (res.headersSent) {
		res.destroy();
		return;
	}
	res.writeHead(status, { "content-type": "application/json" });
	res.end(JSON.stringify({ error: { type: "pistachio_relay", message } }));
}

class TooLarge extends Error {}

async function readBody(req: IncomingMessage, limit = MAX_BODY): Promise<Buffer> {
	const chunks: Buffer[] = [];
	let size = 0;
	for await (const chunk of req) {
		size += (chunk as Buffer).length;
		if (size > limit) throw new TooLarge(`The request is larger than ${limit} bytes.`);
		chunks.push(chunk as Buffer);
	}
	return Buffer.concat(chunks);
}

function origin(url: string | undefined): string | undefined {
	try {
		return url ? new URL(url).origin : undefined;
	} catch {
		return undefined;
	}
}

/** The app's own requests. */
// biome-ignore lint/suspicious/noExplicitAny: pi's extension context
async function control(ctx: any, path: string, req: IncomingMessage, res: ServerResponse) {
	const body = await readBody(req, 1024 * 1024);
	const nonce = appNonce(req, path, body);
	if (!nonce) return reply(res, 403, "forbidden");
	let answer: unknown;
	if (path === "/_pistachio/models" && req.method === "GET") {
		await ctx.modelRegistry.refresh({ allowNetwork: false });
		// biome-ignore lint/suspicious/noExplicitAny: pi models
		const models = ctx.modelRegistry.getAvailable().map(({ headers: _, ...model }: any) => ({
			...model,
			pistachioOAuth: ctx.modelRegistry.isUsingOAuth(model),
			pistachioMessages: typeof ctx.modelRegistry.streamSimple === "function",
		}));
		answer = { models };
	} else if (path === "/_pistachio/tokens" && req.method === "POST") {
		allowed = new Set(JSON.parse(body.toString("utf8")).nonces.map(String));
		answer = {};
	} else {
		return reply(res, 404, "Unknown relay route.");
	}
	const text = JSON.stringify(answer);
	// The app checks that this relay answered.
	const proof = hmac(CONTROL_KEY, `answer ${nonce} ${createHash("sha256").update(text).digest("hex")}`);
	res.writeHead(200, { "content-type": "application/json", "x-pistachio-relay-proof": proof });
	res.end(text);
}

const REASONING = new Set(["minimal", "low", "medium", "high", "xhigh", "max"]);
const CACHE_RETENTION = new Set(["none", "short", "long"]);

/** The options a host may choose, each checked: never a key, header, endpoint or environment. */
// biome-ignore lint/suspicious/noExplicitAny: parsed JSON
function hostOptions(raw: any): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	if (!raw || typeof raw !== "object") return out;
	if (REASONING.has(raw.reasoning)) out.reasoning = raw.reasoning;
	if (CACHE_RETENTION.has(raw.cacheRetention)) out.cacheRetention = raw.cacheRetention;
	if (Number.isFinite(raw.temperature)) out.temperature = raw.temperature;
	if (Number.isInteger(raw.maxTokens) && raw.maxTokens > 0) out.maxTokens = raw.maxTokens;
	if (typeof raw.sessionId === "string" && /^[A-Za-z0-9._-]{1,128}$/.test(raw.sessionId)) out.sessionId = raw.sessionId;
	if (raw.toolChoice === "auto" || raw.toolChoice === "none") out.toolChoice = raw.toolChoice;
	return out;
}

/** One of pi's events as `pi-messages` sends it: only that protocol's fields. */
// biome-ignore lint/suspicious/noExplicitAny: pi's assistant message events
function wireEvent(event: any, redact: (text: string) => string): Record<string, unknown> | undefined {
	const { type, contentIndex } = event;
	const block = contentIndex === undefined ? undefined : event.partial?.content?.[contentIndex];
	switch (type) {
		case "start":
			return { type };
		case "text_start":
		case "thinking_start":
			return { type, contentIndex };
		case "text_delta":
		case "thinking_delta":
		case "toolcall_delta":
			return { type, contentIndex, delta: event.delta };
		case "text_end":
			return { type, contentIndex, content: event.content, contentSignature: block?.textSignature };
		case "thinking_end":
			return {
				type,
				contentIndex,
				content: event.content,
				contentSignature: block?.thinkingSignature,
				redacted: block?.redacted,
			};
		case "toolcall_start":
			return { type, contentIndex, id: block?.id, toolName: block?.name };
		case "toolcall_end":
			return { type, contentIndex, toolCall: event.toolCall };
		case "done":
		case "error": {
			const message = type === "done" ? event.message : event.error;
			return {
				type,
				reason: event.reason,
				usage: message.usage,
				errorMessage: message.errorMessage === undefined ? undefined : redact(String(message.errorMessage)),
				responseId: message.responseId,
				responseModel: message.responseModel,
				providerThinkingLevel: message.providerThinkingLevel,
			};
		}
	}
	return undefined;
}

/** A conversation of a host's pi, sent by the pi here (`pi-messages`). */
// biome-ignore lint/suspicious/noExplicitAny: pi's extension context
async function messages(ctx: any, provider: string, req: IncomingMessage, res: ServerResponse) {
	if (req.method !== "POST") return reply(res, 405, "Unknown relay route.");
	if (!validPlaceholder(String(req.headers.authorization ?? ""))) return reply(res, 401, UNKNOWN_TOKEN);
	if (typeof ctx.modelRegistry.streamSimple !== "function") {
		return reply(res, 501, "pi on the computer running Pistachio is too old for this model.");
	}
	// biome-ignore lint/suspicious/noExplicitAny: parsed JSON
	let request: any;
	try {
		request = JSON.parse((await readBody(req)).toString("utf8"));
	} catch (error) {
		if (error instanceof TooLarge) throw error;
		return reply(res, 400, "The request is not JSON.");
	}
	const list = request?.context?.messages;
	// biome-ignore lint/suspicious/noExplicitAny: parsed JSON
	if (!Array.isArray(list) || !list.every((message: any) => message && typeof message === "object")) {
		return reply(res, 400, "The request has no conversation.");
	}
	const model = ctx.modelRegistry
		.getAvailable()
		// biome-ignore lint/suspicious/noExplicitAny: pi models
		.find((candidate: any) => candidate.provider === provider && candidate.id === request.model);
	if (!model) return reply(res, 403, `The computer running Pistachio has no model ${provider}/${request.model}.`);
	const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
	if (!auth.ok) return reply(res, 401, `${provider} on the computer running Pistachio: ${auth.error}`);

	// On the host these messages came through `pi-messages`. With the model's real API again, pi
	// keeps what only that model understands (its reasoning); it drops that of another model.
	// biome-ignore lint/suspicious/noExplicitAny: parsed JSON
	const conversation = list.map((message: any) =>
		message.role === "assistant" &&
		message.api === "pi-messages" &&
		message.provider === provider &&
		message.model === model.id
			? { ...message, api: model.api }
			: message,
	);
	const context: Record<string, unknown> = { messages: conversation };
	if (typeof request.context.systemPrompt === "string") context.systemPrompt = request.context.systemPrompt;
	if (Array.isArray(request.context.tools)) context.tools = request.context.tools;

	const secrets = [auth.apiKey, ...Object.values(auth.headers ?? {}), ...Object.values(auth.env ?? {})]
		.filter((secret) => secret !== null && secret !== undefined)
		.map(String);
	const redact = (text: string) => redactBytes(Buffer.from(text), secrets).toString("utf8");

	const abort = new AbortController();
	res.on("close", () => abort.abort());
	const events = ctx.modelRegistry.streamSimple(model, context, {
		...hostOptions(request.options),
		signal: abort.signal,
	});
	res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
	const alive = setInterval(() => res.write(": keep-alive\n\n"), KEEP_ALIVE_MS);
	try {
		for await (const event of events) {
			const wire = wireEvent(event, redact);
			if (!wire) continue;
			if (!res.write(`data: ${JSON.stringify(wire)}\n\n`) && !res.destroyed) {
				await new Promise<void>((resolve) => {
					const go = () => {
						res.off("drain", go);
						res.off("close", go);
						resolve();
					};
					res.on("drain", go);
					res.on("close", go);
				});
			}
			if (res.destroyed) break;
		}
	} finally {
		clearInterval(alive);
	}
	res.end();
}

const UNKNOWN_TOKEN =
	"This request did not come from a Pistachio session on this host, or its connection was made again. Reconnect to the host and start the session again.";

// biome-ignore lint/suspicious/noExplicitAny: pi's extension context
async function handle(ctx: any, req: IncomingMessage, res: ServerResponse) {
	const url = new URL(req.url ?? "/", "http://relay");

	const challenge = /^\/_pistachio\/prove\/([0-9a-f]{32})$/.exec(url.pathname);
	if (challenge) {
		res.writeHead(200, { "content-type": "text/plain", connection: "close" });
		res.end(hmac(KEY, `prove ${challenge[1]}`));
		return;
	}
	if (url.pathname.startsWith("/_pistachio/")) return control(ctx, url.pathname, req, res);
	const conversation = /^\/m\/([A-Za-z0-9._-]+)(\/.*)?$/.exec(url.pathname);
	if (conversation) {
		// Anything else is a model only the host's pi knows, sent the provider's own way.
		if (conversation[2] !== "/messages") {
			return reply(res, 404, `The computer running Pistachio does not have this ${conversation[1]} model.`);
		}
		return messages(ctx, conversation[1], req, res);
	}

	const route = /^\/p\/([A-Za-z0-9._-]+)\/(https?)\/([^/]+)(\/.*)?$/.exec(url.pathname);
	if (!route) return reply(res, 404, "Unknown relay route.");
	const [, provider, scheme, authority, rest] = route;

	const headerText = Object.values(req.headers).flat().join("\n");
	if (!validPlaceholder(`${headerText}\n${url.search}`)) {
		return reply(res, 401, UNKNOWN_TOKEN);
	}

	// biome-ignore lint/suspicious/noExplicitAny: pi models
	const models = ctx.modelRegistry.getAvailable().filter((model: any) => model.provider === provider);
	const requested = origin(`${scheme}://${authority}`);
	// biome-ignore lint/suspicious/noExplicitAny: pi models
	const endpoint = requested && models.find((model: any) => origin(model.baseUrl) === requested);
	if (!endpoint) {
		return reply(res, 403, `${requested} is not an endpoint of ${provider} on the computer running Pistachio.`);
	}
	// The origin requests go to comes from this computer's settings, never from the request.
	const target = origin(endpoint.baseUrl) as string;

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

	// Only the path comes from the request: it can't change where the request goes.
	const forward = new URL(target);
	forward.pathname = (rest ?? "/").replace(/\/{2,}/g, "/");
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
	if (!response.body) {
		res.writeHead(response.status, out);
		res.end();
		return;
	}
	// A body that isn't a stream of events (an error, a model list) is read whole, so a server
	// that writes the credential it got into its answer can't hand it back either. Streams go
	// through as they come: holding back their tail would delay every event.
	if (!STREAM_TYPES.test(response.headers.get("content-type") ?? "")) {
		let body: Buffer;
		try {
			body = Buffer.from(await response.arrayBuffer());
		} catch (error) {
			return reply(res, 502, `Could not read the answer of ${target}: ${error}`);
		}
		res.writeHead(response.status, out);
		res.end(redactBytes(body, secrets));
		return;
	}
	res.writeHead(response.status, out);
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
		if (server || !(SOCKET || PORT_FILE) || !KEY || !CONTROL_KEY) return;
		const started = createServer((req, res) => {
			handle(ctx, req, res).catch((error) => reply(res, error instanceof TooLarge ? 413 : 502, String(error)));
		});
		server = started;
		// WebSockets are not relayed: pi here opens its own to the provider (see `messages`).
		started.on("upgrade", (_req, socket) => socket.destroy());
		if (SOCKET) {
			try {
				unlinkSync(SOCKET);
			} catch {}
			started.listen(SOCKET, () => chmodSync(SOCKET, 0o600));
		} else if (PORT_FILE) {
			started.listen({ host: "127.0.0.1", port: 0, exclusive: true }, () => {
				const address = started.address();
				if (!address || typeof address === "string") return;
				writeFileSync(`${PORT_FILE}.tmp`, String(address.port));
				renameSync(`${PORT_FILE}.tmp`, PORT_FILE);
			});
		}
	});
	pi.on("session_shutdown", () => {
		server?.close();
		server = undefined;
	});
}
