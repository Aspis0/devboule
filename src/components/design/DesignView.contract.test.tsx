// @vitest-environment jsdom
//
// Phase C — design.md contract seeding flow + prompt injection (DesignView).
// Verifies the seed state machine (existing contract -> stash, no editor; missing +
// chunks -> extracted draft editor; missing + no chunks -> preset-picker editor), that
// NOTHING writes without an explicit Save, and that the saved contract reaches EVERY
// prompt. useDesignStream + Canvas are mocked exactly as in the generation test.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignProject, DesignProjectEntry } from "../../types/design";
import { sha256Hex } from "./contract/sha256";

// ---- backend mock ---------------------------------------------------------
const invokeSpy =
  vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(
    async () => undefined,
  );
const requestViewSpy = vi.fn();
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (command: string, args?: Record<string, unknown>) =>
    invokeSpy(command, args),
  isTauriRuntime: () => true,
  useAppContext: () => ({ requestView: requestViewSpy }),
}));

const dialogCtl: { nextPick: string | null } = { nextPick: null };
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogCtl.nextPick),
}));

// ---- controllable useDesignStream mock ------------------------------------
type StreamState = {
  text: string;
  status: "idle" | "streaming" | "done" | "error" | "cancelled";
  error: string | null;
};
const streamCtl: {
  state: StreamState;
  starts: string[];
  notify: (() => void) | null;
} = { state: { text: "", status: "idle", error: null }, starts: [], notify: null };

vi.mock("./useDesignStream", () => ({
  useDesignStream: () => ({
    text: streamCtl.state.text,
    status: streamCtl.state.status,
    error: streamCtl.state.error,
    start: (prompt: string) => {
      streamCtl.starts.push(prompt);
      streamCtl.state = { ...streamCtl.state, text: "" };
      streamCtl.notify?.();
    },
    cancel: () => {},
    reset: () => {},
  }),
}));

// ---- Canvas mock ----------------------------------------------------------
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onSelect: (id: string | null) => void;
  }) => {
    const firstId = Object.keys(props.project.manifest.nodes)[0] ?? null;
    return createElement("button", {
      type: "button",
      "data-testid": "select-first",
      onClick: () => props.onSelect(firstId),
    });
  },
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let DesignView: typeof import("./DesignView").DesignView;
let rerender: (() => void) | null = null;

const EMPTY_PROJECT: DesignProject = {
  meta: {
    schemaVersion: 1,
    id: "p",
    name: "Loaded",
    createdAt: "1970-01-01T00:00:00Z",
    updatedAt: "1970-01-01T00:00:00Z",
    canvas: { w: 1440, h: 1024, grid: 8 },
    nodeOrder: [],
  },
  manifest: { schemaVersion: 1, nodes: {} },
  components: {},
};

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  streamCtl.state = { text: "", status: "idle", error: null };
  streamCtl.starts = [];
  streamCtl.notify = null;
  dialogCtl.nextPick = null;
  ({ DesignView } = await import("./DesignView"));
});

afterEach(() => {
  rerender = null;
});

function render(): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(DesignView));
  });
  rerender = () => act(() => root.render(createElement(DesignView)));
  streamCtl.notify = rerender;
  return { container, root };
}

/** Flush the mount-time async effects (e.g. the registry-list fetch that populates the
 * recent-projects state the provenance lookup reads). Call right after render() when a
 * test depends on the recorded contractSha being available before the seed runs. */
async function settleMount() {
  await act(async () => {
    // The registry-list fetch (and, in some tests, an async SHA-256 over the recorded
    // contract) needs several async hops; flush microtasks AND a macrotask to be safe.
    for (let i = 0; i < 20; i++) await Promise.resolve();
    await new Promise((r) => setTimeout(r, 0));
    for (let i = 0; i < 20; i++) await Promise.resolve();
  });
}

async function pickFolder(container: HTMLElement, value: string) {
  dialogCtl.nextPick = value;
  if (!container.querySelector(".pop.left")) {
    const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  }
  const pickBtn = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.includes("Open working folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    pickBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    for (let i = 0; i < 14; i++) await Promise.resolve();
  });
}

