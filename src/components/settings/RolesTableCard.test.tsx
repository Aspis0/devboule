// @vitest-environment jsdom
//
// Tests for RolesTableCard's Orchestrator row inline local-model editor (the settings UX
// gap fix): before this, a Local Orchestrator only showed a static pointer paragraph and
// the actual model editor was buried in the collapsed "Coders (advanced)" group. Now the
// row shows an inline LocalCoderBackend editor (kind/model/baseUrl) that persists through
// the SAME command (`set_local_coder_backend`) the advanced LocalCoderBackendCard uses, so
// the local orchestrator is actually configurable from the one place users look.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { AppConfig, EffectiveRolesConfig } from "../../types/config";

(
	globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

let effectiveRoles: EffectiveRolesConfig = {
	orchestratorClient: "orchestrator",
	coderClient: "claude",
	verifierClient: "claude",
};
let currentConfig: Partial<AppConfig> = {};
const refreshMock = vi.fn(async () => undefined);
const invokeMock = vi.fn(async (...args: unknown[]) => {
	const name = args[0];
	if (name === "detect_providers") return [];
	if (name === "get_roles_config_cmd") return effectiveRoles;
	if (name === "set_roles_config_cmd") {
		const body = args[1] as { input: EffectiveRolesConfig };
		effectiveRoles = { ...effectiveRoles, ...body.input };
		return effectiveRoles;
	}
	if (name === "set_local_coder_backend") return null;
	if (name === "set_verifier_backend_cmd") return null;
	return null;
});

vi.mock("../../context/AppContext", () => ({
	invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
	useAppContext: () => ({ config: currentConfig as AppConfig }),
	useAppActions: () => ({ refreshConfig: refreshMock }),
}));

import { RolesTableCard } from "./RolesTableCard";

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
		root.render(createElement(RolesTableCard));
	});
	// Flush the get_roles_config_cmd / detect_providers mount effects (and the cascading
	// "same as main" seed effect that depends on the resolved clients).
	await flush();
	await flush();
}

function orchestratorRow(): HTMLElement {
	const row = container.querySelector('[data-testid="role-row-orchestrator"]');
	expect(row).toBeTruthy();
	return row as HTMLElement;
}

beforeEach(() => {
	invokeMock.mockClear();
	refreshMock.mockClear();
	effectiveRoles = {
		orchestratorClient: "orchestrator",
		coderClient: "claude",
		verifierClient: "claude",
	};
	currentConfig = {};
});

afterEach(() => {
	act(() => root.unmount());
	container.remove();
});

