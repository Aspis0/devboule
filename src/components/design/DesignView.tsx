// DesignView — Phase 1b surface for the generative-design module.
//
// Scope (Phase 1b only): a deterministic canvas with a HARDCODED fake project of
// a few nodes, dragged/resized live via the pure engine. An optional working-
// folder path lets the operator exercise the Rust persistence commands
// (create/load/consolidate + throttled drag-commit). NO LLM, NO Oracle, NO
// project picker/registry — those are Phase 2/3.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Palette,
  Save,
  FolderPlus,
  FolderOpen,
  FolderSearch,
  X,
  AlertTriangle,
  Sparkles,
  Square,
  Wand2,
  Code2,
  Clock,
  Pencil,
  Trash2,
  Check,
} from "lucide-react";
import {
  invokeBackendCommand,
  isTauriRuntime,
} from "../../context/AppContext";
import type {
  DesignManifest,
  DesignProject,
  DesignProjectEntry,
} from "../../types/design";
import { Canvas } from "./Canvas";
import { sanitizeNodeMarkup } from "./sanitize";
import { useDesignStream } from "./useDesignStream";
import {
  applyEdit,
  applyGeneration,
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
import { tokenNamesForPrompt, type DtcgDocument } from "./engine/tokens";
import { exportCode, type ExportMode } from "./export/exportCode";

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

/** Build the hardcoded Phase-1b demo project (deterministic, no clock fields used
 * for layout). Markup is pre-sanitized so the on-disk/in-memory copy is already
 * the safe form the canvas would inject. */
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

/** Format an ISO `lastOpenedAt` as a short, locale-stable date label for the recent
 * list. An empty/invalid timestamp yields an empty string (no "Invalid Date" leak). */
function formatLastOpened(iso: string): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  return new Date(t).toLocaleDateString();
}

