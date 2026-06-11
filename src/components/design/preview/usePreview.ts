// usePreview — the Phase B preview/visual-check flows for the design module.
//
// Encapsulates the two backend round-trips so DesignView only has to wire its
// existing primitives (runExport, the working-folder path, the registry-remember
// path) and surface the busy flags + critique text in the UI:
//
//   openPreview(mode)  : ensure the export file is fresh (runExport(mode) — the
//                        caller's existing export path, which also consolidates the
//                        in-memory project to disk via design_write_export), then
//                        invoke `design_preview_open(workingFolderPath, mode)` to
//                        spawn/refresh the read-only preview window.
//
//   visualCheck(focus?): capture the live preview window
//                        (`design_preview_capture`) → record the resulting
//                        preview.png in the registry (best-effort, for the project
//                        thumbnail) → ask the local censor AI to critique it
//                        (`design_visual_critique`). Returns the critique text.
//                        Guarded so two visual checks never run concurrently.
//
// All errors are surfaced as clean strings (the Rust commands already return
// human-readable messages: "Export not found — run Export first", an
// Ollama-unconfigured message, a macOS-capture clean Err, etc.). The hook adds a
// hint to the capture-not-open case so the user knows to open the preview first.

import { useCallback, useRef, useState } from "react";
import type { ExportMode } from "../export/exportCode";

/** Shape returned by the Rust `design_preview_capture` command. */
interface PreviewCaptureResult {
  path: string;
  bytes: number;
}

/** Shape returned by the Rust `design_visual_critique` command. */
interface VisualCritiqueResult {
  critique: string;
}

export interface UsePreviewDeps {
  /** Read the current working-folder path at CALL time (never a stale closure). */
  getFolder: () => string;
  /** True only inside the Tauri desktop runtime (IPC available). */
  tauri: boolean;
  /** Generic backend invoker (DesignView passes invokeBackendCommand). */
  invoke: <T = unknown>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T>;
  /**
   * DesignView's EXISTING export path: writes export-<mode>.html to the working
   * folder via design_write_export. Resolves to TRUE only when the export was written
   * successfully; FALSE when it failed (DesignView surfaces the underlying error
   * itself). We await this before opening the preview and ABORT the open on FALSE so a
   * failed export can never leave the window showing a stale (or missing) export.
   */
  runExport: (mode: ExportMode) => Promise<boolean>;
  /**
   * Best-effort registry remember used to record the project thumbnail after a
   * successful capture. DesignView passes a wrapper over rememberProject with the
   * relative "preview.png" path; contractSha is intentionally omitted so the Rust
   * upsert preserves any approved hash. A no-op outside Tauri.
   */
  rememberThumbnail: (workingFolderPath: string) => void;
  /** Optional transient success toast (e.g. "Preview opened"). */
  onToast?: (message: string) => void;
}

/**
 * The terminal result of a visualCheck. A discriminated union so the caller can
 * patch its assistant card WITHOUT racing the hook's `error` state (which only
 * updates on the NEXT render — a closure that read `preview.error` right after the
 * await would see a stale value). `skipped` is the concurrency-guard short-circuit.
 */
export type VisualCheckOutcome =
  | { kind: "ok"; critique: string }
  | { kind: "error"; message: string }
  | { kind: "skipped" };

export interface UsePreview {
  /** Open (or refresh) the read-only preview window for `mode`. */
  openPreview: (mode: ExportMode) => Promise<void>;
  /**
   * SYNCHRONOUSLY claim the visual-check slot. Returns `true` if the caller now owns the
   * in-flight check (it MUST then call `visualCheck`), or `false` if a check is already
   * running (the caller should do nothing). This lets the UI decide WHETHER to push its
   * user/working cards BEFORE any await — two clicks in the same tick can't both claim,
   * so no duplicate cards. The claim is released by the matching `visualCheck`.
   */
  beginCheck: () => boolean;
  /**
   * Capture + critique the live preview; resolves to a discriminated outcome. May be
   * called directly (it claims the slot itself) or AFTER a successful `beginCheck` (it
   * adopts the existing claim). Either way it releases the claim when it settles.
   */
  visualCheck: (focus?: string) => Promise<VisualCheckOutcome>;
  /** True while openPreview is exporting/opening. */
  opening: boolean;
  /** True while a visualCheck round-trip is in flight (also the concurrency guard). */
  checking: boolean;
  /** The last error surfaced by either flow (cleared at the start of the next run). */
  error: string | null;
}

/** Hint appended ONLY to the capture "window not open" error so the user knows to open it. */
const NOT_OPEN_HINT = " — open the preview first (the Preview button in the toolbar).";

/**
 * Stable substring of the Rust `design_preview_capture` "window not open" error
 * (`PREVIEW_NOT_OPEN_ERR` in design_preview.rs). We append [`NOT_OPEN_HINT`] ONLY when the
 * backend error contains this marker — a real capture/timeout/critique failure must NOT be
 * told to "open the preview first" (the window IS open in those cases). Keep in sync.
 */
const PREVIEW_NOT_OPEN_MARK = "preview window is not open";

