import { useEffect, useMemo, useState } from "react";
import {
  BrainCircuit,
  HardDrive,
  ShieldCheck,
  UserCircle,
  Lock,
  type LucideIcon,
} from "lucide-react";
import { useAppActions, useAppContext } from "../../context/AppContext";
import { isViewAllowedForRole } from "../../utils/roles";
import { SecretsView } from "./SecretsView";
import { DevicesView } from "./DevicesView";
import { WorkspaceView } from "./WorkspaceView";
import { ProvidersModelsTab } from "../settings/ProvidersModelsTab";
import { mapLegacySettingsTab, type SettingsTabId } from "./settingsTabs";
import { CollapsibleSection } from "./CollapsibleSection";

interface SettingsTabDef {
  id: SettingsTabId;
  label: string;
  icon: LucideIcon;
}

// Settings is the home for the re-homed "management" pages after the sidebar was
// compressed. Phase 5 collapsed the five tabs into four:
//   - Account            — profile + lock
//   - Providers & Models — every AI provider/model picker (detection strip +
//                          Censor model, Mini-coder, Design LLM)
//   - Workspace & Index  — workspace settings
//   - Security           — Secrets (all roles) + Devices (admin-only)
// Opened from the bottom-left user area in the Sidebar. Deep-links land on a
// specific tab via consumePendingTab, with mapLegacySettingsTab redirecting every
// legacy tab id (secrets/devices→security, oracle→providers, workspace→workspace)
// so persisted AskErrorCard / jump-search links keep working.
export function SettingsView() {
  const { roleStatus, pendingTab } = useAppContext();
  const { consumePendingTab, lock } = useAppActions();

  const tabs = useMemo<SettingsTabDef[]>(
    () => [
      { id: "account", label: "Account", icon: UserCircle },
      { id: "providers", label: "Providers & Models", icon: BrainCircuit },
      { id: "workspace", label: "Workspace & Index", icon: HardDrive },
      { id: "security", label: "Security", icon: ShieldCheck },
    ],
    [],
  );

  const [activeTab, setActiveTab] = useState<SettingsTabId>("account");

  // Consume a deep-link's requested tab. Depend on `pendingTab` (not `tabs`) so a
  // request that arrives while Settings is ALREADY active still re-runs. Every
  // requested id is funnelled through mapLegacySettingsTab so a legacy id (secrets,
  // devices, oracle) lands on its Phase-5 successor; an unknown id falls back to
  // account inside the mapper, so the result is always a real tab.
  useEffect(() => {
    const requested = consumePendingTab();
    if (requested !== null) {
      setActiveTab(mapLegacySettingsTab(requested));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [consumePendingTab, pendingTab]);

  // Devices content is admin-only roster management (matches ADMIN_ONLY_VIEWS). The
  // Security tab is always visible (Secrets is for every role); only the Devices
  // block inside it is gated. The backend still enforces the device commands.
  const canSeeDevices = isViewAllowedForRole(roleStatus?.role ?? null, "devices");

  return (
    <div className="w-full space-y-6">
      <div className="flex w-fit flex-wrap gap-1 rounded-2xl border border-cream-200 bg-white p-1">
        {tabs.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              className={`flex items-center gap-1.5 rounded-xl px-3 py-1.5 text-[12px] font-semibold transition-colors ${
                isActive
                  ? "bg-terracotta text-white"
                  : "text-cream-500 hover:bg-cream-50 hover:text-cream-700"
              }`}
            >
              <Icon className="h-3.5 w-3.5" />
              {tab.label}
            </button>
          );
        })}
      </div>

      {activeTab === "account" && (
        <section className="max-w-2xl space-y-4">
          {/* Profile + Lock — always open (most-used) */}
          <CollapsibleSection title="Profile" defaultOpen={true}>
            <div className="rounded-2xl border border-cream-200 bg-white p-5">
              <div className="flex items-center gap-3">
                <div className="flex h-12 w-12 items-center justify-center rounded-full bg-terracotta-100">
                  <span className="text-[15px] font-semibold text-terracotta-500">
                    MG
                  </span>
                </div>
                <div>
                  <p className="text-[14px] font-semibold text-cream-800">
                    {roleStatus?.isAdmin ? "Administrator" : "Collaborator"}
                  </p>
                  <p className="text-[12px] text-cream-400">
                    {roleStatus?.provisioned === false
                      ? "Onboarding not complete"
                      : "Devboule workspace"}
                  </p>
                </div>
              </div>
              <p className="mt-4 text-[12px] leading-5 text-cream-500">
                Your role decides which admin surfaces are visible. The real cloud
                boundary is the scoped token you hold, enforced by the provider —
                not by hiding pages here.
              </p>
            </div>

            <button
              onClick={() => void lock()}
              className="flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-4 py-2.5 text-[13px] font-semibold text-cream-700 transition-colors hover:border-terracotta-200 hover:text-terracotta"
            >
              <Lock className="h-4 w-4" />
              Lock app
            </button>
          </CollapsibleSection>
        </section>
      )}

      {activeTab === "providers" && <ProvidersModelsTab />}

      {activeTab === "workspace" && (
        <div className="space-y-6">
          <WorkspaceView />
          {/* The Oracle ADMIN surface (runtime, index, doctor, health) lives on
              the restored standalone Oracle page (OracleView), not here, so it is
              not duplicated in Settings. */}
        </div>
      )}

      {activeTab === "security" && (
        <div className="space-y-6">
          <SecretsView />
          {/* Devices is admin-only; collaborators see only the Secrets block. The
              backend still enforces the device commands regardless of the UI. */}
          {canSeeDevices && <DevicesView />}
        </div>
      )}
    </div>
  );
}
