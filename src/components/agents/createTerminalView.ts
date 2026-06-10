// Lazy xterm.js bootstrap for the in-app agent terminal viewer.
//
// This module imports `@xterm/xterm` (and the fit addon + its CSS) at the MODULE
// TOP so Vite/Rollup splits the whole xterm runtime into its OWN async chunk —
// exactly the pattern used by `../polis/createPolis.ts` for PixiJS. Callers MUST
// reach this module only through a dynamic `import("./createTerminalView")` so the
// xterm bytes never land in the initial bundle.
//
// CRITICAL CONTRACT (see src-tauri/src/backend/agent_pty.rs ~spawn_agent_pty):
// on Windows, ConPTY emits a Device Status Report query (`ESC [ 6 n`) at startup
// and STALLS its render pipeline until the controlling terminal replies. xterm
// answers DSR automatically: when the queried bytes are fed in via `term.write`,
// xterm's parser CSI("n") handler calls `coreService.triggerDataEvent(ESC[r;cR)`,
// which fires `onData`. That reply path is INDEPENDENT of the keyboard handler.
// So we MUST pipe the streamed pty bytes through a real xterm instance and wire
// `term.onData` -> `agent_pty_write`, or the child produces no output.
//
// READ-ONLY POLICY: the grid must not act as a free shell. We install
// `attachCustomKeyEventHandler(() => false)`, which makes xterm's keydown/keypress
// handlers return early for EVERY key (verified against the bundled source: the
// only consult sites of the custom key handler are the keyboard handlers, and
// returning false short-circuits them before any `triggerDataEvent`). Automatic
// replies like the DSR answer do NOT go through those keyboard handlers, so they
// still reach `onData`. Net effect: user keystrokes in the grid are ignored, the
// DSR handshake still completes, and all deliberate input flows through the
// separate reply bar via `agent_pty_write`.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

/** Lines of scrollback xterm retains in memory (the backend ring keeps 256 KiB
 *  for late joiners; this is the on-screen scroll history once live). */
const SCROLLBACK = 5000;
const FONT_SIZE = 12;

/** Terminal color theme tuned to the app's dark-cream palette (see
 *  tailwind.config.js `cream`/`terracotta`). A near-black cream-800 background
 *  with a light cream foreground keeps the viewer legible inside the cream UI
 *  without a jarring pure-black box. ANSI colors map onto the app accent tokens
 *  where it reads well, falling back to sensible defaults elsewhere. */
const THEME = {
  background: "#1F1C19", // slightly darker than cream-800 for contrast
  foreground: "#F5F0EB", // cream-100
  cursor: "#C4956A", // terracotta
  cursorAccent: "#1F1C19",
  selectionBackground: "#4A4742", // cream-700
  black: "#2D2A26", // cream-800
  red: "#C47A6A", // coral
  green: "#7BAE7F", // sage
  yellow: "#D4A853", // amber
  blue: "#6A9AB5", // teal
  magenta: "#A47A52", // terracotta-500
  cyan: "#98BDD0", // teal-light
  white: "#EDE8E3", // cream-200
  brightBlack: "#6B6661", // cream-600
  brightRed: "#D9A598", // coral-light
  brightGreen: "#A5CCA8", // sage-light
  brightYellow: "#E4C88A", // amber-light
  brightBlue: "#98BDD0", // teal-light
  brightMagenta: "#C4956A", // terracotta
  brightCyan: "#B8D6E3",
  brightWhite: "#FAF8F5", // cream-50
} as const;

export interface CreateTerminalViewOptions {
  /** Fires for every byte xterm wants to send "back" to the pty. With the grid
   *  read-only, this carries ONLY xterm's automatic replies (DSR etc.), never
   *  user keystrokes. The caller forwards it to `agent_pty_write`. */
  onData: (data: string) => void;
}

export interface TerminalViewHandle {
  /** Feed raw pty output (snapshot or a live chunk) into the terminal. */
  write: (data: string) => void;
  /** Re-fit the grid to the host's current pixel size. Returns `true` when the
   *  fit succeeded (host had real dimensions), `false` when it threw (zero-size /
   *  hidden host). On `false` the geometry from `cols()`/`rows()` is stale and the
   *  caller must NOT report it to the backend. */
  fit: () => boolean;
  /** Tear down: dispose the terminal (removes its DOM, listeners, buffers). */
  dispose: () => void;
  cols: () => number;
  rows: () => number;
}

/**
 * Build an xterm terminal mounted into `host`, wired read-only with `onData`
 * carrying only xterm's automatic replies. Synchronous: xterm has no async init
 * (unlike PixiJS), so callers `await import(...)` this module then call directly.
 */
export function createTerminalView(
  host: HTMLElement,
  opts: CreateTerminalViewOptions,
): TerminalViewHandle {
  const term = new Terminal({
    // stdin stays ENABLED so xterm's parser can emit its automatic replies
    // (DSR). The read-only behaviour comes from the custom key handler below,
    // NOT from disabling stdin (which would also gag the DSR answer).
    disableStdin: false,
    scrollback: SCROLLBACK,
    fontSize: FONT_SIZE,
    fontFamily:
      'JetBrains Mono, "Fira Code", Menlo, Consolas, monospace',
    cursorBlink: false,
    convertEol: false,
    theme: { ...THEME },
  });

  // READ-ONLY: swallow every keyboard event so the grid cannot be typed into.
  // Automatic replies (DSR) bypass this path and still reach `onData`.
  term.attachCustomKeyEventHandler(() => false);

  // INTENTIONAL — PASTE IS ALLOWED INTO THE GRID.
  // attachCustomKeyEventHandler gates only xterm's KEYBOARD handlers; it does NOT
  // intercept the paste path. xterm's paste (Ctrl/Cmd+V, right-click paste, or a
  // bracketed-paste) emits its content through `onData`, which we forward to
  // `agent_pty_write`. So pasting into the grid DOES inject into the PTY. This is
  // a deliberate affordance — pasting multi-line input (a stack trace, a prompt,
  // a path) straight into the agent is genuinely useful and is the one ergonomic
  // exception to the otherwise read-only grid. It is NOT a hole:
  //   - typed keystrokes are still fully blocked (the key handler above),
  //   - the backend (src-tauri/src/backend/agent_pty.rs agent_pty_write) caps a
  //     single write at 64 KiB, so a giant clipboard cannot flood the child.
  // If a future change needs a strictly read-only grid, also override paste
  // (e.g. attach a `paste` listener on the host that preventDefault()s) — do NOT
  // assume the key handler alone covers it.

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);

  // onData carries xterm's outbound bytes. With the grid read-only this is the
  // DSR reply (and other automatic query answers) — forward it to the pty.
  const dataDisposable = term.onData(opts.onData);

  term.open(host);
  // Initial fit so the DSR reply reflects a sane geometry; guarded because a
  // zero-size host (momentarily hidden) makes fit() throw.
  try {
    fitAddon.fit();
  } catch {
    // Host not laid out yet; the ResizeObserver-driven fit() will catch up.
  }

  let disposed = false;

  return {
    write: (data: string) => {
      if (disposed) return;
      term.write(data);
    },
    fit: () => {
      if (disposed) return false;
      try {
        fitAddon.fit();
        return true;
      } catch {
        // Zero-size host; ignore — a later fit() with real dimensions wins.
        return false;
      }
    },
    dispose: () => {
      if (disposed) return;
      disposed = true;
      dataDisposable.dispose();
      term.dispose();
    },
    cols: () => term.cols,
    rows: () => term.rows,
  };
}
