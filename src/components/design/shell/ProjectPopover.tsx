// ProjectPopover — the "DESIGN PROJECTS" picker (prototype's ProjectPopover, left
// variant). Lists the recent-projects registry as `.pop-row`s with a thumbnail, the
// project name + working-folder path, and a Check on the OPEN project. Each row
// loads its working folder; the per-row pencil/X drive the EXISTING inline rename
// and remove (unregister-only) flows. Two footer rows run the create + open-folder
// picker flows.
//
// Behavior (rename/remove/load/create) is OWNED by DesignView and passed in — this
// component is structure + wiring only. The registry list, rename draft, and the
// open project's working-folder path all come from props so the same source of
// truth drives both this popover and persistence.

import { useEffect, useRef, useState } from "react";
import { Palette, Folder, Check, Pencil, Trash2, X, FileText } from "lucide-react";
import type { DesignProjectEntry } from "../../../types/design";
import { Popover } from "./Popover";
import { thumbColorFromId } from "./format";

export interface ProjectPopoverProps {
  open: boolean;
  onClose: () => void;
  /** Registry entries (recent-first), as DesignView holds them. */
  recent: DesignProjectEntry[];
  /** The working folder of the currently-open project (Check + dedupe), "" if none. */
  currentFolder: string;
  busy: boolean;
  /** Inline-rename state, lifted to DesignView. */
  renamingId: string | null;
  renameDraft: string;
  setRenameDraft: (v: string) => void;
  beginRename: (entry: DesignProjectEntry) => void;
  commitRename: (id: string) => void;
  cancelRename: () => void;
  removeEntry: (id: string) => void;
  /** Open a registered entry's working folder. */
  openEntry: (entry: DesignProjectEntry) => void;
  /** Footer actions: pick a folder + create, or pick a folder + load. */
  onNewProject: () => void;
  onOpenFolder: () => void;
  /** Open the design.md contract editor for the open project. Undefined -> the row is
   * hidden (no project is open). */
  onEditContract?: () => void;
  /**
   * Backend invoker used to LAZILY read each entry's preview.png thumbnail
   * (design_read_thumbnail -> a `data:image/png;base64` URI, allowed by the CSP
   * img-src 'self' data:). Absent/web runtime -> the color block is the only art.
   */
  invoke?: <T = unknown>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T>;
  /** True only in the Tauri desktop runtime (thumbnails available). */
  tauri?: boolean;
}

