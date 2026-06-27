// @vitest-environment jsdom
//
// P3 TDD — per-profile MCP TOOL picker for the Work Console modal.
// Contract (failing first): mini-small is tool-forbidden (disabled note, no fetch);
// other profiles fetch the library + current assignment, render a toggle row per
// available server, reflect assigned state, enforce a 5-tool cap, and persist a
// toggle via tools_assignment_set.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(
  async (..._args: unknown[]): Promise<unknown> => undefined,
);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { ToolsPicker } from "./ToolsPicker";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// Backend mock: dispatch by command name.
function wireMock(assigned: string[]) {
  invokeMock.mockImplementation(async (...args: unknown[]) => {
    const cmd = args[0] as string;
    if (cmd === "tools_library_list") {
      return [
        {
          name: "fs",
          transport: "stdio",
          command: "x",
          args: [],
          env: {},
          enabled: true,
        },
        {
          name: "git",
          transport: "stdio",
          command: "x",
          args: [],
          env: {},
          enabled: true,
        },
        {
          name: "web",
          transport: "stdio",
          command: "x",
          args: [],
          env: {},
          enabled: true,
        },
      ];
    }
    if (cmd === "tools_assignment_list") return assigned;
    if (cmd === "tools_assignment_set") return undefined;
    return undefined;
  });
}

describe("ToolsPicker (P3)", () => {
  let root: Root;
  let container: HTMLDivElement;
  const projectRoot = "/proj";

  beforeEach(() => {
    wireMock(["fs"]);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
    invokeMock.mockReset();
  });

  async function mount(profile: string): Promise<void> {
    await act(async () => {
      root.render(createElement(ToolsPicker, { projectRoot, profile }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("mini-small is tool-forbidden: shows a disabled note and does NOT fetch the library", async () => {
    await mount("mini-small");
    expect(
      document.querySelector("[data-testid='tools-picker-disabled']"),
    ).toBeTruthy();
    const libCall = invokeMock.mock.calls.find(
      (c) => c[0] === "tools_library_list",
    );
    expect(libCall).toBeFalsy();
  });

  it("coder fetches the library + assignment and renders a row per available server", async () => {
    await mount("coder");
    const lib = invokeMock.mock.calls.find(
      (c) => c[0] === "tools_library_list",
    );
    expect(lib).toBeTruthy();
    expect(lib![1]).toMatchObject({ workingFolderPath: projectRoot });
    const asg = invokeMock.mock.calls.find(
      (c) => c[0] === "tools_assignment_list",
    );
    expect(asg![1]).toMatchObject({
      workingFolderPath: projectRoot,
      profile: "coder",
    });
    expect(document.querySelector("[data-testid='tools-row-fs']")).toBeTruthy();
    expect(
      document.querySelector("[data-testid='tools-row-git']"),
    ).toBeTruthy();
    expect(
      document.querySelector("[data-testid='tools-row-web']"),
    ).toBeTruthy();
  });

  it("reflects the assigned set (fs assigned, git not)", async () => {
    await mount("coder");
    const fs = document.querySelector(
      "[data-testid='tools-row-fs']",
    ) as HTMLButtonElement;
    const git = document.querySelector(
      "[data-testid='tools-row-git']",
    ) as HTMLButtonElement;
    expect(fs.getAttribute("aria-pressed")).toBe("true");
    expect(git.getAttribute("aria-pressed")).toBe("false");
  });

  it("toggling an unassigned server persists via tools_assignment_set with the new set", async () => {
    await mount("coder");
    await act(async () => {
      document
        .querySelector("[data-testid='tools-row-git']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const setCall = invokeMock.mock.calls.find(
      (c) => c[0] === "tools_assignment_set",
    );
    expect(setCall).toBeTruthy();
    expect(setCall![1]).toMatchObject({
      workingFolderPath: projectRoot,
      profile: "coder",
    });
    const names = (setCall![1] as { names: string[] }).names;
    expect(names).toContain("fs");
    expect(names).toContain("git");
  });

  it("disables an unassigned row once the 5-tool cap is reached", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "tools_library_list") {
        return ["a", "b", "c", "d", "e", "f"].map((name) => ({
          name,
          transport: "stdio",
          command: "x",
          args: [],
          env: {},
          enabled: true,
        }));
      }
      if (cmd === "tools_assignment_list") return ["a", "b", "c", "d", "e"]; // 5 = cap
      return undefined;
    });
    await mount("coder");
    const assignedRow = document.querySelector(
      "[data-testid='tools-row-a']",
    ) as HTMLButtonElement;
    const freeRow = document.querySelector(
      "[data-testid='tools-row-f']",
    ) as HTMLButtonElement;
    expect(assignedRow.disabled).toBe(false); // assigned rows stay toggle-off-able
    expect(freeRow.disabled).toBe(true); // unassigned blocked at cap
  });

  it("shows the count out of the 5-tool cap", async () => {
    await mount("coder");
    const count =
      document.querySelector("[data-testid='tools-count']")?.textContent ?? "";
    expect(count).toContain("1");
    expect(count).toContain("5");
  });
});
