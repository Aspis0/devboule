// @vitest-environment jsdom
//
// OrchestratorHeroCard — the Projects "talk to the Orchestrator" composer. Pure props/callbacks,
// no backend; uses the repo's createRoot + act jsdom pattern.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { OrchestratorHeroCard } from "./OrchestratorHeroCard";

(
	globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
	container = document.createElement("div");
	document.body.appendChild(container);
	root = createRoot(container);
});

afterEach(() => {
	act(() => root.unmount());
	container.remove();
	vi.restoreAllMocks();
});

const CODERS = [
	{ id: "claude", label: "Claude" },
	{ id: "codex", label: "Codex" },
];

function mount(
	props: Partial<Parameters<typeof OrchestratorHeroCard>[0]> = {},
) {
	const full = {
		projectName: "Devboule API",
		hasRoot: true,
		language: "Rust · Tauri",
		plannerModel: "Claude Opus",
		coders: CODERS,
		...props,
	};
	act(() => root.render(createElement(OrchestratorHeroCard, full)));
}

function setGoal(value: string) {
	const ta = container.querySelector("textarea") as HTMLTextAreaElement;
	const setter = Object.getOwnPropertyDescriptor(
		window.HTMLTextAreaElement.prototype,
		"value",
	)!.set!;
	act(() => {
		setter.call(ta, value);
		ta.dispatchEvent(new Event("input", { bubbles: true }));
	});
}

function planButton(): HTMLButtonElement {
	return [...container.querySelectorAll("button")].find((b) =>
		/Plan it|Planning/.test(b.textContent ?? ""),
	) as HTMLButtonElement;
}

describe("OrchestratorHeroCard", () => {
	it("renders the composer, language badge, planner model, and the real coders", () => {
		mount();
		const text = container.textContent ?? "";
		expect(text).toContain("What should we build?");
		expect(text).toContain("Rust · Tauri");
		expect(text).toContain("Planner: Claude Opus");
		expect(text).toContain("Claude");
		expect(text).toContain("Codex");
	});

	it("keeps Plan it disabled until there is a root, a goal, AND an onPlan handler", () => {
		// no onPlan + no goal → disabled
		mount({ onPlan: undefined });
		expect(planButton().disabled).toBe(true);
		// onPlan + root but empty goal → still disabled
		act(() => root.unmount());
		container.remove();
		container = document.createElement("div");
		document.body.appendChild(container);
		root = createRoot(container);
		mount({ onPlan: vi.fn() });
		expect(planButton().disabled).toBe(true);
	});

	it("disables Plan it when the project has no working folder", () => {
		mount({ hasRoot: false, onPlan: vi.fn() });
		setGoal("Add billing");
		expect(planButton().disabled).toBe(true);
		expect(container.textContent).toContain("working folder");
	});

	it("emits onPlan with the goal and the selected coder", () => {
		const onPlan = vi.fn();
		mount({ onPlan });
		setGoal("Add Stripe billing");
		// pick the Codex coder, then plan
		const codexChip = [...container.querySelectorAll("button")].find((b) =>
			(b.textContent ?? "").includes("Codex"),
		) as HTMLButtonElement;
		act(() =>
			codexChip.dispatchEvent(new MouseEvent("click", { bubbles: true })),
		);
		act(() =>
			planButton().dispatchEvent(new MouseEvent("click", { bubbles: true })),
		);
		expect(onPlan).toHaveBeenCalledWith("Add Stripe billing", "codex", true);
	});
});
