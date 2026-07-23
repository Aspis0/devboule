// @vitest-environment jsdom
//
// Pure helpers for the first-run welcome banner (localStorage key + dismiss).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
	WELCOME_DISMISSED_KEY,
	isWelcomeDismissed,
	dismissWelcome,
	openHelpQuickStart,
} from "./welcomeBannerState";

function makeFakeLocalStorage() {
	const map = new Map<string, string>();
	return {
		getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
		setItem: (k: string, v: string) => {
			map.set(k, String(v));
		},
		removeItem: (k: string) => {
			map.delete(k);
		},
		clear: () => map.clear(),
	};
}

describe("welcomeBanner helpers", () => {
	beforeEach(() => {
		(globalThis as unknown as { localStorage: Storage }).localStorage =
			makeFakeLocalStorage() as unknown as Storage;
	});

	afterEach(() => {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		delete (globalThis as any).localStorage;
	});

	it("isWelcomeDismissed is false when key is absent", () => {
		expect(isWelcomeDismissed()).toBe(false);
	});

	it("dismissWelcome persists and isWelcomeDismissed returns true", () => {
		dismissWelcome();
		expect(localStorage.getItem(WELCOME_DISMISSED_KEY)).toBe("1");
		expect(isWelcomeDismissed()).toBe(true);
	});

	it("treats the string 'true' as dismissed (defensive)", () => {
		localStorage.setItem(WELCOME_DISMISSED_KEY, "true");
		expect(isWelcomeDismissed()).toBe(true);
	});

	it("openHelpQuickStart calls requestView('help') and sets the hash", () => {
		const requestView = vi.fn();
		openHelpQuickStart(requestView);
		expect(requestView).toHaveBeenCalledWith("help");
		expect(window.location.hash).toBe("#quick-start");
	});
});
