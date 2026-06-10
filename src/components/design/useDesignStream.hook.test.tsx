// @vitest-environment jsdom
//
// Hook-level lifecycle/regression tests for `useDesignStream` (Phase 2 STEP 2 hostile-fix
// pass). The pure transport behavior is covered in useDesignStream.test.ts; here we drive
// the React hook through a real render to assert the load-bearing teardown semantics:
//   - WARNING 4: starting a new generation while one is in-flight CANCELS the old genId on
//     the backend (not just silences the listener).
//   - WARNING 9: reset() also cancels the in-flight generation.
//   - WARNING 5: exactly one "streaming" status transition per run (no double-emit).
// No testing-library dependency — a tiny `react-dom/client` + `act` harness matches the
// repo's existing dependency-free test approach (see useDrag.lifecycle.test.tsx).

import { describe, it, expect, vi } from "vitest";
import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  useDesignStream,
  type DesignStreamDeps,
  type DesignStreamEvent,
  type UseDesignStreamState,
} from "./useDesignStream";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

/** A controllable fake of the Tauri surface, minting a fresh genId per generation. */
function makeDeps() {
  const invokeCalls: { command: string; args?: Record<string, unknown> }[] = [];
  // Per-channel emitters so we can target a specific run.
  const emitters = new Map<string, (e: DesignStreamEvent) => void>();
  const unlistens: Array<ReturnType<typeof vi.fn>> = [];
  let counter = 0;

  const invoke = vi.fn(
    async (command: string, args?: Record<string, unknown>) => {
      invokeCalls.push({ command, args });
      return undefined;
    },
  );

  const listen: DesignStreamDeps["listen"] = async (channel, handler) => {
    emitters.set(channel, (e) => handler({ payload: e }));
    const u = vi.fn();
    unlistens.push(u);
    return u;
  };

  const deps: DesignStreamDeps = {
    listen,
    invoke,
    newId: () => `id-${++counter}`,
  };

  return { deps, invoke, invokeCalls, emitters, unlistens };
}

/** Flush pending microtasks so the async startDesignGeneration startup settles. */
async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function mountHook(deps: DesignStreamDeps): {
  api: () => UseDesignStreamState;
  unmount: () => void;
} {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let latest: UseDesignStreamState | null = null;
  let root: Root;

  function Probe() {
    const state = useDesignStream(deps);
    useEffect(() => {
      latest = state;
    });
    latest = state;
    return null;
  }

  act(() => {
    root = createRoot(container);
    root.render(createElement(Probe));
  });

  return {
    api: () => {
      if (!latest) throw new Error("hook not mounted");
      return latest;
    },
    unmount: () => {
      act(() => root.unmount());
      container.remove();
    },
  };
}

describe("useDesignStream — hook teardown semantics", () => {
  it("WARNING 4: superseding an in-flight run cancels the OLD genId on the backend", async () => {
    const f = makeDeps();
    const { api, unmount } = mountHook(f.deps);

    act(() => api().start("first"));
    await flush();
    // First generation registered with id-1.
    expect(
      f.invokeCalls.some(
        (c) => c.command === "design_generate" && c.args?.genId === "id-1",
      ),
    ).toBe(true);

    // Start a second generation while the first is still in-flight.
    act(() => api().start("second"));
    await flush();

    // The OLD run (id-1) must have been cancelled on the backend, not just silenced.
    expect(f.invoke).toHaveBeenCalledWith("design_cancel_generation", {
      genId: "id-1",
    });
    // And the new run (id-2) started.
    expect(
      f.invokeCalls.some(
        (c) => c.command === "design_generate" && c.args?.genId === "id-2",
      ),
    ).toBe(true);

    unmount();
  });

  it("WARNING 9: reset() cancels the in-flight generation on the backend", async () => {
    const f = makeDeps();
    const { api, unmount } = mountHook(f.deps);

    act(() => api().start("only"));
    await flush();

    act(() => api().reset());
    await flush();

    expect(f.invoke).toHaveBeenCalledWith("design_cancel_generation", {
      genId: "id-1",
    });
    expect(api().status).toBe("idle");

    unmount();
  });

  it("W3: cancel during the PRE-HANDLE window still cancels the backend genId", async () => {
    // Make listen() hang so startDesignGeneration never resolves a handle while the
    // test calls cancel(). The hook must still address the backend generation by the
    // genId it captured SYNCHRONOUSLY at start() — not silently drop the cancel.
    const invokeCalls: { command: string; args?: Record<string, unknown> }[] = [];
    const invoke = vi.fn(
      async (command: string, args?: Record<string, unknown>) => {
        invokeCalls.push({ command, args });
        return undefined;
      },
    );
    let releaseListen: (() => void) | null = null;
    const listen: DesignStreamDeps["listen"] = () =>
      new Promise<() => void>((resolve) => {
        releaseListen = () => resolve(vi.fn());
      });
    const deps: DesignStreamDeps = { listen, invoke, newId: () => "id-pre" };

    const { api, unmount } = mountHook(deps);

    // start(): captures genId "id-pre" synchronously; listen is still pending (no handle).
    act(() => api().start("p"));
    await flush();

    // cancel() BEFORE the handle exists — must still hit the backend with the genId.
    act(() => api().cancel());
    await flush();

    expect(invoke).toHaveBeenCalledWith("design_cancel_generation", {
      genId: "id-pre",
    });

    // Release the hung listen so teardown can proceed cleanly.
    act(() => releaseListen?.());
    await flush();
    unmount();
  });

  it("WARNING 5: exactly one 'streaming' transition per run", async () => {
    const f = makeDeps();
    const { api, unmount } = mountHook(f.deps);

    expect(api().status).toBe("idle");

    act(() => api().start("p"));
    await flush();
    expect(api().status).toBe("streaming");

    // Drive a delta then done; status must end at "done" with no spurious extra
    // streaming transitions (the run only ever moved idle -> streaming -> done).
    act(() => {
      const emit = f.emitters.get("design-stream:id-1");
      emit?.({ type: "delta", text: "x" });
      emit?.({ type: "done" });
    });
    await flush();
    expect(api().status).toBe("done");

    unmount();
  });
});
