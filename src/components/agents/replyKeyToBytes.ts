// Pure key/text -> PTY byte-string mappers for the agent terminal reply bar.
//
// The in-app terminal viewer is READ-ONLY for raw keyboard input (the xterm grid
// ignores keystrokes so it cannot be used as a free shell). All deliberate input
// instead goes through an explicit reply bar: quick-key buttons and a text field.
// These helpers turn those UI intents into the exact byte sequences the agent's
// pty expects, so the mapping is unit-testable without a real terminal.
//
// The produced strings are written verbatim via `agent_pty_write` (the backend
// caps a single write at 64 KiB). All sequences here are tiny.

/** The discrete quick-reply actions the reply bar exposes as buttons. */
export type ReplyKey =
  | "enter"
  | "yes"
  | "no"
  | "up"
  | "down"
  | "left"
  | "right"
  | "esc"
  | "ctrl-c"
  | "1"
  | "2"
  | "3"
  | "4";

/**
 * Map a quick-reply action to the raw bytes to send to the pty.
 *
 * Notes on the less-obvious choices:
 *   - "yes"/"no" send the letter PLUS a carriage return so a y/n prompt is
 *     answered AND submitted in one click.
 *   - The digit choices ("1".."4") likewise append "\r" so a numbered menu
 *     selection is submitted immediately.
 *   - Arrow keys use the CSI cursor sequences. CAREFUL with the final byte:
 *     A=up, B=down, C=RIGHT, D=LEFT (C/D are the easy ones to swap).
 *   - "esc" is a bare ESC (0x1b); "ctrl-c" is ETX (0x03, the SIGINT char).
 */
export function replyKeyToBytes(key: ReplyKey): string {
  switch (key) {
    case "enter":
      return "\r";
    case "yes":
      return "y\r";
    case "no":
      return "n\r";
    case "up":
      return "\x1b[A";
    case "down":
      return "\x1b[B";
    case "left":
      return "\x1b[D";
    case "right":
      return "\x1b[C";
    case "esc":
      return "\x1b";
    case "ctrl-c":
      return "\x03";
    case "1":
      return "1\r";
    case "2":
      return "2\r";
    case "3":
      return "3\r";
    case "4":
      return "4\r";
  }
}

/**
 * Map free text from the reply input to pty bytes: the text followed by a
 * carriage return so the line is submitted. Empty text still sends a bare "\r"
 * (i.e. an empty line / "accept default"), which matches pressing Enter.
 */
export function replyTextToBytes(text: string): string {
  return text + "\r";
}
