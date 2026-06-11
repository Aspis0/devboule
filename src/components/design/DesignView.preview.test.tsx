// @vitest-environment jsdom
//
// Phase B preview-slice integration tests in DesignView:
//   - the TopBar "Preview" button exports the absolute layout then opens the
//     preview window (design_write_export -> design_preview_open · "absolute"),
//   - the assistant "Visual check" button captures + critiques and patches the
//     working card to a done critique card,
//   - a backend visual-critique error patches the card to an error message,
//   - the Visual-check button is disabled while a check is in flight.
// Canvas + the streaming hook are stubbed (not under test).

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { DesignProject, DesignProjectEntry } from "../../types/design";

const invokeSpy =
  vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(
    async () => undefined,
  );

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (command: string, args?: Record<string, unknown>) =>
    invokeSpy(command, args),
  isTauriRuntime: () => true,
  useAppContext: () => ({ requestView: vi.fn() }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

vi.mock("./useDesignStream", () => ({
  useDesignStream: () => ({
    text: "",
    status: "idle" as const,
    error: null,
    start: () => {},
    cancel: () => {},
    reset: () => {},
  }),
}));

vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: () => createElement("div", { "data-testid": "canvas" }),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;

function entry(over: Partial<DesignProjectEntry>): DesignProjectEntry {
  return {
    id: "a",
    name: "Alpha",
    workingFolderPath: "/x/alpha",
    createdAt: "2021-01-01T00:00:00Z",
    updatedAt: "2021-01-01T00:00:00Z",
    lastOpenedAt: "2021-01-01T00:00:00Z",
    ...over,
  };
}

function loadedProject(name: string): DesignProject {
  return {
    meta: {
      schemaVersion: 1,
      id: "p",
      name,
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      // One node so export has content (and the canvas-empty overlay is hidden).
      nodeOrder: ["hero"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: { hero: { x: 0, y: 0, z: 1, w: 100, h: "auto", kind: "html" } },
    },
    components: { hero: '<div data-node-id="hero">Hero</div>' },
  };
}

/** A resolver registry so individual tests can override per-command behavior. */
const handlers: { value: Record<string, (args?: Record<string, unknown>) => unknown> } =
  { value: {} };

beforeEach(async () => {
  invokeSpy.mockReset();
  handlers.value = {
    design_registry_list: () => [entry({})],
    design_load_project: () => loadedProject("Alpha"),
    design_registry_remember: () => [entry({})],
  };
  invokeSpy.mockImplementation(async (command: string, args) => {
    const h = handlers.value[command];
    return h ? h(args) : undefined;
  });
  ({ DesignView } = await import("./DesignView"));
});

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function render(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(DesignView));
  });
  return container;
}

function findButton(container: HTMLElement, text: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.trim().startsWith(text),
  ) as HTMLButtonElement;
}