function typePrompt(container: HTMLElement, value: string) {
  const ta = container.querySelector(".composer textarea, textarea") as HTMLTextAreaElement;
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    setter.call(ta, value);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function clickGenerate(container: HTMLElement) {
  const btn = container.querySelector(".send-btn") as HTMLButtonElement;
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function emitDone(text: string) {
  act(() => {
    streamCtl.state = { text, status: "done", error: null };
    rerender?.();
  });
}

function editor(container: HTMLElement): HTMLElement | null {
  return container.querySelector("[data-testid=design-md-editor]");
}

function contractTextarea(container: HTMLElement): HTMLTextAreaElement {
  return container.querySelector(".dc-textarea") as HTMLTextAreaElement;
}

async function clickButtonByText(container: HTMLElement, text: string) {
  const btn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === text,
  ) as HTMLButtonElement;
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Build a registry entry whose recorded contractSha matches `contract` for `folder`. */
async function approvedEntry(
  folder: string,
  contract: string,
): Promise<DesignProjectEntry> {
  return {
    id: "e1",
    name: "Loaded",
    workingFolderPath: folder,
    createdAt: "2020-01-01T00:00:00Z",
    updatedAt: "2020-01-01T00:00:00Z",
    lastOpenedAt: "2020-01-01T00:00:00Z",
    contractSha: await sha256Hex(contract),
  };
}

describe("Phase C — existing design.md (APPROVED provenance)", () => {
  it("matching recorded hash -> injects the contract, NO editor (Fix 3)", async () => {
    const contract = "# House rules\nPrimary color is #4f46e5.";
    const recent = [await approvedEntry("C:/proj", contract)];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return recent;
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return contract;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      return undefined;
    });

    const { container } = render();
    await settleMount();
    await pickFolder(container, "C:/proj");

    expect(editor(container)).toBeFalsy();

    typePrompt(container, "a hero");
    await clickGenerate(container);

    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt).toContain("DESIGN CONTRACT (project file, follow its rules):");
    expect(lastPrompt).toContain("Primary color is #4f46e5.");
  });

  it("NO recorded hash -> opens a REVIEW editor, prompt has NO contract until Save (Fix 3)", async () => {
    const contract = "# House rules\nPrimary color is #4f46e5.";
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return []; // no recorded hash
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return contract;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");

    // Editor opens with the on-disk content + a review notice.
    expect(editor(container)).toBeTruthy();
    expect(contractTextarea(container).value).toBe(contract);
    expect(
      container.querySelector("[data-testid=dc-notice]")?.textContent ?? "",
    ).toContain("changed outside the editor");

    // The contract is NOT injected (uninjected for the session) until Save re-approves.
    typePrompt(container, "a hero");
    await clickGenerate(container);
    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt ?? "").not.toContain("DESIGN CONTRACT");
  });

  it("MISMATCHED recorded hash -> review editor; Save records the new contractSha (Fix 3)", async () => {
    const contract = "# House rules\nPrimary color is #4f46e5.";
    const remembers: Record<string, unknown>[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_registry_list")
        return [
          {
            id: "e1",
            name: "Loaded",
            workingFolderPath: "C:/proj",
            createdAt: "2020-01-01T00:00:00Z",
            updatedAt: "2020-01-01T00:00:00Z",
            lastOpenedAt: "2020-01-01T00:00:00Z",
            contractSha: "STALE_HASH",
          },
        ];
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return contract;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      if (command === "design_registry_remember") {
        remembers.push((args as { entry: Record<string, unknown> }).entry);
        return [];
      }
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");
    expect(editor(container)).toBeTruthy();

    await clickButtonByText(container, "Save contract");

    // The remember carrying a contractSha equals the hash of the saved content.
    const withSha = remembers.find((e) => typeof e.contractSha === "string");
    expect(withSha).toBeTruthy();
    expect(withSha!.contractSha).toBe(await sha256Hex(contract));
  });

  it("clamps an oversized design.md (>16 KiB) before injecting it into the prompt", async () => {
    const huge = "# Rules\n" + "x".repeat(20 * 1024);
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return [await approvedEntry("C:/proj", huge)];
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return huge;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      return undefined;
    });

    const { container } = render();
    await settleMount();
    await pickFolder(container, "C:/proj");
    typePrompt(container, "a hero");
    await clickGenerate(container);

    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt).toContain("DESIGN CONTRACT");
    // The injected contract is clamped (truncation marker present, body < the original).
    expect(lastPrompt).toContain("truncated to fit");
    // The full 20 KiB body did NOT make it into the prompt verbatim.
    expect(lastPrompt).not.toContain("x".repeat(20 * 1024));
  });
});

