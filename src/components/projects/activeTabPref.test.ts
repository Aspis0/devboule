import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readActiveTabPref, writeActiveTabPref } from "./activeTabPref";

// The vitest env is "node", so localStorage is absent by default — stub a
// minimal in-memory one, mirroring calendarOpenPref.test.ts.
class MemoryStorage {
	private map = new Map<string, string>();
	getItem(k: string): string | null {
		return this.map.has(k) ? (this.map.get(k) as string) : null;
	}
	setItem(k: string, v: string): void {
		this.map.set(k, v);
	}
	removeItem(k: string): void {
		this.map.delete(k);
	}
	clear(): void {
		this.map.clear();
	}
	key(i: number): string | null {
		return Array.from(this.map.keys())[i] ?? null;
	}
	get length(): number {
		return this.map.size;
	}
}

let store: MemoryStorage;

beforeEach(() => {
	store = new MemoryStorage();
	(globalThis as unknown as { localStorage: MemoryStorage }).localStorage = store;
});

afterEach(() => {
	vi.restoreAllMocks();
	delete (globalThis as unknown as { localStorage?: MemoryStorage }).localStorage;
});

describe("activeTabPref", () => {
	it("defaults to 'tasks' when nothing is stored", () => {
		expect(readActiveTabPref("p1")).toBe("tasks");
	});

	it("returns 'tasks' for an unknown stored value", () => {
		store.setItem("devboule.work.activeTab.p1", "bogus");
		expect(readActiveTabPref("p1")).toBe("tasks");
	});

	it("returns the stored tab when valid", () => {
		store.setItem("devboule.work.activeTab.p1", "git");
		expect(readActiveTabPref("p1")).toBe("git");
	});

	it("persists and reads back each valid tab", () => {
		const tabs = ["tasks", "censor", "git", "changes", "plans", "notes", "mcp", "project"] as const;
		for (const tab of tabs) {
			writeActiveTabPref("proj-x", tab);
			expect(readActiveTabPref("proj-x")).toBe(tab);
		}
	});

	it("isolates per project", () => {
		writeActiveTabPref("p1", "censor");
		writeActiveTabPref("p2", "notes");
		expect(readActiveTabPref("p1")).toBe("censor");
		expect(readActiveTabPref("p2")).toBe("notes");
	});

	it("tolerates a throwing localStorage on read", () => {
		vi.spyOn(store, "getItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		expect(readActiveTabPref("p1")).toBe("tasks");
	});

	it("tolerates a throwing localStorage on write", () => {
		vi.spyOn(store, "setItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		expect(() => writeActiveTabPref("p1", "git")).not.toThrow();
	});
});
