import { describe, it, expect } from "vitest";
import { toOracleError } from "./oracleError";
import { isOracleError, type OracleError } from "../types/backend";

const REAL_ERROR: OracleError = {
  kind: "indexEmpty",
  message: "The dense index is empty.",
  remediation: "Run the indexer before asking the Oracle.",
};

describe("isOracleError", () => {
  it("accepts a well-formed OracleError with a known kind", () => {
    expect(isOracleError(REAL_ERROR)).toBe(true);
  });

  it("rejects an object with an unknown kind", () => {
    expect(isOracleError({ ...REAL_ERROR, kind: "bogus" })).toBe(false);
  });

  it("rejects partial shapes, strings, null and undefined", () => {
    expect(isOracleError({ kind: "internal", message: "x" })).toBe(false);
    expect(isOracleError("internal")).toBe(false);
    expect(isOracleError(null)).toBe(false);
    expect(isOracleError(undefined)).toBe(false);
    expect(isOracleError(42)).toBe(false);
  });
});

describe("toOracleError", () => {
  it("passes through a real OracleError object unchanged", () => {
    const out = toOracleError(REAL_ERROR);
    expect(out).toBe(REAL_ERROR);
    expect(isOracleError(out)).toBe(true);
  });

  it("parses a stringified JSON OracleError back into shape", () => {
    const out = toOracleError(JSON.stringify(REAL_ERROR));
    expect(isOracleError(out)).toBe(true);
    expect(out.kind).toBe("indexEmpty");
    expect(out.message).toBe("The dense index is empty.");
    expect(out.remediation).toBe("Run the indexer before asking the Oracle.");
  });

  it("wraps an arbitrary string into an internal OracleError", () => {
    const out = toOracleError("server returned 500");
    expect(isOracleError(out)).toBe(true);
    expect(out.kind).toBe("internal");
    expect(out.message).toBe("server returned 500");
    expect(out.remediation.length).toBeGreaterThan(0);
  });

  it("wraps an Error instance using its message", () => {
    const out = toOracleError(new Error("boom"));
    expect(isOracleError(out)).toBe(true);
    expect(out.kind).toBe("internal");
    expect(out.message).toBe("boom");
  });

  it("wraps a JSON string that is not an OracleError as internal", () => {
    const out = toOracleError(JSON.stringify({ foo: "bar" }));
    expect(isOracleError(out)).toBe(true);
    expect(out.kind).toBe("internal");
    expect(out.message).toContain("foo");
  });

  it("handles null / unknown values with a sensible default", () => {
    const out = toOracleError(null);
    expect(isOracleError(out)).toBe(true);
    expect(out.kind).toBe("internal");
    expect(out.message).toBe("Oracle request failed.");
  });
});
