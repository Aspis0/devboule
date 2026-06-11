// @vitest-environment jsdom
//
// Regression tests for the Phase-2 step-3 generation pipeline wiring in
// DesignView: the done-effect, structural-shape persistence across edit/load, and
// the serialized single-node disk write. `useDesignStream` is mocked with a
// controllable handle so a test can drive the terminal `done` transition with an
// exact accumulated text; Canvas is mocked so the test can read what reached it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { DesignProject } from "../../types/design";

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

// ---- native folder picker mock --------------------------------------------
// The working folder is now CHOSEN via the native OS directory dialog, never
// typed. A test sets `dialogCtl.nextPick` to the absolute path the dialog should
// return (or null to simulate a dismissed/cancelled dialog).
const dialogCtl: { nextPick: string | null } = { nextPick: null };
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => dialogCtl.nextPick),
}));

// ---- controllable useDesignStream mock ------------------------------------
// A tiny external controller drives status/text; `start` records the prompt and
// resets text to "" (mirroring the real hook's synchronous reset — the exact
// condition BLOCKER 2 guards against).
type StreamState = {
  text: string;
  status: "idle" | "streaming" | "done" | "error" | "cancelled";
  error: string | null;
};
const streamCtl: {
  state: StreamState;
  starts: string[];
  notify: (() => void) | null;
} = {
  state: { text: "", status: "idle", error: null },
  starts: [],
  notify: null,
};

vi.mock("./useDesignStream", () => ({
  useDesignStream: () => {
    return {
      text: streamCtl.state.text,
      status: streamCtl.state.status,
      error: streamCtl.state.error,
      start: (prompt: string) => {
        streamCtl.starts.push(prompt);
        // Mirror the real hook: synchronous text reset; status NOT yet streaming.
        streamCtl.state = { ...streamCtl.state, text: "" };
        streamCtl.notify?.();
      },
      cancel: () => {},
      reset: () => {},
    };
  },
}));

// ---- Canvas mock: expose the project + select-first-node + manifest-change --
let lastProject: DesignProject | null = null;
let lastOnManifestChange:
  | ((m: DesignProject["manifest"]) => void)
  | null = null;
