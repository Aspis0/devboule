// The collapsible LAYERS float card: every node listed by z DESCENDING (top layer
// first), with a visibility eye toggle, a delete action, and click-to-select. All
// mutations go through callbacks the canvas owns so they flow through the SAME
// manifest-commit + history path as drag/inspector edits (single source of truth).

import { useState } from "react";
import {
  Layers,
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  Trash2,
  Code2,
  Image,
} from "lucide-react";
import type { DesignManifest, DesignNodePlacement } from "../../../types/design";

interface LayerEntry {
  id: string;
  placement: DesignNodePlacement;
}

interface LayersPanelProps {
  manifest: DesignManifest;
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** Toggle a node's `hidden` flag (commits + history). */
  onToggleHidden: (id: string) => void;
  /** Remove a node entirely (commits + history). */
  onDelete: (id: string) => void;
}

/** A stable display label for a node row. */
function layerName(id: string, p: DesignNodePlacement): string {
  return p.name ?? id;
}

export function LayersPanel({
  manifest,
  selectedId,
  onSelect,
  onToggleHidden,
  onDelete,
}: LayersPanelProps) {
  const [open, setOpen] = useState(true);

  // Sort by z DESC (top paint order first), ties broken by id for determinism.
  const entries: LayerEntry[] = Object.entries(manifest.nodes)
    .map(([id, placement]) => ({ id, placement }))
    .sort((a, b) =>
      b.placement.z !== a.placement.z
        ? b.placement.z - a.placement.z
        : a.id.localeCompare(b.id),
    );

  return (
    <div className="float-card layers">
      <button
        type="button"
        className="layers-head"
        onClick={() => setOpen((v) => !v)}
      >
        <Layers size={14} />
        LAYERS
        <span className="n">{entries.length}</span>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>
      {open && (
        <div className="layers-list">
          {entries.length === 0 && (
            <div className="layers-empty">No layers yet</div>
          )}
          {entries.map(({ id, placement }) => {
            const hidden = !!placement.hidden;
            const KindIcon = placement.kind === "svg" ? Image : Code2;
            return (
              <button
                key={id}
                type="button"
                className={
                  "layer-row" +
                  (id === selectedId ? " sel" : "") +
                  (hidden ? " is-hidden" : "")
                }
                onClick={() => onSelect(id)}
              >
                <KindIcon size={13} style={{ opacity: 0.7, flex: "none" }} />
                <span className="lr-name">{layerName(id, placement)}</span>
                <span className="layer-acts">
                  <span
                    className="lr-act vis"
                    role="button"
                    tabIndex={-1}
                    title={hidden ? "Show" : "Hide"}
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleHidden(id);
                    }}
                  >
                    {hidden ? <EyeOff size={13} /> : <Eye size={13} />}
                  </span>
                  <span
                    className="lr-act danger"
                    role="button"
                    tabIndex={-1}
                    title="Delete"
                    onClick={(e) => {
                      e.stopPropagation();
                      onDelete(id);
                    }}
                  >
                    <Trash2 size={13} />
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
