import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readCalendarOpenPref, writeCalendarOpenPref } from "./calendarOpenPref";

// The vitest env is "node", so localStorage is absent by default — stub a
// minimal in-memory one, mirroring projectRootDraft.test.ts.
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

describe("calendarOpenPref", () => {
	it("defaults to false when the key is unset", () => {
		expect(readCalendarOpenPref()).toBe(false);
	});

	it("round-trips true (persists '1')", () => {
		writeCalendarOpenPref(true);
		expect(store.getItem("devboule.projects.calendarOpen")).toBe("1");
		expect(readCalendarOpenPref()).toBe(true);
	});

	it("round-trips false (removes the key)", () => {
		writeCalendarOpenPref(true);
		writeCalendarOpenPref(false);
		expect(store.getItem("devboule.projects.calendarOpen")).toBeNull();
		expect(readCalendarOpenPref()).toBe(false);
	});

	it("tolerates a throwing localStorage on read", () => {
		vi.spyOn(store, "getItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		expect(readCalendarOpenPref()).toBe(false);
	});

	it("tolerates a throwing localStorage on write", () => {
		vi.spyOn(store, "setItem").mockImplementation(() => {
			throw new Error("blocked");
		});
		expect(() => writeCalendarOpenPref(true)).not.toThrow();
	});
});
