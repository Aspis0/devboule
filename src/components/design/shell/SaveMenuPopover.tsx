// SaveMenuPopover — the split-button's extra menu (prototype's SaveMenuPopover,
// `.save-pop`). Row 1 re-runs the consolidate save; row 2 ("Save & hand off") is the
// Phase-D agent dispatch — now ENABLED (opens HandoffModal). The NEW badge mirrors the
// prototype.

import { Save, Cpu } from "lucide-react";
import { Popover } from "./Popover";

export interface SaveMenuPopoverProps {
  open: boolean;
  onClose: () => void;
  /** No project open / a save in flight disables the consolidate AND hand-off rows. */
  disabled: boolean;
  onSave: () => void;
  /** Open the Phase-D hand-off modal. */
  onHandoff: () => void;
}

export function SaveMenuPopover({
  open,
  onClose,
  disabled,
  onSave,
  onHandoff,
}: SaveMenuPopoverProps) {
  return (
    <Popover open={open} onClose={onClose} className="right save-pop">
      <div className="pop-head">DELIVER</div>
      <button
        type="button"
        className="pop-row"
        disabled={disabled}
        onClick={() => {
          onClose();
          onSave();
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
          <Save size={16} />
        </span>
        <div>
          <b>Save to repo</b>
          <span>Write manifest + components to the working folder</span>
        </div>
      </button>
      <div className="pop-sep" />
      <button
        type="button"
        className="pop-row agents"
        disabled={disabled}
        title="Package the design and dispatch a coding agent"
        aria-label="Save and hand off to coding agents"
        onClick={() => {
          onClose();
          onHandoff();
        }}
      >
        <span className="agents-ic">
          <Cpu size={16} />
        </span>
        <div>
          <b>Save &amp; hand off</b>
          <span>Dispatch to coding agents</span>
        </div>
        <span className="new-badge">NEW</span>
      </button>
    </Popover>
  );
}

export default SaveMenuPopover;
