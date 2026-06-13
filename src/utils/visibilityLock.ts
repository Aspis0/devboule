/**
 * Auto-lock the app when the window stays HIDDEN for a grace period.
 *
 * The app used to lock the instant `visibilitychange` reported "hidden", which
 * fires on momentary macOS Space switches, a window briefly covered, or the
 * dev-rebuild window flash — locking the user out constantly. The grace period
 * keeps the security intent (lock when you actually leave) without the
 * false-positive lockouts: a hide that returns to visible before `graceMs`
 * cancels the pending lock.
 *
 * Pure and dependency-free so it is unit-testable with fake timers.
 *
 * @param lock     called once the window has stayed hidden for `graceMs`.
 * @param graceMs  how long the window must stay hidden before locking.
 * @returns a cleanup function (remove the listener + cancel any pending lock).
 */
export function installVisibilityLock(
  lock: () => void,
  graceMs: number,
): () => void {
  let pending: number | null = null;

  const clearPending = () => {
    if (pending !== null) {
      window.clearTimeout(pending);
      pending = null;
    }
  };

  const onVisibilityChange = () => {
    if (document.visibilityState === "hidden") {
      // Re-arm cleanly: a prior pending timer is replaced, never stacked.
      clearPending();
      pending = window.setTimeout(() => {
        pending = null;
        lock();
      }, graceMs);
    } else {
      // Back in view before the grace elapsed -> cancel the lock.
      clearPending();
    }
  };

  document.addEventListener("visibilitychange", onVisibilityChange);
  return () => {
    document.removeEventListener("visibilitychange", onVisibilityChange);
    clearPending();
  };
}