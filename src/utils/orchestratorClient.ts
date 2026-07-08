// Maps a `/model` provider token (the first arg of the `model` slash command) to
// a valid planner orchestrator backend id. The valid orchestrator ids are
// "orchestrator" (the local Devboule Stage/TUI), "claude", and "codex".
//
// "openai" is NOT a real orchestrator id, but the OpenAI-compatible cloud CLI is
// Codex, so "openai" maps to "codex" for now (audit finding #1).
export type OrchestratorClient = "orchestrator" | "claude" | "codex";

export function resolveOrchestratorClient(
	provider: string | undefined | null,
): OrchestratorClient | null {
	switch (provider) {
		case "local":
			return "orchestrator";
		case "claude":
			return "claude";
		// Codex is the OpenAI-compatible cloud CLI, so "openai" is an alias for it.
		case "openai":
		case "codex":
			return "codex";
		default:
			return null;
	}
}
