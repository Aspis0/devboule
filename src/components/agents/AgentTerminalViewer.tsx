// In-app terminal viewer for an app-hosted agent PTY session.
//
// Renders a read-only xterm grid (the agent's live terminal) plus a deliberate
// reply bar underneath. The non-trivial data flow — subscribe-before-snapshot
// ordering, exited handling, ctrl-c two-step, debounced resize, write-failure
// banner — lives in the headless `TerminalSession` controller (unit-tested in
// node without a DOM); this component is the thin React shell that owns the host
// element and the reply-bar UI and forwards user intents to the controller.
//
// CRITICAL CONTRACT (src-tauri/src/backend/agent_pty.rs): ConPTY stalls until the
// terminal answers its startup DSR query; `createTerminalView` pipes bytes through
// a real xterm whose automatic DSR reply flows back via `onData` ->
// `agent_pty_write`. See that module + this component's controller for details.
//
// xterm is loaded via a dynamic `import("./createTerminalView")` so its runtime
// lands in a SEPARATE lazy chunk (the Polis/PixiJS pattern) and never bloats the
// initial bundle.

import { useEffect, useRef, useState, useCallback } from "react";
import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  CornerDownLeft,
  OctagonX,
  Send,
} from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import {
  TerminalSession,
  type TerminalBanner,
  type TerminalEvent,
} from "./terminalSession";
import { replyKeyToBytes, replyTextToBytes } from "./replyKeyToBytes";

export interface AgentTerminalViewerProps {
  agentId: string;
  onExited?: () => void;
}

/** Quick-key buttons rendered in the reply bar (label + ReplyKey + optional icon). */
const QUICK_KEYS = [
  { key: "enter", label: "Enter", icon: CornerDownLeft },
  { key: "yes", label: "y" },
  { key: "no", label: "n" },
  { key: "1", label: "1" },
  { key: "2", label: "2" },
  { key: "3", label: "3" },
  { key: "4", label: "4" },
  { key: "up", label: "", icon: ArrowUp },
  { key: "down", label: "", icon: ArrowDown },
  { key: "left", label: "", icon: ArrowLeft },
  { key: "right", label: "", icon: ArrowRight },
  { key: "esc", label: "Esc" },
] as const;

