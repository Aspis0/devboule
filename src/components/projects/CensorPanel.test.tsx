import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { CensorPanelView } from "./CensorPanel";
import { finalReviewLaunchInput } from "./censorPanelModel";
import type { CensorFinding, CensorStatus } from "../../types/backend";

function finding(over: Partial<CensorFinding> = {}): CensorFinding {
  return {
    id: "f1",
    file: "src/app.ts",
    contentHash: "h",
    line: 12,
    severity: "high",
    category: "security",
    source: "gitleaks",
    title: "Hardcoded secret",
    body: "redacted summary",
    verdict: "suspected",
    disposition: "open",
    provenance: [],
    createdAt: "",
    commit: null,
    ...over,
  };
}

const noop = () => undefined;
const baseProps = {
  status: null as CensorStatus | null,
  loadError: null as string | null,
  hasRoot: true,
  actionBusy: false,
  launchBusy: false,
  canLaunch: true,
  onOpen: noop,
  onDispose: noop,
  onReviewNow: noop,
  onRunFinalReview: noop,
  onTrust: noop,
  onDisable: noop,
};

describe("CensorPanelView states", () => {
  it("renders the findings list grouped by file with the row badges", () => {
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[finding()]} />,
    );
    expect(html).toContain("src/app.ts"); // group header + file:line
    expect(html).toContain("Hardcoded secret"); // finding title
    expect(html).toContain("Security"); // category badge from the row
    expect(html).toContain("1 open finding");
  });

  it("renders the empty state when there are no findings", () => {
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[]} />,
    );
    expect(html).toContain("No open findings");
    expect(html).toContain("0 open findings");
  });

  it("renders the no-root state when the project has no working root", () => {
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[]} hasRoot={false} />,
    );
    expect(html).toContain("no working root configured");
  });

  it("shows the Gemma-offline banner when censor_status reports offline", () => {
    const status: CensorStatus = { gemmaStatus: "offline", tools: [], trusted: true };
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[]} status={status} />,
    );
    expect(html).toContain("Gemma layer offline");
  });

  it("does NOT show the Gemma banner when available or unknown", () => {
    for (const gemmaStatus of ["available", "unknown"] as const) {
      const html = renderToStaticMarkup(
        <CensorPanelView {...baseProps} findings={[]} status={{ gemmaStatus, tools: [], trusted: true }} />,
      );
      expect(html).not.toContain("Gemma layer offline");
    }
  });

  it("lists absent tools as a skipped-layers hint", () => {
    const status: CensorStatus = {
      gemmaStatus: "available",
      tools: [
        { name: "eslint", available: true },
        { name: "gitleaks", available: false },
      ],
      trusted: true,
    };
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[]} status={status} />,
    );
    expect(html).toContain("gitleaks");
    expect(html).toContain("layers are skipped");
    // absent tool → a red chip with the data attribute; an installed tool → no chip
    expect(html).toContain('data-censor-missing-tool="gitleaks"');
    expect(html).not.toContain('data-censor-missing-tool="eslint"');
  });

  it("exposes Review now and Run final review actions", () => {
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[]} />,
    );
    expect(html).toContain("Review now");
    expect(html).toContain("Run final review");
  });

  it("renders the TRUST GATE (not findings) when the project is untrusted", () => {
    const status: CensorStatus = {
      gemmaStatus: "available",
      tools: [],
      trusted: false,
    };
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[finding()]} status={status} />,
    );
    // The gate copy + the enable button.
    expect(html).toContain("Censor runs this project");
    expect(html).toContain("Only enable for repos you trust");
    expect(html).toContain("Trust");
    expect(html).toContain("enable Censor");
    // And it must NOT leak the findings/actions UI while untrusted.
    expect(html).not.toContain("Hardcoded secret");
    expect(html).not.toContain("Run final review");
    expect(html).not.toContain("No open findings");
  });

  it("renders the normal findings UI (not the gate) once trusted", () => {
    const status: CensorStatus = {
      gemmaStatus: "available",
      tools: [],
      trusted: true,
    };
    const html = renderToStaticMarkup(
      <CensorPanelView {...baseProps} findings={[finding()]} status={status} />,
    );
    expect(html).toContain("Hardcoded secret");
    expect(html).toContain("Run final review");
    expect(html).not.toContain("Only enable for repos you trust");
  });

  it("the trust gate surfaces a load error (e.g. enable failed)", () => {
    const status: CensorStatus = {
      gemmaStatus: "available",
      tools: [],
      trusted: false,
    };
    const html = renderToStaticMarkup(
      <CensorPanelView
        {...baseProps}
        findings={[]}
        status={status}
        loadError="Could not enable Censor for this project."
      />,
    );
    expect(html).toContain("Could not enable Censor for this project.");
  });
});

describe("Run final review launch input", () => {
  it("the handler is given a verifier app-launch input scoped to the project", () => {
    // The panel's onRunFinalReview calls onLaunch(finalReviewLaunchInput(projectId)).
    // We assert the exact input shape the launch path receives.
    const onLaunch = vi.fn();
    const projectId = "scrna-seq";
    // Simulate the panel handler body (isBusy=false, canLaunch=true).
    onLaunch(finalReviewLaunchInput(projectId));
    expect(onLaunch).toHaveBeenCalledWith({
      projectId: "scrna-seq",
      role: "verifier",
      client: "claude",
      taskId: null,
      host: "app",
      model: null,
      // Phase H: the final-review launch carries the Censor residual flag so the
      // backend appends the verifier residual-adjudication addendum.
      censorReview: true,
    });
  });
});
