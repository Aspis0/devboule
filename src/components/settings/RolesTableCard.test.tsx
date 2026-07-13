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
	it("blocks saving when placement=Cloud API and consent is unchecked, then allows after consent", async () => {
		currentConfig = { localCoderBackend: undefined };
		await mount();
		const row = orchestratorRow();

		// Switch to the "Cloud API" placement (the reworked 3-way control) — this reveals
		// the consent checkbox.
		const cloudApiBtn = Array.from(row.querySelectorAll("button")).find(
			(b) => b.textContent?.trim() === "Cloud API",
		);
		expect(cloudApiBtn).toBeTruthy();
		await act(async () => {
			cloudApiBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();

		const fields = row.querySelector(
			'[data-testid="roles-orchestrator-fields"]',
		) as HTMLElement;
		expect(fields).toBeTruthy();

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

		// Now save should succeed (no consent error thrown).
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

describe("RolesTableCard — 3-way placement + Cloud API backends", () => {
	function rowFor(key: string): HTMLElement {
		const row = container.querySelector(`[data-testid="role-row-${key}"]`);
		expect(row).toBeTruthy();
		return row as HTMLElement;
	}

	function clickButton(row: HTMLElement, label: string): HTMLButtonElement {
		const btn = Array.from(row.querySelectorAll("button")).find(
			(b) => b.textContent?.trim() === label,
		);
		expect(btn).toBeTruthy();
		return btn as HTMLButtonElement;
	}

	function setInput(row: HTMLElement, placeholder: string, value: string) {
		const input = row.querySelector(
			`input[placeholder="${placeholder}"]`,
		) as HTMLInputElement;
		expect(input).toBeTruthy();
		const setter = Object.getOwnPropertyDescriptor(
			window.HTMLInputElement.prototype,
			"value",
		)!.set!;
		act(() => {
			setter.call(input, value);
			input.dispatchEvent(new Event("input", { bubbles: true }));
		});
	}

	it("(a) orchestrator Cloud API save persists kind cloud and keeps client 'orchestrator'", async () => {
		currentConfig = {};
		await mount();
		const row = orchestratorRow();
		// Switch to the Cloud API placement.
		const cloudApiBtn = clickButton(row, "Cloud API");
		await act(async () => {
			cloudApiBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		// Orchestrator cloud editor uses the LocalCoderBackend-shaped fields.
		setInput(row, "qwen2.5-coder", "gpt-4o");
		setInput(
			row,
			"https://openrouter.ai/api/v1",
			"https://openrouter.ai/api/v1",
		);
		// Acknowledge consent, then save.
		const consent = row.querySelector(
			'[data-testid="cloud-consent-ack"]',
		) as HTMLInputElement;
		expect(consent).toBeTruthy();
		await act(async () => {
			consent.click();
		});
		await flush();
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		)!;
		await act(async () => {
			saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		const localCall = invokeMock.mock.calls.find(
			(c) => c[0] === "set_local_coder_backend",
		);
		expect(localCall).toBeTruthy();
		expect(localCall![1]).toEqual({
			backend: {
				kind: "cloud",
				model: "gpt-4o",
				baseUrl: "https://openrouter.ai/api/v1",
			},
		});
		const rolesCall = invokeMock.mock.calls.find(
			(c) => c[0] === "set_roles_config_cmd",
		);
		expect(rolesCall).toBeTruthy();
		expect(
			(rolesCall![1] as { input: { orchestratorClient: string } }).input
				.orchestratorClient,
		).toBe("orchestrator");
	});

	it("(b) coder Cloud API save calls set_main_coder_backend_cmd with kind cloud", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "local",
			verifierClient: "local",
		};
		currentConfig = {};
		await mount();
		const row = rowFor("coder");
		const cloudApiBtn = clickButton(row, "Cloud API");
		await act(async () => {
			cloudApiBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		// The coder cloud editor uses the MiniCoderBackend-shaped cloud fields.
		setInput(row, "gpt-4o", "gpt-4o");
		setInput(
			row,
			"https://openrouter.ai/api/v1",
			"https://openrouter.ai/api/v1",
		);
		const consent = row.querySelector(
			'[data-testid="cloud-consent-ack"]',
		) as HTMLInputElement;
		expect(consent).toBeTruthy();
		await act(async () => {
			consent.click();
		});
		await flush();
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		)!;
		await act(async () => {
			saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		const call = invokeMock.mock.calls.find(
			(c) => c[0] === "set_main_coder_backend_cmd",
		);
		expect(call).toBeTruthy();
		expect((call![1] as { backend: { kind: string } }).backend.kind).toBe(
			"cloud",
		);
	});

	it("(c) mini row cloud kind select shows the four cloud options", async () => {
		// Seed the mini on the new "cloud" kind so it opens in the Cloud branch.
		currentConfig = {
			miniCoderBackend: {
				kind: "cloud",
				model: "gpt-4o",
				baseUrl: "https://openrouter.ai/api/v1",
			},
		};
		await mount();
		const row = rowFor("mini");
		const select = row.querySelector("select") as HTMLSelectElement;
		expect(select).toBeTruthy();
		// kinds = MINI_CLOUD_KINDS = ["cloud","openai","codex","api"]
		const opts = Array.from(select.options).map((o) => o.value);
		expect(opts).toEqual(["cloud", "openai", "codex", "api"]);
	});

	it("(d) verifier Cloud API persists a backend when Same as Main coder is unchecked", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "local",
			verifierClient: "local",
		};
		currentConfig = {};
		await mount();
		const row = rowFor("verifier");
		// Uncheck "Same as Main coder" to reveal the verifier's own 3-way placement.
		const sameAsMain = row.querySelector(
			'input[type="checkbox"]',
		) as HTMLInputElement;
		expect(sameAsMain).toBeTruthy();
		await act(async () => {
			sameAsMain.click();
		});
		await flush();
		const cloudApiBtn = clickButton(row, "Cloud API");
		await act(async () => {
			cloudApiBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		setInput(row, "gpt-4o", "gpt-4o");
		setInput(
			row,
			"https://openrouter.ai/api/v1",
			"https://openrouter.ai/api/v1",
		);
		const consent = row.querySelector(
			'[data-testid="cloud-consent-ack"]',
		) as HTMLInputElement;
		expect(consent).toBeTruthy();
		await act(async () => {
			consent.click();
		});
		await flush();
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		)!;
		await act(async () => {
			saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		const call = invokeMock.mock.calls.find(
			(c) => c[0] === "set_verifier_backend_cmd",
		);
		expect(call).toBeTruthy();
		const backend = (call![1] as { backend: unknown }).backend;
		expect(backend).not.toBeNull();
		expect((backend as { kind: string }).kind).toBe("cloud");
	});
});

describe("RolesTableCard — B1/M1/M4/M8: no silent kind coercion", () => {
	function rowFor(key: string): HTMLElement {
		const row = container.querySelector(`[data-testid="role-row-${key}"]`);
		expect(row).toBeTruthy();
		return row as HTMLElement;
	}

	function clickButton(row: HTMLElement, label: string): HTMLButtonElement {
		const btn = Array.from(row.querySelectorAll("button")).find(
			(b) => b.textContent?.trim() === label,
		);
		expect(btn);
		return btn as HTMLButtonElement;
	}

	it("renders persisted codex coder backend without coercion and disables Save until offered kind is picked", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "claude",
			verifierClient: "claude",
		};
		currentConfig = {
			mainCoderBackend: { kind: "codex", model: "gpt-4o" },
		};
		await mount();
		const row = rowFor("coder");

		// Coder starts on Cloud CLI (client='claude'). Click Local to reveal the kind select.
		const localBtn = clickButton(row, "Local");
		await act(async () => {
			localBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();

		// Now on Local placement: kind select should show codex as the foreign option.
		// Re-query — the DOM changed after switching placement.
		const select = row.querySelector("select") as HTMLSelectElement;
		expect(select).toBeTruthy();
		expect(select.value).toBe("codex");

		// The foreign option should be disabled.
		const foreignOption = select.querySelector(
			'option[value="codex"]',
		) as HTMLOptionElement;
		expect(foreignOption).toBeTruthy();
		expect(foreignOption.disabled).toBe(true);

		// Foreign-kind note should be visible.
		expect(
			row.querySelector('[data-testid="roles-coder-foreign-kind-note"]'),
		).toBeTruthy();

		// Save button should be disabled.
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		)!;
		expect(saveBtn.disabled).toBe(true);

		// Kind is still codex — NOT coerced to ollama.
		expect(select.value).toBe("codex");

		// Pick an offered kind (ollama).
		const nativeSelectSetter = Object.getOwnPropertyDescriptor(
			window.HTMLSelectElement.prototype,
			"value",
		)!.set!;
		await act(async () => {
			nativeSelectSetter.call(select, "ollama");
			select.dispatchEvent(new Event("change", { bubbles: true }));
		});
		await flush();

		// Save should now be enabled.
		expect(saveBtn.disabled).toBe(false);
	});

	it("Local click does NOT mutate kind; Cloud API click DOES set kind=cloud", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "claude",
			verifierClient: "claude",
		};
		currentConfig = {
			mainCoderBackend: { kind: "codex", model: "gpt-4o" },
		};
		await mount();
		const row = rowFor("coder");

		// Click Local — should NOT change kind from codex (destination ambiguous).
		const localBtn = clickButton(row, "Local");
		await act(async () => {
			localBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		const select = row.querySelector("select") as HTMLSelectElement;
		expect(select.value).toBe("codex"); // NOT coerced to ollama

		// Click Cloud API — IS an explicit action into single-kind placement; sets kind=cloud.
		// CloudApiFields renders model + base URL inputs (no kind select), proving kind=cloud.
		const cloudApiBtn = clickButton(row, "Cloud API");
		await act(async () => {
			cloudApiBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		expect(row.querySelector('input[placeholder="gpt-4o"]')).toBeTruthy();
		expect(
			row.querySelector('input[placeholder="https://openrouter.ai/api/v1"]'),
		).toBeTruthy();
	});

	it("m7: mini cloud fields show consent asymmetry note", async () => {
		currentConfig = {
			miniCoderBackend: {
				kind: "cloud",
				model: "gpt-4o",
				baseUrl: "https://openrouter.ai/api/v1",
			},
		};
		await mount();
		const row = rowFor("mini");
		expect(row.textContent).toContain("Cloud mode sends the mini");
		expect(row.textContent).toContain(
			"consent checkbox lives on the Orchestrator/Coder rows",
		);
	});
});

describe("RolesTableCard — M2: coder save must not silently wipe verifier backend", () => {
	function rowFor(key: string): HTMLElement {
		const row = container.querySelector(`[data-testid="role-row-${key}"]`);
		expect(row).toBeTruthy();
		return row as HTMLElement;
	}

	it("does not call set_verifier_backend_cmd when saving coder with existing verifier backend", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "claude",
			verifierClient: "claude",
		};
		currentConfig = {
			mainCoderBackend: { kind: "ollama", model: "qwen" },
			verifierBackend: { kind: "ollama", model: "test-verifier" },
		};
		await mount();
		invokeMock.mockClear();
		const row = rowFor("coder");
		const saveBtn = Array.from(row.querySelectorAll("button")).find((b) =>
			b.textContent?.includes("Save"),
		)!;
		await act(async () => {
			saveBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
		});
		await flush();
		const verifierCall = invokeMock.mock.calls.find(
			(c) => c[0] === "set_verifier_backend_cmd",
		);
		expect(verifierCall).toBeUndefined();
	});
});

describe("RolesTableCard — M5: hoisted cloud key status fetch", () => {
	it("fetches cloud key status only once across multiple cloud key fields", async () => {
		effectiveRoles = {
			orchestratorClient: "orchestrator",
			coderClient: "local",
			verifierClient: "local",
		};
		currentConfig = {
			localCoderBackend: {
				kind: "cloud",
				model: "gpt-4o",
				baseUrl: "https://openrouter.ai/api/v1",
			},
			mainCoderBackend: {
				kind: "cloud",
				model: "gpt-4o",
				baseUrl: "https://openrouter.ai/api/v1",
			},
		};
		await mount();
		const keyStatusCalls = invokeMock.mock.calls.filter(
			(c) => c[0] === "get_cloud_llm_key_status",
		);
		expect(keyStatusCalls.length).toBe(1);
	});
});
