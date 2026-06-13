// Headless data-flow controller for the in-app agent terminal viewer.
//
// The React component (`AgentTerminalViewer.tsx`) is a thin shell around this
// controller: it owns the host <div> and the reply-bar UI, but ALL of the
// non-trivial sequencing lives here so it can be unit-tested in the node test
// environment WITHOUT a DOM (the repo's vitest runs `environment: "node"`, no
// jsdom). Everything external (the terminal view, `invoke`, `listen`, timers) is
// injected, so the controller is deterministic under fake timers and mocks.
//
// What it sequences (the parts that are easy to get subtly wrong):
//   - NO-DATA-LOSS startup ordering: subscribe to the event channel FIRST and
//     queue incoming chunks, THEN fetch the snapshot, write the snapshot, then
//     flush the queued chunks. (Snapshot-after-subscribe means a chunk that
//     arrives during the snapshot fetch is not dropped. A tiny overlap between
//     the tail of the snapshot and the first queued chunk is possible and
//     ACCEPTED — deduping mid-stream terminal bytes is not worth the risk of
//     corrupting an escape sequence; xterm renders the small overlap harmlessly.)
//   - `exited` sentinel -> terminal closed state + onExited callback (once).
//   - Ctrl-C TWO-STEP arm/disarm: first request arms, second within the window
//     actually sends ETX, and the arm auto-disarms after a timeout.
//   - Resize DEBOUNCE: fit + agent_pty_resize coalesced so a drag doesn't spam
//     the backend.
//   - write error backoff: repeated `agent_pty_write` failures surface a banner
//     (so a dead session does not silently swallow input forever).

import type { TerminalViewHandle } from "./createTerminalView";

/** Matches the Rust `TerminalEvent` payload (camelCase). Either a `data` chunk
 *  or the `exited: true` sentinel; both fields optional on the wire. */
export interface TerminalEventPayload {
  data?: string;
  exited?: boolean;
}

/** The terminal-event envelope as delivered by Tauri's `listen`. */
export interface TerminalEvent {
  payload: TerminalEventPayload;
}

export type UnlistenFn = () => void;

/** Banner shown over/under the grid for a non-fatal condition. `null` = none. */
export type TerminalBanner =
  | { kind: "exited" }
  | { kind: "error"; message: string }
  | null;

/** Everything the controller needs from the outside, injected for testability. */
export interface TerminalSessionDeps {
  agentId: string;
  /** Create the xterm view bound to a host element. */
  createView: (
    host: HTMLElement,
    opts: { onData: (data: string) => void; onCtrlC: () => void },
  ) => Promise<TerminalViewHandle>;
  /** The host element the view mounts into. */
  host: HTMLElement;
  /** `invoke`-style backend call. */
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  /** Subscribe to the per-agent terminal event channel. */
  listen: (
    channel: string,
    handler: (event: TerminalEvent) => void,
  ) => Promise<UnlistenFn>;
  /** Surface a banner state change to the React layer. */
  onBanner: (banner: TerminalBanner) => void;
  /** Surface the ctrl-c armed state to the React layer. */
  onCtrlCArmed: (armed: boolean) => void;
  /** Called once when the session reports it has exited. */
  onExited?: () => void;
  /** Injectable timers (default to window timers in production). */
  setTimeout?: (fn: () => void, ms: number) => number;
  clearTimeout?: (id: number) => void;
}

/** Debounce window for resize -> agent_pty_resize (ms). */
const RESIZE_DEBOUNCE_MS = 150;
/** How long a ctrl-c stays armed before auto-disarming (ms). */
const CTRL_C_ARM_MS = 3000;
/** Consecutive write failures before the error banner is shown. */
const WRITE_FAIL_THRESHOLD = 2;

export function terminalEventChannel(agentId: string): string {
  return `agent-terminal://${agentId}`;
}

/**
 * The controller. Construct, call `start()`, drive UI intents via its methods,
 * and call `dispose()` on unmount. All methods are safe to call after dispose
 * (they no-op), so a late async callback can never act on a torn-down session.
 */
export class TerminalSession {
  private readonly deps: Required<
    Pick<TerminalSessionDeps, "setTimeout" | "clearTimeout">
  > &
    TerminalSessionDeps;

  private view: TerminalViewHandle | null = null;
  private unlisten: UnlistenFn | null = null;
  private disposed = false;
  private exited = false;

