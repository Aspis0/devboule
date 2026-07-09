// Focused unit tests for the Augure sin ledger store actions (P1.4 Part D).
//
// Follows the agentPoll test pattern: mock AppContext, use fake timers,
// verify arg mapping + error propagation + reload side effects.
//
// Invariants verified:
//   (a) loadSinRecords invokes polis_list_sins with projectPath = selectedFolder
//   (b) disposeSin invokes polis_dispose_sin with camelCase args, returns null on success
//   (c) disposeSin returns error string on backend failure
//   (d) fixSin invokes polis_fix_sin with correct args
//   (e) both disposeSin and fixSin refresh records + city on success

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import type { CityState, SinRecord } from "../types/city";

// ---- Controllable backend mock -------------------------------------------
interface Deferred {
  command: string;
  args: unknown;
  resolve: (val: unknown) => void;
  reject: (err: unknown) => void;
}
const pending: Deferred[] = [];
let invokeCalls: { command: string; args: unknown }[] = [];

vi.mock("../context/AppContext", () => ({
  isTauriRuntime: () => true,
  invokeBackendCommand: (command: string, args?: unknown) => {
    invokeCalls.push({ command, args });
    return new Promise<unknown>((resolve, reject) => {
      pending.push({ command, args: args ?? null, resolve, reject });
    });
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

function mkSinRecord(overrides: Partial<SinRecord> & { id: string }): SinRecord {
  return {
    relPath: "src/a.ts",
    ruleId: "R001",
    line: null,
    severity: "smoke",
    description: "desc",
    evidence: "ev",
    disposition: "open",
    createdAt: "",
    updatedAt: "",
    fixDirectiveId: null,
    ...overrides,
  };
}

async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

async function resolveNext<T>(val: T): Promise<void> {
  const d = pending.shift();
  if (!d) throw new Error("no pending invoke");
  d.resolve(val);
  await flush();
}

async function rejectNext(err: string): Promise<void> {
  const d = pending.shift();
  if (!d) throw new Error("no pending invoke");
  d.reject(err);
  await flush();
}

let useCityStore: typeof import("./cityStore").useCityStore;

beforeEach(async () => {
  vi.resetModules();
  pending.length = 0;
  invokeCalls = [];
  vi.useFakeTimers();

  const store = new Map<string, string>();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).window = globalThis;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).document = { visibilityState: "visible" };

  ({ useCityStore } = await import("./cityStore"));
  // Pre-load a folder so the store has a selectedFolder.
  useCityStore.setState({
    cityState: mkCity("p"),
    selectedFolder: "/test/project",
  });
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).document;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  delete (globalThis as any).localStorage;
});

describe("sin ledger store actions", () => {
  it("(a) loadSinRecords calls polis_list_sins with projectPath", async () => {
    useCityStore.getState().loadSinRecords();
    await flush();

    expect(pending.length).toBe(1);
    expect(pending[0].command).toBe("polis_list_sins");
    expect(pending[0].args).toEqual({ projectPath: "/test/project" });

    const records = [mkSinRecord({ id: "s1" })];
    await resolveNext(records);
    expect(useCityStore.getState().sinRecords).toEqual(records);
  });

  it("(a) loadSinRecords clears records on error (enrichment, never blocks)", async () => {
    useCityStore.setState({ sinRecords: [mkSinRecord({ id: "old" })] });
    useCityStore.getState().loadSinRecords();
    await flush();

    await rejectNext("some error");
    expect(useCityStore.getState().sinRecords).toBeNull();
  });

  it("(b) disposeSin invokes polis_dispose_sin with camelCase args", async () => {
    const p = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    expect(pending.length).toBe(1);
    expect(pending[0].command).toBe("polis_dispose_sin");
    expect(pending[0].args).toEqual({
      projectPath: "/test/project",
      relPath: "src/a.ts",
      sinId: "s1",
      disposition: "ignored",
    });

    // Resolve the dispose, then the loadSinRecords + refresh it triggers
    await resolveNext(true);
    await flush(); // loadSinRecords call
    await resolveNext([]); // polis_list_sins
    await flush(); // refresh -> loadFolder -> scanFolder
    await resolveNext(mkCity("p")); // generate_city_state
    await flush();

    const result = await p;
    expect(result).toBeNull();
  });

  it("(c) disposeSin returns error string on backend failure", async () => {
    const p = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    await rejectNext("not a registered project");
    const result = await p;
    expect(result).toBe("not a registered project");
  });

  it("(d) fixSin invokes polis_fix_sin with correct args", async () => {
    const p = useCityStore.getState().fixSin("src/a.ts", "s2");
    await flush();

    expect(pending.length).toBe(1);
    expect(pending[0].command).toBe("polis_fix_sin");
    expect(pending[0].args).toEqual({
      projectPath: "/test/project",
      relPath: "src/a.ts",
      sinId: "s2",
    });

    // Resolve the fix, then the loadSinRecords + refresh it triggers
    await resolveNext("directive-42");
    await flush(); // loadSinRecords call
    await resolveNext([]); // polis_list_sins
    await flush(); // refresh -> loadFolder -> scanFolder
    await resolveNext(mkCity("p")); // generate_city_state
    await flush();

    const result = await p;
    expect(result).toBeNull();
  });

  it("(e) sinActionPending tracks the in-flight sin id", async () => {
    expect(useCityStore.getState().sinActionPending).toEqual([]);

    const p = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    expect(useCityStore.getState().sinActionPending).toContain("s1");

    await resolveNext(true);
    await flush();
    await resolveNext([]);
    await flush();
    await resolveNext(mkCity("p"));
    await flush();
    await p;

    expect(useCityStore.getState().sinActionPending).not.toContain("s1");
  });

  it("(e) sinActionPending clears on error too", async () => {
    const p = useCityStore.getState().fixSin("src/a.ts", "s3");
    await flush();

    expect(useCityStore.getState().sinActionPending).toContain("s3");

    await rejectNext("already in flight");
    await p;

    expect(useCityStore.getState().sinActionPending).not.toContain("s3");
  });

  it("(a) disposeSin success triggers both loadSinRecords and forced refresh", async () => {
    const p = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    // Resolve the mutation — locate it by command (an unrelated store-timer
    // invoke can precede it in the queue during full-suite runs).
    const idx = pending.findIndex((x) => x.command === "polis_dispose_sin");
    expect(idx).toBeGreaterThanOrEqual(0);
    const d = pending.splice(idx, 1)[0];
    d.resolve(true);
    await flush();

    // Both loadSinRecords and refresh (via loadFolder) should have started
    const commands = pending.map((d) => d.command);
    expect(commands).toContain("polis_list_sins");
    expect(commands).toContain("generate_city_state");

    // Clean up — drain by command so an unrelated queued invoke can't shift
    // the resolution order.
    while (pending.length > 0) {
      const cmd = pending[0].command;
      if (cmd === "polis_list_sins") await resolveNext([]);
      else await resolveNext(mkCity("p"));
      await flush();
    }
    await flush();
    const result = await p;
    expect(result).toBeNull();
  });

  it("(b) loadSinRecords bails on folder switch mid-flight", async () => {
    useCityStore.getState().loadSinRecords();
    await flush();

    // Locate the list invoke by command (an unrelated store-timer invoke can
    // share the queue during full-suite runs).
    const idx = pending.findIndex((x) => x.command === "polis_list_sins");
    expect(idx).toBeGreaterThanOrEqual(0);

    // Simulate folder switch while request is in flight
    useCityStore.setState({ selectedFolder: "/different/folder" });

    // Resolve the invoke — records should be dropped
    const d = pending.splice(idx, 1)[0];
    d.resolve([mkSinRecord({ id: "s-new" })]);
    await flush();

    // Records should NOT be overwritten (was null initially)
    expect(useCityStore.getState().sinRecords).toBeNull();
  });

  it("(c) tracks multiple pending sins independently (M1)", async () => {
    expect(useCityStore.getState().sinActionPending).toEqual([]);

    const p1 = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();
    expect(useCityStore.getState().sinActionPending).toContain("s1");

    const p2 = useCityStore.getState().disposeSin("src/b.ts", "s2", "open");
    await flush();
    expect(useCityStore.getState().sinActionPending).toContain("s1");
    expect(useCityStore.getState().sinActionPending).toContain("s2");

    // Resolve s1's dispose mutation only
    await resolveNext(true);
    await flush();

    // s2 should still be pending (s1 post-mutation in flight)
    expect(useCityStore.getState().sinActionPending).toContain("s2");

    // Drain all pending invokes (s1 post-mutation + s2 dispose + s2 post-mutation)
    while (pending.length > 0) {
      const cmd = pending[0].command;
      if (cmd === "polis_dispose_sin") await resolveNext(true);
      else if (cmd === "polis_list_sins") await resolveNext([]);
      else await resolveNext(mkCity("p"));
      await flush();
    }
    await p1;
    await p2;

    expect(useCityStore.getState().sinActionPending).toEqual([]);
  });

  it("returns null immediately if sin is already pending (M1 idempotent)", async () => {
    const p1 = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    // Second call for same sin — returns null without new invoke
    const p2 = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    const result2 = await p2;
    expect(result2).toBeNull();
    // Only the first polis_dispose_sin should be pending. Count dispose invokes
    // specifically — in the full-suite run a store poll timer can enqueue an
    // unrelated invoke, which is noise for this assertion.
    const disposes = pending.filter((d) => d.command === "polis_dispose_sin");
    expect(disposes.length).toBe(1);

    // Clean up — drain by command so an unrelated queued invoke can't shift
    // the resolution order.
    while (pending.length > 0) {
      const cmd = pending[0].command;
      if (cmd === "polis_dispose_sin") await resolveNext(true);
      else if (cmd === "polis_list_sins") await resolveNext([]);
      else await resolveNext(mkCity("p"));
      await flush();
    }
    await p1;
  });

  it("disposeSin skips reload on folder switch after mutation", async () => {
    const p = useCityStore.getState().disposeSin("src/a.ts", "s1", "ignored");
    await flush();

    // Resolve the mutation — locate it by command (an unrelated store-timer
    // invoke can precede it in the queue during full-suite runs).
    const idx = pending.findIndex((x) => x.command === "polis_dispose_sin");
    expect(idx).toBeGreaterThanOrEqual(0);
    const d = pending.splice(idx, 1)[0];
    d.resolve(true);

    // Switch folder BEFORE flush (so when flush runs, folder has changed)
    useCityStore.setState({ selectedFolder: "/other/folder" });
    await flush();

    // No reloads should have started (polis_list_sins, generate_city_state).
    // Assert by command, not queue length — in the full-suite run a store poll
    // timer can enqueue an unrelated invoke.
    expect(invokeCalls.filter((c) => c.command === "polis_list_sins").length).toBe(0);
    expect(invokeCalls.filter((c) => c.command === "generate_city_state").length).toBe(0);

    const result = await p;
    expect(result).toBeNull();
  });
});
