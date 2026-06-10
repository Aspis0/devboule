// Pure undo/redo history for the design module. NO React, no DOM, no clock —
// just two stacks (`past`, `future`) over an opaque snapshot type `S`. The DOM/
// React layer owns WHAT a snapshot is (e.g. a DesignProject) and WHEN to push;
// this module only sequences them. Mirrors the immutable, dependency-free posture
// of `engine/manifestOps.ts`: every mutating call returns a NEW history value and
// never touches the one passed in.
//
// Model: `current` (the live value) is owned by the caller, NOT stored here — so
// `undo(current)`/`redo(current)` take it as an argument. `push(s)` records the
// PREVIOUS current `s` onto `past` and clears `future` (the standard "a new edit
// invalidates the redo branch" rule). `undo(current)` pops `past` into the new
// current and pushes the given `current` onto `future`; `redo` is its mirror.

/** Max snapshots retained on the `past` stack. Oldest are evicted past the cap so
 *  a long editing session can never grow history unboundedly. */
export const MAX_HISTORY = 60;

/** Immutable history value: the undo (`past`) and redo (`future`) stacks. The tops
 *  of each stack are the most-recently-pushed entries (`at(length-1)`). */
export interface History<S> {
  readonly past: readonly S[];
  readonly future: readonly S[];
  readonly canUndo: boolean;
  readonly canRedo: boolean;
}

/** An empty history (nothing to undo or redo). */
export function createHistory<S>(): History<S> {
  return freeze([], []);
}

/** Build a frozen History from raw stacks, deriving the can* flags. Internal. */
function freeze<S>(past: readonly S[], future: readonly S[]): History<S> {
  return {
    past,
    future,
    canUndo: past.length > 0,
    canRedo: future.length > 0,
  };
}

/**
 * Record `snapshot` (the value being replaced) onto the undo stack and clear the
 * redo branch. Caps `past` at {@link MAX_HISTORY}, evicting the OLDEST entry on
 * overflow. Returns a NEW History; never mutates the input.
 */
export function push<S>(history: History<S>, snapshot: S): History<S> {
  const past = [...history.past, snapshot];
  // Evict from the front (oldest) so the cap bounds memory while keeping the most
  // recent MAX_HISTORY undo steps.
  if (past.length > MAX_HISTORY) past.splice(0, past.length - MAX_HISTORY);
  // Any new edit invalidates the redo branch.
  return freeze(past, []);
}

/**
 * Undo: pop the top of `past` and return it as the value to become the new
 * current, with a NEW history whose `future` has `current` pushed on top. Returns
 * `null` (and no history change is needed) when there is nothing to undo — the
 * caller keeps its current value untouched.
 */
export function undo<S>(
  history: History<S>,
  current: S,
): { value: S; history: History<S> } | null {
  if (history.past.length === 0) return null;
  const past = history.past.slice(0, -1);
  const value = history.past[history.past.length - 1];
  const future = [...history.future, current];
  return { value, history: freeze(past, future) };
}

/**
 * Redo: mirror of {@link undo}. Pop the top of `future` as the new current and
 * push `current` back onto `past`. Returns `null` when there is nothing to redo.
 * Redo does NOT re-cap `past` (it only restores an entry that was previously
 * within the cap), but we guard the cap anyway for symmetry/robustness.
 */
export function redo<S>(
  history: History<S>,
  current: S,
): { value: S; history: History<S> } | null {
  if (history.future.length === 0) return null;
  const future = history.future.slice(0, -1);
  const value = history.future[history.future.length - 1];
  const past = [...history.past, current];
  if (past.length > MAX_HISTORY) past.splice(0, past.length - MAX_HISTORY);
  return { value, history: freeze(past, future) };
}
