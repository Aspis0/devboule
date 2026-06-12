import { describe, expect, it, vi } from "vitest";
import { CENSOR_FINDINGS_UPDATED_EVENT, type CensorFinding } from "../../types/backend";
import {
  CensorFindingsTracker,
  censorPanelViewState,
  disposeArgs,
  finalReviewLaunchInput,
  groupFindingsByFile,
  openInEditorArgs,
  reviewNowArgs,
  setCensorTrustedArgs,
} from "./censorPanelModel";

function finding(over: Partial<CensorFinding>): CensorFinding {
  return {
    id: "f",
    file: "src/a.ts",
    contentHash: "h",
    line: 1,
    severity: "low",
    category: "style",
    source: "eslint",
    title: "t",
    body: "b",
    verdict: "suspected",
    disposition: "open",
    provenance: [],
    createdAt: "",
    commit: null,
    ...over,
  };
}

describe("groupFindingsByFile", () => {
  it("buckets by file and sorts groups by worst severity then path", () => {
    const groups = groupFindingsByFile([
      finding({ id: "1", file: "src/b.ts", severity: "low" }),
      finding({ id: "2", file: "src/a.ts", severity: "high" }),
      finding({ id: "3", file: "src/a.ts", severity: "low" }),
    ]);
    expect(groups.map((g) => g.file)).toEqual(["src/a.ts", "src/b.ts"]);
    // Within src/a.ts the high finding sorts before the low one.
    expect(groups[0].findings.map((f) => f.id)).toEqual(["2", "3"]);
  });

  it("is total on empty / missing input", () => {
    expect(groupFindingsByFile([])).toEqual([]);
    expect(groupFindingsByFile(undefined as never)).toEqual([]);
  });

  it("sorts same-severity findings by line", () => {
    const groups = groupFindingsByFile([
      finding({ id: "late", line: 99, severity: "high" }),
      finding({ id: "early", line: 3, severity: "high" }),
    ]);
    expect(groups[0].findings.map((f) => f.id)).toEqual(["early", "late"]);
  });
});

describe("command-arg builders", () => {
  it("builds censor_dispose_finding args", () => {
    expect(
      disposeArgs({ projectId: "p", root: "/r", file: "src/a.ts", id: "f", disposition: "fp" }),
    ).toEqual({ projectId: "p", root: "/r", file: "src/a.ts", id: "f", disposition: "fp" });
  });

  it("builds censor_open_in_editor args (forwards project id + rel path + editor)", () => {
    expect(
      openInEditorArgs({ projectId: "p", root: "/r", file: "src/a.ts", editor: "vscode" }),
    ).toEqual({
      projectId: "p",
      root: "/r",
      file: "src/a.ts",
      editor: "vscode",
    });
  });

  it("builds censor_review_now whole-project args (file:null)", () => {
    expect(reviewNowArgs({ projectId: "p", root: "/r" })).toEqual({
      projectId: "p",
      root: "/r",
      file: null,
    });
  });

  it("builds a verifier launch input for Run final review (censorReview:true)", () => {
    const input = finalReviewLaunchInput("p");
    expect(input.role).toBe("verifier");
    expect(input.host).toBe("app");
    expect(input.projectId).toBe("p");
    // Phase H: the final-review launch carries the Censor residual flag so the
    // backend appends the verifier residual-adjudication addendum.
    expect(input.censorReview).toBe(true);
  });

  it("builds set_censor_trusted args for the trust gate", () => {
    expect(setCensorTrustedArgs({ projectId: "p", trusted: true })).toEqual({
      projectId: "p",
      trusted: true,
    });
    expect(setCensorTrustedArgs({ projectId: "p", trusted: false })).toEqual({
      projectId: "p",
      trusted: false,
    });
  });
});

describe("censorPanelViewState (trust gate decision)", () => {
  it("no-root wins even when a status would say trusted", () => {
    expect(
      censorPanelViewState({ hasRoot: false, status: { trusted: true } }),
    ).toBe("no-root");
  });

  it("is loading while status has not been read (does NOT assume trusted)", () => {
    expect(censorPanelViewState({ hasRoot: true, status: null })).toBe(
      "loading",
    );
    expect(censorPanelViewState({ hasRoot: true, status: undefined })).toBe(
      "loading",
    );
  });

  it("shows the untrusted gate when trusted === false", () => {
    expect(
      censorPanelViewState({ hasRoot: true, status: { trusted: false } }),
    ).toBe("untrusted");
  });

  it("shows the normal findings UI only when trusted === true", () => {
    expect(
      censorPanelViewState({ hasRoot: true, status: { trusted: true } }),
    ).toBe("findings");
  });
});

