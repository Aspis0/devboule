// Inline per-row detail for an agent: its claims (active / waiting / history),
// its recent MCP events, and its self-reported subagent breakdown. This is where
// the old global Claim-History and Event-Feed CONTENT moved — scoped to one agent
// so the page stops being a flat dump of every claim/event. Pure derivation lives
// in agentRowModel.drawerData; this is the thin JSX shell.

import { Activity, GitPullRequest, Users } from "lucide-react";
import type {
  AgentClaim,
  AgentEvent,
  AgentSession,
} from "../../types/backend";
import { drawerData, formatStamp, subagentChipLabel } from "./agentRowModel";

const statusTone: Record<string, string> = {
  wip: "bg-teal/10 text-teal",
  review: "bg-sage/10 text-sage-dark",
  blocked: "bg-coral/10 text-coral-dark",
  done: "bg-sage/10 text-sage-dark",
  claimed: "bg-terracotta/10 text-terracotta",
};

function ClaimLine({ claim }: { claim: AgentClaim }) {
  return (
    <div className="rounded-md bg-cream-50 px-2 py-1">
      <div className="flex items-start justify-between gap-2">
        <p className="min-w-0 break-words text-[11px] font-semibold text-cream-800">
          {claim.taskTitle ?? claim.taskId}
        </p>
        <span
          className={`shrink-0 rounded-md px-1.5 py-0.5 text-[9px] font-semibold ${
            statusTone[claim.status] ?? "bg-white text-cream-500"
          }`}
        >
          {claim.status}
        </span>
      </div>
      <p className="truncate text-[9px] text-cream-400">
        {claim.projectTitle ?? claim.projectId} · {formatStamp(claim.updatedAt)}
      </p>
    </div>
  );
}

export function AgentDetailDrawer({
  session,
  claims,
  events,
  now,
}: {
  session: AgentSession;
  claims: AgentClaim[];
  events: AgentEvent[];
  now: number;
}) {
  const data = drawerData(session, claims, events, now);
  const openClaims = [...data.activeClaims, ...data.waitingClaims];

  return (
    <div className="mt-3 grid grid-cols-1 gap-3 border-t border-cream-200 pt-3 md:grid-cols-3">
      {/* Subagent breakdown. */}
      <div
        data-help-title="Subagents this agent reported over MCP."
        data-help-lines="Subagents are the fan-out an orchestrator/coder self-reports (advisory).|Each line is a label, model, and headcount.|The numbers also feed the fleet summary at the top of the page.|Empty means the agent reported no subagents."
      >
        <div className="mb-1.5 flex items-center gap-1.5">
          <Users className="h-3.5 w-3.5 text-terracotta" />
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            Subagents
          </p>
        </div>
        {data.subagents.length === 0 ? (
          <p className="text-[10px] text-cream-400">None reported.</p>
        ) : (
          <ul className="space-y-1">
            {data.subagents.map((sub, i) => (
              <li
                key={`${sub.label}-${i}`}
                className="rounded-md bg-cream-50 px-2 py-1 text-[10px] text-cream-600"
              >
                <span className="font-semibold text-cream-800">{sub.label}</span>{" "}
                <span className="text-cream-400">{subagentChipLabel(sub)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* This agent's claims. */}
      <div
        data-help-title="Claims this agent holds or held."
        data-help-lines="Open claims show what this agent currently owns or is waiting on.|History shows finished/expired ownership records for this agent.|Claims prevent two agents editing or verifying the same thing blindly.|Inspect evidence before closing important work just because a claim exists."
      >
        <div className="mb-1.5 flex items-center gap-1.5">
          <GitPullRequest className="h-3.5 w-3.5 text-teal" />
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            Claims
          </p>
        </div>
        {openClaims.length === 0 && data.historyClaims.length === 0 ? (
          <p className="text-[10px] text-cream-400">No claims for this agent.</p>
        ) : (
          <div className="space-y-1">
            {openClaims.map((claim) => (
              <ClaimLine
                key={`open-${claim.projectId}:${claim.taskId}:${claim.status}:${claim.updatedAt ?? claim.claimedAt}`}
                claim={claim}
              />
            ))}
            {data.historyClaims.slice(0, 4).map((claim) => (
              <div
                key={`hist-${claim.projectId}:${claim.taskId}:${claim.status}:${claim.updatedAt ?? claim.claimedAt}`}
                className="opacity-70"
              >
                <ClaimLine claim={claim} />
              </div>
            ))}
          </div>
        )}
      </div>

      {/* This agent's recent events. */}
      <div
        data-help-title="Recent MCP events from this agent."
        data-help-lines="Events are status messages this agent emitted over MCP.|They explain why the Kanban moved, why a task is blocked, or what evidence was attached.|This is also where MCP errors surface per agent.|Use it with the terminal output when debugging agent behavior."
      >
        <div className="mb-1.5 flex items-center gap-1.5">
          <Activity className="h-3.5 w-3.5 text-terracotta" />
          <p className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
            Recent events
          </p>
        </div>
        {data.events.length === 0 ? (
          <p className="text-[10px] text-cream-400">No recent events.</p>
        ) : (
          <ul className="space-y-1">
            {data.events.map((event) => (
              <li
                key={event.id}
                className="rounded-md bg-cream-50 px-2 py-1 text-[10px]"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-semibold text-cream-600">
                    {event.eventType}
                  </span>
                  <span className="text-cream-400">
                    {formatStamp(event.timestamp)}
                  </span>
                </div>
                <p className="break-words text-cream-500">{event.message}</p>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export default AgentDetailDrawer;
