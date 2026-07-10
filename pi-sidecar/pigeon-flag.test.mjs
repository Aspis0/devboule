/**
 * Smoke test for the Pigeon-enabled flag parsing (Task #2b).
 * Run: node pi-sidecar/pigeon-flag.test.mjs
 */

import { pigeonEnabled } from "./sidecar.mjs";
import { strict as assert } from "node:assert";

let passed = 0;
let failed = 0;

function test(name, fn) {
	try {
		fn();
		console.log(`  ✅ ${name}`);
		passed++;
	} catch (err) {
		console.error(`  ❌ ${name}: ${err.message}`);
		failed++;
	}
}

console.log("Pigeon flag — pigeonEnabled() truthy parsing tests\n");

// Helper that sets the env var, runs f, then restores.
function withEnv(value, f) {
	const prev = process.env.DEVBOULE_PIGEON_ENABLED;
	if (value === undefined) {
		delete process.env.DEVBOULE_PIGEON_ENABLED;
	} else {
		process.env.DEVBOULE_PIGEON_ENABLED = value;
	}
	try {
		f();
	} finally {
		if (prev === undefined) {
			delete process.env.DEVBOULE_PIGEON_ENABLED;
		} else {
			process.env.DEVBOULE_PIGEON_ENABLED = prev;
		}
	}
}

// --- Test 1: unset ⇒ false (default OFF) ---
test("unset env ⇒ false (default OFF)", () => {
	withEnv(undefined, () => {
		assert.equal(pigeonEnabled(), false);
	});
});

// --- Test 2: explicit true ⇒ true ---
test('"true" ⇒ true', () => {
	withEnv("true", () => {
		assert.equal(pigeonEnabled(), true);
	});
});

// --- Test 3: "0" ⇒ false ---
test('"0" ⇒ false', () => {
	withEnv("0", () => {
		assert.equal(pigeonEnabled(), false);
	});
});

// --- Test 4: "on" ⇒ true ---
test('"on" ⇒ true', () => {
	withEnv("on", () => {
		assert.equal(pigeonEnabled(), true);
	});
});

// --- Test 5: "false" ⇒ false ---
test('"false" ⇒ false', () => {
	withEnv("false", () => {
		assert.equal(pigeonEnabled(), false);
	});
});

// --- Test 6: "off" ⇒ false ---
test('"off" ⇒ false', () => {
	withEnv("off", () => {
		assert.equal(pigeonEnabled(), false);
	});
});

// --- Test 7: "yes" ⇒ true ---
test('"yes" ⇒ true', () => {
	withEnv("yes", () => {
		assert.equal(pigeonEnabled(), true);
	});
});

// --- Test 8: unknown value ⇒ false (safe default) ---
test('unknown value "maybe" ⇒ false', () => {
	withEnv("maybe", () => {
		assert.equal(pigeonEnabled(), false);
	});
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