export function AgentTerminalViewer({
  agentId,
  onExited,
}: AgentTerminalViewerProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const mountedRef = useRef(true);

  const [banner, setBanner] = useState<TerminalBanner>(null);
  const [ctrlCArmed, setCtrlCArmed] = useState(false);
  const [replyText, setReplyText] = useState("");

  // onExited is captured in a ref so the mount effect (which must run once per
  // agentId) does not re-fire when the parent passes a fresh callback identity.
  const onExitedRef = useRef(onExited);
  useEffect(() => {
    onExitedRef.current = onExited;
  }, [onExited]);

  useEffect(() => {
    mountedRef.current = true;
    const host = hostRef.current;
    if (!host) return;

    let cancelled = false;
    let session: TerminalSession | null = null;

    // STRICTMODE DOUBLE-MOUNT TRACE (React 18 dev runs this effect mount ->
    // cleanup -> mount). Each invocation captures its OWN `cancelled` flag and
    // local `session`; `host`/`sessionRef` are the only per-component-instance
    // state shared across the two runs. Cleanup is synchronous and always runs
    // between the two mounts, BEFORE any pending await callback of run #1 fires,
    // so run #1's IIFE sees `cancelled === true` at its next `await` checkpoint
    // and bails before constructing a session (or, if it already built one,
    // cleanup disposed it via `sessionRef`). The only truly shared external
    // resource is the PTY `listen` subscription: TerminalSession.start() rechecks
    // `disposed` after the listen() await and calls unlisten() exactly once if it
    // resolved late (see terminalSession.ts + the "dispose during in-flight
    // listen" test), so two concurrent subscriptions can never persist. A late
    // DSR reply from an orphaned view is harmless (disposed view ignores writes).
    // Net: the current per-mount + cancelled-guard + disposed-recheck design is
    // already StrictMode-safe; no module-level slot keying is needed.
    void (async () => {
      // Lazy-load the xterm chunk only when the viewer actually mounts.
      const { createTerminalView } = await import("./createTerminalView");
      if (cancelled) return;

      const { listen } = await import("@tauri-apps/api/event");
      if (cancelled) return;

      session = new TerminalSession({
        agentId,
        host,
        createView: (h, opts) => Promise.resolve(createTerminalView(h, opts)),
        invoke: invokeBackendCommand,
        listen: (channel, handler) =>
          listen<TerminalEvent["payload"]>(channel, (event) =>
            handler({ payload: event.payload }),
          ),
        onBanner: (b) => {
          if (mountedRef.current) setBanner(b);
        },
        onCtrlCArmed: (a) => {
          if (mountedRef.current) setCtrlCArmed(a);
        },
        onExited: () => {
          onExitedRef.current?.();
        },
      });
      sessionRef.current = session;
      await session.start();
    })();

    // ResizeObserver -> debounced fit + agent_pty_resize (the controller debounces).
    const ro = new ResizeObserver(() => {
      sessionRef.current?.requestResize();
    });
    ro.observe(host);

    return () => {
      cancelled = true;
      mountedRef.current = false;
      ro.disconnect();
      sessionRef.current?.dispose();
      sessionRef.current = null;
    };
  }, [agentId]);

  const sendText = useCallback(() => {
    const session = sessionRef.current;
    if (!session) return;
    void session.writeToPty(replyTextToBytes(replyText));
    setReplyText("");
  }, [replyText]);

  const sendKey = useCallback((key: (typeof QUICK_KEYS)[number]["key"]) => {
    sessionRef.current?.writeToPty(replyKeyToBytes(key));
  }, []);

  const isExited = banner?.kind === "exited";
  // An empty/whitespace reply would send a bare Enter ("\r") via the text path,
  // which is almost never intended. The dedicated Enter quick-key is the explicit
  // "accept the default" affordance, so the free-text Send/Enter is gated on real
  // content. Guards both the button (disabled) and the input's Enter keydown.
  const canSendText = !isExited && replyText.trim().length > 0;

  return (
    <div
      className="mt-3 overflow-hidden rounded-2xl border border-cream-200 bg-white"
      data-help-title="This is the agent's live in-app terminal."
      data-help-lines="The grid mirrors the agent's terminal output; typing into it is ignored.|Pasting into the grid IS sent to the agent — handy for multi-line input like a stack trace or a path.|Use the reply bar to answer prompts (Enter, y/n, numbered choices, arrows, Esc).|Ctrl-C requires two clicks to avoid an accidental interrupt.|The terminal closes when the agent process exits."
    >
      {/* The xterm host. Fixed height so the grid has dimensions to fit into. */}
      <div
        ref={hostRef}
        className="h-72 w-full"
        style={{ backgroundColor: "#1F1C19" }}
      />

      {/* Non-fatal banner (exited / error). */}
      {banner && (
        <div
          className={`px-3 py-1.5 text-[11px] font-semibold ${
            banner.kind === "exited"
              ? "bg-cream-100 text-cream-600"
              : "bg-coral/10 text-coral-dark"
          }`}
        >
          {banner.kind === "exited"
            ? "The agent terminal has exited."
            : banner.message}
        </div>
      )}

      {/* Reply bar. */}
      <div className="flex flex-col gap-2 border-t border-cream-200 bg-cream-50 p-2">
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={replyText}
            disabled={isExited}
            onChange={(e) => setReplyText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                // No-op on an empty reply: a bare Enter must go through the
                // explicit Enter quick-key, never an accidental empty submit.
                if (canSendText) sendText();
              }
            }}
            placeholder="Type a reply for the agent…"
            className="min-w-0 flex-1 rounded-md border border-cream-200 bg-white px-2 py-1 font-mono text-[12px] text-cream-800 placeholder:text-cream-400 focus:border-terracotta focus:outline-none disabled:opacity-50"
            data-help-title="Send a typed reply to the agent terminal."
            data-help-lines="The text is sent followed by Enter, as if typed at the prompt.|This is the only way to type into the otherwise read-only grid.|Use it for free-form answers the quick keys do not cover."
          />
          <button
            type="button"
            onClick={sendText}
            disabled={!canSendText}
            className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-1 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-50"
          >
            <Send className="h-3 w-3" />
            Send
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1">
          {QUICK_KEYS.map((qk) => {
            const Icon = "icon" in qk ? qk.icon : undefined;
            return (
              <button
                key={qk.key}
                type="button"
                onClick={() => sendKey(qk.key)}
                disabled={isExited}
                className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-2 py-1 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-50"
              >
                {Icon && <Icon className="h-3 w-3" />}
                {qk.label}
              </button>
            );
          })}

          {/* Ctrl-C behind a two-step confirm. */}
          <button
            type="button"
            onClick={() => sessionRef.current?.requestCtrlC()}
            disabled={isExited}
            className={`inline-flex items-center gap-1 rounded-md border px-2 py-1 text-[11px] font-semibold disabled:opacity-50 ${
              ctrlCArmed
                ? "border-coral bg-coral text-white"
                : "border-cream-200 bg-white text-cream-600 hover:text-coral"
            }`}
            data-help-title="Send Ctrl-C (SIGINT) to the agent."
            data-help-lines="Ctrl-C interrupts the agent's current command.|It is guarded: the first click arms it, the second within a few seconds sends it.|If you do not confirm, it disarms automatically."
          >
            <OctagonX className="h-3 w-3" />
            {ctrlCArmed ? "Confirm Ctrl-C" : "Ctrl-C"}
          </button>
        </div>
      </div>
    </div>
  );
}

export default AgentTerminalViewer;
