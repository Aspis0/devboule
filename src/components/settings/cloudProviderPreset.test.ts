import { describe, expect, it } from "vitest";
import {
	baseUrlForProviderPreset,
	keyLabelForBaseUrl,
	providerPresetFromBaseUrl,
	CLOUD_PROVIDER_PRESETS,
} from "./cloudProviderPreset";

describe("providerPresetFromBaseUrl", () => {
	it("matches known hosts", () => {
		expect(providerPresetFromBaseUrl("https://openrouter.ai/api/v1")).toBe(
			"openrouter",
		);
		expect(providerPresetFromBaseUrl("https://api.openai.com/v1/")).toBe(
			"openai",
		);
		expect(providerPresetFromBaseUrl("https://api.anthropic.com/v1")).toBe(
			"anthropic",
		);
		expect(providerPresetFromBaseUrl("https://api.deepseek.com/v1")).toBe(
			"deepseek",
		);
	});

	it("unknown or empty → neutral empty (select shows —)", () => {
		expect(providerPresetFromBaseUrl("")).toBe("");
		expect(providerPresetFromBaseUrl("https://example.com/v1")).toBe("");
	});

	it("has no Custom option among presets", () => {
		expect(CLOUD_PROVIDER_PRESETS.every((p) => p.id !== ("custom" as string))).toBe(
			true,
		);
		expect(CLOUD_PROVIDER_PRESETS.map((p) => p.id)).toEqual([
			"openrouter",
			"openai",
			"anthropic",
			"deepseek",
		]);
	});
});

describe("baseUrlForProviderPreset", () => {
	it("every selectable preset returns a concrete URL (no no-ops)", () => {
		for (const p of CLOUD_PROVIDER_PRESETS) {
			expect(baseUrlForProviderPreset(p.id)).toBe(p.baseUrl);
			expect(baseUrlForProviderPreset(p.id).length).toBeGreaterThan(0);
		}
	});
});

describe("keyLabelForBaseUrl", () => {
	it("names the provider when URL matches a preset", () => {
		expect(keyLabelForBaseUrl("https://openrouter.ai/api/v1")).toBe(
			"OpenRouter API key",
		);
		expect(keyLabelForBaseUrl("https://api.anthropic.com/v1")).toBe(
			"Anthropic API key",
		);
	});

	it("generic API key when no preset matches", () => {
		expect(keyLabelForBaseUrl("https://example.com/v1")).toBe("API key");
		expect(keyLabelForBaseUrl("")).toBe("API key");
	});
});
