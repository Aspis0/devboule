import { test, describe, mock } from "node:test";
import assert from "node:assert/strict";
import { parseModelChain, switchToNextModel, runModelFallbackLoop } from "./sidecar.mjs";

// ---------------------------------------------------------------------------
// parseModelChain
// ---------------------------------------------------------------------------

describe("parseModelChain", () => {
	test("absent var → [primary] from provider/model env", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 1);
		assert.equal(chain[0].provider, "openai");
		assert.equal(chain[0].model, "gpt-4o");
	});

	test("valid JSON array → cleaned entries", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
			DEVBOULE_PI_MODEL_CHAIN: JSON.stringify([
				{ provider: "openai", model: "gpt-4o" },
				{ provider: "openrouter", model: "tencent/hy3:free", baseUrl: "https://openrouter.ai/api/v1" },
			]),
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 2);
		assert.equal(chain[0].provider, "openai");
		assert.equal(chain[0].model, "gpt-4o");
		assert.equal(chain[1].provider, "openrouter");
		assert.equal(chain[1].model, "tencent/hy3:free");
		assert.equal(chain[1].baseUrl, "https://openrouter.ai/api/v1");
	});

	test("malformed JSON → [primary]", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
			DEVBOULE_PI_MODEL_CHAIN: "{not json",
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 1);
		assert.equal(chain[0].model, "gpt-4o");
	});

	test("empty array → [primary]", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
			DEVBOULE_PI_MODEL_CHAIN: "[]",
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 1);
		assert.equal(chain[0].model, "gpt-4o");
	});

	test("entries with empty model filtered out", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
			DEVBOULE_PI_MODEL_CHAIN: JSON.stringify([
				{ provider: "openai", model: "gpt-4o" },
				{ provider: "openai", model: "" },
				{ provider: "openai", model: "gpt-4" },
			]),
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 2);
		assert.equal(chain[0].model, "gpt-4o");
		assert.equal(chain[1].model, "gpt-4");
	});

	test("missing provider inherits primary provider", () => {
		const env = {
			DEVBOULE_PI_PROVIDER: "openai",
			DEVBOULE_PI_MODEL: "gpt-4o",
			DEVBOULE_PI_MODEL_CHAIN: JSON.stringify([
				{ model: "gpt-4" },
			]),
		};
		const chain = parseModelChain(env);
		assert.equal(chain.length, 1);
		assert.equal(chain[0].provider, "openai");
		assert.equal(chain[0].model, "gpt-4");
	});
});

// ---------------------------------------------------------------------------
// switchToNextModel
// ---------------------------------------------------------------------------

