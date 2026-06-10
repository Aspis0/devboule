// MC-P6: pure, DOM-free formatters + display model for the per-agent token / cost
// badge (tokenBadgeModel). Kept separate from the React shell (TokenUsageBadge.tsx)
// and the fetch hook so the number/label formatting is unit-testable in node
// without a DOM.
//
// The badge is BEST-EFFORT and degrades silently: an "unavailable" source yields a
// hidden badge (the component renders nothing); "subscription" yields a flat
// "subscription" label with no cost; "claude-transcript" yields "1.2M tok · $3.40".

import type { AgentTokenUsage } from "../../types/backend";

// Compact token count: 1_234 -> "1.2k", 1_200_000 -> "1.2M", 850_000 -> "850k".
// One decimal for k/M, dropped when it would be ".0". Sub-1000 shows the raw int.
// A negative / non-finite input collapses to "0" so a malformed number never
// renders garbage.
export function formatTokenCount(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  const fmt = (value: number, suffix: string): string => {
    // One decimal, but trim a trailing ".0" so 2.0M reads "2M".
    const rounded = Math.round(value * 10) / 10;
    const text = Number.isInteger(rounded)
      ? String(rounded)
      : rounded.toFixed(1);
    return `${text}${suffix}`;
  };
  if (n >= 1_000_000) return fmt(n / 1_000_000, "M");
  if (n >= 1_000) {
    // FIX 6 (k/M boundary): a value like 999_950 rounds to 1000k at one decimal;
    // promote it to "1.0M" instead of rendering the nonsensical "1000k". Done by
    // re-checking the ROUNDED k value (not the raw input) against the 1000 boundary.
    const k = Math.round((n / 1_000) * 10) / 10;
    if (k >= 1_000) return fmt(n / 1_000_000, "M");
    return fmt(n / 1_000, "k");
  }
  // Units -> k boundary: an input like 999.95 rounds to 1000; promote to "1.0k"
  // rather than a bare "1000".
  if (Math.round(n) >= 1_000) return fmt(n / 1_000, "k");
  return String(Math.round(n));
}

// USD cost: 3.4 -> "$3.40", 0.005 -> "$0.01" (rounded up to a visible cent so a
// tiny real cost never reads "$0.00"), 0 -> "$0.00". null -> null (no cost shown).
// Two decimals so it reads as money. A non-finite value collapses to null.
export function formatCostUsd(cost: number | null): string | null {
  if (cost === null || !Number.isFinite(cost)) return null;
  if (cost <= 0) return "$0.00";
  // Avoid a misleading "$0.00" for a real, tiny non-zero cost.
  const shown = cost < 0.01 ? 0.01 : cost;
  return `$${shown.toFixed(2)}`;
}

// What the chip renders. `hidden: true` means render nothing (unavailable). For a
// visible chip, `text` is the full label and `tone` selects a muted color class.
export interface TokenBadgeView {
  hidden: boolean;
  text: string;
  // "claude" = numeric tokens(+cost); "subscription" = flat-rate label. Drives the
  // chip tone so the two read distinctly.
  tone: "claude" | "subscription";
  // Long-form tooltip clarifying the best-effort / approximate nature.
  title: string;
}

const APPROX_NOTE =
  "Best-effort estimate from the Claude Code session transcript (newest session in this project). Cost is approximate (manually-maintained pricing).";

// Map a usage result to the chip's display model. Pure so the component is a thin
// shell and the branching is unit-tested.
export function tokenBadgeView(
  usage: AgentTokenUsage | null | undefined,
): TokenBadgeView {
  if (!usage || usage.source === "unavailable") {
    return { hidden: true, text: "", tone: "claude", title: "" };
  }
  if (usage.source === "subscription") {
    return {
      hidden: false,
      text: "subscription",
      tone: "subscription",
      title:
        "This agent runs on a subscription CLI (no per-token API cost to show).",
    };
  }
  // claude-transcript: tokens (+ cost when known).
  const tokens = `${formatTokenCount(usage.tokens.total)} tok`;
  const cost = formatCostUsd(usage.costUsd);
  const text = cost ? `${tokens} · ${cost}` : tokens;
  return { hidden: false, text, tone: "claude", title: APPROX_NOTE };
}
