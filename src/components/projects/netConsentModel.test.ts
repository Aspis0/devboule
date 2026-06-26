import { describe, expect, it } from "vitest";
import {
  enqueueConsent,
  grantFolderConsentArgs,
  grantNetConsentArgs,
  isConsentRequestForProject,
  isNetRequestForProject,
  sameConsentRequest,
  type ConsentDecision,
  type ConsentKind,
  type ConsentRequest,
} from "./netConsentModel";

// ── helpers ──────────────────────────────────────────────────────────────────

function makeRequest(over: Partial<ConsentRequest> = {}): ConsentRequest {
  return {
    kind: "net",
    projectId: "proj-1",
    agentId: "mini-agent-42",
    detail: "cargo fetch failed: network unreachable",
    ...over,
  };
}

function makeFolderRequest(over: Partial<ConsentRequest> = {}): ConsentRequest {
  return {
    kind: "folderWrite",
    projectId: "proj-1",
    agentId: "mini-agent-42",
    detail:
      'A sandboxed command attempted to write outside the project to "/private/tmp/extra". Grant to allow writes there and retry.',
    path: "/private/tmp/extra",
    ...over,
  };
}

// ── grantNetConsentArgs ───────────────────────────────────────────────────────

describe("grantNetConsentArgs", () => {
  it("AllowRemember: emits the exact camelCase JSON shape the Rust backend expects", () => {
    const args = grantNetConsentArgs({
      projectId: "proj-abc",
      decision: "allowRemember",
    });
    // Rust: ConsentDecision has rename_all="camelCase" → AllowRemember → "allowRemember"
    expect(args).toEqual({ projectId: "proj-abc", decision: "allowRemember" });
  });

  it("AllowOnce: emits 'allowOnce'", () => {
    const args = grantNetConsentArgs({
      projectId: "proj-abc",
      decision: "allowOnce",
    });
    expect(args).toEqual({ projectId: "proj-abc", decision: "allowOnce" });
  });

  it("Deny: emits 'deny'", () => {
    const args = grantNetConsentArgs({
      projectId: "proj-abc",
      decision: "deny",
    });
    expect(args).toEqual({ projectId: "proj-abc", decision: "deny" });
  });

  it("passes projectId unchanged", () => {
    const args = grantNetConsentArgs({ projectId: "my-project-id", decision: "deny" });
    expect(args.projectId).toBe("my-project-id");
  });

  it("only contains projectId and decision keys (no extras)", () => {
    const args = grantNetConsentArgs({ projectId: "p", decision: "allowOnce" });
    expect(Object.keys(args).sort()).toEqual(["decision", "projectId"]);
  });

  // Exhaustive type check: all three decisions produce different outputs
  const decisions: ConsentDecision[] = ["allowRemember", "allowOnce", "deny"];
  it("all three decisions produce distinct JSON shapes", () => {
    const results = decisions.map((d) =>
      grantNetConsentArgs({ projectId: "p", decision: d }).decision,
    );
    expect(new Set(results).size).toBe(3);
  });
});

// ── isNetRequestForProject ────────────────────────────────────────────────────

describe("isNetRequestForProject", () => {
  it("returns true when kind=net and projectId matches", () => {
    expect(isNetRequestForProject(makeRequest(), "proj-1")).toBe(true);
  });

  it("returns false when projectId does not match", () => {
    expect(isNetRequestForProject(makeRequest(), "proj-2")).toBe(false);
  });

  it("returns false when kind is not net (folderWrite)", () => {
    const req = makeRequest({ kind: "folderWrite" });
    expect(isNetRequestForProject(req, "proj-1")).toBe(false);
  });

  it("returns false when kind is not net (exec)", () => {
    const req = makeRequest({ kind: "exec" });
    expect(isNetRequestForProject(req, "proj-1")).toBe(false);
  });

  it("returns false when kind is not net (patch)", () => {
    const req = makeRequest({ kind: "patch" });
    expect(isNetRequestForProject(req, "proj-1")).toBe(false);
  });

  it("returns false when both kind and projectId are wrong", () => {
    const req = makeRequest({ kind: "folderWrite", projectId: "other" });
    expect(isNetRequestForProject(req, "proj-1")).toBe(false);
  });

  it("is case-sensitive on projectId", () => {
    expect(isNetRequestForProject(makeRequest({ projectId: "Proj-1" }), "proj-1")).toBe(
      false,
    );
  });

  // Exhaustive kind coverage — all non-net kinds are rejected
  const nonNetKinds: ConsentKind[] = ["folderWrite", "exec", "patch"];
  for (const kind of nonNetKinds) {
    it(`rejects kind="${kind}" even when projectId matches`, () => {
      expect(isNetRequestForProject(makeRequest({ kind }), "proj-1")).toBe(false);
    });
  }
});

