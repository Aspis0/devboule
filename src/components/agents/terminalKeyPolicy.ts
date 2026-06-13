/**
 * Keyboard policy for the interactive in-app terminal grid (#16).
 *
 * The grid is a real terminal: ordinary keys flow through to the PTY. The ONE
 * exception is a PLAIN Ctrl+C — emitting a raw ETX (\x03) straight to the child
 * would bypass the deliberate two-step SIGINT guard, so we swallow it and route
 * it to `onCtrlC` (the same arm/confirm path the Ctrl-C button uses). Only
 * `keydown` arms, so a held key or the matching keyup never double-fires.
 *
 * Modifier discipline (max-recall): ONLY plain Ctrl+C is the SIGINT chord. We
 * exclude Ctrl+Shift+C (the Linux/Windows terminal COPY shortcut) and Ctrl+Alt /
 * AltGr+C (a character key on EU layouts, where Windows synthesizes AltGr as
 * Ctrl+Alt) so neither accidentally arms the guard. Cmd+C (copy: metaKey, not
 * ctrlKey) was never matched and still passes through untouched.
 *
 * Returns xterm's `attachCustomKeyEventHandler` signal: `true` = handle normally,
 * `false` = swallow (xterm emits nothing for this event).
 */
export function terminalKeyPolicy(
  ev: {
    type: string;
    ctrlKey: boolean;
    shiftKey?: boolean;
    altKey?: boolean;
    key: string;
  },
  onCtrlC?: () => void,
): boolean {
  if (
    ev.ctrlKey &&
    !ev.shiftKey &&
    !ev.altKey &&
    (ev.key === "c" || ev.key === "C")
  ) {
    if (ev.type === "keydown") onCtrlC?.();
    return false;
  }
  return true;
}
