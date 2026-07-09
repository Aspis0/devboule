// Focused unit tests for the visibleProviders store state (T1b).
//
// Verifies:
//   (a) defaults to [] when localStorage is empty
//   (b) setProviderVisible(true) persists and adds to the array
//   (c) setProviderVisible(false) removes from the array and persists
//   (d) visibleProviders is NOT reset on folder switch (unlike filter)

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { CityState } from "../types/city";

// ---- Backend mock (minimal — we only test store state, not backend calls) ----
vi.mock("../context/AppContext", () => ({
  isTauriRuntime: () => true,
  invokeBackendCommand: (command: string) => {
    // Return a minimal empty city for scanFolder calls.
    if (command === "generate_city_state") {
      return Promise.resolve(mkCity("test"));
    }
    // Stub all other commands.
    return Promise.resolve(undefined);
  },
}));

function mkCity(label: string): CityState {
  return {
    projectName: label,
    era: "Alpha",
    generatedAt: "",
    buildings: [],
    roads: [],
    districts: [],
    agents: [],
    sins: [],
    externalServices: [],
    features: [],
    notes: [],
  } as unknown as CityState;
}

let useCityStore: typeof import("./cityStore").useCityStore;

beforeEach(async () => {
  vi.resetModules();
  // Provide a fake localStorage + window on globalThis (mirrors the agentPoll
  // test pattern). The store reads localStorage at module-init for the restored
  // folder and the visibleProviders preference.
  const store = new Map<string, string>();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).window = globalThis;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  };

  const mod = await import("./cityStore");
  useCityStore = mod.useCityStore;
});

afterEach(() => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).localStorage;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).window;
});

describe("cityStore — visibleProviders", () => {
  it("defaults to [] when localStorage is empty", () => {
    const state = useCityStore.getState();
    expect(state.visibleProviders).toEqual([]);
  });

  it("setProviderVisible(true) adds a provider and persists", () => {
    useCityStore.getState().setProviderVisible("scaleway", true);

    const state = useCityStore.getState();
    expect(state.visibleProviders).toContain("scaleway");

    // Verify persistence.
    const raw = window.localStorage.getItem("polis:visibleProviders");
    expect(raw).toBeTruthy();
    const parsed = JSON.parse(raw!);
    expect(parsed).toContain("scaleway");
  });

  it("setProviderVisible(true) deduplicates", () => {
    useCityStore.getState().setProviderVisible("scaleway", true);
    useCityStore.getState().setProviderVisible("scaleway", true);

    const state = useCityStore.getState();
    const count = state.visibleProviders.filter((p: string) => p === "scaleway").length;
    expect(count).toBe(1);
  });

  it("setProviderVisible(false) removes a provider and persists", () => {
    useCityStore.getState().setProviderVisible("scaleway", true);
    useCityStore.getState().setProviderVisible("cloudflare", true);
    useCityStore.getState().setProviderVisible("scaleway", false);

    const state = useCityStore.getState();
    expect(state.visibleProviders).not.toContain("scaleway");
    expect(state.visibleProviders).toContain("cloudflare");

    // Verify persistence.
    const raw = window.localStorage.getItem("polis:visibleProviders");
    const parsed = JSON.parse(raw!);
    expect(parsed).not.toContain("scaleway");
    expect(parsed).toContain("cloudflare");
  });

  it("visibleProviders is NOT reset on folder switch", async () => {
    // Set some providers visible.
    useCityStore.getState().setProviderVisible("scaleway", true);
    useCityStore.getState().setProviderVisible("cloudflare", true);

    expect(useCityStore.getState().visibleProviders).toEqual(["scaleway", "cloudflare"]);

    // Simulate a folder switch via loadFolder (which resets filter but should
    // NOT reset visibleProviders).
    await useCityStore.getState().loadFolder("/tmp/test-folder");

    // visibleProviders should survive.
    expect(useCityStore.getState().visibleProviders).toEqual(["scaleway", "cloudflare"]);

    // filter should be reset (default values).
    expect(useCityStore.getState().filter).toEqual({
      categories: [],
      minSeverity: null,
      features: [],
      pathGlob: "",
      mode: "ghost",
    });
  });

  it("survives restart (re-reads from localStorage)", async () => {
    // Simulate persistence.
    useCityStore.getState().setProviderVisible("scaleway", true);

    // Re-import to simulate a fresh session (the store re-reads from localStorage
    // at initialization).
    vi.resetModules();
    const mod = await import("./cityStore");
    const freshStore = mod.useCityStore;
    expect(freshStore.getState().visibleProviders).toContain("scaleway");
  });
});
