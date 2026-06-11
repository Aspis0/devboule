// useSaveState — derive the TopBar's persistence indicator ("saved" | "dirty" |
// "writing") from DesignView's EXISTING persistence signals. It adds NO new writes
// and NO new timers; it only READS state the view already maintains:
//
//   - `writing`  : an IPC save is in flight (consolidate / generation save / node
//                  save / undo-redo save), i.e. bytes are being written right now.
//   - `dirty`    : a throttled drag-commit manifest write is PENDING (a change that
//                  hasn't hit disk yet) and nothing is currently being written.
//   - else       : "saved".
//
// Precedence: writing > dirty > saved. This mirrors the prototype's data-state on
// `.tb-status` (clean / dirty / writing) so the dot color/animation matches.

/** The three persistence states the topbar dot renders. */
export type SaveState = "saved" | "dirty" | "writing";

/** Raw signals the deriver consumes. Both are plain booleans the view already has. */
export interface SaveSignals {
  /** An IPC save is in flight (any disk write awaiting completion). */
  writing: boolean;
  /** A throttled change is queued for disk but not yet written. */
  pendingDirty: boolean;
}

/**
 * PURE derivation of the persistence state. Unit-tested. `writing` wins over
 * `pendingDirty` (we are actively flushing), `pendingDirty` over the clean default.
 */
export function deriveSaveState({ writing, pendingDirty }: SaveSignals): SaveState {
  if (writing) return "writing";
  if (pendingDirty) return "dirty";
  return "saved";
}

/**
 * Thin React hook wrapping {@link deriveSaveState}. Returns the derived `state` plus
 * a `saving` boolean (true only while `writing`) for disabling the Save button. No
 * effects, no timers — recomputed from props on each render.
 */
export function useSaveState(signals: SaveSignals): {
  state: SaveState;
  saving: boolean;
} {
  const state = deriveSaveState(signals);
  return { state, saving: state === "writing" };
}
