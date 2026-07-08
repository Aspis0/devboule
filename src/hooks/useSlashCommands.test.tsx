// @vitest-environment jsdom
import { describe, it, expect, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useSlashCommands } from "./useSlashCommands";
import type { SlashApi, SlashResult } from "./useSlashCommands";

(
	globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// Lightweight harness: render the hook and capture its live API each render.
let captured: SlashApi | null = null;
function Harness() {
	captured = useSlashCommands();
	return null;
}

let root: Root | null = null;
let container: HTMLDivElement | null = null;
function mount() {
	container = document.createElement("div");
	document.body.appendChild(container);
	root = createRoot(container);
	act(() => root!.render(createElement(Harness)));
}
afterEach(() => {
	if (root) act(() => root!.unmount());
	root = null;
	if (container) container.remove();
	container = null;
	captured = null;
});

describe("useSlashCommands", () => {
	it("activates on a leading slash and filters by prefix", () => {
		mount();
		act(() => captured!.handleInput("/mo"));
		expect(captured!.isSlashActive).toBe(true);
		expect(captured!.showPopup).toBe(true);
		expect(captured!.filteredCommands.map((c) => c.command)).toEqual(["model"]);
	});

	it("hides the popup when no command matches the prefix", () => {
		mount();
		act(() => captured!.handleInput("/zzz"));
		expect(captured!.isSlashActive).toBe(true);
		expect(captured!.showPopup).toBe(false);
	});

	it("does not activate for text that does not start with a slash", () => {
		mount();
		act(() => captured!.handleInput("model local"));
		expect(captured!.isSlashActive).toBe(false);
		expect(captured!.showPopup).toBe(false);
	});

	it("parses /model local [name] into a switchModel action", () => {
		mount();
		act(() => captured!.handleInput("/model local llama3"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({
			type: "action",
			action: "switchModel",
			payload: { provider: "local", model: "llama3" },
		});
	});

	it("parses /model claude with no model name", () => {
		mount();
		act(() => captured!.handleInput("/model claude"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r.type).toBe("action");
		expect(r.action).toBe("switchModel");
		expect(r.payload?.provider).toBe("claude");
		expect(r.payload?.model).toBeUndefined();
	});

	it("parses /stop into a stopSession action", () => {
		mount();
		act(() => captured!.handleInput("/stop"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({ type: "action", action: "stopSession" });
	});

	it("parses /review into a message", () => {
		mount();
		act(() => captured!.handleInput("/review"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({ type: "message", message: "/review" });
	});

	it("forwards free-form args for /websearch", () => {
		mount();
		act(() => captured!.handleInput("/websearch cats"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({ type: "message", message: "/websearch cats" });
	});

	it("returns none for an unmatched slash (=> treated as normal text)", () => {
		mount();
		act(() => captured!.handleInput("/zzz"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({ type: "none" });
	});

	it("selectIndex runs the chosen command", () => {
		mount();
		act(() => captured!.handleInput("/"));
		const idx = captured!.filteredCommands.findIndex(
			(c) => c.command === "review",
		);
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.selectIndex(idx);
		});
		expect(r).toEqual({ type: "message", message: "/review" });
	});

	it("moveActive clamps at the boundaries", () => {
		mount();
		act(() => captured!.handleInput("/"));
		act(() => captured!.moveActive(1));
		expect(captured!.activeIndex).toBe(1);
		act(() => captured!.moveActive(-5));
		expect(captured!.activeIndex).toBe(0);
	});

	it("escape clears the slash state", () => {
		mount();
		act(() => captured!.handleInput("/model"));
		act(() => captured!.onEscape());
		expect(captured!.isSlashActive).toBe(false);
		expect(captured!.showPopup).toBe(false);
	});

	it("does NOT activate for a slash mid-sentence (audit finding #4)", () => {
		mount();
		act(() => captured!.handleInput("hello /foo"));
		expect(captured!.isSlashActive).toBe(false);
		expect(captured!.showPopup).toBe(false);
	});

	it("activates for a slash after a whitespace-only prefix (audit finding #4)", () => {
		mount();
		act(() => captured!.handleInput("  /model"));
		expect(captured!.isSlashActive).toBe(true);
		expect(captured!.showPopup).toBe(true);
		expect(captured!.filteredCommands.map((c) => c.command)).toContain("model");
	});

	it("clears the slash buffer on Enter for an unmatched slash (audit finding #2)", () => {
		mount();
		act(() => captured!.handleInput("/zzz"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r).toEqual({ type: "none" });
		// The unmatched slash must not leave isSlashActive stuck true.
		expect(captured!.isSlashActive).toBe(false);
		expect(captured!.showPopup).toBe(false);
	});

	it("selectIndex with an out-of-bounds index is a no-op, not the first command (audit finding #3)", () => {
		mount();
		act(() => captured!.handleInput("/"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.selectIndex(999);
		});
		expect(r).toEqual({ type: "none" });
	});

	it("parses /model openai into a switchModel action with provider 'openai' (maps to codex downstream)", () => {
		mount();
		act(() => captured!.handleInput("/model openai"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r.type).toBe("action");
		expect(r.action).toBe("switchModel");
		expect(r.payload?.provider).toBe("openai");
	});

	it("parses /model codex into a switchModel action with provider 'codex'", () => {
		mount();
		act(() => captured!.handleInput("/model codex"));
		let r: SlashResult = { type: "none" };
		act(() => {
			r = captured!.onEnter();
		});
		expect(r.type).toBe("action");
		expect(r.action).toBe("switchModel");
		expect(r.payload?.provider).toBe("codex");
	});
});
