import { useEffect, useState } from "react";

// A live "current time" clock that re-renders the calling component on a fixed
// interval. Use it so derived values like heartbeat age and session health keep
// updating even when the underlying data poll skips a setState (a stable agent
// snapshot would otherwise freeze the displayed age/health between polls).
//
// Returns Date.now() and bumps it every `intervalMs`. The interval is cleared on
// unmount so no timer leaks. Keep the interval coarse (default 10s) since age is
// shown at second/minute granularity and a faster tick wastes renders.
//
// `enabled` (default true) lets a caller that conditionally needs the live value
// keep calling the hook UNCONDITIONALLY (Rules of Hooks) while skipping the
// interval — so a view that doesn't render any age/health (e.g. the compact
// project header) doesn't pay a 10s re-render for nothing. When false, the
// returned value is the mount-time `Date.now()` and never advances.
export function useNow(intervalMs = 10000, enabled = true): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!enabled) return;
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs, enabled]);
  return now;
}
