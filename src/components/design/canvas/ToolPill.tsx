// The canvas tool pill: Move (select/drag) and Spot Edit (the AI region tool). The
// active tool is controlled by the parent canvas; selecting Move clears any region
// (the parent wires `onSelectMove` to do that). `Wand2` is the Spot-Edit affordance,
// matching the prototype's marquee/AI tool.

import { MousePointer2, Wand2 } from "lucide-react";

export interface ToolPillProps {
  /** The active tool. */
  tool: "move" | "ai";
  /** Switch to Move (and clear any in-progress region). */
  onSelectMove: () => void;
  /** Switch to Spot Edit (the AI region tool). */
  onSelectAi: () => void;
}

export function ToolPill({ tool, onSelectMove, onSelectAi }: ToolPillProps) {
  return (
    <div className="float-card tool-pill">
      <button
        type="button"
        className={tool === "move" ? "sel" : ""}
        title="Move / select"
        onClick={onSelectMove}
      >
        <MousePointer2 size={14} />
        Move
      </button>
      <button
        type="button"
        className={tool === "ai" ? "sel" : ""}
        title="Drag a region, then let the AI analyze and fix it"
        onClick={onSelectAi}
      >
        <Wand2 size={14} />
        Spot Edit
      </button>
    </div>
  );
}