  /** Chunks that arrived after we subscribed but before the snapshot was
   *  written. Replayed (flushed) once the snapshot is on screen. */
  private pendingChunks: string[] = [];
  private snapshotDone = false;

  private ctrlCArmed = false;
  private ctrlCTimer: number | null = null;

  private resizeTimer: number | null = null;
  private writeFailCount = 0;

  constructor(deps: TerminalSessionDeps) {
    this.deps = {
      ...deps,
      setTimeout:
        deps.setTimeout ??
        ((fn, ms) => window.setTimeout(fn, ms) as unknown as number),
      clearTimeout: deps.clearTimeout ?? ((id) => window.clearTimeout(id)),
    };
  }

  /**
   * Wire everything up: build the view, subscribe FIRST (queuing chunks), then
   * fetch + write the snapshot, then flush the queue. Resolves once startup is
   * done; rejects only if the view itself cannot be created (a snapshot failure
   * is non-fatal — it just shows an error banner and goes live).
   */
  async start(): Promise<void> {
    if (this.disposed) return;

    // 1) Build the xterm view first so we have somewhere to write.
    let view: TerminalViewHandle;
    try {
      view = await this.deps.createView(this.deps.host, {
        onData: (data) => this.handleViewData(data),
        // #16: Ctrl+C typed in the grid arms/confirms the two-step SIGINT guard
        // (createTerminalView swallows the raw key) — never a raw ETX bypass.
        onCtrlC: () => this.requestCtrlC(),
      });
    } catch {
      if (!this.disposed) {
        this.deps.onBanner({
          kind: "error",
          message: "Could not open the terminal view.",
        });
      }
      return;
    }
    if (this.disposed) {
      // Unmounted while createView was awaiting: dispose the orphan view.
      view.dispose();
      return;
    }
    this.view = view;

    // 2) Subscribe BEFORE the snapshot so no live chunk is lost. Chunks that
    //    arrive before the snapshot is written are queued and flushed after.
    const channel = terminalEventChannel(this.deps.agentId);
    let unlisten: UnlistenFn;
    try {
      unlisten = await this.deps.listen(channel, (event) =>
        this.handleEvent(event),
      );
    } catch {
      // Without the live channel the viewer is snapshot-only; surface it but
      // still show the snapshot below.
      if (!this.disposed) {
        this.deps.onBanner({
          kind: "error",
          message: "Live terminal updates are unavailable.",
        });
      }
      unlisten = () => {};
    }
    if (this.disposed) {
      unlisten();
      return;
    }
    this.unlisten = unlisten;

    // 3) Fetch + write the snapshot (best-effort), then flush queued chunks.
    let snapshot = "";
    try {
      snapshot = await this.deps.invoke<string>("agent_pty_snapshot", {
        agentId: this.deps.agentId,
      });
    } catch {
      if (!this.disposed) {
        this.deps.onBanner({
          kind: "error",
          message: "No app terminal for this agent.",
        });
      }
      // No snapshot, but we may still receive live data — flush whatever queued.
    }
    if (this.disposed) return;

    if (snapshot && this.view) {
      this.view.write(snapshot);
    }
    this.flushPending();
    this.snapshotDone = true;

    // Report the viewer's ACTUAL geometry to the backend once at startup. The
    // ResizeObserver only fires on subsequent size CHANGES, so without this the
    // pty would stay at its initial 120x32 if the host never resizes again.
    this.doResize();
  }

  /** Flush queued live chunks into the view, in arrival order. */
  private flushPending(): void {
    if (!this.view) return;
    for (const chunk of this.pendingChunks) {
      this.view.write(chunk);
    }
    this.pendingChunks = [];
  }

  /** Handle a terminal event: queue/write a data chunk or mark exited. */
  private handleEvent(event: TerminalEvent): void {
    if (this.disposed) return;
    const payload = event.payload;
    if (payload.exited === true) {
      this.markExited();
      return;
    }
    if (typeof payload.data === "string") {
      if (this.snapshotDone && this.view) {
        // Snapshot already written: go straight to the view.
        this.view.write(payload.data);
      } else {
        // Still fetching/writing the snapshot: queue to preserve order.
        this.pendingChunks.push(payload.data);
      }
    }
  }

  /** Mark the session exited exactly once: banner + onExited callback. */
  private markExited(): void {
    if (this.exited) return;
    this.exited = true;
    this.deps.onBanner({ kind: "exited" });
    this.deps.onExited?.();
  }

