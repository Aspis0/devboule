// Tiny external store tracking which Risk Flags the user has manually dismissed
// from the Header notifications dropdown.
//
// Risk flags are advisory warnings produced by the last provider sync, each with
// a stable `risk.id`. They are NOT live (unlike "Agents need you" attention
// items), so a human can choose to hide ones they've already seen or decided to
// ignore. Dismissed ids persist to localStorage and we filter by id, so a
// dismissed risk stays hidden across cloudSnapshot refreshes (it disappears on
// its own if the underlying risk resolves). There is deliberately NO auto-un-
// dismiss: dismissal is a user choice, not a reflection of live state.
//
// Pattern mirrors labsSettings.ts: a localStorage-backed external store consumed
// through `useSyncExternalStore` (no Zustand), so only the Header needs to react
// to the same piece of persisted state.

import { useSyncExternalStore } from "react";

/** localStorage key for the persisted set of dismissed risk ids. */
const DISMISSED_RISKS_KEY = "notifications:dismissedRisks";

/**
 * Upper bound on how many dismissed ids we persist. Risk flags accumulate over
 * time; without a cap the localStorage payload would grow without limit. When
 * the set would exceed this, the OLDEST ids are dropped (insertion order is
 * preserved by `Set`), so only the most-recent `MAX_DISMISSED_RISKS` survive.
 */
const MAX_DISMISSED_RISKS = 300;

/** Drop the oldest ids so the set never exceeds `MAX_DISMISSED_RISKS`. */
function enforceCap(set: Set<string>): void {
  if (set.size <= MAX_DISMISSED_RISKS) return;
  const excess = set.size - MAX_DISMISSED_RISKS;
  let removed = 0;
  for (const id of set) {
    if (removed >= excess) break;
    set.delete(id);
    removed += 1;
  }
}

/** Read the persisted set of dismissed risk ids. Malformed/missing ⇒ empty set. */
function readDismissedRisks(): Set<string> {
  try {
    const raw = window.localStorage.getItem(DISMISSED_RISKS_KEY);
    if (raw === null) return new Set<string>();
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set<string>();
    // Keep only string ids; ignore anything else so a partial/odd array is safe.
    return new Set<string>(
      parsed.filter((item): item is string => typeof item === "string"),
    );
  } catch {
    // Corrupt JSON / private mode / quota / no window — fail to empty set.
    return new Set<string>();
  }
}

/** Persist the set of dismissed risk ids as a JSON array. Non-fatal. */
function writeDismissedRisks(value: Set<string>): void {
  try {
    window.localStorage.setItem(
      DISMISSED_RISKS_KEY,
      JSON.stringify(Array.from(value)),
    );
  } catch {
    // Private mode / quota — non-fatal; in-memory value still drives this session.
  }
}

// ---- External store plumbing (module-level, framework-agnostic) ----

/** The current value, kept in memory so reads never touch localStorage. */
let dismissedRisks: Set<string> = readDismissedRisks();

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
function getSnapshot(): ReadonlySet<string> {
  return dismissedRisks;
}

/** Read the current set of dismissed risk ids. */
export function getDismissedRisks(): ReadonlySet<string> {
  return dismissedRisks;
}

/**
 * Dismiss a single risk by id: adds it to the set, persists, and notifies
 * subscribers. No-op if the id is already dismissed (avoids needless re-renders
 * + localStorage writes).
 */
export function dismissRisk(id: string): void {
  if (dismissedRisks.has(id)) return;
  const next = new Set(dismissedRisks);
  next.add(id);
  enforceCap(next);
  dismissedRisks = next;
  writeDismissedRisks(dismissedRisks);
  emitChange();
}

/**
 * Dismiss many risks at once: adds all ids to the set, persists once, and
 * notifies subscribers a single time. No-op (no notify) if none of the ids are
 * new.
 */
export function clearRisks(ids: string[]): void {
  let changed = false;
  const next = new Set(dismissedRisks);
  for (const id of ids) {
    if (!next.has(id)) {
      next.add(id);
      changed = true;
    }
  }
  if (!changed) return;
  enforceCap(next);
  dismissedRisks = next;
  writeDismissedRisks(dismissedRisks);
  emitChange();
}

/**
 * React hook used by the Header notifications dropdown. Re-renders whenever the
 * dismissed set changes so the visible Risk Flags and bell badge update.
 */
export function useDismissedRisks(): ReadonlySet<string> {
  return useSyncExternalStore(subscribe, getSnapshot);
}
