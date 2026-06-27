// @vitest-environment jsdom
//
// P1 TDD — Work Console "Skills & Tools" modal.
// Covers: skills_list fetch on mount, dialog render, role-tab enabled/disabled
// states (coder+mini active, design+orchestrator disabled "coming soon"),
// default coder manual shown, tab switch to mini, close button.
// Test authored from the spec via local oMLX; finalized (truncated tail + import).

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

describe("SkillsToolsModal (P1)", () => {
  let root: Root;
  let container: HTMLDivElement;
  const projectRoot = "/proj";
  const onCloseMock = vi.fn();

  beforeEach(() => {
    invokeMock.mockResolvedValue([
      { role: "coder", exists: true, enabled: true, content: "CODER MANUAL BODY", bytes: 0, truncated: false },
      { role: "mini", exists: true, enabled: true, content: "MINI MANUAL BODY", bytes: 0, truncated: false },
      { role: "design", exists: true, enabled: true, content: "DESIGN MANUAL BODY", bytes: 0, truncated: false },
      { role: "orchestrator", exists: true, enabled: true, content: "ORCH MANUAL BODY", bytes: 0, truncated: false },
    ]);
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

  it("calls skills_list with the project root on mount", async () => {
    await mount();
    const call = invokeMock.mock.calls.find((c) => c[0] === "skills_list");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ workingFolderPath: projectRoot });
  });

  it("renders the modal dialog", async () => {
    await mount();
    expect(document.querySelector("[data-testid='skills-tools-modal']")).toBeTruthy();
  });

  it("renders role tabs: coder+mini enabled, design+orchestrator disabled", async () => {
    await mount();
    const coder = document.querySelector("[data-testid='skills-tools-tab-coder']") as HTMLButtonElement;
    const mini = document.querySelector("[data-testid='skills-tools-tab-mini']") as HTMLButtonElement;
    const design = document.querySelector("[data-testid='skills-tools-tab-design']") as HTMLButtonElement;
    const orch = document.querySelector("[data-testid='skills-tools-tab-orchestrator']") as HTMLButtonElement;
    expect(coder).toBeTruthy();
    expect(mini).toBeTruthy();
    expect(design).toBeTruthy();
    expect(orch).toBeTruthy();
    expect(coder.disabled).toBe(false);
    expect(mini.disabled).toBe(false);
    expect(design.disabled).toBe(true);
    expect(orch.disabled).toBe(true);
  });

  it("shows the coder manual content by default", async () => {
    await mount();
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("CODER MANUAL BODY");
  });

  it("switches to the mini manual when the mini tab is clicked", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='skills-tools-tab-mini']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("MINI MANUAL BODY");
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

  it("shows an empty-state when skills_list returns no entries", async () => {
    invokeMock.mockResolvedValue([]);
    await mount();
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("No skill manual");
  });

  it("shows an error state when skills_list rejects", async () => {
    invokeMock.mockRejectedValue(new Error("backend down"));
    await mount();
    await act(async () => {
      await Promise.resolve();
    });
    const content = document.querySelector("[data-testid='skills-tools-skill-content']")?.textContent;
    expect(content).toContain("Couldn't load");
  });
});
