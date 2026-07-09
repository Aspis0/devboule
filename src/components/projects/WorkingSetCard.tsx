// WorkingSetCard — per-project extra writable folders outside the project root
// (Slice 2 of the permission broker).
//
// Shows the persisted "working set" from `project.metadata.workingSet ?? []`.
// Each folder has a Remove button that calls `remove_project_working_set_folder_cmd`
// and is reflected by adopting the RETURNED canonical list (BLOCKER 2 fix: the backend
// canonicalizes /tmp → /private/tmp on macOS; trusting the return value avoids the
// pendingFoldersRef superset-guessing that previously caused display freezes).
// A "+ Add folder" button uses the Tauri dialog plugin (already in the repo, same
// pattern as DesignView.tsx) to pick a new directory and calls
// `add_project_working_set_folder_cmd`.
//
// Mirrors SandboxModeSelector for the busy/error and prop-sync patterns; mirrors
// CensorPanel for the section header style. Both are adjacent in the same dock tab.

import { useCallback, useEffect, useRef, useState } from "react";
import { FolderOpen, Trash2 } from "lucide-react";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import { stripSpoofChars } from "../agents/attentionNotifier";
import { shouldAdoptWorkingSet } from "./workingSetModel";

export interface WorkingSetCardProps {
  projectId: string;
  /** The effective working-set folders. Absent (undefined) means empty. */
  workingSet?: string[];
  /**
   * Called after a successful add or remove so the parent can patch the
   * in-memory project metadata without waiting for the next poll.
   */
  onWorkingSetChange?: (next: string[]) => void;
}

