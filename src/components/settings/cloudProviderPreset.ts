// Pure UI helpers for Cloud API provider presets (Roles table).
// Prefill base URL only — stored config shape (model + baseUrl) is unchanged.
// Every selectable preset visibly changes the URL; unmatched URLs show "—" in the select.

export type CloudProviderPresetId =
	| "openrouter"
	| "openai"
	| "anthropic"
	| "deepseek";

/** Select value: a known preset id, or `""` for the neutral "—" (no match / custom URL). */
export type CloudProviderSelectValue = CloudProviderPresetId | "";

export const CLOUD_PROVIDER_PRESETS: {
	id: CloudProviderPresetId;
	label: string;
	baseUrl: string;
	/** Paste-field label when this preset matches. */
	keyLabel: string;
}[] = [
	{
		id: "openrouter",
		label: "OpenRouter",
		baseUrl: "https://openrouter.ai/api/v1",
		keyLabel: "OpenRouter API key",
	},
	{
		id: "openai",
		label: "OpenAI",
		baseUrl: "https://api.openai.com/v1",
		keyLabel: "OpenAI API key",
	},
	{
		id: "anthropic",
		label: "Anthropic",
		baseUrl: "https://api.anthropic.com/v1",
		keyLabel: "Anthropic API key",
	},
	{
		id: "deepseek",
		label: "DeepSeek",
		baseUrl: "https://api.deepseek.com/v1",
		keyLabel: "DeepSeek API key",
	},
];

/** Infer the select value from a stored base URL (trailing slash / case insensitive). */
export function providerPresetFromBaseUrl(baseUrl: string): CloudProviderSelectValue {
	const n = baseUrl.trim().replace(/\/+$/, "").toLowerCase();
	if (!n) return "";
	for (const p of CLOUD_PROVIDER_PRESETS) {
		const target = p.baseUrl.replace(/\/+$/, "").toLowerCase();
		if (n === target || n.startsWith(`${target}/`)) return p.id;
	}
	return "";
}

/** Base URL for a selectable preset (always a real URL — no no-op options). */
export function baseUrlForProviderPreset(id: CloudProviderPresetId): string {
	const p = CLOUD_PROVIDER_PRESETS.find((x) => x.id === id);
	return p?.baseUrl ?? CLOUD_PROVIDER_PRESETS[0].baseUrl;
}

/** Per-role key field label from the current base URL (or "API key" when no preset matches). */
export function keyLabelForBaseUrl(baseUrl: string): string {
	const id = providerPresetFromBaseUrl(baseUrl);
	if (!id) return "API key";
	const p = CLOUD_PROVIDER_PRESETS.find((x) => x.id === id);
	return p?.keyLabel ?? "API key";
}