function openProjectPopover(container: HTMLElement) {
  if (container.querySelector(".pop.left")) return;
  const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
  act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** Open a recent project so projectOpen === true (enables Preview + Visual check). */
async function openProject(container: HTMLElement) {
  await flush();
  openProjectPopover(container);
  const openBtn = (
    container.querySelector("[data-testid=design-recent-item]") as HTMLElement
  ).querySelector("button") as HTMLButtonElement;
  await act(async () => {
    openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function commandsAfter(predicate: (cmd: string) => boolean): string[] {
  return invokeSpy.mock.calls.map((c) => c[0]).filter(predicate);
}

describe("DesignView — Preview button", () => {
  it("exports the absolute layout then opens the preview window, in order", async () => {
    const container = render();
    await openProject(container);
    invokeSpy.mockClear();

    const previewBtn = findButton(container, "Preview");
    expect(previewBtn).toBeTruthy();
    expect(previewBtn.disabled).toBe(false);
    await act(async () => {
      previewBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    const exportCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_write_export",
    );
    const openCall = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_preview_open",
    );
    expect(exportCall?.[1]).toMatchObject({
      workingFolderPath: "/x/alpha",
      filename: "export-absolute.html",
    });
    expect(openCall?.[1]).toMatchObject({
      workingFolderPath: "/x/alpha",
      mode: "absolute",
    });
    // Export strictly precedes the window open.
    const seq = commandsAfter(
      (cmd) => cmd === "design_write_export" || cmd === "design_preview_open",
    );
    expect(seq).toEqual(["design_write_export", "design_preview_open"]);
  });

  it("disables Preview when no project is open", async () => {
    handlers.value.design_registry_list = () => [];
    const container = render();
    await flush();
    const previewBtn = findButton(container, "Preview");
    expect(previewBtn.disabled).toBe(true);
  });
});

describe("DesignView — Visual check", () => {
  it("patches the working card to a done critique on success", async () => {
    handlers.value.design_preview_capture = () => ({ path: "preview.png", bytes: 10 });
    handlers.value.design_visual_critique = () => ({
      critique: "The hero contrast is low; tighten the CTA spacing.",
    });
    const container = render();
    await openProject(container);

    const vcBtn = container.querySelector(
      "[aria-label='Visual check']",
    ) as HTMLButtonElement;
    expect(vcBtn).toBeTruthy();
    await act(async () => {
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    await flush();

    expect(container.textContent).toContain("Visual critique");
    expect(container.textContent).toContain("The hero contrast is low");
    // The capture + critique ran, and the thumbnail was recorded best-effort.
    expect(commandsAfter((c) => c === "design_preview_capture")).toHaveLength(1);
    expect(commandsAfter((c) => c === "design_visual_critique")).toHaveLength(1);
  });

  it("records the thumbnail via registry remember with thumbnailPath", async () => {
    handlers.value.design_preview_capture = () => ({ path: "preview.png", bytes: 10 });
    handlers.value.design_visual_critique = () => ({ critique: "ok" });
    const container = render();
    await openProject(container);
    invokeSpy.mockClear();

    const vcBtn = container.querySelector(
      "[aria-label='Visual check']",
    ) as HTMLButtonElement;
    await act(async () => {
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    await flush();

    const remember = invokeSpy.mock.calls.find(
      (c) => c[0] === "design_registry_remember",
    );
    expect(remember?.[1]).toMatchObject({
      entry: { workingFolderPath: "/x/alpha", thumbnailPath: "preview.png" },
    });
    // contractSha must NOT be sent on a thumbnail remember (the upsert preserves it).
    expect(
      (remember?.[1] as { entry: Record<string, unknown> }).entry,
    ).not.toHaveProperty("contractSha");
  });

  it("shows the backend's clean error when critique fails", async () => {
    handlers.value.design_preview_capture = () => ({ path: "preview.png", bytes: 10 });
    handlers.value.design_visual_critique = () => {
      // Tauri rejects IPC with a plain string.
      return Promise.reject("Local AI (Ollama) is not configured for this project.");
    };
    const container = render();
    await openProject(container);

    const vcBtn = container.querySelector(
      "[aria-label='Visual check']",
    ) as HTMLButtonElement;
    await act(async () => {
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    await flush();

    expect(container.textContent).toContain("Visual check failed");
    expect(container.textContent).toContain("Local AI (Ollama) is not configured");
  });

  it("two synchronous clicks push exactly one card + one capture (no ghost cards)", async () => {
    // Re-entry regression: beginCheck() claims the slot synchronously, so a second click in
    // the same tick must NOT push a second user chip / working card nor fire a second
    // capture. The capture is gated so both clicks land while the first is in flight.
    let releaseCapture: (v: unknown) => void = () => {};
    const gate = new Promise((r) => (releaseCapture = r));
    handlers.value.design_preview_capture = async () => {
      await gate;
      return { path: "preview.png", bytes: 10 };
    };
    handlers.value.design_visual_critique = () => ({ critique: "ok" });
    const container = render();
    await openProject(container);
    invokeSpy.mockClear();

    const vcBtn = container.querySelector(
      "[aria-label='Visual check']",
    ) as HTMLButtonElement;
    // Two clicks in the SAME act tick (synchronous double-click).
    await act(async () => {
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Exactly one user chip (.msg-user) and one assistant/working card (.msg-ai).
    expect(container.querySelectorAll(".msg-user").length).toBe(1);
    expect(container.querySelectorAll(".msg-ai").length).toBe(1);
    // Exactly one capture reached the backend.
    expect(commandsAfter((c) => c === "design_preview_capture")).toHaveLength(1);

    await act(async () => {
      releaseCapture({});
      await Promise.resolve();
      await Promise.resolve();
    });
    await flush();
    // Still exactly one critique card resolved.
    expect(commandsAfter((c) => c === "design_visual_critique")).toHaveLength(1);
  });

  it("disables the Visual-check button while a check is in flight", async () => {
    let releaseCapture: (v: unknown) => void = () => {};
    const gate = new Promise((r) => (releaseCapture = r));
    handlers.value.design_preview_capture = async () => {
      await gate;
      return { path: "preview.png", bytes: 10 };
    };
    handlers.value.design_visual_critique = () => ({ critique: "ok" });
    const container = render();
    await openProject(container);

    const vcBtn = container.querySelector(
      "[aria-label='Visual check']",
    ) as HTMLButtonElement;
    await act(async () => {
      vcBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });
    // Mid-flight: button disabled.
    expect(
      (container.querySelector("[aria-label='Visual check']") as HTMLButtonElement)
        .disabled,
    ).toBe(true);

    await act(async () => {
      releaseCapture({});
      await Promise.resolve();
      await Promise.resolve();
    });
    await flush();
    // After completion: re-enabled.
    expect(
      (container.querySelector("[aria-label='Visual check']") as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });
});
