// Top strip of the Agents fleet view: the one-line fleet headline, per-(role,
// model) count chips, a compact health roll-up (online/stale/lost), and a
// needs-you badge. Replaces the old four counters + six-bucket health grid.
//
// All numbers come from the pure agentFleet selectors + agentLiveStatus health,
// computed from the SAME live sessions the rows render, so the summary can never
// disagree with the list.

import { AlertTriangle, Bot, Users } from "lucide-react";
import { useMemo } from "react";
import type { AgentSession } from "../../types/backend";
import {
  attentionSessions,
  fleetCounts,
  fleetHeadlineSuffix,
  summarizeFleet,
} from "./agentFleet";
import { fleetHealthRollup } from "./agentRowModel";

const modelChipTone: Record<string, string> = {
  opus: "bg-terracotta/10 text-terracotta",
  sonnet: "bg-teal/10 text-teal",
  haiku: "bg-sage/10 text-sage-dark",
};

export function FleetSummary({
  sessions,
  now,
}: {
  sessions: AgentSession[];
  now: number;
}) {
  const counts = useMemo(() => fleetCounts(sessions, now), [sessions, now]);
  const headline = useMemo(() => summarizeFleet(counts), [counts]);
  // Muted disclaimer appended to the headline when the counts fold in any
  // self-reported subagents, so the number is not read as a verified count.
  const subagentSuffix = useMemo(
    () => fleetHeadlineSuffix(sessions, now),
    [sessions, now],
  );
  const attention = useMemo(
    () => attentionSessions(sessions, now),
    [sessions, now],
  );

  // Compact health roll-up over the (non-closed) live sessions.
  const health = useMemo(() => fleetHealthRollup(sessions, now), [sessions, now]);

  const totalAgents = counts.reduce((sum, c) => sum + c.count, 0);

  return (
    <section
      className="rounded-2xl border border-cream-200 bg-white p-4"
      data-help-title="The fleet summary is the at-a-glance state of all agents."
      data-help-lines="The headline counts agents by model and role, including self-reported subagents.|Counts include subagents self-reported by agents over MCP; these are advisory and not independently verified.|The chips break that down per model/role pair.|The health roll-up shows how many are online vs. need attention.|The needs-you badge counts agents blocked waiting on you or with a stale heartbeat."
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-terracotta/10">
            <Bot className="h-5 w-5 text-terracotta" />
          </div>
          <div className="min-w-0">
            <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-400">
              Fleet
            </p>
            <p className="truncate text-sm font-semibold text-cream-800">
              {totalAgents === 0 ? "No agents running" : headline}
              {totalAgents > 0 && subagentSuffix && (
                <span className="font-normal text-cream-400">
                  {subagentSuffix}
                </span>
              )}
            </p>
          </div>
        </div>

        {/* Health roll-up + needs-you. */}
        <div className="flex flex-wrap items-center gap-2">
          <span className="inline-flex items-center gap-1 rounded-md bg-sage/10 px-2 py-1 text-[11px] font-semibold text-sage-dark">
            {health.online} online
          </span>
          {health.stale > 0 && (
            <span className="inline-flex items-center gap-1 rounded-md bg-amber/10 px-2 py-1 text-[11px] font-semibold text-amber-dark">
              {health.stale} stale
            </span>
          )}
          {health.lost > 0 && (
            <span className="inline-flex items-center gap-1 rounded-md bg-coral/10 px-2 py-1 text-[11px] font-semibold text-coral-dark">
              {health.lost} lost
            </span>
          )}
          {attention.length > 0 && (
            <span className="inline-flex items-center gap-1 rounded-md bg-amber/20 px-2 py-1 text-[11px] font-semibold text-amber-dark">
              <AlertTriangle className="h-3.5 w-3.5" aria-hidden />
              {attention.length} need you
            </span>
          )}
        </div>
      </div>

      {/* Per-(role, model) chips. */}
      {counts.length > 0 && (
        <div className="mt-3 flex flex-wrap gap-1.5">
          {counts.map((c) => (
            <span
              key={`${c.role}:${c.model}`}
              className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-semibold ${
                modelChipTone[c.model] ?? "bg-cream-100 text-cream-500"
              }`}
              title={`${c.count} ${c.model} ${c.role}`}
            >
              <Users className="h-3 w-3" aria-hidden />
              {c.count} {c.model} {c.role}
            </span>
          ))}
        </div>
      )}
    </section>
  );
}

export default FleetSummary;
