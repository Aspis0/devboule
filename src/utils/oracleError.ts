// Normalizes whatever a rejected Oracle Tauri command throws into a typed
// OracleError the UI can branch on. The backend returns `Err(OracleError)`,
// but the Tauri boundary can deliver it three ways:
//   1. as the serialized object `{ kind, message, remediation }` (best case),
//   2. as a JSON string of that object (some serializers stringify),
//   3. as an arbitrary string / Error (older commands, panics, transport).
// We recover the original shape when possible and otherwise wrap the text into
// a sensible `internal` error so callers always receive an `isOracleError`
// value.
//
// Pure module (no React, no DOM) — unit-tested in oracleError.test.ts.

import { isOracleError, type OracleError } from "../types/backend";

const GENERIC_REMEDIATION =
  "Retry the request. If it keeps failing, run the Oracle doctor to diagnose the runtime.";

export function toOracleError(e: unknown): OracleError {
  // Case 1: already the right shape (object delivered across the boundary).
  if (isOracleError(e)) return e;

  // Case 2: a JSON string of an OracleError (serializer stringified the Err).
  if (typeof e === "string") {
    const trimmed = e.trim();
    if (trimmed.startsWith("{")) {
      try {
        const parsed: unknown = JSON.parse(trimmed);
        if (isOracleError(parsed)) return parsed;
      } catch {
        // Not valid JSON — fall through to the plain-string wrap below.
      }
    }
    // Case 3a: arbitrary string.
    return {
      kind: "internal",
      message: trimmed.length > 0 ? trimmed : "Oracle request failed.",
      remediation: GENERIC_REMEDIATION,
    };
  }

  // Case 3b: an Error instance (or anything with a string `message`).
  if (e instanceof Error) {
    return {
      kind: "internal",
      message: e.message || "Oracle request failed.",
      remediation: GENERIC_REMEDIATION,
    };
  }

  // Case 3c: anything else (null, number, object without the shape, ...).
  return {
    kind: "internal",
    message: "Oracle request failed.",
    remediation: GENERIC_REMEDIATION,
  };
}
