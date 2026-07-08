// Pure, DOM-free helpers that turn the raw `detect_providers` IPC result into the
// per-kind availability / label / model-list the detection-aware Design LLM backend card
// renders. Kept next to designLlmBackend.ts so it can be unit-tested in node without DOM.
//
// DESIGN: the card must let the user pick a provider that is REALLY available, instead of
// blind options that fail at run time. The Rust engine (`detect_providers`) reports, per
// kind, whether it can be used right now (a CLI on PATH, a reachable HTTP server) plus any
// live model tags. This module:
//   - clamps the UNTRUSTED IPC array into a kind -> status map (a stale / hand-edited /
//     duplicate / bogus entry must never crash the form),
//   - exposes a stable per-kind status (availability, secret-free detail, models),
//   - produces a short human availability LABEL per kind for the selector,
//   - decides the "selected-but-unavailable" inline hint (CLI providers only — `api` is
//     always configurable, never blocked).
//
// IT MAKES NO DECISIONS ABOUT VALIDITY of the user's draft — that stays in
// validateDesignBackend. This module is purely about "is this provider present on the box".

import type { DesignLlmBackendKind } from "../../types/config";
import { DESIGN_BACKEND_KINDS } from "./designLlmBackend";

// The CLI-launched providers. These are the ones whose absence is a hard problem: if the
// CLI is not on PATH, saving that kind yields a config that fails at generation time. `api`
// is operator-configured (any command) so it is NEVER gated on detection; ollama/omlx are
// HTTP and degrade to a hint + free-text model rather than a hard block.
export const DESIGN_CLI_KINDS: readonly DesignLlmBackendKind[] = [
	"claude",
	"codex",
] as const;

// Per-kind status distilled from one DetectedProvider (or its absence). Total + safe to
// render: `available` defaults false, `detail` is "" when unknown, `models` is always an
// array. W2: there is NO `path` field — the engine never sends the resolved CLI path over
// IPC (it would leak the filesystem layout), and the UI only needs availability.
export interface ProviderStatus {
	kind: DesignLlmBackendKind;
	available: boolean;
	// Secret-free human hint from the engine (e.g. "running", "cli only"); "" when unknown.
	detail: string;
	// Live model tags from a reachable HTTP provider; [] for CLI/api or when none found.
	models: string[];
}

// The full per-kind status map.
export type ProviderStatusMap = Record<DesignLlmBackendKind, ProviderStatus>;

// A loosely-typed view of one raw IPC entry (the boundary is untrusted — see config.ts).
interface RawDetected {
	kind?: unknown;
	available?: unknown;
	detail?: unknown;
	models?: unknown;
}

// Strip C0 controls + DEL to spaces, collapse whitespace, trim, and clamp length. The
// control-char regex uses explicit \u escapes so there are NO literal invisible chars in
// the source. The engine already redacts/sanitizes, but the IPC surface is untrusted.
function stripControl(raw: string, max: number): string {
	const clean = raw
		.replace(/[\u{0000}-\u{001F}\u{007F}]/gu, " ")
		.replace(/\s+/g, " ")
		.trim();
	return clean.length > max ? clean.slice(0, max) : clean;
}

// Coerce an unknown to a trimmed, capped, single-line string ("" for anything else). The
// goal is only to keep the form from rendering a pathological value, not to re-validate.
function safeString(raw: unknown, max: number): string {
	if (typeof raw !== "string") return "";
	return stripControl(raw, max);
}

// Coerce an unknown to a clean string[] of model tags. Drops non-strings/empties, dedupes,
// and caps the count so a hostile/huge array can't blow up the dropdown.
function safeModels(raw: unknown): string[] {
	if (!Array.isArray(raw)) return [];
	const out: string[] = [];
	const seen = new Set<string>();
	for (const item of raw) {
		const s = safeString(item, 120);
		if (s.length === 0 || seen.has(s)) continue;
		seen.add(s);
		out.push(s);
		if (out.length >= 100) break;
	}
	return out;
}

function emptyStatus(kind: DesignLlmBackendKind): ProviderStatus {
	return { kind, available: false, detail: "", models: [] };
}

