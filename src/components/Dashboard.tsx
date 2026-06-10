import type { AppConfig } from "../types/config";
import { useAppContext } from "../context/AppContext";
import { KpiCard } from "./dashboard/KpiCard";
import { WorkersTable } from "./dashboard/WorkersTable";
import { ScalewayTable } from "./dashboard/ScalewayTable";
import { RiskFlags } from "./dashboard/RiskFlags";
import { ProviderHealth } from "./dashboard/ProviderHealth";
import { ActivityFeed } from "./dashboard/ActivityFeed";
import { OraclePanel } from "./dashboard/OraclePanel";

interface DashboardProps {
  config: AppConfig;
}

export function Dashboard({ config: _config }: DashboardProps) {
  const { cloudSnapshot, syncProviderInventory, isLoading } = useAppContext();
  const snapshot = cloudSnapshot;
  const kpis = snapshot?.kpis ?? [];
  const cloudflareHealth = snapshot?.providerHealth.find((provider) => provider.id === "cloudflare");

  return (
    <div className="space-y-5 max-w-[1400px]">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-[18px] font-semibold text-cream-800">
            Cloud Operations
          </h2>
          <p className="text-[12px] text-cream-400">
            {snapshot?.lastSyncAt ? `Last sync: ${snapshot.lastSyncAt}` : "No live sync yet"}
          </p>
        </div>
        <button
          onClick={() => void syncProviderInventory()}
          disabled={isLoading}
          data-help-title="This syncs all configured cloud providers."
          data-help-lines="Sync asks Cloudflare and Scaleway what exists right now.|It uses saved tokens and pinned scopes from Secrets.|It should be a read operation, not a write or delete.|If a token expired, the related cards will show missing or degraded data."
          className="px-3 py-2 rounded-xl border border-cream-200 bg-white text-[12px] font-medium text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-60"
        >
          {isLoading ? "Syncing..." : "Sync providers"}
        </button>
      </div>

      {/* KPI Row */}
      <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-4">
        {kpis.map((kpi) => (
          <KpiCard key={kpi.label} data={kpi} />
        ))}
        {kpis.length === 0 && (
          <div className="bg-white rounded-2xl border border-cream-200 p-5 text-[13px] text-cream-400">
            Unlock and sync providers to load live dashboard data.
          </div>
        )}
      </div>

      {/* Main content: 9-col tables + 3-col rail */}
      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
        {/* Left: tables + risk flags */}
        <div className="xl:col-span-9 space-y-4">
          <WorkersTable
            workers={snapshot?.workers ?? []}
            canRotateSecrets={
              cloudflareHealth?.status === "healthy" &&
              cloudflareHealth?.tokenHealth === "valid"
            }
            rotationDisabledReason={
              cloudflareHealth?.message ??
              "Cloudflare Worker secret rotation requires Workers Scripts Write."
            }
          />
          <ScalewayTable resources={snapshot?.compute ?? []} />
        </div>

        {/* Right rail */}
        <div className="xl:col-span-3 space-y-4">
          <OraclePanel />
          <RiskFlags flags={snapshot?.risks ?? []} />
          <ProviderHealth
            providers={snapshot?.providerHealth ?? []}
            selectedScopes={snapshot?.selectedScopes ?? []}
          />
          <ActivityFeed entries={snapshot?.activity ?? []} />
        </div>
      </div>
    </div>
  );
}