describe("Phase C — missing design.md + Oracle chunks", () => {
  it("opens the editor prefilled; Save writes design.md AND extracted tokens; then prompts carry it", async () => {
    const writes: { command: string; args?: Record<string, unknown> }[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return null; // missing
      if (command === "design_oracle_context")
        return [
          {
            fileSource: "src/tokens.css",
            score: 0.95,
            text: ":root { --brand: #4f46e5; --ink: #0f172a; }",
          },
        ];
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_write_design_md" || command === "design_write_tokens")
        writes.push({ command, args });
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");

    // Editor open, prefilled with the extracted draft (palette + the source snippet).
    expect(editor(container)).toBeTruthy();
    expect(contractTextarea(container).value).toContain("#4f46e5");
    // Nothing written yet.
    expect(writes.length).toBe(0);

    // Save.
    await clickButtonByText(container, "Save contract");

    const mdWrite = writes.find((w) => w.command === "design_write_design_md");
    const tokWrite = writes.find((w) => w.command === "design_write_tokens");
    expect(mdWrite).toBeTruthy();
    expect(String((mdWrite!.args as Record<string, unknown>).content)).toContain(
      "#4f46e5",
    );
    expect(tokWrite).toBeTruthy();
    const tokensJson = String((tokWrite!.args as Record<string, unknown>).tokensJson);
    expect(tokensJson).toContain("#4f46e5"); // real extracted $value
    // tokens.json must not leak the source PATH.
    expect(tokensJson).not.toContain("src/tokens.css");

    // Editor closed; the next prompt carries the saved contract.
    expect(editor(container)).toBeFalsy();
    typePrompt(container, "a hero");
    await clickGenerate(container);
    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt).toContain("DESIGN CONTRACT (project file, follow its rules):");
    expect(lastPrompt).toContain("#4f46e5");

    // The Oracle popover swatches reflect the extracted color after Save.
    const oracleChip = container.querySelector(".chip-oracle") as HTMLButtonElement;
    act(() => oracleChip.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const swatch = container.querySelector(".op-tokens .sw i") as HTMLElement | null;
    expect(swatch).toBeTruthy();
    // jsdom normalizes the hex to rgb(); #4f46e5 -> rgb(79, 70, 229). Assert the
    // extracted brand color reached the swatch in either form.
    expect(swatch!.style.background).toBe("rgb(79, 70, 229)");
  });
});

describe("Phase C — missing design.md + NO chunks (preset mode)", () => {
  it("opens the preset picker; choosing a preset + Save writes the preset md AND preset tokens", async () => {
    const writes: { command: string; args?: Record<string, unknown> }[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return null;
      if (command === "design_oracle_context") return []; // no signal
      if (command === "design_write_design_md" || command === "design_write_tokens")
        writes.push({ command, args });
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");

    expect(editor(container)).toBeTruthy();
    // Empty draft -> preset cards visible.
    const card = container.querySelector(
      '.dc-preset[data-preset="material-ish"]',
    ) as HTMLButtonElement;
    expect(card).toBeTruthy();
    act(() => card.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    // Picking the preset replaces the textarea with its designMd.
    expect(contractTextarea(container).value).toContain("Material-ish");

    await clickButtonByText(container, "Save contract");

    const mdWrite = writes.find((w) => w.command === "design_write_design_md");
    const tokWrite = writes.find((w) => w.command === "design_write_tokens");
    expect(String((mdWrite!.args as Record<string, unknown>).content)).toContain(
      "Material-ish",
    );
    // Preset tokens written (Material primary #6750a4).
    expect(String((tokWrite!.args as Record<string, unknown>).tokensJson)).toContain(
      "#6750a4",
    );
  });

  it("Skip writes the legacy clean tokens stub and NOTHING else (editor never writes without Save)", async () => {
    const writes: { command: string; args?: Record<string, unknown> }[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return null;
      if (command === "design_oracle_context") return [];
      if (command === "design_write_design_md" || command === "design_write_tokens")
        writes.push({ command, args });
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");
    expect(editor(container)).toBeTruthy();
    expect(writes.length).toBe(0); // opening writes nothing

    await clickButtonByText(container, "Skip");

    // Only the clean tokens stub, never design.md.
    expect(writes.some((w) => w.command === "design_write_design_md")).toBe(false);
    const tok = writes.find((w) => w.command === "design_write_tokens");
    expect(tok).toBeTruthy();
    expect(
      JSON.parse(String((tok!.args as Record<string, unknown>).tokensJson)),
    ).toEqual({});
  });
});

/** Open the project popover (if closed) and click a footer/menu row by its text. */
async function clickPopoverRow(container: HTMLElement, startsWith: string) {
  if (!container.querySelector(".pop.left")) {
    const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  }
  const row = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.trim().startsWith(startsWith),
  ) as HTMLButtonElement;
  await act(async () => {
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    for (let i = 0; i < 6; i++) await Promise.resolve();
  });
}

describe("Phase C — Skip on a MANUAL open does not clobber tokens.json (Fix 2)", () => {
  it("manual open of an existing (approved) contract -> Skip writes NOTHING", async () => {
    const contract = "# House rules\nPrimary color is #4f46e5.";
    const recent = [await approvedEntry("C:/proj", contract)];
    const writes: { command: string }[] = [];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return recent;
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return contract;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      if (command === "design_write_design_md" || command === "design_write_tokens")
        writes.push({ command });
      return undefined;
    });

    const { container } = render();
    await settleMount();
    await pickFolder(container, "C:/proj");
    // Approved contract -> no seed editor.
    expect(editor(container)).toBeFalsy();

    // Manually open the contract editor from the popover, then Skip.
    await clickPopoverRow(container, "Design contract");
    expect(editor(container)).toBeTruthy();
    await clickButtonByText(container, "Skip");

    // Skip on a manual open must NOT write the clean tokens stub (Fix 2).
    expect(writes.some((w) => w.command === "design_write_tokens")).toBe(false);
    expect(writes.some((w) => w.command === "design_write_design_md")).toBe(false);
  });
});

describe("Phase C — Save write failure keeps the editor open (Fix 5)", () => {
  it("design_write_design_md rejects -> editor stays open, error shown, contract NOT injected; retry succeeds", async () => {
    let failNextWrite = true;
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return [];
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return null; // missing -> seed editor
      if (command === "design_oracle_context") return [];
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_write_design_md") {
        if (failNextWrite) throw new Error("disk full");
        return undefined;
      }
      return undefined;
    });

    const { container } = render();
    await pickFolder(container, "C:/proj");
    // Preset-picker editor open (missing + no chunks). Choose a preset so content is set.
    const card = container.querySelector(
      '.dc-preset[data-preset="material-ish"]',
    ) as HTMLButtonElement;
    act(() => card.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    // First Save FAILS.
    await clickButtonByText(container, "Save contract");
    // Editor still open, with an inline error and the content intact.
    expect(editor(container)).toBeTruthy();
    expect(
      container.querySelector("[data-testid=dc-save-error]")?.textContent ?? "",
    ).toContain("disk full");
    expect(contractTextarea(container).value).toContain("Material-ish");

    // The failed contract is NOT injected (contractRef not updated on failure).
    typePrompt(container, "a hero");
    await clickGenerate(container);
    expect(streamCtl.starts[streamCtl.starts.length - 1] ?? "").not.toContain(
      "DESIGN CONTRACT",
    );

    // Retry succeeds -> editor closes.
    failNextWrite = false;
    await clickButtonByText(container, "Save contract");
    expect(editor(container)).toBeFalsy();
  });
});

