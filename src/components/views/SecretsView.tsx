import { Fingerprint } from "lucide-react";
import { authMethodLabel, isAppleHost } from "../../lib/platform";
import { GithubProviderCard } from "./GithubProviderCard";

/**
 * Secrets page — GitHub vault only.
 * Cloudflare / Scaleway provider tokens, scopes, agent profiles, and object keys
 * were removed with the cloud-provider subsystem.
 */
export function SecretsView() {
  return (
    <div className="max-w-4xl space-y-6">
      {/* Device-auth protection banner */}
      <div className="flex items-center gap-4 p-5 bg-white rounded-2xl border border-cream-200">
        <div className="w-10 h-10 rounded-xl bg-terracotta-50 flex items-center justify-center shrink-0">
          <Fingerprint className="w-5 h-5 text-terracotta" />
        </div>
        <div>
          <p className="text-[14px] font-medium text-cream-800">
            Protected by {authMethodLabel(isAppleHost())}
          </p>
          <p className="text-[12px] text-cream-400">
            All secret values are encrypted at rest and only accessible after
            biometric verification. Token values are never displayed.
          </p>
        </div>
      </div>

      {/* Secrets list — GitHub only */}
      <div className="bg-white rounded-2xl border border-cream-200 divide-y divide-cream-100 overflow-hidden">
        <GithubProviderCard />
      </div>
    </div>
  );
}
