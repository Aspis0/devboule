import { useEffect, useState } from "react";

// A live "current time" clock that re-renders the calling component on a fixed
// interval. Use it so derived values like heartbeat age and session health keep
// updating even when the underlying data poll skips a setState (a stable agent
// snapshot would otherwise freeze the displayed age/health between polls).
//
// Returns Date.now() and bumps it every `intervalMs`. The interval is cleared on
// unmount so no timer leaks. Keep the interval coarse (default 10s) since age is
// shown at second/minute granularity and a faster tick wastes renders.
export function useNow(intervalMs = 10000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}