describe("CensorFindingsTracker", () => {
  it("fetches on start and refetches on a findings-updated event", async () => {
    let captured: ((e: { payload: unknown }) => void) | null = null;
    const invoke = vi
      .fn()
      .mockResolvedValueOnce([finding({ id: "1" })])
      .mockResolvedValueOnce([finding({ id: "1" }), finding({ id: "2" })]);
    const unlisten = vi.fn();
    const listen = vi.fn(async (_channel: string, handler: (e: { payload: unknown }) => void) => {
      captured = handler;
      return unlisten;
    });
    const onChange = vi.fn();

    const tracker = new CensorFindingsTracker({
      projectId: "p",
      root: "/r",
      invoke,
      listen,
      onChange,
    });
    await tracker.start();

    expect(invoke).toHaveBeenCalledWith("censor_get_findings", { root: "/r" });
    expect(listen).toHaveBeenCalledWith(CENSOR_FINDINGS_UPDATED_EVENT, expect.any(Function));
    expect(onChange).toHaveBeenLastCalledWith([finding({ id: "1" })]);

    // Simulate a findings-updated event for THIS project → one refetch.
    captured!({ payload: { projectId: "p", files: ["src/a.ts"] } });
    await Promise.resolve();
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(onChange).toHaveBeenLastCalledWith([finding({ id: "1" }), finding({ id: "2" })]);

    tracker.stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("ignores an event for a DIFFERENT project (no extra fetch)", async () => {
    let captured: ((e: { payload: unknown }) => void) | null = null;
    const invoke = vi.fn().mockResolvedValue([finding({ id: "1" })]);
    const listen = vi.fn(async (_c: string, handler: (e: { payload: unknown }) => void) => {
      captured = handler;
      return vi.fn();
    });
    const tracker = new CensorFindingsTracker({
      projectId: "p",
      root: "/r",
      invoke,
      listen,
      onChange: vi.fn(),
    });
    await tracker.start();
    expect(invoke).toHaveBeenCalledTimes(1);

    captured!({ payload: { projectId: "other", files: [] } });
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledTimes(1); // unchanged
    tracker.stop();
  });

  it("ignores a malformed event with no/empty projectId (WARNING 5: no spurious refetch)", async () => {
    let captured: ((e: { payload: unknown }) => void) | null = null;
    const invoke = vi.fn().mockResolvedValue([finding({ id: "1" })]);
    const listen = vi.fn(async (_c: string, handler: (e: { payload: unknown }) => void) => {
      captured = handler;
      return vi.fn();
    });
    const tracker = new CensorFindingsTracker({
      projectId: "p",
      root: "/r",
      invoke,
      listen,
      onChange: vi.fn(),
    });
    await tracker.start();
    expect(invoke).toHaveBeenCalledTimes(1);

    // The Rust emitter always sends a projectId; a missing/empty/non-object payload
    // is malformed and must be SKIPPED, not treated as "refetch everything".
    captured!({ payload: { files: [] } }); // no projectId
    captured!({ payload: { projectId: "", files: [] } }); // empty projectId
    captured!({ payload: undefined }); // no payload
    captured!({ payload: "garbage" }); // non-object payload
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledTimes(1); // still only the initial fetch
    tracker.stop();
  });

  it("cleans up the listener on stop and drops late callbacks", async () => {
    let captured: ((e: { payload: unknown }) => void) | null = null;
    const invoke = vi.fn().mockResolvedValue([finding({ id: "1" })]);
    const unlisten = vi.fn();
    const listen = vi.fn(async (_c: string, handler: (e: { payload: unknown }) => void) => {
      captured = handler;
      return unlisten;
    });
    const onChange = vi.fn();
    const tracker = new CensorFindingsTracker({
      projectId: "p",
      root: "/r",
      invoke,
      listen,
      onChange,
    });
    await tracker.start();
    tracker.stop();
    expect(unlisten).toHaveBeenCalledTimes(1);

    // A late event after stop() must NOT trigger a refetch (epoch guard).
    onChange.mockClear();
    invoke.mockClear();
    captured!({ payload: { projectId: "p", files: [] } });
    await Promise.resolve();
    expect(invoke).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("surfaces a fetch error via onError without throwing", async () => {
    const invoke = vi.fn().mockRejectedValue(new Error("boom"));
    const onError = vi.fn();
    const tracker = new CensorFindingsTracker({
      projectId: "p",
      root: "/r",
      invoke,
      listen: async () => vi.fn(),
      onChange: vi.fn(),
      onError,
    });
    await tracker.start();
    expect(onError).toHaveBeenCalledWith("boom");
    tracker.stop();
  });
});
