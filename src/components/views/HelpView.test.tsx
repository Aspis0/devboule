// @vitest-environment jsdom
//
// HelpView is pure static content (no backend calls), so it mounts with a plain
// createRoot + act render. We mock useAppActions to assert the cross-link buttons
// actually call requestView.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const requestView = vi.fn();

vi.mock("../../context/AppContext", () => ({
  useAppActions: () => ({ requestView }),
  useAppContext: () => ({
    activeView: "help",
    config: { project: { name: "Devboule" } },
    roleStatus: { role: "admin", isAdmin: true, provisioned: true },
  }),
}));

import { HelpView, HELP_SECTIONS } from "./HelpView";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("HelpView (static help page)", () => {
  let root: Root;
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    requestView.mockClear();
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(createElement(HelpView));
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("renders the page header with an h1 'Help' and subtitle", async () => {
    await mount();
    const h1 = Array.from(document.querySelectorAll("h1")).find(
      (el) => el.textContent?.trim() === "Help",
    );
    expect(h1).toBeTruthy();
    expect(document.body.textContent ?? "").toContain(
      "start here if you're new",
    );
  });

  it("renders every HELP_SECTIONS title", async () => {
    await mount();
    const body = document.body.textContent ?? "";
    for (const section of HELP_SECTIONS) {
      expect(body).toContain(section.title);
      // Each section is also anchored for in-page nav / cross-links.
      expect(document.getElementById(section.id)).toBeTruthy();
    }
  });

  it("quick-start section lists exactly 5 steps", async () => {
    await mount();
    const quickStart = document.getElementById("quick-start");
    expect(quickStart).toBeTruthy();
    const items = quickStart?.querySelectorAll("ol li") ?? [];
    expect(items.length).toBe(5);
    // Spot-check a couple of the expected step substrings.
    const text = quickStart?.textContent ?? "";
    expect(text).toContain("Create");
    expect(text).toContain("Launch a coder");
    expect(text).toContain("Commit/Push");
  });

  it("clicking the Skills cross-link fires requestView('skills')", async () => {
    await mount();
    const link = document.querySelector<HTMLButtonElement>(
      "[data-testid='help-link-skills']",
    );
    expect(link).toBeTruthy();
    await act(async () => {
      link?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(requestView).toHaveBeenCalledTimes(1);
    expect(requestView).toHaveBeenCalledWith("skills");
  });

  it("renders an in-page nav with an anchor for every section", async () => {
    await mount();
    const nav = document.querySelector("nav");
    expect(nav).toBeTruthy();
    const anchors = Array.from(nav?.querySelectorAll("a") ?? []);
    expect(anchors.length).toBe(HELP_SECTIONS.length);
    for (const a of anchors) {
      const href = a.getAttribute("href") ?? "";
      expect(href.startsWith("#")).toBe(true);
    }
  });
});
