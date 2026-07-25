// Regressions for silent folder-map failures (step 6).
//
// 1a — a plain-string backend rejection must reach the store `error` field with
//     its text preserved (the old `e instanceof Error ? … : generic` hid it).
// 1b — a failed load while a city is already present must set `error` without
//     clearing `cityState` (UI surfaces a banner, not a wipe).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { CityState } from "../types/city";
import {
  folderMapErrorMessage,
  folderLoadErrorSurface,
} from "./cityStore";

// ---- Controllable backend mock -------------------------------------------
interface Deferred {
  command: string;
  resolve: (v: unknown) => void;
  reject: (err: unknown) => void;
}
const pending: Deferred[] = [];

vi.mock("../context/AppContext", () => ({
  isTauriRuntime: () => true,
  invokeBackendCommand: (command: string) => {
    if (command === "polis_debug_log") {
      return Promise.resolve(undefined);
    }
    return new Promise((resolve, reject) => {
      pending.push({ command, resolve, reject });
    });
  },
}));

function mkCity(label: string): CityState {
  return {
    projectName: label,
    era: "Alpha",
    generatedAt: "",
    buildings: [
      {
        fileId: "fid-1",
        filePath: "src/a.ts",
        districtId: "core",
        purpose: "house",
        purposeSource: "default",
        featureId: "commons",
        featureSource: "commons",
        provider: null,
        linesOfCode: 10,
        visualTier: "kalybe",
        coords: { x: 0, y: 0 },
        status: "normal",
        label: "a.ts",
        description: "",
        lastModified: "",
        agentPresent: null,
        kanbanCardId: null,
        untrackedChange: null,
        sins: [],
        notes: [],
      },
    ],
    roads: [],
    districts: [],
    agents: [],
    sins: [],
    externalServices: [],
    features: [],
    notes: [],
  } as unknown as CityState;
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function takePending(command: string): Deferred {
  const i = pending.findIndex((d) => d.command === command);
  if (i < 0) throw new Error(`no pending invoke for ${command}`);
  return pending.splice(i, 1)[0]!;
}

describe("folderMapErrorMessage (pure)", () => {
  it("preserves a plain string rejection from the backend", () => {
    const reason =
      "Project path is not a registered project root; refusing access.";
    expect(folderMapErrorMessage(reason)).toBe(reason);
  });

  it("uses Error.message when the rejection is an Error", () => {
    expect(folderMapErrorMessage(new Error("App is locked. Unlock to continue."))).toBe(
      "App is locked. Unlock to continue.",
    );
  });

  it("does not collapse a string into the old generic fallback", () => {
    const reason = "Cannot resolve project path: /tmp/x";
    const msg = folderMapErrorMessage(reason);
    expect(msg).toBe(reason);
    expect(msg).not.toBe("Failed to map the selected folder.");
  });
});

describe("folderLoadErrorSurface (pure)", () => {
  it("is blocking when there is no city yet", () => {
    expect(folderLoadErrorSurface("boom", false)).toBe("blocking");
  });

  it("is a non-destructive banner when a city is already present", () => {
    expect(folderLoadErrorSurface("boom", true)).toBe("banner");
  });

  it("is null when there is no error", () => {
    expect(folderLoadErrorSurface(null, true)).toBeNull();
    expect(folderLoadErrorSurface(undefined, false)).toBeNull();
  });
});

describe("loadFolder preserves backend reason + existing city", () => {
  let useCityStore: typeof import("./cityStore").useCityStore;

  beforeEach(async () => {
    vi.resetModules();
    pending.length = 0;
    // Fresh module so store state starts clean after mock is in place.
    ({ useCityStore } = await import("./cityStore"));
    useCityStore.setState({
      cityState: null,
      liveCity: null,
      loading: false,
      error: null,
      selectedFolder: null,
      usingFixture: false,
    });
  });

  afterEach(() => {
    pending.length = 0;
  });

  it("surfaces a string rejection as the store error (not a generic sentence)", async () => {
    const reason =
      "Project path is not a registered project root; refusing access.";
    const p = useCityStore.getState().loadFolder("/tmp/not-a-project");
    // drain microtasks so the invoke is enqueued
    await flush();
    takePending("generate_city_state").reject(reason);
    await p;
    await flush();

    const s = useCityStore.getState();
    expect(s.loading).toBe(false);
    expect(s.error).toBe(reason);
    expect(s.error).not.toBe("Failed to map the selected folder.");
  });

  it("keeps the previous city when a re-map fails and still sets error", async () => {
    const existing = mkCity("kept-city");
    useCityStore.setState({
      cityState: existing,
      selectedFolder: "/tmp/good",
      error: null,
      loading: false,
    });

    const reason = "Project path is not a directory: /tmp/bad";
    const p = useCityStore.getState().loadFolder("/tmp/bad");
    await flush();
    takePending("generate_city_state").reject(reason);
    await p;
    await flush();

    const s = useCityStore.getState();
    expect(s.cityState).toBe(existing);
    expect(s.cityState?.projectName).toBe("kept-city");
    expect(s.error).toBe(reason);
    expect(folderLoadErrorSurface(s.error, !!s.cityState)).toBe("banner");
  });
});