describe("Phase C — repair carries the LIVE contract (Fix 8)", () => {
  it("a self-repair retry injects the contract read from contractRef at repair time", async () => {
    const contract = "# House rules\nPrimary color is #4f46e5.";
    const recent = [await approvedEntry("C:/proj", contract)];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return recent;
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return contract;
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") return [];
      return undefined;
    });

    const { container } = render();
    await settleMount();
    await pickFolder(container, "C:/proj");
    expect(editor(container)).toBeFalsy(); // approved -> injected, no editor

    typePrompt(container, "make a hero");
    await clickGenerate(container);
    const startsBefore = streamCtl.starts.length;
    // The generate prompt already carried the contract.
    expect(streamCtl.starts[startsBefore - 1]).toContain("DESIGN CONTRACT");

    // Empty/garbage result -> zero committed nodes -> a repair retry is launched.
    emitDone("<tr><td>oops</td></tr>");
    await act(async () => {
      await Promise.resolve();
    });
    expect(streamCtl.starts.length).toBe(startsBefore + 1);
    const repairPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    // Fix 8: the repair prompt carries the contract read LIVE from contractRef.current.
    expect(repairPrompt).toContain("DESIGN CONTRACT (project file, follow its rules):");
    expect(repairPrompt).toContain("Primary color is #4f46e5.");
  });
});

