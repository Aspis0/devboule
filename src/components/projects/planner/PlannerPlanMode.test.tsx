// @vitest-environment jsdom
//
// S4 (planner simplification): the chat is now the first substantial block and
// the stage panels are a collapsed-by-default drawer that opens itself when there
// is something to show (live, artifact, or doubts) or the user asks for it.
// Mirrors PlannerChat.test.tsx: static markup for state assertions, a real
// mount only where a user interaction (collapse) must be exercised.

import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement, act } from "react";
import { createRoot } from "react-dom/client";
import { Simulate } from "react-dom/test-utils";
import { PlannerPlanMode } from "./PlannerPlanMode";

// Mark the jsdom env as React-act-aware so act(...) warnings stay quiet.
(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;

const orcs = [
	{ id: "orchestrator", label: "Local" },
	{ id: "claude", label: "Claude" },
	{ id: "codex", label: "Codex" },
	{ id: "openai", label: "OpenAI" },
];

// The expanded stage box is the only place the literal "316" appears (height:316).
// Its absence is a reliable "collapsed" signal; the collapsed slim header carries
// the live badge counts instead (e.g. "Websearch (7)", "Plan (3)").
const baseProps = (overrides: Record<string, unknown> = {}) => ({
	goal: null,
	contextLabel: "",
	plannerModelLabel: "test",
	live: false,
	planCards: [] as any[],
	questions: [] as any[],
	pages: [] as any[],
	findings: [] as any[],
	webMode: "auto" as const,
	onWebModeChange: () => {},
	onManualSearch: () => {},
	design: null,
	linkedTask: null,
	onOpenInDesign: () => {},
	projectRoot: null,
	onGenerated: () => {},
	messages: [] as any[],
	awaitingReply: false,
	onSend: () => {},
	orchestrators: orcs,
	orchestratorId: "orchestrator",
	onOrchestratorChange: () => {},
	coders: [{ id: "local", label: "Local" }],
	mainCoderOverride: null,
	defaultCoderLabel: "Local",
	onCoderChange: () => {},
	autoCreate: false,
	onAutoCreateToggle: () => {},
	...overrides,
});

const render = (props: Record<string, unknown>) =>
	renderToStaticMarkup(createElement(PlannerPlanMode, props as any));

describe("PlannerPlanMode layout (S4)", () => {
	it("renders the chat before the stage container", () => {
		// live => stage is expanded, so its "Websearch" tab label is in the markup.
		const out = render(baseProps({ live: true }));
		const chatIdx = out.indexOf("Message the Orchestrator");
		const stageIdx = out.indexOf("Websearch");
		expect(chatIdx).toBeGreaterThanOrEqual(0);
		expect(stageIdx).toBeGreaterThan(chatIdx);
	});

	it("is collapsed by default when idle and empty", () => {
		const out = render(baseProps());
		expect(out).not.toContain("316"); // expanded stage box height
		expect(out).toContain("Websearch (0)");
		expect(out).toContain("Plan (0)");
		expect(out).toContain("Design (0)");
	});

	it("expands when live", () => {
		const out = render(baseProps({ live: true }));
		expect(out).toContain("316");
	});

	it("expands when questions (doubts) are present even if idle and otherwise empty", () => {
		const out = render(
			baseProps({
				questions: [{ id: "q1", affects: [], text: "doubt" }],
			}),
		);
		// Hard requirement: unanswered doubts must never be hidden by a collapsed drawer.
		// The drawer auto-expands (the 316px box is present in the markup).
		expect(out).toContain("316");
	});

	it("renders live tab badge counts for a non-trivial fixture", () => {
		const props = baseProps({
			planCards: [1, 2, 3].map((n) => ({ n, title: `T${n}`, state: "todo" })),
			pages: [1, 2, 3, 4, 5].map((n) => ({ url: `u${n}`, title: `P${n}`, summary: "s" })),
			findings: [1, 2].map((n) => ({ text: `f${n}` })),
		});
		// With content the drawer auto-expands; collapse it to read the slim-header badges.
		const container = document.createElement("div");
		document.body.appendChild(container);
		const root = createRoot(container);
		act(() => {
			root.render(createElement(PlannerPlanMode, props as any));
		});
		const chevron = container.querySelector(
			'[title="Collapse stage panels"]',
		) as HTMLButtonElement | null;
		expect(chevron).not.toBeNull();
		act(() => {
			Simulate.click(chevron!);
		});
		const html = container.innerHTML;
		// Websearch badge = pages + findings (5 + 2 = 7); Plan badge = planCards (3).
		expect(html).toContain("Websearch (7)");
		expect(html).toContain("Plan (3)");
		act(() => {
			root.unmount();
		});
		container.remove();
	});

	it("force-expands the drawer on a second doubt even after a manual collapse", () => {
		// Render with one doubt: the drawer auto-expands (hard invariant) and a manual
		// collapse sets userToggled = true.
		const container = document.createElement("div");
		document.body.appendChild(container);
		const root = createRoot(container);
		act(() => {
			root.render(
				createElement(
					PlannerPlanMode,
					baseProps({
						questions: [{ id: "q1", affects: [], text: "d" }],
					}) as any,
				),
			);
		});
		// One doubt => drawer is open.
		expect(container.innerHTML).toContain("316");
		// User collapses it by hand (userToggled becomes true).
		const chevron = container.querySelector(
			'[title="Collapse stage panels"]',
		) as HTMLButtonElement | null;
		expect(chevron).not.toBeNull();
		act(() => {
			Simulate.click(chevron!);
		});
		expect(container.innerHTML).not.toContain("316"); // now collapsed by the user
		// A second doubt arrives (1 -> 2) while the user had collapsed the drawer.
		act(() => {
			root.render(
				createElement(
					PlannerPlanMode,
					baseProps({
						questions: [
							{ id: "q1", affects: [], text: "d" },
							{ id: "q2", affects: [], text: "d" },
						],
					}) as any,
				),
			);
		});
		// The incoming doubt MUST surface — the drawer force-expands again.
		expect(container.innerHTML).toContain("316");
		act(() => {
			root.unmount();
		});
		container.remove();
	});

	it("shows the doubt count inside the Plan badge when doubts exist", () => {
		const props = baseProps({
			planCards: [1, 2, 3].map((n) => ({ n, title: `T${n}`, state: "todo" })),
			questions: [{ id: "q1", affects: [], text: "d" }, { id: "q2", affects: [], text: "d" }],
		});
		const container = document.createElement("div");
		document.body.appendChild(container);
		const root = createRoot(container);
		act(() => {
			root.render(createElement(PlannerPlanMode, props as any));
		});
		const chevron = container.querySelector(
			'[title="Collapse stage panels"]',
		) as HTMLButtonElement | null;
		expect(chevron).not.toBeNull();
		act(() => {
			Simulate.click(chevron!);
		});
		expect(container.innerHTML).toContain("Plan (3 · 2 doubts)");
		act(() => {
			root.unmount();
		});
		container.remove();
	});
});
