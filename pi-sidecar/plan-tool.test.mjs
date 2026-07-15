/**
 * Unit tests for the `plan` custom tool (Task 4).
 *
 * Run: node pi-sidecar/plan-tool.test.mjs
 *
 * sidecar.mjs guards main() behind a realpathSync(argv[1]) check, so importing
 * it here does NOT spin up a sidecar session. We only import the exported
 * buildPlanTool() and test it directly — no LLM, no stdin listener.
 */

import { buildPlanTool } from "./sidecar.mjs";
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

console.log("Plan tool — buildPlanTool tests\n");

// --- Test 1: buildPlanTool returns a def with name "plan" and a parameters schema ---
test("buildPlanTool returns a def with name 'plan' and a parameters schema", () => {
	const def = buildPlanTool();

	assert.equal(def.name, "plan", "name must be 'plan'");
	assert.equal(def.label, "Plan", "label must be 'Plan'");
	assert.ok(def.description, "must have a description");
	assert.ok(def.promptSnippet, "must have a promptSnippet");
	assert.ok(def.parameters, "must have a parameters schema");
	assert.ok(typeof def.execute === "function", "must have an execute function");
});

// --- Test 2: execute normalizes steps and returns the expected details ---
test("execute normalizes steps and returns expected details", async () => {
	const def = buildPlanTool();
	const result = await def.execute("tc-1", {
		title: "T",
		steps: [{ text: "a" }, { text: "b", status: "done" }],
		notes: "n",
	});

	assert.deepEqual(result.details, {
		title: "T",
		steps: [
			{ text: "a", status: "pending" },
			{ text: "b", status: "done" },
		],
		notes: "n",
	}, "details shape + normalization");

	assert.equal(result.content.length, 1, "single content block");
	assert.equal(result.content[0].type, "text", "content block is text");
	const text = result.content[0].text;
	assert.ok(text.includes("T"), "text mentions title 'T'");
	assert.ok(text.includes("2"), "text mentions step count '2'");
});

// --- Test 3: execute with empty steps array works (0 steps) ---
test("execute with empty steps array works (0 steps)", async () => {
	const def = buildPlanTool();
	const result = await def.execute("tc-empty", {
		title: "Empty plan",
		steps: [],
	});

	assert.deepEqual(result.details, {
		title: "Empty plan",
		steps: [],
	}, "empty steps preserved");

	assert.ok(result.content[0].text.includes("0"), "text mentions step count '0'");
});

// --- Test 4: notes omitted → no notes key in details ---
test("notes omitted produces no notes key in details", async () => {
	const def = buildPlanTool();
	const result = await def.execute("tc-no-notes", {
		title: "No notes",
		steps: [{ text: "x" }],
	});

	assert.deepEqual(result.details, {
		title: "No notes",
		steps: [{ text: "x", status: "pending" }],
	}, "no notes key when not provided");
	assert.equal(Object.hasOwn(result.details, "notes"), false, "notes key absent");
});

// --- Test 5: notes empty string → no notes key in details ---
test("notes empty string produces no notes key in details", async () => {
	const def = buildPlanTool();
	const result = await def.execute("tc-empty-notes", {
		title: "Empty notes",
		steps: [],
		notes: "",
	});

	assert.equal(Object.hasOwn(result.details, "notes"), false, "notes key absent for empty string");
});

// --- Test 6: garbage status values are normalized to "pending" ---
test("garbage status values are normalized to pending", async () => {
	const def = buildPlanTool();
	const result = await def.execute("tc-garbage-status", {
		title: "Garbage",
		steps: [{ text: "a", status: "garbage" }, { text: "b", status: "done" }],
	});

	assert.deepEqual(result.details.steps, [
		{ text: "a", status: "pending" },
		{ text: "b", status: "done" },
	], "unknown status normalized to pending");
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
