// Alpha: these cloud providers are hidden from the UI (not removed from the
// backend or types). Clear this set to re-enable them in ProvidersView and
// SecretsView. This is intentionally trivially reversible.
export const ALPHA_HIDDEN_PROVIDERS = new Set<string>(["scaleway", "cloudflare"]);
