import {
  Minus,
  Zap,
  Server,
  AlertTriangle,
  type LucideIcon,
} from "lucide-react";
import type { DashboardKpi } from "../../types/backend";

const iconMap: Record<string, LucideIcon> = {
  cloudflare_workers: Zap,
  scaleway_compute: Server,
  risk_flags: AlertTriangle,
};

const statusColors: Record<string, string> = {
  healthy: "bg-sage",
  warning: "bg-amber",
  error: "bg-coral",
  unknown: "bg-cream-400",
};

export function KpiCard({ data }: { data: DashboardKpi }) {
  const Icon = iconMap[data.id] || Minus;
  const color = statusColors[data.status] || "bg-teal";

  return (
    <div
      className="bg-white rounded-2xl border border-cream-200 p-5 flex items-start gap-4"
      data-help-title={`${data.label} is a live operations signal.`}
      data-help-lines="A KPI is a compact warning light, not a full diagnosis.|For Aspis Bio, use these numbers to notice Workers, compute, budget, or risk changes before launching agents.|The value comes from the latest provider sync, so stale syncs can show stale KPIs.|Click into Cloudflare, Compute, Budget, or Oracle when a signal looks wrong."
    >
      <div
        className={`w-10 h-10 rounded-xl ${color} flex items-center justify-center shrink-0`}
      >
        <Icon className="w-5 h-5 text-white" />
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest mb-1">
          {data.label}
        </p>
        <p className="text-2xl font-semibold text-cream-800 tabular-nums leading-tight">
          {data.value}
        </p>
        <div className="flex items-center gap-1.5 mt-1.5">
          <Minus className="w-3.5 h-3.5 text-cream-400" />
          <span className="text-[11px] text-cream-400">{data.subtext}</span>
        </div>
      </div>
    </div>
  );
}
