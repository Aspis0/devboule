// The floating zoom pill: −  pct(reset to 100%)  +  Fit. The pct button label
// shows the current zoom; clicking it resets to 100%. "Fit" asks the parent to
// frame all nodes (it owns the viewport size + node bounds). Pure presentational —
// all state lives in the canvas.

import { Minus, Plus, Maximize } from "lucide-react";

interface ZoomPillProps {
  zoom: number;
  /** Multiply/clamp the zoom by a delta around the viewport center. */
  onZoomBy: (delta: number) => void;
  /** Reset zoom to 1.0 (100%). */
  onReset: () => void;
  /** Fit all nodes into the viewport. */
  onFit: () => void;
}

export function ZoomPill({ zoom, onZoomBy, onReset, onFit }: ZoomPillProps) {
  return (
    <div className="float-card zoom-pill">
      <button type="button" onClick={() => onZoomBy(-0.1)} title="Zoom out">
        <Minus size={14} />
      </button>
      <button
        type="button"
        className="zoom-pct"
        onClick={onReset}
        title="Reset to 100%"
      >
        {Math.round(zoom * 100)}%
      </button>
      <button type="button" onClick={() => onZoomBy(0.1)} title="Zoom in">
        <Plus size={14} />
      </button>
      <button type="button" onClick={onFit} title="Fit">
        <Maximize size={14} />
      </button>
    </div>
  );
}