describe("RolesTableCard — Orchestrator cloud consent gate", () => {
	it("blocks saving when kind=cloud and consent is unchecked, then allows after consent", async () => {
		currentConfig = { localCoderBackend: undefined };
		await mount();
		const row = orchestratorRow();
		const fields = row.querySelector(
			'[data-testid="roles-orchestrator-fields"]',
		) as HTMLElement;
		expect(fields).toBeTruthy();

		const nativeSelectSetter = Object.getOwnPropertyDescriptor(
			window.HTMLSelectElement.prototype,
			"value",
		)!.set!;

		// Switch kind to cloud — this should reveal the consent checkbox.
		const select = fields.querySelector("select") as HTMLSelectElement;
		expect(select).toBeTruthy();
		await act(async () => {
			nativeSelectSetter.call(select, "cloud");
			select.dispatchEvent(new Event("change", { bubbles: true }));
		});

		// The consent checkbox should be present.
		const consentCheckbox = fields.querySelector(
			'[data-testid="cloud-consent-ack"]',
		) as HTMLInputElement;
		expect(consentCheckbox).toBeTruthy();
		expect(consentCheckbox.checked).toBe(false);

		// Try to save without consent — should show an error.
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		);
		expect(saveBtn).toBeTruthy();
		await act(async () => {
			saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		// The error message should appear.
		expect(container.textContent).toContain(
			"Please acknowledge the Cloud consent checkbox",
		);

		// Check the consent checkbox.
		await act(async () => {
			consentCheckbox.click();
		});
		await flush();

		// Now save should succeed (no error thrown).
		invokeMock.mockClear();
		await act(async () => {
			saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		// The error should be gone.
		expect(container.textContent).not.toContain(
			"Please acknowledge the Cloud consent checkbox",
		);
	});
});

describe("RolesTableCard — Orchestrator inline local-model editor", () => {
	it("renders the inline editor (not the old pointer text) when placement is Local and no backend is configured", async () => {
		currentConfig = { localCoderBackend: undefined };
		await mount();
		const row = orchestratorRow();

		expect(
			row.querySelector('[data-testid="roles-orchestrator-fields"]'),
		).not.toBeNull();
		// The old static pointer paragraph is gone.
		expect(row.textContent).not.toContain(
			"configure its model in the Local coder card below",
		);
		expect(row.textContent).not.toContain("Runs as the local Devboule binary");
		// The inline editor exposes the same field vocabulary as the sibling rows'
		// MiniBackendFields (kind dropdown + model input).
		expect(row.textContent).toContain("Backend");
		expect(row.textContent).toContain("Model tag");
	});

	it("saves kind/model through set_local_coder_backend with the right payload", async () => {
		currentConfig = {};
		await mount();
		const row = orchestratorRow();
		const fields = row.querySelector(
			'[data-testid="roles-orchestrator-fields"]',
		) as HTMLElement;
		expect(fields).toBeTruthy();

		const nativeSelectSetter = Object.getOwnPropertyDescriptor(
			window.HTMLSelectElement.prototype,
			"value",
		)!.set!;
		const nativeInputSetter = Object.getOwnPropertyDescriptor(
			window.HTMLInputElement.prototype,
			"value",
		)!.set!;

		// Switch kind to omlx — this reveals the base URL field (required for omlx).
		const select = fields.querySelector("select") as HTMLSelectElement;
		expect(select).toBeTruthy();
		await act(async () => {
			nativeSelectSetter.call(select, "omlx");
			select.dispatchEvent(new Event("change", { bubbles: true }));
		});

		const modelInput = fields.querySelector(
			'input[placeholder="qwen2.5-coder"]',
		) as HTMLInputElement;
		expect(modelInput).toBeTruthy();
		await act(async () => {
			nativeInputSetter.call(modelInput, "mlx-qwen");
			modelInput.dispatchEvent(new Event("input", { bubbles: true }));
		});

		const baseUrlInput = fields.querySelector(
			'input[placeholder="http://localhost:8000/v1"]',
		) as HTMLInputElement;
		expect(baseUrlInput).toBeTruthy();
		await act(async () => {
			nativeInputSetter.call(baseUrlInput, "http://localhost:8000/v1");
			baseUrlInput.dispatchEvent(new Event("input", { bubbles: true }));
		});

		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		);
		expect(saveBtn).toBeTruthy();
		await act(async () => {
			saveBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});

		const saveCall = invokeMock.mock.calls.find(
			(c) => c[0] === "set_local_coder_backend",
		);
		expect(saveCall).toBeTruthy();
		expect(saveCall![1]).toEqual({
			backend: {
				kind: "omlx",
				model: "mlx-qwen",
				baseUrl: "http://localhost:8000/v1",
			},
		});
	});

	it("renders no inline editor when placement is Cloud", async () => {
		effectiveRoles = {
			orchestratorClient: "claude",
			coderClient: "claude",
			verifierClient: "claude",
		};
		currentConfig = {};
		await mount();
		const row = orchestratorRow();

		expect(
			row.querySelector('[data-testid="roles-orchestrator-fields"]'),
		).toBeNull();
		// The Cloud CLI selector renders instead — today's untouched behavior.
		expect(row.textContent).toContain("Cloud CLI");
	});
});