export function WorkingSetCard({
  projectId,
  workingSet,
  onWorkingSetChange,
}: WorkingSetCardProps) {
  // Local state: starts from the prop; on a successful add/remove is replaced by
  // the CANONICAL list returned by the backend (BLOCKER 2 fix — avoids the
  // /tmp → /private/tmp mismatch that caused display freezes on macOS).
  const [localFolders, setLocalFolders] = useState<string[]>(() => workingSet ?? []);

  // Single in-flight mutex: only one add or remove at a time.
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [error, setError] = useState<string | null>(null);

  // Stale-poll guard (BLOCKER 3 fix): after a successful add/remove we adopt the
  // CANONICAL list returned by the backend and record it here. The prop-sync
  // effect then waits until the parent's prop has SET-EQUAL caught up before
  // accepting it — a background poll whose disk read preceded the write cannot
  // clobber the canonical value we already received. Cleared when the prop
  // catches up, or when no write is pending (null = normal poll adoption).
  const lastWrittenRef = useRef<string[] | null>(null);

  // Unmount guard: the Tauri folder dialog can stay open for seconds; navigating
  // away while it is open must not setState on a dead component.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Prop-sync effect: adopt external updates (e.g. 10-second project refetch, or a
  // folder added via the "Allow & remember" consent path).
  // Uses shouldAdoptWorkingSet to guard against a stale background poll clobbering
  // the canonical list we just received from a successful add/remove:
  //   - busy in-flight → skip (don't touch the optimistic state).
  //   - lastWrittenRef set → only adopt once the prop SET-EQUALS lastWritten
  //     (parent has caught up); then clear the ref.
  //   - no pending write → adopt normally.
  useEffect(() => {
    const incoming = workingSet ?? [];
    if (!shouldAdoptWorkingSet(incoming, lastWrittenRef.current, busyRef.current)) return;
    // If we had a pending canonical write and the prop just matched it, clear the
    // guard so future external changes (unrelated to our write) adopt normally.
    if (lastWrittenRef.current !== null) {
      lastWrittenRef.current = null;
    }
    setLocalFolders(incoming);
  }, [workingSet]);

  const handleRemove = useCallback(
    async (folder: string) => {
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setError(null);
      // Optimistic: remove immediately for snappy UX; replaced by canonical list on success.
      const previous = localFolders;
      setLocalFolders(previous.filter((f) => f !== folder));
      try {
        // BLOCKER 2 fix: command now returns the canonical list — adopt it directly.
        const canonical = await invokeBackendCommand<string[]>(
          "remove_project_working_set_folder_cmd",
          { projectId, folder },
        );
        if (!mountedRef.current) return;
        // Record the canonical list so the prop-sync guard knows what to expect.
        lastWrittenRef.current = canonical;
        setLocalFolders(canonical);
        onWorkingSetChange?.(canonical);
      } catch (e) {
        if (!mountedRef.current) return;
        // Revert on failure.
        setLocalFolders(previous);
        setError(
          typeof e === "string"
            ? e
            : e instanceof Error
              ? e.message
              : "Could not remove the folder.",
        );
      } finally {
        if (mountedRef.current) {
          busyRef.current = false;
          setBusy(false);
        } else {
          busyRef.current = false;
        }
      }
    },
    [localFolders, projectId, onWorkingSetChange],
  );

  const handleAdd = useCallback(async () => {
    if (busyRef.current || !isTauriRuntime()) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const result = await open({
        directory: true,
        multiple: false,
        title: "Select a folder to add to the working set",
      });
      const picked =
        typeof result === "string" && result.trim() ? result.trim() : null;
      if (picked === null) {
        // User dismissed the dialog — no-op.
        return;
      }
      // Skip if already present (the backend also deduplicates, but avoid a round-trip).
      if (localFolders.includes(picked)) return;
      // Optimistic: add the raw picked path immediately; replaced by canonical on success.
      const previous = localFolders;
      setLocalFolders([...previous, picked]);
      try {
        // BLOCKER 2 fix: command now returns the canonical list — adopt it directly.
        // This corrects any /tmp → /private/tmp (or similar) canonicalization so the
        // displayed path matches what the backend stores and checks.
        const canonical = await invokeBackendCommand<string[]>(
          "add_project_working_set_folder_cmd",
          { projectId, folder: picked },
        );
        if (!mountedRef.current) return;
        // Record the canonical list so the prop-sync guard knows what to expect.
        lastWrittenRef.current = canonical;
        setLocalFolders(canonical);
        onWorkingSetChange?.(canonical);
      } catch (e) {
        if (!mountedRef.current) return;
        // Revert on IPC failure.
        setLocalFolders(previous);
        setError(
          typeof e === "string"
            ? e
            : e instanceof Error
              ? e.message
              : "Could not add the folder.",
        );
      }
    } catch {
      // Dialog plugin unavailable or dialog threw — swallow, no state change.
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setBusy(false);
    }
  }, [localFolders, projectId, onWorkingSetChange]);

  return (
    <div
      className="space-y-3"
      data-help-title="Working-set folders are extra directories outside the project root that agents may write to."
      data-help-lines="When a mini-coder tries to write outside the project root, a consent prompt appears.|Allow &amp; remember adds the folder here permanently; Allow once grants it for the next run only.|Remove a folder to revoke permanent write access; agents will be prompted again.|Folders added here are used on the NEXT agent spawn — the current run is not affected."
    >
      <div className="flex items-center gap-2">
        <FolderOpen className="h-4 w-4 text-teal" aria-hidden />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Working-set folders
        </h3>
      </div>

      {localFolders.length === 0 ? (
        <p className="text-[11px] text-cream-400">
          No extra folders granted. When an agent is blocked writing outside the
          project root, a consent prompt will appear.
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5" aria-label="Working-set folders">
          {localFolders.map((folder) => (
            <li
              key={folder}
              className="flex items-center justify-between gap-2 rounded-lg border border-cream-200 bg-cream-50 px-2.5 py-1.5"
            >
              <span className="min-w-0 truncate font-mono text-[11px] text-cream-700">
                {stripSpoofChars(folder)}
              </span>
              <button
                type="button"
                onClick={() => void handleRemove(folder)}
                disabled={busy}
                aria-label={`Remove ${folder} from working set`}
                className="shrink-0 rounded p-0.5 text-cream-400 hover:text-coral-dark disabled:opacity-60"
              >
                <Trash2 className="h-3.5 w-3.5" aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}

      <button
        type="button"
        onClick={() => void handleAdd()}
        disabled={busy}
        className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-600 hover:bg-cream-50 disabled:opacity-60"
      >
        <FolderOpen className="h-3.5 w-3.5" aria-hidden />
        {busy ? "Working…" : "+ Add folder"}
      </button>

      {error && (
        <p className="rounded-lg bg-coral/10 px-3 py-2 text-[11px] text-coral-dark">
          {error}
        </p>
      )}
    </div>
  );
}

export default WorkingSetCard;
