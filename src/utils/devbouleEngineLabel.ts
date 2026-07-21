import type { LocalCoderBackend, MiniCoderBackend } from "../types/config";

/** True when the Devboule in-process engine sends prompts off-device. */
export function isCloudApiBackend(
	backend: LocalCoderBackend | MiniCoderBackend | null | undefined,
): boolean {
	return backend?.kind === "cloud";
}

/**
 * Label for the planner / spawn selector id `"orchestrator"` (Devboule engine).
 * Must not say "Local" when Settings → Roles uses Cloud API (e.g. OpenRouter) —
 * prompts leave the machine. OpenAI CLI is a different client id (`openai`).
 */
export function orchestratorDevbouleLabel(
	backend: LocalCoderBackend | null | undefined,
): string {
	if (!isCloudApiBackend(backend)) return "Local";
	const blob = `${backend?.baseUrl ?? ""} ${backend?.model ?? ""}`;
	if (/openrouter/i.test(blob)) return "Cloud API (OpenRouter)";
	return "Cloud API";
}

/**
 * Label for Main-coder hand-off id `"local"` (in-process agentic engine).
 * Same honesty rule as the orchestrator Devboule engine.
 */
export function mainCoderDevbouleLabel(
	backend: MiniCoderBackend | null | undefined,
): string {
	if (!isCloudApiBackend(backend)) return "Local (Devboule)";
	const blob = `${backend?.baseUrl ?? ""} ${backend?.model ?? ""}`;
	if (/openrouter/i.test(blob)) return "Cloud API (OpenRouter)";
	return "Cloud API (Devboule)";
}
