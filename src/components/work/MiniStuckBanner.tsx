/**
 * MiniStuckBanner — renders an amber banner row for each stuck mini-coder report.
 * Mirrors the Tailwind token vocabulary of the "read-only (archived)" banner in
 * ProjectWorkspace.tsx (`border-amber/30`, `bg-amber/[0.06]`, `text-amber-dark`).
 */

import type { MiniStuckReport } from "./miniStuckModel";
import { stuckReasonLabel } from "./miniStuckModel";

export interface MiniStuckBannerProps {
  reports: MiniStuckReport[];
  onDismiss: (taskId: string) => void;
}

function shortAgent(id: string): string {
  return id.length > 10 ? id.slice(0, 8) + "…" : id;
}

export function MiniStuckBanner({ reports, onDismiss }: MiniStuckBannerProps) {
  if (reports.length === 0) return null;

  return (
    <div data-testid="mini-stuck-banner" className="flex flex-col gap-2">
      {reports.map((report) => (
        <div
          key={report.taskId}
          className="flex flex-col gap-2 rounded-2xl border border-amber/30 bg-amber/[0.06] p-3 sm:flex-row sm:items-center sm:justify-between"
        >
          <p className="text-[12px] font-semibold text-amber-dark">
            ⚠︎ Mini {shortAgent(report.agentId)} {stuckReasonLabel(report.reason)}{" "}
            after {report.attempts} attempt(s)
          </p>
          <button
            type="button"
            data-testid={`mini-stuck-dismiss-${report.taskId}`}
            onClick={() => onDismiss(report.taskId)}
            className="shrink-0 self-start rounded-lg border border-amber/30 bg-white px-2.5 py-1 text-[11px] font-semibold text-amber-dark hover:bg-amber/[0.06] sm:self-auto"
          >
            Dismiss
          </button>
        </div>
      ))}
    </div>
  );
}
