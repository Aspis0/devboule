import { describe, it, expect } from "vitest";
import { resolveOrchestratorClient } from "./orchestratorClient";

describe("resolveOrchestratorClient", () => {
	it("maps local -> orchestrator (the local Devboule Stage/TUI)", () => {
		expect(resolveOrchestratorClient("local")).toBe("orchestrator");
	});

	it("maps claude -> claude unchanged", () => {
		expect(resolveOrchestratorClient("claude")).toBe("claude");
	});

	it("maps codex -> codex (audit finding #1: /model codex handled)", () => {
		expect(resolveOrchestratorClient("codex")).toBe("codex");
	});

	it("maps openai -> codex (audit finding #1: openai is an alias for the OpenAI-compatible Codex CLI)", () => {
		expect(resolveOrchestratorClient("openai")).toBe("codex");
	});

	it("returns null for unknown/empty tokens (no-op, never an invalid id)", () => {
		expect(resolveOrchestratorClient("gemini")).toBeNull();
		expect(resolveOrchestratorClient("")).toBeNull();
		expect(resolveOrchestratorClient(undefined)).toBeNull();
		expect(resolveOrchestratorClient(null)).toBeNull();
	});
});
