// @vitest-environment jsdom
//
// P10(b) Step 3 — SkillsView. The view owns a native folder picker, lists per-role
// skills on pick/refresh, and saves/toggles/installs against the backend. This uses
// jsdom + createRoot + act (the repo's interactive-test pattern, mirroring
// MiniWriteBehaviorCard.test.tsx). We mock invokeBackendCommand to drive the five
// skills_* commands and the dialog plugin's `open` to return a chosen folder.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import type { CatalogEntry, SkillEntry } from "../../types/skills";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// --- mock state --------------------------------------------------------------
const FOLDER = "/tmp/project";

function makeEntries(overrides: Partial<Record<string, Partial<SkillEntry>>> = {}): SkillEntry[] {
  const base: SkillEntry[] = [
    { role: "mini", exists: true, enabled: true, content: "mini body", bytes: 9, truncated: false },
    { role: "coder", exists: true, enabled: false, content: "coder body", bytes: 10, truncated: false },
    { role: "design", exists: false, enabled: true, content: "", bytes: 0, truncated: false },
  ];
  return base.map((e) => ({ ...e, ...(overrides[e.role] ?? {}) }));
}

const CATALOG: CatalogEntry[] = [
  {
    id: "starter-mini",
    name: "Mini executor — edit discipline",
    role: "mini",
    description: "Stay in scope, emit clean edits.",
    sourceUrl: null,
    body: "# mini template",
  },
  {
    id: "starter-coder",
    name: "Coder agent — delivery discipline",
    role: "coder",
    description: "Delegate mechanical edits.",
    sourceUrl: null,
    body: "# coder template",
  },
];

let listEntries: SkillEntry[];
let listThrowsOnce = false;
let setEnabledThrowsOnce = false;
const calls: Array<{ name: string; args?: Record<string, unknown> }> = [];

const invokeMock = vi.fn(async (name: string, args?: Record<string, unknown>) => {
  calls.push({ name, args });
  if (name === "skills_catalog") return CATALOG;
  if (name === "skills_list") {
    if (listThrowsOnce) {
      listThrowsOnce = false;
      throw new Error("list failed");
    }
    return listEntries;
  }
  if (name === "skills_set_enabled") {
    if (setEnabledThrowsOnce) {
      setEnabledThrowsOnce = false;
      throw new Error(
        "skills-state.json exists but is unreadable or corrupt; fix or delete it before changing a skill toggle",
      );
    }
    return null;
  }
  if (name === "skills_save") return null;
  if (name === "skills_install_from_catalog") return null;
  return null;
});

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (name: string, args?: Record<string, unknown>) =>
    invokeMock(name, args),
}));

// Mock the dialog plugin's dynamic import so pickFolder resolves to FOLDER.
let dialogReturns: string | null = FOLDER;
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogReturns),
}));

import { SkillsView } from "./SkillsView";

let container: HTMLDivElement;
let root: Root;

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(createElement(SkillsView));
  });
  await flush();
}

// Click the "Choose project folder" button and let the list resolve.
async function chooseFolder() {
  const btn = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.includes("Choose project folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    btn.click();
    await Promise.resolve();
  });
  await flush();
}

function roleSwitches(): HTMLButtonElement[] {
  return Array.from(
    container.querySelectorAll('[role="switch"]'),
  ) as HTMLButtonElement[];
}

// Drive a React-controlled <textarea>: set the value via the native prototype
// setter, then dispatch a real InputEvent (React 18 listens for `input` on
// textareas), all inside act(). Centralised so the brittle bits live in ONE
// place if React/jsdom change. Mirrors the repo's pure-jsdom test pattern (no
// @testing-library/react in this project).
const NATIVE_TEXTAREA_VALUE_SETTER = Object.getOwnPropertyDescriptor(
  window.HTMLTextAreaElement.prototype,
  "value",
)!.set!;

async function editTextarea(el: HTMLTextAreaElement, value: string) {
  await act(async () => {
    NATIVE_TEXTAREA_VALUE_SETTER.call(el, value);
    el.dispatchEvent(new InputEvent("input", { bubbles: true }));
    await Promise.resolve();
  });
}

function saveButton(): HTMLButtonElement {
  return Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent === "Save",
  ) as HTMLButtonElement;
}

