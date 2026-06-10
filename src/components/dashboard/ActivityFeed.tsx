import { Rocket, AlertTriangle, Wallet, KeyRound, ArrowUpDown } from "lucide-react";
import type { ActivityEvent } from "../../types/backend";

const typeConfig = {
  deploy: { icon: Rocket, color: "text-sage", bg: "bg-sage/10" },
  alert: { icon: AlertTriangle, color: "text-amber", bg: "bg-amber/10" },
  budget: { icon: Wallet, color: "text-teal", bg: "bg-teal/10" },
  secret: { icon: KeyRound, color: "text-terracotta", bg: "bg-terracotta-50" },
  spawn: { icon: Rocket, color: "text-terracotta", bg: "bg-terracotta-50" },
  scale: { icon: ArrowUpDown, color: "text-teal", bg: "bg-teal/10" },
  sync: { icon: ArrowUpDown, color: "text-teal", bg: "bg-teal/10" },
};

export function ActivityFeed({ entries }: { entries: ActivityEvent[] }) {
  return (
    <div
      className="bg-white rounded-2xl border border-cream-200 p-5"
      data-help-title="Recent Activity is the operational timeline."
      data-help-lines="Activity records what the app recently saw or did: syncs, alerts, budget signals, secret events, deploys, or compute changes.|For Aspis Bio, this is useful evidence before asking Oracle or launching verifier agents.|It is not a full audit log; project notes and provider-specific audit tabs hold stronger evidence.|If something critical is missing here, check the specific provider page."
    >
      <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest mb-4">
        Recent Activity
      </h3>

      <div className="space-y-2.5">
        {entries.map((entry) => {
          const cfg = typeConfig[entry.eventType as keyof typeof typeConfig] || typeConfig.sync;
          const Icon = cfg.icon;

          return (
            <div
              key={entry.id}
              className="flex items-start gap-3"
              data-help-title={`${entry.eventType} activity from ${entry.source}.`}
              data-help-lines="This entry is a recent observation from the dashboard backend.|For Aspis Bio, use it to reconstruct what changed before a task, deploy, cost spike, or provider action.|Activity is useful context, but provider state should be re-synced before risky actions.|Important evidence should also be attached to the relevant project note."
            >
              <div
                className={`w-7 h-7 rounded-lg ${cfg.bg} flex items-center justify-center shrink-0 mt-0.5`}
              >
                <Icon className={`w-3.5 h-3.5 ${cfg.color}`} />
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-[13px] text-cream-700 leading-snug line-clamp-2">
                  {entry.message}
                </p>
                <div className="flex items-center gap-2 mt-0.5">
                  <span className="text-[10px] text-cream-400">
                    {entry.source}
                  </span>
                  <span className="text-[10px] text-cream-300">·</span>
                  <span className="text-[10px] text-cream-400">
                    {entry.timestamp}
                  </span>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      {entries.length === 0 && (
        <p className="text-[12px] text-cream-400">No activity yet.</p>
      )}
    </div>
  );
}
