import type { OracleLlmSettingsStatus, SecretStatus } from "../../types/backend";

// Shared, pure derivation of "is an answer-provider configured?" used by BOTH
// the Oracle admin panel (Settings → Workspace) and the future Polis ask-panel,
// so the two surfaces always agree. Lifted verbatim from the original OracleView
// `providerConfigured` memo to avoid duplicating the rule.
//
// Lightweight only — no model load. Primary signal is the Oracle LLM settings
// status. Local providers (oMLX/Ollama) are keyless and signal via status.
// Remote providers (OpenAI, OpenRouter, DeepSeek) require an API key.
export function deriveProviderConfigured(
  oracleLlmSettings: OracleLlmSettingsStatus | null,
  // Reserved (kept for a stable 2-arg signature shared by the admin panel and
  // the Polis ask-panel). The Scaleway-token fallback that consumed it is gone;
  // configuration is now derived solely from the Oracle LLM settings status.
  _secretStatuses: SecretStatus[] | undefined,
): boolean {
  if (oracleLlmSettings?.apiKeyConfigured) return true;
  // LOCAL providers (oMLX/Ollama) are KEYLESS by design — the Rust vault
  // encodes "usable" into `status` ("configured" for both a keyed remote AND a
  // keyless local; "missing_api_key"/"local" otherwise). So a "configured"
  // status means the answer provider is ready even with no API key — without
  // this, selecting a local provider left the ask panel falsely "not
  // configured" and refused questions.
  if (oracleLlmSettings?.status === "configured") return true;
  return false;
}
