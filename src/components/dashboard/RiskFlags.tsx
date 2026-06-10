import { AlertTriangle, AlertCircle, Info } from "lucide-react";
import type { RiskFlag } from "../../types/backend";

const severityConfig = {
  high: {
    icon: AlertTriangle,
    bg: "bg-coral/8",
    border: "border-coral/20",
    dot: "bg-coral",
    text: "text-coral-dark",
    badge: "bg-coral/10 text-coral-dark",
  },
  medium: {
    icon: AlertCircle,
    bg: "bg-amber/8",
    border: "border-amber/20",
    dot: "bg-amber",
    text: "text-amber-dark",
    badge: "bg-amber/10 text-amber-dark",
  },
  low: {
    icon: Info,
    bg: "bg-teal/8",
    border: "border-teal/20",
    dot: "bg-teal",
    text: "text-teal-dark",
    badge: "bg-teal/10 text-teal-dark",
  },
};

export function RiskFlags({ flags }: { flags: RiskFlag[] }) {
  return (
    <div
      className="bg-white rounded-2xl border border-cream-200 p-5"
      data-help-title="Risk Flags are provider warnings that need human attention."
      data-help-lines="A risk flag is a live or computed warning from Cloudflare, Scaleway, budget, or secret state.|For Aspis Bio, high risks should block agent launches or destructive actions until reviewed.|Risk flags are only as fresh as the last sync and audit.|Use the linked source page to confirm whether the risk is real before changing cloud resources."
    >
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest">
          Risk Flags
        </h3>
        <span className="text-[11px] text-cream-400">
          {flags.filter((f) => f.severity === "high").length} high
        </span>
      </div>

      <div className="space-y-2.5">
        {flags.map((flag) => {
          const cfg = severityConfig[flag.severity as keyof typeof severityConfig] || severityConfig.low;
          const Icon = cfg.icon;

          return (
            <div
              key={flag.id}
              data-help-title={`${flag.title} is a ${flag.severity} risk.`}
              data-help-lines="This warning explains what could break, leak, or cost money.|For Aspis Bio, treat high risks as blockers for production-like actions and agent automation.|Check the source and timestamp before acting because stale provider data can mislead.|A verifier should confirm fixes before closing the related project task."
              className={`flex items-start gap-3 p-3 rounded-xl ${cfg.bg} border ${cfg.border}`}
            >
              <Icon className={`w-4 h-4 mt-0.5 shrink-0 ${cfg.text}`} />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 mb-0.5">
                  <p className={`text-[13px] font-medium ${cfg.text} truncate`}>
                    {flag.title}
                  </p>
                  <span
                    className={`shrink-0 px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wider ${cfg.badge}`}
                  >
                    {flag.severity}
                  </span>
                </div>
                <p className="text-[11px] text-cream-500 leading-relaxed">
                  {flag.description}
                </p>
                <div className="flex items-center gap-2 mt-1.5">
                  <span className="text-[10px] text-cream-400">
                    {flag.source}
                  </span>
                  <span className="text-[10px] text-cream-300">·</span>
                  <span className="text-[10px] text-cream-400">
                    {flag.timestamp}
                  </span>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      {flags.length === 0 && (
        <p className="text-[12px] text-cream-400">No provider risks reported.</p>
      )}
    </div>
  );
}
