// The floating content-edit toolbar that hovers over the SELECTED inner element of a
// node in CE mode. Ported from the prototype's `ContentToolbar` (content-edit.jsx):
// it shows the element's tag, an Edit-text button, text/fill swatch-target toggles,
// the project's real color-token swatches, reorder up/down, delete, and an "Ask AI"
// accent button. It mounts in the CANVAS OVERLAY (a sibling of the world, INSIDE
// `.canvas-wrap`) — NEVER inside the sanitized node content — and is positioned by
// translating the selected element's client rect into `.canvas-wrap`-relative coords.
//
// POSITIONING (verbatim from the prototype): above the element by 46px; flips BELOW
// (+10px) when there is no room above; clamped horizontally into the wrap. It
// repositions whenever `version` (a CE bump on select / pan / zoom / reorder / color)
// changes, so a pan/zoom or DOM reorder keeps the toolbar pinned to its element.

import { useLayoutEffect, useState } from "react";
import { ArrowDown, ArrowUp, Sparkles, Trash2, Type } from "lucide-react";
import { elHasText } from "./contentEdit";
import type { ColorTokenSwatch } from "../engine/tokens";

/** Which inline property a swatch click applies. */
export type SwatchTarget = "text" | "fill";

/** Neutral fallback palette used when the project has no color tokens yet. Mirrors
 *  the prototype's hardcoded TOKEN_COLORS shape (name + value) so the toolbar always
 *  shows a usable swatch row. */
const FALLBACK_SWATCHES: ColorTokenSwatch[] = [
  { name: "neutral.ink", value: "#37291A" },
  { name: "neutral.muted", value: "#7A6B56" },
  { name: "neutral.accent", value: "#C14B1B" },
  { name: "neutral.cream", value: "#F3E3CB" },
  { name: "neutral.paper", value: "#FFFFFF" },
];

export interface ContentToolbarProps {
  /** The selected inner element the toolbar acts on. */
  el: HTMLElement;
  /** The `.canvas-wrap` element the toolbar is positioned relative to. */
  wrapEl: HTMLElement | null;
  /** Bumped to force a reposition (select / pan / zoom / reorder / recolor). */
  version: number;
  /** Real project color tokens; falls back to a neutral set when empty. */
  swatches: ColorTokenSwatch[];
  /** Enter inline text editing on the element (also reachable via double-click). */
  onEditText: () => void;
  /** Apply a color swatch to the element (text color or background fill). */
  onColor: (value: string, target: SwatchTarget) => void;
  /** Reorder the element among its siblings. */
  onMove: (dir: "up" | "down") => void;
  /** Remove the element from the node. */
  onRemove: () => void;
  /** Seed the composer with this element's edit context and exit CE mode. */
  onAskAi: () => void;
}

interface ToolbarPos {
  top: number;
  left: number;
}

export function ContentToolbar({
  el,
  wrapEl,
  version,
  swatches,
  onEditText,
  onColor,
  onMove,
  onRemove,
  onAskAi,
}: ContentToolbarProps) {
  const [pos, setPos] = useState<ToolbarPos | null>(null);
  const [mode, setMode] = useState<SwatchTarget>("text");

  // Reposition over the element. Runs after layout so the element's measured rect is
  // current; recomputed on every `version`/pan/zoom bump (the canvas bumps version).
  useLayoutEffect(() => {
    if (!el || !el.isConnected || !wrapEl) {
      setPos(null);
      return;
    }
    const er = el.getBoundingClientRect();
    const wr = wrapEl.getBoundingClientRect();
    let top = er.top - wr.top - 46;
    if (top < 8) top = er.bottom - wr.top + 10; // flip below when no room above
    let left = er.left - wr.left;
    left = Math.max(8, Math.min(left, wr.width - 360)); // clamp horizontally
    setPos({ top, left });
  }, [el, wrapEl, version]);

  if (!el || !pos) return null;

  const canText = elHasText(el);
  const palette = swatches.length > 0 ? swatches : FALLBACK_SWATCHES;
  const tag = el.tagName.toLowerCase();

  return (
    <div
      className="ce-toolbar"
      style={{ top: pos.top, left: pos.left }}
      // Don't let a click on the toolbar bubble to the viewport (which would commit
      // + exit CE). Also stop pointerdown so it never starts a region/empty-deselect.
      onPointerDown={(e) => e.stopPropagation()}
    >
      <span className="ce-tag">{tag}</span>
      <button
        type="button"
        className="tb"
        disabled={!canText}
        title={canText ? "Edit text (or double-click it)" : "No direct text here"}
        onClick={onEditText}
      >
        <Type size={14} />
      </button>
      <span className="sep" />
      <button
        type="button"
        className={"tb" + (mode === "text" ? " sel" : "")}
        title="Apply swatches to text color"
        onClick={() => setMode("text")}
      >
        <span style={{ fontWeight: 700, fontSize: 12.5 }}>A</span>
      </button>
      <button
        type="button"
        className={"tb" + (mode === "fill" ? " sel" : "")}
        title="Apply swatches to fill"
        onClick={() => setMode("fill")}
      >
        <span
          style={{
            width: 11,
            height: 11,
            borderRadius: 3,
            background: "currentColor",
            display: "block",
            opacity: 0.65,
          }}
        />
      </button>
      {palette.map((c) => (
        <button
          key={c.name}
          type="button"
          className="ce-sw"
          title={`${c.name} → ${mode === "fill" ? "fill" : "text"}`}
          style={{ background: c.value }}
          onClick={() => onColor(c.value, mode)}
        />
      ))}
      <span className="sep" />
      <button
        type="button"
        className="tb"
        title="Move earlier in layout"
        disabled={!el.previousElementSibling}
        onClick={() => onMove("up")}
      >
        <ArrowUp size={13} />
      </button>
      <button
        type="button"
        className="tb"
        title="Move later in layout"
        disabled={!el.nextElementSibling}
        onClick={() => onMove("down")}
      >
        <ArrowDown size={13} />
      </button>
      <button
        type="button"
        className="tb"
        title="Remove element (Del)"
        onClick={onRemove}
      >
        <Trash2 size={13} />
      </button>
      <span className="sep" />
      <button
        type="button"
        className="tb"
        title="Ask the AI to change this element"
        onClick={onAskAi}
        style={{ color: "var(--accent)" }}
      >
        <Sparkles size={14} />
      </button>
    </div>
  );
}

export default ContentToolbar;
