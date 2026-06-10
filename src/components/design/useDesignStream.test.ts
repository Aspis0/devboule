// Tests for the design-LLM generation transport controller (Phase 2 STEP 2).
//
// We drive `startDesignGeneration` with INJECTED deps (a fake listen/invoke) so no real
// Tauri runtime is needed. The load-bearing properties under test:
//   - SUBSCRIBE BEFORE INVOKE (no early delta can be missed).
//   - delta accumulation (onText sees the FULL text so far).
//   - exactly-once terminal status + ALWAYS unlisten on terminal.
//   - cancel() invokes design_cancel_generation and the cancelled event settles cleanly.
//   - late events after terminal/dispose are ignored.

import { describe, it, expect, vi } from "vitest";
import {
  startDesignGeneration,
  designStreamChannel,
  type DesignStreamDeps,
  type DesignStreamEvent,
} from "./useDesignStream";

/** A controllable fake of the Tauri surface the controller depends on. */
function makeDeps() {
  const order: string[] = [];
  let emit: ((e: DesignStreamEvent) => void) | null = null;
  let subscribedChannel: string | null = null;
  const unlisten = vi.fn();

  const invoke = vi.fn(
    async (command: string, _args?: Record<string, unknown>) => {
      order.push(`invoke:${command}`);
      return undefined;
    },
  );

  const listen: DesignStreamDeps["listen"] = async (channel, handler) => {
    order.push(`listen:${channel}`);
    subscribedChannel = channel;
    emit = (e) => handler({ payload: e });
    return unlisten;
  };

  const deps: DesignStreamDeps = {
    listen,
    invoke,
    newId: () => "fixed-id",
  };

  return {
    deps,
    order,
    invoke,
    unlisten,
    emit: (e: DesignStreamEvent) => {
      if (!emit) throw new Error("not subscribed yet");
      emit(e);
    },
    get channel() {
      return subscribedChannel;
    },
  };
}

