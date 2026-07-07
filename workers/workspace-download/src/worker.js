/**
 * Devboule workspace bootstrap download Worker.
 *
 * Serves the encrypted `.aspiswspkg` bootstrap package from an R2 bucket to a
 * collaborator who does NOT yet have the project folder. This Worker is the
 * transport host ONLY: the package is already AES-256-GCM encrypted, Ed25519
 * signed, and its data key is wrapped solely for approved device fingerprints by
 * the desktop app BEFORE upload. The Worker never sees plaintext and performs no
 * decryption — the desktop app still runs the full signature-verified decrypt on
 * the bytes it receives here.
 *
 * The `DOWNLOAD_TOKEN` gate is defense-in-depth (it stops casual access and
 * bucket enumeration), NOT the confidentiality boundary — that is the
 * client-side crypto. Pass the token as `?t=<token>` (the desktop app fetches a
 * plain URL) or as an `Authorization: Bearer <token>` header.
 */

export default {
	async fetch(request, env) {
		if (request.method !== "GET" && request.method !== "HEAD") {
			return new Response("Method Not Allowed", { status: 405 });
		}

		const url = new URL(request.url);

		// --- auth gate ---------------------------------------------------------
		// Fail closed if the secret was never set, so a half-configured Worker does
		// not serve packages without a token.
		const expected = env.DOWNLOAD_TOKEN;
		if (!expected) {
			return new Response("Not Found", { status: 404 });
		}
		const headerToken = (request.headers.get("Authorization") || "").replace(
			/^Bearer\s+/i,
			"",
		);
		const provided = url.searchParams.get("t") || headerToken;
		if (!timingSafeEqual(provided, expected)) {
			// 404 (not 401/403) so an unauthenticated probe cannot tell the route apart
			// from a missing object.
			return new Response("Not Found", { status: 404 });
		}

		// --- object key: package files only, no traversal ----------------------
		let key;
		try {
			key = decodeURIComponent(url.pathname.replace(/^\/+/, ""));
		} catch {
			return new Response("Not Found", { status: 404 });
		}
		// A single path segment ending in `.aspiswspkg`; no slashes, dots-runs or
		// separators, so the key can never address anything but a bootstrap package.
		if (!/^[A-Za-z0-9._-]+\.aspiswspkg$/.test(key) || key.includes("..")) {
			return new Response("Not Found", { status: 404 });
		}

		// --- range support (resumable 1 GiB download) --------------------------
		const rangeHeader = request.headers.get("Range");
		const r2Options = {};
		if (rangeHeader) {
			const parsed = parseRange(rangeHeader);
			if (parsed) r2Options.range = parsed;
		}

		const object = await env.WORKSPACE_BUCKET.get(key, r2Options);
		if (object === null) {
			return new Response("Not Found", { status: 404 });
		}

		const headers = new Headers();
		object.writeHttpMetadata(headers);
		headers.set("etag", object.httpEtag);
		headers.set("content-type", "application/octet-stream");
		headers.set("content-disposition", `attachment; filename="${key}"`);
		headers.set("accept-ranges", "bytes");
		headers.set("cache-control", "no-store");

		const body = request.method === "HEAD" ? null : object.body;

		// A ranged hit returns 206 + Content-Range; a full hit returns 200.
		if (object.range && rangeHeader) {
			const offset = object.range.offset ?? 0;
			const length = object.range.length ?? object.size - offset;
			const end = offset + length - 1;
			headers.set("content-range", `bytes ${offset}-${end}/${object.size}`);
			headers.set("content-length", String(length));
			return new Response(body, { status: 206, headers });
		}

		headers.set("content-length", String(object.size));
		return new Response(body, { status: 200, headers });
	},
};

/** Constant-time string compare; false on any length mismatch. */
function timingSafeEqual(a, b) {
	const enc = new TextEncoder();
	const ba = enc.encode(a || "");
	const bb = enc.encode(b || "");
	if (ba.length !== bb.length) return false;
	let diff = 0;
	for (let i = 0; i < ba.length; i++) diff |= ba[i] ^ bb[i];
	return diff === 0;
}

/** Parse a single `bytes=START-END` range header into R2's range option. */
function parseRange(header) {
	const match = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
	if (!match) return null;
	const [, startRaw, endRaw] = match;
	if (startRaw === "" && endRaw === "") return null;
	if (startRaw === "") {
		// suffix range: the last N bytes
		return { suffix: Number(endRaw) };
	}
	const offset = Number(startRaw);
	if (endRaw === "") return { offset };
	return { offset, length: Number(endRaw) - offset + 1 };
}
