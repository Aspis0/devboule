// Pure helper for the Header bell badge. The badge sums provider RISK flags and
// the count of agent sessions needing the human, so a "needs you" request bumps
// the same bell the risk flags use. Extracted so the arithmetic is unit-testable
// without rendering the Header.

/** Combine the provider-risk count and the agent-attention count into the single
 *  bell badge number. Negative inputs are clamped to 0 (defensive). */
export function combineBadgeCount(riskCount: number, attentionCount: number): number {
  const risks = Number.isFinite(riskCount) && riskCount > 0 ? riskCount : 0;
  const attention =
    Number.isFinite(attentionCount) && attentionCount > 0 ? attentionCount : 0;
  return risks + attention;
}
