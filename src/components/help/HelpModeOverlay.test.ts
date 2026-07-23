// @vitest-environment jsdom
import { afterEach, describe, it, expect } from "vitest";
import {
	findHelpTarget,
	pageUseLineFor,
	readHelp,
	shouldEnterHelpMode,
} from "./HelpModeOverlay";

// Minimal stand-in for a KeyboardEvent's relevant fields. The helper only reads
// `key` and the modifier flags, so a plain object cast works without a DOM.
function key(props: Partial<KeyboardEvent>): KeyboardEvent {
	return {
		key: "",
		shiftKey: false,
		ctrlKey: false,
		metaKey: false,
		altKey: false,
		...props,
	} as unknown as KeyboardEvent;
}

describe("shouldEnterHelpMode", () => {
	it("engages on a bare Alt keydown", () => {
		const e = key({ key: "Alt", altKey: true });
		expect(shouldEnterHelpMode(e)).toBe(true);
	});

	it("does NOT engage on Alt combined with a character (Option+8 on macOS)", () => {
		// macOS Option+8: key is the composed char, altKey is true.
		const e = key({ key: "8", altKey: true });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});

	it("does NOT engage on Alt+letter (Option+e accent composition)", () => {
		const e = key({ key: "e", altKey: true });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});

	it("does NOT engage on Alt+Shift (dead-key + shift)", () => {
		const e = key({ key: "Alt", altKey: true, shiftKey: true });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});

	it("does NOT engage on Alt+Ctrl", () => {
		const e = key({ key: "Alt", altKey: true, ctrlKey: true });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});

	it("does NOT engage on Alt+Meta (Option+Cmd on macOS)", () => {
		const e = key({ key: "Alt", altKey: true, metaKey: true });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});

	it("does NOT engage on a non-Alt key without Alt", () => {
		const e = key({ key: "h" });
		expect(shouldEnterHelpMode(e)).toBe(false);
	});
});

describe("findHelpTarget", () => {
	afterEach(() => {
		document.body.innerHTML = "";
	});

	it("returns null when the element has no data-help-* ancestor", () => {
		document.body.innerHTML = `<div><button id="btn">Go</button></div>`;
		const btn = document.getElementById("btn");
		expect(findHelpTarget(btn)).toBeNull();
	});

	it("returns null for null / non-Element input", () => {
		expect(findHelpTarget(null)).toBeNull();
	});

	it("returns the nearest data-help-title ancestor", () => {
		document.body.innerHTML = `
			<div data-help-title="Outer help" data-help-lines="Line one">
				<span id="inner"><button id="btn">Go</button></span>
			</div>
		`;
		const target = findHelpTarget(document.getElementById("btn"));
		expect(target).not.toBeNull();
		expect(target?.getAttribute("data-help-title")).toBe("Outer help");
		expect(readHelp(target!, "projects").title).toBe("Outer help");
		expect(readHelp(target!, "projects").lines).toContain("Line one");
	});

	it("returns an element that itself carries data-help-lines", () => {
		document.body.innerHTML = `
			<button id="btn" data-help-lines="Does the thing">Run</button>
		`;
		const target = findHelpTarget(document.getElementById("btn"));
		expect(target?.id).toBe("btn");
		expect(readHelp(target!, "agents").lines).toContain("Does the thing");
	});

	it("returns null when the help target is under data-help-skip", () => {
		document.body.innerHTML = `
			<div data-help-skip="true">
				<button id="btn" data-help-title="Skipped">Go</button>
			</div>
		`;
		expect(findHelpTarget(document.getElementById("btn"))).toBeNull();
	});
});

describe("pageUseLineFor", () => {
	// Coverage for the page-context line used in the banner and single tooltip.
	// Keys should match Sidebar nav ids and surviving settings sub-views.
	const expectedViews = [
		"secrets",
		"oracle",
		"projects",
		"agents",
		"graph",
		"design",
		"polis",
		"skills",
		"labs",
		"help",
		"settings",
		"devices",
		"workspace",
	] as const;

	it("returns a Devboule-specific line for every known nav/view key", () => {
		for (const view of expectedViews) {
			const line = pageUseLineFor(view);
			expect(line.length).toBeGreaterThan(20);
			expect(line.toLowerCase()).toContain("devboule");
			// Known views must not fall through to the generic default.
			expect(line).not.toMatch(/use this only when it makes the local project/);
		}
	});

	it("falls back to a generic line for unknown views", () => {
		const line = pageUseLineFor("not-a-real-view");
		expect(line).toMatch(/use this only when it makes the local project/);
	});
});
