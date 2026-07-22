// Pure MiniCoderBackendKind ↔ placement/engine mapping for the Roles Mini card.
// Presentation-only: every existing kind loads into a placement without data loss.

import type { MiniCoderBackendKind } from "../../types/config";

export type MiniPlacement = "On this Mac" | "Cloud API" | "Agent CLI";

/** Engines offered (selectable) in the "On this Mac" placement. */
export const MINI_LOCAL_ENGINES: readonly MiniCoderBackendKind[] = [
	"ollama",
	"omlx",
	"appleFm",
] as const;

/**
 * Engines for "Cloud API" (selectable):
 * - `cloud` = OpenAI-compatible HTTPS path (provider preset + key)
 * - `api` = original "Custom command (advanced)" shell path (stdin prompt)
 */
export const MINI_CLOUD_API_ENGINES: readonly MiniCoderBackendKind[] = [
	"cloud",
	"api",
] as const;

/**
 * Selectable Agent CLI engines. MiniCoderBackend has no `claude` kind — only
 * `codex` is a real CLI. `openai` is a stub (launched like codex); never offered
 * as a new choice — saved configs show as unsupported.
 */
export const MINI_AGENT_CLI_ENGINES: readonly MiniCoderBackendKind[] = [
	"codex",
] as const;

/** Saved-only stubs that load into Agent CLI but are not offered for new picks. */
export const MINI_AGENT_CLI_UNSUPPORTED: readonly MiniCoderBackendKind[] = [
	"openai",
] as const;

const ALL_MINI_KINDS: readonly MiniCoderBackendKind[] = [
	...MINI_LOCAL_ENGINES,
	...MINI_CLOUD_API_ENGINES,
	...MINI_AGENT_CLI_ENGINES,
	...MINI_AGENT_CLI_UNSUPPORTED,
];

/** Placement that owns a persisted kind (including unsupported/legacy). */
export function miniPlacementFromKind(kind: MiniCoderBackendKind): MiniPlacement {
	if ((MINI_LOCAL_ENGINES as readonly string[]).includes(kind)) {
		return "On this Mac";
	}
	if ((MINI_CLOUD_API_ENGINES as readonly string[]).includes(kind)) {
		return "Cloud API";
	}
	// codex + openai (stub)
	return "Agent CLI";
}

/** Selectable engine options for a placement (excludes unsupported stubs). */
export function miniEnginesForPlacement(
	placement: MiniPlacement,
): readonly MiniCoderBackendKind[] {
	switch (placement) {
		case "On this Mac":
			return MINI_LOCAL_ENGINES;
		case "Cloud API":
			return MINI_CLOUD_API_ENGINES;
		case "Agent CLI":
			return MINI_AGENT_CLI_ENGINES;
	}
}

/** True when a kind loads under this placement (including unsupported saved kinds). */
export function miniKindBelongsToPlacement(
	kind: MiniCoderBackendKind,
	placement: MiniPlacement,
): boolean {
	if (miniEnginesForPlacement(placement).includes(kind)) return true;
	// Saved-only stubs still "belong" so load/round-trip is lossless.
	if (
		placement === "Agent CLI" &&
		(MINI_AGENT_CLI_UNSUPPORTED as readonly string[]).includes(kind)
	) {
		return true;
	}
	return false;
}

export function miniKindIsUnsupported(kind: MiniCoderBackendKind): boolean {
	return (MINI_AGENT_CLI_UNSUPPORTED as readonly string[]).includes(kind);
}

/** Default kind when the user switches into a placement. */
export function defaultMiniKindForPlacement(
	placement: MiniPlacement,
): MiniCoderBackendKind {
	return miniEnginesForPlacement(placement)[0];
}

/**
 * Kind to use after a placement switch: keep current if already valid for the
 * placement (including unsupported-but-saved), otherwise the placement default.
 * Never defaults to an unsupported kind.
 */
export function miniKindAfterPlacementSwitch(
	current: MiniCoderBackendKind,
	placement: MiniPlacement,
): MiniCoderBackendKind {
	return miniKindBelongsToPlacement(current, placement)
		? current
		: defaultMiniKindForPlacement(placement);
}

/**
 * Engine labels for the Mini sub-select.
 * `api` keeps the original card wording; `openai` is saved-only unsupported.
 */
export function miniEngineLabel(kind: MiniCoderBackendKind): string {
	switch (kind) {
		case "ollama":
			return "Ollama";
		case "omlx":
			return "oMLX";
		case "appleFm":
			return "Apple on-device";
		case "cloud":
			return "HTTP API (OpenAI-compatible)";
		case "api":
			// Original KIND_LABELS: "Custom command (advanced): a shell command…"
			return "Custom command (advanced)";
		case "codex":
			return "Codex";
		case "openai":
			return "OpenAI (unsupported)";
	}
}

/** Every MiniCoderBackendKind known to the mapper (for round-trip tests). */
export function allMiniCoderBackendKinds(): readonly MiniCoderBackendKind[] {
	return ALL_MINI_KINDS;
}