  /** xterm wants to send bytes back: the user's keystrokes/paste (#16 interactive
   *  grid) and automatic replies (DSR). Forward to the pty. A hard, repeated write
   *  failure still surfaces the banner; a single transient failure is swallowed. */
  private handleViewData(data: string): void {
    void this.writeToPty(data);
  }

  /** Send raw bytes to the pty. Swallows a single error; surfaces a banner once
   *  failures cross the threshold (a dead session must not silently eat input).
   *  Resets the failure counter on the first success. */
  async writeToPty(data: string): Promise<void> {
    if (this.disposed || this.exited) return;
    try {
      await this.deps.invoke<void>("agent_pty_write", {
        agentId: this.deps.agentId,
        data,
      });
      this.writeFailCount = 0;
    } catch {
      if (this.disposed) return;
      this.writeFailCount += 1;
      if (this.writeFailCount >= WRITE_FAIL_THRESHOLD) {
        this.deps.onBanner({
          kind: "error",
          message: "Could not send input to the agent terminal.",
        });
      }
    }
  }

  /**
   * Ctrl-C two-step: the first call ARMS (and starts the auto-disarm timer); a
   * second call while armed actually SENDS ETX and disarms. This guards against
   * an accidental SIGINT to a long-running agent.
   */
  requestCtrlC(): void {
    if (this.disposed || this.exited) return;
    if (this.ctrlCArmed) {
      // Confirmed: send and disarm.
      this.disarmCtrlC();
      void this.writeToPty("\x03");
      return;
    }
    // Arm and schedule auto-disarm.
    this.ctrlCArmed = true;
    this.deps.onCtrlCArmed(true);
    this.ctrlCTimer = this.deps.setTimeout(() => {
      this.ctrlCTimer = null;
      this.disarmCtrlC();
    }, CTRL_C_ARM_MS);
  }

  private disarmCtrlC(): void {
    if (this.ctrlCTimer !== null) {
      this.deps.clearTimeout(this.ctrlCTimer);
      this.ctrlCTimer = null;
    }
    if (this.ctrlCArmed) {
      this.ctrlCArmed = false;
      this.deps.onCtrlCArmed(false);
    }
  }

  /**
   * Debounced resize: coalesce a burst of host-size changes into one fit() +
   * agent_pty_resize so a drag doesn't flood the backend. The fit happens at the
   * trailing edge so the grid geometry matches the final size we report.
   */
  requestResize(): void {
    if (this.disposed) return;
    if (this.resizeTimer !== null) {
      this.deps.clearTimeout(this.resizeTimer);
    }
    this.resizeTimer = this.deps.setTimeout(() => {
      this.resizeTimer = null;
      this.doResize();
    }, RESIZE_DEBOUNCE_MS);
  }

  private doResize(): void {
    if (this.disposed || this.exited || !this.view) return;
    // Skip the backend resize when fit() failed (zero-size / hidden host) or the
    // grid reports a degenerate size: otherwise we'd tell the pty a stale/default
    // geometry (e.g. 80x24) the grid never actually fitted to, leaving the child
    // wrapping at the wrong width once the viewer becomes visible again. The
    // ResizeObserver fires another requestResize() when real dimensions arrive.
    const fitted = this.view.fit();
    const cols = this.view.cols();
    const rows = this.view.rows();
    if (!fitted || cols <= 0 || rows <= 0) return;
    // Resize is fire-and-forget; a failure here is non-fatal (a momentarily
    // hidden viewer can report a degenerate size the backend clamps/rejects).
    void this.deps
      .invoke<void>("agent_pty_resize", {
        agentId: this.deps.agentId,
        cols,
        rows,
      })
      .catch(() => {
        /* non-fatal */
      });
  }

  /** Tear down: unlisten, dispose the view, clear timers. Idempotent. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.ctrlCTimer !== null) {
      this.deps.clearTimeout(this.ctrlCTimer);
      this.ctrlCTimer = null;
    }
    if (this.resizeTimer !== null) {
      this.deps.clearTimeout(this.resizeTimer);
      this.resizeTimer = null;
    }
    if (this.unlisten) {
      try {
        this.unlisten();
      } catch {
        /* already gone */
      }
      this.unlisten = null;
    }
    if (this.view) {
      this.view.dispose();
      this.view = null;
    }
  }
}
