// WorkingSetCard — per-project extra writable folders outside the project root
// (Slice 2 of the permission broker).
//
// Shows the persisted "working set" from `project.metadata.workingSet ?? []`.
// Each folder has a Remove button that calls `remove_project_working_set_folder_cmd`
// and is reflected optimistically (the folder disappears immediately; reverts on error).
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
  // Local optimistic state: starts from the prop; reflects adds/removes
  // immediately; reverts on error.
  const [localFolders, setLocalFolders] = useState<string[]>(() => workingSet ?? []);

  // Single in-flight mutex: only one add or remove at a time (prevents
  // concurrent IPC calls from racing the optimistic state).
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [error, setError] = useState<string | null>(null);

  // Tracks the last confirmed on-disk state so the prop-sync effect can tell
  // whether the incoming prop is a genuine refetch (different from what we wrote)
  // or just the parent echoing our own optimistic write back.
  // Set on every successful IPC write; cleared on error-revert. This prevents
  // a stale prop from overwriting a confirmed write that hasn't reached the
  // parent yet. Mirrors pendingModeRef in SandboxModeSelector.
  const pendingFoldersRef = useRef<string[] | null>(null);

  // Unmount guard: the Tauri folder dialog can stay open for seconds; navigating
  // away while it is open must not setState on a dead component.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Prop-sync effect: keep localFolders in sync when the parent prop changes
  // (e.g. 10-second project refetch), but never clobber a confirmed optimistic
  // state with a stale prop. Only update when not in-flight AND the incoming
  // value differs from what we last confirmed we wrote. Mirrors the pattern
  // in SandboxModeSelector (shouldAdoptProp equivalent inline here).
  useEffect(() => {
    const incoming = workingSet ?? [];
    // If a write is in-flight, the incoming prop is almost certainly stale.
    if (busyRef.current) return;
    // If we have a pending confirmed write, don't let the parent echo a stale
    // snapshot back. But the backend may have MORE folders than we wrote (e.g. a
    // folder added via "Allow & remember" consent path). Accept the incoming prop
    // as soon as it is a SUPERSET of what we wrote — i.e. every folder we wrote
    // is present in incoming. A set-equal guard would never clear when the backend
    // canonicalizes paths or adds extra entries, freezing the card forever.
    if (pendingFoldersRef.current !== null) {
      const pending = pendingFoldersRef.current;
      if (pending.every((f) => incoming.includes(f))) {
        // Backend has absorbed our write (and may have added more) — clear the
        // guard and adopt the authoritative server state.
        pendingFoldersRef.current = null;
        setLocalFolders(incoming);
      }
      // If the backend hasn't caught up yet, leave localFolders as-is (it already
      // reflects our optimistic write) and wait for the next prop update.
      return;
    }
    setLocalFolders(incoming);
  }, [workingSet]);

  const handleRemove = useCallback(
    async (folder: string) => {
      if (busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      setError(null);
      // Optimistic: remove immediately.
      const previous = localFolders;
      const next = previous.filter((f) => f !== folder);
      setLocalFolders(next);
      try {
        await invokeBackendCommand<void>("remove_project_working_set_folder_cmd", {
          projectId,
          folder,
        });
        if (!mountedRef.current) return;
        pendingFoldersRef.current = next;
        onWorkingSetChange?.(next);
      } catch (e) {
        if (!mountedRef.current) return;
        // Revert on failure.
        setLocalFolders(previous);
        pendingFoldersRef.current = null;
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
          // Release the ref lock even when unmounted so it doesn't leak.
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
      // Avoid client-side duplicates (the backend also deduplicates, but we
      // skip the IPC call entirely if the folder is already shown).
      if (localFolders.includes(picked)) return;
      // Optimistic: add the folder immediately.
      const previous = localFolders;
      const next = [...previous, picked];
      setLocalFolders(next);
      try {
        await invokeBackendCommand<void>("add_project_working_set_folder_cmd", {
          projectId,
          folder: picked,
        });
        if (!mountedRef.current) return;
        pendingFoldersRef.current = next;
        onWorkingSetChange?.(next);
      } catch (e) {
        if (!mountedRef.current) return;
        // Revert on IPC failure.
        setLocalFolders(previous);
        pendingFoldersRef.current = null;
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
      // Release the ref lock unconditionally; only write React state when mounted.
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
        <p className="rounded-lg bg-coral/8 px-3 py-2 text-[11px] text-coral-dark">
          {error}
        </p>
      )}
    </div>
  );
}

export default WorkingSetCard;
