// @vitest-environment jsdom
//
// P4 TDD — global skills library search (cmdk-scored) + apply-to-profile.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(
  async (..._args: unknown[]): Promise<unknown> => undefined,
);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { LibrarySearch } from "./LibrarySearch";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const LIB = [
  {
    name: "plan-first",
    content: "PLAN FIRST BODY",
    bytes: 14,
    truncated: false,
  },
  {
    name: "review-hard",
    content: "REVIEW HARD BODY",
    bytes: 16,
    truncated: false,
  },
];

describe("LibrarySearch (P4)", () => {
  let root: Root;
  let container: HTMLDivElement;
  const projectRoot = "/proj";

  beforeEach(() => {
    vi.spyOn(window, "confirm").mockReturnValue(true); // Apply confirms overwrite
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      if ((args[0] as string) === "global_skills_list") return LIB;
      return undefined; // skills_save_profile -> ok
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    invokeMock.mockReset();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(
        createElement(LibrarySearch, { projectRoot, profile: "coder" }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  function typeSearch(value: string) {
    const input = container.querySelector(
      "[data-testid='library-search']",
    ) as HTMLInputElement;
    const proto = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    );
    proto!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  it("lists the global library rows", async () => {
    await mount();
    expect(
      container.querySelector("[data-testid='library-row-plan-first']"),
    ).toBeTruthy();
    expect(
      container.querySelector("[data-testid='library-row-review-hard']"),
    ).toBeTruthy();
  });

  it("filters via fuzzy score on query", async () => {
    await mount();
    await act(async () => {
      typeSearch("review");
    });
    expect(
      container.querySelector("[data-testid='library-row-review-hard']"),
    ).toBeTruthy();
    expect(
      container.querySelector("[data-testid='library-row-plan-first']"),
    ).toBeFalsy();
  });

  it("Apply copies the skill content into the active profile and calls onApplied", async () => {
    const onApplied = vi.fn();
    await act(async () => {
      root.render(
        createElement(LibrarySearch, {
          projectRoot,
          profile: "mini-big",
          onApplied,
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });
    await act(async () => {
      container
        .querySelector("[data-testid='library-apply-plan-first']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "skills_save_profile",
    );
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({
      workingFolderPath: projectRoot,
      profile: "mini-big",
      content: "PLAN FIRST BODY",
    });
    expect(onApplied).toHaveBeenCalled();
  });
});
