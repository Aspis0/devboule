// The canvas tool pill. Phase A1b ships ONLY the "Move" tool — Spot Edit (the AI
// region tool) is a later phase, so it is omitted here (not rendered). The pill
// is still its own `.float-card` so the later phase can add the second button
// without restructuring the canvas chrome.

import { MousePointer2 } from "lucide-react";

export function ToolPill() {
  return (
    <div className="float-card tool-pill">
      <button className="sel" title="Move / select" type="button">
        <MousePointer2 size={14} />
        Move
      </button>
    </div>
  );
}
