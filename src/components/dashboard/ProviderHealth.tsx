import {
  Cloud,
  Server,
  Brain,
  Github,
  ExternalLink,
  type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import type {
  ProviderHealth as ProviderHealthEntry,
  ProviderScopeSelection,
} from "../../types/backend";
import { safeOpenExternal } from "../../utils/safeOpenExternal";

const iconMap: Record<string, LucideIcon> = {
  cloudflare: Cloud,
  scaleway: Server,
  Brain,
  Github,
};

const statusDot = {
  healthy: "bg-sage",
  degraded: "bg-amber",
  down: "bg-coral",
  error: "bg-coral",
  missing_token: "bg-coral",
};

const tokenBadge = {
  valid: { bg: "bg-sage/10", text: "text-sage-dark", label: "Valid" },
  expiring: { bg: "bg-amber/10", text: "text-amber-dark", label: "Expiring" },
  expired: { bg: "bg-coral/10", text: "text-coral-dark", label: "Expired" },
  unknown: { bg: "bg-cream-100", text: "text-cream-500", label: "Unknown" },
  missing: { bg: "bg-coral/10", text: "text-coral-dark", label: "Missing" },
  invalid: { bg: "bg-coral/10", text: "text-coral-dark", label: "Invalid" },
  insufficient_scope: { bg: "bg-coral/10", text: "text-coral-dark", label: "Scope" },
  valid_read_only: { bg: "bg-amber/10", text: "text-amber-dark", label: "Read-only" },
  valid_unverified: { bg: "bg-amber/10", text: "text-amber-dark", label: "Unverified" },
};

const consoleUrls: Record<string, string> = {
  cloudflare: "https://dash.cloudflare.com",
  scaleway: "https://console.scaleway.com",
};

export function ProviderHealth({
  providers,
  selectedScopes,
}: {
  providers: ProviderHealthEntry[];
  selectedScopes: ProviderScopeSelection[];
}) {
  const [externalError, setExternalError] = useState<string | null>(null);

  const openExternal = async (url: string) => {
    setExternalError(null);
    try {
      await safeOpenExternal(url);
    } catch (e) {
      setExternalError(e instanceof Error ? e.message : "External link failed.");
    }
  };

  return (
    <div className="bg-white rounded-2xl border border-cream-200 p-5">
      <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest mb-4">
        Provider Health
      </h3>

      <div className="space-y-3">
        {providers.map((p) => {
          const Icon = iconMap[p.id] || Cloud;
          const badge = tokenBadge[p.tokenHealth as keyof typeof tokenBadge] || tokenBadge.unknown;
          const dot = statusDot[p.status as keyof typeof statusDot] || "bg-cream-400";
          const selectedScope = selectedScopes.find((scope) => scope.provider === p.id);

          return (
            <div
              key={p.id}
              className="flex items-center gap-3 p-3 rounded-xl bg-cream-50/60 hover:bg-cream-50 transition-colors"
            >
              <div className="w-8 h-8 rounded-lg bg-white border border-cream-200 flex items-center justify-center shrink-0">
                <Icon className="w-4 h-4 text-cream-600" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span
                    className={`w-1.5 h-1.5 rounded-full shrink-0 ${dot}`}
                  />
                  <p className="text-[13px] font-medium text-cream-800 truncate">
                    {p.name}
                  </p>
                </div>
                <div className="flex items-center gap-2 mt-1">
                  <span className="text-[10px] text-cream-400">
                    {p.resourceCount} resources
                  </span>
                  <span className="text-[10px] text-cream-300">·</span>
                  <span className="text-[10px] text-cream-400">
                    synced {p.lastSync || "never"}
                  </span>
                </div>
                {selectedScope && (
                  <p className="mt-1 truncate font-mono text-[10px] text-cream-400">
                    Scope: {selectedScope.name ?? selectedScope.id} / {selectedScope.source}
                  </p>
                )}
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <span
                  className={`px-2 py-0.5 rounded-full text-[10px] font-medium ${badge.bg} ${badge.text}`}
                >
                  {badge.label}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    const url = consoleUrls[p.id];
                    if (url) void openExternal(url);
                  }}
                  data-help-title="This opens the provider's web console."
                  data-help-lines="The web console is outside Aspis Management.|Use it to confirm billing, revoke tokens, or inspect permissions directly.|Opening it does not run any provider action.|For repeatable operations, add guarded app actions instead of doing them manually."
                  className="p-1.5 rounded-lg hover:bg-cream-200/60 transition-colors"
                  title="Open console"
                >
                  <ExternalLink className="w-3.5 h-3.5 text-cream-400" />
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {providers.length === 0 && (
        <p className="text-[12px] text-cream-400">No provider status synced.</p>
      )}
      {externalError && (
        <p className="mt-3 rounded-xl border border-coral/20 bg-coral/[0.04] px-3 py-2 text-[11px] font-medium text-coral-dark">
          {externalError}
        </p>
      )}
    </div>
  );
}
