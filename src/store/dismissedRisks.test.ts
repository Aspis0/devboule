// Unit tests for the dismissed-risks store (TDD for Task #11).
//
// Verifies:
//   (a) empty set by default when localStorage is missing
//   (b) dismissRisk("a") adds + persists "a" and the hook/get contains it
//   (c) clearRisks(["a","b"]) adds both at once and persists
//   (d) malformed storage (non-JSON / non-array) ⇒ empty set
//   (e) subscribers are notified on change
//
// NOTE on environment: vitest is configured with `environment: "node"`. The
// store only touches `window.localStorage`, so we inject a minimal fake
// `window`/`localStorage` on globalThis (mirroring labsSettings.test.ts and
// cityStore.visibleProviders.test.ts) instead of relying on a jsdom package.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

function makeFakeLocalStorage(): Storage {
  const store = new Map<string, string>();
  return {
    getItem: (k: string) => (store.has(k) ? (store.get(k) as string) : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => (Array.from(store.keys())[i] ?? null),
    get length() {
      return store.size;
    },
  } as Storage;
}

describe("dismissedRisks store", () => {
  beforeEach(() => {
    vi.resetModules();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).window = globalThis;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any).localStorage = makeFakeLocalStorage();
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).localStorage;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (globalThis as any).window;
  });

  it("starts empty by default", async () => {
    const { getDismissedRisks } = await import("./dismissedRisks");
    expect(getDismissedRisks().size).toBe(0);
  });

  it("dismissRisk(id) adds + persists the id", async () => {
    const { getDismissedRisks, dismissRisk } = await import("./dismissedRisks");
    dismissRisk("a");

    expect(getDismissedRisks().has("a")).toBe(true);
    expect(getDismissedRisks().size).toBe(1);
    expect(window.localStorage.getItem("notifications:dismissedRisks")).toBe(
      JSON.stringify(["a"]),
    );
  });

  it("dismissRisk is a no-op when the id is already dismissed", async () => {
    const { dismissRisk, getDismissedRisks } = await import("./dismissedRisks");
    dismissRisk("a");
    dismissRisk("a");

    expect(getDismissedRisks().size).toBe(1);
    // localStorage written once (length 1), not re-written redundantly.
    expect(window.localStorage.getItem("notifications:dismissedRisks")).toBe(
      JSON.stringify(["a"]),
    );
  });

  it("clearRisks(ids) adds many at once", async () => {
    const { getDismissedRisks, clearRisks } = await import("./dismissedRisks");
    clearRisks(["a", "b"]);

    expect(getDismissedRisks().has("a")).toBe(true);
    expect(getDismissedRisks().has("b")).toBe(true);
    expect(getDismissedRisks().size).toBe(2);
    expect(window.localStorage.getItem("notifications:dismissedRisks")).toBe(
      JSON.stringify(["a", "b"]),
    );
  });

  it("dismissed ids survive a fresh module import (re-reads persisted value)", async () => {
    const { dismissRisk } = await import("./dismissedRisks");
    dismissRisk("a");

    vi.resetModules();
    const fresh = await import("./dismissedRisks");
    expect(fresh.getDismissedRisks().has("a")).toBe(true);
  });

  it("malformed JSON storage falls back to empty set", async () => {
    window.localStorage.setItem("notifications:dismissedRisks", "not-json");
    const { getDismissedRisks } = await import("./dismissedRisks");
    expect(getDismissedRisks().size).toBe(0);
  });

  it("non-array JSON storage falls back to empty set", async () => {
    window.localStorage.setItem(
      "notifications:dismissedRisks",
      JSON.stringify({ not: "an array" }),
    );
    const { getDismissedRisks } = await import("./dismissedRisks");
    expect(getDismissedRisks().size).toBe(0);
  });

  it("notifies subscribers when ids are added", async () => {
    const { subscribe, dismissRisk, clearRisks } = await import(
      "./dismissedRisks"
    );
    let calls = 0;
    const unsub = subscribe(() => {
      calls += 1;
    });

    dismissRisk("a");
    expect(calls).toBe(1);

    // No-op when id unchanged → no extra notification.
    dismissRisk("a");
    expect(calls).toBe(1);

    clearRisks(["b", "c"]);
    expect(calls).toBe(2);

    // No-op when all ids already present → no extra notification.
    clearRisks(["a", "b", "c"]);
    expect(calls).toBe(2);

    unsub();
  });

  it("caps the persisted set to the most-recent ids and drops the oldest", async () => {
    const { dismissRisk, getDismissedRisks } = await import(
      "./dismissedRisks"
    );
    // Dismiss 305 unique ids; only the newest 300 should survive.
    for (let i = 0; i < 305; i += 1) {
      dismissRisk(`id-${i}`);
    }

    const stored = getDismissedRisks();
    expect(stored.size).toBeLessThanOrEqual(300);
    expect(stored.size).toBe(300);
    // Newest retained, oldest dropped.
    expect(stored.has("id-304")).toBe(true);
    expect(stored.has("id-303")).toBe(true);
    expect(stored.has("id-0")).toBe(false);
    expect(stored.has("id-4")).toBe(false);
    // What's persisted reflects the capped set (newest 300: id-5..id-304).
    const raw = window.localStorage.getItem("notifications:dismissedRisks");
    const parsed = JSON.parse(raw ?? "[]") as string[];
    expect(parsed).toHaveLength(300);
    expect(parsed).toContain("id-304");
    expect(parsed).not.toContain("id-0");
  });
});
