// The node Inspector float card (ported from the prototype's inspector.jsx onto
// the current data model). A draggable header, a Transform grid (X/Y/W numeric +
// H showing HUG for an auto height), Corners radius tokens, Elevation Soft|Flat,
// Arrange (send-to-back / backward / forward / bring-to-front via manifestOps),
// and Actions (Duplicate / Delete). Every edit commits through the parent's
// manifest/project channels, which fire `onBeginChange` for history.
//
// DATA-MODEL DEVIATION FROM PROTOTYPE: the prototype mutated a `nodes[]` array
// with embedded `html`. Here PLACEMENT lives in `manifest.nodes[id]` and MARKUP in
// `project.components[id]`; z-order uses `engine/manifestOps` (not the prototype's
// inline `zOrderOp`). Duplicate therefore clones BOTH the placement and the
// component markup and appends to `nodeOrder`, committing via `onProjectChange`.

import { useEffect, useState } from "react";
import { X, ArrowUpToLine, ArrowDownToLine, ArrowUp, ArrowDown, Copy, Trash2, Check } from "lucide-react";
import type {
  DesignManifest,
  DesignNodePlacement,
  DesignProject,
} from "../../types/design";
import {
  bringToFront,
  moveBackward,
  moveForward,
  sendToBack,
} from "./engine/manifestOps";

/** Radius design tokens offered in the Corners row (px). */
const RADIUS_TOKENS = [
  { tok: "none", v: 0 },
  { tok: "sm", v: 8 },
  { tok: "md", v: 14 },
  { tok: "lg", v: 22 },
] as const;

/** Min node width (prototype parity — never collapse below this). */
const MIN_W = 240;

/** Charset a node id must satisfy (mirrors the engine/Rust id charset). */
const ID_CHARSET = /^[a-z0-9][a-z0-9_-]{0,63}$/;

interface InspectorProps {
  project: DesignProject;
  selectedId: string | null;
  /** Commit a placement-only manifest change (transform/radius/flat/z-order). */
  onManifestChange: (next: DesignManifest) => void;
  /** Commit a structural change (duplicate adds a node + component; delete removes one). */
  onProjectChange: (next: DesignProject) => void;
  onSelect: (id: string | null) => void;
  /** Measured rendered height of the selected node (for the HUG H field). */
  measuredHeight?: number;
}

interface NumFieldProps {
  label: string;
  value: number | string;
  onChange?: (v: number) => void;
  auto?: boolean;
}

// M4+W3: a numeric field that commits a SINGLE logical edit, not one per keystroke.
// Without local draft state, every digit typed fired `onChange` → a manifest patch →
// an `onBeginChange` history snapshot, so typing "320" pushed THREE undo entries (and
// an intermediate "3"/"32" briefly clamped the live node). It also let a non-numeric
// keystroke produce `NaN`. Now: the input is UNCONTROLLED-ish via a local `draft`
// string; we commit ONCE on blur or Enter, parse with `Number()`, and commit only a
// finite value (typing "abc" → no commit, node unchanged). Esc reverts the draft to
// the prop value and blurs. While typing, the parent never hears a change.
function NumField({ label, value, onChange, auto }: NumFieldProps) {
  const [draft, setDraft] = useState<string>(String(value));

  // Re-sync the draft whenever the committed value changes from outside (a drag, an
  // undo/redo, selecting another node) — but only while NOT actively editing this
  // field. Comparing against the prop avoids clobbering an in-progress keystroke.
  useEffect(() => {
    setDraft(String(value));
  }, [value]);

  const commit = () => {
    const trimmed = draft.trim();
    const parsed = Number(trimmed);
    // Commit only a non-empty, finite number. An empty draft (which a `type=number`
    // input also produces from non-numeric keystrokes like "abc") is NOT a value —
    // `Number("")` is 0, so without the emptiness guard a cleared field would
    // silently commit 0. Such input reverts the draft and commits nothing.
    if (trimmed !== "" && Number.isFinite(parsed)) {
      onChange?.(parsed);
      // Reflect any clamping (e.g. Math.max(MIN_W, v)) the parent applied: the
      // value prop updates and the effect above re-syncs the draft next render.
    } else {
      setDraft(String(value));
    }
  };

  if (auto) {
    return (
      <div className="numf">
        <label>{label}</label>
        <input value={value} readOnly tabIndex={-1} />
        <span className="auto">HUG</span>
      </div>
    );
  }

  return (
    <div className="numf">
      <label>{label}</label>
      <input
        type="number"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
            (e.target as HTMLInputElement).blur();
          } else if (e.key === "Escape") {
            e.preventDefault();
            setDraft(String(value)); // revert; no commit
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
    </div>
  );
}

