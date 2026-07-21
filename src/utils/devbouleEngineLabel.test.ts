import { describe, expect, it } from "vitest";
import {
	enginePlacementBadge,
	isCloudApiBackend,
	mainCoderDevbouleLabel,
	orchestratorDevbouleLabel,
} from "./devbouleEngineLabel";

describe("orchestratorDevbouleLabel", () => {
	it("always says Local on the planner WHO axis (even if Settings is Cloud API)", () => {
		expect(orchestratorDevbouleLabel(undefined)).toBe("Local");
		expect(orchestratorDevbouleLabel({ kind: "omlx", model: "qwen" })).toBe(
			"Local",
		);
		expect(
			orchestratorDevbouleLabel({
				kind: "cloud",
				model: "openrouter/auto",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Local");
	});
});

describe("mainCoderDevbouleLabel", () => {
	it("always says Local (Devboule) on the hand-off WHO axis", () => {
		expect(mainCoderDevbouleLabel({ kind: "omlx", model: "x" })).toBe(
			"Local (Devboule)",
		);
		expect(
			mainCoderDevbouleLabel({
				kind: "cloud",
				model: "openrouter/auto",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Local (Devboule)");
	});
});

describe("enginePlacementBadge", () => {
	it("names on-device vs Cloud API for Settings/status, not planner chips", () => {
		expect(enginePlacementBadge({ kind: "omlx", model: "q" })).toBe("oMLX");
		expect(enginePlacementBadge({ kind: "ollama", model: "q" })).toBe("Ollama");
		expect(
			enginePlacementBadge({
				kind: "cloud",
				model: "openrouter/auto",
				baseUrl: "https://openrouter.ai/api/v1",
			}),
		).toBe("Cloud API · OpenRouter");
	});
});

describe("isCloudApiBackend", () => {
	it("is true only for kind cloud", () => {
		expect(isCloudApiBackend({ kind: "cloud", model: "m" })).toBe(true);
		expect(isCloudApiBackend({ kind: "omlx", model: "m" })).toBe(false);
		expect(isCloudApiBackend(null)).toBe(false);
	});
});
