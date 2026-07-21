import type { LocalCoderBackend, MiniCoderBackend } from "../types/config";

/** True when the Devboule in-process engine sends prompts off-device. */
export function isCloudApiBackend(
	backend: LocalCoderBackend | MiniCoderBackend | null | undefined,
): boolean {
	return backend?.kind === "cloud";
}

/**
 * Planner / spawn selector label for client id `"orchestrator"`.
 *
 * This selector is WHO runs the plan (Devboule engine vs Claude/Codex/OpenAI CLI),
 * NOT Settings → Roles placement (Local oMLX vs Cloud API OpenRouter). Always
 * "Local" here — on-device vs OpenRouter is chosen under Settings → Roles, and the
 * active model is shown next to the chip (e.g. openrouter/auto or an oMLX id).
 *
 * Do NOT rename this to "Cloud API" when kind is cloud: that made Local disappear
 * from the planner even though Local oMLX is still a first-class Settings path.
 */
export function orchestratorDevbouleLabel(
	_backend?: LocalCoderBackend | null,
): string {
	return "Local";
}

/**
 * Planner hand-off label for Main-coder engine id `"local"` (Devboule agentic).
 * Same axis as {@link orchestratorDevbouleLabel}: WHO, not placement.
 */
export function mainCoderDevbouleLabel(
	_backend?: MiniCoderBackend | null,
): string {
	return "Local (Devboule)";
}

/**
 * Optional honesty badge for Settings / status lines (not the planner WHO chips).
 * Local oMLX/Ollama vs Cloud API OpenRouter live here.
 */
export function enginePlacementBadge(
	backend: LocalCoderBackend | MiniCoderBackend | null | undefined,
): string | null {
	if (!backend) return null;
	if (backend.kind === "omlx") return "oMLX";
	if (backend.kind === "ollama") return "Ollama";
	if (backend.kind === "cloud") {
		const blob = `${backend.baseUrl ?? ""} ${backend.model ?? ""}`;
		if (/openrouter/i.test(blob)) return "Cloud API · OpenRouter";
		return "Cloud API";
	}
	return null;
}