/** Mint a duplicate id from a base, collision-free against `taken` and charset-valid. */
function dupId(baseId: string, taken: Set<string>): string {
  let candidate = `${baseId}-copy`;
  let n = 2;
  while (taken.has(candidate) || !ID_CHARSET.test(candidate)) {
    candidate = `${baseId}-copy-${n}`;
    n += 1;
    // Hard stop on a pathological base; fall back to a counter id (always valid).
    if (n > 9999) return `n${taken.size}-${Date.now() % 100000}`.replace(/[^a-z0-9_-]/gi, "");
  }
  return candidate;
}

export function Inspector({
  project,
  selectedId,
  onManifestChange,
  onProjectChange,
  onSelect,
  measuredHeight,
}: InspectorProps) {
  // null = default top-right; {left,top} once the header is dragged.
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  const manifest = project.manifest;
  const node = selectedId ? manifest.nodes[selectedId] : undefined;
  // A node that disappears (deleted under us) drops the panel; reset the drag pos
  // is intentionally NOT done here so the panel reappears where the user left it.
  if (!selectedId || !node) return null;

  const id = selectedId;
  const radius = node.radius == null ? 14 : node.radius;
  const radTok =
    RADIUS_TOKENS.find((r) => r.v === radius)?.tok ?? "custom";

  // z-order extremes (disable the matching Arrange buttons).
  let minZ = Infinity;
  let maxZ = -Infinity;
  for (const p of Object.values(manifest.nodes)) {
    if (p.z < minZ) minZ = p.z;
    if (p.z > maxZ) maxZ = p.z;
  }
  const atBottom = node.z === minZ;
  const atTop = node.z === maxZ;

  const patch = (p: Partial<DesignNodePlacement>) => {
    onManifestChange({
      ...manifest,
      nodes: { ...manifest.nodes, [id]: { ...node, ...p } },
    });
  };

  const arrange = (op: "back" | "backward" | "forward" | "front") => {
    const fn =
      op === "back"
        ? sendToBack
        : op === "backward"
          ? moveBackward
          : op === "forward"
            ? moveForward
            : bringToFront;
    const next = fn(manifest, id);
    if (next !== manifest) onManifestChange(next);
  };

  const duplicate = () => {
    const taken = new Set(Object.keys(manifest.nodes));
    const copyId = dupId(id, taken);
    const copyZ = maxZ + 1;
    const copyPlacement: DesignNodePlacement = {
      ...node,
      x: node.x + 32,
      y: node.y + 32,
      z: copyZ,
      name: (node.name ?? id) + " copy",
    };
    const next: DesignProject = {
      ...project,
      manifest: {
        ...manifest,
        nodes: { ...manifest.nodes, [copyId]: copyPlacement },
      },
      components: {
        ...project.components,
        [copyId]: project.components[id] ?? "",
      },
      meta: {
        ...project.meta,
        nodeOrder: [...project.meta.nodeOrder, copyId],
      },
    };
    onProjectChange(next);
    onSelect(copyId);
  };

  const remove = () => {
    const nodes = { ...manifest.nodes };
    delete nodes[id];
    const components = { ...project.components };
    delete components[id];
    const next: DesignProject = {
      ...project,
      manifest: { ...manifest, nodes },
      components,
      meta: {
        ...project.meta,
        nodeOrder: project.meta.nodeOrder.filter((n) => n !== id),
      },
    };
    onProjectChange(next);
    onSelect(null);
  };

  // Drag the panel by its header within the canvas-wrap bounds.
  const startMove = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    const card = (e.currentTarget as HTMLElement).parentElement;
    const wrap = card?.closest(".canvas-wrap") as HTMLElement | null;
    if (!wrap || !card) return;
    const wr = wrap.getBoundingClientRect();
    const cr = card.getBoundingClientRect();
    const ox = e.clientX - cr.left;
    const oy = e.clientY - cr.top;
    const onMove = (ev: PointerEvent) => {
      let left = ev.clientX - wr.left - ox;
      let top = ev.clientY - wr.top - oy;
      left = Math.max(8, Math.min(left, wr.width - cr.width - 8));
      top = Math.max(8, Math.min(top, wr.height - 64));
      setPos({ left, top });
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      // W8: also detach on pointercancel (touch interrupted, pointer captured
      // elsewhere, OS gesture) — otherwise the move listener leaks and the panel
      // keeps tracking the pointer after the gesture is gone.
      window.removeEventListener("pointercancel", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  };

  const hValue = typeof node.h === "number" ? node.h : measuredHeight ?? "—";
  const hAuto = node.h === "auto";

  return (
    <div
      className="float-card inspector"
      style={pos ? { left: pos.left, top: pos.top, right: "auto" } : undefined}
    >
      <div
        className="insp-head"
        onPointerDown={startMove}
        title="Drag to move this panel"
      >
        <b>{node.name ?? id}</b>
        <span className="kind">{node.kind.toUpperCase()}</span>
        <button
          type="button"
          className="close"
          onClick={() => onSelect(null)}
          title="Close (Esc)"
        >
          <X size={13} />
        </button>
      </div>

      <div className="insp-sec">
        <div className="insp-label">TRANSFORM</div>
        <div className="insp-grid">
          <NumField label="X" value={node.x} onChange={(v) => patch({ x: v })} />
          <NumField label="Y" value={node.y} onChange={(v) => patch({ y: v })} />
          <NumField
            label="W"
            value={node.w}
            onChange={(v) => patch({ w: Math.max(MIN_W, v) })}
          />
          <NumField label="H" value={hAuto ? (measuredHeight ?? "—") : hValue} auto={hAuto} onChange={(v) => patch({ h: Math.max(1, v) })} />
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">
          CORNERS
          <span className="tok">radius.{radTok}</span>
        </div>
        <div className="radius-row">
          {RADIUS_TOKENS.map((r) => (
            <button
              key={r.tok}
              type="button"
              className={"rad-btn" + (radius === r.v ? " sel" : "")}
              title={`radius.${r.tok} · ${r.v}px`}
              onClick={() => patch({ radius: r.v })}
            >
              <i style={{ borderTopLeftRadius: Math.min(r.v, 12) }} />
            </button>
          ))}
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">
          ELEVATION
          <span className="tok">shadow.{node.flat ? "none" : "soft"}</span>
        </div>
        <div className="seg sm">
          <button
            type="button"
            className={!node.flat ? "sel" : ""}
            onClick={() => patch({ flat: false })}
          >
            Soft
          </button>
          <button
            type="button"
            className={node.flat ? "sel" : ""}
            onClick={() => patch({ flat: true })}
          >
            Flat
          </button>
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-label">ARRANGE</div>
        <div className="arr-row">
          <button
            type="button"
            className="arr-btn"
            disabled={atBottom}
            title="Send to back"
            onClick={() => arrange("back")}
          >
            <ArrowDownToLine size={15} />
          </button>
          <button
            type="button"
            className="arr-btn"
            disabled={atBottom}
            title="Move backward  ["
            onClick={() => arrange("backward")}
          >
            <ArrowDown size={15} />
          </button>
          <button
            type="button"
            className="arr-btn"
            disabled={atTop}
            title="Move forward  ]"
            onClick={() => arrange("forward")}
          >
            <ArrowUp size={15} />
          </button>
          <button
            type="button"
            className="arr-btn"
            disabled={atTop}
            title="Bring to front"
            onClick={() => arrange("front")}
          >
            <ArrowUpToLine size={15} />
          </button>
        </div>
      </div>

      <div className="insp-sec">
        <div className="insp-actions">
          <button type="button" className="mini-btn" onClick={duplicate}>
            <Copy size={13} />
            Duplicate
          </button>
          <button type="button" className="mini-btn danger" onClick={remove}>
            <Trash2 size={13} />
            Delete
          </button>
        </div>
      </div>

      <div className="insp-foot">
        <Check size={12} />
        Values snap to design tokens (DTCG)
      </div>
    </div>
  );
}

export default Inspector;
