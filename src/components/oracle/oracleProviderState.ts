import type { OracleLlmSettingsStatus } from "../../types/backend";

// Shared, pure derivation of "is an answer-provider configured?" used by BOTH
// the Oracle admin panel (Settings → Workspace) and the Polis ask-panel,
// so the two surfaces always agree. Lifted verbatim from the original OracleView
// `providerConfigured` memo to avoid duplicating the rule.
//
// Lightweight only — no model load. Primary signal is the Oracle LLM settings
// status. Local providers (oMLX/Ollama) are keyless and signal via status.
// Remote providers (OpenAI, OpenRouter, DeepSeek) require an API key.
//
// IMPORTANT: a former cloud-provider secret (Cloudflare/Scaleway inventory
// token) must NEVER make Oracle appear configured — those were unrelated vault
// entries. Configuration is derived solely from Oracle LLM settings status.
export function deriveProviderConfigured(
  oracleLlmSettings: OracleLlmSettingsStatus | null,
  // Reserved second arg kept for a stable signature shared by callers.
  // Intentionally ignored — never use vault inventory secrets for Oracle.
  _unused?: unknown,
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
