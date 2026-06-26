// AgentConsentModal — generic inline permission-broker card for LIVE cloud-agent
// (Claude / Codex) requests of kind "exec" (a command the agent wants to run) and
// "patch" (a file change the agent wants to make). Slice 5.
//
// Unlike NetConsentModal / FolderConsentModal (local seatbelt, grant applies on the
// NEXT run), a cloud agent is blocked RIGHT NOW on a synchronous request — the decision
// round-trips back to it immediately via respond_cloud_consent. The copy reflects this
// ("the agent is waiting"), and "Allow for session" maps to the agent's session-scoped
// accept (Codex acceptForSession). Mirrors FolderConsentModal for card style/aria/layout.

import { useEffect, useRef, useState } from "react";
import { Terminal, FilePen } from "lucide-react";
import { stripSpoofChars } from "../agents/attentionNotifier";
import type { ConsentDecision, ConsentRequest } from "./netConsentModel";

export interface AgentConsentModalProps {
  request: ConsentRequest;
  onDecision: (d: ConsentDecision) => void;
  busy: boolean;
  error: string | null;
}

function clip(raw: string | null | undefined): string {
  // Defensive: a cloud adapter (external CLI protocol) could deliver a missing/null
  // detail/path. Coalesce before trim so a malformed payload can't crash the render.
  const sanitized = stripSpoofChars((raw ?? "").trim());
  return sanitized.length > 200 ? `${sanitized.slice(0, 197)}…` : sanitized;
}

export function AgentConsentModal({ request, busy, error, onDecision }: AgentConsentModalProps) {
  const agentId = stripSpoofChars(request.agentId).slice(0, 40);
  const isExec = request.kind === "exec";
  const [activeDecision, setActiveDecision] = useState<ConsentDecision | null>(null);
  const prevBusyRef = useRef(busy);
  useEffect(() => {
    if (prevBusyRef.current && !busy) setActiveDecision(null);
    prevBusyRef.current = busy;
  }, [busy]);
  const handleDecision = (d: ConsentDecision) => { setActiveDecision(d); onDecision(d); };
  return (
    <div role="alertdialog" aria-modal="true" aria-labelledby="agent-consent-title" aria-describedby="agent-consent-body"
      className="flex flex-col gap-3 rounded-2xl border border-amber/30 bg-amber/[0.05] p-3">
      <div className="flex items-center gap-2">
        {isExec ? <Terminal className="h-4 w-4 shrink-0 text-amber-dark" aria-hidden /> : <FilePen className="h-4 w-4 shrink-0 text-amber-dark" aria-hidden />}
        <h3 id="agent-consent-title" className="text-[12px] font-semibold text-amber-dark">{isExec ? "Command execution requested" : "File change requested"}</h3>
      </div>
      <div id="agent-consent-body" className="flex flex-col gap-1.5 rounded-lg border border-cream-200 bg-white p-2.5">
        <p className="text-[12px] text-cream-800"><span className="font-semibold">{agentId}</span> wants to {isExec ? "run a command" : "make a file change"} that needs your approval.</p>
        <p className="break-all rounded bg-cream-50 px-2 py-1 font-mono text-[11px] text-cream-600">{clip(request.detail)}</p>
        {request.path && (<p className="text-[11px] text-cream-600">in {clip(request.path)}</p>)}
        <p className="mt-0.5 text-[11px] text-cream-500">The agent is <span className="font-semibold">waiting</span> for your decision.</p>
      </div>
      {error && (<p className="rounded-lg bg-coral/[0.06] px-3 py-2 text-[11px] font-semibold text-coral-dark">{error}</p>)}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <button type="button" onClick={() => handleDecision("deny")} disabled={busy} className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60">{activeDecision === "deny" ? "Working…" : "Deny"}</button>
        <button type="button" onClick={() => handleDecision("allowOnce")} disabled={busy} className="rounded-lg border border-cream-200 bg-white px-3 py-1.5 text-[12px] font-semibold text-cream-700 hover:bg-cream-50 disabled:opacity-60">{activeDecision === "allowOnce" ? "Working…" : "Allow once"}</button>
        <button type="button" onClick={() => handleDecision("allowRemember")} disabled={busy} className="rounded-lg bg-teal px-3 py-1.5 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60">{activeDecision === "allowRemember" ? "Working…" : "Allow for session"}</button>
      </div>
    </div>
  );
}

export default AgentConsentModal;
