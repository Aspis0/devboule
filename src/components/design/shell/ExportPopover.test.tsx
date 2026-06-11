// @vitest-environment jsdom
//
// ExportPopover tests: the three rows call the matching DesignView export handlers
// with the right mode and close the popover; rows disable with no project.

import { describe, it, expect, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { ExportPopover, type ExportPopoverProps } from "./ExportPopover";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function render(props: ExportPopoverProps): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  act(() => {
    createRoot(container).render(createElement(ExportPopover, props));
  });
  return container;
}

function row(container: HTMLElement, prefix: string): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.trim().startsWith(prefix),
  ) as HTMLButtonElement;
}

describe("ExportPopover", () => {
  it("absolute row -> runExport('absolute') and closes", () => {
    const runExport = vi.fn();
    const onClose = vi.fn();
    const c = render({
      open: true,
      onClose,
      disabled: false,
      runExport,
      exportTokens: vi.fn(),
    });
    act(() =>
      row(c, "Standalone HTML").dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      ),
    );
    expect(runExport).toHaveBeenCalledWith("absolute");
    expect(onClose).toHaveBeenCalled();
  });

  it("flow row -> runExport('flow')", () => {
    const runExport = vi.fn();
    const c = render({
      open: true,
      onClose: vi.fn(),
      disabled: false,
      runExport,
      exportTokens: vi.fn(),
    });
    act(() =>
      row(c, "HTML scaffold").dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      ),
    );
    expect(runExport).toHaveBeenCalledWith("flow");
  });

  it("tokens row -> exportTokens()", () => {
    const exportTokens = vi.fn();
    const c = render({
      open: true,
      onClose: vi.fn(),
      disabled: false,
      runExport: vi.fn(),
      exportTokens,
    });
    act(() =>
      row(c, "Design tokens").dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      ),
    );
    expect(exportTokens).toHaveBeenCalledTimes(1);
  });

  it("disables all rows when no project is open", () => {
    const c = render({
      open: true,
      onClose: vi.fn(),
      disabled: true,
      runExport: vi.fn(),
      exportTokens: vi.fn(),
    });
    expect(row(c, "Standalone HTML").disabled).toBe(true);
    expect(row(c, "HTML scaffold").disabled).toBe(true);
    expect(row(c, "Design tokens").disabled).toBe(true);
  });

  it("renders nothing when closed", () => {
    const c = render({
      open: false,
      onClose: vi.fn(),
      disabled: false,
      runExport: vi.fn(),
      exportTokens: vi.fn(),
    });
    expect(c.querySelector(".pop")).toBeNull();
  });
});