export function usePreview(deps: UsePreviewDeps): UsePreview {
  const { getFolder, tauri, invoke, runExport, rememberThumbnail, onToast } = deps;

  const [opening, setOpening] = useState(false);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-entry guard for visualCheck. A ref (not just the `checking` state) so the
  // guard is synchronous — two clicks in the same tick can't both pass. `beginCheck`
  // claims it; `visualCheck` adopts an existing claim or makes its own.
  const checkingRef = useRef(false);
  // Marks an ACTIVELY-executing visualCheck body (vs a merely-claimed slot). Distinguishes
  // "beginCheck claimed it for this call" from "another call is mid-flight".
  const runningRef = useRef(false);

  // Synchronously claim the slot. Returns false when a check already owns it. The matching
  // visualCheck releases it (and also flips the `checking` state for the UI). NOTE: this
  // does NOT call setChecking — that would re-render before visualCheck runs; visualCheck
  // owns the state transition. The ref alone gives the synchronous mutual exclusion.
  const beginCheck = useCallback((): boolean => {
    if (checkingRef.current) return false;
    checkingRef.current = true;
    return true;
  }, []);

  const openPreview = useCallback(
    async (mode: ExportMode) => {
      const folder = getFolder().trim();
      if (!folder || !tauri) {
        setError("Choose a working folder first.");
        return;
      }
      setOpening(true);
      setError(null);
      try {
        // Refresh the on-disk export the preview window will load. runExport also
        // sanitizes + writes the file; we await it so the window never reads a stale
        // (or missing) export. On FAILURE it returns false and has already surfaced its
        // own error — we MUST abort here and NOT open the window over a stale export.
        const exported = await runExport(mode);
        if (!exported) {
          // DesignView's runExport set the visible error; don't overwrite it. Just stop.
          return;
        }
        await invoke("design_preview_open", {
          workingFolderPath: folder,
          mode,
        });
        onToast?.("Preview opened");
      } catch (e) {
        setError(String(e));
      } finally {
        setOpening(false);
      }
    },
    [getFolder, tauri, invoke, runExport, onToast],
  );

  const visualCheck = useCallback(
    async (focus?: string): Promise<VisualCheckOutcome> => {
      // Concurrency guard: never two visual checks at once (each captures the same single
      // preview window + drives the local AI; overlapping runs would race). `runningRef`
      // marks an ACTIVE async body; `checkingRef` marks a CLAIMED slot (beginCheck may have
      // already set it for this very call). A direct call while one is running → skip.
      if (runningRef.current) {
        // A check is genuinely executing. If THIS call had pre-claimed via beginCheck we
        // would not reach here (beginCheck returns false when already claimed), so this is
        // a direct re-entrant call — short-circuit without touching the existing claim.
        return { kind: "skipped" };
      }
      // Adopt an existing claim (from beginCheck) or make our own (direct call).
      if (!checkingRef.current) checkingRef.current = true;
      runningRef.current = true;

      const folder = getFolder().trim();
      if (!folder || !tauri) {
        // Release the claim we own before bailing (otherwise the slot would wedge).
        runningRef.current = false;
        checkingRef.current = false;
        const message = "Choose a working folder first.";
        setError(message);
        return { kind: "error", message };
      }
      setChecking(true);
      setError(null);
      try {
        // 1) Capture the live preview window to preview.png. If the window isn't
        //    open the backend returns a clean "not open" error — surface it with a
        //    hint rather than silently opening (the user controls the window).
        let capture: PreviewCaptureResult;
        try {
          capture = await invoke<PreviewCaptureResult>("design_preview_capture", {
            workingFolderPath: folder,
          });
        } catch (e) {
          // Append the "open it first" hint ONLY when the backend says the window is not
          // open. A genuine capture/timeout error means the window IS open — telling the
          // user to open it would be misleading, so we surface those verbatim.
          const raw = String(e);
          const isNotOpen = raw.toLowerCase().includes(PREVIEW_NOT_OPEN_MARK);
          const message = isNotOpen ? raw + NOT_OPEN_HINT : raw;
          setError(message);
          return { kind: "error", message };
        }

        // 2) Best-effort: record the freshly-captured preview.png as the project
        //    thumbnail. Never blocks or fails the critique.
        if (capture && typeof capture.path === "string") {
          rememberThumbnail(folder);
        }

        // 3) Critique the screenshot via the local censor AI. A clean Err here
        //    (Ollama unconfigured, macOS-unverified capture, etc.) is surfaced
        //    verbatim — the backend message is already user-facing.
        const trimmedFocus = focus?.trim();
        const result = await invoke<VisualCritiqueResult>(
          "design_visual_critique",
          {
            workingFolderPath: folder,
            ...(trimmedFocus ? { focus: trimmedFocus } : {}),
          },
        );
        const critique = result?.critique ?? "";
        if (!critique.trim()) {
          const message = "The visual critique returned no text.";
          setError(message);
          return { kind: "error", message };
        }
        return { kind: "ok", critique };
      } catch (e) {
        const message = String(e);
        setError(message);
        return { kind: "error", message };
      } finally {
        runningRef.current = false;
        checkingRef.current = false;
        setChecking(false);
      }
    },
    [getFolder, tauri, invoke, rememberThumbnail],
  );

  return { openPreview, beginCheck, visualCheck, opening, checking, error };
}