export function DesignView() {
  const [project, setProject] = useState<DesignProject>(() => buildDemoProject());
  const [folder, setFolder] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Phase 3 (management plane) — the recent-projects registry (metadata only, in
  // config.json). Loaded on mount; refreshed after every remember/rename/remove (the
  // commands return the full sorted list). Lets the operator re-open a working folder
  // without re-picking it. `renamingId`/`renameDraft` drive the inline rename editor.
  const [recent, setRecent] = useState<DesignProjectEntry[]>([]);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  // Phase-2 STEP 3: the full generation pipeline. The stream feeds raw model TEXT;
  // on `done` the deterministic pipeline parses -> re-anchors -> places -> sanitizes
  // it into the canvas, then persists. Two flows share one stream: a full GENERATE
  // (prompt box) and a per-node EDIT (selected node + instruction).
  const [prompt, setPrompt] = useState("");
  const [editInstruction, setEditInstruction] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const {
    text: streamText,
    status: streamStatus,
    error: streamError,
    start: startStream,
    cancel: cancelGeneration,
  } = useDesignStream();

  const tauri = isTauriRuntime();

  // The structural shapes from the LAST generation, keyed by node id. Persisted in
  // memory so the NEXT regeneration can structurally re-anchor dropped/renamed ids
  // (the `prevShapes` arg of reanchorIds). NOT written to disk (it's not part of the
  // on-the-wire DesignProject; it's a deterministic-layer cache).
  const shapesRef = useRef<ShapeMap>({});

  // The W3C DTCG tokens document for this project (seeded from the target via
  // Oracle on load; empty by default). Kept in memory; persisted to tokens.json.
  // Its NAMES feed the generate prompt as a soft "prefer these tokens" preference.
  const [tokens, setTokens] = useState<DtcgDocument>({});
  const tokensRef = useRef<DtcgDocument>(tokens);
  tokensRef.current = tokens;

  // Export mode toggle for the "Export code" action.
  const [exportMode, setExportMode] = useState<ExportMode>("absolute");

  // Per-run metadata carried from start -> terminal `done`, so the token-free audit
  // line records the right backend/prompt-size/grounding/duration. Stamped at start.
  interface RunMeta {
    startedAt: number;
    backendKind: string;
    promptChars: number;
    oracleGrounded: boolean;
  }

  // Bounded self-repair (Phase 2.5 Tier 1) state for a FULL generation. Carries the
  // ORIGINAL user instruction + grounding context so a repair attempt can re-prompt
  // the SAME provider with a targeted correction, plus the attempt counter the cap
  // is enforced against. `null` between generations. Held in a ref (not state) so the
  // terminal done-effect reads the live value without re-subscribing.
  interface RepairState {
    instruction: string;
    context: string;
    attempts: number;
  }
  const repairRef = useRef<RepairState | null>(null);

  // What the in-flight stream is doing, consumed exactly once on the terminal
  // `done` transition. `null` means no pipeline action is owed (idle / cancelled /
  // error / already consumed). Carries the audit metadata for the run.
  const pendingRunRef = useRef<
    | { mode: "generate"; meta: RunMeta }
    | { mode: "edit"; nodeId: string; meta: RunMeta }
    | null
  >(null);
  // Guards the done-effect against re-running for the same completed stream (the
  // hook's `status` stays "done" until the next start).
  const consumedRef = useRef(false);

  // W3: re-entry guard for the async PREPARE window of a start. runGenerate/runEdit
  // `await readBackendKind()` (and, for generate, the Oracle grounding fetch) BEFORE
  // calling startStream — during that await `streamStatus` is still "idle"/"done", so
  // the disabled-on-streaming button does NOT block a second click. This flag is set
  // synchronously at the top of a prepare and cleared once startStream is dispatched
  // (or the prepare bails), so a second Generate/Edit while preparing is ignored — no
  // two live backend generations. A React state mirror drives the button disabled UI.
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

  // Live folder ref: `flushManifest` reads the folder at CALL time, not at
  // closure-capture time. Without this, the throttle/unmount cleanup (which
  // re-runs whenever `folder` changes — i.e. on every keystroke) would flush a
  // pending manifest to the OLD/half-typed path captured in the prior closure.
  const folderRef = useRef(folder);
  folderRef.current = folder;

  // Throttle handle for drag-commit manifest writes.
  const writeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingManifest = useRef<DesignManifest | null>(null);

  const flushManifest = useCallback(() => {
    writeTimer.current = null;
    const manifest = pendingManifest.current;
    pendingManifest.current = null;
    const folderPath = folderRef.current.trim();
    if (!manifest || !folderPath || !tauri) return;
    invokeBackendCommand("design_write_manifest", {
      workingFolderPath: folderPath,
      manifest,
    }).catch((e) => setError(String(e)));
  }, [tauri]);

  // Drag/resize/bring-to-front commit: update in-memory state immediately, and
  // schedule a throttled disk write of just the manifest (cheap path).
  const onManifestChange = useCallback(
    (next: DesignManifest) => {
      setProject((prev) => ({ ...prev, manifest: next }));
      pendingManifest.current = next;
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

  // Best-effort seed of the W3C DTCG token document from the TARGET via Oracle.
  // This step's scope is a LIGHT seed: we do NOT parse code for concrete $values
  // (that is the deferred Token Coherence Loop). We probe the index to learn
  // whether a token-bearing surface exists, then write a MINIMAL, CLEAN DTCG stub.
  //
  // WARNING 3: we MUST NOT persist Oracle-derived target FILE PATHS into
  // tokens.json (e.g. via a `$description` listing `src/theme.ts, ...`). tokens.json
  // is a portable, possibly-committed artifact; embedding the target's design-system
  // file structure leaks layout for zero benefit (the paths are unused by
  // `tokenNamesForPrompt`). We write only an empty/clean stub here. Always degrades
  // to an empty document when Oracle is unavailable.
  const seedTokens = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    if (!folderPath || !tauri) {
      setTokens({});
      return;
    }
    // A clean, path-free stub. (Concrete $value extraction is deferred; until then
    // there are no named tokens, so the prompt token-name preference stays empty.)
    const doc: DtcgDocument = {};
    try {
      // Probe is best-effort: its only purpose is to surface Oracle errors early so
      // the catch can degrade. We deliberately ignore the returned chunks — they
      // carry target file paths we must not persist.
      await invokeBackendCommand<DesignContextChunk[]>("design_oracle_context", {
        workingFolderPath: folderPath,
        query: "design tokens palette colors typography spacing theme",
        limit: 8,
      });
    } catch {
      // Non-fatal: degrade to the empty document already in `doc`.
    }
    setTokens(doc);
    // Persist the (currently empty) document; best-effort. tokens.json carries NO
    // target file paths.
    await invokeBackendCommand("design_write_tokens", {
      workingFolderPath: folderPath,
      tokensJson: JSON.stringify(doc, null, 2),
    }).catch(() => {
      // Non-fatal: tokens persistence failing must not break load.
    });
  }, [tauri]);

  // Open the NATIVE OS directory picker and store the chosen absolute path. The
  // working folder must be CHOSEN, never typed — so this is the only way `folder`
  // gets set. A dismissed dialog (open -> null) or an unavailable plugin is a
  // silent no-op (no error). Only the dialog import/open is wrapped here.
  const pickFolder = useCallback(async () => {
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
    if (picked === null) return;
    setFolder(picked);
    setError(null);
  }, []);

  // --- Phase 3: recent-projects registry ------------------------------------

  // Load the registry on mount (best-effort; reader-or-empty on the Rust side, so a
  // missing key / config never errors). Web runtime has no backend -> stays empty.
  useEffect(() => {
    if (!tauri) return;
    let cancelled = false;
    void (async () => {
      try {
        const list = await invokeBackendCommand<DesignProjectEntry[]>(
          "design_registry_list",
          {},
        );
        if (!cancelled) setRecent(list ?? []);
      } catch {
        // Non-fatal: the recent list is a convenience; leave it empty.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauri]);

  // Upsert the given folder + name into the registry after a successful create/open.
  // The Rust command dedupes by canonical folder + returns the full sorted list, which
  // we adopt as the new recent state. Best-effort: a registry failure never breaks the
  // create/load it follows.
  const rememberProject = useCallback(
    async (workingFolderPath: string, name: string) => {
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

  const runCreate = useCallback(async () => {
    if (!folder.trim()) {
      setError("Choose a working folder first.");
      return;
    }
    setBusy("create");
    setError(null);
    setStatus(null);
    try {
      const created = await invokeBackendCommand<DesignProject>(
        "design_create_project",
        { workingFolderPath: folder.trim(), name: "Demo landing" },
      );
      // Seed the freshly created (empty) project with the demo nodes, then save.
      const demo = buildDemoProject();
      const seeded: DesignProject = {
        ...created,
        manifest: demo.manifest,
        components: demo.components,
        meta: { ...created.meta, nodeOrder: demo.meta.nodeOrder },
      };
      await invokeBackendCommand("design_save_project", {
        workingFolderPath: folder.trim(),
        project: seeded,
      });
      setProject(seeded);
      setStatus("Created and seeded the project on disk.");
      // Remember it in the recent-projects registry (metadata only). Use the project
      // name from the freshly created meta.
      await rememberProject(folder.trim(), seeded.meta.name);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [folder, rememberProject]);

  // Core load flow shared by the "Load" button and the recent-projects list. Loads the
  // given working folder, adopts it as the current `folder`, re-derives shapes, seeds
  // tokens, and remembers it in the registry. `fromRecentId` (when opening a recent
  // entry) lets a missing-folder failure offer a one-click "remove from list".
  const loadFolder = useCallback(
    async (path: string, fromRecentId?: string) => {
      const folderPath = path.trim();
      if (!folderPath) {
        setError("Choose a working folder first.");
        return;
      }
      setBusy("load");
      setError(null);
      setStatus(null);
      // Adopt the path as the working folder so subsequent saves/drags target it.
      setFolder(folderPath);
      try {
        const loaded = await invokeBackendCommand<DesignProject>(
          "design_load_project",
          { workingFolderPath: folderPath },
        );
        setProject(loaded);
        // Re-derive the in-memory ShapeMap from the loaded markup (BLOCKER 3):
        // shapesRef is in-memory only and would otherwise be {} after a reload, so
        // the first post-reload regeneration would re-mint any id the model drops and
        // reset its placement. Re-deriving restores structural recovery.
        shapesRef.current = deriveShapes(loaded.components);
        setStatus(
          loaded.warnings && loaded.warnings.length > 0
            ? `Loaded with ${loaded.warnings.length} warning(s).`
            : "Loaded from disk.",
        );
        // WARNING 5: AWAIT the token seed inside the blocking "Loading…" UX so the
        // token document is ready BEFORE the user can fire the first generation
        // (clicking Generate immediately after Load otherwise saw `tokens` still {}
        // and raced the seed). `seedTokens` is self-contained best-effort (it swallows
        // Oracle/persistence failures internally), so awaiting it cannot reject and
        // never blocks load on a token error.
        await seedTokens();
        // Remember the just-opened project (bumps lastOpenedAt; dedupes by folder).
        await rememberProject(folderPath, loaded.meta.name);
      } catch (e) {
        // Graceful registry↔folder drift: a deleted/renamed folder makes the backend
        // canonicalize fail ("working folder does not exist or is unreadable"). Surface
        // a clear status (never crash) and, when opening a recent entry, point the user
        // at the per-row Remove control to prune the stale entry.
        setError(
          fromRecentId
            ? `${String(e)} — the folder may have moved; use Remove on the entry to prune it.`
            : String(e),
        );
      } finally {
        setBusy(null);
      }
    },
    [seedTokens, rememberProject],
  );

  const runLoad = useCallback(async () => {
    await loadFolder(folder);
  }, [folder, loadFolder]);

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

  // Remove a registry entry (unregister only — removeFiles defaults OFF, so the
  // working folder's design files are never touched). Returns the new sorted list.
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

  const runConsolidate = useCallback(async () => {
    if (!folder.trim()) {
      setError("Choose a working folder first.");
      return;
    }
    setBusy("save");
    setError(null);
    setStatus(null);
    try {
      await invokeBackendCommand("design_save_project", {
        workingFolderPath: folder.trim(),
        project: projectRef.current,
      });
      setStatus("Consolidated the whole project to disk.");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [folder]);

  // Export the current project to standalone HTML (absolute or flow) and write it to
  // the working folder via the path-confined Rust command. Best-effort with status.
  const runExport = useCallback(async () => {
    const folderPath = folderRef.current.trim();
    if (!folderPath || !tauri) {
      setError("Choose a working folder first.");
      return;
    }
    setBusy("export");
    setError(null);
    setStatus(null);
    try {
      // BLOCKER 1: `exportCode` is PURE/DOM-free and inlines stored component
      // markup VERBATIM, trusting it to be sanitized. But `design_load_project`
      // returns the raw bytes of `components/<id>.html`, so a hand-edited/malicious
      // `<script>` / `<img onerror>` on disk would survive into the exported HTML
      // (the canvas re-sanitizes on inject; export did not). Sanitize every
      // component HERE — the DOM-capable caller — via the single chokepoint, then
      // export a sanitized COPY of the project. Manifest/meta are untouched.
      const src = projectRef.current;
      const safeComponents: Record<string, string> = {};
      for (const [id, markup] of Object.entries(src.components)) {
        safeComponents[id] = sanitizeNodeMarkup(markup);
      }
      const safeProject: DesignProject = { ...src, components: safeComponents };
      const content = exportCode(safeProject, exportMode);
      const filename =
        exportMode === "absolute" ? "export-absolute.html" : "export-flow.html";
      await invokeBackendCommand("design_write_export", {
        workingFolderPath: folderPath,
        filename,
        content,
      });
      setStatus(`Exported ${exportMode} layout to ${filename}.`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, [tauri, exportMode]);

  // --- Generation / edit flow ------------------------------------------------

  // Persist the WHOLE project after a full generation (nodeOrder + many nodes +
  // manifest changed → one consolidating write). Best-effort: a disk failure
  // surfaces a non-fatal error but the in-memory canvas already reflects it.
  const persistProject = useCallback(
    (next: DesignProject) => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) return;
      // W4: a generation's whole-project save WRITES the manifest. A throttled
      // drag-commit manifest write may be pending (timer scheduled, `pendingManifest`
      // set). If that fires AFTER this save it would clobber the generation's manifest
      // on disk with the stale drag-only manifest (lost generation). Cancel the pending
      // drag write here and DROP its manifest: `next.manifest` was built from
      // `projectRef.current` which already reflects every committed drag (onManifestChange
      // updates state synchronously before any generation), so it is the freshest and
      // supersedes the pending drag-only write. Pointer-up commits already in state are
      // therefore NOT lost — they are folded into `next`.
      if (writeTimer.current !== null) {
        clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
      pendingManifest.current = null;
      invokeBackendCommand("design_save_project", {
        workingFolderPath: folderPath,
        project: next,
      }).catch((e) => setError(String(e)));
    },
    [tauri],
  );

  // Persist a single node after an edit (cheap path): its markup + the manifest
  // (kind may have changed). Best-effort.
  const persistNode = useCallback(
    (next: DesignProject, nodeId: string) => {
      const folderPath = folderRef.current.trim();
      if (!folderPath || !tauri) return;
      // W4: like persistProject, this writes the manifest — cancel/drop any pending
      // drag-commit manifest write so it can't clobber the edit's manifest afterwards.
      // `next.manifest` reflects current in-memory state (incl. committed drags).
      if (writeTimer.current !== null) {
        clearTimeout(writeTimer.current);
        writeTimer.current = null;
      }
      pendingManifest.current = null;
      // WARNING 4: SERIALIZE the two writes — node markup FIRST, then the manifest.
      // Firing both as un-ordered fire-and-forget could land them out of order or
      // half-applied, desyncing disk. Node-first means a crash between the two
      // leaves a fresh component referenced by a slightly stale manifest, which the
      // load path already tolerates (the reverse — manifest pointing at markup not
      // yet written — is the corrupting order). Errors surface to status.
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
        }
      })();
    },
    [tauri],
  );

  // Append ONE token-free audit line to generations.jsonl (best-effort, never
  // surfaces as a fatal error). The entry is METADATA-ONLY — no prompt text, no
  // chunk text, no secrets — built here and re-validated/re-serialized by Rust.
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

  // Read the configured design backend kind for the audit log. Best-effort:
  // unconfigured/unavailable -> "unknown" (the generate itself will surface the
  // real "no backend" error). NEVER returns secrets — only the kind string.
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

  // Start a FULL generation: for HTTP providers (ollama/omlx) PRE-FETCH
  // target-scoped Oracle grounding and fold the chunk snippets + the target's DTCG
  // token names into the prompt `context`. CLI providers (api/codex) call Oracle
  // agentically via their own MCP config, so we do NOT pre-fetch chunks for them
  // (we still pass the cheap token-name preference). Any grounding failure/empty
  // result degrades to generating without it (never blocks). Then wrap in the
  // versioned contract and stream; the pipeline runs on the terminal `done` event.
  const runGenerate = useCallback(async () => {
    const instruction = prompt.trim();
    if (instruction.length === 0) return;
    // W3: ignore a re-entrant Generate while a previous start is still PREPARING
    // (awaiting backend kind / grounding) — otherwise two backend runs could launch.
    if (!beginPrepare()) return;
    try {
    const backendKind = await readBackendKind();
    const folderPath = folderRef.current.trim();
    const isHttpProvider = backendKind === "ollama" || backendKind === "omlx";
    const cli = isCliBackend(backendKind);

    // B4: a CLI provider gets NO grounding context at all (not even the token-name
    // preference) — it reaches Oracle agentically and must never be handed untrusted
    // pre-fetched target source in its prompt. HTTP providers get the token-name
    // preference plus (best-effort) the pre-fetched chunks.
    const tokenNames = cli ? [] : tokenNamesForPrompt(tokensRef.current);

    // Pre-fetch grounding chunks ONLY for HTTP providers. The chunk text stays
    // in-process — folded into the prompt only, never logged/emitted.
    let context = cli ? "" : buildGroundingBlock([], tokenNames);
    let oracleGrounded = false;
    if (tauri && folderPath && isHttpProvider) {
      try {
        const chunks = await invokeBackendCommand<DesignContextChunk[]>(
          "design_oracle_context",
          { workingFolderPath: folderPath, query: instruction, limit: 8 },
        );
        context = buildGroundingBlock(chunks ?? [], tokenNames);
        oracleGrounded = (chunks?.length ?? 0) > 0;
      } catch {
        // Degrade to the token-name-only block already in `context`.
      }
    }

    // B4 defense in depth: even if a future code path set `context`, hard-clamp it
    // empty for a CLI backend at the point the prompt is built.
    if (cli) context = "";

    const fullPrompt = buildGeneratePrompt(instruction, { context });
    // Seed the bounded self-repair state for THIS generation: a fresh attempt
    // counter + the inputs a repair re-prompt needs (Phase 2.5 Tier 1).
    repairRef.current = { instruction, context, attempts: 0 };
    consumedRef.current = false;
    pendingRunRef.current = {
      mode: "generate",
      meta: {
        startedAt: Date.now(),
        backendKind,
        promptChars: fullPrompt.length,
        oracleGrounded,
      },
    };
    // Pass the design project's folder so a CLI provider (codex/claude) runs in that
    // directory — a real, trusted context (the design project lives inside the target repo).
    startStream(fullPrompt, folderPath || undefined);
    } finally {
      endPrepare();
    }
  }, [prompt, startStream, tauri, readBackendKind, beginPrepare, endPrepare]);

  // Launch ONE bounded self-repair retry of the in-flight generation (Phase 2.5
  // Tier 1). Reuses the ORIGINAL instruction + grounding context, appends a targeted
  // correction built from the dropped-node violations, increments the attempt
  // counter, and re-streams via the SAME provider. PURE prompt build; the cap +
  // cancel-awareness are enforced by the caller (the done-effect). Returns true if a
  // retry was actually launched.
  const launchRepair = useCallback(
    (
      committedNodeCount: number,
      remainingViolations: GenerationResult["remainingViolations"],
      meta: RunMeta,
    ): boolean => {
      const state = repairRef.current;
      if (!state) return false;
      // B4: the repair re-prompts the SAME provider. If that provider is a CLI
      // backend, FORCE the grounding context empty here — `state.context` may carry
      // a grounding block from a PRIOR HTTP run, and handing it to a CLI provider is
      // exactly the prompt-injection vector we gate against. Recompute at the point
      // the prompt is built, not just at initial runGenerate.
      const repairContext = isCliBackend(meta.backendKind) ? "" : state.context;
      const repaired = buildRepairPrompt(
        state.instruction,
        { committedNodeCount, remainingViolations },
        repairContext,
      );
      if (repaired === null) return false;

      state.attempts += 1;
      consumedRef.current = false;
      pendingRunRef.current = {
        mode: "generate",
        meta: {
          // Preserve the ORIGINAL run's start time so durationMs spans the whole
          // generate+repair sequence; refresh prompt size for the retry.
          startedAt: meta.startedAt,
          backendKind: meta.backendKind,
          promptChars: repaired.length,
          oracleGrounded: meta.oracleGrounded,
        },
      };
      startStream(repaired, folderRef.current.trim() || undefined);
      return true;
    },
    [startStream],
  );

  // Start a per-node EDIT: send ONLY the selected node's current markup +
  // instruction; the pipeline swaps just that node on `done`. (No grounding
  // pre-fetch for an edit — it is a localized restyle of existing markup.)
  const runEdit = useCallback(async () => {
    const id = selectedId;
    const instruction = editInstruction.trim();
    if (!id || instruction.length === 0) return;
    // W3: same re-entry guard as runGenerate — block a second start during the
    // async readBackendKind() prepare window so two backend runs can't launch.
    if (!beginPrepare()) return;
    try {
      const current = projectRef.current.components[id] ?? "";
      const backendKind = await readBackendKind();
      const editPrompt = buildEditPrompt(current, instruction);
      consumedRef.current = false;
      pendingRunRef.current = {
        mode: "edit",
        nodeId: id,
        meta: {
          startedAt: Date.now(),
          backendKind,
          promptChars: editPrompt.length,
          oracleGrounded: false,
        },
      };
      // Pass the design project's folder so a CLI provider runs in that (trusted) directory.
      startStream(editPrompt, folderRef.current.trim() || undefined);
    } finally {
      endPrepare();
    }
  }, [selectedId, editInstruction, startStream, readBackendKind, beginPrepare, endPrepare]);

  // Apply the pipeline exactly once when a stream completes. Reads the live
  // accumulated `streamText`; the run mode decides generate vs edit.
  useEffect(() => {
    // Gate on NON-EMPTY text too (BLOCKER 2): when the user clicks Generate again,
    // `start` synchronously resets `text` to "" while `status` is still "done" from
    // the prior run and `consumedRef` was just reset to false. Without the length
    // guard this effect would re-fire with empty text, trip the zero-node guard,
    // set consumedRef=true, and then SILENTLY DROP the real completion. We only
    // enter the pipeline once the fresh stream has actually accumulated text.
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

    // The audit line is appended on EVERY terminal path below (applied/empty/error).
    // It is metadata-only; `ts` is stamped here (the frontend owns the clock — the
    // Rust command never calls it). durationMs is measured from the run's start.
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

        // BOUNDED SELF-REPAIR (Phase 2.5 Tier 1): if the generation produced ZERO
        // usable nodes OR dropped >=1 node for an unfixable contract violation, ask
        // the SAME provider for ONE corrected pass (capped). We do NOT commit this
        // result first — committing then regenerating would churn the canvas. The
        // cap + the cancel-aware reset (a user Stop bumps runId so a late `done`
        // never reaches here) keep the loop bounded and non-infinite.
        const attemptsSoFar = repairRef.current?.attempts ?? 0;
        const wantsRepair = shouldSelfRepair(
          { committedNodeCount, remainingViolations },
          attemptsSoFar,
          DEFAULT_REPAIR_RETRIES,
        );
        if (wantsRepair && launchRepair(committedNodeCount, remainingViolations, meta)) {
          // A retry was launched; log THIS attempt as empty (no commit yet) and let
          // the retry's own terminal `done` re-enter this effect. Do not touch the
          // canvas/project.
          setStatus(
            committedNodeCount === 0
              ? "No usable markup; retrying with a corrected prompt…"
              : `Some nodes were invalid; retrying with a corrected prompt…`,
          );
          logOutcome(committedNodeCount === 0 ? "empty" : "applied", []);
          setError(null);
          return; // IMPORTANT: skip the commit/edit branches for this attempt
        }

        // No (further) repair: either the result is clean, or the cap is reached —
        // commit what we have (never corrupt the canvas with an empty result).
        if (committedNodeCount === 0) {
          // Cap reached with nothing usable: surface a clear give-up status.
          setStatus(
            attemptsSoFar > 0
              ? `Couldn't get valid markup after ${attemptsSoFar + 1} tries; canvas unchanged.`
              : "No usable markup in the response; canvas unchanged.",
          );
          logOutcome("empty", []);
        } else {
          shapesRef.current = shapes;
          setProject(next);
          persistProject(next);
          // Surface any dropped-node warnings (foster-parented / empty roots) on the
          // committed result so the operator sees what the guard removed.
          const base =
            newIds.length > 0
              ? `Generated ${newIds.length} new node(s).`
              : "Regenerated; existing placements kept.";
          setStatus(
            genWarnings.length > 0
              ? `${base} ${genWarnings.length} node(s) dropped: invalid root element.`
              : base,
          );
          logOutcome("applied", Object.keys(next.manifest.nodes));
        }
      } else {
        // NITPICK 11: applyEdit ignores prevShapes — don't pass a dead arg.
        const {
          project: next,
          changed,
          warnings: editWarnings,
        } = applyEdit(projectRef.current, run.nodeId, streamText);

        // WARNING 5: an unfixable edit (foster/empty/unparseable root) is a NO-OP.
        // Do NOT persist or claim "Updated" — keep the existing node + placement
        // intact and surface a clear warning status instead.
        if (!changed) {
          setStatus("Couldn't apply the edit: invalid markup. Node unchanged.");
          setEditInstruction("");
          logOutcome("empty", []);
          setError(null);
          return;
        }

        setProject(next);
        // BLOCKER 1: refresh the edited node's stored structural shape from its NEW
        // markup. Without this, shapesRef holds the pre-edit shape, so the next full
        // generation can't structurally re-anchor the (possibly restructured / id-
        // dropped) node and it teleports to a default placement. Parse the swapped
        // markup and update only this id's shape (others unchanged).
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
        // WARNING 9: if the edit returned multiple top-level elements, only the
        // first was kept — tell the operator extra content was dropped.
        setStatus(
          editWarnings.length > 0
            ? `Updated node "${run.nodeId}". ${editWarnings[0]}`
            : `Updated node "${run.nodeId}".`,
        );
        setEditInstruction("");
        logOutcome("applied", [run.nodeId]);
      }
      setError(null);
    } catch (e) {
      setError(String(e));
      logOutcome("error", []);
    }
  }, [
    streamStatus,
    streamText,
    persistProject,
    persistNode,
    appendGenerationLog,
    launchRepair,
  ]);

  // Drop a stale selection: after a generation/load removes the selected node,
  // clear it so the edit affordance never targets a node that no longer exists.
  useEffect(() => {
    if (selectedId && !project.manifest.nodes[selectedId]) setSelectedId(null);
  }, [project.manifest.nodes, selectedId]);

  const warnings = project.warnings ?? [];
  const nodeCount = useMemo(
    () => Object.keys(project.manifest.nodes).length,
    [project.manifest.nodes],
  );

  return (
    <div className="flex h-full flex-col gap-4">
      <header className="flex flex-wrap items-center gap-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-2xl bg-terracotta">
          <Palette className="h-5 w-5 text-white" />
        </div>
        <div className="min-w-0">
          <h2 className="text-sm font-semibold text-cream-800">Design</h2>
          <p className="text-[12px] text-cream-400">
            {project.meta.name} · {nodeCount} node{nodeCount === 1 ? "" : "s"} ·
            deterministic canvas
          </p>
        </div>
      </header>

      <section className="flex flex-wrap items-center gap-2 rounded-2xl border border-cream-200 bg-cream-50 p-3">
        <button
          type="button"
          onClick={pickFolder}
          disabled={!tauri || busy !== null}
          className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100 disabled:opacity-50"
        >
          <FolderSearch className="h-4 w-4" />
          Choose folder…
        </button>
        <div className="flex min-w-0 flex-1 items-center gap-1.5 rounded-xl border border-cream-200 bg-white px-3 py-2">
          <span
            className="min-w-0 flex-1 truncate text-[13px] text-cream-800"
            title={folder || undefined}
            data-testid="design-folder-path"
          >
            {folder || (
              <span className="text-cream-400">No folder chosen</span>
            )}
          </span>
          {folder && (
            <button
              type="button"
              onClick={() => setFolder("")}
              disabled={busy !== null}
              aria-label="Clear chosen folder"
              className="shrink-0 rounded-lg p-1 text-cream-400 transition-colors hover:bg-cream-100 hover:text-cream-700 disabled:opacity-50"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
        <button
          type="button"
          onClick={runCreate}
          disabled={!tauri || busy !== null || !folder.trim()}
          className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100 disabled:opacity-50"
        >
          <FolderPlus className="h-4 w-4" />
          Create
        </button>
        <button
          type="button"
          onClick={runLoad}
          disabled={!tauri || busy !== null || !folder.trim()}
          className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100 disabled:opacity-50"
        >
          <FolderOpen className="h-4 w-4" />
          Load
        </button>
        <button
          type="button"
          onClick={runConsolidate}
          disabled={!tauri || busy !== null || !folder.trim()}
          className="inline-flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[13px] font-medium text-white shadow-soft-sm transition-colors hover:bg-terracotta-500 disabled:opacity-50"
        >
          <Save className="h-4 w-4" />
          Consolidate
        </button>
        <div className="flex items-center gap-1.5">
          <select
            value={exportMode}
            onChange={(e) => setExportMode(e.target.value as ExportMode)}
            disabled={!tauri || busy !== null}
            className="rounded-xl border border-cream-200 bg-white px-2 py-2 text-[13px] text-cream-700 outline-none focus:border-terracotta/40 disabled:opacity-50"
            aria-label="Export layout mode"
          >
            <option value="absolute">Absolute layout</option>
            <option value="flow">Flow layout</option>
          </select>
          <button
            type="button"
            onClick={runExport}
            disabled={!tauri || busy !== null}
            className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100 disabled:opacity-50"
          >
            <Code2 className="h-4 w-4" />
            Export code
          </button>
        </div>
      </section>

      {tauri && recent.length > 0 && (
        <section
          className="flex flex-col gap-1.5 rounded-2xl border border-cream-200 bg-cream-50 p-3"
          data-testid="design-recent"
        >
          <div className="flex items-center gap-1.5 text-[12px] font-medium text-cream-700">
            <Clock className="h-3.5 w-3.5 text-terracotta" />
            Recent projects
          </div>
          <ul className="flex flex-col gap-1">
            {recent.map((entry) => (
              <li
                key={entry.id}
                data-testid="design-recent-item"
                className="flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2"
              >
                {renamingId === entry.id ? (
                  <input
                    type="text"
                    value={renameDraft}
                    autoFocus
                    onChange={(e) => setRenameDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void commitRename(entry.id);
                      if (e.key === "Escape") cancelRename();
                    }}
                    aria-label="Rename project"
                    className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-2 py-1 text-[13px] text-cream-800 outline-none focus:border-terracotta/40"
                  />
                ) : (
                  <button
                    type="button"
                    onClick={() => void loadFolder(entry.workingFolderPath, entry.id)}
                    disabled={busy !== null}
                    title={entry.workingFolderPath}
                    className="flex min-w-0 flex-1 flex-col items-start gap-0.5 text-left disabled:opacity-50"
                  >
                    <span className="w-full truncate text-[13px] font-medium text-cream-800">
                      {entry.name}
                    </span>
                    <span className="w-full truncate text-[11px] text-cream-400">
                      {entry.workingFolderPath}
                    </span>
                  </button>
                )}
                {renamingId === entry.id ? (
                  <>
                    <button
                      type="button"
                      onClick={() => void commitRename(entry.id)}
                      aria-label="Save name"
                      className="shrink-0 rounded-lg p-1 text-cream-400 transition-colors hover:bg-cream-100 hover:text-terracotta"
                    >
                      <Check className="h-3.5 w-3.5" />
                    </button>
                    <button
                      type="button"
                      onClick={cancelRename}
                      aria-label="Cancel rename"
                      className="shrink-0 rounded-lg p-1 text-cream-400 transition-colors hover:bg-cream-100 hover:text-cream-700"
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </>
                ) : (
                  <>
                    <span className="hidden shrink-0 text-[11px] text-cream-400 sm:inline">
                      {formatLastOpened(entry.lastOpenedAt)}
                    </span>
                    <button
                      type="button"
                      onClick={() => beginRename(entry)}
                      disabled={busy !== null}
                      aria-label={`Rename ${entry.name}`}
                      className="shrink-0 rounded-lg p-1 text-cream-400 transition-colors hover:bg-cream-100 hover:text-cream-700 disabled:opacity-50"
                    >
                      <Pencil className="h-3.5 w-3.5" />
                    </button>
                    <button
                      type="button"
                      onClick={() => void removeEntry(entry.id)}
                      disabled={busy !== null}
                      aria-label={`Remove ${entry.name} from the list`}
                      className="shrink-0 rounded-lg p-1 text-cream-400 transition-colors hover:bg-coral/10 hover:text-coral-dark disabled:opacity-50"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
          <p className="text-[11px] text-cream-400">
            Remove only unregisters here; your files on disk are kept.
          </p>
        </section>
      )}

      {!tauri && (
        <p className="text-[12px] text-cream-400">
          Persistence buttons need the desktop app. The canvas below works in any
          runtime.
        </p>
      )}
      {status && (
        <p className="rounded-xl bg-terracotta-100/50 px-3 py-2 text-[12px] text-terracotta-500">
          {status}
        </p>
      )}
      {error && (
        <p className="rounded-xl bg-coral/10 px-3 py-2 text-[12px] text-coral-dark">
          {error}
        </p>
      )}
      {warnings.length > 0 && (
        <div className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-[12px] text-amber-700">
          <div className="mb-1 flex items-center gap-1.5 font-medium">
            <AlertTriangle className="h-3.5 w-3.5" />
            Load warnings
          </div>
          <ul className="list-disc pl-5">
            {warnings.slice(0, 8).map((w, i) => (
              <li key={`${i}:${w}`}>{w}</li>
            ))}
          </ul>
        </div>
      )}

      <section className="flex flex-col gap-2 rounded-2xl border border-cream-200 bg-cream-50 p-3">
        <div className="flex items-center gap-1.5 text-[12px] font-medium text-cream-700">
          <Sparkles className="h-3.5 w-3.5 text-terracotta" />
          Generate
        </div>
        <textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="Describe what to generate (e.g. a pricing section coherent with our app)."
          rows={3}
          className="w-full resize-y rounded-xl border border-cream-200 bg-white px-3 py-2 text-[13px] text-cream-800 outline-none focus:border-terracotta/40"
        />
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={runGenerate}
            disabled={
              !tauri ||
              preparing ||
              streamStatus === "streaming" ||
              prompt.trim().length === 0
            }
            className="inline-flex items-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[13px] font-medium text-white shadow-soft-sm transition-colors hover:bg-terracotta-500 disabled:opacity-50"
          >
            <Sparkles className="h-4 w-4" />
            Generate
          </button>
          {streamStatus === "streaming" && (
            <button
              type="button"
              onClick={cancelGeneration}
              className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100"
            >
              <Square className="h-4 w-4" />
              Stop
            </button>
          )}
          <span className="text-[12px] text-cream-400">
            {streamStatus === "streaming"
              ? "Streaming…"
              : streamStatus === "done"
                ? "Done."
                : streamStatus === "cancelled"
                  ? "Cancelled."
                  : streamStatus === "error"
                    ? "Error."
                    : ""}
          </span>
        </div>
        {streamError && (
          <p className="rounded-xl bg-coral/10 px-3 py-2 text-[12px] text-coral-dark">
            {streamError}
          </p>
        )}

        {/* Per-node edit affordance: select a node on the canvas, then describe a
            change. Only that node's markup is sent + swapped. */}
        <div className="mt-1 flex flex-col gap-2 border-t border-cream-200 pt-2">
          <div className="flex items-center gap-1.5 text-[12px] font-medium text-cream-700">
            <Wand2 className="h-3.5 w-3.5 text-terracotta" />
            Edit selected node
            <span className="font-normal text-cream-400">
              {selectedId ? `· ${selectedId}` : "· click a node on the canvas"}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <input
              type="text"
              value={editInstruction}
              onChange={(e) => setEditInstruction(e.target.value)}
              placeholder="Describe the change (e.g. make it the brand accent)."
              disabled={!selectedId}
              className="min-w-0 flex-1 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[13px] text-cream-800 outline-none focus:border-terracotta/40 disabled:opacity-50"
            />
            <button
              type="button"
              onClick={runEdit}
              disabled={
                !tauri ||
                !selectedId ||
                preparing ||
                streamStatus === "streaming" ||
                editInstruction.trim().length === 0
              }
              className="inline-flex items-center gap-1.5 rounded-xl bg-white px-3 py-2 text-[13px] font-medium text-cream-700 shadow-soft-sm transition-colors hover:bg-cream-100 disabled:opacity-50"
            >
              <Wand2 className="h-4 w-4" />
              Edit
            </button>
          </div>
        </div>
      </section>

      <div className="min-h-0 flex-1">
        <Canvas
          project={project}
          onManifestChange={onManifestChange}
          onSelect={setSelectedId}
        />
      </div>
    </div>
  );
}

export default DesignView;
