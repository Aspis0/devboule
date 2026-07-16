import { test } from "node:test";
import assert from "node:assert/strict";
import { resolveModelWithFallback } from "./sidecar.mjs";

const fakeRegistry = {
	find: (p, i) =>
		p === "openrouter-curated" && i === "tencent/hy3"
			? { provider: p, id: i }
			: undefined,
	getAvailable: () => [
		{ provider: "openrouter-curated", id: "tencent/hy3" },
		{ provider: "ollama", id: "qwen" },
	],
};

test("resolveModelWithFallback — exact match returns model", () => {
	const result = resolveModelWithFallback(
		fakeRegistry,
		"openrouter-curated",
		"tencent/hy3",
	);
	assert.ok(result, "exact match should return a model");
	assert.equal(result.provider, "openrouter-curated");
	assert.equal(result.id, "tencent/hy3");
});

test("resolveModelWithFallback — provider skew falls back by id", () => {
	const result = resolveModelWithFallback(
		fakeRegistry,
		"openrouter",
		"tencent/hy3",
	);
	assert.ok(result, "provider-skew should resolve by id");
	assert.equal(result.provider, "openrouter-curated");
	assert.equal(result.id, "tencent/hy3");
});

test("resolveModelWithFallback — no match returns undefined", () => {
	const result = resolveModelWithFallback(fakeRegistry, "x", "nope");
	assert.equal(result, undefined);
});

test("resolveModelWithFallback — missing getAvailable does not throw", () => {
	const noGetAvailable = {
		find: () => undefined,
		// no getAvailable
	};
	const result = resolveModelWithFallback(noGetAvailable, "openrouter", "tencent/hy3");
	assert.equal(result, undefined);
});
