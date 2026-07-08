import { describe, it, expect } from "vitest";
import {
	validateDesignBackend,
	validateDesignEffort,
	validateDesignTimeoutSecs,
	DESIGN_MODEL_MAX_LENGTH,
	DESIGN_COMMAND_MAX_LENGTH,
	DESIGN_BASE_URL_MAX_LENGTH,
	DESIGN_BACKEND_KINDS,
	DESIGN_TIMEOUT_SECS_MIN,
	DESIGN_TIMEOUT_SECS_MAX,
} from "./designLlmBackend";

// The design-LLM backend is a 1:1 MIRROR of the mini-coder backend (validateDesignBackend
// delegates to validateMiniBackend). These tests mirror miniCoderBackend.test.ts so a
// drift between the two surfaces — or with the Rust validate_design_llm_backend — is
// caught here.

// Build an omlx draft with sensible defaults so each test sets only what it varies.
function omlxDraft(model: string, baseUrl: string) {
	return { kind: "omlx" as const, model, command: "dropped", baseUrl };
}

describe("validateDesignBackend", () => {
	it("ollama requires a model and keeps only the model", () => {
		const missing = validateDesignBackend({
			kind: "ollama",
			model: "",
			command: "x",
		});
		expect(missing.ok).toBe(false);
		expect(missing.errors.model).toBeTruthy();
		expect(missing.value).toBeNull();

		const ok = validateDesignBackend({
			kind: "ollama",
			model: "  qwen2.5-coder  ",
			command: "dropped",
		});
		expect(ok.ok).toBe(true);
		expect(ok.value).toEqual({ kind: "ollama", model: "qwen2.5-coder" });
	});

	it("api requires a command and keeps only the command", () => {
		const missing = validateDesignBackend({
			kind: "api",
			model: "m",
			command: "",
		});
		expect(missing.ok).toBe(false);
		expect(missing.errors.command).toBeTruthy();

		const ok = validateDesignBackend({
			kind: "api",
			model: "dropped",
			command: "  mycli chat --json  ",
		});
		expect(ok.ok).toBe(true);
		expect(ok.value).toEqual({ kind: "api", command: "mycli chat --json" });
	});

	it("api rejects a command with a control char (script-injection guard)", () => {
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a\nb" }).errors
				.command,
		).toBeTruthy();
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a\tb" }).errors
				.command,
		).toBeTruthy();
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a\x1bb" })
				.errors.command,
		).toBeTruthy();
	});

	it("api rejects DEL and bidi/invisible chars (WARNING 6 parity with Rust)", () => {
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a\x7fb" })
				.errors.command,
		).toBeTruthy();
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a‮b" }).errors
				.command,
		).toBeTruthy();
		expect(
			validateDesignBackend({ kind: "api", model: "", command: "a​b" }).errors
				.command,
		).toBeTruthy();
	});

	it("codex is OK bare and with an optional model", () => {
		const bare = validateDesignBackend({
			kind: "codex",
			model: "",
			command: "",
		});
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "codex" });

		const withModel = validateDesignBackend({
			kind: "codex",
			model: "gpt-5-codex",
			command: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({ kind: "codex", model: "gpt-5-codex" });
	});

	it("claude is OK bare and with an optional model (mirrors codex)", () => {
		const bare = validateDesignBackend({
			kind: "claude",
			model: "",
			command: "",
		});
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "claude" });

		const withModel = validateDesignBackend({
			kind: "claude",
			model: "claude-sonnet-4-5",
			command: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({
			kind: "claude",
			model: "claude-sonnet-4-5",
		});

		// A bad model tag is rejected just like codex.
		expect(
			validateDesignBackend({
				kind: "claude",
				model: "bad model!",
				command: "",
			}).errors.model,
		).toBeTruthy();
	});

	it("openai is OK bare and with an optional model", () => {
		const bare = validateDesignBackend({
			kind: "openai",
			model: "",
			command: "",
		});
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "openai" });

		const withModel = validateDesignBackend({
			kind: "openai",
			model: "gpt-4o",
			command: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({ kind: "openai", model: "gpt-4o" });
	});

	it("rejects a model with whitespace or shell metacharacters", () => {
		for (const bad of ["qwen coder", "model;rm", "$(evil)", "bad model"]) {
			expect(
				validateDesignBackend({ kind: "ollama", model: bad, command: "" })
					.errors.model,
				`model ${JSON.stringify(bad)} must be rejected`,
			).toBeTruthy();
		}
	});

	it("enforces the model and command length caps", () => {
		const longModel = "a".repeat(DESIGN_MODEL_MAX_LENGTH + 1);
		const longCommand = "b".repeat(DESIGN_COMMAND_MAX_LENGTH + 1);
		expect(
			validateDesignBackend({ kind: "ollama", model: longModel, command: "" })
				.errors.model,
		).toBeTruthy();
		expect(
			validateDesignBackend({ kind: "api", model: "", command: longCommand })
				.errors.command,
		).toBeTruthy();
	});

	// -- effort + timeout (A2, parity with Rust) ----------------------------

	it("normalizes effort to lowercase and accepts only low/medium/high", () => {
		expect(validateDesignEffort("  HIGH ")).toEqual({
			ok: true,
			value: "high",
		});
		expect(validateDesignEffort("low")).toEqual({ ok: true, value: "low" });
		expect(validateDesignEffort("Medium")).toEqual({
			ok: true,
			value: "medium",
		});
		// Absent / empty => no override (ok, undefined).
		expect(validateDesignEffort(undefined)).toEqual({
			ok: true,
			value: undefined,
		});
		expect(validateDesignEffort("   ")).toEqual({ ok: true, value: undefined });
		// Unknown values are REJECTED (not silently dropped).
		for (const bad of ["ultra", "none", "highest", "0", "low high"]) {
			expect(validateDesignEffort(bad).ok, `${bad} must be rejected`).toBe(
				false,
			);
		}
	});

	it("accepts an in-range timeout and rejects out-of-range / non-integer", () => {
		for (const ok of [DESIGN_TIMEOUT_SECS_MIN, 180, DESIGN_TIMEOUT_SECS_MAX]) {
			expect(validateDesignTimeoutSecs(ok)).toEqual({ ok: true, value: ok });
		}
		expect(validateDesignTimeoutSecs(undefined)).toEqual({
			ok: true,
			value: undefined,
		});
		for (const bad of [
			DESIGN_TIMEOUT_SECS_MIN - 1,
			0,
			DESIGN_TIMEOUT_SECS_MAX + 1,
			9999,
			120.5,
			Number.NaN,
			Number.POSITIVE_INFINITY,
		]) {
			expect(validateDesignTimeoutSecs(bad).ok, `${bad} must be rejected`).toBe(
				false,
			);
		}
	});

	it("carries valid effort/timeout onto the normalized value (codex + ollama)", () => {
		const codex = validateDesignBackend({
			kind: "codex",
			model: "",
			command: "",
			effort: "  High ",
			timeoutSecs: 300,
		});
		expect(codex.ok).toBe(true);
		expect(codex.value).toEqual({
			kind: "codex",
			effort: "high",
			timeoutSecs: 300,
		});

		const ollama = validateDesignBackend({
			kind: "ollama",
			model: "qwen2.5-coder",
			command: "",
			effort: "low",
			timeoutSecs: 90,
		});
		expect(ollama.ok).toBe(true);
		expect(ollama.value).toEqual({
			kind: "ollama",
			model: "qwen2.5-coder",
			effort: "low",
			timeoutSecs: 90,
		});
	});

	it("carries valid effort/timeout onto the normalized value (openai)", () => {
		const openai = validateDesignBackend({
			kind: "openai",
			model: "",
			command: "",
			effort: "  High ",
			timeoutSecs: 300,
		});
		expect(openai.ok).toBe(true);
		expect(openai.value).toEqual({
			kind: "openai",
			effort: "high",
			timeoutSecs: 300,
		});
	});

	it("omits absent effort/timeout (no churn) and rejects invalid knobs", () => {
		// Absent knobs => the value has neither key.
		const bare = validateDesignBackend({
			kind: "codex",
			model: "",
			command: "",
		});
		expect(bare.value).toEqual({ kind: "codex" });

		// An invalid knob fails the whole validation with a field-keyed error AND the
		// overall value must be null (never a partially-valid object: the kind validated
		// fine, but a rejected knob invalidates the entire save).
		const badEffort = validateDesignBackend({
			kind: "codex",
			model: "",
			command: "",
			effort: "ultra",
		});
		expect(badEffort.ok).toBe(false);
		expect(badEffort.errors.effort).toBeTruthy();
		expect(badEffort.value).toBeNull();

		const badTimeout = validateDesignBackend({
			kind: "codex",
			model: "",
			command: "",
			timeoutSecs: 9999,
		});
		expect(badTimeout.ok).toBe(false);
		expect(badTimeout.errors.timeoutSecs).toBeTruthy();
		expect(badTimeout.value).toBeNull();
	});

	it("builds a NEW value when applying valid knobs (non-mutating, frozen draft OK)", () => {
		// The draft is frozen: any in-place write to it would throw. validateDesignBackend
		// must read the knobs and emit a fresh value object without writing back.
		const frozen = Object.freeze({
			kind: "codex" as const,
			model: "",
			command: "",
			effort: "high",
			timeoutSecs: 300,
		});
		const res = validateDesignBackend(frozen);
		expect(res.ok).toBe(true);
		expect(res.value).toEqual({
			kind: "codex",
			effort: "high",
			timeoutSecs: 300,
		});
		// The returned value is a fresh object, distinct from the input draft.
		expect(res.value).not.toBe(frozen as unknown as object);
		// The draft was not mutated (no effort/timeout written back; it stays as authored).
		expect(frozen).toEqual({
			kind: "codex",
			model: "",
			command: "",
			effort: "high",
			timeoutSecs: 300,
		});
	});

	it("exposes exactly the six backend kinds (incl. claude + openai)", () => {
		expect([...DESIGN_BACKEND_KINDS]).toEqual([
			"ollama",
			"api",
			"codex",
			"openai",
			"claude",
			"omlx",
		]);
	});

	// -- oMLX parity with the Rust validator --------------------------------
	describe("omlx (parity with Rust validate_design_llm_backend)", () => {
		it("requires both a model and a base URL", () => {
			expect(
				validateDesignBackend(omlxDraft("", "http://localhost:8000/v1")).errors
					.model,
			).toBeTruthy();
			const noUrl = validateDesignBackend(omlxDraft("qwen2.5-coder", ""));
			expect(noUrl.ok).toBe(false);
			expect(noUrl.errors.baseUrl).toBeTruthy();
			expect(
				validateDesignBackend(omlxDraft("qwen2.5-coder", "   ")).errors.baseUrl,
			).toBeTruthy();
		});

		it("accepts loopback http (only) and keeps only {kind, model, baseUrl}", () => {
			for (const url of [
				"http://localhost:8000/v1",
				"http://127.0.0.1:8000/v1",
				"http://127.5.4.3:8000/v1",
				"http://[::1]:8000/v1",
				"http://localhost/v1",
				"http://127.0.0.1",
			]) {
				const res = validateDesignBackend(omlxDraft("  qwen2.5-coder  ", url));
				expect(res.ok, `url ${JSON.stringify(url)} should be accepted`).toBe(
					true,
				);
				expect(res.value).toEqual({
					kind: "omlx",
					model: "qwen2.5-coder",
					baseUrl: url,
				});
			}
		});

		it("rejects https (oMLX is http-only on loopback)", () => {
			for (const bad of [
				"https://localhost:8000/v1",
				"https://127.0.0.1:8000/v1",
				"https://[::1]:8000/v1",
				"https://localhost",
			]) {
				expect(
					validateDesignBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`https url ${JSON.stringify(bad)} must be REJECTED (http only)`,
				).toBeTruthy();
			}
		});

		it("rejects non-loopback host, userinfo, suffix trick, bad scheme, missing scheme", () => {
			for (const bad of [
				"http://evil.com/v1",
				"http://192.168.0.1:8000/v1",
				"http://127.0.0.1.evil.com/v1",
				"http://127.0.0.1@evil.com/v1",
				"http://localhost.evil.com/v1",
				"ftp://localhost:8000/v1",
				"localhost:8000/v1",
				"http://[::1]extra/v1",
				"http://[::1]:8000@evil.com/v1",
				"http://[::1]:@evil.com/v1",
				"http://[::1]@evil.com/v1",
			]) {
				expect(
					validateDesignBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`base url ${JSON.stringify(bad)} must be rejected`,
				).toBeTruthy();
			}
		});

		it("validates the optional :port (parity with Rust)", () => {
			for (const ok of [
				"http://localhost:8000/v1",
				"http://127.0.0.1:1/v1",
				"http://127.0.0.1:65535/v1",
				"http://[::1]:8000/v1",
				"http://[::1]:65535",
				"http://localhost/v1",
			]) {
				expect(
					validateDesignBackend(omlxDraft("qwen2.5-coder", ok)).ok,
					`valid-port url ${JSON.stringify(ok)} must be accepted`,
				).toBe(true);
			}
			for (const bad of [
				"http://localhost:abc/v1",
				"http://localhost:65536/v1",
				"http://localhost:999999",
				"http://localhost:/v1",
				"http://localhost:",
				"http://[::1]:abc",
				"http://[::1]:65536/v1",
				"http://[::1]:",
			]) {
				expect(
					validateDesignBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`invalid-port url ${JSON.stringify(bad)} must be rejected`,
				).toBeTruthy();
			}
		});

		it("rejects control / bidi / invisible chars in the base URL", () => {
			for (const bad of [
				"http://localhost:8000/v1\nrm -rf /",
				"http://localhost:8000/v1\x7f",
				"http://localhost:8000/‮v1",
				"http://localhost:8000/​v1",
			]) {
				expect(
					validateDesignBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`control char in ${JSON.stringify(bad)} must be rejected`,
				).toBeTruthy();
			}
		});

		it("normalizes a trailing slash on the stored base URL", () => {
			expect(
				validateDesignBackend(
					omlxDraft("qwen2.5-coder", "http://localhost:8000/v1/"),
				).value,
			).toEqual({
				kind: "omlx",
				model: "qwen2.5-coder",
				baseUrl: "http://localhost:8000/v1",
			});
			expect(
				validateDesignBackend(
					omlxDraft("qwen2.5-coder", "http://localhost:8000/"),
				).value,
			).toEqual({
				kind: "omlx",
				model: "qwen2.5-coder",
				baseUrl: "http://localhost:8000",
			});
		});

		it("rejects an overlong base URL and a bad model tag", () => {
			const long = `http://localhost:8000/${"a".repeat(DESIGN_BASE_URL_MAX_LENGTH)}`;
			expect(
				validateDesignBackend(omlxDraft("qwen2.5-coder", long)).errors.baseUrl,
			).toBeTruthy();
			expect(
				validateDesignBackend(
					omlxDraft("qwen coder", "http://localhost:8000/v1"),
				).errors.model,
			).toBeTruthy();
		});
	});
});
