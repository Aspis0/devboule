// Polis 4b — unit tests for the PURE "More details" dossier decision logic.
//
// The persistence / fingerprint / staleness CORE lives in Rust (and is tested
// there); these cover the frontend's lazy + fail-closed decisions:
//   - decideDossierOpen: serve a fresh cached dossier instantly (no Oracle) vs
//     generate when stale or absent, always carrying any cached text along.
//   - decideDossierResult: fail-closed — keep cached text on an unavailable
//     result; only become "unavailable" when there is genuinely nothing to show.
//
// Pure (no DOM, no Tauri) so they run under the node-environment vitest config.

import { describe, it, expect } from "vitest";
import { decideDossierOpen, decideDossierResult } from "./InspectSidebar";

describe("decideDossierOpen", () => {
  it("serves a fresh cached dossier instantly (no Oracle call)", () => {
    const d = decideDossierOpen({ text: "Narrative.", stale: false });
    expect(d.action).toBe("serveCached");
    expect(d.cached).toBe("Narrative.");
  });

  it("generates when the dossier is stale, carrying the cached text along", () => {
    const d = decideDossierOpen({ text: "Old narrative.", stale: true });
    expect(d.action).toBe("generate");
    expect(d.cached).toBe("Old narrative.");
  });

  it("generates when there is no dossier yet (no cached text)", () => {
    const d = decideDossierOpen({ text: null, stale: true });
    expect(d.action).toBe("generate");
    expect(d.cached).toBeNull();
  });

  it("treats whitespace-only cached text as absent (generates)", () => {
    const d = decideDossierOpen({ text: "   ", stale: false });
    expect(d.action).toBe("generate");
    expect(d.cached).toBeNull();
  });
});

describe("decideDossierResult (fail-closed)", () => {
  it("uses the fresh text when the Oracle answered (available)", () => {
    const s = decideDossierResult({ text: "Fresh.", available: true }, "Old.");
    expect(s).toEqual({ kind: "ok", text: "Fresh." });
  });

  it("keeps the cached text when the result is unavailable (fail-closed)", () => {
    const s = decideDossierResult({ text: null, available: false }, "Old.");
    expect(s).toEqual({ kind: "ok", text: "Old." });
  });

  it("keeps cached text on a thrown/null result (transport failure)", () => {
    const s = decideDossierResult(null, "Old.");
    expect(s).toEqual({ kind: "ok", text: "Old." });
  });

  it("becomes unavailable only when there is nothing to show", () => {
    const s = decideDossierResult({ text: null, available: false }, null);
    expect(s.kind).toBe("unavailable");
    if (s.kind === "unavailable") {
      // Honest default (no longer a bare kind with discarded reason).
      expect(s.message.length).toBeGreaterThan(0);
    }
  });

  it("preserves an explicit failure message when nothing to show", () => {
    const s = decideDossierResult(null, null, "Index is empty. Run Index now.");
    expect(s).toEqual({
      kind: "unavailable",
      message: "Index is empty. Run Index now.",
    });
  });

  it("falls back to a non-empty cached result text even if available=false", () => {
    // The backend returns the cached text with available=false on fail-closed;
    // that text is still shown.
    const s = decideDossierResult({ text: "Cached from backend.", available: false }, null);
    expect(s).toEqual({ kind: "ok", text: "Cached from backend." });
  });
});
