import type { OracleLlmSettingsStatus, SecretStatus } from "../../types/backend";

// Shared, pure derivation of "is an answer-provider configured?" used by BOTH
// the Oracle admin panel (Settings → Workspace) and the future Polis ask-panel,
// so the two surfaces always agree. Lifted verbatim from the original OracleView
// `providerConfigured` memo to avoid duplicating the rule.
//
// Lightweight only — no model load. Primary signal is the Oracle LLM settings
// status; a configured Scaleway secret is accepted as a fallback so the state is
// not falsely "not configured" before the LLM settings have been refreshed.
// Scaleway is the only valid reuse path: the Oracle LLM runs on Scaleway and the
// OracleAnswerSettings UI says "reused Scaleway token". A Cloudflare secret is
// unrelated to Oracle and must NOT make the Oracle provider appear configured.
export function deriveProviderConfigured(
  oracleLlmSettings: OracleLlmSettingsStatus | null,
  secretStatuses: SecretStatus[] | undefined,
): boolean {
  if (oracleLlmSettings?.apiKeyConfigured) return true;
  return (secretStatuses ?? []).some(
    (s) => s.provider === "scaleway" && s.configured,
  );
}
