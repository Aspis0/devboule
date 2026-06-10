// MC-P6: the token / cost chip (thin JSX shell). Renders the pure `tokenBadgeView`
// model with the shared badge idiom (rounded-md px-1.5 py-0.5 text-[9px]
// font-semibold). Hidden entirely when the source is unavailable.

import { Coins } from "lucide-react";
import type { AgentTokenUsage } from "../../types/backend";
import { tokenBadgeView } from "./tokenBadgeModel";

export interface TokenUsageBadgeProps {
  usage: AgentTokenUsage | null | undefined;
}

export function TokenUsageBadge({ usage }: TokenUsageBadgeProps) {
  const view = tokenBadgeView(usage);
  if (view.hidden) return null;
  // A distinct muted tone, separate from the role/CLI chips so the cost reads as
  // metadata rather than an action.
  const toneClass =
    view.tone === "subscription"
      ? "bg-cream-100 text-cream-500"
      : "bg-cream-100 text-cream-600";
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[9px] font-semibold tabular-nums ${toneClass}`}
      title={view.title}
      data-help-title="Approximate tokens this agent has used."
      data-help-lines="A best-effort estimate read from the Claude Code session transcript (newest session in this project).|For API-priced Claude it also shows an approximate cost; pricing is manually maintained and may drift.|Subscription CLIs show 'subscription' (no per-token cost).|Only the selected agent is measured, on a slow refresh."
    >
      <Coins className="h-2.5 w-2.5" aria-hidden />
      {view.text}
    </span>
  );
}

export default TokenUsageBadge;
