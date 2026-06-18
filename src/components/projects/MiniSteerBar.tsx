// MiniSteerBar — human steering of a RUNNING mini from the Agent Console (UX piece 3,
// Part A). A small input bar rendered directly under <AgentConsole> in the console dock
// tab. It REUSES the existing `mini_coder_steer` Tauri command:
//
//   - "Send"  -> mini_coder_steer({ agentId, message })       — queue a correction.
//   - "Stop"  -> mini_coder_steer({ agentId, message: "stop" }) — the steer→kill path.
//     ("stop" already drives the live PTY to EOF; we do NOT also call mini_coder_kill.)
//
// Stopping is mini/agent-scoped, so this control lives in the Console (keyed by the
// selected agentId) rather than on a plan task row — a task row can't stop a mini it
// doesn't own. NOTE this is a deliberate deviation from the mockup, which is correct.
//
// CSP-strict: no dangerouslySetInnerHTML, no inline HTML on*= handlers, no eval. React
// onClick/onChange handlers are fine. The component owns only its input + transient
// status state; the IPC is fire-and-await.

import { useCallback, useEffect, useRef, useState } from "react";

import { invokeBackendCommand } from "../../context/AppContext";

// Mirror the backend cap (mini_coder::MAX_STEER_MESSAGE_LEN) so an over-long message is
// trimmed client-side before the round trip (the backend re-sanitizes + re-caps anyway).
export const MAX_STEER_MESSAGE_LEN = 2000;

// The shape mini_coder_steer returns. `queued` is present for queued/queue_full.
interface SteerResult {
  status: "queued" | "stopped" | "queue_full" | "noop";
  queued?: number;
}

/** Map a steer result to a short, human-readable status line for the bar. */
export function steerStatusLabel(result: SteerResult): string {
  switch (result.status) {
    case "queued":
      return "Steer queued.";
    case "stopped":
      return "Stop sent — mini halting.";
    case "queue_full":
      return "Steer queue full — try again shortly.";
    case "noop":
      return "No running mini to steer.";
    default:
      return "Done.";
  }
}

export interface MiniSteerBarProps {
  /** The selected agent id (the running mini). Null disables the whole bar. */
  agentId: string | null;
  /** Force the whole bar disabled regardless of selection — used to lock steering
   *  on a read-only (archived) project. Steering is a mutation. Defaults to false. */
  disabled?: boolean;
}

export function MiniSteerBar({ agentId, disabled: forcedDisabled = false }: MiniSteerBarProps) {
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Guards a transient status timeout so we never set state after the timeout would
  // race a new send (and so a clearStatus can cancel a stale clear).
  const statusTimer = useRef<number | null>(null);
  // Guards all setState calls from landing on an unmounted component.
  const mountedRef = useRef(true);

  // On unmount: mark unmounted + cancel any pending status-clear timeout.
  useEffect(
    () => () => {
      mountedRef.current = false;
      if (statusTimer.current !== null) window.clearTimeout(statusTimer.current);
    },
    [],
  );

  const flashStatus = useCallback((text: string) => {
    if (!mountedRef.current) return;
    setStatus(text);
    if (statusTimer.current !== null) window.clearTimeout(statusTimer.current);
    // Only schedule the clear-timer if still mounted; the timer itself re-checks too.
    statusTimer.current = window.setTimeout(() => {
      if (mountedRef.current) {
        setStatus(null);
      }
      statusTimer.current = null;
    }, 4000);
  }, []);

  // Single steer path for BOTH Send (the typed message) and Stop (message:"stop").
  const steer = useCallback(
    async (rawMessage: string, clearInputOnSuccess: boolean) => {
      if (agentId === null || busy || forcedDisabled) return;
      const text = rawMessage.trim().slice(0, MAX_STEER_MESSAGE_LEN);
      if (text.length === 0) return;
      setBusy(true);
      setError(null);
      try {
        const result = await invokeBackendCommand<SteerResult>("mini_coder_steer", {
          agentId,
          message: text,
        });
        // Guard: the component may have unmounted while the IPC was in flight.
        if (!mountedRef.current) return;
        flashStatus(steerStatusLabel(result));
        // Clear the input only when the human's typed correction was accepted.
        if (clearInputOnSuccess && result.status !== "queue_full") {
          setMessage("");
        }
      } catch (e) {
        if (mountedRef.current) {
          setError(e instanceof Error ? e.message : "Steer failed.");
        }
      } finally {
        if (mountedRef.current) setBusy(false);
      }
    },
    [agentId, busy, forcedDisabled, flashStatus],
  );

  const disabled = agentId === null || busy || forcedDisabled;
  const sendDisabled = disabled || message.trim().length === 0;

  return (
    <div className="mt-2 flex flex-col gap-1 border-t border-cream-100 pt-2">
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={message}
          onChange={(e) => {
            setMessage(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void steer(message, true);
            }
          }}
          placeholder={
            agentId === null
              ? "Select a running mini to steer…"
              : "Steer the mini… (↵ to send)"
          }
          maxLength={MAX_STEER_MESSAGE_LEN}
          disabled={disabled}
          aria-label="Steer message"
          className="min-w-0 flex-1 rounded-lg border border-cream-200 bg-white px-2.5 py-1.5 text-[12px] text-cream-800 placeholder:text-cream-400 focus:outline-none focus:ring-1 focus:ring-teal/40 disabled:opacity-60"
        />
        <button
          type="button"
          onClick={() => void steer(message, true)}
          disabled={sendDisabled}
          className="shrink-0 rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white transition-colors hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Send
        </button>
        <button
          type="button"
          onClick={() => void steer("stop", false)}
          disabled={disabled}
          aria-label="Stop the running mini"
          className="shrink-0 rounded-lg bg-terracotta px-3 py-1.5 text-[12px] font-semibold text-white transition-colors hover:bg-terracotta/90 disabled:cursor-not-allowed disabled:opacity-50"
        >
          Stop
        </button>
      </div>
      {error !== null ? (
        <p className="text-[10px] font-semibold text-coral-dark">{error}</p>
      ) : status !== null ? (
        <p className="text-[10px] text-cream-500">{status}</p>
      ) : null}
    </div>
  );
}

export default MiniSteerBar;
