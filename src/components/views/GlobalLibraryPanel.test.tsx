// @vitest-environment jsdom
//
// Phase B2 TDD — the sidebar SkillsView's GLOBAL LIBRARY panel.
// The hybrid sidebar keeps the per-role RoleCards AND adds this name-keyed global
// library manager backed by the P4 global store (<app-data>/global-skills/), with the
// bundled catalog UNIONed in. Covers: mount fetch of global_skills_list +
// skills_library_catalog, a global-skill row with delete, inline edit/save, the new-skill
// creator, the bundled "Add to my library" install (and dedup of already-installed
// bundled skills), and the fuzzy search filter.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { GlobalLibraryPanel } from "./GlobalLibraryPanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

describe("GlobalLibraryPanel (B2 — sidebar global library)", () => {
  let root: Root;
  let container: HTMLDivElement;
  const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

  beforeEach(() => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "global_skills_list") {
        return [
          { name: "my-skill", content: "MY GLOBAL BODY", bytes: 13, truncated: false },
          { name: "code-review", content: "INSTALLED REVIEW BODY", bytes: 21, truncated: false },
        ];
      }
      if (cmd === "skills_library_catalog") {
        return [
          { name: "code-review", description: "Review a PR" }, // already in global → must dedup
          { name: "debugging", description: "Debug systematically" }, // not installed → "Add"
        ];
      }
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
    confirmSpy.mockClear();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(createElement(GlobalLibraryPanel));
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("fetches the global store and the bundled catalog on mount", async () => {
    await mount();
    expect(invokeMock.mock.calls.some((c) => c[0] === "global_skills_list")).toBe(true);
    expect(invokeMock.mock.calls.some((c) => c[0] === "skills_library_catalog")).toBe(true);
  });

  it("renders a row for each global skill", async () => {
    await mount();
    expect(document.querySelector("[data-testid='global-skill-row-my-skill']")).toBeTruthy();
    expect(document.querySelector("[data-testid='global-skill-row-code-review']")).toBeTruthy();
  });

  it("deletes a global skill (confirmed) via global_skills_delete", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-delete-my-skill']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find((c) => c[0] === "global_skills_delete");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ name: "my-skill" });
  });

  it("shows 'Add to my library' for a bundled skill NOT yet in the global store", async () => {
    await mount();
    // debugging is not in the global store → add button present
    expect(document.querySelector("[data-testid='bundled-skill-add-debugging']")).toBeTruthy();
    // code-review IS already in the global store → no add button (dedup)
    expect(document.querySelector("[data-testid='bundled-skill-add-code-review']")).toBeNull();
  });

  it("installs a bundled skill via global_skills_install_bundled", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='bundled-skill-add-debugging']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find((c) => c[0] === "global_skills_install_bundled");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ skillName: "debugging" });
  });

  it("creates a new global skill via global_skills_save", async () => {
    await mount();
    const nameInput = document.querySelector(
      "[data-testid='global-library-new-name']",
    ) as HTMLInputElement;
    const contentInput = document.querySelector(
      "[data-testid='global-library-new-content']",
    ) as HTMLTextAreaElement;
    const setVal = (el: HTMLInputElement | HTMLTextAreaElement, v: string) => {
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(proto, "value")!.set!;
      setter.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    await act(async () => {
      setVal(nameInput, "fresh-skill");
      setVal(contentInput, "FRESH BODY");
    });
    await act(async () => {
      document
        .querySelector("[data-testid='global-library-new-save']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find((c) => c[0] === "global_skills_save");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ name: "fresh-skill", content: "FRESH BODY" });
  });

  it("filters the global list with the search box", async () => {
    await mount();
    const search = document.querySelector(
      "[data-testid='global-library-search']",
    ) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    await act(async () => {
      setter.call(search, "my-skill");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(document.querySelector("[data-testid='global-skill-row-my-skill']")).toBeTruthy();
    expect(document.querySelector("[data-testid='global-skill-row-code-review']")).toBeNull();
  });

  // --- Reviewer-driven hardening ---

  it("fires delete only once on a rapid double-click (synchronous busy guard)", async () => {
    await mount();
    await act(async () => {
      const b = document.querySelector("[data-testid='global-skill-delete-my-skill']");
      b?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      b?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const calls = invokeMock.mock.calls.filter((c) => c[0] === "global_skills_delete");
    expect(calls.length).toBe(1);
  });

  it("blocks saving a truncated global skill until acknowledged", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "global_skills_list") {
        return [{ name: "big-skill", content: "HEAD", bytes: 8192, truncated: true }];
      }
      if (cmd === "skills_library_catalog") return [];
      return undefined;
    });
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-edit-big-skill']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const save = document.querySelector(
      "[data-testid='global-skill-save-big-skill']",
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    const ack = document.querySelector(
      "[data-testid='global-skill-ack-big-skill']",
    ) as HTMLInputElement;
    expect(ack).toBeTruthy();
    // A real click toggles `checked` AND triggers React's onChange (React maps checkbox
    // change onto the click event); a bare synthetic "change" would not fire it.
    await act(async () => {
      ack.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(
      (document.querySelector(
        "[data-testid='global-skill-save-big-skill']",
      ) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("keeps edit drafts independent per row", async () => {
    await mount();
    const setVal = (el: HTMLTextAreaElement, v: string) => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )!.set!;
      setter.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    // Open my-skill, type a draft.
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-edit-my-skill']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      setVal(
        document.querySelector(
          "[data-testid='global-skill-textarea-my-skill']",
        ) as HTMLTextAreaElement,
        "EDITED A",
      );
    });
    // Switch to code-review's editor, then back to my-skill.
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-edit-code-review']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-edit-my-skill']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const reopened = document.querySelector(
      "[data-testid='global-skill-textarea-my-skill']",
    ) as HTMLTextAreaElement;
    expect(reopened.value).toBe("EDITED A"); // not clobbered by opening code-review
  });

  it("associates the truncation-ack checkbox with its label (a11y)", async () => {
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "global_skills_list") {
        return [{ name: "big-skill", content: "HEAD", bytes: 8192, truncated: true }];
      }
      if (cmd === "skills_library_catalog") return [];
      return undefined;
    });
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='global-skill-edit-big-skill']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const ack = document.querySelector(
      "[data-testid='global-skill-ack-big-skill']",
    ) as HTMLInputElement;
    expect(ack.id).toBeTruthy();
    const label = document.querySelector(`label[for='${ack.id}']`);
    expect(label).toBeTruthy();
  });

  it("does not create a skill whose content is whitespace-only", async () => {
    await mount();
    const setVal = (el: HTMLInputElement | HTMLTextAreaElement, v: string) => {
      const proto =
        el instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    };
    await act(async () => {
      setVal(
        document.querySelector("[data-testid='global-library-new-name']") as HTMLInputElement,
        "ok-name",
      );
      setVal(
        document.querySelector(
          "[data-testid='global-library-new-content']",
        ) as HTMLTextAreaElement,
        "   ",
      );
    });
    const save = document.querySelector(
      "[data-testid='global-library-new-save']",
    ) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    await act(async () => {
      save.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(invokeMock.mock.calls.some((c) => c[0] === "global_skills_save")).toBe(false);
  });
});