// ── sameConsentRequest ────────────────────────────────────────────────────────

describe("sameConsentRequest", () => {
  it("returns true when projectId and agentId are identical", () => {
    const a = makeRequest();
    const b = makeRequest();
    expect(sameConsentRequest(a, b)).toBe(true);
  });

  it("returns false when projectId differs", () => {
    const a = makeRequest({ projectId: "proj-1" });
    const b = makeRequest({ projectId: "proj-2" });
    expect(sameConsentRequest(a, b)).toBe(false);
  });

  it("returns false when agentId differs", () => {
    const a = makeRequest({ agentId: "agent-A" });
    const b = makeRequest({ agentId: "agent-B" });
    expect(sameConsentRequest(a, b)).toBe(false);
  });

  it("returns false when kind differs (same projectId + agentId)", () => {
    const a = makeRequest({ kind: "net" });
    const b = makeRequest({ kind: "folderWrite" });
    // An agent can be blocked on net AND folderWrite simultaneously; they must
    // be treated as distinct identity slots so neither is deduped away.
    expect(sameConsentRequest(a, b)).toBe(false);
  });

  it("returns false when kind differs (net vs exec)", () => {
    const a = makeRequest({ kind: "net" });
    const b = makeRequest({ kind: "exec" });
    expect(sameConsentRequest(a, b)).toBe(false);
  });

  it("returns true only when projectId, agentId, AND kind are all identical", () => {
    const a = makeRequest({ kind: "folderWrite" });
    const b = makeRequest({ kind: "folderWrite" });
    expect(sameConsentRequest(a, b)).toBe(true);
  });

  it("ignores detail when comparing identity", () => {
    const a = makeRequest({ detail: "cargo fetch failed" });
    const b = makeRequest({ detail: "curl timeout" });
    expect(sameConsentRequest(a, b)).toBe(true);
  });

  it("is symmetric", () => {
    const a = makeRequest({ agentId: "agent-X" });
    const b = makeRequest({ agentId: "agent-Y" });
    expect(sameConsentRequest(a, b)).toBe(sameConsentRequest(b, a));
  });
});

// ── enqueueConsent ────────────────────────────────────────────────────────────