describe("switchToNextModel", () => {
	test("chain [{a},{b}] index 0, registry resolves b → returns true, sets model, emits", async () => {
		const chain = [{ provider: "a", model: "model-a" }, { provider: "b", model: "model-b" }];
		const chainState = { chain, index: 0 };
		const calls = [];
		const events = [];
		const session = {
			setModel: async (m) => {
				calls.push(m);
			},
		};
		const modelRegistry = {
			find: (provider, modelId) => {
				if (provider === "b" && modelId === "model-b") return { provider: "b", model: "model-b" };
				return undefined;
			},
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const result = await switchToNextModel(session, modelRegistry, "provider error", chainState, emitFn);

		assert.equal(result, true);
		assert.equal(chainState.index, 1);
		assert.equal(calls.length, 1);
		assert.equal(calls[0].model, "model-b");
		assert.equal(events.length, 1);
		assert.equal(events[0].type, "devboule_model_switch");
		assert.equal(events[0].from, "model-a");
		assert.equal(events[0].to, "model-b");
		assert.equal(events[0].resolved, true);
	});

	test("index at last entry → returns false, no setModel, no index change", async () => {
		const chain = [{ provider: "a", model: "model-a" }, { provider: "b", model: "model-b" }];
		const chainState = { chain, index: 1 };
		const calls = [];
		const events = [];
		const session = {
			setModel: async (m) => {
				calls.push(m);
			},
		};
		const modelRegistry = {
			find: () => undefined,
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const result = await switchToNextModel(session, modelRegistry, "provider error", chainState, emitFn);

		assert.equal(result, false);
		assert.equal(chainState.index, 1);
		assert.equal(calls.length, 0);
		assert.equal(events.length, 0);
	});

	test("next model unresolvable → returns false, emit resolved:false, index unchanged, setModel NOT called", async () => {
		const chain = [{ provider: "a", model: "model-a" }, { provider: "b", model: "model-b" }];
		const chainState = { chain, index: 0 };
		const calls = [];
		const events = [];
		const session = {
			setModel: async (m) => {
				calls.push(m);
			},
		};
		const modelRegistry = {
			find: () => undefined,
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const result = await switchToNextModel(session, modelRegistry, "provider error", chainState, emitFn);

		assert.equal(result, false);
		assert.equal(chainState.index, 0);
		assert.equal(calls.length, 0);
		assert.equal(events.length, 1);
		assert.equal(events[0].type, "devboule_model_switch");
		assert.equal(events[0].resolved, false);
		assert.equal(events[0].from, "model-a");
		assert.equal(events[0].to, "model-b");
	});
});

// ---------------------------------------------------------------------------
// runModelFallbackLoop
// ---------------------------------------------------------------------------

describe("runModelFallbackLoop", () => {
	test("recovers after one switch", async () => {
		const chain = [{ provider: "a", model: "model-a" }, { provider: "b", model: "model-b" }];
		const chainState = { chain, index: 0 };
		const calls = [];
		const events = [];
		let failed = true;
		const failState = {
			isFailed: () => failed,
			error: () => "rate limit",
			clear: () => { failed = false; },
		};
		const session = {
			setModel: async (m) => { calls.push(m); },
			prompt: async () => {},
		};
		const modelRegistry = {
			find: (provider, modelId) => {
				if (provider === "b" && modelId === "model-b") return { provider: "b", model: "model-b" };
				return undefined;
			},
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const hops = await runModelFallbackLoop(session, modelRegistry, chainState, emitFn, failState, async () => {});

		assert.equal(hops, 1);
		assert.equal(calls.length, 1);
		assert.equal(calls[0].model, "model-b");
		assert.equal(chainState.index, 1);
		assert.equal(events.length, 1);
		assert.equal(events[0].type, "devboule_model_switch");
		assert.equal(events[0].resolved, true);
	});

	test("exhausts chain: isFailed always true, terminates cleanly", async () => {
		const chain = [{ provider: "a", model: "model-a" }, { provider: "b", model: "model-b" }];
		const chainState = { chain, index: 0 };
		const calls = [];
		const events = [];
		// failState never clears — simulates every re-prompt also failing.
		const failState = {
			isFailed: () => true,
			error: () => "x",
			clear: () => {},
		};
		const session = {
			setModel: async (m) => { calls.push(m); },
			prompt: async () => {},
		};
		const modelRegistry = {
			find: (provider, modelId) => {
				if (provider === "b" && modelId === "model-b") return { provider: "b", model: "model-b" };
				return undefined;
			},
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const hops = await runModelFallbackLoop(session, modelRegistry, chainState, emitFn, failState, async () => {});

		// hop1: switch a→b (setModel called once, index 1), clear (no-op), runPrompt
		// hop2: switchToNextModel at index 1 (last entry) returns false → break (no emit)
		assert.equal(hops, 2);
		assert.equal(calls.length, 1);
		assert.equal(chainState.index, 1);
		// one successful switch emit (hop1); the exhausted second attempt returns false without emitting
		assert.equal(events.length, 1);
		assert.equal(events[0].resolved, true);
	});

	test("no fallbacks: single-entry chain, zero setModel calls", async () => {
		const chain = [{ provider: "a", model: "model-a" }];
		const chainState = { chain, index: 0 };
		const calls = [];
		const events = [];
		const failState = {
			isFailed: () => true,
			error: () => "x",
			clear: () => {},
		};
		const session = {
			setModel: async (m) => { calls.push(m); },
			prompt: async () => {},
		};
		const modelRegistry = {
			find: () => undefined,
			getAvailable: () => [],
		};
		const emitFn = (e) => events.push(e);

		const hops = await runModelFallbackLoop(session, modelRegistry, chainState, emitFn, failState, async () => {});

		// chain.length === 1, so maxHops === 1: hop1 → switchToNextModel returns false immediately (no emit).
		assert.equal(hops, 1);
		assert.equal(calls.length, 0);
		assert.equal(chainState.index, 0);
		assert.equal(events.length, 0);
	});
});
