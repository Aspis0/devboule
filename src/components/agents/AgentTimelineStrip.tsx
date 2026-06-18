import type { ConsoleActivity, Action } from "./agentConsoleModel";
import { FileText, Pencil, Terminal, Search } from "lucide-react";

/** Flatten a console activity into its ordered tool actions (the chips of the
 *  strip). Only delegated mini runs (`spawn` entries) carry actions; coder
 *  milestones contribute none. Pure + unit-testable. */
export function flattenActions(activity: ConsoleActivity): Action[] {
  const out: Action[] = [];
  for (const entry of activity.entries ?? []) {
    if (entry.type !== "spawn") continue;
    for (const round of entry.mini.rounds ?? []) {
      for (const a of round.actions ?? []) {
        out.push(a);
      }
    }
  }
  return out;
}

/** Keep the strip a single thin row + bounded DOM: show at most the most-recent
 *  MAX_CHIPS steps (a leading "…" flags that older ones are hidden). */
const MAX_CHIPS = 80;

export interface AgentTimelineStripProps {
  activity: ConsoleActivity;
}

/** A Cline-style horizontal storyboard: one small colored chip per tool action,
 *  scannable at a glance, sitting ABOVE the full structured Console timeline.
 *  ONE thin horizontally-scrollable row; renders nothing until there are steps. */
export function AgentTimelineStrip({ activity }: AgentTimelineStripProps) {
  const actions = flattenActions(activity);
  if (actions.length === 0) return null;
  const truncated = actions.length > MAX_CHIPS;
  const shown = truncated ? actions.slice(-MAX_CHIPS) : actions;

  return (
    <div className="flex items-center gap-1 px-1 py-1">
      <span className="shrink-0 text-[10px] font-medium uppercase tracking-widest text-cream-400">
        steps
      </span>
      <div className="flex items-center gap-1 overflow-x-auto">
        {truncated && (
          <span className="shrink-0 text-[11px] text-cream-400" aria-hidden>
            …
          </span>
        )}
        {shown.map((a, i) => {
          const Icon =
            a.kind === "read"
              ? FileText
              : a.kind === "write"
                ? Pencil
                : a.kind === "search"
                  ? Search
                  : Terminal;

          // Status precedence: running > fail > ok (mirrors actionStatus).
          let cls = "border border-sage/30 bg-sage/10 text-sage-dark";
          if (a.status === "run") {
            cls =
              "border border-indigo/30 bg-indigo/10 text-indigo-dark motion-safe:animate-pulse";
          } else if (a.ok === false) {
            cls = "border border-coral/30 bg-coral/[0.06] text-coral-dark";
          }

          const label = `${a.verb}${a.target ? " " + a.target : ""}`;

          return (
            <span
              key={i}
              title={label}
              aria-label={label}
              className={`inline-flex shrink-0 items-center rounded px-1 py-0.5 ${cls}`}
            >
              <Icon size={11} strokeWidth={2} aria-hidden />
            </span>
          );
        })}
      </div>
    </div>
  );
}
