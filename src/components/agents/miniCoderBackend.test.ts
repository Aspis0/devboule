import { describe, it, expect } from "vitest";
import {
	validateMiniBackend,
	MINI_MODEL_MAX_LENGTH,
	MINI_COMMAND_MAX_LENGTH,
	MINI_BASE_URL_MAX_LENGTH,
	MINI_BACKEND_KINDS,
} from "./miniCoderBackend";

// Build an omlx draft with sensible defaults so each test sets only what it varies.
function omlxDraft(model: string, baseUrl: string) {
	return { kind: "omlx" as const, model, command: "dropped", baseUrl };
}

describe("validateMiniBackend", () => {
	it("ollama requires a model and keeps only the model", () => {
		const missing = validateMiniBackend({
			kind: "ollama",
			model: "",
			command: "x",
		});
		expect(missing.ok).toBe(false);
		expect(missing.errors.model).toBeTruthy();
		expect(missing.value).toBeNull();

		const ok = validateMiniBackend({
			kind: "ollama",
			model: "  qwen2.5-coder  ",
			command: "dropped",
		});
		expect(ok.ok).toBe(true);
		expect(ok.value).toEqual({ kind: "ollama", model: "qwen2.5-coder" });
	});

	it("api requires a command and keeps only the command", () => {
		const missing = validateMiniBackend({
			kind: "api",
			model: "m",
			command: "",
		});
		expect(missing.ok).toBe(false);
		expect(missing.errors.command).toBeTruthy();

		const ok = validateMiniBackend({
			kind: "api",
			model: "dropped",
			command: "  mycli chat --json  ",
		});
		expect(ok.ok).toBe(true);
		expect(ok.value).toEqual({ kind: "api", command: "mycli chat --json" });
	});

	it("api rejects a command with a control char (script-injection guard)", () => {
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a\nb" }).errors
				.command,
		).toBeTruthy();
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a\tb" }).errors
				.command,
		).toBeTruthy();
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a\x1bb" }).errors
				.command,
		).toBeTruthy();
	});

	it("api rejects DEL and bidi/invisible chars (WARNING 6 parity with Rust)", () => {
		// 0x7f (DEL) — missed by a plain `< 0x20` check.
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a\x7fb" }).errors
				.command,
		).toBeTruthy();
		// U+202E RIGHT-TO-LEFT OVERRIDE (bidi, category Cf).
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a‮b" }).errors
				.command,
		).toBeTruthy();
		// U+200B ZERO WIDTH SPACE (invisible).
		expect(
			validateMiniBackend({ kind: "api", model: "", command: "a​b" }).errors
				.command,
		).toBeTruthy();
	});

	it("codex is OK bare and with an optional model", () => {
		const bare = validateMiniBackend({ kind: "codex", model: "", command: "" });
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "codex" });

		const withModel = validateMiniBackend({
			kind: "codex",
			model: "gpt-5-codex",
			command: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({ kind: "codex", model: "gpt-5-codex" });
	});

	it("openai is OK bare and with an optional model (does not silently save as codex)", () => {
		const bare = validateMiniBackend({
			kind: "openai",
			model: "",
			command: "",
		});
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "openai" });

		const withModel = validateMiniBackend({
			kind: "openai",
			model: "gpt-4o",
			command: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({ kind: "openai", model: "gpt-4o" });
	});

	it("appleFm accepts optional model and preserves only kind/model", () => {
		const bare = validateMiniBackend({
			kind: "appleFm",
			model: "",
			command: "dropped",
			baseUrl: "dropped",
		});
		expect(bare.ok).toBe(true);
		expect(bare.value).toEqual({ kind: "appleFm" });

		const withModel = validateMiniBackend({
			kind: "appleFm",
			model: "apple-default",
			command: "dropped",
			baseUrl: "dropped",
		});
		expect(withModel.ok).toBe(true);
		expect(withModel.value).toEqual({
			kind: "appleFm",
			model: "apple-default",
		});
	});

	it("rejects a model with whitespace or shell metacharacters", () => {
		for (const bad of ["qwen coder", "model;rm", "$(evil)", "bad model"]) {
			expect(
				validateMiniBackend({ kind: "ollama", model: bad, command: "" }).errors
					.model,
				`model ${JSON.stringify(bad)} must be rejected`,
			).toBeTruthy();
		}
	});

	it("enforces the model and command length caps", () => {
		const longModel = "a".repeat(MINI_MODEL_MAX_LENGTH + 1);
		const longCommand = "b".repeat(MINI_COMMAND_MAX_LENGTH + 1);
		expect(
			validateMiniBackend({ kind: "ollama", model: longModel, command: "" })
				.errors.model,
		).toBeTruthy();
		expect(
			validateMiniBackend({ kind: "api", model: "", command: longCommand })
				.errors.command,
		).toBeTruthy();
	});

	it("exposes all backend kinds", () => {
		expect([...MINI_BACKEND_KINDS]).toEqual([
			"ollama",
			"api",
			"codex",
			"openai",
			"omlx",
			"appleFm",
		]);
	});

	// -- oMLX parity with the Rust validator --------------------------------
	describe("omlx (parity with Rust validate_omlx_base_url)", () => {
		it("requires both a model and a base URL", () => {
			expect(
				validateMiniBackend(omlxDraft("", "http://localhost:8000/v1")).errors
					.model,
			).toBeTruthy();
			const noUrl = validateMiniBackend(omlxDraft("qwen2.5-coder", ""));
			expect(noUrl.ok).toBe(false);
			expect(noUrl.errors.baseUrl).toBeTruthy();
			expect(
				validateMiniBackend(omlxDraft("qwen2.5-coder", "   ")).errors.baseUrl,
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
				const res = validateMiniBackend(omlxDraft("  qwen2.5-coder  ", url));
				expect(res.ok, `url ${JSON.stringify(url)} should be accepted`).toBe(
					true,
				);
				// trailing-slash-free urls round-trip unchanged; model trimmed; no command.
				expect(res.value).toEqual({
					kind: "omlx",
					model: "qwen2.5-coder",
					baseUrl: url,
				});
			}
		});

		it("rejects https (F3 parity with Rust — oMLX is http-only on loopback)", () => {
			for (const bad of [
				"https://localhost:8000/v1",
				"https://127.0.0.1:8000/v1",
				"https://[::1]:8000/v1",
				"https://localhost",
			]) {
				expect(
					validateMiniBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
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
				"http://[::1]:8000@evil.com/v1", // F1: ipv6 userinfo bypass
				"http://[::1]:@evil.com/v1", // F1: minimal ipv6 userinfo bypass
				"http://[::1]@evil.com/v1", // F1: ipv6 userinfo, no port
			]) {
				expect(
					validateMiniBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`base url ${JSON.stringify(bad)} must be rejected`,
				).toBeTruthy();
			}
		});

		it("rejects the IPv6 userinfo loopback bypass (F1 parity with Rust)", () => {
			for (const bad of [
				"http://[::1]:8000@evil.com",
				"http://[::1]:@evil.com",
				"http://[::1]@evil.com",
				"https://[::1]:8000@evil.com/v1",
			]) {
				expect(
					validateMiniBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`ipv6 userinfo bypass ${JSON.stringify(bad)} must be REJECTED`,
				).toBeTruthy();
			}
		});

		it("validates the optional :port (F2 parity with Rust)", () => {
			for (const ok of [
				"http://localhost:8000/v1",
				"http://127.0.0.1:1/v1",
				"http://127.0.0.1:65535/v1",
				"http://[::1]:8000/v1",
				"http://[::1]:65535",
				"http://localhost/v1", // no port at all is fine
			]) {
				expect(
					validateMiniBackend(omlxDraft("qwen2.5-coder", ok)).ok,
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
					validateMiniBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
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
					validateMiniBackend(omlxDraft("qwen2.5-coder", bad)).errors.baseUrl,
					`control char in ${JSON.stringify(bad)} must be rejected`,
				).toBeTruthy();
			}
		});

		it("normalizes a trailing slash on the stored base URL", () => {
			expect(
				validateMiniBackend(
					omlxDraft("qwen2.5-coder", "http://localhost:8000/v1/"),
				).value,
			).toEqual({
				kind: "omlx",
				model: "qwen2.5-coder",
				baseUrl: "http://localhost:8000/v1",
			});
			expect(
				validateMiniBackend(
					omlxDraft("qwen2.5-coder", "http://localhost:8000/"),
				).value,
			).toEqual({
				kind: "omlx",
				model: "qwen2.5-coder",
				baseUrl: "http://localhost:8000",
			});
		});

		it("rejects an overlong base URL and a bad model tag", () => {
			const long = `http://localhost:8000/${"a".repeat(MINI_BASE_URL_MAX_LENGTH)}`;
			expect(
				validateMiniBackend(omlxDraft("qwen2.5-coder", long)).errors.baseUrl,
			).toBeTruthy();
			expect(
				validateMiniBackend(omlxDraft("qwen coder", "http://localhost:8000/v1"))
					.errors.model,
			).toBeTruthy();
		});
	});
});
