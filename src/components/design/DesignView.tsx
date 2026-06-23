// DesignView — generative-design module surface.
//
// Phase A2 (shell reskin, FINAL pass): the view root is the prototype's `.dsgn` layout
// — a `.main` column with the TopBar + popovers (project picker / Oracle / export /
// save split) over the `.work` row: the direct-DOM DesignCanvas | a draggable
// `.panel-resizer` | the prototype's `AssistantPanel` (transcript + composer + model
// popover). The composer drives BOTH real flows — a node selection routes to the
// per-node EDIT round-trip, no selection to a full GENERATE — and the assistant
// transcript is a PRESENTATION-ONLY projection of the existing flow control points
// (runGenerate / runEdit / the done-effect / cancel / self-repair). ALL the Phase-2/3
// pipeline logic (generation, self-repair, Oracle grounding, registry, undo/redo,
// throttled persistence, the B4 CLI-grounding gate) is preserved verbatim; this pass
// only adds the message-model state next to it and swaps the temp forms for the panel.

import { useCallback, useEffect, useRef, useState } from "react";
import { FolderOpen, Sparkles } from "lucide-react";
import {
  invokeBackendCommand,
  isTauriRuntime,
  useAppContext,
} from "../../context/AppContext";
import type {
  DesignManifest,
  DesignProject,
  DesignProjectEntry,
  DesignOracleStatus,
} from "../../types/design";
import type { DesignLlmBackend } from "../../types/config";
import type { ProjectSummary } from "../../types/backend";
import { AssistantPanel } from "./panel/AssistantPanel";
import { useAssistantMessages } from "./panel/useAssistantMessages";
import type { AssistantMessage } from "./panel/types";
import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/source-serif-4";
import "./design.css";
import { DesignCanvas } from "./canvas/DesignCanvas";
import { polygonIntersectsRect } from "./canvas/spotEditGeometry";
import type { NodeRect, Point } from "../../types/design";
import {
  createHistory,
  push as pushHistory,
  undo as undoHistory,
  redo as redoHistory,
  type History,
} from "./history";
import { sanitizeNodeMarkup } from "./sanitize";
import { useDesignStream } from "./useDesignStream";
import {
  applyEdit,
  applyGeneration,
  applyNodeId,
  type ShapeMap,
  type GenerationResult,
} from "./generation/pipeline";
import { buildEditPrompt, buildGeneratePrompt } from "./generation/prompt";
import {
  shouldSelfRepair,
  buildRepairPrompt,
  DEFAULT_REPAIR_RETRIES,
} from "./generation/selfRepair";
import { parseTopLevelNodes } from "./iframeInject";
import type { ParsedNode } from "./engine/keyedDiff";
import {
  buildGroundingBlock,
  type DesignContextChunk,
} from "./generation/grounding";
import {
  tokenNamesForPrompt,
  isValidTokensDoc,
  type DtcgDocument,
} from "./engine/tokens";
import { extractTokensFromChunks } from "./contract/extractTokens";
import { buildDesignMdDraft, clampDesignMd } from "./contract/designMd";
import { sha256Hex } from "./contract/sha256";
import { DesignMdEditor } from "./contract/DesignMdEditor";
import { exportCode, type ExportMode } from "./export/exportCode";
import { TopBar } from "./shell/TopBar";
import { HandoffModal } from "./shell/HandoffModal";
import { useHandoff } from "./shell/useHandoff";
import { Toast } from "./shell/Toast";
import { useSaveState } from "./shell/useSaveState";
import { usePreview } from "./preview/usePreview";

/** A token-free audit entry mirroring the Rust `GenerationLogEntry` (camelCase). */
interface GenerationLogEntry {
  ts: string;
  kind: "generate" | "edit";
  nodeIds: string[];
  backendKind: string;
  promptChars: number;
  oracleGrounded: boolean;
  durationMs: number;
  outcome: "applied" | "empty" | "error";
}

/** Throttle interval (ms) for drag-commit manifest writes to disk. */
const MANIFEST_WRITE_THROTTLE_MS = 400;

/**
 * B4 — backend kinds that run as a LOCAL CLI subprocess (claude/codex) or a
 * user-supplied command (`api`). These reach Oracle AGENTICALLY through their own
 * MCP config; they must NEVER receive a pre-fetched Oracle grounding BLOCK in the
 * prompt, because the chunk text is untrusted target source and a CLI provider
 * could be steered by an injected instruction into `codex exec` etc. Grounding is
 * pre-fetched ONLY for the in-process HTTP providers (ollama/omlx). This is the
 * authoritative gate, applied at EVERY point a prompt is built (generate + repair).
 */
const CLI_BACKEND_KINDS = new Set(["claude", "codex", "api"]);

function isCliBackend(backendKind: string): boolean {
  return CLI_BACKEND_KINDS.has(backendKind);
}

/** V1: strip the Windows extended-length (`\\?\`) verbatim prefix that `fs::canonicalize`
 * historically wrote into stored registry paths. The Rust side now strips this at write
 * time, but registry entries persisted BEFORE that fix may still carry the prefix; this
 * TS-side strip (defense in depth) makes `findRecordedSha` still match those legacy
 * entries. Mirrors `strip_verbatim_prefix` in design.rs:
 *   `\\?\UNC\server\share\...` → `\\server\share\...`
 *   `\\?\C:\...`               → `C:\...`
 * MUST run BEFORE the separator collapse below (which would otherwise mangle `\\?\`). PURE. */
function stripVerbatimPrefix(path: string): string {
  if (path.startsWith("\\\\?\\UNC\\")) {
    return "\\\\" + path.slice("\\\\?\\UNC\\".length);
  }
  if (path.startsWith("\\\\?\\")) {
    return path.slice("\\\\?\\".length);
  }
  return path;
}

/** Normalize a folder path for a lenient registry lookup: strip the verbatim prefix, trim,
 * unify separators, drop a trailing separator, and lowercase (Windows FS is case-
 * insensitive; the stored path is server-canonicalized and may differ from the raw picked
 * path only in these ways). PURE. */
export function normFolderKey(path: string): string {
  return stripVerbatimPrefix(path.trim())
    .replace(/[\\/]+/g, "/")
    .replace(/\/+$/, "")
    .toLowerCase();
}

/** Find the APPROVED contract hash recorded for `folderPath` in the registry list, if
 * any. Matches the stored (canonicalized) path leniently against the raw folder. PURE. */
function findRecordedSha(
  entries: DesignProjectEntry[],
  folderPath: string,
): string | undefined {
  const key = normFolderKey(folderPath);
  const hit = entries.find((e) => normFolderKey(e.workingFolderPath) === key);
  return hit?.contractSha;
}

/** Build the hardcoded demo project (deterministic, no clock fields used for
 * layout). Markup is pre-sanitized so the on-disk/in-memory copy is already the
 * safe form the canvas would inject. */
function buildDemoProject(): DesignProject {
  const hero =
    '<section data-node-id="hero" style="background:#fff7ed;border-radius:16px;padding:24px"><h1 style="margin:0;font:600 24px sans-serif;color:#7c2d12">Build in lockstep</h1><p style="margin:8px 0 0;color:#9a3412">Design grounded in your real codebase.</p></section>';
  const cta =
    '<button data-node-id="cta" style="background:#c2410c;color:#fff;border:0;border-radius:12px;padding:12px 20px;font:600 14px sans-serif">Get started</button>';
  const note =
    '<div data-node-id="note" style="background:#fff;border:1px solid #fed7aa;border-radius:12px;padding:16px;font:13px sans-serif;color:#7c2d12">Drag a card. Drag its bottom-right corner to resize.</div>';
  return {
    meta: {
      schemaVersion: 1,
      id: "demo",
      name: "Demo landing",
      createdAt: "1970-01-01T00:00:00Z",
      updatedAt: "1970-01-01T00:00:00Z",
      canvas: { w: 1440, h: 1024, grid: 8 },
      nodeOrder: ["hero", "cta", "note"],
    },
    manifest: {
      schemaVersion: 1,
      nodes: {
        hero: { x: 80, y: 80, z: 1, w: 420, h: "auto", kind: "html" },
        cta: { x: 80, y: 260, z: 2, w: 160, h: "auto", kind: "html" },
        note: { x: 560, y: 80, z: 3, w: 320, h: "auto", kind: "html" },
      },
    },
    components: {
      hero: sanitizeNodeMarkup(hero),
      cta: sanitizeNodeMarkup(cta),
      note: sanitizeNodeMarkup(note),
    },
  };
}

/**
 * Re-derive the per-id structural ShapeMap from a project's stored component
 * markup, so a freshly LOADED (or in-memory) project can structurally re-anchor
 * dropped/renamed ids on its FIRST regeneration (BLOCKER 3). For each component we
 * parse the top-level elements and take the first one's ParsedNode shape (each
 * component is exactly one top-level node by construction). Ids missing markup or
 * yielding no element are skipped (they simply won't get structural recovery).
 */
function deriveShapes(components: Record<string, string>): ShapeMap {
  const shapes: ShapeMap = {};
  for (const [id, markup] of Object.entries(components)) {
    if (!markup) continue;
    const parsed: ParsedNode[] = parseTopLevelNodes(markup);
    if (parsed.length > 0) shapes[id] = parsed[0];
  }
  return shapes;
}

/** An undo/redo snapshot: the parts of a project an interactive edit can change.
 *  Meta fields other than `nodeOrder` are not mutated by canvas edits, so they are
 *  restored from the live project at apply time. */
interface DesignSnapshot {
  manifest: DesignManifest;
  components: Record<string, string>;
  nodeOrder: string[];
}

// W2: SHALLOW by design. A snapshot holds REFERENCES to the live manifest /
// components / nodeOrder objects — it does NOT deep-clone them. Component markup is
// stored as immutable strings (every edit replaces the whole string, never mutates
// it in place) and manifest/nodeOrder mutations always produce NEW objects via the
// immutable engine ops, so sharing references between snapshots is safe: a later
// edit cannot retroactively alter an earlier snapshot. Memory posture: strings are
// shared (not duplicated) across snapshots, and the 60-entry history cap
// (MAX_HISTORY) bounds the snapshot COUNT, so total retained memory stays small.
function snapshotOf(project: DesignProject): DesignSnapshot {
  return {
    manifest: project.manifest,
    components: project.components,
    nodeOrder: project.meta.nodeOrder,
  };
}

/** Fixed prompt prefix for a user-described Spot Edit region edit. Module scope — it is
 *  a constant, not per-render state. */
const SPOT_PREFIX = "Spot edit (region selection): ";
/** Fixed instruction when a Spot Edit region prompt is empty (auto-detect mode). */
const SPOT_AUTODETECT =
  "Fix off-token colors, contrast and spacing inconsistencies in this section; return the same element.";

