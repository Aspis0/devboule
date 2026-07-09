// @vitest-environment jsdom
//
// SkillsView is now GLOBAL-only: a Library tab (global skill store + URL install) and a Tools tab
// (global MCP). All per-project/per-role skill editing moved to the Work Console modal, so this
// suite only covers the global shell + tab switching.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(async (...args: unknown[]): Promise<unknown> => {
  const cmd = args[0] as string;
  if (cmd === "global_skills_list") return [];
  if (cmd === "skills_library_catalog") return [];
  if (cmd === "user_mcp_list") return [];
  return undefined;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...a: unknown[]) => invokeMock(...(a as [])),
}));

import { SkillsView } from "./SkillsView";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("SkillsView (global-only shell)", () => {
  let root: Root;
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    invokeMock.mockClear();
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(createElement(SkillsView));
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("renders the global view container and both tabs (no folder picker)", async () => {
    await mount();
    expect(document.querySelector("[data-testid='skills-view']")).toBeTruthy();
    expect(document.querySelector("[data-testid='skills-view-tab-library']")).toBeTruthy();
    expect(document.querySelector("[data-testid='skills-view-tab-tools']")).toBeTruthy();
  });

  it("defaults to the Library tab: global library panel + URL install section", async () => {
    await mount();
    expect(document.querySelector("[data-testid='global-library-panel']")).toBeTruthy();
    expect(document.body.textContent).toContain("Install from a URL");
  });

  it("switches to the Tools tab (library panel unmounts)", async () => {
    await mount();
    expect(document.querySelector("[data-testid='global-library-panel']")).toBeTruthy();
    await act(async () => {
      document
        .querySelector("[data-testid='skills-view-tab-tools']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(document.querySelector("[data-testid='global-library-panel']")).toBeNull();
  });

  it("shows a page header with h1 'Skills' and an explainer subtitle", async () => {
    await mount();
    const h1 = Array.from(document.querySelectorAll("h1")).find(
      (el) => el.textContent?.trim() === "Skills",
    );
    expect(h1).toBeTruthy();

    const body = document.body.textContent ?? "";
    expect(body.toLowerCase()).toContain("library");
    expect(body.toLowerCase()).toContain("tools");

    // The old duplicate banner text must now appear exactly ONCE (in the subtitle).
    const needle = "shared across every project";
    const matches = body.match(new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"));
    expect(matches).not.toBeNull();
    expect(matches?.length).toBe(1);

    // Tabs are unchanged.
    expect(document.querySelector("[data-testid='skills-view-tab-library']")).toBeTruthy();
    expect(document.querySelector("[data-testid='skills-view-tab-tools']")).toBeTruthy();
  });
});
