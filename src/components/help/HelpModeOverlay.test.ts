import { describe, it, expect } from "vitest";
import { shouldEnterHelpMode } from "./HelpModeOverlay";

// Minimal stand-in for a KeyboardEvent's relevant fields. The helper only reads
// `key` and the modifier flags, so a plain object cast works without a DOM.
function key(
	props: Partial<KeyboardEvent>,
): KeyboardEvent {
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
