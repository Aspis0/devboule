// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const mockInvoke = vi.fn(async (..._args: unknown[]): Promise<unknown> => ({
  status: "queued",
  queued: 1,
}));
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => mockInvoke(...args),
  isTauriRuntime: () => false,
}));

import { MiniSteerBar, steerStatusLabel } from "./MiniSteerBar";

// ---- DOM helpers ------------------------------------------------------------

let container: HTMLDivElement;
let root: Root;

function mount(agentId: string | null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(createElement(MiniSteerBar, { agentId }));
  });
}

function unmount() {
  act(() => root.unmount());
  container.remove();
}

/** Set a controlled input's value the way React tracks it (native setter + input event). */
function typeInto(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  )?.set;
  setter?.call(input, value);
  act(() => {
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function clickButton(label: string) {
  const btn = [...container.querySelectorAll("button")].find(
    (b) => (b.getAttribute("aria-label") ?? b.textContent ?? "").includes(label),
  );
  if (!btn) throw new Error(`button "${label}" not found`);
  return btn;
}

// ---- tests ------------------------------------------------------------------

describe("steerStatusLabel", () => {
  it("maps every status to a non-empty label", () => {
    expect(steerStatusLabel({ status: "queued", queued: 1 })).toContain("queued");
    expect(steerStatusLabel({ status: "stopped" }).toLowerCase()).toContain("stop");
    expect(steerStatusLabel({ status: "queue_full", queued: 4 }).toLowerCase()).toContain(
      "full",
    );
    expect(steerStatusLabel({ status: "noop" }).length).toBeGreaterThan(0);
  });
});

describe("MiniSteerBar", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue({ status: "queued", queued: 1 });
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  });
  afterEach(() => {
    unmount();
  });

  it("Send invokes mini_coder_steer with {agentId, message}", async () => {
    mount("mini-7");
    const input = container.querySelector("input") as HTMLInputElement;
    typeInto(input, "  use a Map here  ");
    await act(async () => {
      clickButton("Send").click();
      await Promise.resolve();
    });
    expect(mockInvoke).toHaveBeenCalledWith("mini_coder_steer", {
      agentId: "mini-7",
      message: "use a Map here", // trimmed
    });
  });

  it("Send clears the input on success", async () => {
    mount("mini-7");
    const input = container.querySelector("input") as HTMLInputElement;
    typeInto(input, "do the thing");
    await act(async () => {
      clickButton("Send").click();
      await Promise.resolve();
    });
    expect((container.querySelector("input") as HTMLInputElement).value).toBe("");
  });

  it("Stop invokes mini_coder_steer with message:'stop' (not mini_coder_kill)", async () => {
    mockInvoke.mockResolvedValue({ status: "stopped" });
    mount("mini-7");
    await act(async () => {
      clickButton("Stop").click();
      await Promise.resolve();
    });
    expect(mockInvoke).toHaveBeenCalledWith("mini_coder_steer", {
      agentId: "mini-7",
      message: "stop",
    });
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "mini_coder_kill",
      expect.anything(),
    );
  });

  it("Send is a no-op (no invoke) when the message is blank", async () => {
    mount("mini-7");
    const input = container.querySelector("input") as HTMLInputElement;
    typeInto(input, "    ");
    // Send is disabled, but force-clicking must also not invoke.
    await act(async () => {
      clickButton("Send").click();
      await Promise.resolve();
    });
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("Send button is disabled when message is blank", () => {
    mount("mini-7");
    expect((clickButton("Send") as HTMLButtonElement).disabled).toBe(true);
  });

  it("all controls disabled when agentId is null and Stop does not invoke", async () => {
    mount(null);
    expect((clickButton("Send") as HTMLButtonElement).disabled).toBe(true);
    expect((clickButton("Stop") as HTMLButtonElement).disabled).toBe(true);
    const input = container.querySelector("input") as HTMLInputElement;
    expect(input.disabled).toBe(true);
    await act(async () => {
      clickButton("Stop").click();
      await Promise.resolve();
    });
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("shows the returned status label after a Send", async () => {
    mockInvoke.mockResolvedValue({ status: "queue_full", queued: 4 });
    mount("mini-7");
    const input = container.querySelector("input") as HTMLInputElement;
    typeInto(input, "fix it");
    await act(async () => {
      clickButton("Send").click();
      await Promise.resolve();
    });
    expect(container.textContent?.toLowerCase()).toContain("full");
    // queue_full must NOT clear the input (the correction was not accepted).
    expect((container.querySelector("input") as HTMLInputElement).value).toBe("fix it");
  });

  it("unmounting mid-steer does not setState or leak a status timer", async () => {
    // steer resolves AFTER the component unmounts — no setState must fire,
    // and no uncancelled timer must remain (vitest fake-timers would catch it).
    vi.useFakeTimers();
    let resolveSteer!: (v: unknown) => void;
    mockInvoke.mockImplementation(
      () => new Promise((res) => { resolveSteer = res; }),
    );

    mount("mini-7");
    const input = container.querySelector("input") as HTMLInputElement;
    typeInto(input, "abort me");

    // Fire steer — it is now in-flight (mockInvoke pending).
    act(() => { clickButton("Send").click(); });

    // Unmount BEFORE the IPC resolves — must not throw / setState after unmount.
    unmount();

    // Resolve the IPC after unmount. If mountedRef guard is missing,
    // flashStatus schedules a setTimeout that leaks into the fake timer queue.
    await act(async () => {
      resolveSteer({ status: "queued", queued: 1 });
      await Promise.resolve();
    });

    // Drain all pending fake timers — no error must be thrown (no setState on
    // unmounted component). If a timer was leaked, vitest would warn/throw here.
    act(() => { vi.runAllTimers(); });

    vi.useRealTimers();
  });
});
