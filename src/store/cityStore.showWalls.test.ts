// Unit tests for the Polis aesthetic `showWalls` display preference on cityStore.
//
// Verifies:
//   (a) default is `true` when localStorage is empty
//   (b) setShowWalls(false) updates state AND persists to localStorage
//   (c) setShowWalls(true) re-persists
//   (d) persisted false survives a fresh module import (reload)
//   (e) invalid stored value falls back to true
//
// Mirrors labsSettings.test.ts localStorage fake pattern. cityStore is
// heavier (mocks AppContext) so we only assert the walls slice.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

vi.mock("../context/AppContext", () => ({
  isTauriRuntime: () => false,
  invokeBackendCommand: vi.fn(),
}));

function makeFakeLocalStorage(): Storage {
  const store = new Map<string, string>();
  return {
    getItem: (k: string) => (store.has(k) ? (store.get(k) as string) : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  } as Storage;
}

describe("cityStore — showWalls", () => {
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
    const { useCityStore } = await import("./cityStore");
    expect(useCityStore.getState().showWalls).toBe(true);
  });

  it("setShowWalls(false) updates state and persists", async () => {
    const { useCityStore } = await import("./cityStore");
    expect(useCityStore.getState().showWalls).toBe(true);

    useCityStore.getState().setShowWalls(false);

    expect(useCityStore.getState().showWalls).toBe(false);
    expect(window.localStorage.getItem("polis:showWalls")).toBe("false");
  });

  it("setShowWalls(true) re-persists", async () => {
    const { useCityStore } = await import("./cityStore");
    useCityStore.getState().setShowWalls(false);
    useCityStore.getState().setShowWalls(true);

    expect(useCityStore.getState().showWalls).toBe(true);
    expect(window.localStorage.getItem("polis:showWalls")).toBe("true");
  });

  it("persisted false survives a fresh module import", async () => {
    const { useCityStore } = await import("./cityStore");
    useCityStore.getState().setShowWalls(false);
    expect(window.localStorage.getItem("polis:showWalls")).toBe("false");

    vi.resetModules();
    const fresh = await import("./cityStore");
    expect(fresh.useCityStore.getState().showWalls).toBe(false);
  });

  it("invalid stored value falls back to true", async () => {
    window.localStorage.setItem("polis:showWalls", "not-a-boolean");
    const { useCityStore } = await import("./cityStore");
    expect(useCityStore.getState().showWalls).toBe(true);
  });
});