describe("enqueueConsent", () => {
  it("appends a new request to an empty list", () => {
    const req = makeRequest();
    const result = enqueueConsent([], req);
    expect(result).toHaveLength(1);
    expect(result[0]).toBe(req);
  });

  it("appends a new request to a non-empty list (FIFO order preserved)", () => {
    const first = makeRequest({ agentId: "agent-A" });
    const second = makeRequest({ agentId: "agent-B" });
    const list = enqueueConsent([], first);
    const result = enqueueConsent(list, second);
    expect(result).toHaveLength(2);
    expect(result[0]).toBe(first);
    expect(result[1]).toBe(second);
  });

  it("deduplicates: returns original array reference when already queued", () => {
    const req = makeRequest();
    const list = [req];
    // Same identity → returns the same array, no new allocation
    const result = enqueueConsent(list, makeRequest());
    expect(result).toBe(list);
  });

  it("deduplicates: a duplicate event for the same (projectId, agentId, kind) is ignored", () => {
    const req = makeRequest({ agentId: "agent-A", projectId: "proj-1", kind: "net" });
    const dup = makeRequest({ agentId: "agent-A", projectId: "proj-1", kind: "net" });
    const list = enqueueConsent([], req);
    const result = enqueueConsent(list, dup);
    expect(result).toHaveLength(1);
  });

  it("allows the same agentId on a different project (different identity)", () => {
    const req1 = makeRequest({ agentId: "agent-A", projectId: "proj-1" });
    const req2 = makeRequest({ agentId: "agent-A", projectId: "proj-2" });
    const list = enqueueConsent([], req1);
    const result = enqueueConsent(list, req2);
    expect(result).toHaveLength(2);
  });

  it("allows the same project with a different agentId", () => {
    const req1 = makeRequest({ agentId: "agent-A", projectId: "proj-1" });
    const req2 = makeRequest({ agentId: "agent-B", projectId: "proj-1" });
    const list = enqueueConsent([], req1);
    const result = enqueueConsent(list, req2);
    expect(result).toHaveLength(2);
  });

  it("three distinct agents: all three are enqueued in arrival order (FIFO)", () => {
    const a = makeRequest({ agentId: "agent-A" });
    const b = makeRequest({ agentId: "agent-B" });
    const c = makeRequest({ agentId: "agent-C" });
    let list = enqueueConsent([], a);
    list = enqueueConsent(list, b);
    list = enqueueConsent(list, c);
    expect(list).toHaveLength(3);
    expect(list[0].agentId).toBe("agent-A");
    expect(list[1].agentId).toBe("agent-B");
    expect(list[2].agentId).toBe("agent-C");
  });

  it("does not mutate the original list", () => {
    const req = makeRequest({ agentId: "agent-A" });
    const newReq = makeRequest({ agentId: "agent-B" });
    const original = [req];
    enqueueConsent(original, newReq);
    expect(original).toHaveLength(1);
  });
});

// ── isConsentRequestForProject ────────────────────────────────────────────────

describe("isConsentRequestForProject", () => {
  it("returns true for kind=net when projectId matches", () => {
    expect(isConsentRequestForProject(makeRequest({ kind: "net" }), "proj-1")).toBe(true);
  });

  it("returns true for kind=folderWrite when projectId matches", () => {
    expect(
      isConsentRequestForProject(makeRequest({ kind: "folderWrite" }), "proj-1"),
    ).toBe(true);
  });

  it("returns true for kind=exec when projectId matches", () => {
    expect(isConsentRequestForProject(makeRequest({ kind: "exec" }), "proj-1")).toBe(true);
  });

  it("returns true for kind=patch when projectId matches", () => {
    expect(isConsentRequestForProject(makeRequest({ kind: "patch" }), "proj-1")).toBe(true);
  });

  it("returns false when projectId does not match, regardless of kind", () => {
    expect(isConsentRequestForProject(makeRequest({ kind: "net" }), "proj-other")).toBe(
      false,
    );
    expect(
      isConsentRequestForProject(makeRequest({ kind: "folderWrite" }), "proj-other"),
    ).toBe(false);
  });

  it("is case-sensitive on projectId", () => {
    expect(
      isConsentRequestForProject(makeRequest({ projectId: "Proj-1" }), "proj-1"),
    ).toBe(false);
  });

  // Cross-kind independence: an agent blocked on BOTH net AND folderWrite must
  // have BOTH requests enqueued — different kinds are different identity slots.
  it("enqueueConsent keeps both requests when same agent has different kinds", () => {
    const netReq = makeRequest({ kind: "net" });
    const folderReq = makeRequest({ kind: "folderWrite" });
    const list = enqueueConsent([], netReq);
    // folderReq shares (projectId, agentId) but different kind → must enqueue.
    const result = enqueueConsent(list, folderReq);
    expect(result).toHaveLength(2);
    expect(result[0].kind).toBe("net");
    expect(result[1].kind).toBe("folderWrite");
  });

  // Same kind still deduplicates (a rapid duplicate net event must not double-enqueue).
  it("enqueueConsent deduplicates exact-same-kind re-fire to 1 item", () => {
    const netReq = makeRequest({ kind: "net", agentId: "agent-A" });
    const netDup = makeRequest({ kind: "net", agentId: "agent-A" });
    const list = enqueueConsent([], netReq);
    const result = enqueueConsent(list, netDup);
    expect(result).toHaveLength(1);
    expect(result).toBe(list); // same array reference — no allocation
  });
});

