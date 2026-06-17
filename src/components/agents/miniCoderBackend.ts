// Pure, DOM-free validation + normalization for the single global mini-coder
// backend config (Settings → Workspace "Mini-coder backend" card).
//
// These rules are the SINGLE source of truth for the UI form and must mirror the
// Rust boundary validation (backend/mini_coder.rs validate_mini_coder_backend) so
// a value the UI accepts is never rejected by the backend and vice-versa. Kept
// here (next to customAgentClients) so it can be unit-tested in node without DOM.

import type {
  MiniCoderBackend,
  MiniCoderBackendKind,
} from "../../types/config";

// The model tag (ollama/codex/omlx) is capped to keep `ollama run <tag>` argv sane.
export const MINI_MODEL_MAX_LENGTH = 80;
// The api CLI command shares the custom-client command cap (verbatim, stdin-fed).
export const MINI_COMMAND_MAX_LENGTH = 400;
// The omlx base URL cap. Mirrors the Rust `MINI_BASE_URL_MAX_LEN`.
export const MINI_BASE_URL_MAX_LENGTH = 200;

export const MINI_BACKEND_KINDS: readonly MiniCoderBackendKind[] = [
  "ollama",
  "api",
  "codex",
  "omlx",
  "appleFm",
] as const;

// WARNING 6: any control char (0x00-0x1f, DEL 0x7f) PLUS the bidi-control /
// zero-width / invisible-format blocklist (Unicode category Cf). The api command is
// embedded verbatim into the launch script; a control char would split it into extra
// script statements, and a bidi-override / invisible char could hide its true
// semantics. MUST stay equivalent to the Rust `is_forbidden_command_char` check. The
// `u` flag lets every forbidden code point be an explicit \u{XXXX} escape — NO literal
// invisible/bidi chars in the source (the very thing this pattern detects). Ranges,
// mirroring `is_forbidden_command_char` EXACTLY: C0 controls + DEL (U+0000-001F,
// U+007F); SOFT HYPHEN (U+00AD); ARABIC LETTER MARK (U+061C); MONGOLIAN VOWEL
// SEPARATOR (U+180E); zero-width + bidi marks (U+200B-200F); bidi embeddings/overrides
// (U+202A-202E); word-joiner..invisible-plus (U+2060-2064); bidi isolates
// (U+2066-2069); BOM/ZWNBSP (U+FEFF).
// eslint-disable-next-line no-control-regex
const CONTROL_CHAR_PATTERN =
  /[\u{0000}-\u{001F}\u{007F}\u{00AD}\u{061C}\u{180E}\u{200B}-\u{200F}\u{202A}-\u{202E}\u{2060}-\u{2064}\u{2066}-\u{2069}\u{FEFF}]/u;

// A model tag is a bare token (no whitespace, no control chars) so it is safe as
// a single `ollama run <tag>` / `codex exec -m <tag>` argv positional. Mirrors the
// Rust validator (`mini_coder::is_valid_model`).
//
// EXPORTED so the Censor oMLX model field (censorLocalAi.ts) reuses the EXACT same
// char-class — every oMLX model id (mini Rust / Censor Rust / both TS) must satisfy the
// same bare-token rule, no drift.
export const MODEL_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/;

