import { describe, it, expect } from "vitest";
import {
  validateCustomClient,
  slugifyClientId,
  CLIENT_LABEL_MAX_LENGTH,
  CLIENT_COMMAND_MAX_LENGTH,
} from "./customAgentClients";
import type { CustomAgentClient } from "../../types/config";

const existing: CustomAgentClient[] = [
  { id: "deepseek", label: "DeepSeek", command: "deepseek chat" },
];

describe("slugifyClientId", () => {
  it("lowercases, collapses non-alphanumerics to single hyphens and trims", () => {
    expect(slugifyClientId("DeepSeek CLI")).toBe("deepseek-cli");
    expect(slugifyClientId("  My__Local!! Model  ")).toBe("my-local-model");
    expect(slugifyClientId("a")).toBe("a");
  });

  it("caps to 32 chars without a trailing hyphen", () => {
    const out = slugifyClientId("x".repeat(40));
    expect(out).toHaveLength(32);
    expect(out.endsWith("-")).toBe(false);
  });
});

describe("validateCustomClient", () => {
  it("accepts a clean draft and returns a normalized value", () => {
    const r = validateCustomClient(
      { id: " My-LLM ", label: "  My LLM  ", command: "  my-llm run  " },
      [],
    );
    expect(r.ok).toBe(true);
    expect(r.errors).toEqual({});
    expect(r.value).toEqual({ id: "my-llm", label: "My LLM", command: "my-llm run" });
  });

  it("rejects a blank id/label/command with inline messages", () => {
    const r = validateCustomClient({ id: "", label: "", command: "" }, []);
    expect(r.ok).toBe(false);
    expect(r.value).toBeNull();
    expect(r.errors.id).toBeTruthy();
    expect(r.errors.label).toBeTruthy();
    expect(r.errors.command).toBeTruthy();
  });

  it("rejects an id with illegal characters", () => {
    expect(validateCustomClient({ id: "Bad Id", label: "L", command: "c" }, []).errors.id)
      .toMatch(/a-z/);
    expect(validateCustomClient({ id: "under_score", label: "L", command: "c" }, []).errors.id)
      .toBeTruthy();
  });

  it("rejects an id longer than 32 chars", () => {
    expect(
      validateCustomClient({ id: "a".repeat(33), label: "L", command: "c" }, []).errors.id,
    ).toBeTruthy();
  });

  it("rejects a reserved built-in id (case-insensitively)", () => {
    expect(validateCustomClient({ id: "codex", label: "L", command: "c" }, []).errors.id)
      .toMatch(/reserved/);
    expect(validateCustomClient({ id: "CLAUDE", label: "L", command: "c" }, []).errors.id)
      .toMatch(/reserved/);
    expect(validateCustomClient({ id: "PowerShell", label: "L", command: "c" }, []).errors.id)
      .toMatch(/reserved/);
  });

  it("rejects a duplicate id against the existing list", () => {
    expect(
      validateCustomClient({ id: "deepseek", label: "L", command: "c" }, existing).errors.id,
    ).toMatch(/already in use/);
  });

  it("enforces label and command length caps", () => {
    const longLabel = "x".repeat(CLIENT_LABEL_MAX_LENGTH + 1);
    const longCommand = "y".repeat(CLIENT_COMMAND_MAX_LENGTH + 1);
    expect(validateCustomClient({ id: "ok", label: longLabel, command: "c" }, []).errors.label)
      .toBeTruthy();
    expect(validateCustomClient({ id: "ok", label: "L", command: longCommand }, []).errors.command)
      .toBeTruthy();
  });

  // A command is embedded VERBATIM into the launch script; an interior newline /
  // carriage-return / NUL / other control char (< 0x20) would split it into extra
  // script statements while the launch token is still in scope. The single-line UI
  // input can't produce one, but a hand-edited config.json (read leniently by the
  // backend) can — so the byte-equal Rust + TS validators both reject control chars.
  it("rejects a command containing a newline, CR, NUL or other control char", () => {
    expect(validateCustomClient({ id: "ok", label: "L", command: "a\nb" }, []).errors.command)
      .toBeTruthy();
    expect(validateCustomClient({ id: "ok", label: "L", command: "a\rb" }, []).errors.command)
      .toBeTruthy();
    expect(validateCustomClient({ id: "ok", label: "L", command: "a\0b" }, []).errors.command)
      .toBeTruthy();
    expect(validateCustomClient({ id: "ok", label: "L", command: "a\x1bb" }, []).errors.command)
      .toBeTruthy();
    expect(validateCustomClient({ id: "ok", label: "L", command: "a\tb" }, []).errors.command)
      .toBeTruthy();
  });

  it("accepts a normal command with spaces and flags", () => {
    const r = validateCustomClient({ id: "ok", label: "L", command: "deepseek chat --flag" }, []);
    expect(r.ok).toBe(true);
    expect(r.value?.command).toBe("deepseek chat --flag");
  });
});