export function DesignView() {
  const [project, setProject] = useState<DesignProject>(() => buildDemoProject());
  const [folder, setFolder] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Transient confirmation toast (export/save success etc.). Auto-dismissed by the
  // Toast component; `showToast` just sets the message.
  const [toast, setToast] = useState<string | null>(null);
  const showToast = useCallback((msg: string) => setToast(msg), []);

  // Focus / fullscreen mode: hide the app shell chrome and lift `.dsgn` to fill the
  // viewport (the `.dsgn-full` class). Esc does NOT exit (matches the prototype).
  const [fullscreen, setFullscreen] = useState(false);

  // --- save-state signals for the TopBar dot (no new writes/timers) -----------
  // `pendingDirty`: a throttled drag-commit write is queued but not yet on disk.
  // `savingCount`: how many IPC saves are in flight right now. Both are instrumented
  // around the EXISTING persistence calls, not new machinery. A ref counter +
  // mirror state keeps the count correct under StrictMode double-invocation.
  const [pendingDirty, setPendingDirty] = useState(false);
  const savingCountRef = useRef(0);
  const [savingCount, setSavingCount] = useState(0);
  const beginSaving = useCallback(() => {
    savingCountRef.current += 1;
    setSavingCount(savingCountRef.current);
  }, []);
  const endSaving = useCallback(() => {
    savingCountRef.current = Math.max(0, savingCountRef.current - 1);
    setSavingCount(savingCountRef.current);
  }, []);
  const { state: saveState, saving } = useSaveState({
    writing: savingCount > 0,
    pendingDirty,
  });

  // Undo/redo history over interactive canvas/inspector edits. `onBeginChange`
  // (fired by the canvas BEFORE each committed mutation) snapshots the live project
  // here; undo/redo restore a snapshot through the SAME persistence path the edits
  // use. Generation/load REPLACE the project wholesale and clear history (a new
  // baseline) so undo never crosses a generation boundary.
  const historyRef = useRef<History<DesignSnapshot>>(createHistory<DesignSnapshot>());
  const [historyFlags, setHistoryFlags] = useState<{
    canUndo: boolean;
    canRedo: boolean;
  }>({ canUndo: false, canRedo: false });
  const setHistoryValue = useCallback((h: History<DesignSnapshot>) => {
    historyRef.current = h;
    setHistoryFlags((prev) =>
      prev.canUndo === h.canUndo && prev.canRedo === h.canRedo
        ? prev
        : { canUndo: h.canUndo, canRedo: h.canRedo },
    );
  }, []);

  // Phase 3 (management plane) — the recent-projects registry (metadata only, in
  // config.json). Loaded on mount; refreshed after every remember/rename/remove (the
  // commands return the full sorted list). `renamingId`/`renameDraft` drive the
  // inline rename editor inside the ProjectPopover.
  const [recent, setRecent] = useState<DesignProjectEntry[]>([]);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  // Phase-2 STEP 3: the full generation pipeline. The stream feeds raw model TEXT;
  // on `done` the deterministic pipeline parses -> re-anchors -> places -> sanitizes
  // it into the canvas, then persists. Two flows share one stream: a full GENERATE
  // (prompt box) and a per-node EDIT (selected node + instruction).
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const {
    text: streamText,
    status: streamStatus,
    start: startStream,
    cancel: cancelGeneration,
  } = useDesignStream();

  const tauri = isTauriRuntime();

  // Navigation to Settings → Providers (the full backend editor the model popover
  // links to). Read from the app context; guarded so a missing provider (tests that
  // don't render the AppProvider) degrades to a no-op rather than crashing.
  const ctx = useAppContext();
  const requestView = ctx?.requestView;

  // --- Assistant panel state (the prototype's right column) -------------------
  // The composer's controlled draft (one box drives both generate + edit). The
  // run functions take the instruction explicitly, so the draft is purely UI.
  const [draft, setDraft] = useState("");
  // Bumped to imperatively focus the composer (empty-canvas "Generate a section").
  const [focusSignal, setFocusSignal] = useState(0);
  // Assistant-panel width (px), drag-resized between 290 and 540 (default 350).
  const [panelW, setPanelW] = useState(350);
  // The transcript of user prompts + assistant cards (presentation only). Destructure
  // the STABLE callbacks (push/patch are useCallback-memoized) so the run callbacks +
  // the done-effect depend on them, not the whole api object (whose identity changes
  // every render as `messages` updates) — avoiding needless effect/callback churn.
  const {
    messages: panelMessages,
    push: pushMessage,
    patch: patchMessage,
    doneCount: panelDoneCount,
  } = useAssistantMessages();
  // The id of the assistant card for the CURRENT in-flight run, patched by the
  // done-effect / self-repair. Carried through repair so one card spans the loop.
  const activeMsgRef = useRef<number | null>(null);

  // Phase D — the management-plane projects (id, title, rootPath) the hand-off
  // dispatch can target. Loaded lazily when the modal opens (cheap list_projects), so
  // a design session that never hands off pays nothing. Empty in non-Tauri tests.
  const [handoffProjects, setHandoffProjects] = useState<ProjectSummary[]>([]);
  // Surfaced in the hand-off modal near the project selector when list_projects fails:
  // without it the selector silently shows no projects and the user cannot tell whether
  // the repo genuinely has none or the load errored (Fix 6).
  const [handoffProjectsError, setHandoffProjectsError] = useState<string | null>(null);
  const loadHandoffProjects = useCallback(async () => {
    if (!tauri) return;
    try {
      const list = await invokeBackendCommand<ProjectSummary[]>("list_projects");
      setHandoffProjects(list ?? []);
      setHandoffProjectsError(null);
    } catch {
      // The packaging steps still run; but the dispatch selector would otherwise show no
      // projects with no explanation. Surface a hint so the user can recover.
      setHandoffProjectsError("Could not load projects — close and reopen.");
    }
  }, [tauri]);

  // The current global design-LLM backend (for the composer's model chip + popover).
  // Fetched on mount and refreshed after any save from the popover.
  const [backend, setBackend] = useState<DesignLlmBackend | null>(null);
  const refreshBackend = useCallback(async () => {
    if (!tauri) return;
    try {
      const b = await invokeBackendCommand<DesignLlmBackend | null>(
        "get_design_llm_backend",
        {},
      );
      setBackend(b ?? null);
    } catch {
      // Non-fatal: the chip falls back to the first provider label.
    }
  }, [tauri]);
  useEffect(() => {
    void refreshBackend();
  }, [refreshBackend]);

  // Persist a backend chosen in the model popover, then refresh the local copy.
  const saveBackend = useCallback(
    (next: DesignLlmBackend) => {
      if (!tauri) {
        setBackend(next);
        return;
      }
      invokeBackendCommand<DesignLlmBackend | null>("set_design_llm_backend", {
        backend: next,
      })
        .then(() => refreshBackend())
        .catch((e) => setError(String(e)));
    },
    [tauri, refreshBackend],
  );

  const openProviderSettings = useCallback(() => {
    requestView?.("settings", "providers");
  }, [requestView]);

  // The structural shapes from the LAST generation, keyed by node id. Persisted in
  // memory so the NEXT regeneration can structurally re-anchor dropped/renamed ids.
  const shapesRef = useRef<ShapeMap>({});

  // The W3C DTCG tokens document for this project (seeded from the target via Oracle
  // on load; empty by default). Kept in memory; persisted to tokens.json. Its NAMES
  // feed the generate prompt as a soft "prefer these tokens" preference; its color
  // $values feed the Oracle popover swatches.
  const [tokens, setTokens] = useState<DtcgDocument>({});
  const tokensRef = useRef<DtcgDocument>(tokens);
  tokensRef.current = tokens;

  // The project's design.md contract, stashed in memory for prompt injection. It is
  // populated from design_read_design_md on load (when present) or after the user
  // Saves the contract editor. Injected (clamped to 16 KiB) into EVERY prompt — see
  // the trust note at the prompt build sites + in generation/prompt.ts. Empty = no
  // contract; nothing is injected. NEVER written to from a render path; only the
  // editor's explicit Save updates it.
  const contractRef = useRef<string>("");

  // Fix 4: a monotonically-increasing SEED EPOCH. Bumped at the START of every
  // loadFolder / createInFolder. seedContract() captures the epoch it began under and
  // re-checks it before EVERY state-applying step (setContractEditor / contractRef stash
  // / token stub), so a slow async seed for project A that resolves AFTER the user has
  // switched to project B can never apply A's draft/contract over B's session.
  const seedEpochRef = useRef(0);

  // Fix 3: live mirror of the recent-projects registry so seedContract can look up the
  // APPROVED contract hash for the current folder at call time (no stale closure).
  const recentRef = useRef<DesignProjectEntry[]>(recent);
  recentRef.current = recent;

  // V10: the in-flight initial registry-list load. On a COLD start (deep-link straight into
  // a project) seedContract's findRecordedSha can run BEFORE the mount-time
  // design_registry_list effect has populated recentRef — the approved contract would then
  // be mis-detected as "changed out of band" and pop the review editor. seedContract awaits
  // this promise (epoch guard intact) so the recorded hash is present before the lookup. It
  // resolves (never rejects) once the list has settled into `recent`.
  const registryReadyRef = useRef<Promise<void> | null>(null);

  // Contract editor (DesignMdEditor) modal state. `null` = closed. `draftTokens` are
  // written alongside design.md on Save when the draft came from token extraction;
  // a preset the user picks inside the editor supplies its own tokens instead.
  //
  // `fromSeedFlow` (Fix 2): true ONLY when the editor was opened by the post-create/open
  // SEED flow. Skip writes the clean empty tokens.json ONLY in that case — a MANUAL open
  // (or the provenance-review path) must NEVER clobber an existing tokens.json on Skip.
  // `notice` (Fix 3): an optional banner shown when the editor opened to REVIEW a contract
  // that changed outside the editor (hash mismatch) before it can be used.
  // `saveError` (Fix 5): an inline error surfaced when a Save write FAILED so the editor
  // stays open with the user's content intact for a retry.
  const [contractEditor, setContractEditor] = useState<{
    initialContent: string;
    draftTokens?: DtcgDocument;
    fromSeedFlow: boolean;
    notice?: string;
    saveError?: string;
  } | null>(null);

  // Compact Oracle grounding status for the topbar chip + popover head label. Fetched
  // best-effort after a load (and refreshed by the popover when it opens). The chip
  // label reads from this; the popover re-fetches its own deeper stats on open.
  const [oracleStatus, setOracleStatus] = useState<DesignOracleStatus | undefined>(
    undefined,
  );

  // Per-run metadata carried from start -> terminal `done`, so the token-free audit
  // line records the right backend/prompt-size/grounding/duration. Stamped at start.
  interface RunMeta {
    startedAt: number;
    backendKind: string;
    promptChars: number;
    oracleGrounded: boolean;
    // Presentation-only carry: the assistant card id for this run, the instruction
    // (so the done card can offer Regenerate/Retry), the fetched grounding sources
    // (HTTP providers — file paths shown as src-chips), and whether the provider is
    // CLI/agentic (B4: no fetched sources, render the "via MCP" note instead).
    msgId: number;
    instruction: string;
    sources: string[];
    agentic: boolean;
  }

  // Bounded self-repair (Phase 2.5 Tier 1) state for a FULL generation. Fix 8: the
  // design.md contract is NO LONGER stored here — launchRepair reads the LIVE
  // `contractRef.current` at repair time so a contract Save between the initial generate
  // and the repair is honoured (and a contract that became UNINJECTED is not resurrected).
  interface RepairState {
    instruction: string;
    context: string;
    attempts: number;
  }
  const repairRef = useRef<RepairState | null>(null);

  // What the in-flight stream is doing, consumed exactly once on the terminal
  // `done` transition. `null` means no pipeline action is owed.
  const pendingRunRef = useRef<
    | { mode: "generate"; meta: RunMeta }
    | { mode: "edit"; nodeId: string; meta: RunMeta }
    | null
  >(null);
  const consumedRef = useRef(false);

  // V5c: re-entry guard for runConsolidate. A whole-project save is a single in-flight
  // operation; a second concurrent call (double-click Save, or Save racing the hand-off
  // packaging step) would issue two design_save_project writes against the same folder.
  // The ref claims the slot synchronously; the second caller returns false immediately.
  const consolidatingRef = useRef(false);

  // W3: re-entry guard for the async PREPARE window of a start.
  const preparingRef = useRef(false);
  const [preparing, setPreparing] = useState(false);
  const beginPrepare = useCallback((): boolean => {
    if (preparingRef.current) return false;
    preparingRef.current = true;
    setPreparing(true);
    return true;
  }, []);
  const endPrepare = useCallback(() => {
    preparingRef.current = false;
    setPreparing(false);
  }, []);

  // Live project ref for the throttled writer (never captures a stale project).
  const projectRef = useRef(project);
  projectRef.current = project;

  // V7: a live mirror of `panelBusy` (preparing || streaming || spotBusy) read by the
  // window-level undo/redo keydown handler. The handler is a stable listener whose closure
  // must NOT capture a stale `panelBusy` value; a ref kept in sync (below, once panelBusy is
  // computed) lets the handler early-return while a generation/edit/Spot-Edit chain is live
  // so Ctrl+Z/Y cannot mutate the project (applySnapshot + persist) mid-stream.
  const panelBusyRef = useRef(false);

  // Live folder ref: `flushManifest` reads the folder at CALL time, not at
  // closure-capture time.
  const folderRef = useRef(folder);
  folderRef.current = folder;

  // Throttle handle for drag-commit manifest writes.
  const writeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingManifest = useRef<DesignManifest | null>(null);

  const flushManifest = useCallback(() => {
    writeTimer.current = null;
    const manifest = pendingManifest.current;
    pendingManifest.current = null;
    setPendingDirty(false);
    const folderPath = folderRef.current.trim();
    if (!manifest || !folderPath || !tauri) return;
    beginSaving();
    invokeBackendCommand("design_write_manifest", {
      workingFolderPath: folderPath,
      manifest,
    })
      .catch((e) => setError(String(e)))
      .finally(endSaving);
  }, [tauri, beginSaving, endSaving]);

  // Drag/resize/bring-to-front commit: update in-memory state immediately, and
  // schedule a throttled disk write of just the manifest (cheap path).
  const onManifestChange = useCallback(
    (next: DesignManifest) => {
      setProject((prev) => ({ ...prev, manifest: next }));
      pendingManifest.current = next;
      setPendingDirty(true);
      if (writeTimer.current === null) {
        writeTimer.current = setTimeout(flushManifest, MANIFEST_WRITE_THROTTLE_MS);
      }
    },
    [flushManifest],
  );

  // Flush any pending manifest write on unmount so a final drag isn't lost.
  useEffect(() => {
    return () => {
      if (writeTimer.current !== null) {
        clearTimeout(writeTimer.current);
        flushManifest();
      }
    };
  }, [flushManifest]);

  // Upsert this project into the recent-projects registry. Declared early (before the
  // seed/save flows that call it) to avoid a TDZ in their dependency arrays.
  // `contractSha` (Fix 3): supplied ONLY by the contract Save path to RECORD the approved
  // hash. Omitted on load/create remembers — the backend upsert PRESERVES any existing
  // recorded hash when `contractSha` is absent (never wipes it on a plain reopen).
  const rememberProject = useCallback(
    async (workingFolderPath: string, name: string, contractSha?: string) => {
      if (!tauri || !workingFolderPath.trim()) return;
      try {
        const list = await invokeBackendCommand<DesignProjectEntry[]>(
          "design_registry_remember",
          {
            entry: {
              id: "",
              name,
              workingFolderPath: workingFolderPath.trim(),
              createdAt: "",
              updatedAt: "",
              lastOpenedAt: "",
              ...(contractSha ? { contractSha } : {}),
            },
          },
        );
        setRecent(list ?? []);
      } catch {
        // Non-fatal: remembering is a convenience.
      }
    },
    [tauri],
  );

  // Persist a clean (empty) DTCG token stub. PRESERVES the legacy guarantee that the
  // seed flow always leaves a valid tokens.json behind when the user provides no
  // contract (Skip): a target with no extractable tokens still gets a tokens.json.
  // WARNING 3: the stub carries NO Oracle-derived file paths.
  const writeCleanTokenStub = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    setTokens({});
    if (!folderPath || !tauri) return;
    await invokeBackendCommand("design_write_tokens", {
      workingFolderPath: folderPath,
      tokensJson: JSON.stringify({}, null, 2),
    }).catch(() => {
      // Non-fatal: tokens persistence failing must not break load.
    });
  }, [tauri]);

  // Seed flow (Phase C). State machine, run after create/open:
  //   (i)  read design.md -> PRESENT: re-hash it and check provenance (Fix 3):
  //          - hash MATCHES the recorded approved hash -> stash for injection, no editor.
  //          - MISMATCH or NO recorded hash -> do NOT stash/inject; open the editor
  //            prefilled with the on-disk content + a REVIEW notice. Save re-approves;
  //            Skip leaves design.md on disk but UNINJECTED for the session.
  //   (ii) MISSING -> probe Oracle for design signal:
  //          - chunks found -> extract REAL tokens + build a REVIEW-FIRST draft, open
  //            the editor prefilled (Save writes design.md AND the extracted tokens).
  //          - no chunks    -> open the editor with an empty draft (preset-picker mode).
  // NOTHING here writes design.md or tokens.json — only the editor's explicit Save (or
  // the Skip-fallback clean stub) writes. WARNING 3 preserved: the draft (which may
  // quote target source) is shown to the user and only persisted on Save.
  //
  // Fix 4: every state-applying step is gated on the SEED EPOCH captured at entry, so a
  // slow seed for project A that resolves after a switch to B cannot apply A's result.
  const seedContract = useCallback(async () => {
    const epoch = seedEpochRef.current;
    const stale = () => seedEpochRef.current !== epoch;
    const folderPath = folderRef.current.trim();
    contractRef.current = "";
    setContractEditor(null);
    if (!folderPath || !tauri) {
      setTokens({});
      return;
    }

    // (i) Existing contract: re-hash + provenance check before any injection.
    let existing: string | null = null;
    try {
      existing = await invokeBackendCommand<string | null>(
        "design_read_design_md",
        { workingFolderPath: folderPath },
      );
    } catch {
      existing = null;
    }
    if (stale()) return;
    if (typeof existing === "string" && existing.trim().length > 0) {
      const onDiskHash = await sha256Hex(existing);
      if (stale()) return;
      // V10: ensure the initial registry list has settled before the recorded-hash lookup.
      // On a cold start the mount-time load may still be in flight; awaiting it (then the
      // epoch re-check below) prevents a recorded contract from being mis-flagged as an
      // out-of-band change just because recentRef hadn't been populated yet.
      if (registryReadyRef.current) {
        await registryReadyRef.current;
        if (stale()) return;
      }
      const recordedHash = findRecordedSha(recentRef.current, folderPath);
      if (recordedHash && recordedHash === onDiskHash) {
        // Approved + unchanged: stash for injection, skip the editor.
        contractRef.current = existing;
      } else {
        // Out-of-band change (or never approved): open the editor to REVIEW. Do NOT
        // stash — nothing is injected until the user Saves (re-approves). This is NOT a
        // seed flow, so Skip must not touch tokens.json.
        setContractEditor({
          initialContent: existing,
          draftTokens: undefined,
          fromSeedFlow: false,
          notice:
            "This contract changed outside the editor — review before it is used.",
        });
      }
      return;
    }

    // (ii) Missing: probe Oracle for a draft. The probe NEVER throws fatally.
    let chunks: DesignContextChunk[] = [];
    try {
      chunks =
        (await invokeBackendCommand<DesignContextChunk[]>("design_oracle_context", {
          workingFolderPath: folderPath,
          query: "design tokens palette colors typography spacing theme",
          limit: 8,
        })) ?? [];
    } catch {
      chunks = [];
    }
    if (stale()) return;

    if (chunks.length > 0) {
      const extracted = extractTokensFromChunks(chunks);
      const draft = buildDesignMdDraft(chunks, extracted);
      const hasTokens = Object.keys(extracted).length > 0;
      setContractEditor({
        initialContent: draft,
        draftTokens: hasTokens ? extracted : undefined,
        fromSeedFlow: true,
      });
    } else {
      // No grounding signal: open the editor in preset-picker mode (empty draft).
      setContractEditor({
        initialContent: "",
        draftTokens: undefined,
        fromSeedFlow: true,
      });
    }
  }, [tauri]);

  // Contract editor SAVE: the ONLY path that writes design.md / tokens.json from the
  // editor. Fix 5: the design.md write happens FIRST and, on failure, the editor stays
  // OPEN with an inline error and NEITHER the in-memory stash NOR the approved registry
  // hash is updated (no data loss, no false provenance). Only after a successful write
  // (or for empty content, which clears the contract) do we stash, record the approved
  // SHA-256 (Fix 3), persist tokens, and close.
  const onContractSave = useCallback(
    async (content: string, tokensDoc: DtcgDocument | undefined) => {
      const folderPath = folderRef.current.trim();
      const trimmed = content.trim();
      const hasContract = trimmed.length > 0;

      // 1) Persist design.md FIRST when there is content. On failure: keep the editor
      //    open with the error, do not touch contractRef/hash. The user can retry Save.
      if (folderPath && tauri && hasContract) {
        try {
          await invokeBackendCommand("design_write_design_md", {
            workingFolderPath: folderPath,
            content,
          });
        } catch (e) {
          setContractEditor((prev) =>
            prev ? { ...prev, saveError: String(e) } : prev,
          );
          return; // editor STAYS open; no optimistic stash/hash update.
        }
      }

      // 2) Write succeeded (or there is no content). NOW adopt the contract for this
      //    session — empty content CLEARS the stash.
      contractRef.current = hasContract ? trimmed : "";

      // 3) Record the approved hash so a later load injects this contract without a
      //    review prompt (Fix 3). Hash the EXACT saved string (what reload reads back).
      //    Only when we actually wrote a non-empty contract to a real folder.
      if (folderPath && tauri && hasContract) {
        try {
          const sha = await sha256Hex(content);
          await rememberProject(folderPath, projectRef.current.meta.name, sha);
        } catch {
          // Non-fatal: a missing recorded hash only means the NEXT load re-prompts a
          // review — it never corrupts data or injects an unapproved contract.
        }
      }

      // 4) Persist accompanying tokens (extracted draft or chosen preset).
      if (tokensDoc && isValidTokensDoc(tokensDoc)) {
        setTokens(tokensDoc);
        if (folderPath && tauri) {
          await invokeBackendCommand("design_write_tokens", {
            workingFolderPath: folderPath,
            tokensJson: JSON.stringify(tokensDoc, null, 2),
          }).catch((e) => setError(String(e)));
        }
      }
      setContractEditor(null);
    },
    [tauri, rememberProject],
  );

  // Contract editor SKIP: writes NOTHING to design.md. Fix 2: the clean empty
  // tokens.json fallback is written ONLY when the editor was opened by the SEED flow
  // (a brand-new project with no contract yet). A MANUAL open, or the provenance-review
  // path, must NEVER clobber the project's existing tokens.json on Skip.
  const onContractSkip = useCallback(() => {
    const fromSeedFlow = contractEditor?.fromSeedFlow ?? false;
    setContractEditor(null);
    if (fromSeedFlow) void writeCleanTokenStub();
  }, [contractEditor, writeCleanTokenStub]);

  // Manual entry point (ProjectPopover "Design contract…" row): load the current
  // design.md into the editor for editing, or build a fresh draft from Oracle when
  // none exists. Unlike the seed flow, Skip here does NOT touch tokens.json.
  const openContractEditor = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    if (!folderPath || !tauri) return;
    let existing: string | null = null;
    try {
      existing = await invokeBackendCommand<string | null>(
        "design_read_design_md",
        { workingFolderPath: folderPath },
      );
    } catch {
      existing = null;
    }
    if (typeof existing === "string" && existing.trim().length > 0) {
      // Manual open of an existing contract: NOT a seed flow (Skip must not write the
      // token stub). This is the user's own action so no provenance review is needed.
      setContractEditor({
        initialContent: existing,
        draftTokens: undefined,
        fromSeedFlow: false,
      });
      return;
    }
    let chunks: DesignContextChunk[] = [];
    try {
      chunks =
        (await invokeBackendCommand<DesignContextChunk[]>("design_oracle_context", {
          workingFolderPath: folderPath,
          query: "design tokens palette colors typography spacing theme",
          limit: 8,
        })) ?? [];
    } catch {
      chunks = [];
    }
    const extracted = chunks.length > 0 ? extractTokensFromChunks(chunks) : {};
    const draft =
      chunks.length > 0 ? buildDesignMdDraft(chunks, extracted) : "";
    setContractEditor({
      initialContent: draft,
      draftTokens: Object.keys(extracted).length > 0 ? extracted : undefined,
      fromSeedFlow: false,
    });
  }, [tauri]);

  // Best-effort refresh of the topbar Oracle chip status after a load. Never errors
  // (the Rust command returns `{grounded:false}` on any failure). Web runtime -> not
  // grounded.
  const refreshOracleStatus = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    if (!folderPath || !tauri) {
      setOracleStatus({ grounded: false });
      return;
    }
    try {
      const st = await invokeBackendCommand<DesignOracleStatus>(
        "design_oracle_status",
        { workingFolderPath: folderPath },
      );
      setOracleStatus(st ?? { grounded: false });
    } catch {
      setOracleStatus({ grounded: false });
    }
  }, [tauri]);

  // Open the NATIVE OS directory picker and store the chosen absolute path. Returns
  // the picked path (or null on cancel/unavailable) so a caller can chain create/load.
  const pickFolder = useCallback(async (): Promise<string | null> => {
    let picked: string | null = null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const result = await open({
        directory: true,
        multiple: false,
        title: "Select the design working folder",
      });
      if (typeof result === "string" && result.trim()) picked = result;
    } catch {
      // Dialog plugin unavailable or user dismissed — no-op.
    }
    if (picked === null) return null;
    setFolder(picked);
    setError(null);
    return picked;
  }, []);

  // --- Phase 3: recent-projects registry ------------------------------------

  useEffect(() => {
    if (!tauri) return;
    let cancelled = false;
    // V10: expose this load as a promise seedContract can await so a cold-start project load
    // sees the recorded contract hash. Resolves on success OR failure (the recent list is a
    // convenience — a failed load must not block the seed forever).
    registryReadyRef.current = (async () => {
      try {
        const list = await invokeBackendCommand<DesignProjectEntry[]>(
          "design_registry_list",
          {},
        );
        if (!cancelled) {
          // V10: update the REF synchronously (not only the state) so a seedContract that
          // awaited this promise reads the fresh list immediately, without waiting for the
          // setRecent-driven re-render to mirror it into recentRef on commit.
          recentRef.current = list ?? [];
          setRecent(list ?? []);
        }
      } catch {
        // Non-fatal: the recent list is a convenience; leave it empty.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauri]);

  // Create a project in the given folder (seeds the demo nodes, saves, remembers).
  // Shared by the ProjectPopover "New project…" row (which picks a folder first).
  const createInFolder = useCallback(
    async (folderPath: string) => {
      const path = folderPath.trim();
      if (!path) {
        setError("Choose a working folder first.");
        return;
      }
      // Fix 4: open a new seed epoch so a still-running seed from a previous project
      // cannot apply its result over this one.
      seedEpochRef.current += 1;
      setBusy("create");
      setError(null);
      setStatus(null);
      // Adopt the folder as the working folder (so the path chip + saves target it).
      setFolder(path);
      folderRef.current = path;
      try {
        const created = await invokeBackendCommand<DesignProject>(
          "design_create_project",
          { workingFolderPath: path, name: "Demo landing" },
        );
        const demo = buildDemoProject();
        const seeded: DesignProject = {
          ...created,
          manifest: demo.manifest,
          components: demo.components,
          meta: { ...created.meta, nodeOrder: demo.meta.nodeOrder },
        };
        await invokeBackendCommand("design_save_project", {
          workingFolderPath: path,
          project: seeded,
        });
        setProject(seeded);
        setHistoryValue(createHistory<DesignSnapshot>()); // new baseline — clear undo/redo
        setStatus("Created and seeded the project on disk.");
        showToast("Project created in the working folder");
        await rememberProject(path, seeded.meta.name);
        // A brand-new project has no design.md -> the seed flow opens the contract
        // editor (preset-picker or an extracted draft) so the user curates one.
        await seedContract();
        await refreshOracleStatus();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [rememberProject, setHistoryValue, showToast, seedContract, refreshOracleStatus],
  );

  // Core load flow shared by the picker and the recent-projects list.
  const loadFolder = useCallback(
    async (path: string, fromRecentId?: string) => {
      const folderPath = path.trim();
      if (!folderPath) {
        setError("Choose a working folder first.");
        return;
      }
      // Fix 4: open a new seed epoch so a still-running seed from a previous project
      // cannot apply its result over this load.
      seedEpochRef.current += 1;
      setBusy("load");
      setError(null);
      setStatus(null);
      setFolder(folderPath);
      // Update the live folder ref SYNCHRONOUSLY too: seedContract / refreshOracleStatus
      // (awaited below) read `folderRef.current`, but the React render that mirrors
      // `folder` into the ref hasn't run yet in this chained flow (pick -> load),
      // so without this they would see a stale/empty path and skip the seed.
      folderRef.current = folderPath;
      try {
        const loaded = await invokeBackendCommand<DesignProject>(
          "design_load_project",
          { workingFolderPath: folderPath },
        );
        if (!loaded || !loaded.meta || !loaded.manifest) {
          throw new Error("design_load_project returned no project");
        }
        setProject(loaded);
        setHistoryValue(createHistory<DesignSnapshot>()); // new baseline — clear undo/redo
        shapesRef.current = deriveShapes(loaded.components);
        setStatus(
          loaded.warnings && loaded.warnings.length > 0
            ? `Loaded with ${loaded.warnings.length} warning(s).`
            : "Loaded from disk.",
        );
        await seedContract();
        await rememberProject(folderPath, loaded.meta.name);
        // Persist the last-opened folder so the next cold start restores it (see the mount effect).
        try {
          localStorage.setItem("devboule.design.lastFolder", folderPath);
        } catch {
          /* localStorage unavailable — non-fatal */
        }
        await refreshOracleStatus();
      } catch (e) {
        setError(
          fromRecentId
            ? `${String(e)} — the folder may have moved; use Remove on the entry to prune it.`
            : String(e),
        );
      } finally {
        setBusy(null);
      }
    },
    [seedContract, rememberProject, setHistoryValue, refreshOracleStatus],
  );

  // ProjectPopover footer actions: pick a folder, then create / load it.
  const onNewProject = useCallback(async () => {
    const picked = await pickFolder();
    if (picked) await createInFolder(picked);
  }, [pickFolder, createInFolder]);

  const onOpenFolder = useCallback(async () => {
    const picked = await pickFolder();
    if (picked) await loadFolder(picked);
  }, [pickFolder, loadFolder]);

  // --- Phase 3: recent-projects rename / remove ------------------------------

  const beginRename = useCallback((entry: DesignProjectEntry) => {
    setRenamingId(entry.id);
    setRenameDraft(entry.name);
  }, []);

  const cancelRename = useCallback(() => {
    setRenamingId(null);
    setRenameDraft("");
  }, []);

  const commitRename = useCallback(
    async (id: string) => {
      const name = renameDraft.trim();
      if (!name) {
        cancelRename();
        return;
      }
      try {
        const list = await invokeBackendCommand<DesignProjectEntry[]>(
          "design_registry_rename",
          { id, name },
        );
        setRecent(list ?? []);
      } catch (e) {
        setError(String(e));
      } finally {
        cancelRename();
      }
    },
    [renameDraft, cancelRename],
  );

  const removeEntry = useCallback(async (id: string) => {
    try {
      const list = await invokeBackendCommand<DesignProjectEntry[]>(
        "design_registry_remove",
        { args: { id, removeFiles: false } },
      );
      setRecent(list ?? []);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const openEntry = useCallback(
    (entry: DesignProjectEntry) => {
      void loadFolder(entry.workingFolderPath, entry.id);
    },
    [loadFolder],
  );

  // BUGFIX (P0): restore the LAST-OPENED project on cold start so Design isn't empty every launch.
  // Keyed on a persisted last-folder (written by loadFolder on a successful open), NOT on "the
  // registry has entries" — so merely having known projects never force-opens one. Guarded by
  // folderRef so it can't clobber a project the user already opened (or a deep-link).
  useEffect(() => {
    if (!tauri || folderRef.current) return;
    let saved: string | null = null;
    try {
      saved = localStorage.getItem("devboule.design.lastFolder");
    } catch {
      saved = null;
    }
    if (saved) void loadFolder(saved);
    // Run once on mount; loadFolder is a stable useCallback.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri]);

  // Consolidate the whole design project to disk. Returns TRUE on a successful save,
  // FALSE on any failure (no folder or backend error). The hand-off packaging flow
  // relies on this boolean to HARD-STOP the save step on a failed write (BLOCKER): a
  // swallowed save error must not mark the step done and let a stale bundle dispatch.
  // Toolbar/SaveMenu callers ignore the result (best-effort save with on-screen status),
  // matching the existing `void runConsolidate()` call sites.
  const runConsolidate = useCallback(async (): Promise<boolean> => {
    const folderPath = folderRef.current.trim();
    if (!folderPath) {
      setError("Choose a working folder first.");
      return false;
    }
    // V5c: re-entry guard — a consolidate already in flight wins; the second call returns
    // false without issuing a duplicate design_save_project write.
    if (consolidatingRef.current) return false;
    consolidatingRef.current = true;
    // V5b: cancel any throttled drag-commit manifest write and drop its pending payload
    // BEFORE we snapshot+save the whole project (mirrors persistProject). Otherwise the
    // queued writeTimer could fire AFTER this save and overwrite the freshly-consolidated
    // manifest on disk with the older, throttled manifest (stale-manifest overwrite).
    if (writeTimer.current !== null) {
      clearTimeout(writeTimer.current);
      writeTimer.current = null;
    }
    pendingManifest.current = null;
    setPendingDirty(false);
    setBusy("save");
    setError(null);
    setStatus(null);
    beginSaving();
    try {
      await invokeBackendCommand("design_save_project", {
        workingFolderPath: folderPath,
        project: projectRef.current,
      });
      setStatus("Consolidated the whole project to disk.");
      showToast("Saved to working folder");
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      endSaving();
      setBusy(null);
      // V5c: release the re-entry guard on EVERY exit path (success or error).
      consolidatingRef.current = false;
    }
  }, [beginSaving, endSaving, showToast]);

  // Export the current project to standalone HTML (absolute or flow) and write it to
  // the working folder via the path-confined Rust command. Returns TRUE on a successful
  // write, FALSE on any failure (validation or backend error). The preview flow relies on
  // this boolean to NEVER open a window over a stale/missing export (BLOCKER): a swallowed
  // export error must not let openPreview proceed to design_preview_open. Toolbar callers
  // ignore the result (best-effort export with on-screen status).
  const runExport = useCallback(
    async (mode: ExportMode): Promise<boolean> => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) {
        setError("Choose a working folder first.");
        return false;
      }
      setBusy("export");
      setError(null);
      setStatus(null);
      try {
        // BLOCKER 1: sanitize every disk-loaded component before inlining it (the
        // canvas re-sanitizes on inject; export must too).
        const src = projectRef.current;
        const safeComponents: Record<string, string> = {};
        for (const [id, markup] of Object.entries(src.components)) {
          safeComponents[id] = sanitizeNodeMarkup(markup);
        }
        const safeProject: DesignProject = { ...src, components: safeComponents };
        const content = exportCode(safeProject, mode);
        const filename =
          mode === "absolute" ? "export-absolute.html" : "export-flow.html";
        await invokeBackendCommand("design_write_export", {
          workingFolderPath: folderPath,
          filename,
          content,
        });
        setStatus(`Exported ${mode} layout to ${filename}.`);
        showToast(`Exported to ${filename}`);
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      } finally {
        setBusy(null);
      }
    },
    [tauri, showToast],
  );

  // Re-save the current DTCG token document to tokens.json via the existing path.
  const exportTokens = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    if (!folderPath || !tauri) {
      setError("Choose a working folder first.");
      return;
    }
    setBusy("export");
    setError(null);
    setStatus(null);
    try {
      await invokeBackendCommand("design_write_tokens", {
        workingFolderPath: folderPath,
        tokensJson: JSON.stringify(tokensRef.current, null, 2),
      });
      setStatus("Exported design tokens to tokens.json.");
      showToast("Exported tokens to tokens.json");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [tauri, showToast]);

  // --- Phase B: preview window / visual check --------------------------------

  // Best-effort: record the freshly-captured preview.png as the project thumbnail in
  // the registry. The path is RELATIVE ("preview.png") — ProjectPopover renders it by
  // lazily reading design_read_thumbnail (a data: URI), so an absolute path would never
  // resolve under the CSP img-src 'self' data:. contractSha is intentionally OMITTED so
  // the Rust upsert PRESERVES any approved contract hash (the preserve-when-None rule).
  const rememberThumbnail = useCallback(
    (workingFolderPath: string) => {
      if (!tauri || !workingFolderPath.trim()) return;
      invokeBackendCommand<DesignProjectEntry[]>("design_registry_remember", {
        entry: {
          id: "",
          name: projectRef.current.meta.name,
          workingFolderPath: workingFolderPath.trim(),
          createdAt: "",
          updatedAt: "",
          lastOpenedAt: "",
          thumbnailPath: "preview.png",
        },
      })
        .then((list) => setRecent(list ?? []))
        .catch(() => {
          // Non-fatal: the thumbnail is a convenience; a critique still proceeds.
        });
    },
    [tauri],
  );

  const preview = usePreview({
    getFolder: () => folderRef.current,
    tauri,
    invoke: invokeBackendCommand,
    runExport,
    rememberThumbnail,
    onToast: showToast,
  });

  // Phase D — the "Save & hand off to agents" flow. The hook owns the phase machine
  // (packaging -> dispatch -> done), the per-step rows, the project/client selection,
  // the SINGLE launch_project_agent_terminal dispatch (host "app", role "coder", a
  // designHandoff payload), and the Work-mode deep-link out. DesignView only supplies
  // the real callbacks + the app project list; the modal below is presentational.
  const handoff = useHandoff({
    workingFolderPath: folder,
    projects: handoffProjects,
    backendKind: backend?.kind ?? null,
    runConsolidate,
    runExport,
    invoke: invokeBackendCommand,
    onOpenTerminal: (projectId) => {
      // Deep-link to the project's Work-mode terminal via the same pendingTab format
      // ProjectsView consumes (parseWorkTab -> work:<projectId>). The launched agent
      // appears in that project's Work-mode agent list. Degrades to no-op if the app
      // context is absent (tests without AppProvider).
      requestView?.("projects", `work:${projectId}`);
    },
  });

  // Open the hand-off modal: kick off the project load AND open in the same tick. The
  // load is ASYNCHRONOUS (fire-and-forget) — it does NOT populate the selector before
  // openHandoff runs, so openHandoff's own preselection may see an empty/stale list. That
  // is fine: when the list lands it updates `handoffProjects`, the hook re-renders, and
  // its preselect effect fills the still-null selection (without overriding a user pick).
  // So the selection settles once the list arrives, a render or two after open.
  const openHandoff = useCallback(() => {
    void loadHandoffProjects();
    handoff.openHandoff();
  }, [loadHandoffProjects, handoff]);

  // Visual check (assist-head icon-button): push a user-style chip + a working card,
  // run the capture→critique flow, then patch the card to done (the critique text) or
  // error (the backend's clean message). Reuses the existing assistant message model.
  const onVisualCheck = useCallback(async () => {
    // SYNCHRONOUS re-entry guard BEFORE pushing anything. beginCheck() claims the slot in
    // the same tick (a ref, not the async `checking` state), so two rapid clicks can't both
    // claim — the second returns false and pushes NO cards. This avoids ghost user-chip +
    // working-card pairs while keeping the card appearing immediately for the real click.
    if (!preview.beginCheck()) return;
    pushMessage({ role: "user", text: "Visual check", ctx: "Visual check" });
    const msgId = pushMessage({
      role: "assistant",
      status: "working",
      title: "Reviewing the preview…",
      desc: "Capturing the preview window and asking the local AI to critique it…",
    });
    // visualCheck adopts the claim beginCheck just made (it won't skip) and releases it.
    const outcome = await preview.visualCheck();
    if (outcome.kind === "ok") {
      patchMessage(msgId, {
        status: "done",
        title: "Visual critique",
        desc: outcome.critique,
      });
    } else if (outcome.kind === "error") {
      patchMessage(msgId, {
        status: "error",
        title: "Visual check failed",
        desc: outcome.message,
      });
    } else {
      // skipped: should not happen (we hold the claim), but stay defensive — drop the card
      // so it never spins forever.
      patchMessage(msgId, {
        status: "error",
        title: "Visual check in progress",
        desc: "A visual check is already running.",
      });
    }
  }, [preview, pushMessage, patchMessage]);

  // --- Generation / edit flow ------------------------------------------------

  const persistProject = useCallback(
    (next: DesignProject) => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) return;
      // W4: cancel any pending drag-commit manifest write so it can't clobber this
      // whole-project save afterwards.
      if (writeTimer.current !== null) {
        clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
      pendingManifest.current = null;
      setPendingDirty(false);
      beginSaving();
      invokeBackendCommand("design_save_project", {
        workingFolderPath: folderPath,
        project: next,
      })
        .catch((e) => setError(String(e)))
        .finally(endSaving);
    },
    [tauri, beginSaving, endSaving],
  );

  // --- interactive edit history (undo/redo) ---------------------------------

  const onBeginChange = useCallback(() => {
    setHistoryValue(pushHistory(historyRef.current, snapshotOf(projectRef.current)));
  }, [setHistoryValue]);

  const onProjectChange = useCallback(
    (next: DesignProject) => {
      setProject(next);
      persistProject(next);
    },
    [persistProject],
  );

  const applySnapshot = useCallback(
    (snap: DesignSnapshot) => {
      const next: DesignProject = {
        ...projectRef.current,
        manifest: snap.manifest,
        components: snap.components,
        meta: { ...projectRef.current.meta, nodeOrder: snap.nodeOrder },
      };
      setProject(next);
      persistProject(next);
    },
    [persistProject],
  );

  const undo = useCallback(() => {
    const res = undoHistory(historyRef.current, snapshotOf(projectRef.current));
    if (!res) return;
    applySnapshot(res.value);
    setHistoryValue(res.history);
  }, [applySnapshot, setHistoryValue]);

  const redo = useCallback(() => {
    const res = redoHistory(historyRef.current, snapshotOf(projectRef.current));
    if (!res) return;
    applySnapshot(res.value);
    setHistoryValue(res.history);
  }, [applySnapshot, setHistoryValue]);

  // Ctrl/Cmd+Z = undo, Ctrl/Cmd+Shift+Z or Ctrl+Y = redo. Ignored while focus is in
  // an input/textarea/contenteditable.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const target = e.target as HTMLElement | null;
      const tag = (target?.tagName ?? "").toLowerCase();
      if (tag === "input" || tag === "textarea" || target?.isContentEditable) {
        return;
      }
      // V7: ignore undo/redo while a generation/edit/Spot-Edit chain is live. Mutating the
      // project (applySnapshot + persist) mid-stream races the in-flight pipeline writing
      // into the SAME project — it can drop/duplicate the streamed node, clobber the manifest,
      // or persist a half-applied tree. We read the live `panelBusyRef` (not a captured state)
      // so this stable listener never sees a stale value.
      if (panelBusyRef.current) return;
      const key = e.key.toLowerCase();
      if (key === "z" && !e.shiftKey) {
        e.preventDefault();
        undo();
      } else if ((key === "z" && e.shiftKey) || (key === "y" && !e.shiftKey)) {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);

  const persistNode = useCallback(
    (next: DesignProject, nodeId: string) => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) return;
      if (writeTimer.current !== null) {
        clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
      pendingManifest.current = null;
      setPendingDirty(false);
      // WARNING 4: SERIALIZE the two writes — node markup FIRST, then the manifest.
      // V6 (ACCEPTED RISK): these are TWO separate Rust commands (design_write_node then
      // design_write_manifest), not one atomic operation. A runConsolidate that interleaves
      // between them (it clears the same writeTimer/pendingManifest, but these awaits are
      // already in flight) could observe a node file written without its matching manifest
      // entry yet — a brief, self-healing inconsistency: the next save/seed reconciles it,
      // and the node markup on disk is never lost. Closing the window fully would require a
      // single combined `design_write_node_and_manifest` Rust command + its tauri::command
      // registration (out of scope here — lib.rs is off-limits), so it is deferred.
      beginSaving();
      void (async () => {
        try {
          await invokeBackendCommand("design_write_node", {
            workingFolderPath: folderPath,
            nodeId,
            markup: next.components[nodeId] ?? "",
          });
          await invokeBackendCommand("design_write_manifest", {
            workingFolderPath: folderPath,
            manifest: next.manifest,
          });
        } catch (e) {
          setError(String(e));
        } finally {
          endSaving();
        }
      })();
    },
    [tauri, beginSaving, endSaving],
  );

  const appendGenerationLog = useCallback(
    (entry: GenerationLogEntry) => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) return;
      invokeBackendCommand("design_append_generation_log", {
        workingFolderPath: folderPath,
        entry,
      }).catch(() => {
        // Audit is best-effort: a log failure must never break the design flow.
      });
    },
    [tauri],
  );

  const readBackendKind = useCallback(async (): Promise<string> => {
    if (!tauri) return "unknown";
    try {
      const backend = await invokeBackendCommand<{ kind?: string } | null>(
        "get_design_llm_backend",
        {},
      );
      return backend?.kind ?? "unknown";
    } catch {
      return "unknown";
    }
  }, [tauri]);

  const runGenerate = useCallback(
    async (rawInstruction: string) => {
      const instruction = rawInstruction.trim();
      if (instruction.length === 0) return;
      if (!beginPrepare()) return;
      // Push the user prompt + a working assistant card up front (presentation only;
      // the pipeline below is unchanged). The card id is patched by the done-effect.
      pushMessage({ role: "user", text: instruction });
      const msgId = pushMessage({
        role: "assistant",
        status: "working",
        title: "Generating…",
        desc: "Grounding on the target codebase…",
        instruction,
      });
      activeMsgRef.current = msgId;
      try {
        const backendKind = await readBackendKind();
        const folderPath = folderRef.current.trim();
        const isHttpProvider = backendKind === "ollama" || backendKind === "omlx";
        const cli = isCliBackend(backendKind);

        const tokenNames = cli ? [] : tokenNamesForPrompt(tokensRef.current);

        let context = cli ? "" : buildGroundingBlock([], tokenNames);
        let oracleGrounded = false;
        let sources: string[] = [];
        if (tauri && folderPath && isHttpProvider) {
          try {
            const chunks = await invokeBackendCommand<DesignContextChunk[]>(
              "design_oracle_context",
              { workingFolderPath: folderPath, query: instruction, limit: 8 },
            );
            context = buildGroundingBlock(chunks ?? [], tokenNames);
            oracleGrounded = (chunks?.length ?? 0) > 0;
            // The src-chips show the retrieved file sources (deduped, bounded).
            sources = Array.from(
              new Set(
                (chunks ?? [])
                  .map((c) => c.fileSource)
                  .filter((s): s is string => typeof s === "string" && s.length > 0),
              ),
            ).slice(0, 8);
          } catch {
            // Degrade to the token-name-only block already in `context`.
          }
        }

        if (cli) context = "";

        // While the prompt streams, reflect the provider in the working card.
        patchMessage(msgId, {
          desc: cli
            ? "Grounding agentically via MCP…"
            : oracleGrounded
              ? "Streaming markup — grounded on the target codebase…"
              : "Streaming markup…",
          agentic: cli,
          sources: cli ? undefined : sources,
        });

        // TRUST (B4 sibling rule): the design.md contract is injected for ALL
        // providers — INCLUDING CLI ones that get no pre-fetched grounding block —
        // because it is USER-CURATED (only ever written via the contract editor's
        // explicit Save), unlike raw Oracle chunk text. Clamped to 16 KiB and fenced
        // as data in the prompt builder. Empty stash -> nothing injected.
        const designContract = clampDesignMd(contractRef.current);
        const fullPrompt = buildGeneratePrompt(instruction, {
          context,
          designContract,
        });
        repairRef.current = { instruction, context, attempts: 0 };
        consumedRef.current = false;
        pendingRunRef.current = {
          mode: "generate",
          meta: {
            startedAt: Date.now(),
            backendKind,
            promptChars: fullPrompt.length,
            oracleGrounded,
            msgId,
            instruction,
            sources,
            agentic: cli,
          },
        };
        startStream(fullPrompt, folderPath || undefined);
      } catch {
        // Swallow: a prepare throw is surfaced on the working card by the finally guard
        // below (the card is flipped to an error state), so it must not escape as an
        // unhandled rejection from the fire-and-forget caller.
      } finally {
        endPrepare();
        // If we exited prepare WITHOUT arming a run (a throw/early-fail before the
        // stream started), the working card would spin forever — the terminal
        // cancel/error effect only fires for an actual stream. Flip it to error here.
        // Guard on pendingRunRef === null so we never touch a card whose stream DID start.
        if (pendingRunRef.current === null && activeMsgRef.current !== null) {
          const failedId = activeMsgRef.current;
          activeMsgRef.current = null;
          patchMessage(failedId, {
            status: "error",
            title: "Failed to start",
            desc: "Could not start generation. Check Settings → Providers.",
          });
        }
      }
    },
    [pushMessage, patchMessage, startStream, tauri, readBackendKind, beginPrepare, endPrepare],
  );

  const launchRepair = useCallback(
    (
      committedNodeCount: number,
      remainingViolations: GenerationResult["remainingViolations"],
      meta: RunMeta,
    ): boolean => {
      const state = repairRef.current;
      if (!state) return false;
      const repairContext = isCliBackend(meta.backendKind) ? "" : state.context;
      const repaired = buildRepairPrompt(
        state.instruction,
        { committedNodeCount, remainingViolations },
        repairContext,
        // Fix 8: read the LIVE contract at repair time (clamped), not a value snapshotted
        // at generate start. The contract is carried into the repair for ALL providers
        // (user-curated, same trust class as the instruction — see runGenerate's note).
        clampDesignMd(contractRef.current),
      );
      if (repaired === null) return false;

      state.attempts += 1;
      consumedRef.current = false;
      // Keep the SAME assistant card across the repair loop; update its desc to the
      // prototype-style "Refining: …" with a short violation summary.
      const violationSummary =
        committedNodeCount === 0
          ? "no usable markup"
          : remainingViolations
              .map((v) => v.message)
              .filter(Boolean)
              .slice(0, 2)
              .join("; ") || "invalid markup";
      patchMessage(meta.msgId, {
        status: "working",
        desc: `Refining: ${violationSummary}…`,
      });
      pendingRunRef.current = {
        mode: "generate",
        meta: {
          startedAt: meta.startedAt,
          backendKind: meta.backendKind,
          promptChars: repaired.length,
          oracleGrounded: meta.oracleGrounded,
          msgId: meta.msgId,
          instruction: meta.instruction,
          sources: meta.sources,
          agentic: meta.agentic,
        },
      };
      startStream(repaired, folderRef.current.trim() || undefined);
      return true;
    },
    [startStream, patchMessage],
  );

  const runEdit = useCallback(
    async (rawInstruction: string, targetNodeId?: string) => {
      const id = targetNodeId ?? selectedId;
      const instruction = rawInstruction.trim();
      if (!id || instruction.length === 0) return;
      if (!beginPrepare()) return;
      const nodeName = id; // node ids are the user-facing label in this module
      pushMessage({
        role: "user",
        text: instruction,
        ctx: `Editing ${nodeName}`,
      });
      const msgId = pushMessage({
        role: "assistant",
        status: "working",
        title: `Updating ${nodeName}…`,
        desc: "Sending only this node's markup; placement stays untouched.",
        instruction,
        editNodeId: id,
      });
      activeMsgRef.current = msgId;
      try {
        const current = projectRef.current.components[id] ?? "";
        const backendKind = await readBackendKind();
        const cli = isCliBackend(backendKind);
        patchMessage(msgId, { agentic: cli });
        // B4 EXEMPTION: `current` is the node's EXISTING canvas markup, embedded in the edit
        // prompt for ALL providers (including CLI/agentic ones). This is NOT a B4 violation:
        // the B4 gate bars only UN-REVIEWED TARGET TEXT (raw Oracle chunk text that a CLI
        // provider could be steered by). Node markup is the user's own canvas working
        // material — visible and editable in the canvas, the same trust class as the
        // user-approved design.md contract — not fetched, untrusted target source. So it is
        // safe to send agentically; the gate's concern (prompt-injection via un-reviewed
        // retrieved code) does not apply here.
        // The design contract grounds edits too (all providers — user-curated, clamped).
        const editPrompt = buildEditPrompt(current, instruction, {
          designContract: clampDesignMd(contractRef.current),
        });
        consumedRef.current = false;
        pendingRunRef.current = {
          mode: "edit",
          nodeId: id,
          meta: {
            startedAt: Date.now(),
            backendKind,
            promptChars: editPrompt.length,
            oracleGrounded: false,
            msgId,
            instruction,
            sources: [],
            agentic: cli,
          },
        };
        startStream(editPrompt, folderRef.current.trim() || undefined);
      } catch {
        // Swallow: surfaced on the card by the finally guard below; never escape as an
        // unhandled rejection from the fire-and-forget caller.
      } finally {
        endPrepare();
        // Same stuck-card guard as runGenerate: a throw before the stream armed
        // would otherwise leave this edit card spinning forever.
        if (pendingRunRef.current === null && activeMsgRef.current !== null) {
          const failedId = activeMsgRef.current;
          activeMsgRef.current = null;
          patchMessage(failedId, {
            status: "error",
            title: "Failed to start",
            desc: "Could not start the edit. Check Settings → Providers.",
          });
        }
      }
    },
    [selectedId, pushMessage, patchMessage, startStream, readBackendKind, beginPrepare, endPrepare],
  );

  // --- content-edit (CE) commit ---------------------------------------------
  // The canvas serialized an edited node's content root (helper classes/attrs already
  // stripped) and hands us the RAW markup. We RE-SANITIZE it (the chokepoint — CE
  // edited live DOM, an injected onerror/class must not survive), re-stamp the node
  // id onto the root (a reorder/delete could have changed which element is first),
  // refresh the sanitizer kind + structural shape, push history, and persist via the
  // EXISTING node-persist path (node markup first, then manifest) — identical to the
  // single-node edit pipeline's apply branch.
  const onNodeMarkupCommit = useCallback(
    (nodeId: string, rawSerialized: string) => {
      const prev = projectRef.current;
      const placement = prev.manifest.nodes[nodeId];
      if (!placement) return; // node vanished — nothing to commit (CE already aborts)
      // A node is a SINGLE top-level element, but CE can leave MULTIPLE roots behind
      // (e.g. the user deleted the wrapping element, hoisting its children to the top
      // level). `applyNodeId` only stamps + keeps `body.firstElementChild`, so the 2nd+
      // roots would be SILENTLY DROPPED. Detect >1 root and wrap them in one `<div>` so
      // all the content is preserved under a single re-anchorable root.
      const roots = parseTopLevelNodes(rawSerialized);
      const serialized =
        roots.length > 1 ? `<div>${rawSerialized}</div>` : rawSerialized;
      // Stamp the id onto the (now single) top-level element, then sanitize (chokepoint).
      const markup = sanitizeNodeMarkup(applyNodeId(serialized, nodeId));
      if (markup.length === 0) return; // empty/garbage serialization — ignore
      // No-op check: compare NORMALIZED forms (both run through applyNodeId+sanitize) so
      // a pure click (or cosmetic whitespace/attr-order drift from the serializer) does
      // not push history or write to disk. Comparing raw `markup` against the stored
      // string directly was fragile — the stored value is already normalized, but a
      // freshly-stored node and a re-serialized identical edit could differ only in
      // normalization, causing spurious churn.
      const prevNormalized = sanitizeNodeMarkup(
        applyNodeId(prev.components[nodeId] ?? "", nodeId),
      );
      if (markup === prevNormalized) return; // no net change

      // Refresh kind + shape from the committed markup (an edit may swap html<->svg).
      const reparsed = parseTopLevelNodes(markup);
      const kind = reparsed.length > 0 && reparsed[0].tag === "svg" ? "svg" : "html";

      const next: DesignProject = {
        ...prev,
        manifest: {
          ...prev.manifest,
          nodes: {
            ...prev.manifest.nodes,
            [nodeId]: { ...placement, kind },
          },
        },
        components: { ...prev.components, [nodeId]: markup },
      };

      onBeginChange(); // snapshot BEFORE applying so undo restores the pre-CE markup
      setProject(next);
      if (reparsed.length > 0) {
        shapesRef.current = { ...shapesRef.current, [nodeId]: reparsed[0] };
      }
      persistNode(next, nodeId);
      setStatus(`Updated node "${nodeId}" (content edit).`);
    },
    [onBeginChange, persistNode],
  );

  // Seed the composer with a node's CE "Ask AI" context: select the node (so a send
  // routes to its edit round-trip) + prefill the draft + focus the composer.
  const onSeedComposer = useCallback(
    ({ nodeId, tag }: { nodeId: string; tag: string }) => {
      if (!projectRef.current.manifest.nodes[nodeId]) return;
      setSelectedId(nodeId);
      setDraft(tag ? `Update this ${tag} element: ` : "");
      setFocusSignal((v) => v + 1);
    },
    [],
  );

  // --- Spot Edit: sequential per-node edit chain ----------------------------
  // True while the Spot Edit chain is running (drives the region shimmer + locks
  // re-entry). A chain runs the EXISTING `runEdit` ONE node at a time: each next run
  // starts only after the previous stream reaches a terminal transition (the W3
  // single-stream invariant — never two streams at once). A Stop cancels the chain.
  const [spotBusy, setSpotBusy] = useState(false);
  const spotChainRef = useRef<{ queue: string[]; prompt: string } | null>(null);

  // Set when a microtask advance is already scheduled, so one terminal transition
  // schedules exactly one advance even if multiple effects observe it.
  const spotAdvanceScheduledRef = useRef(false);

  // Start the next node in the chain, or finish (clear busy) when the queue is empty.
  const advanceSpotChain = useCallback(() => {
    spotAdvanceScheduledRef.current = false;
    const chain = spotChainRef.current;
    if (!chain) return;
    // Re-check the single-stream invariant at advance time: if a run is still owed or
    // a prepare is in flight, defer one more microtask (the done-effect may not have
    // consumed yet). Never start a second stream concurrently (W3).
    if (pendingRunRef.current !== null || preparingRef.current) {
      if (!spotAdvanceScheduledRef.current) {
        spotAdvanceScheduledRef.current = true;
        queueMicrotask(advanceSpotChainRef.current);
      }
      return;
    }
    // Skip ids that vanished since the chain was planned (a prior run could remove one).
    let nextId: string | undefined;
    while (chain.queue.length > 0) {
      const candidate = chain.queue.shift() as string;
      if (projectRef.current.manifest.nodes[candidate]) {
        nextId = candidate;
        break;
      }
    }
    if (!nextId) {
      spotChainRef.current = null;
      setSpotBusy(false);
      return;
    }
    const instruction =
      chain.prompt.length > 0 ? SPOT_PREFIX + chain.prompt : SPOT_AUTODETECT;
    void runEdit(instruction, nextId);
  }, [runEdit]);

  // Live ref to advanceSpotChain so a deferred re-schedule never captures a stale one.
  const advanceSpotChainRef = useRef(advanceSpotChain);
  advanceSpotChainRef.current = advanceSpotChain;

  // Compute hit nodes for a polygon region and start the sequential chain. Hit-test:
  // each VISIBLE node's manifest rect vs. the polygon (`polygonIntersectsRect`).
  // NOTE: measured rendered heights live in the canvas, not here, so an `"auto"`-height
  // node uses a nominal 200px fallback for its rect height (rect-vs-bbox fallback —
  // matches the prototype's `measuredH || 200`). Fixed numeric heights are exact.
  const onRegionAnalyze = useCallback(
    (polygonWorldPts: Point[], prompt: string) => {
      if (spotChainRef.current) return; // a chain is already running
      const p = projectRef.current;
      const rects: NodeRect[] = Object.entries(p.manifest.nodes)
        .filter(([, pl]) => !pl.hidden)
        .map(([id, pl]) => ({
          id,
          x: pl.x,
          y: pl.y,
          w: pl.w,
          h: typeof pl.h === "number" ? pl.h : 200,
          z: pl.z,
        }));
      const hits = rects
        .filter((r) => polygonIntersectsRect(polygonWorldPts, r))
        .map((r) => r.id);

      if (hits.length === 0) {
        showToast("No sections in the region");
        return;
      }

      // One user message framing the batch; each per-node runEdit appends its own
      // assistant card (the existing edit cards do this naturally).
      pushMessage({
        role: "user",
        text: prompt.length > 0 ? prompt : "Auto-detect issues in the selected area",
        ctx: `Spot Edit · ${hits.length} section${hits.length === 1 ? "" : "s"}`,
      });

      spotChainRef.current = { queue: [...hits], prompt };
      setSpotBusy(true);
      advanceSpotChain();
    },
    [advanceSpotChain, pushMessage, showToast],
  );

  // Drive the chain forward on each terminal stream transition. The pipeline
  // done-effect / cancel-error effect run on these same statuses and finalize the
  // current run FIRST (they null pendingRunRef / patch the card); this effect then
  // advances. A `cancelled` terminal ABORTS the whole chain (a Stop cancels it).
  // Guarded so it only acts while a chain is active and the run is fully consumed.
  useEffect(() => {
    if (!spotChainRef.current) return;
    if (streamStatus === "cancelled") {
      spotChainRef.current = null;
      // Clear any pending advance flag: a microtask advance may have been scheduled by
      // a prior `done`/`error` and not yet run. Leaving it `true` would make the NEXT
      // chain's first terminal transition skip its own scheduling (it sees the flag set
      // from this aborted chain) and stall — the stale-schedule stall.
      spotAdvanceScheduledRef.current = false;
      setSpotBusy(false);
      return;
    }
    if (streamStatus === "done" || streamStatus === "error") {
      // Defer to a microtask so the pipeline done-effect / cancel-error effect (which
      // consume the run: null pendingRunRef, patch the card, persist) run FIRST. The
      // advance re-checks the single-stream invariant before starting the next node.
      if (!spotAdvanceScheduledRef.current) {
        spotAdvanceScheduledRef.current = true;
        queueMicrotask(advanceSpotChainRef.current);
      }
    }
  }, [streamStatus]);

  // Stop from the composer. Must reliably abort a running Spot Edit chain EVEN in the
  // inter-node gap (no live stream): if the cancel only hit `cancelGeneration` and no
  // stream is currently streaming, the chain would survive and the next queued node
  // would still fire. So tear the chain down synchronously FIRST (null the queue, clear
  // the schedule flag + busy), THEN cancel any active stream. The chain-drive effect's
  // `cancelled` branch is a no-op once `spotChainRef` is already null, so no double-free.
  const onStop = useCallback(() => {
    if (spotChainRef.current) {
      spotChainRef.current = null;
      spotAdvanceScheduledRef.current = false;
      setSpotBusy(false);
    }
    cancelGeneration();
  }, [cancelGeneration]);

  // Apply the pipeline exactly once when a stream completes.
  useEffect(() => {
    if (
      streamStatus !== "done" ||
      consumedRef.current ||
      streamText.trim().length === 0
    )
      return;
    const run = pendingRunRef.current;
    if (!run) return;
    consumedRef.current = true;
    pendingRunRef.current = null;

    const meta = run.meta;
    const logOutcome = (
      outcome: GenerationLogEntry["outcome"],
      nodeIds: string[],
    ) => {
      appendGenerationLog({
        ts: new Date().toISOString(),
        kind: run.mode,
        nodeIds,
        backendKind: meta.backendKind,
        promptChars: meta.promptChars,
        oracleGrounded: meta.oracleGrounded,
        durationMs: Math.max(0, Date.now() - meta.startedAt),
        outcome,
      });
    };

    try {
      if (run.mode === "generate") {
        const {
          project: next,
          newIds,
          shapes,
          warnings: genWarnings,
          remainingViolations,
        } = applyGeneration(projectRef.current, streamText, {
          prevShapes: shapesRef.current,
        });
        const committedNodeCount = Object.keys(next.manifest.nodes).length;

        const attemptsSoFar = repairRef.current?.attempts ?? 0;
        const wantsRepair = shouldSelfRepair(
          { committedNodeCount, remainingViolations },
          attemptsSoFar,
          DEFAULT_REPAIR_RETRIES,
        );
        if (wantsRepair && launchRepair(committedNodeCount, remainingViolations, meta)) {
          setStatus(
            committedNodeCount === 0
              ? "No usable markup; retrying with a corrected prompt…"
              : `Some nodes were invalid; retrying with a corrected prompt…`,
          );
          logOutcome(committedNodeCount === 0 ? "empty" : "applied", []);
          setError(null);
          return;
        }

        if (committedNodeCount === 0) {
          const detail =
            attemptsSoFar > 0
              ? `Couldn't get valid markup after ${attemptsSoFar + 1} tries; canvas unchanged.`
              : "No usable markup in the response; canvas unchanged.";
          setStatus(detail);
          patchMessage(meta.msgId, {
            status: "error",
            title: "Generation failed",
            desc: detail,
          });
          logOutcome("empty", []);
        } else {
          shapesRef.current = shapes;
          setProject(next);
          setHistoryValue(createHistory<DesignSnapshot>()); // generation = new baseline
          persistProject(next);
          const base =
            newIds.length > 0
              ? `Generated ${newIds.length} new node(s).`
              : "Regenerated; existing placements kept.";
          const detail =
            genWarnings.length > 0
              ? `${base} ${genWarnings.length} node(s) dropped: invalid root element.`
              : base;
          setStatus(detail);
          const committedIds = Object.keys(next.manifest.nodes);
          patchMessage(meta.msgId, {
            status: "done",
            title:
              newIds.length > 0
                ? `Added ${newIds.length} node${newIds.length === 1 ? "" : "s"}`
                : "Regenerated",
            desc: detail,
            // Prefer locating the freshly-minted nodes; fall back to all committed.
            nodeIds: newIds.length > 0 ? newIds : committedIds,
          });
          logOutcome("applied", committedIds);
        }
      } else {
        const {
          project: next,
          changed,
          warnings: editWarnings,
        } = applyEdit(projectRef.current, run.nodeId, streamText);

        if (!changed) {
          setStatus("Couldn't apply the edit: invalid markup. Node unchanged.");
          patchMessage(meta.msgId, {
            status: "error",
            title: `Couldn't update ${run.nodeId}`,
            desc: "The model returned invalid markup; the node was left untouched.",
          });
          logOutcome("empty", []);
          setError(null);
          return;
        }

        setProject(next);
        setHistoryValue(createHistory<DesignSnapshot>()); // edit = new baseline
        const editedMarkup = next.components[run.nodeId];
        if (editedMarkup) {
          const reparsed = parseTopLevelNodes(editedMarkup);
          if (reparsed.length > 0) {
            shapesRef.current = {
              ...shapesRef.current,
              [run.nodeId]: reparsed[0],
            };
          }
        }
        persistNode(next, run.nodeId);
        const detail =
          editWarnings.length > 0
            ? `Updated node "${run.nodeId}". ${editWarnings[0]}`
            : `Updated node "${run.nodeId}".`;
        setStatus(detail);
        patchMessage(meta.msgId, {
          status: "done",
          title: `Updated ${run.nodeId}`,
          desc: detail,
          nodeIds: [run.nodeId],
        });
        logOutcome("applied", [run.nodeId]);
      }
      setError(null);
    } catch (e) {
      setError(String(e));
      patchMessage(meta.msgId, {
        status: "error",
        title: "Generation error",
        desc: String(e),
      });
      logOutcome("error", []);
    }
  }, [
    streamStatus,
    streamText,
    persistProject,
    persistNode,
    appendGenerationLog,
    launchRepair,
    setHistoryValue,
    patchMessage,
  ]);

  // Terminal CANCEL / ERROR: flip the in-flight assistant card to an error state with
  // a Retry affordance. (The `done` transition is owned by the pipeline done-effect
  // above; this only handles the non-applied terminals so the card never sticks on
  // "working".) Guarded by activeMsgRef so a single transition patches once.
  useEffect(() => {
    if (streamStatus !== "cancelled" && streamStatus !== "error") return;
    const msgId = activeMsgRef.current;
    if (msgId === null) return;
    activeMsgRef.current = null;
    // Clear any owed pipeline action so a late `done` can't double-fire.
    pendingRunRef.current = null;
    consumedRef.current = true;
    patchMessage(msgId, {
      status: "error",
      title: streamStatus === "cancelled" ? "Stopped" : "Generation failed",
      desc:
        streamStatus === "cancelled"
          ? "You stopped this run; the canvas was left unchanged."
          : "The provider returned an error; the canvas was left unchanged.",
    });
  }, [streamStatus, patchMessage]);

  // When the pipeline done-effect consumes a run, clear the active card marker so the
  // cancel/error effect above doesn't re-touch the just-finalized card.
  //
  // We intentionally do NOT special-case a `done` with EMPTY text here: that state is
  // also the TRANSIENT window right after a NEW run starts (the stream resets text to
  // "" while the previous status is still "done"), so reacting to it would wrongly
  // tear down the freshly-started run (the BLOCKER-2 family). A genuinely empty
  // terminal response is handled by the pipeline done-effect once real text arrives.
  useEffect(() => {
    if (streamStatus === "done" && consumedRef.current) {
      activeMsgRef.current = null;
    }
  }, [streamStatus]);

  // Drop a stale selection: after a generation/load removes the selected node.
  useEffect(() => {
    if (selectedId && !project.manifest.nodes[selectedId]) setSelectedId(null);
  }, [project.manifest.nodes, selectedId]);

  const projectOpen = folder.trim().length > 0;
  const streaming = streamStatus === "streaming";
  // The composer/panel is "busy" while either the async prepare window OR the stream
  // itself is live — mirroring the temp panel's disable condition exactly. ALSO busy
  // for the WHOLE Spot Edit chain (`spotBusy`), including the inter-node gap between
  // two runs where no single stream is live: a composer send in that gap would inject
  // a generate/edit run that interleaves with — and hijacks — the chain's stream slot.
  const panelBusy = preparing || streaming || spotBusy;
  // V7: mirror panelBusy into the ref the undo/redo keydown listener reads. Assigned during
  // render (cheap, idempotent) so the window-level handler always sees the current value
  // without re-subscribing the listener on every busy-state flip.
  panelBusyRef.current = panelBusy;

  // The display name of the node selected for edit (the composer's context chip). Node
  // ids are the user-facing label in this module.
  const selectedNodeName =
    selectedId && project.manifest.nodes[selectedId] ? selectedId : null;

  // Composer send: a selected node routes to the EXISTING edit round-trip; otherwise
  // the EXISTING generate flow. The invoke payloads are byte-identical to the old form.
  const onComposerSend = useCallback(
    (text: string) => {
      if (selectedId && projectRef.current.manifest.nodes[selectedId]) {
        void runEdit(text);
      } else {
        void runGenerate(text);
      }
    },
    [selectedId, runEdit, runGenerate],
  );

  // A suggestion SEEDS the composer draft (does not send) — matching the prototype.
  const onSuggest = useCallback((text: string) => setDraft(text), []);

  // Regenerate / Retry re-runs the SAME instruction (edit when the card was an edit).
  // Ignore while a run is already preparing or in flight (pendingRunRef is set for the
  // whole prepare→stream window) so a re-run can't supersede a live run and strand its
  // working card.
  const onRerun = useCallback(
    (msg: AssistantMessage) => {
      if (!msg.instruction) return;
      if (preparingRef.current || pendingRunRef.current !== null) return;
      if (msg.editNodeId) {
        // This card was an EDIT. If its target node no longer exists (deleted by a
        // later generation/edit), silently re-running as a generate would surprise the
        // user. Surface a clear error instead of changing the operation.
        if (projectRef.current.manifest.nodes[msg.editNodeId]) {
          void runEdit(msg.instruction, msg.editNodeId);
        } else {
          patchMessage(msg.id, {
            status: "error",
            title: "Can't retry edit",
            desc: "Node no longer exists — select a node and try again.",
          });
        }
        return;
      }
      void runGenerate(msg.instruction);
    },
    [runEdit, runGenerate, patchMessage],
  );

  // "Select on canvas": select the first listed node that still exists.
  const onLocate = useCallback(
    (nodeIds: string[]) => {
      const id = nodeIds.find((n) => projectRef.current.manifest.nodes[n]);
      if (id) setSelectedId(id);
    },
    [],
  );

  // Empty-canvas "Generate a section" focuses the composer textarea.
  const focusComposer = useCallback(() => setFocusSignal((v) => v + 1), []);

  // Assistant panel resize: pointer-drag the `.panel-resizer`. The panel is on the
  // RIGHT, so dragging left (negative dx) WIDENS it. Clamped to 290–540.
  const panelResizeRef = useRef<{ startX: number; startW: number } | null>(null);
  const onResizerPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      panelResizeRef.current = { startX: e.clientX, startW: panelW };
      e.currentTarget.setPointerCapture(e.pointerId);
    },
    [panelW],
  );
  const onResizerPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const st = panelResizeRef.current;
      if (!st) return;
      const dx = e.clientX - st.startX;
      const w = st.startW - dx; // right-anchored panel
      setPanelW(Math.max(290, Math.min(540, w)));
    },
    [],
  );
  const endResize = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    panelResizeRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }, []);

  return (
    <div
      className={"dsgn" + (fullscreen ? " dsgn-full" : "")}
      data-screen-label="Design module"
    >
      <div className="main">
        <TopBar
          projectName={projectOpen ? project.meta.name : ""}
          workingFolderPath={folder}
          projectOpen={projectOpen}
          saveState={saveState}
          saving={saving}
          busy={busy !== null}
          canUndo={historyFlags.canUndo}
          canRedo={historyFlags.canRedo}
          onUndo={undo}
          onRedo={redo}
          fullscreen={fullscreen}
          onToggleFullscreen={() => setFullscreen((v) => !v)}
          recent={recent}
          renamingId={renamingId}
          renameDraft={renameDraft}
          setRenameDraft={setRenameDraft}
          beginRename={beginRename}
          commitRename={(id) => void commitRename(id)}
          cancelRename={cancelRename}
          removeEntry={(id) => void removeEntry(id)}
          openEntry={openEntry}
          onNewProject={() => void onNewProject()}
          onOpenFolder={() => void onOpenFolder()}
          onEditContract={projectOpen ? () => void openContractEditor() : undefined}
          oracleStatus={oracleStatus}
          tokens={tokens}
          invoke={invokeBackendCommand}
          tauri={tauri}
          runExport={(mode) => void runExport(mode)}
          exportTokens={() => void exportTokens()}
          onConsolidate={() => void runConsolidate()}
          onHandoff={openHandoff}
          onPreview={() => void preview.openPreview("absolute")}
          previewing={preview.opening}
        />

        <div className="work" data-side="right" data-screen-label="Design workspace">
          <div className="canvas-wrap">
            {projectOpen ? (
              <>
                <DesignCanvas
                  project={project}
                  onManifestChange={onManifestChange}
                  onProjectChange={onProjectChange}
                  onSelect={setSelectedId}
                  selectedId={selectedId}
                  onBeginChange={onBeginChange}
                  tokens={tokens}
                  onNodeMarkupCommit={onNodeMarkupCommit}
                  onRegionAnalyze={onRegionAnalyze}
                  spotBusy={spotBusy}
                  onSeedComposer={onSeedComposer}
                />
                {Object.keys(project.manifest.nodes).length === 0 ? (
                  <div className="canvas-empty">
                    <div className="ce-card">
                      <div className="ce-ic">
                        <Sparkles size={22} />
                      </div>
                      <b>Generate a section</b>
                      <p>
                        Describe a section in the assistant — it&apos;s generated
                        grounded in your real codebase and placed on the canvas.
                      </p>
                      <button
                        type="button"
                        className="btn btn-primary"
                        onClick={focusComposer}
                      >
                        <Sparkles size={15} />
                        Generate a section
                      </button>
                    </div>
                  </div>
                ) : null}
              </>
            ) : (
              <>
                {/* Hidden canvas keeps the test-mock hooks (select-first / commit)
                    available even before a folder is chosen, AND the demo project
                    renders for in-runtime exploration. */}
                <DesignCanvas
                  project={project}
                  onManifestChange={onManifestChange}
                  onProjectChange={onProjectChange}
                  onSelect={setSelectedId}
                  selectedId={selectedId}
                  onBeginChange={onBeginChange}
                  tokens={tokens}
                  onNodeMarkupCommit={onNodeMarkupCommit}
                  onRegionAnalyze={onRegionAnalyze}
                  spotBusy={spotBusy}
                  onSeedComposer={onSeedComposer}
                />
                <div className="canvas-empty">
                  <div className="ce-card">
                    <div className="ce-ic">
                      <FolderOpen size={22} />
                    </div>
                    <b>Open or create a project</b>
                    <p>
                      Pick a working folder inside your codebase. The design grounds on
                      it via Oracle and saves back into it.
                    </p>
                    <button
                      type="button"
                      className="btn btn-primary"
                      disabled={!tauri || busy !== null}
                      onClick={() => void onOpenFolder()}
                    >
                      <FolderOpen size={15} />
                      Open working folder…
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost"
                      disabled={!tauri || busy !== null}
                      onClick={() => void onNewProject()}
                    >
                      New project…
                    </button>
                    {!tauri && (
                      <p className="at-note">
                        Persistence needs the desktop app; the canvas works in any
                        runtime.
                      </p>
                    )}
                  </div>
                </div>
              </>
            )}
          </div>

          {/* Draggable divider between the canvas and the assistant panel. */}
          <div
            className="panel-resizer"
            title="Drag to resize"
            onPointerDown={onResizerPointerDown}
            onPointerMove={onResizerPointerMove}
            onPointerUp={endResize}
            onPointerCancel={endResize}
          />

          <AssistantPanel
            width={panelW}
            messages={panelMessages}
            doneCount={panelDoneCount}
            selectedNodeName={selectedNodeName}
            onClearContext={() => setSelectedId(null)}
            onSend={onComposerSend}
            onSuggest={onSuggest}
            onRerun={onRerun}
            onLocate={onLocate}
            onStop={onStop}
            busy={panelBusy}
            backend={backend}
            onSaveBackend={saveBackend}
            onOpenSettings={openProviderSettings}
            draft={draft}
            setDraft={setDraft}
            focusSignal={focusSignal}
            notice={status}
            error={error}
            onVisualCheck={() => void onVisualCheck()}
            visualCheckDisabled={!projectOpen}
            visualChecking={preview.checking}
          />
        </div>
      </div>

      <DesignMdEditor
        open={contractEditor !== null}
        initialContent={contractEditor?.initialContent ?? ""}
        draftTokens={contractEditor?.draftTokens}
        notice={contractEditor?.notice}
        saveError={contractEditor?.saveError}
        onSave={onContractSave}
        onSkip={onContractSkip}
      />

      <HandoffModal
        open={handoff.open}
        workingFolderPath={folder}
        phase={handoff.phase}
        steps={handoff.steps}
        flow={handoff.flow}
        projects={handoff.projects}
        projectsError={handoffProjectsError}
        selectedProjectId={handoff.selectedProjectId}
        client={handoff.client}
        agentId={handoff.agentId}
        errorStage={handoff.errorStage}
        errorMessage={handoff.errorMessage}
        dispatching={handoff.dispatching}
        canDispatch={handoff.canDispatch}
        closable={handoff.closable}
        onClose={handoff.close}
        onSelectProject={handoff.selectProject}
        onSelectClient={handoff.selectClient}
        onRetryPackaging={handoff.runPackaging}
        onDispatch={handoff.dispatch}
        onOpenTerminal={handoff.openTerminal}
      />

      <Toast msg={toast} onDismiss={() => setToast(null)} />
    </div>
  );
}

export default DesignView;