// Validate + NORMALIZE an oMLX base URL. MUST stay EXACTLY equivalent to the Rust
// `validate_omlx_base_url` (a value one accepts the other accepts; same rejects).
//
// LOOPBACK-ONLY by design (privacy): the mini POSTs the prompt — which may carry file
// content — to this URL, so any non-loopback host could route it off the machine. The
// loopback notion mirrors Censor's `is_loopback_base` (localhost / 127.0.0.0/8 / [::1];
// userinfo `user@host` and the `127.0.0.1.evil.com` suffix trick are rejected because
// the host must PARSE as a loopback addr, not merely start with `127.`). oMLX is HTTP-ONLY
// on loopback (like Ollama): a self-signed TLS cert on a loopback oMLX server would fail
// the client's default TLS verification and silently disable the tier, so `https://` is
// rejected. Returns the normalized URL (trailing slash stripped) or null if invalid.
//
// EXPORTED so the Censor local-AI provider card (Settings → Workspace) reuses the EXACT
// same loopback/http/port rules for its oMLX base URL — one validator, no drift between
// the two oMLX surfaces (and both mirror the Rust loopback clamp).
export function validateOmlxBaseUrl(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;
  if (trimmed.length > MINI_BASE_URL_MAX_LENGTH) return null;
  // The URL is embedded verbatim into the launch script; reject the SAME control/
  // bidi/invisible blocklist as the api command (WARNING 6).
  if (CONTROL_CHAR_PATTERN.test(trimmed)) return null;

  // http only (loopback, like Ollama) — reject https (self-signed-TLS silent-degrade trap).
  let rest: string;
  if (trimmed.startsWith("http://")) {
    rest = trimmed.slice("http://".length);
  } else {
    return null;
  }

  // Authority = everything up to the first path/query/fragment delimiter.
  const authority = rest.split(/[/?#]/, 1)[0] ?? "";
  if (authority.length === 0) return null;

  let isLoopback: boolean;
  if (authority.startsWith("[::1]")) {
    // IPv6 loopback `[::1]` optionally followed by `:port`. Reject a userinfo trick
    // (`[::1]:8000@evil.com` / `[::1]:@evil.com`): an `@` in the remainder means the
    // real host is after the `@`, not the loopback addr (F1). `after` is "" or ":<port>".
    const after = authority.slice("[::1]".length);
    isLoopback =
      !after.includes("@") &&
      (after.length === 0 || after.startsWith(":")) &&
      isValidOptionalPort(after.startsWith(":") ? after.slice(1) : null);
  } else if (authority.includes("@")) {
    // Reject a userinfo trick (`127.0.0.1@evil.com`): real host is after the `@`.
    isLoopback = false;
  } else {
    // Split off an optional `:port`; IPv4/hostname hosts have no `:` in the host.
    const colon = authority.indexOf(":");
    const host = colon === -1 ? authority : authority.slice(0, colon);
    const port = colon === -1 ? null : authority.slice(colon + 1);
    isLoopback =
      (host === "localhost" || isIpv4Loopback(host)) && isValidOptionalPort(port);
  }
  if (!isLoopback) return null;

  // Normalize: strip a single trailing slash so `<baseUrl>/chat/completions` is clean.
  return trimmed.endsWith("/") ? trimmed.slice(0, -1) : trimmed;
}

// Validate + NORMALIZE a CLOUD base URL. MUST stay EXACTLY equivalent to the Rust
// `validate_cloud_base_url` (the local_coder.rs + devboule-coder versions) — a value one
// accepts the other accepts; same rejects.
//
// This is the OPT-IN, consent-gated counterpart to `validateOmlxBaseUrl`: where the loopback
// validator FORBIDS leaving the machine, this REQUIRES it — https (TLS), a NON-loopback,
// fully-qualified public host. The SAME control/bidi/invisible blocklist + the SAME
// optional-port rule apply. SSRF/privacy hardening: reject loopback hosts, bare IP literals
// (IPv4/IPv6), single-label/intranet names (require a dot), and userinfo (`user@host` /
// credentials in the URL — they belong in the Authorization header). Returns the normalized
// URL (trailing slash stripped) or null if invalid.
//
// EXPORTED so the local-coder card's `validateLocalBackend` reuses the EXACT same rules — one
// validator, no drift with the Rust boundary.
export function validateCloudBaseUrl(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;
  if (trimmed.length > MINI_BASE_URL_MAX_LENGTH) return null;
  if (CONTROL_CHAR_PATTERN.test(trimmed)) return null;

  // https ONLY (TLS): http would send the prompt (which can carry file content) in clear text.
  if (!trimmed.startsWith("https://")) return null;
  const rest = trimmed.slice("https://".length);

  const authority = rest.split(/[/?#]/, 1)[0] ?? "";
  if (authority.length === 0) return null;
  // Reject userinfo: credentials never live in the URL, and an `@` hides the real host.
  if (authority.includes("@")) return null;
  // IPv6 literal `[..]` rejected outright: a cloud provider is addressed by hostname.
  if (authority.startsWith("[")) return null;

  // Split off an optional `:port`.
  const colon = authority.indexOf(":");
  const host = colon === -1 ? authority : authority.slice(0, colon);
  const port = colon === -1 ? null : authority.slice(colon + 1);
  if (!isValidOptionalPort(port)) return null;
  if (host.length === 0) return null;
  if (host.toLowerCase() === "localhost") return null;
  // Bare IPv4 literal -> SSRF surface.
  if (isIpv4(host)) return null;
  // Mirror the Rust all-numeric-4-label fallback: Rust's Ipv4Addr parser rejects
  // leading-zero / out-of-range dotted-quads (`01.02.03.04`, `999.999.999.999`), so those
  // would otherwise look like a hostname. A numeric dotted-quad is always an IP literal.
  if (isNumericQuad(host)) return null;
  // PARTIAL SSRF mitigation (mirrors Rust): deny the cloud-metadata FQDN + the conventional
  // `.internal` / `.local` intranet suffixes. NOT complete — full protection needs
  // post-DNS-resolution IP filtering in the connect layer (a deliberate follow-up, not done).
  const hostLower = host.toLowerCase();
  if (
    hostLower === "metadata.google.internal" ||
    hostLower.endsWith(".internal") ||
    hostLower.endsWith(".local")
  ) {
    return null;
  }
  // Require a dot so a single-label intranet name (internal/metadata) cannot be targeted.
  if (!host.includes(".")) return null;
  // Each DNS label must be alnum + hyphen and non-empty.
  const labelsOk = host
    .split(".")
    .every((label) => label.length > 0 && /^[A-Za-z0-9-]+$/.test(label));
  if (!labelsOk) return null;

  return trimmed.endsWith("/") ? trimmed.slice(0, -1) : trimmed;
}

// Does `host` parse as ANY dotted-quad IPv4 literal (not just loopback)? Used by the cloud
// validator to reject a bare IP (SSRF surface). Mirrors the Rust `host.parse::<Ipv4Addr>()`.
function isIpv4(host: string): boolean {
  const parts = host.split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^[0-9]+$/.test(p) && Number(p) <= 255);
}

// Is `host` exactly 4 dot-separated all-ASCII-digit labels (regardless of leading zeros
// or range)? Mirrors the Rust all-numeric-4-label fallback in `validate_cloud_base_url`:
// it catches IP literals that Rust's strict `Ipv4Addr` parser rejects (`01.02.03.04`,
// `010.0.0.1`, `0177.0.0.1`, `999.999.999.999`) and which `isIpv4` therefore misses.
function isNumericQuad(host: string): boolean {
  const parts = host.split(".");
  return parts.length === 4 && parts.every((p) => /^[0-9]+$/.test(p));
}

// A `:port` suffix, when present, must be 1-5 ASCII digits and <= 65535 (F2). An EMPTY
// port (`host:`) is rejected as invalid. `null` = no port component to check. Mirrors the
// Rust `is_valid_optional_port`; keep both sides byte-for-byte equivalent.
function isValidOptionalPort(port: string | null): boolean {
  if (port === null) return true;
  if (port.length < 1 || port.length > 5) return false;
  if (!/^[0-9]+$/.test(port)) return false;
  return Number(port) <= 65535;
}

// Does `host` PARSE as an IPv4 address in 127.0.0.0/8? Mirrors Rust's
// `host.parse::<Ipv4Addr>().map(|ip| ip.is_loopback())`. A `startsWith("127.")` check
// would wrongly accept `127.0.0.1.evil.com`; full parsing rejects it.
function isIpv4Loopback(host: string): boolean {
  const parts = host.split(".");
  if (parts.length !== 4) return false;
  const octets: number[] = [];
  for (const p of parts) {
    // Match Rust's strict `Ipv4Addr` parser EXACTLY: 1-3 ASCII digits, NO leading
    // zero on a multi-digit octet (Rust rejects `01`, `000`), range 0-255. This is
    // stricter than a bare `/^\d+$/` and keeps the accept/reject set identical.
    if (!/^(0|[1-9][0-9]{0,2})$/.test(p)) return false;
    const n = Number(p);
    if (n > 255) return false;
    octets.push(n);
  }
  return octets[0] === 127;
}

export interface MiniBackendDraft {
  kind: MiniCoderBackendKind;
  model: string;
  command: string;
  // The oMLX base URL field. Optional so non-omlx callers (and older drafts) need
  // not supply it; treated as "" when absent. Required+validated only for kind "omlx".
  baseUrl?: string;
}

export interface MiniBackendValidation {
  ok: boolean;
  // Field-keyed inline messages; absent key == that field is valid.
  errors: Partial<Record<"model" | "command" | "baseUrl", string>>;
  // The normalized backend when ok (only the fields the kind uses are kept).
  value: MiniCoderBackend | null;
}

// Validate one draft. Pure and total: never throws, returns inline messages for
// each invalid field. Only the field(s) the kind requires are checked + kept, so
// switching kind clears stale errors for the now-unused field.
export function validateMiniBackend(
  draft: MiniBackendDraft,
): MiniBackendValidation {
  const errors: MiniBackendValidation["errors"] = {};
  const model = draft.model.trim();
  const command = draft.command.trim();
  const baseUrl = (draft.baseUrl ?? "").trim();
  // Normalized base URL (trailing slash stripped) when valid; null when invalid.
  // Computed once so the error branch and the keep-only-used-fields branch agree.
  let normalizedBaseUrl: string | null = null;

  if (draft.kind === "ollama") {
    if (model.length === 0) {
      errors.model = "Enter the Ollama model tag (e.g. qwen2.5-coder).";
    } else if (model.length > MINI_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${MINI_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(model)) {
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }
  } else if (draft.kind === "api") {
    if (command.length === 0) {
      errors.command = "Enter the CLI command line to run.";
    } else if (command.length > MINI_COMMAND_MAX_LENGTH) {
      errors.command = `Command must be at most ${MINI_COMMAND_MAX_LENGTH} characters.`;
    } else if (CONTROL_CHAR_PATTERN.test(command)) {
      errors.command =
        "Command must not contain newlines, tabs or control characters.";
    }
  } else if (draft.kind === "codex") {
    // model is OPTIONAL for codex; validate only if provided.
    if (model.length > MINI_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${MINI_MODEL_MAX_LENGTH} characters.`;
    } else if (model.length > 0 && !MODEL_PATTERN.test(model)) {
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }
  } else if (draft.kind === "omlx") {
    // omlx requires BOTH a model (bare tag, same rule as ollama) AND a loopback
    // http (only) base URL. The accept/reject set MUST match the Rust validator.
    if (model.length === 0) {
      errors.model = "Enter the oMLX model name (e.g. qwen2.5-coder).";
    } else if (model.length > MINI_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${MINI_MODEL_MAX_LENGTH} characters.`;
    } else if (!MODEL_PATTERN.test(model)) {
      errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
    }
    if (baseUrl.length === 0) {
      errors.baseUrl = "Enter the oMLX server base URL (e.g. http://localhost:8000/v1).";
    } else if (baseUrl.length > MINI_BASE_URL_MAX_LENGTH) {
      errors.baseUrl = `Base URL must be at most ${MINI_BASE_URL_MAX_LENGTH} characters.`;
    } else {
      normalizedBaseUrl = validateOmlxBaseUrl(baseUrl);
      if (normalizedBaseUrl === null) {
        errors.baseUrl =
          "Base URL must be a loopback http origin (localhost, 127.0.0.1 or [::1]).";
      }
    }
  } else if (draft.kind === "appleFm") {
    // appleFm is Apple on-device and uses an optional model field only.
    // Save remains permissive for cross-machine workflow: empty keeps only
    // `{kind:"appleFm"}`, non-empty stores `{kind:"appleFm", model}`.
    if (model.length > MINI_MODEL_MAX_LENGTH) {
      errors.model = `Model must be at most ${MINI_MODEL_MAX_LENGTH} characters.`;
    } else if (model.length > 0 && !MODEL_PATTERN.test(model)) {
      errors.model =
        "Model must be a bare tag (letters, digits, . _ : / -).";
    }
  }

  const ok = Object.keys(errors).length === 0;
  if (!ok) return { ok, errors, value: null };

  // Keep ONLY the fields the kind uses, so the persisted config is minimal and a
  // later kind switch never leaves a stale model/command behind.
  let value: MiniCoderBackend;
  if (draft.kind === "ollama") {
    value = { kind: "ollama", model };
  } else if (draft.kind === "api") {
    value = { kind: "api", command };
  } else if (draft.kind === "omlx") {
    // normalizedBaseUrl is non-null here: ok === true means no baseUrl error, and the
    // only ok-with-omlx path sets it via validateOmlxBaseUrl. Use a non-null assertion
    // (NOT `?? baseUrl`) so a future refactor that breaks this invariant surfaces
    // immediately instead of silently persisting an UNVALIDATED url (F3).
    value = { kind: "omlx", model, baseUrl: normalizedBaseUrl! };
  } else if (draft.kind === "appleFm") {
    value = model.length > 0 ? { kind: "appleFm", model } : { kind: "appleFm" };
  } else {
    value = model.length > 0 ? { kind: "codex", model } : { kind: "codex" };
  }
  return { ok: true, errors, value };
}
