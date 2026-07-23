// Unit tests for the Labs `designVisible` preference store (TDD for Task #12).
//
// Verifies:
//   (a) default is `true` when localStorage is empty (Design visible by default)
//   (b) setDesignVisible(false) persists and getDesignVisible() returns false
//   (c) invalid stored value falls back to `true`
//   (d) setDesignVisible(true) re-persists and getDesignVisible() returns true
//   (e) subscribers are notified on change (so Sidebar + LabsView re-render)
//
// NOTE on environment: vitest is configured with `environment: "node"`. The
// store only touches `window.localStorage`, so we inject a minimal fake
// `window`/`localStorage` on globalThis (mirroring the repo's
// in-memory store pattern) instead of relying on a jsdom
// environment package. The behaviour under test is identical.

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

describe("labsSettings — designVisible", () => {
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

  it("defaults to true when localStorage is empty", async () => {
    const { getDesignVisible } = await import("./labsSettings");
    expect(getDesignVisible()).toBe(true);
  });

  it("setDesignVisible(false) persists and getDesignVisible() returns false", async () => {
    const { getDesignVisible, setDesignVisible } = await import("./labsSettings");
    expect(getDesignVisible()).toBe(true);

    setDesignVisible(false);

    expect(getDesignVisible()).toBe(false);
    expect(window.localStorage.getItem("labs:designVisible")).toBe("false");
  });

  it("setDesignVisible(true) persists and getDesignVisible() returns true", async () => {
    const { getDesignVisible, setDesignVisible } = await import("./labsSettings");
    setDesignVisible(false);
    expect(getDesignVisible()).toBe(false);

    setDesignVisible(true);

    expect(getDesignVisible()).toBe(true);
    expect(window.localStorage.getItem("labs:designVisible")).toBe("true");
  });

  it("invalid stored value falls back to true", async () => {
    window.localStorage.setItem("labs:designVisible", "not-a-boolean");
    const { getDesignVisible } = await import("./labsSettings");
    expect(getDesignVisible()).toBe(true);
  });

  it("survives a fresh module import (re-reads persisted value)", async () => {
    const { getDesignVisible, setDesignVisible } = await import("./labsSettings");
    expect(getDesignVisible()).toBe(true);

    setDesignVisible(false);
    vi.resetModules();
    const fresh = await import("./labsSettings");
    expect(fresh.getDesignVisible()).toBe(false);
  });

  it("notifies subscribers when set", async () => {
    const { subscribe, setDesignVisible } = await import("./labsSettings");
    let calls = 0;
    const unsub = subscribe(() => {
      calls += 1;
    });

    setDesignVisible(false);
    expect(calls).toBe(1);

    // No-op when value unchanged → no extra notification.
    setDesignVisible(false);
    expect(calls).toBe(1);

    setDesignVisible(true);
    expect(calls).toBe(2);

    unsub();
  });
});
