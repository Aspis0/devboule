// @vitest-environment jsdom
//
// Regression test for the throttled manifest-flush path in DesignView. The bug:
// `flushManifest` captured `folder` in its closure, so the throttle/unmount
// cleanup (which re-runs on EVERY folder keystroke) flushed a pending manifest to
// the OLD/half-typed path. The fix reads the folder from a live ref at call time.
//
// Canvas is mocked to a tiny stub exposing a "commit" button so the test drives
// `onManifestChange` deterministically without an iframe. The backend is mocked so
// we can assert the EXACT `workingFolderPath` a flush writes to.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignManifest } from "../../types/design";

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

// Native folder picker mock: a test sets `nextPick` to the path the dialog should
// return (or null to simulate a dismissed dialog). The folder is now CHOSEN via
// this picker, never typed.
const dialogCtl: { nextPick: string | null } = { nextPick: null };
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogCtl.nextPick),
}));

// Stub Canvas: render a button that fires the parent's onManifestChange with a
// known manifest, so the test triggers exactly one pending write.
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: { onManifestChange: (m: DesignManifest) => void }) =>
    createElement("button", {
      type: "button",
      "data-testid": "commit",
      onClick: () =>
        props.onManifestChange({ schemaVersion: 1, nodes: {} }),
    }),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;

beforeEach(async () => {
  vi.useFakeTimers();
  invokeSpy.mockClear();
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  vi.useRealTimers();
});

function render(): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(DesignView));
  });
  return { container, root };
}

// Choose the working folder via the native picker. The folder controls now live in
// the TopBar's ProjectPopover: open it, then click "Open working folder…", which
// picks the folder itself (dialog mocked to `value`). The folder is adopted
// synchronously by loadFolder even when the (unmocked) load resolves to nothing, so
// subsequent manifest writes target it — exactly what this test asserts.
async function setFolder(container: HTMLElement, value: string) {
  dialogCtl.nextPick = value;
  if (!container.querySelector(".pop.left")) {
    const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  }
  const pickBtn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.includes("Open working folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    pickBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    // Flush the dynamic import + awaited open() microtasks (works under fake timers).
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("DesignView — throttled manifest flush targets the CURRENT folder", () => {
  it("flushes a pending manifest to the latest folder, not a stale closure path", async () => {
    const { container } = render();

    // Pick an initial folder, commit a manifest (schedules a throttled write).
    await setFolder(container, "C:/old/path");
    const commit = container.querySelector("[data-testid=commit]") as HTMLButtonElement;
    act(() => commit.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    // Now re-pick the folder BEFORE the throttle fires (the state change that used
    // to re-create flushManifest and flush to the OLD path on cleanup).
    await setFolder(container, "C:/new/path");

    // Let the throttle elapse.
    act(() => {
      vi.advanceTimersByTime(500);
    });

    // Exactly one manifest write, to the NEW folder.
    const writeCalls = invokeSpy.mock.calls.filter(
      (c) => c[0] === "design_write_manifest",
    );
    expect(writeCalls).toHaveLength(1);
    expect((writeCalls[0][1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "C:/new/path",
    );
  });

  it("flushes the pending manifest on unmount to the current folder", async () => {
    const { container, root } = render();
    await setFolder(container, "C:/live");
    const commit = container.querySelector("[data-testid=commit]") as HTMLButtonElement;
    act(() => commit.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    // Unmount before the throttle fires -> cleanup flushes the pending write.
    act(() => root.unmount());

    const writeCalls = invokeSpy.mock.calls.filter(
      (c) => c[0] === "design_write_manifest",
    );
    expect(writeCalls).toHaveLength(1);
    expect((writeCalls[0][1] as { workingFolderPath: string }).workingFolderPath).toBe(
      "C:/live",
    );
  });
});