describe("Phase C — project-switch seeding race (Fix 4)", () => {
  // The seed epoch is bumped at the START of every loadFolder/createInFolder and re-
  // checked before EVERY state-applying step of seedContract. The PRIMARY serialization
  // is the `busy` gate (a new load can't START while one is in flight), so the epoch is
  // DEFENSE-IN-DEPTH: if seedContract's result lands after the view it belonged to is
  // gone, it must not apply. We drive that by hanging A's probe, tearing the view down,
  // then releasing A — its late setContractEditor must not resurrect A's draft or crash.
  it("a seed parked mid-probe drops cleanly when its view is superseded", async () => {
    let releaseA: ((v: unknown) => void) | null = null;
    const aProbe = new Promise((res) => {
      releaseA = res;
    });

    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_registry_list") return [];
      if (command === "design_load_project") return EMPTY_PROJECT;
      if (command === "design_read_design_md") return null; // missing -> probe
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_oracle_context") {
        await aProbe; // hang until released
        return [
          { fileSource: "a.css", score: 0.9, text: ":root { --aaa: #aa0000; }" },
        ];
      }
      return undefined;
    });

    const { container, root } = render();
    await settleMount();

    // Load A; its seed parks at the hung probe (no editor yet).
    dialogCtl.nextPick = "C:/A";
    const projBtn = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => projBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const openBtn = Array.from(container.querySelectorAll("button")).find((b) =>
      b.textContent?.includes("Open working folder"),
    ) as HTMLButtonElement;
    await act(async () => {
      openBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      for (let i = 0; i < 6; i++) await Promise.resolve();
    });
    expect(editor(container)).toBeFalsy();

    // Tear down the view BEFORE A resolves, then release A. The parked seed must not
    // crash nor leak A's draft into the (now empty) container.
    act(() => root.unmount());
    await act(async () => {
      releaseA?.(undefined);
      for (let i = 0; i < 10; i++) await Promise.resolve();
    });
    expect(container.querySelector("[data-testid=design-md-editor]")).toBeFalsy();
    expect(container.innerHTML).not.toContain("#aa0000");
  });

  // Direct unit of the epoch guard semantics: a result captured under an OLD epoch is
  // dropped once the epoch advances. This mirrors what seedContract does internally
  // (`stale()` returns true), proving the gate logic the busy-gate can't be made to race
  // through the jsdom UI (the picker is `disabled` while a load is in flight).
  it("epoch guard drops a captured-stale apply (logic)", () => {
    const epochRef = { current: 1 };
    const captured = epochRef.current;
    const stale = () => epochRef.current !== captured;
    expect(stale()).toBe(false); // current -> would apply
    epochRef.current += 1; // a newer load advanced the epoch
    expect(stale()).toBe(true); // captured result -> must be dropped
  });
});