// Build the per-kind status map from the raw IPC array. Total: a null/undefined/non-array
// input yields an all-unavailable map; unknown/duplicate kinds are ignored (first wins).
// `api` is forced available === true in the map regardless of what the engine reports,
// because it is always configurable (the card never blocks it).
export function buildProviderStatusMap(
	detected: readonly RawDetected[] | null | undefined,
): ProviderStatusMap {
	const map = {} as ProviderStatusMap;
	for (const kind of DESIGN_BACKEND_KINDS) {
		map[kind] = emptyStatus(kind);
	}

	if (Array.isArray(detected)) {
		const filled = new Set<DesignLlmBackendKind>();
		for (const entry of detected) {
			const kind = typeof entry?.kind === "string" ? entry.kind : "";
			if (!(DESIGN_BACKEND_KINDS as readonly string[]).includes(kind)) continue;
			const typed = kind as DesignLlmBackendKind;
			// First entry for a kind wins (ignore accidental duplicates).
			if (filled.has(typed)) continue;
			filled.add(typed);
			map[typed] = {
				kind: typed,
				available: entry.available === true,
				detail: safeString(entry.detail, 160),
				models: safeModels(entry.models),
			};
		}
	}

	// `api` is always configurable — never gate it on detection.
	map.api = { ...map.api, available: true };
	return map;
}

// Human-readable static base label per kind for the selector option. Detection state is
// appended separately (see selectorLabel) so the base never drifts from the prose.
const BASE_LABELS: Record<DesignLlmBackendKind, string> = {
	codex: "Codex (subscription)",
	openai: "OpenAI (API)",
	claude: "Claude (subscription)",
	ollama: "Ollama (local model)",
	omlx: "oMLX (local MLX server)",
	api: "API CLI (your command)",
};

export function baseLabel(kind: DesignLlmBackendKind): string {
	return BASE_LABELS[kind];
}

// The short availability suffix shown after the base label in the selector + in the status
// row, e.g. "detected", "not found", "running (3 models)", "cli only". `api` is special:
// it is always "configurable". Pure; never throws.
export function availabilityLabel(status: ProviderStatus): string {
	if (status.kind === "api") return "configure a command";
	if (!status.available) return "not found";
	// HTTP providers: prefer a model-count hint when models were discovered.
	if (status.kind === "ollama" || status.kind === "omlx") {
		if (status.models.length > 0) {
			const n = status.models.length;
			return `running (${n} model${n === 1 ? "" : "s"})`;
		}
		// Available but no models surfaced: fall back to the engine detail or a generic hint.
		return status.detail || "running";
	}
	// CLI providers (claude/codex): "detected" (+ detail if present, e.g. version).
	return status.detail ? `detected (${status.detail})` : "detected";
}

// The full selector option label: base + availability suffix.
export function selectorLabel(status: ProviderStatus): string {
	return `${baseLabel(status.kind)} — ${availabilityLabel(status)}`;
}

// Whether the currently-selected kind is an UNAVAILABLE CLI provider — the only case that
// gets the inline "install it / put it on PATH" hint and blocks Save. `api` is never
// blocked; ollama/omlx are HTTP and only get a soft hint (they may simply not be running),
// so they are NOT returned here. Returns the hint string when applicable, else null.
export function selectedUnavailableHint(
	kind: DesignLlmBackendKind,
	map: ProviderStatusMap,
): string | null {
	if (!(DESIGN_CLI_KINDS as readonly string[]).includes(kind)) return null;
	const status = map[kind];
	if (status.available) return null;
	const name = kind === "claude" ? "Claude" : "Codex";
	return `${name} was not found on this PC — install it or make sure it is on your PATH, then re-detect.`;
}

// Whether selecting `kind` should HARD-BLOCK Save (independent of draft validity). Only an
// unavailable CLI provider blocks; api/ollama/omlx never do (api is configurable; ollama/
// omlx may just be offline and the user can still save a free-text model to use later).
export function isKindBlocked(
	kind: DesignLlmBackendKind,
	map: ProviderStatusMap,
): boolean {
	return selectedUnavailableHint(kind, map) !== null;
}

// A soft hint for an HTTP provider (ollama/omlx) that detection did NOT find available —
// it likely just is not running. Not a hard block (unlike CLI). Returns null when the
// provider IS available or the kind is not an HTTP provider.
export function offlineHttpHint(
	kind: DesignLlmBackendKind,
	map: ProviderStatusMap,
): string | null {
	if (kind !== "ollama" && kind !== "omlx") return null;
	if (map[kind].available) return null;
	const name = kind === "ollama" ? "Ollama" : "the oMLX server";
	return `${name} was not detected — it may not be running. You can still enter a model tag and start it later.`;
}
