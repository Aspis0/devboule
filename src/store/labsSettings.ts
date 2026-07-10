// Tiny external store for a single persisted Labs preference:
// whether the experimental "Design" nav entry is visible in the Sidebar.
//
// ON by default (Design visible). The choice persists to localStorage so it
// survives reloads. Both the Sidebar (gating the nav entry) and LabsView (the
// toggle UI) subscribe through `useDesignVisible()` and re-render on change.
//
// This deliberately mirrors the localStorage-helper style used in cityStore
// (readVisibleProviders/writeVisibleProviders) — but as a minimal external
// store backed by `useSyncExternalStore` rather than a Zustand store, because
// both the Sidebar and LabsView need to react to the SAME piece of persisted
// state without pulling in the larger Polis-owned store.

import { useSyncExternalStore } from "react";

/** localStorage key for the Design nav visibility preference. */
const DESIGN_VISIBLE_KEY = "labs:designVisible";

/**
 * Read the persisted boolean, falling back to `true` (Design visible) when the
 * key is missing OR holds a value that is not the literal string "false".
 * Missing/invalid ⇒ true is the required default behaviour.
 */
function readDesignVisible(): boolean {
  try {
    const raw = window.localStorage.getItem(DESIGN_VISIBLE_KEY);
    if (raw === null) return true; // default: Design visible
    if (raw === "false") return false;
    if (raw === "true") return true;
    return true; // invalid stored value ⇒ default (true)
  } catch {
    // Private mode / quota / no window — fail open to the default.
    return true;
  }
}

/** Persist the boolean as the literal "true"/"false" string. Non-fatal. */
function writeDesignVisible(value: boolean): void {
  try {
    window.localStorage.setItem(DESIGN_VISIBLE_KEY, value ? "true" : "false");
  } catch {
    // Private mode / quota — non-fatal; in-memory value still drives this session.
  }
}

// ---- External store plumbing (module-level, framework-agnostic) ----

/** The current value, kept in memory so reads never touch localStorage. */
let designVisible: boolean = readDesignVisible();

/** The set of subscribers; notified on every change. */
const listeners = new Set<() => void>();

function emitChange(): void {
  for (const listener of listeners) {
    listener();
  }
}

/** React 18 `useSyncExternalStore` subscribe callback. Exported for tests. */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** React 18 `useSyncExternalStore` getSnapshot callback. */
function getSnapshot(): boolean {
  return designVisible;
}

/** Read the current Design-visibility preference (default `true`). */
export function getDesignVisible(): boolean {
  return designVisible;
}

/**
 * Set the Design-visibility preference: writes localStorage AND notifies all
 * subscribers (so the Sidebar and LabsView re-render together). No-op if the
 * value is unchanged (avoids needless re-renders + localStorage writes).
 */
export function setDesignVisible(value: boolean): void {
  if (designVisible === value) return;
  designVisible = value;
  writeDesignVisible(value);
  emitChange();
}

/**
 * React hook used by the Sidebar (to gate the nav entry) and by LabsView (to
 * render the toggle). Both re-render on any `setDesignVisible` change.
 */
export function useDesignVisible(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot);
}