describe("startDesignGeneration", () => {
  it("subscribes to the channel BEFORE invoking design_generate", async () => {
    const f = makeDeps();
    await startDesignGeneration("hi", {}, f.deps);

    expect(f.channel).toBe(designStreamChannel("fixed-id"));
    // listen must come strictly before the design_generate invoke.
    const listenIdx = f.order.indexOf(`listen:${designStreamChannel("fixed-id")}`);
    const invokeIdx = f.order.indexOf("invoke:design_generate");
    expect(listenIdx).toBeGreaterThanOrEqual(0);
    expect(invokeIdx).toBeGreaterThan(listenIdx);
  });

  it("passes genId + prompt to design_generate", async () => {
    const f = makeDeps();
    await startDesignGeneration("make a hero", {}, f.deps);
    expect(f.invoke).toHaveBeenCalledWith("design_generate", {
      genId: "fixed-id",
      prompt: "make a hero",
    });
  });

  it("forwards a non-empty workingFolderPath (camelCase) to design_generate", async () => {
    const f = makeDeps();
    await startDesignGeneration("hi", {}, f.deps, "  C:/target/.aspis-design/landing  ");
    expect(f.invoke).toHaveBeenCalledWith("design_generate", {
      genId: "fixed-id",
      prompt: "hi",
      // Trimmed; the camelCase key mirrors the Rust `working_folder_path` arg.
      workingFolderPath: "C:/target/.aspis-design/landing",
    });
  });

  it("omits workingFolderPath when absent or blank", async () => {
    const f = makeDeps();
    await startDesignGeneration("hi", {}, f.deps, "   ");
    expect(f.invoke).toHaveBeenCalledWith("design_generate", {
      genId: "fixed-id",
      prompt: "hi",
    });
    const args = f.invoke.mock.calls.find(
      (c) => c[0] === "design_generate",
    )?.[1] as Record<string, unknown>;
    expect("workingFolderPath" in args).toBe(false);
  });

  it("accumulates delta text and reports the full string each time", async () => {
    const f = makeDeps();
    const texts: string[] = [];
    await startDesignGeneration("p", { onText: (t) => texts.push(t) }, f.deps);

    f.emit({ type: "delta", text: "Hello" });
    f.emit({ type: "delta", text: ", " });
    f.emit({ type: "delta", text: "world" });

    expect(texts).toEqual(["Hello", "Hello, ", "Hello, world"]);
  });

  it("fires a streaming status then a single done, and unlistens on done", async () => {
    const f = makeDeps();
    const statuses: string[] = [];
    await startDesignGeneration(
      "p",
      { onStatus: (s) => statuses.push(s) },
      f.deps,
    );

    f.emit({ type: "delta", text: "x" });
    f.emit({ type: "done" });

    expect(statuses).toEqual(["streaming", "done"]);
    expect(f.unlisten).toHaveBeenCalledTimes(1);
  });

  it("surfaces an error status with the message and unlistens", async () => {
    const f = makeDeps();
    const captured: { status: string; message?: string }[] = [];
    await startDesignGeneration(
      "p",
      { onStatus: (status, message) => captured.push({ status, message }) },
      f.deps,
    );
    f.emit({ type: "error", message: "boom" });

    expect(captured[captured.length - 1]).toEqual({
      status: "error",
      message: "boom",
    });
    expect(f.unlisten).toHaveBeenCalledTimes(1);
  });

  it("cancel() invokes design_cancel_generation; the cancelled event settles + unlistens", async () => {
    const f = makeDeps();
    const statuses: string[] = [];
    const handle = await startDesignGeneration(
      "p",
      { onStatus: (s) => statuses.push(s) },
      f.deps,
    );

    handle.cancel();
    expect(f.invoke).toHaveBeenCalledWith("design_cancel_generation", {
      genId: "fixed-id",
    });

    // The backend then emits the terminal cancelled event over the channel.
    f.emit({ type: "cancelled" });
    expect(statuses).toEqual(["streaming", "cancelled"]);
    expect(f.unlisten).toHaveBeenCalledTimes(1);
  });

  it("ignores late events after a terminal event (no double-settle, no extra text)", async () => {
    const f = makeDeps();
    const statuses: string[] = [];
    const texts: string[] = [];
    await startDesignGeneration(
      "p",
      {
        onStatus: (s) => statuses.push(s),
        onText: (t) => texts.push(t),
      },
      f.deps,
    );

    f.emit({ type: "done" });
    // These must all be ignored.
    f.emit({ type: "delta", text: "late" });
    f.emit({ type: "done" });
    f.emit({ type: "error", message: "late err" });

    expect(statuses).toEqual(["streaming", "done"]);
    expect(texts).toEqual([]);
    expect(f.unlisten).toHaveBeenCalledTimes(1);
  });

  it("dispose() unlistens and suppresses any later event without firing a status", async () => {
    const f = makeDeps();
    const statuses: string[] = [];
    const handle = await startDesignGeneration(
      "p",
      { onStatus: (s) => statuses.push(s) },
      f.deps,
    );

    handle.dispose();
    expect(f.unlisten).toHaveBeenCalledTimes(1);

    // A late delta/terminal after dispose is ignored and never fires a status.
    f.emit({ type: "delta", text: "x" });
    f.emit({ type: "done" });
    expect(statuses).toEqual(["streaming"]); // only the initial streaming, no terminal
  });

  it("fires the 'streaming' status exactly once per run (no double-emit)", async () => {
    const f = makeDeps();
    const statuses: string[] = [];
    await startDesignGeneration("p", { onStatus: (s) => statuses.push(s) }, f.deps);
    expect(statuses).toEqual(["streaming"]);
  });

  it("reports an error (not a throw) when subscription fails", async () => {
    const failingDeps: DesignStreamDeps = {
      listen: async () => {
        throw new Error("no runtime");
      },
      invoke: vi.fn(async () => undefined),
      newId: () => "fixed-id",
    };
    const captured: { status: string; message?: string }[] = [];
    const handle = await startDesignGeneration(
      "p",
      { onStatus: (status, message) => captured.push({ status, message }) },
      failingDeps,
    );

    expect(captured[captured.length - 1]?.status).toBe("error");
    // design_generate must NOT have been invoked if we never subscribed.
    expect(failingDeps.invoke).not.toHaveBeenCalled();
    // The handle's cancel is a safe no-op here.
    expect(() => handle.cancel()).not.toThrow();
  });

  it("surfaces an invoke failure as an error status", async () => {
    const deps: DesignStreamDeps = {
      listen: async (_c, _h) => vi.fn(),
      invoke: vi.fn(async (command: string) => {
        if (command === "design_generate") throw new Error("backend down");
        return undefined;
      }),
      newId: () => "fixed-id",
    };
    const statuses: { status: string; message?: string }[] = [];
    await startDesignGeneration(
      "p",
      { onStatus: (status, message) => statuses.push({ status, message }) },
      deps,
    );
    // Allow the rejected invoke promise's .catch to run.
    await Promise.resolve();
    await Promise.resolve();

    expect(statuses.some((s) => s.status === "error")).toBe(true);
  });
});