vi.mock("./canvas/DesignCanvas", () => ({
  DesignCanvas: (props: {
    project: DesignProject;
    onSelect: (id: string | null) => void;
    onManifestChange: (m: DesignProject["manifest"]) => void;
  }) => {
    lastProject = props.project;
    lastOnManifestChange = props.onManifestChange;
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

beforeEach(async () => {
  invokeSpy.mockClear();
  invokeSpy.mockImplementation(async () => undefined);
  streamCtl.state = { text: "", status: "idle", error: null };
  streamCtl.starts = [];
  streamCtl.notify = null;
  lastProject = null;
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
  // The mock notifies on `start`; re-render the tree so the new stream state is read.
  rerender = () => act(() => root.render(createElement(DesignView)));
  streamCtl.notify = rerender;
  return { container, root };
}

/** Drive a terminal `done` with the given accumulated text, then flush effects. */
function emitDone(text: string) {
  act(() => {
    streamCtl.state = { text, status: "done", error: null };
    rerender?.();
  });
}

function typePrompt(container: HTMLElement, value: string) {
  const ta = container.querySelector("textarea") as HTMLTextAreaElement;
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    setter.call(ta, value);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function openProjectPopover(container: HTMLElement) {
  if (container.querySelector(".pop.left")) return;
  const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
  act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
}

/** Open the working folder via the ProjectPopover's "Open working folder…" row,
 *  which picks the folder itself (dialog mock returns `value`) and loads it. The
 *  folder is the ONLY way the working path is set now (no text input). NOTE: when a
 *  test mocks `design_load_project` to a project, this also LOADS it. */
async function pickFolder(container: HTMLElement, value: string) {
  dialogCtl.nextPick = value;
  openProjectPopover(container);
  const pickBtn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.includes("Open working folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    pickBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    // Flush the deep async chain: dynamic import -> open() -> loadFolder ->
    // seedTokens (oracle probe + token write) -> remember -> oracle status.
    for (let i = 0; i < 12; i++) await Promise.resolve();
  });
}

/** Open the ExportPopover and click an export row. `which` is the row label prefix
 *  ("Standalone HTML" for absolute, "HTML scaffold" for flow). */
async function clickExport(container: HTMLElement, which: string) {
  const exportBtn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === "Export" && b.querySelector("svg"),
  ) as HTMLButtonElement;
  act(() => exportBtn.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  const row = Array.from(container.querySelectorAll("button")).find((b) =>
    b.textContent?.trim().startsWith(which),
  ) as HTMLButtonElement;
  await act(async () => {
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function clickGenerate(container: HTMLElement) {
  // The composer's send button drives BOTH generate (no selection) and edit (a node
  // selected). runGenerate/runEdit are async (await readBackendKind + grounding before
  // startStream); flush the awaited microtasks so the stream has started before the
  // test drives the terminal `done`.
  const btn = container.querySelector(".send-btn") as HTMLButtonElement;
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

/** Type an edit instruction into the (single) composer textarea — used after a node
 *  is selected, when the composer routes to the edit flow. Same element as the prompt;
 *  this alias documents intent. */
function typeEdit(container: HTMLElement, value: string) {
  typePrompt(container, value);
}

/** Click the composer send button (alias of clickGenerate; the same button sends the
 *  edit when a node is selected). */
async function clickSend(container: HTMLElement) {
  await clickGenerate(container);
}

describe("DesignView — BLOCKER 2: two sequential generations both apply", () => {
  it("does not drop the second generation to a spurious zero-node guard", async () => {
    const { container } = render();

    // First generation.
    typePrompt(container, "make a hero");
    await clickGenerate(container);
    emitDone('<section><h1>First</h1></section>');
    expect(lastProject?.components).toBeTruthy();
    const firstIds = Object.keys(lastProject!.manifest.nodes);
    expect(firstIds.length).toBeGreaterThan(0);
    const firstMarkup = lastProject!.components[firstIds[0]];
    expect(firstMarkup).toContain("First");

    // Second generation: clicking Generate resets text to "" while status is still
    // "done". The buggy code re-fired the done-effect with empty text, set
    // consumedRef, and ignored the real completion -> "No usable markup".
    typePrompt(container, "make a footer");
    await clickGenerate(container);
    // The real completion arrives.
    emitDone('<footer><p>Second</p></footer>');

    // The status must NOT be the zero-node guard; the second markup reached canvas.
    const status = container.textContent ?? "";
    expect(status).not.toContain("No usable markup");
    const allMarkup = Object.values(lastProject!.components).join(" ");
    expect(allMarkup).toContain("Second");
  });
});

describe("DesignView — BLOCKER 3: reload restores structural recovery", () => {
  it("preserves placement on a post-reload regen that drops the id", async () => {
    // The loaded project has hero at a non-default placement and its markup carries
    // the id. After reload, shapesRef must be re-derived so a regen where the model
    // DROPS the id re-anchors structurally instead of re-minting (placement reset).
    const loaded: DesignProject = {
      meta: {
        schemaVersion: 1,
        id: "p",
        name: "Loaded",
        createdAt: "1970-01-01T00:00:00Z",
        updatedAt: "1970-01-01T00:00:00Z",
        canvas: { w: 1440, h: 1024, grid: 8 },
        nodeOrder: ["hero"],
      },
      manifest: {
        schemaVersion: 1,
        nodes: { hero: { x: 600, y: 400, z: 5, w: 420, h: "auto", kind: "html" } },
      },
      components: {
        hero: '<section data-node-id="hero"><h1>Hero</h1></section>',
      },
    };
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_load_project") return loaded;
      return undefined;
    });

    const { container } = render();
    // "Open working folder…" picks C:/proj and loads it (design_load_project mock).
    await pickFolder(container, "C:/proj");
    expect(lastProject?.manifest.nodes["hero"]).toMatchObject({ x: 600, y: 400 });

    // Regenerate with the SAME structure but the model dropped the id.
    typePrompt(container, "regenerate");
    await clickGenerate(container);
    emitDone("<section><h1>Hero v2</h1></section>");

    // Placement preserved via re-derived shapes (id kept, not re-minted).
    expect(Object.keys(lastProject!.manifest.nodes)).toEqual(["hero"]);
    expect(lastProject!.manifest.nodes["hero"]).toMatchObject({ x: 600, y: 400 });
    expect(lastProject!.components["hero"]).toContain("Hero v2");
  });
});

describe("DesignView — BLOCKER 1: edit refreshes the stored structural shape", () => {
  it("keeps a dragged placement when a regen drops the id of a node edited into a NEW structure", async () => {
    const { container } = render();

    // 1) Generate ONE node (minted id, default placement). Structure: section>h1.
    typePrompt(container, "make a card");
    await clickGenerate(container);
    emitDone("<section><h1>Card</h1></section>");
    const id = Object.keys(lastProject!.manifest.nodes)[0];

    // 2) Drag it to a custom placement (so a teleport on regen is detectable).
    act(() => {
      lastOnManifestChange?.({
        schemaVersion: 1,
        nodes: {
          [id]: { x: 700, y: 500, z: 9, w: 360, h: "auto", kind: "html" },
        },
      });
    });
    expect(lastProject!.manifest.nodes[id]).toMatchObject({ x: 700, y: 500 });

    // 3) Select it, EDIT so the model RESTRUCTURES (section>h1,p,p) and DROPS the id.
    const select = container.querySelector(
      "[data-testid=select-first]",
    ) as HTMLButtonElement;
    act(() => select.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    // With a node selected, the composer routes to the edit flow: type into the same
    // composer textarea and press send.
    typeEdit(container, "add two paragraphs");
    await clickSend(container);
    // Edit returns the restructured markup WITHOUT the id (model dropped it).
    emitDone("<section><h1>Card</h1><p>a</p><p>b</p></section>");
    // Edit keeps placement + id; shape stored must now be the RESTRUCTURED form.
    expect(lastProject!.manifest.nodes[id]).toMatchObject({ x: 700, y: 500 });

    // 4) Full regenerate, model emits the SAME restructured structure but DROPS the
    //    id. Re-anchor must succeed using the refreshed shape (section>h1,p,p) and
    //    keep the dragged placement. Without BLOCKER 1's shape refresh, shapesRef
    //    still held section>h1, which no longer matches -> id re-minted -> teleport.
    typePrompt(container, "regenerate dropping id");
    await clickGenerate(container);
    emitDone("<section><h1>Card v4</h1><p>a</p><p>b</p></section>");

    expect(Object.keys(lastProject!.manifest.nodes)).toEqual([id]);
    expect(lastProject!.manifest.nodes[id]).toMatchObject({ x: 700, y: 500 });
    expect(lastProject!.components[id]).toContain("Card v4");
  });
});

describe("DesignView — STEP 4: Oracle grounding + audit log wiring", () => {
  const setFolder = pickFolder;

  it("pre-fetches grounding and folds the chunk text into the prompt context", async () => {
    const calls: { command: string; args?: Record<string, unknown> }[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      calls.push({ command, args });
      if (command === "design_oracle_context") {
        return [
          { fileSource: "src/Button.tsx", score: 0.9, text: "GROUNDING_SNIPPET_XYZ" },
        ];
      }
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      return undefined;
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");

    typePrompt(container, "a pricing section");
    await clickGenerate(container);

    // The grounding command was queried with the user instruction over the folder.
    const grounding = calls.find((c) => c.command === "design_oracle_context");
    expect(grounding).toBeTruthy();
    expect(grounding!.args).toMatchObject({
      workingFolderPath: "C:/target/.aspis-design/landing",
      query: "a pricing section",
    });

    // The streamed prompt carries the grounding snippet verbatim (context block).
    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt).toContain("GROUNDING_SNIPPET_XYZ");
    expect(lastPrompt).toContain("a pricing section");
  });

  it("B4: a CLI backend (claude) receives NO grounding block, even if a prior context existed", async () => {
    // The engine reports a CLI backend. Even though Oracle would return chunks, a CLI
    // provider must NEVER get a pre-fetched grounding block in its prompt (it reaches
    // Oracle agentically). Assert the streamed prompt carries no grounding snippet and
    // that we did NOT even pre-fetch chunks for it.
    const calls: string[] = [];
    invokeSpy.mockImplementation(async (command: string) => {
      calls.push(command);
      if (command === "get_design_llm_backend") return { kind: "claude" };
      if (command === "design_oracle_context") {
        return [
          { fileSource: "src/Secret.tsx", score: 0.9, text: "GROUNDING_SNIPPET_XYZ" },
        ];
      }
      return undefined;
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");

    typePrompt(container, "a pricing section");
    await clickGenerate(container);

    // No grounding pre-fetch for a CLI backend.
    expect(calls).not.toContain("design_oracle_context");
    // The streamed prompt has no grounding snippet block.
    const lastPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(lastPrompt).not.toContain("GROUNDING_SNIPPET_XYZ");
    expect(lastPrompt).not.toContain("Relevant excerpts from the target codebase");
    // The user instruction itself is still present.
    expect(lastPrompt).toContain("a pricing section");
  });

  it("W3: a second Generate during the prepare window does NOT start two backend runs", async () => {
    // readBackendKind is slow (a deferred promise) so the FIRST runGenerate is still
    // awaiting when the SECOND click lands. The preparing guard must drop the second.
    let resolveBackend: ((v: unknown) => void) | null = null;
    invokeSpy.mockImplementation((command: string) => {
      if (command === "get_design_llm_backend") {
        return new Promise((res) => {
          resolveBackend = res;
        });
      }
      return Promise.resolve(undefined);
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");
    typePrompt(container, "make a hero");

    const genBtn = container.querySelector(".send-btn") as HTMLButtonElement;

    // First click: enters runGenerate, blocks on the pending backend-kind promise.
    await act(async () => {
      genBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });
    // Second click WHILE still preparing: must be ignored.
    await act(async () => {
      genBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
    });

    // Release the backend-kind promise -> the single in-flight prepare proceeds.
    await act(async () => {
      resolveBackend?.({ kind: "ollama" });
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Exactly ONE backend run was started despite two clicks.
    expect(streamCtl.starts.length).toBe(1);
  });

  it("appends a metadata-only audit line after a generation", async () => {
    const appends: Record<string, unknown>[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_oracle_context") return [];
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      if (command === "design_append_generation_log") {
        appends.push(args!.entry as Record<string, unknown>);
      }
      return undefined;
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");

    typePrompt(container, "make a hero");
    await clickGenerate(container);
    emitDone("<section><h1>Hero</h1></section>");
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(appends.length).toBe(1);
    const entry = appends[0];
    expect(entry.kind).toBe("generate");
    expect(entry.outcome).toBe("applied");
    expect(entry.backendKind).toBe("ollama");
    expect(entry.oracleGrounded).toBe(false); // empty chunks -> not grounded
    expect(typeof entry.promptChars).toBe("number");
    expect(typeof entry.durationMs).toBe("number");
    // The audit entry must carry NO prompt text / chunk text / secret.
    const serialized = JSON.stringify(entry);
    expect(serialized).not.toContain("make a hero");
    expect(serialized).not.toContain("Hero");
  });

  it("generates without grounding when Oracle fails (graceful degrade)", async () => {
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_oracle_context") throw new Error("oracle down");
      if (command === "get_design_llm_backend") return { kind: "ollama" };
      return undefined;
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");
    typePrompt(container, "a footer");
    await clickGenerate(container);
    emitDone("<footer><p>F</p></footer>");

    // Generation still applied despite the grounding failure.
    const allMarkup = Object.values(lastProject!.components).join(" ");
    expect(allMarkup).toContain("F");
  });

  it("exports code via design_write_export with the chosen mode filename", async () => {
    const exports: Record<string, unknown>[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_write_export") exports.push(args!);
      return undefined;
    });

    const { container } = render();
    await setFolder(container, "C:/target/.aspis-design/landing");

    // Export via the topbar ExportPopover's "Standalone HTML — absolute layout" row.
    await clickExport(container, "Standalone HTML");

    expect(exports.length).toBe(1);
    expect(exports[0].filename).toBe("export-absolute.html");
    expect(String(exports[0].content)).toContain("position:absolute");
  });

  it("BLOCKER 1: sanitizes disk-loaded markup before exporting (no XSS survives)", async () => {
    // A LOADED project whose component carries hand-edited/malicious markup
    // (design_load_project returns raw disk bytes). The export must NOT inline a
    // live <script> or onerror handler — sanitizeNodeMarkup runs in runExport.
    const loaded: DesignProject = {
      meta: {
        schemaVersion: 1,
        id: "p",
        name: "Loaded",
        createdAt: "1970-01-01T00:00:00Z",
        updatedAt: "1970-01-01T00:00:00Z",
        canvas: { w: 1440, h: 1024, grid: 8 },
        nodeOrder: ["hero"],
      },
      manifest: {
        schemaVersion: 1,
        nodes: { hero: { x: 80, y: 80, z: 1, w: 420, h: "auto", kind: "html" } },
      },
      components: {
        hero:
          '<section data-node-id="hero"><script>alert(1)</script>' +
          '<img src="x" onerror="alert(2)"><p>safe</p></section>',
      },
    };
    const exports: Record<string, unknown>[] = [];
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_load_project") return loaded;
      if (command === "design_write_export") exports.push(args!);
      return undefined;
    });

    const { container } = render();
    // "Open working folder…" loads the malicious project from disk in one step.
    await setFolder(container, "C:/target/.aspis-design/landing");

    // Export it via the topbar ExportPopover.
    await clickExport(container, "Standalone HTML");

    expect(exports.length).toBe(1);
    const html = String(exports[0].content);
    // The malicious vectors are neutralized; the safe content survives.
    expect(html).not.toContain("<script");
    expect(html).not.toContain("alert(1)");
    expect(html).not.toContain("onerror");
    expect(html).not.toContain("alert(2)");
    expect(html).toContain("safe");
  });

  it("WARNING 3+5: load awaits a path-free token seed (tokens.json leaks no file paths)", async () => {
    const writes: Record<string, unknown>[] = [];
    let oracleQueried = false;
    let writeAfterLoad = false;
    let loadReturned = false;
    invokeSpy.mockImplementation(async (command: string, args) => {
      if (command === "design_load_project") {
        loadReturned = true;
        return {
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
        } as DesignProject;
      }
      if (command === "design_oracle_context") {
        oracleQueried = true;
        // Oracle returns chunks carrying SENSITIVE target file paths.
        return [
          { fileSource: "src/theme/tokens.ts", score: 0.9, text: "secret" },
          { fileSource: "src/design/palette.scss", score: 0.8, text: "secret" },
        ];
      }
      if (command === "design_write_tokens") {
        // The seed write must come AFTER load resolved (awaited inside runLoad).
        writeAfterLoad = loadReturned;
        writes.push(args!);
      }
      return undefined;
    });

    const { container } = render();
    // "Open working folder…" picks the path and loads it (running the token seed).
    await setFolder(container, "C:/target/.aspis-design/landing");

    // The seed ran (Oracle probed) and persisted within the awaited load flow.
    expect(oracleQueried).toBe(true);
    expect(writes.length).toBe(1);
    expect(writeAfterLoad).toBe(true);
    // The persisted tokens.json carries NO target file paths (WARNING 3).
    const tokensJson = String(writes[0].tokensJson);
    expect(tokensJson).not.toContain("src/theme/tokens.ts");
    expect(tokensJson).not.toContain("src/design/palette.scss");
    expect(tokensJson).not.toContain("Seeded from");
    // It is a clean (empty) DTCG stub.
    expect(JSON.parse(tokensJson)).toEqual({});
  });
});

describe("DesignView — Phase 2.5 Tier 1: bounded self-repair loop", () => {
  it("retries ONCE on an empty/garbage generation, then commits the corrected pass", async () => {
    const { container } = render();
    typePrompt(container, "make a hero");
    await clickGenerate(container);
    const startsBefore = streamCtl.starts.length;

    // First completion: a bare <tr> — the parser discards it, so zero nodes commit.
    emitDone("<tr><td>oops</td></tr>");
    await act(async () => {
      await Promise.resolve();
    });

    // A repair retry was launched (a second start), and the canvas was NOT committed
    // with the empty result (still the demo project's nodes, unchanged count).
    expect(streamCtl.starts.length).toBe(startsBefore + 1);
    const repairPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    expect(repairPrompt).toContain("non-empty UI markup"); // EMPTY-fallback correction
    expect(repairPrompt).toContain("make a hero"); // original instruction preserved
    const statusMid = container.textContent ?? "";
    expect(statusMid).toContain("retrying with a corrected prompt");

    // The retry returns valid markup -> it commits.
    emitDone("<section><h1>Fixed hero</h1></section>");
    await act(async () => {
      await Promise.resolve();
    });
    const allMarkup = Object.values(lastProject!.components).join(" ");
    expect(allMarkup).toContain("Fixed hero");
    const statusEnd = container.textContent ?? "";
    expect(statusEnd).not.toContain("retrying");
  });

  it("retries with a targeted correction when a node is DROPPED (foster <option>)", async () => {
    const { container } = render();
    typePrompt(container, "a list");
    await clickGenerate(container);
    const startsBefore = streamCtl.starts.length;

    // One valid node + one <option> (a foster tag that SURVIVES parsing -> dropped
    // with a remaining FOSTER_PARENTED_ROOT violation -> triggers repair).
    emitDone("<section><h1>Good</h1></section><option>bad</option>");
    await act(async () => {
      await Promise.resolve();
    });

    expect(streamCtl.starts.length).toBe(startsBefore + 1);
    const repairPrompt = streamCtl.starts[streamCtl.starts.length - 1];
    // The correction lists the foster-parent tags.
    expect(repairPrompt).toContain("<tr>");
    expect(repairPrompt).toContain("a list");
  });

  it("GIVES UP after the cap (never loops); canvas stays unchanged", async () => {
    const { container } = render();
    const demoNodeCount = Object.keys(lastProject!.manifest.nodes).length;
    typePrompt(container, "make a hero");
    await clickGenerate(container);

    // Attempt 0: empty -> retry.
    emitDone("<tr><td>x</td></tr>");
    await act(async () => {
      await Promise.resolve();
    });
    // Attempt 1 (the single retry): still empty -> cap reached, NO further retry.
    const startsAfterFirstRetry = streamCtl.starts.length;
    emitDone("<td>still bad</td>");
    await act(async () => {
      await Promise.resolve();
    });

    // No third start was issued (loop is bounded at DEFAULT_REPAIR_RETRIES=1).
    expect(streamCtl.starts.length).toBe(startsAfterFirstRetry);
    const status = container.textContent ?? "";
    expect(status).toContain("Couldn't get valid markup after 2 tries");
    // The canvas was never corrupted: the demo nodes remain.
    expect(Object.keys(lastProject!.manifest.nodes).length).toBe(demoNodeCount);
  });

  it("is cancel-aware: a cancelled stream never triggers a repair", async () => {
    const { container } = render();
    typePrompt(container, "make a hero");
    await clickGenerate(container);
    const startsBefore = streamCtl.starts.length;

    // Terminal CANCELLED (not done): the done-effect must not run -> no repair start.
    act(() => {
      streamCtl.state = { text: "<tr><td>x</td></tr>", status: "cancelled", error: null };
      rerender?.();
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(streamCtl.starts.length).toBe(startsBefore);
  });

  it("a clean generation does NOT trigger a repair", async () => {
    const { container } = render();
    typePrompt(container, "make a hero");
    await clickGenerate(container);
    const startsBefore = streamCtl.starts.length;
    emitDone("<section><h1>All good</h1></section>");
    await act(async () => {
      await Promise.resolve();
    });
    expect(streamCtl.starts.length).toBe(startsBefore); // no retry
    const allMarkup = Object.values(lastProject!.components).join(" ");
    expect(allMarkup).toContain("All good");
  });
});

describe("DesignView — W4: generation persist cancels a pending drag manifest write", () => {
  it("a generation save drops the throttled drag write so it can't clobber the manifest", async () => {
    vi.useFakeTimers();
    try {
      const { container } = render();
      await setFolderForTest(container, "C:/proj");

      // 1) Generate one node so there is a real project to persist.
      typePromptSync(container, "make a hero");
      await clickGenerateFake(container);
      emitDone("<section><h1>Hero</h1></section>");
      const id = Object.keys(lastProject!.manifest.nodes)[0];

      // 2) Commit a DRAG manifest (schedules a throttled design_write_manifest).
      act(() => {
        lastOnManifestChange?.({
          schemaVersion: 1,
          nodes: {
            [id]: { x: 700, y: 500, z: 9, w: 360, h: "auto", kind: "html" },
          },
        });
      });

      const manifestWritesBefore = invokeSpy.mock.calls.filter(
        (c) => c[0] === "design_write_manifest",
      ).length;

      // 3) A SECOND generation completes -> persistProject runs (design_save_project)
      //    and must cancel the pending drag write.
      typePromptSync(container, "make a footer");
      await clickGenerateFake(container);
      emitDone("<footer><p>Foot</p></footer>");

      const saves = invokeSpy.mock.calls.filter(
        (c) => c[0] === "design_save_project",
      ).length;
      expect(saves).toBeGreaterThan(0);

      // 4) Advance PAST the throttle: the pending drag write must NOT fire (it would
      //    clobber the just-saved generation manifest with a stale drag-only one).
      act(() => {
        vi.advanceTimersByTime(1000);
      });
      const manifestWritesAfter = invokeSpy.mock.calls.filter(
        (c) => c[0] === "design_write_manifest",
      ).length;
      expect(manifestWritesAfter).toBe(manifestWritesBefore);
    } finally {
      vi.useRealTimers();
    }
  });
});

// Fake-timer-safe variants of the helpers (the shared ones await real microtasks
// which still resolve under fake timers, but we keep these explicit for clarity).
function typePromptSync(container: HTMLElement, value: string) {
  const ta = container.querySelector("textarea") as HTMLTextAreaElement;
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    setter.call(ta, value);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function setFolderForTest(container: HTMLElement, value: string) {
  dialogCtl.nextPick = value;
  if (!container.querySelector(".pop.left")) {
    const proj = container.querySelector(".tb-proj") as HTMLButtonElement;
    act(() => proj.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  }
  const pickBtn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.includes("Open working folder"),
  ) as HTMLButtonElement;
  await act(async () => {
    pickBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function clickGenerateFake(container: HTMLElement) {
  const btn = container.querySelector(".send-btn") as HTMLButtonElement;
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

describe("DesignView — WARNING 4: persistNode serializes node then manifest", () => {
  it("writes design_write_node BEFORE design_write_manifest for an edit", async () => {
    const order: string[] = [];
    invokeSpy.mockImplementation(async (command: string) => {
      if (command === "design_write_node" || command === "design_write_manifest") {
        order.push(command);
      }
      return undefined;
    });

    const { container } = render();
    // Choose a folder so persistence runs.
    await pickFolder(container, "C:/proj");

    // Select the first node, type an edit instruction into the composer, send.
    const select = container.querySelector(
      "[data-testid=select-first]",
    ) as HTMLButtonElement;
    act(() => select.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    typeEdit(container, "make it blue");
    await clickSend(container);

    emitDone('<section data-node-id="hero"><h1>Edited</h1></section>');
    // Let the serialized async writes settle.
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(order).toEqual(["design_write_node", "design_write_manifest"]);
  });
});

describe("DesignView — MAJOR: retrying an edit whose node was deleted errors (no silent generate)", () => {
  it("patches the card to a 'node no longer exists' error instead of running a generate", async () => {
    const { container } = render();

    // 1) Generate a node so there is a real node id to edit.
    typePrompt(container, "make a card");
    await clickGenerate(container);
    emitDone("<section><h1>Card</h1></section>");
    const id = Object.keys(lastProject!.manifest.nodes)[0];

    // 2) Select it and EDIT it (the resulting card carries editNodeId === id).
    const select = container.querySelector(
      "[data-testid=select-first]",
    ) as HTMLButtonElement;
    act(() => select.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    typeEdit(container, "make it blue");
    await clickSend(container);
    emitDone('<section data-node-id="' + id + '"><h1>Edited</h1></section>');

    // 3) Delete the edited node from the manifest (a later op removed it).
    act(() => {
      lastOnManifestChange?.({ schemaVersion: 1, nodes: {} });
    });
    expect(lastProject!.manifest.nodes[id]).toBeUndefined();

    const startsBefore = streamCtl.starts.length;

    // 4) Click the EDIT card's Regenerate (onRerun) — it is the LAST card rendered, so
    //    pick the last matching button (the first Regenerate belongs to the generate card).
    //    It must NOT start a generate; it must flip the card to a clear
    //    "node no longer exists" error.
    const reruns = Array.from(container.querySelectorAll("button")).filter((b) =>
      b.textContent?.trim().startsWith("Regenerate"),
    ) as HTMLButtonElement[];
    expect(reruns.length).toBeGreaterThan(0);
    const rerun = reruns[reruns.length - 1];
    await act(async () => {
      rerun.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });

    // No new stream was started (it did NOT silently become a generate).
    expect(streamCtl.starts.length).toBe(startsBefore);
    const text = container.textContent ?? "";
    expect(text).toContain("Node no longer exists");
  });
});
