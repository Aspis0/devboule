// @vitest-environment jsdom
//
// TASK #13: the Dependencies tab. Mock `invokeBackendCommand` to return a small,
// hand-built `detect_dependencies` payload (one found + one missing across two
// categories) and assert the page groups rows by category and renders an
// Installed / Missing badge per row.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { DetectedDependency } from "../../types/backend";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let detectResult: DetectedDependency[] = [];
let detectCalls = 0;
const invokeMock = vi.fn(async (name: string) => {
  if (name === "detect_dependencies") {
    detectCalls += 1;
    return detectResult;
  }
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string) => invokeMock(name),
}));

import { DependenciesTab } from "./DependenciesTab";

let container: HTMLDivElement;
let root: Root;

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(DependenciesTab));
  });
  // Flush the detection mount effect + IPC resolve.
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  // One found + one missing, spanning two categories (Runtime + Code review (Censor)).
  detectResult = [
    {
      name: "node",
      purpose: "Runs the pi sidecar that powers the orchestrator and coders.",
      category: "Runtime",
      found: true,
      path: "/usr/local/bin/node",
      version: "v20.11.0",
    },
    {
      name: "python3",
      purpose: "Oracle indexing and embeddings.",
      category: "Runtime",
      found: false,
      path: null,
      version: null,
    },
    {
      name: "ruff",
      purpose: "Python linter used by the Censor code-review gate.",
      category: "Code review (Censor)",
      found: true,
      path: "/home/me/.local/bin/ruff",
      version: "ruff 0.5.0",
    },
    {
      name: "eslint",
      purpose: "JS/TS linter (Censor gate, if configured).",
      category: "Code review (Censor)",
      found: false,
      path: null,
      version: null,
    },
  ];
  detectCalls = 0;
  invokeMock.mockClear();
});

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  container?.remove();
});

describe("DependenciesTab", () => {
  it("calls detect_dependencies on mount", async () => {
    await mount();
    expect(detectCalls).toBeGreaterThanOrEqual(1);
    expect(invokeMock).toHaveBeenCalledWith("detect_dependencies");
  });

  it("groups rows by category and renders each tool name", async () => {
    await mount();
    const headings = Array.from(
      container.querySelectorAll("h2"),
    ).map((h) => h.textContent);
    expect(headings).toContain("Runtime");
    expect(headings).toContain("Code review (Censor)");

    // Every tool name appears (monospace spans).
    const names = Array.from(container.querySelectorAll("span.font-mono")).map(
      (s) => s.textContent,
    );
    expect(names).toContain("node");
    expect(names).toContain("python3");
    expect(names).toContain("ruff");
    expect(names).toContain("eslint");
  });

  it("renders an Installed badge for found tools and Missing for absent ones", async () => {
    await mount();
    expect(container.textContent).toContain("Installed");
    expect(container.textContent).toContain("Missing");

    // The found tools render their version.
    expect(container.textContent).toContain("v20.11.0");
    expect(container.textContent).toContain("ruff 0.5.0");

    // The found tools render their resolved path (title + visible truncated text).
    const titled = Array.from(container.querySelectorAll("span[title]")).map(
      (s) => s.getAttribute("title"),
    );
    expect(titled).toContain("/usr/local/bin/node");
    expect(titled).toContain("/home/me/.local/bin/ruff");

    // Missing tools do NOT render a path title.
    expect(titled).not.toContain(null);
    expect(
      Array.from(container.querySelectorAll("span[title]")).some(
        (s) => s.getAttribute("title") === null,
      ),
    ).toBe(false);
  });

  it("shows the intro line about missing-only-disabling features", async () => {
    await mount();
    expect(container.textContent).toContain(
      "Missing ones only disable the features that need them.",
    );
  });
});
