// NetConsentModal — inline permission-broker card shown when a mini-coder agent
// is blocked by a missing network permission (sandbox://consent-request, kind=Net).
//
// Design constraints from the plan:
//   - Seatbelt cannot be widened mid-run. The grant activates on the NEXT spawn/retry.
//   - Copy must make this clear so the user knows to re-launch their task.
//   - detail is shown as a short hint only; never dump raw secrets.
//   - Three decisions:
//     AllowRemember → persists net_enabled=true for the project (survives restart)
//     AllowOnce     → one-shot transient grant consumed at the next spawn
//     Deny          → no-op; the next run will fail again
//
// Mirrors PlanApprovalCard.tsx for inline card style.

import { ShieldAlert } from "lucide-react";
import { stripSpoofChars } from "../agents/attentionNotifier";
import type { ConsentDecision, ConsentRequest } from "./netConsentModel";

export interface NetConsentModalProps {
  /** The pending consent request to show. */
  request: ConsentRequest;
  /** Called with the user's decision — parent handles the invoke and queue pop. */
  onDecision: (d: ConsentDecision) => void;
  /** Whether any action is currently in-flight (disables all buttons). */
  busy: boolean;
  /** Non-null when the last action errored. */
  error: string | null;
}

/**
 * Truncate the detail string to a safe display length.
 * The backend sets `detail` to a human-readable context string (e.g. the command
 * that hit the block); it can contain file paths / env-var names but never raw
 * secrets. We clip to 120 chars so the card stays compact.
 */
function safeDetail(raw: string): string {
  const sanitized = stripSpoofChars(raw.trim());
  return sanitized.length > 120 ? `${sanitized.slice(0, 117)}…` : sanitized;
}

export function NetConsentModal({
  request,
  busy,
  error,
  onDecision,
}: NetConsentModalProps) {
  // FIX 5: clip agentId to 40 chars (same sanitize+clip as detail)
  const agentId = stripSpoofChars(request.agentId).slice(0, 40);
  const detail = safeDetail(request.detail);

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="net-consent-title"
      aria-describedby="net-consent-body"
      className="flex flex-col gap-3 rounded-2xl border border-amber/30 bg-amber/[0.05] p-3"
    >
      <div className="flex items-center gap-2">
        <ShieldAlert className="h-4 w-4 shrink-0 text-amber-dark" aria-hidden />
        <h3
          id="net-consent-title"
          className="text-[12px] font-semibold text-amber-dark"
        >
          Network access blocked
        </h3>
      </div>

      <div
        id="net-consent-body"
        className="flex flex-col gap-1.5 rounded-lg border border-cream-200 bg-white p-2.5"
      >
        <p className="text-[12px] text-cream-800">
          <span className="font-semibold">{agentId}</span> tried to reach the
          network but this project has network access disabled.
        </p>
        {detail && (
          <p className="break-all rounded bg-cream-50 px-2 py-1 font-mono text-[11px] text-cream-600">
            {detail}
          </p>
        )}
        <p className="mt-0.5 text-[11px] text-cream-500">
          Your choice applies on the <span className="font-semibold">next run</span> — the
          current agent task will need to be re-launched.
        </p>
      </div>

      {error && (
        <p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] font-semibold text-coral-dark">
          {error}
        </p>
      )}

      <div className="flex flex-wrap items-center justify-end gap-2">
        <button
          type="button"
          onClick={() => onDecision("deny")}
          disabled={busy}
          className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
        >
          Deny
        </button>
        <button
          type="button"
          onClick={() => onDecision("allowOnce")}
          disabled={busy}
          className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
        >
          Allow once
        </button>
        <button
          type="button"
          onClick={() => onDecision("allowRemember")}
          disabled={busy}
          className="rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
        >
          {busy ? "Working…" : "Allow for this project"}
        </button>
      </div>
    </div>
  );
}

export default NetConsentModal;