beforeEach(() => {
  listEntries = makeEntries();
  listThrowsOnce = false;
  setEnabledThrowsOnce = false;
  dialogReturns = FOLDER;
  calls.length = 0;
  invokeMock.mockClear();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("SkillsView", () => {
  it("shows the empty-state prompt and NO role cards before a folder is chosen", async () => {
    await mount();
    expect(container.innerHTML).toContain("Choose a project folder to manage its skills");
    // No role toggles render until a folder is chosen.
    expect(roleSwitches().length).toBe(0);
  });

  it("renders three role cards from skills_list after choosing a folder", async () => {
    await mount();
    await chooseFolder();
    expect(roleSwitches().length).toBe(3);
    const html = container.innerHTML;
    expect(html).toContain("Mini");
    expect(html).toContain("Coder");
    expect(html).toContain("Design");
    // The status lines reflect the mocked entries.
    expect(html).toContain("active"); // mini exists+enabled
    expect(html).toContain("disabled"); // coder exists+disabled
    expect(html).toContain("no skill yet"); // design absent
    // skills_list was called with the picked folder.
    const listCall = calls.find((c) => c.name === "skills_list");
    expect(listCall?.args).toEqual({ workingFolderPath: FOLDER });
  });

  it("toggles a role via skills_set_enabled with the right args and re-lists", async () => {
    await mount();
    await chooseFolder();
    calls.length = 0;
    // mini is the first card (enabled) -> clicking should set enabled=false.
    const miniSwitch = roleSwitches()[0];
    await act(async () => {
      miniSwitch.click();
      await Promise.resolve();
    });
    await flush();
    const setCall = calls.find((c) => c.name === "skills_set_enabled");
    expect(setCall?.args).toEqual({
      workingFolderPath: FOLDER,
      role: "mini",
      enabled: false,
    });
    // It re-lists after the toggle.
    expect(calls.some((c) => c.name === "skills_list")).toBe(true);
  });

  it("saves the draft via skills_save with the current content", async () => {
    await mount();
    await chooseFolder();
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await editTextarea(textarea, "edited mini body");
    calls.length = 0;
    await act(async () => {
      saveButton().click();
      await Promise.resolve();
    });
    await flush();
    const saveCall = calls.find((c) => c.name === "skills_save");
    // The EDITED text (not the seeded "mini body") reaches the backend.
    expect(saveCall?.args).toEqual({
      workingFolderPath: FOLDER,
      role: "mini",
      content: "edited mini body",
    });
  });

  it("drops a second mutation fired before the first settles (synchronous busy lock)", async () => {
    await mount();
    await chooseFolder();
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await editTextarea(textarea, "edited mini body");
    calls.length = 0;
    // Two Save clicks in the SAME tick, before `busy` state propagates: the
    // synchronous busyRef gate must drop the second so skills_save fires once.
    await act(async () => {
      const btn = saveButton();
      btn.click();
      btn.click();
      await Promise.resolve();
    });
    await flush();
    const saveCalls = calls.filter((c) => c.name === "skills_save");
    expect(saveCalls.length).toBe(1);
  });

  it("disables Save when the draft byte length exceeds 8192", async () => {
    await mount();
    await chooseFolder();
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    await editTextarea(textarea, "a".repeat(8193));
    expect(saveButton().disabled).toBe(true);
    expect(container.innerHTML).toContain("trim to 8192 bytes before saving");
  });

  it("counts BYTES not chars (multi-byte chars push over the cap)", async () => {
    await mount();
    await chooseFolder();
    const textarea = container.querySelector("textarea") as HTMLTextAreaElement;
    // 3000 "€" = 3000 chars but 9000 bytes (3 bytes each) -> over cap.
    await editTextarea(textarea, "€".repeat(3000));
    expect(saveButton().disabled).toBe(true);
    expect(container.innerHTML).toContain("9000 / 8192 bytes");
  });

  it("gates Save behind the explicit acknowledgement when truncated, and shows the warning", async () => {
    listEntries = makeEntries({
      mini: { truncated: true, content: "a".repeat(8192), bytes: 8192 },
    });
    await mount();
    await chooseFolder();
    expect(container.innerHTML).toContain(
      "Saving will permanently discard everything past",
    );
    // The mini card's Save is disabled until the checkbox is ticked.
    expect(saveButton().disabled).toBe(true);
    const checkbox = container.querySelector(
      'input[type="checkbox"]',
    ) as HTMLInputElement;
    expect(checkbox).not.toBeNull();
    await act(async () => {
      checkbox.click();
      await Promise.resolve();
    });
    expect(saveButton().disabled).toBe(false);
  });

  it("installs a template via skills_install_from_catalog (confirming overwrite when the skill exists)", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    await mount();
    await chooseFolder();
    calls.length = 0;
    // The mini card exists -> install must confirm before overwriting.
    const installBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Mini executor — edit discipline"),
    ) as HTMLButtonElement;
    expect(installBtn).toBeDefined();
    await act(async () => {
      installBtn.click();
      await Promise.resolve();
    });
    await flush();
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    const installCall = calls.find((c) => c.name === "skills_install_from_catalog");
    expect(installCall?.args).toEqual({
      workingFolderPath: FOLDER,
      role: "mini",
      catalogId: "starter-mini",
    });
    confirmSpy.mockRestore();
  });

  it("does NOT install when the overwrite confirm is declined", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    await mount();
    await chooseFolder();
    calls.length = 0;
    const installBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Mini executor — edit discipline"),
    ) as HTMLButtonElement;
    await act(async () => {
      installBtn.click();
      await Promise.resolve();
    });
    await flush();
    expect(confirmSpy).toHaveBeenCalledTimes(1);
    expect(calls.some((c) => c.name === "skills_install_from_catalog")).toBe(false);
    confirmSpy.mockRestore();
  });

  it("preserves an unsaved draft in one role when another role is toggled (re-list)", async () => {
    await mount();
    await chooseFolder();
    // Edit the coder draft (2nd textarea) but do NOT save it.
    const textareas = Array.from(
      container.querySelectorAll("textarea"),
    ) as HTMLTextAreaElement[];
    const coderArea = textareas[1];
    await editTextarea(coderArea, "UNSAVED coder edit");
    // Toggle the mini skill — this triggers a re-list. The coder draft must survive.
    const miniSwitch = roleSwitches()[0];
    await act(async () => {
      miniSwitch.click();
      await Promise.resolve();
    });
    await flush();
    const coderAreaAfter = (
      Array.from(container.querySelectorAll("textarea")) as HTMLTextAreaElement[]
    )[1];
    expect(coderAreaAfter.value).toBe("UNSAVED coder edit");
  });

  it("force-reseeds the just-saved role from the backend re-list while preserving another role's unsaved edit", async () => {
    await mount();
    await chooseFolder();
    // Diverge BOTH the mini (1st) and coder (2nd) textareas from their loaded
    // content. Only mini gets saved; coder's edit must survive the post-save
    // re-list (it diverged and was not the forceReseed role).
    const textareas = Array.from(
      container.querySelectorAll("textarea"),
    ) as HTMLTextAreaElement[];
    const miniArea = textareas[0];
    const coderArea = textareas[1];
    await editTextarea(miniArea, "edited mini draft");
    await editTextarea(coderArea, "UNSAVED coder edit");
    // The post-save re-list returns NEW mini content (the backend normalised the
    // saved body); the just-saved mini role must force-reseed to THIS value, not
    // keep the pre-save "edited mini draft".
    listEntries = makeEntries({
      mini: { content: "normalised mini body", bytes: 20 },
    });
    calls.length = 0;
    await act(async () => {
      // saveButton() returns the FIRST Save button — the mini card's.
      saveButton().click();
      await Promise.resolve();
    });
    await flush();
    // skills_save fired with the edited mini draft, then a re-list followed.
    const saveCall = calls.find((c) => c.name === "skills_save");
    expect(saveCall?.args).toEqual({
      workingFolderPath: FOLDER,
      role: "mini",
      content: "edited mini draft",
    });
    expect(calls.some((c) => c.name === "skills_list")).toBe(true);
    const after = Array.from(
      container.querySelectorAll("textarea"),
    ) as HTMLTextAreaElement[];
    // (1) mini force-reseeded to the new backend content (NOT the pre-save draft).
    expect(after[0].value).toBe("normalised mini body");
    // (2) coder's unsaved edit preserved (diverged, not the saved role).
    expect(after[1].value).toBe("UNSAVED coder edit");
  });

  it("surfaces a backend error (corrupt skills-state) verbatim in the banner", async () => {
    await mount();
    await chooseFolder();
    setEnabledThrowsOnce = true;
    const miniSwitch = roleSwitches()[0];
    await act(async () => {
      miniSwitch.click();
      await Promise.resolve();
    });
    await flush();
    expect(container.innerHTML).toContain(
      "skills-state.json exists but is unreadable or corrupt",
    );
  });
});
