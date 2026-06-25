import { describe, expect, it } from "vitest";
import {
  enqueueConsent,
  grantNetConsentArgs,
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

  it("ignores kind when comparing identity", () => {
    const a = makeRequest({ kind: "net" });
    const b = makeRequest({ kind: "exec" });
    // same projectId + agentId → same identity regardless of kind
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

  it("deduplicates: a duplicate event for the same (projectId, agentId) is ignored", () => {
    const req = makeRequest({ agentId: "agent-A", projectId: "proj-1" });
    const dup = makeRequest({ agentId: "agent-A", projectId: "proj-1" });
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
