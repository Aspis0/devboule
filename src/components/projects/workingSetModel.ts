// workingSetModel — pure model helpers for WorkingSetCard (Slice 2 of the
// permission broker).
//
// Kept in a separate, vitest-friendly module following the same split used by
// sandboxModeModel.ts and netConsentModel.ts.

// ─────────────────────────────────────────────────────────────
// Pure helpers
// ─────────────────────────────────────────────────────────────

/**
 * Pure guard for the prop-sync useEffect in WorkingSetCard.
 *
 * After a successful add/remove the backend returns the CANONICAL folder list
 * and we adopt it as `lastWritten`. A background poll whose `loadProject` read
 * disk BEFORE that write committed can arrive afterward carrying the old
 * working-set — if the prop-sync effect adopted it unconditionally the display
 * would snap back. This guard prevents that.
 *
 * Rules (applied in order):
 * 1. When a write is in-flight (`busy`), never clobber the optimistic value.
 * 2. When a confirmed canonical list is pending (`lastWritten` set), only
 *    adopt `incoming` once it is SET-EQUAL to `lastWritten` (same length +
 *    same members, order-independent). A stale poll that does not yet reflect
 *    our write must be ignored. Once the prop catches up, return true (adopt)
 *    so the caller can also clear `lastWritten`.
 * 3. With no pending write and not busy, always adopt the prop (normal
 *    external refresh, e.g. the 10s poll or an "Allow & remember" reload).
 *
 * Because both `lastWritten` and `incoming` are backend-canonical paths, the
 * set-equality comparison matches correctly — no macOS /tmp → /private/tmp
 * mismatch (that was the old `pendingFoldersRef` string-compare freeze).
 *
 * @param incoming     The new working-set prop from the parent.
 * @param lastWritten  The list returned by the last successful add/remove IPC,
 *                     or `null` if no write is pending parent acknowledgement.
 * @param busy         Whether an IPC call is currently in-flight.
 */
export function shouldAdoptWorkingSet(
  incoming: string[],
  lastWritten: string[] | null,
  busy: boolean,
): boolean {
  if (busy) return false;
  if (lastWritten !== null) {
    // Adopt only when the prop has caught up to our confirmed canonical list.
    if (incoming.length !== lastWritten.length) return false;
    const set = new Set(lastWritten);
    return incoming.every((f) => set.has(f));
  }
  return true;
}
