// @vitest-environment jsdom
//
// Tests for the OracleAnswerSettingsCard enable/disable toggle button.
//
// When the Answer LLM is disabled the card shows "Enable answer LLM"; when
// enabled it shows "Disable answer LLM". Each button calls
// saveOracleLlmSettings with the appropriate payload.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { OracleLlmSettings, OracleLlmSettingsStatus } from "../../types/backend";

(
	globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

let mockOracleLlmSettings: OracleLlmSettingsStatus | null = null;
const refreshMock = vi.fn(async () => undefined);
const saveMock = vi.fn(async (_settings: OracleLlmSettings, _apiKey: string | null) => mockOracleLlmSettings);
const deleteKeyMock = vi.fn(async () => mockOracleLlmSettings);
const isLoadingMock = { current: false };

vi.mock("../../context/AppContext", () => ({
	useAppContext: () => ({
		oracleLlmSettings: mockOracleLlmSettings,
		refreshOracleLlmSettings: refreshMock,
		saveOracleLlmSettings: saveMock,
		deleteOracleLlmApiKey: deleteKeyMock,
		isLoading: isLoadingMock.current,
	}),
}));

import { OracleAnswerSettingsCard } from "./OracleAnswerSettingsCard";

let container: HTMLDivElement;
let root: Root;

async function flush() {
	await act(async () => {
		await Promise.resolve();
		await Promise.resolve();
	});
}

async function mount() {
	container = document.createElement("div");
	document.body.appendChild(container);
	root = createRoot(container);
	await act(async () => {
		root.render(createElement(OracleAnswerSettingsCard));
	});
	// Flush the mount effect (refreshOracleLlmSettings) and any cascading state.
	await flush();
	await flush();
}

beforeEach(() => {
	mockOracleLlmSettings = null;
	refreshMock.mockClear();
	saveMock.mockClear();
	deleteKeyMock.mockClear();
	isLoadingMock.current = false;
});

afterEach(() => {
	act(() => root.unmount());
	container.remove();
});

describe("OracleAnswerSettingsCard — enable/disable toggle", () => {
	it("renders 'Enable answer LLM' when status is disabled", async () => {
		mockOracleLlmSettings = {
			settings: { provider: "", model: "", baseUrl: null, remoteEnabled: false },
			apiKeyConfigured: false,
			status: "disabled",
			message: null,
		};
		await mount();

		expect(container.textContent).toContain("Enable answer LLM");
		expect(container.textContent).not.toContain("Disable answer LLM");
	});

	it("renders 'Disable answer LLM' when status is configured", async () => {
		mockOracleLlmSettings = {
			settings: {
				provider: "openai",
				model: "gpt-4o-mini",
				baseUrl: null,
				remoteEnabled: true,
			},
			apiKeyConfigured: true,
			status: "configured",
			message: null,
		};
		await mount();

		expect(container.textContent).toContain("Disable answer LLM");
		expect(container.textContent).not.toContain("Enable answer LLM");
	});

	it("clicking Enable does NOT call saveOracleLlmSettings and populates form with openai/gpt-4o-mini", async () => {
		mockOracleLlmSettings = {
			settings: { provider: "", model: "", baseUrl: null, remoteEnabled: false },
			apiKeyConfigured: false,
			status: "disabled",
			message: null,
		};
		await mount();

		const enableBtn = Array.from(container.querySelectorAll("button")).find(
			(b) => b.textContent?.includes("Enable answer LLM"),
		);
		expect(enableBtn).toBeTruthy();
		expect((enableBtn as HTMLButtonElement).disabled).toBe(false);

		await act(async () => {
			enableBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();

		// Enable must NOT persist — user reviews then presses Save LLM.
		expect(saveMock).not.toHaveBeenCalled();

		// Form should now show the enabled defaults.
		const modelInput = container.querySelector(
			'input[placeholder="provider/model"]',
		) as HTMLInputElement;
		expect(modelInput).toBeTruthy();
		expect(modelInput.value).toBe("gpt-4o-mini");

		const providerSelect = container.querySelector("select") as HTMLSelectElement;
		expect(providerSelect).toBeTruthy();
		expect(providerSelect.value).toBe("openai");
	});

	it("clicking Disable calls saveOracleLlmSettings with remoteEnabled: false and empty provider", async () => {
		mockOracleLlmSettings = {
			settings: {
				provider: "openai",
				model: "gpt-4o-mini",
				baseUrl: null,
				remoteEnabled: true,
			},
			apiKeyConfigured: true,
			status: "configured",
			message: null,
		};
		await mount();

		const disableBtn = Array.from(container.querySelectorAll("button")).find(
			(b) => b.textContent?.includes("Disable answer LLM"),
		);
		expect(disableBtn).toBeTruthy();
		expect((disableBtn as HTMLButtonElement).disabled).toBe(false);

		await act(async () => {
			disableBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});

		expect(saveMock).toHaveBeenCalledOnce();
		const [settings, apiKey] = saveMock.mock.calls[0]!;
		expect(settings.remoteEnabled).toBe(false);
		expect(settings.provider).toBe("");
		expect(settings.model).toBe("");
		expect(apiKey).toBeNull();
	});

	it("clicking Enable with local provider selected populates model but keeps provider and does not save", async () => {
		mockOracleLlmSettings = {
			settings: { provider: "", model: "", baseUrl: null, remoteEnabled: false },
			apiKeyConfigured: false,
			status: "disabled",
			message: null,
		};
		await mount();

		// Switch provider to omlx (local) via the select.
		const providerSelect = container.querySelector("select") as HTMLSelectElement;
		expect(providerSelect).toBeTruthy();
		await act(async () => {
			providerSelect.value = "omlx";
			providerSelect.dispatchEvent(new Event("change", { bubbles: true }));
		});
		await flush();

		// Verify provider select now shows omlx.
		expect(providerSelect.value).toBe("omlx");

		// Click Enable.
		const enableBtn = Array.from(container.querySelectorAll("button")).find(
			(b) => b.textContent?.includes("Enable answer LLM"),
		);
		expect(enableBtn).toBeTruthy();

		await act(async () => {
			enableBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();

		// Must NOT save.
		expect(saveMock).not.toHaveBeenCalled();

		// Provider should still be omlx (local), not overridden to openai.
		expect(providerSelect.value).toBe("omlx");

		// Model input should be populated with gpt-4o-mini default.
		const modelInput = container.querySelector(
			'input[placeholder="qwen3.5:4b"]',
		) as HTMLInputElement;
		expect(modelInput).toBeTruthy();
		expect(modelInput.value).toBe("gpt-4o-mini");
	});

	it("renders 'Disable answer LLM' when status is missing_api_key", async () => {
		mockOracleLlmSettings = {
			settings: {
				provider: "openai",
				model: "gpt-4o-mini",
				baseUrl: null,
				remoteEnabled: true,
			},
			apiKeyConfigured: false,
			status: "missing_api_key",
			message: null,
		};
		await mount();

		expect(container.textContent).toContain("Disable answer LLM");
		expect(container.textContent).not.toContain("Enable answer LLM");
	});
});
