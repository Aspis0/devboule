/**
 * Keyboard policy for the interactive in-app terminal grid (#16).
 *
 * The grid is a real terminal: ordinary keys flow through to the PTY. The ONE
 * exception is Ctrl+C — emitting a raw ETX (\x03) straight to the child would
 * bypass the deliberate two-step SIGINT guard, so we swallow Ctrl+C and route it
 * to `onCtrlC` (the same arm/confirm path the Ctrl-C button uses). Only `keydown`
 * arms, so a held key or the matching keyup never double-fires. Cmd+C (copy:
 * metaKey, not ctrlKey) is NOT Ctrl+C and passes through untouched.
 *
 * Returns xterm's `attachCustomKeyEventHandler` signal: `true` = handle normally,
 * `false` = swallow (xterm emits nothing for this event).
 */
export function terminalKeyPolicy(
  ev: { type: string; ctrlKey: boolean; key: string },
  onCtrlC?: () => void,
): boolean {
  if (ev.ctrlKey && (ev.key === "c" || ev.key === "C")) {
    if (ev.type === "keydown") onCtrlC?.();
    return false;
  }
  return true;
}