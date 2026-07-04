// @vitest-environment jsdom
//
// Phase B3 TDD — the Work Console modal's per-PROFILE language-personas editor.
// Covers the profile-flavored language commands (skills_list/save/reset_lang_profile).

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

const invokeMock = vi.fn(async (..._args: unknown[]): Promise<unknown> => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { ModalLanguages } from "./ModalLanguages";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

describe("ModalLanguages (B3 — per-profile language personas)", () => {
  let container: HTMLDivElement;
  let root: Root;
  const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    invokeMock.mockImplementation(async (...args: unknown[]) => {
      const cmd = args[0] as string;
      if (cmd === "skills_list_langs_profile") {
        return [
          { role: "mini-big", lang: "rust", source: "project", content: "RUST OVR", bytes: 7, truncated: false },
          { role: "mini-big", lang: "python", source: "bundled", content: "PY DEF", bytes: 6, truncated: false },
        ];
      }
      return undefined;
    });
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
    invokeMock.mockClear();
    confirmSpy.mockClear();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(createElement(ModalLanguages, { projectRoot: "/proj", profile: "mini-big" }));
    });
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("calls skills_list_langs_profile with the project root + profile on mount", async () => {
    await mount();
    const call = invokeMock.mock.calls.find((c) => c[0] === "skills_list_langs_profile");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ workingFolderPath: "/proj", profile: "mini-big" });
  });

  it("shows Reset only for a project-source row", async () => {
    await mount();
    expect(document.querySelector("[data-testid='ml-reset-rust']")).toBeTruthy();
    expect(document.querySelector("[data-testid='ml-reset-python']")).toBeNull();
  });

  it("saves an edited language persona via skills_save_lang_profile", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='ml-edit-rust']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const ta = document.querySelector("[data-testid='ml-textarea-rust']") as HTMLTextAreaElement;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    await act(async () => {
      setter.call(ta, "RUST EDITED");
      ta.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      document
        .querySelector("[data-testid='ml-save-rust']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find((c) => c[0] === "skills_save_lang_profile");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({
      workingFolderPath: "/proj",
      profile: "mini-big",
      lang: "rust",
      content: "RUST EDITED",
    });
  });

  it("resets a project override via skills_reset_lang_profile (confirmed)", async () => {
    await mount();
    await act(async () => {
      document
        .querySelector("[data-testid='ml-reset-rust']")
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => {
      await Promise.resolve();
    });
    const call = invokeMock.mock.calls.find((c) => c[0] === "skills_reset_lang_profile");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ workingFolderPath: "/proj", profile: "mini-big", lang: "rust" });
  });
});
