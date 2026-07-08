// Tests for DesignLlmBackendCard — focused on A2: the card must NOT DROP the
// effort/timeoutSecs knobs (owned by the composer's model popover, not this card) when it
// saves the backend.
//
// The vitest environment here is `node` (see vitest.config.ts), so we cannot drive a real
// click → save. Instead we assert the exact save contract the card relies on: the card
// threads `current.effort`/`current.timeoutSecs` into the draft it validates, and
// `validateDesignBackend` carries them onto `validation.value` (which is exactly what
// `onSave` persists via set_design_llm_backend). A static render smoke confirms the card
// mounts cleanly with a fields-carrying config.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { AppConfig } from "../../types/config";
import { validateDesignBackend } from "../design/designLlmBackend";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

const invokeMock = vi.fn(async () => null);
let currentConfig: AppConfig["designLlmBackend"] | undefined;

vi.mock("../../context/AppContext", () => ({
	invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
	useAppContext: () => ({
		config: { designLlmBackend: currentConfig } as AppConfig,
	}),
	useAppActions: () => ({ refreshConfig: vi.fn(async () => undefined) }),
}));

import { DesignLlmBackendCard } from "./DesignLlmBackendCard";

beforeEach(() => {
	invokeMock.mockClear();
	currentConfig = undefined;
});

describe("DesignLlmBackendCard — effort/timeout are not dropped on save (A2)", () => {
	it("carries persisted effort/timeoutSecs onto the value the card would save", () => {
		// Reproduce EXACTLY the draft the card builds in its `validation` useMemo: the editable
		// fields plus the preserved knobs from the current backend.
		const current = {
			kind: "claude" as const,
			effort: "high" as const,
			timeoutSecs: 300,
		};
		const result = validateDesignBackend({
			kind: current.kind,
			model: "",
			command: "",
			baseUrl: "",
			effort: current.effort,
			timeoutSecs: current.timeoutSecs,
		});
		expect(result.ok).toBe(true);
		// The save payload (validation.value) must STILL carry the knobs the card did not edit.
		expect(result.value).toEqual({
			kind: "claude",
			effort: "high",
			timeoutSecs: 300,
		});
	});

	it("does not invent knobs when the current backend has none", () => {
		const result = validateDesignBackend({
			kind: "claude",
			model: "",
			command: "",
			baseUrl: "",
			effort: undefined,
			timeoutSecs: undefined,
		});
		expect(result.value).toEqual({ kind: "claude" });
	});

	it("mounts cleanly when the current backend carries effort/timeoutSecs", () => {
		currentConfig = { kind: "claude", effort: "medium", timeoutSecs: 240 };
		// Must not throw; the card edits only kind/model/command/baseUrl but tolerates the
		// extra persisted fields on `current`.
		const html = renderToStaticMarkup(<DesignLlmBackendCard />);
		expect(html).toContain("Design LLM backend");
	});
});
