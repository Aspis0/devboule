// Table-driven regressions for honest Oracle panel copy (step 6).
//
// Pins behaviour, not exact marketing wording:
//   - each distinct OracleError kind maps to its own message
//   - remediation is reused when the backend provides one
//   - empty / notFound / browser are distinct from typed errors
//   - while index status reports a running/queued job, the panel reports
//     indexing (and shouldDeferOracleAsk is true) rather than a generic failure

import { describe, it, expect } from "vitest";
import type { OracleError, OracleErrorKind, OracleIndexStatus } from "../../types/backend";
import {
  browserOracleMessage,
  emptyOracleResultMessage,
  indexingUnavailableMessage,
  isOracleIndexJobActive,
  oracleFailureMessage,
  oracleUnavailableMessage,
  shouldDeferOracleAsk,
} from "./oraclePanelMessages";

const KINDS: OracleErrorKind[] = [
  "noWorkspaceRoot",
  "serverUnavailable",
  "pythonError",
  "embedderUnavailable",
  "indexEmpty",
  "missingApiKey",
  "internal",
];

function err(
  kind: OracleErrorKind,
  message: string,
  remediation: string,
): OracleError {
  return { kind, message, remediation };
}

function indexingStatus(
  jobStatus: "queued" | "running" | "idle" | null,
  indexed = 3,
  expected = 10,
): OracleIndexStatus {
  return {
    job:
      jobStatus === null
        ? null
        : {
            jobId: "j1",
            status: jobStatus,
            startedAt: "2026-07-10T00:00:00Z",
          },
    watcherRunning: true,
    index: {
      root: "/work",
      expectedFiles: expected,
      indexedFiles: indexed,
      pendingFiles: Math.max(0, expected - indexed),
      staleFiles: 0,
      sqliteChunkFiles: 0,
      sqliteChunks: 0,
      vectorRecords: 0,
      firstPending: [],
      firstStale: [],
      freeRamGb: 8,
    },
  };
}

describe("isOracleIndexJobActive / shouldDeferOracleAsk", () => {
  it("is true for queued and running jobs", () => {
    expect(isOracleIndexJobActive(indexingStatus("queued"))).toBe(true);
    expect(isOracleIndexJobActive(indexingStatus("running"))).toBe(true);
    expect(shouldDeferOracleAsk(indexingStatus("running"))).toBe(true);
  });

  it("is false when idle, missing job, or null status", () => {
    expect(isOracleIndexJobActive(indexingStatus("idle"))).toBe(false);
    expect(isOracleIndexJobActive(indexingStatus(null))).toBe(false);
    expect(isOracleIndexJobActive(null)).toBe(false);
    expect(shouldDeferOracleAsk(null)).toBe(false);
  });
});

describe("oracleFailureMessage — distinct causes", () => {
  it("maps each OracleError kind to a distinct message (table-driven)", () => {
    const messages = KINDS.map((kind) =>
      oracleFailureMessage(
        err(kind, `message-for-${kind}`, `remediation-for-${kind}`),
        "blurb",
      ),
    );
    expect(new Set(messages).size).toBe(KINDS.length);
    // Each includes the kind-specific backend text (remediation and/or message).
    for (let i = 0; i < KINDS.length; i++) {
      const kind = KINDS[i]!;
      expect(messages[i]).toContain(`message-for-${kind}`);
      expect(messages[i]).toContain(`remediation-for-${kind}`);
    }
  });

  it("reuses remediation when message is empty", () => {
    const m = oracleFailureMessage(
      err("indexEmpty", "", "Run Index in Oracle ▸ Indexing."),
      "blurb",
    );
    expect(m).toContain("Run Index");
  });

  it("falls back to a kind-specific calm line when both fields are blank", () => {
    const byKind = KINDS.map((kind) =>
      oracleFailureMessage(err(kind, "", ""), "blurb"),
    );
    expect(new Set(byKind).size).toBe(KINDS.length);
  });

  it("dossier surface differs from blurb for empty-result defaults", () => {
    expect(emptyOracleResultMessage("blurb", true)).not.toBe(
      emptyOracleResultMessage("dossier", true),
    );
    expect(browserOracleMessage("blurb")).not.toBe(
      browserOracleMessage("dossier"),
    );
  });
});

describe("indexing vs generic failure", () => {
  it("while a running job is reported, surfaces indexing not the typed timeout", () => {
    const status = indexingStatus("running", 4, 20);
    const serverDown = err(
      "serverUnavailable",
      "Oracle server is not responding",
      "Start Oracle and retry.",
    );
    const withIndexing = oracleFailureMessage(serverDown, "blurb", {
      indexing: true,
      indexStatus: status,
    });
    const without = oracleFailureMessage(serverDown, "blurb", {
      indexing: false,
    });
    expect(withIndexing.toLowerCase()).toMatch(/index/);
    expect(withIndexing).toMatch(/4/);
    expect(withIndexing).toMatch(/20/);
    expect(withIndexing).not.toBe(without);
    // Pure indexing cause (no request fired) uses the same family of copy.
    expect(
      indexingUnavailableMessage(status, "blurb").toLowerCase(),
    ).toMatch(/index/);
  });

  it("oracleUnavailableMessage dispatches each cause distinctly", () => {
    const status = indexingStatus("queued", 1, 5);
    const causes = [
      oracleUnavailableMessage({ kind: "browser" }, "blurb"),
      oracleUnavailableMessage({ kind: "indexing", status }, "blurb"),
      oracleUnavailableMessage({ kind: "empty" }, "blurb"),
      oracleUnavailableMessage({ kind: "notFound" }, "blurb"),
      oracleUnavailableMessage(
        {
          kind: "error",
          error: err("missingApiKey", "No API key", "Add a key in Settings."),
        },
        "blurb",
      ),
      oracleUnavailableMessage(
        {
          kind: "error",
          error: err("indexEmpty", "Index empty", "Run Index now."),
        },
        "blurb",
      ),
    ];
    expect(new Set(causes).size).toBe(causes.length);
  });
});
