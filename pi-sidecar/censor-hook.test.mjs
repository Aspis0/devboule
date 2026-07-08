/**
 * Smoke test for the Censor review prompt composition.
 * Run: node pi-sidecar/censor-hook.test.mjs
 */

import { composeCensorReviewPrompt, enqueuePrompt } from "./sidecar.mjs";
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

console.log("Censor hook — composeCensorReviewPrompt tests\n");

// --- Test 1: single file with diff ---
test("single file with diff", () => {
	const files = new Map([
		["src/foo.rs", { patch: "@@ -10,3 +10,4 @@\n+let x = 1;\n let y = 2;" }],
	]);
	const prompt = composeCensorReviewPrompt(files);

	assert.ok(prompt.includes("## Censor Review"), "has header");
	assert.ok(prompt.includes("src/foo.rs (diff available)"), "lists file");
	assert.ok(prompt.includes("--- src/foo.rs"), "has diff header");
	assert.ok(prompt.includes("+let x = 1;"), "has diff content");
	assert.ok(prompt.includes("HIGH (must fix)"), "has severity instructions");
	assert.ok(prompt.includes("If clean, reply: CLEAN"), "has clean instruction");
	assert.ok(!prompt.includes("### Diffs:\n\n"), "no empty diffs section");
});

// --- Test 2: single file, full write (no diff) ---
test("single file full write (no diff)", () => {
	const files = new Map([["src/bar.rs", {}]]);
	const prompt = composeCensorReviewPrompt(files);

	assert.ok(
		prompt.includes("src/bar.rs (full write)"),
		"lists file as full write",
	);
	assert.ok(!prompt.includes("### Diffs:"), "no diffs section when no patches");
});

// --- Test 3: multiple files, mixed ---
test("multiple files mixed", () => {
	const files = new Map([
		["src/a.rs", { patch: "@@ -1,1 +1,2 @@\n+fn main() {}" }],
		["src/b.rs", {}],
	]);
	const prompt = composeCensorReviewPrompt(files);

	assert.ok(prompt.includes("src/a.rs (diff available)"), "file a with diff");
	assert.ok(prompt.includes("src/b.rs (full write)"), "file b full write");
	assert.ok(prompt.includes("--- src/a.rs"), "diff for a");
	assert.ok(!prompt.includes("--- src/b.rs"), "no diff for b");
});

// --- Test 4: empty input ---
test("empty files map produces minimal prompt", () => {
	const files = new Map();
	const prompt = composeCensorReviewPrompt(files);

	assert.ok(prompt.includes("## Censor Review"), "still has header");
	assert.ok(prompt.includes("### Files edited:"), "still has files section");
	assert.ok(!prompt.includes("### Diffs:"), "no diffs section");
});

// --- Test 5: prompt structure is valid for LLM consumption ---
test("prompt has reviewable structure", () => {
	const files = new Map([
		["src/main.rs", { patch: "@@ -5,2 +5,3 @@\n+unsafe { ptr::null() }" }],
	]);
	const prompt = composeCensorReviewPrompt(files);

	// Should have clear sections the LLM can parse
	assert.ok(
		prompt.includes("[severity] file:line — description"),
		"has format instructions",
	);
	assert.ok(prompt.includes("MEDIUM (should fix)"), "lists all severities");
	assert.ok(prompt.includes("LOW (consider fixing)"), "lists all severities");
});

// --- Test 6: enqueuePrompt pure helper (Fix 1 FIFO queue) ---
test("enqueuePrompt accepts when below max", () => {
	const { accepted, queue } = enqueuePrompt([], { type: "prompt", message: "a" }, 5);
	assert.ok(accepted, "should accept");
	assert.equal(queue.length, 1);
});

test("enqueuePrompt rejects when full", () => {
	const full = Array.from({ length: 5 }, (_, i) => ({ type: "prompt", message: String(i) }));
	const { accepted, queue } = enqueuePrompt(full, { type: "prompt", message: "x" }, 5);
	assert.ok(!accepted, "should reject when full");
	assert.equal(queue.length, 5);
	assert.deepEqual(queue, full, "queue unchanged on reject");
});

test("enqueuePrompt is immutable", () => {
	const orig = [{ type: "prompt", message: "a" }];
	const { queue } = enqueuePrompt(orig, { type: "prompt", message: "b" }, 5);
	assert.equal(orig.length, 1, "original not mutated");
	assert.equal(queue.length, 2, "new array returned");
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