export function ProjectPopover({
  open,
  onClose,
  recent,
  currentFolder,
  busy,
  renamingId,
  renameDraft,
  setRenameDraft,
  beginRename,
  commitRename,
  cancelRename,
  removeEntry,
  openEntry,
  onNewProject,
  onOpenFolder,
  onEditContract,
  invoke,
  tauri,
}: ProjectPopoverProps) {
  // Lazily-loaded preview.png thumbnails, keyed by entry id. `data:` URI on success,
  // `null` when the project has no preview.png yet (or any read error) — the row then
  // keeps its deterministic color block. The effect runs when the popover opens (or the
  // entry set changes) and fetches ONLY entries not already cached, so a registry update
  // while the popover is open (e.g. a fresh capture re-orders `recent`) does NOT clear and
  // re-fetch every thumbnail — already-loaded thumbs stay put (no flicker). An abort flag
  // drops late results after a close/unmount so we never setState on a stale render.
  const [thumbs, setThumbs] = useState<Record<string, string | null>>({});
  // Mirror of `thumbs` for the effect so reading the cache does NOT make the effect depend
  // on `thumbs` (which would re-run it on every fetch result — defeating the point).
  const thumbsRef = useRef(thumbs);
  thumbsRef.current = thumbs;

  useEffect(() => {
    if (!open || !tauri || !invoke) return;
    let cancelled = false;
    // Fetch only entries we have not already resolved (the cache survives deps changes
    // while open). `id in cache` covers both a loaded URI and a resolved-null.
    const cache = thumbsRef.current;
    const pending = recent.filter((entry) => !(entry.id in cache));
    if (pending.length === 0) return;
    void Promise.all(
      pending.map(async (entry) => {
        try {
          const uri = await invoke<string | null>("design_read_thumbnail", {
            workingFolderPath: entry.workingFolderPath,
          });
          if (cancelled) return;
          setThumbs((prev) => ({ ...prev, [entry.id]: uri ?? null }));
        } catch {
          if (cancelled) return;
          setThumbs((prev) => ({ ...prev, [entry.id]: null }));
        }
      }),
    );
    return () => {
      cancelled = true;
    };
  }, [open, tauri, invoke, recent]);

  // Reset the cache when the popover CLOSES so a later re-open re-fetches fresh thumbnails
  // (a capture may have changed a preview.png while it was closed). We DON'T reset on a
  // deps change while open — that is exactly the flicker this fix removes.
  useEffect(() => {
    if (!open) setThumbs({});
  }, [open]);

  return (
    <Popover open={open} onClose={onClose} className="left">
      <div className="pop-head">DESIGN PROJECTS</div>
      <div data-testid="design-recent">
        {recent.map((entry) => {
          const isOpen =
            currentFolder.trim().length > 0 &&
            entry.workingFolderPath === currentFolder.trim();
          const renaming = renamingId === entry.id;
          return (
            <div
              key={entry.id}
              data-testid="design-recent-item"
              className={"pop-row" + (isOpen ? " sel" : "")}
            >
              {thumbs[entry.id] ? (
                <img
                  className="thumb"
                  src={thumbs[entry.id] as string}
                  alt=""
                  style={{ objectFit: "cover" }}
                />
              ) : (
                <div
                  className="thumb"
                  style={{ background: thumbColorFromId(entry.id) }}
                />
              )}
              {renaming ? (
                <input
                  type="text"
                  value={renameDraft}
                  autoFocus
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename(entry.id);
                    if (e.key === "Escape") cancelRename();
                  }}
                  aria-label="Rename project"
                  style={{
                    flex: 1,
                    minWidth: 0,
                    border: "1px solid var(--border)",
                    borderRadius: 8,
                    padding: "5px 8px",
                    fontSize: 13,
                    outline: "none",
                    background: "var(--card)",
                  }}
                />
              ) : (
                <button
                  type="button"
                  onClick={() => openEntry(entry)}
                  disabled={busy}
                  title={entry.workingFolderPath}
                  style={{
                    flex: 1,
                    minWidth: 0,
                    display: "block",
                    textAlign: "left",
                  }}
                >
                  <b
                    style={{
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {entry.name}
                  </b>
                  <span
                    style={{
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {entry.workingFolderPath}
                  </span>
                </button>
              )}
              {renaming ? (
                <span className="layer-acts">
                  <button
                    type="button"
                    onClick={() => commitRename(entry.id)}
                    aria-label="Save name"
                    className="lr-act"
                    style={{ opacity: 0.85 }}
                  >
                    <Check size={15} />
                  </button>
                  <button
                    type="button"
                    onClick={cancelRename}
                    aria-label="Cancel rename"
                    className="lr-act"
                    style={{ opacity: 0.85 }}
                  >
                    <X size={15} />
                  </button>
                </span>
              ) : isOpen ? (
                <span className="check">
                  <Check size={15} />
                </span>
              ) : (
                <span className="layer-acts">
                  <button
                    type="button"
                    onClick={() => beginRename(entry)}
                    disabled={busy}
                    aria-label={`Rename ${entry.name}`}
                    className="lr-act"
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    type="button"
                    onClick={() => removeEntry(entry.id)}
                    disabled={busy}
                    aria-label={`Remove ${entry.name} from the list`}
                    className="lr-act danger"
                  >
                    <Trash2 size={14} />
                  </button>
                </span>
              )}
            </div>
          );
        })}
      </div>
      <div className="pop-sep" />
      {onEditContract ? (
        <button
          type="button"
          className="pop-row"
          disabled={busy}
          onClick={() => {
            onClose();
            onEditContract();
          }}
        >
          <span
            style={{
              color: "var(--accent)",
              display: "grid",
              placeItems: "center",
              width: 30,
            }}
          >
            <FileText size={16} />
          </span>
          <b style={{ fontWeight: 600 }}>Design contract…</b>
        </button>
      ) : null}
      <button
        type="button"
        className="pop-row"
        disabled={busy}
        onClick={() => {
          onClose();
          onNewProject();
        }}
      >
        <span
          style={{
            color: "var(--accent)",
            display: "grid",
            placeItems: "center",
            width: 30,
          }}
        >
          <Palette size={16} />
        </span>
        <b style={{ fontWeight: 600 }}>New project…</b>
      </button>
      <button
        type="button"
        className="pop-row"
        disabled={busy}
        onClick={() => {
          onClose();
          onOpenFolder();
        }}
      >
        <span
          style={{
            color: "var(--ink-2)",
            display: "grid",
            placeItems: "center",
            width: 30,
          }}
        >
          <Folder size={16} />
        </span>
        <b style={{ fontWeight: 600 }}>Open working folder…</b>
      </button>
    </Popover>
  );
}

export default ProjectPopover;
