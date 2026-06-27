// @vitest-environment jsdom
//
// P2 TDD — Work Console "Skills & Tools" modal, ASSIGNMENT-PROFILE tiers.
// Covers: skills_list_profiles fetch on mount, dialog render, profile-tab
// enabled/disabled states (coder + mini-big + mini-small active; design +
// orchestrator disabled "coming soon"), default coder manual shown, tab switch
// to mini-big, the two mini tiers being DISTINCT tabs, close button, Escape,
// scrim click, empty-state, error-state.
// Adapted from the P1 suite (10 cases) to the profile model; +1 case for the tiers.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { SkillsToolsModal } from "./SkillsToolsModal";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

describe("SkillsToolsModal (P2 — profile tiers)", () => {
  let root: Root;
  let container: HTMLDivElement;
  const projectRoot = "/proj";
  const onCloseMock = vi.fn();

  beforeEach(() => {
    // Command-aware: skills_list_profiles returns the manuals; the ToolsPicker's
    // tools_* commands return empty so the embedded picker renders cleanly.
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "skills_list_profiles") {
        return [
          { role: "coder", exists: true, enabled: true, content: "CODER MANUAL BODY", bytes: 0, truncated: false },
          { role: "mini-big", exists: true, enabled: true, content: "MINI BIG MANUAL BODY", bytes: 0, truncated: false },
          { role: "mini-small", exists: true, enabled: true, content: "MINI SMALL MANUAL BODY", bytes: 0, truncated: false },
          { role: "design", exists: true, enabled: true, content: "DESIGN MANUAL BODY", bytes: 0, truncated: false },
          { role: "orchestrator", exists: true, enabled: true, content: "ORCH MANUAL BODY", bytes: 0, truncated: false },
        ];
      }
      if (cmd === "tools_library_list") return [];
      if (cmd === "tools_assignment_list") return [];
      if (cmd === "global_skills_list") return [];
      return undefined;
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
    invokeMock.mockClear();
    onCloseMock.mockClear();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(createElement(SkillsToolsModal, { projectRoot, onClose: onCloseMock }));
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("calls skills_list_profiles with the project root on mount", async () => {
    await mount();
    const call = invokeMock.mock.calls.find((c) => c[0] === "skills_list_profiles");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ workingFolderPath: projectRoot });
  });

  it("renders the modal dialog", async () => {
    await mount();
    expect(document.querySelector("[data-testid='skills-tools-modal']")).toBeTruthy();
  });

  it("renders profile tabs: coder + both mini tiers enabled, design + orchestrator disabled", async () => {
    await mount();
    const coder = document.querySelector("[data-testid='skills-tools-tab-coder']") as HTMLButtonElement;
    const big = document.querySelector("[data-testid='skills-tools-tab-mini-big']") as HTMLButtonElement;
    const small = document.querySelector("[data-testid='skills-tools-tab-mini-small']") as HTMLButtonElement;
    const design = document.querySelector("[data-testid='skills-tools-tab-design']") as HTMLButtonElement;
    const orch = document.querySelector("[data-testid='skills-tools-tab-orchestrator']") as HTMLButtonElement;
    expect(coder).toBeTruthy();
    expect(big).toBeTruthy();
    expect(small).toBeTruthy();
    expect(design).toBeTruthy();
    expect(orch).toBeTruthy();
    expect(coder.disabled).toBe(false);
    expect(big.disabled).toBe(false);
    expect(small.disabled).toBe(false);
    expect(design.disabled).toBe(true);
    expect(orch.disabled).toBe(true);
  });

  it("shows the coder manual content by default", async () => {
    await mount();
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("CODER MANUAL BODY");
  });

  it("switches to the mini-big manual when the mini-big tab is clicked", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='skills-tools-tab-mini-big']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("MINI BIG MANUAL BODY");
  });

  it("treats mini-big and mini-small as DISTINCT tabs with distinct manuals", async () => {
    await mount();
    // Switch to mini-small and confirm its manual (NOT the big tier's) shows.
    await act(async () => {
      document
        .querySelector("[data-testid='skills-tools-tab-mini-small']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const small = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(small).toContain("MINI SMALL MANUAL BODY");
    expect(small).not.toContain("MINI BIG MANUAL BODY");
    // The two tabs are different DOM nodes.
    const bigTab = document.querySelector("[data-testid='skills-tools-tab-mini-big']");
    const smallTab = document.querySelector("[data-testid='skills-tools-tab-mini-small']");
    expect(bigTab).not.toBe(smallTab);
  });

  it("calls onClose when the close button is clicked", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='skills-tools-close']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onCloseMock).toHaveBeenCalledOnce();
  });

  it("closes on Escape key", async () => {
    await mount();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(onCloseMock).toHaveBeenCalledOnce();
  });

  it("closes on scrim (backdrop) click", async () => {
    await mount();
    const scrim = document.querySelector("[data-testid='skills-tools-modal']")
      ?.parentElement as HTMLElement;
    await act(async () => {
      scrim.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onCloseMock).toHaveBeenCalledOnce();
  });

  it("shows an empty-state when skills_list_profiles returns no entries", async () => {
    invokeMock.mockResolvedValue([]);
    await mount();
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("No skill manual");
  });

  it("shows an error state when skills_list_profiles rejects", async () => {
    invokeMock.mockRejectedValue(new Error("backend down"));
    await mount();
    await act(async () => {
      await Promise.resolve();
    });
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("Couldn't load");
  });
});
