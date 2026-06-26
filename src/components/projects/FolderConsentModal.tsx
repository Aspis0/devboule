// FolderConsentModal — inline permission-broker card shown when a mini-coder
// agent is blocked by a missing folder-write permission (sandbox://consent-request,
// kind=FolderWrite).
//
// Design constraints (mirror NetConsentModal):
//   - Seatbelt cannot be widened mid-run. The grant activates on the NEXT spawn.
//   - Copy must make this clear so the user knows to re-launch their task.
//   - detail holds the absolute folder path; clipped to 120 chars, never raw secrets.
//   - Three decisions:
//     AllowRemember → persists the folder in working_set (survives restart)
//     AllowOnce     → one-shot transient grant consumed at the next spawn
//     Deny          → no-op; the next run will fail again
//
// Mirrors NetConsentModal for card style, aria, layout, and button ordering.

import { useEffect, useRef, useState } from "react";
import { FolderLock } from "lucide-react";
import { stripSpoofChars } from "../agents/attentionNotifier";
import type { ConsentDecision, ConsentRequest } from "./netConsentModel";

export interface FolderConsentModalProps {
  /** The pending FolderWrite consent request to show. */
  request: ConsentRequest;
  /** Called with the user's decision — parent handles the invoke and queue pop. */
  onDecision: (d: ConsentDecision) => void;
  /** Whether any action is currently in-flight (disables all buttons). */
  busy: boolean;
  /** Non-null when the last action errored. */
  error: string | null;
}

/**
 * Truncate the folder path to a safe display length.
 * `detail` from the backend is the absolute path that hit the block; it is
 * already sanitized by the backend but we clip to 120 chars so the card stays
 * compact, and run stripSpoofChars to neutralize any unicode control sequences.
 */
function safeFolder(raw: string): string {
  const sanitized = stripSpoofChars(raw.trim());
  return sanitized.length > 120 ? `${sanitized.slice(0, 117)}…` : sanitized;
}

export function FolderConsentModal({
  request,
  busy,
  error,
  onDecision,
}: FolderConsentModalProps) {
  const agentId = stripSpoofChars(request.agentId).slice(0, 40);
  const folder = safeFolder(request.detail);

  // Track which decision is currently in-flight so "Working…" appears only on
  // the clicked button (not always on "Allow & remember" regardless of click).
  // Reset when busy goes false (action settled or error cleared).
  const [activeDecision, setActiveDecision] = useState<ConsentDecision | null>(null);
  const prevBusyRef = useRef(busy);
  useEffect(() => {
    if (prevBusyRef.current && !busy) {
      // Action settled (success or error) — clear the in-flight indicator.
      setActiveDecision(null);
    }
    prevBusyRef.current = busy;
  }, [busy]);

  const handleDecision = (d: ConsentDecision) => {
    setActiveDecision(d);
    onDecision(d);
  };

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="folder-consent-title"
      aria-describedby="folder-consent-body"
      className="flex flex-col gap-3 rounded-2xl border border-amber/30 bg-amber/[0.05] p-3"
    >
      <div className="flex items-center gap-2">
        <FolderLock className="h-4 w-4 shrink-0 text-amber-dark" aria-hidden />
        <h3
          id="folder-consent-title"
          className="text-[12px] font-semibold text-amber-dark"
        >
          Folder write blocked
        </h3>
      </div>

      <div
        id="folder-consent-body"
        className="flex flex-col gap-1.5 rounded-lg border border-cream-200 bg-white p-2.5"
      >
        <p className="text-[12px] text-cream-800">
          <span className="font-semibold">{agentId}</span> tried to write to a
          folder outside this project that has not been granted write access.
        </p>
        {folder && (
          <p className="break-all rounded bg-cream-50 px-2 py-1 font-mono text-[11px] text-cream-600">
            {folder}
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
          onClick={() => handleDecision("deny")}
          disabled={busy}
          className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
        >
          {activeDecision === "deny" ? "Working…" : "Deny"}
        </button>
        <button
          type="button"
          onClick={() => handleDecision("allowOnce")}
          disabled={busy}
          className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60"
        >
          {activeDecision === "allowOnce" ? "Working…" : "Allow once"}
        </button>
        <button
          type="button"
          onClick={() => handleDecision("allowRemember")}
          disabled={busy}
          className="rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
        >
          {activeDecision === "allowRemember" ? "Working…" : "Allow & remember"}
        </button>
      </div>
    </div>
  );
}

export default FolderConsentModal;
