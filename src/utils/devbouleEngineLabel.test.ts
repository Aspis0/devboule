import { describe, expect, it } from "vitest";
import {
	isCloudApiBackend,
	mainCoderDevbouleLabel,
	orchestratorDevbouleLabel,
} from "./devbouleEngineLabel";

describe("orchestratorDevbouleLabel", () => {
	it("says Local for on-device kinds and missing backend", () => {
		expect(orchestratorDevbouleLabel(undefined)).toBe("Local");
		expect(orchestratorDevbouleLabel({ kind: "omlx", model: "qwen" })).toBe(
			"Local",
		);
		expect(orchestratorDevbouleLabel({ kind: "ollama", model: "qwen" })).toBe(
			"Local",
		);
	});

	it("says Cloud API for kind=cloud (not OpenAI CLI)", () => {
		expect(
			orchestratorDevbouleLabel({
				kind: "cloud",
				model: "anthropic/claude-sonnet-4",
				baseUrl: "https://api.example.com/v1",
			}),
		).toBe("Cloud API");
	});

	it("names OpenRouter when model or base URL mentions it", () => {
		expect(
			orchestratorDevbouleLabel({
				kind: "cloud",
				model: "openrouter/auto",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Cloud API (OpenRouter)");
		expect(
			orchestratorDevbouleLabel({
				kind: "cloud",
				model: "gpt-4o-mini",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Cloud API (OpenRouter)");
	});
});

describe("mainCoderDevbouleLabel", () => {
	it("keeps Local (Devboule) for on-device", () => {
		expect(mainCoderDevbouleLabel({ kind: "omlx", model: "x" })).toBe(
			"Local (Devboule)",
		);
	});

	it("labels cloud honestly", () => {
		expect(
			mainCoderDevbouleLabel({
				kind: "cloud",
				model: "openrouter/auto",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Cloud API (OpenRouter)");
	});
});

describe("isCloudApiBackend", () => {
	it("is true only for kind cloud", () => {
		expect(isCloudApiBackend({ kind: "cloud", model: "m" })).toBe(true);
		expect(isCloudApiBackend({ kind: "omlx", model: "m" })).toBe(false);
		expect(isCloudApiBackend(null)).toBe(false);
	});
});
