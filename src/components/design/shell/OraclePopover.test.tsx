// @vitest-environment jsdom
//
// OraclePopover tests: fetches design_oracle_status ON OPEN (not before) with the
// working folder, renders the stats (thousands-sep chunks, files, relative sync),
// degrades to not-grounded on error, and shows token swatches from the doc.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { DesignOracleStatus } from "../../../types/design";
import { OraclePopover, type OraclePopoverProps } from "./OraclePopover";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function mount(props: OraclePopoverProps) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: ReturnType<typeof createRoot>;
  act(() => {
    root = createRoot(container);
    root.render(createElement(OraclePopover, props));
  });
  const rerender = (next: OraclePopoverProps) =>
    act(() => root.render(createElement(OraclePopover, next)));
  return { container, rerender };
}

type Invoke = OraclePopoverProps["invoke"];

/** Wrap a status-returning fn as the generic `invoke<T>` the popover expects. */
function asInvoke(fn: (...a: unknown[]) => Promise<unknown>): Invoke {
  return fn as unknown as Invoke;
}

function base(over: Partial<OraclePopoverProps> = {}): OraclePopoverProps {
  return {
    open: false,
    onClose: vi.fn(),
    workingFolderPath: "/x/landing",
    tokens: {},
    invoke: asInvoke(async () => ({ grounded: false }) as DesignOracleStatus),
    tauri: true,
    ...over,
  };
}

describe("OraclePopover", () => {
  it("does NOT fetch while closed", async () => {
    const invoke = vi.fn(async () => ({ grounded: false }) as DesignOracleStatus);
    mount(base({ open: false, invoke: asInvoke(invoke) }));
    await flush();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("fetches design_oracle_status with the working folder when opened", async () => {
    const invoke = vi.fn(
      async () =>
        ({
          grounded: true,
          rootLabel: "devboule",
          chunks: 1284,
          files: 212,
          lastSyncIso: new Date(Date.now() - 2 * 60_000).toISOString(),
        }) as DesignOracleStatus,
    );
    const { container, rerender } = mount(base({ open: false, invoke: asInvoke(invoke) }));
    rerender(base({ open: true, invoke: asInvoke(invoke) }));
    await flush();

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("design_oracle_status", {
      workingFolderPath: "/x/landing",
    });

    // Stats render: chunks with thousands sep, files, relative sync.
    const chunks = container.querySelector(
      "[data-testid=op-stat-chunks] b",
    ) as HTMLElement;
    const files = container.querySelector(
      "[data-testid=op-stat-files] b",
    ) as HTMLElement;
    const sync = container.querySelector(
      "[data-testid=op-stat-sync] b",
    ) as HTMLElement;
    expect(chunks.textContent).toBe("1,284");
    expect(files.textContent).toBe("212");
    expect(sync.textContent).toBe("2m ago");
    // Grounded head label uses the root.
    expect(container.textContent).toContain("target: devboule");
  });

  it("degrades to not-grounded when the command rejects (never errors)", async () => {
    const invoke = vi.fn(async () => {
      throw new Error("oracle down");
    });
    const { container, rerender } = mount(base({ open: false, invoke: asInvoke(invoke) }));
    rerender(base({ open: true, invoke: asInvoke(invoke) }));
    await flush();
    expect(container.textContent).toContain("no index found");
    const chunks = container.querySelector(
      "[data-testid=op-stat-chunks] b",
    ) as HTMLElement;
    expect(chunks.textContent).toBe("0");
  });

  it("does not fetch in web runtime (tauri=false) and shows not-grounded", async () => {
    const invoke = vi.fn(async () => ({ grounded: false }) as DesignOracleStatus);
    const { container, rerender } = mount(base({ open: false, invoke: asInvoke(invoke), tauri: false }));
    rerender(base({ open: true, invoke: asInvoke(invoke), tauri: false }));
    await flush();
    expect(invoke).not.toHaveBeenCalled();
    expect(container.textContent).toContain("no index found");
  });

  it("renders up to 4 color swatches from the tokens doc", async () => {
    const tokens = {
      color: {
        a: { $value: "#111111", $type: "color" },
        b: { $value: "#222222", $type: "color" },
        c: { $value: "#333333", $type: "color" },
        d: { $value: "#444444", $type: "color" },
        e: { $value: "#555555", $type: "color" },
      },
    };
    const { container, rerender } = mount(base({ open: false, tokens }));
    rerender(base({ open: true, tokens }));
    await flush();
    const sw = container.querySelectorAll(".op-tokens .sw i");
    expect(sw.length).toBe(4);
  });

  it("shows 'No tokens yet' when the doc has no colors", async () => {
    const { container, rerender } = mount(base({ open: false, tokens: {} }));
    rerender(base({ open: true, tokens: {} }));
    await flush();
    expect(container.textContent).toContain("No tokens yet");
  });
});
