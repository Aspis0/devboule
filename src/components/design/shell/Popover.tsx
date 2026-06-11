// Popover — generic anchored popover for the design shell chrome. Mirrors the
// prototype's `Popover` (shell.jsx): a full-screen `.overlay-catch` underlay that
// closes on click, plus the anchored `.pop` panel (right/left variant). It mounts
// ONLY when `open` (no hidden DOM when closed) and closes on Escape.
//
// The panel must be rendered INSIDE a `.pop-wrap` (position:relative) anchor so the
// absolute `.pop` positions against the trigger — callers wrap the trigger button +
// <Popover> in a `.pop-wrap` div, exactly like the prototype.

import { useEffect, type ReactNode } from "react";

export interface PopoverProps {
  open: boolean;
  onClose: () => void;
  /** Anchor side: "right" (default) aligns the panel's right edge to the trigger;
   *  "left" aligns its left edge. Extra class names (e.g. "oracle-pop save-pop")
   *  may be appended for per-popover sizing. */
  className?: string;
  children: ReactNode;
}

export function Popover({ open, onClose, className, children }: PopoverProps) {
  // Escape closes — only while open (listener added/removed with the panel). Reads
  // the latest `onClose` via the dependency so a re-rendered handler is honored.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <>
      <div className="overlay-catch" onClick={onClose} data-testid="overlay-catch" />
      <div className={"pop " + (className || "right")} role="menu">
        {children}
      </div>
    </>
  );
}

export default Popover;