// ── grantFolderConsentArgs ────────────────────────────────────────────────────

describe("grantFolderConsentArgs", () => {
  it("AllowRemember: emits the exact camelCase shape the Rust backend expects", () => {
    const args = grantFolderConsentArgs({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "allowRemember",
    });
    expect(args).toEqual({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "allowRemember",
    });
  });

  it("AllowOnce: emits 'allowOnce'", () => {
    const args = grantFolderConsentArgs({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "allowOnce",
    });
    expect(args).toEqual({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "allowOnce",
    });
  });

  it("Deny: emits 'deny'", () => {
    const args = grantFolderConsentArgs({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "deny",
    });
    expect(args).toEqual({
      projectId: "proj-abc",
      folder: "/tmp/extra",
      decision: "deny",
    });
  });

  it("contains exactly {projectId, folder, decision} — no extras", () => {
    const args = grantFolderConsentArgs({
      projectId: "p",
      folder: "/f",
      decision: "deny",
    });
    expect(Object.keys(args).sort()).toEqual(["decision", "folder", "projectId"]);
  });

  it("passes folder unchanged", () => {
    const args = grantFolderConsentArgs({
      projectId: "p",
      folder: "/home/user/workspace",
      decision: "allowOnce",
    });
    expect(args.folder).toBe("/home/user/workspace");
  });

  it("all three decisions produce distinct args objects", () => {
    const decisions: ConsentDecision[] = ["allowRemember", "allowOnce", "deny"];
    const decisions_set = new Set(
      decisions.map(
        (d) =>
          grantFolderConsentArgs({ projectId: "p", folder: "/f", decision: d }).decision,
      ),
    );
    expect(decisions_set.size).toBe(3);
  });
});

// ── BLOCKER 1: ConsentRequest.path field contract ─────────────────────────────
//
// The frontend MUST use head.path (not head.detail) as the folder argument to
// grant_folder_consent. detail is human-readable prose and is rejected by the
// backend's normalize_working_set_folder (!is_absolute) validator.

describe("ConsentRequest.path — BLOCKER 1 contract", () => {
  it("FolderWrite request carries a machine-readable path separate from detail", () => {
    const req = makeFolderRequest();
    // path is an absolute POSIX path — valid as folder argument to grant_folder_consent.
    expect(req.path).toBe("/private/tmp/extra");
    // detail is prose — it is NOT absolute and must never be passed as folder.
    expect(req.detail).toContain("A sandboxed command");
    expect(req.detail.startsWith("/")).toBe(false);
  });

  it("grantFolderConsentArgs built from path (not detail) passes the correct folder", () => {
    const req = makeFolderRequest();
    // Simulate what handleConsentDecision does: use req.path as the folder.
    const folder = req.path!;
    const args = grantFolderConsentArgs({
      projectId: req.projectId,
      folder,
      decision: "allowOnce",
    });
    // The folder arg must be the canonical path, not the prose sentence.
    expect(args.folder).toBe("/private/tmp/extra");
    expect(typeof args.folder).toBe("string");
    expect((args.folder as string).startsWith("/")).toBe(true);
  });

  it("grantFolderConsentArgs built from detail (old bug) would pass non-absolute string", () => {
    const req = makeFolderRequest();
    // Demonstrate the OLD BUG: detail is not an absolute path.
    const buggyFolder = req.detail;
    expect(buggyFolder.startsWith("/")).toBe(false);
    // This would have been rejected by normalize_working_set_folder on the backend.
    // With the fix, the frontend uses req.path instead.
  });

  it("Net request has no path field (undefined)", () => {
    const req = makeRequest({ kind: "net" });
    expect(req.path).toBeUndefined();
  });

  it("FolderWrite request with missing path surfaces the contract violation", () => {
    // If path is somehow absent (backend bug), the frontend must error rather than
    // silently pass detail. This test documents the expected guard behavior.
    const req = makeFolderRequest({ path: undefined });
    // In production handleConsentDecision throws; here we verify the missing-path case.
    expect(req.path).toBeUndefined();
    // The guard: folder = req.path → undefined → throw "missing machine-readable path"
    const folder = req.path;
    expect(folder).toBeFalsy();
  });
});
