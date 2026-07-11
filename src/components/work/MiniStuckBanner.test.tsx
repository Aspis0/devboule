// @vitest-environment jsdom

import { describe, it, expect, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { MiniStuckBanner } from "./MiniStuckBanner";
import type { MiniStuckReport } from "./miniStuckModel";

function makeReport(overrides: Partial<MiniStuckReport> = {}): MiniStuckReport {
  return {
    taskId: "T1",
    agentId: "agent-1",
    reason: "timeout",
    attempts: 2,
    lastOutput: "",
    filesTouched: [],
    ...overrides,
  };
}

const html = (props: Parameters<typeof MiniStuckBanner>[0]) =>
  renderToStaticMarkup(createElement(MiniStuckBanner, props));

describe("MiniStuckBanner", () => {
  it("renders nothing when reports is empty", () => {
    const out = html({ reports: [], onDismiss: vi.fn() });
    expect(out).toBe("");
  });

  it("renders one row per report with agent id / reason / attempts", () => {
    const out = html({
      reports: [
        makeReport({ agentId: "coder-abc", reason: "timeout", attempts: 3 }),
        makeReport({ taskId: "T2", agentId: "coder-def", reason: "failed", attempts: 1 }),
      ],
      onDismiss: vi.fn(),
    });
    expect(out).toContain("coder-abc");
    expect(out).toContain("timed out");
    expect(out).toContain("3 attempt(s)");
    expect(out).toContain("coder-def");
    expect(out).toContain("failed");
    expect(out).toContain("1 attempt(s)");
  });

  it("dismiss buttons have the correct data-testid per report", () => {
    const out = html({
      reports: [
        makeReport({ taskId: "T-alpha" }),
        makeReport({ taskId: "T-beta", agentId: "agent-2" }),
      ],
      onDismiss: vi.fn(),
    });
    expect(out).toContain('data-testid="mini-stuck-dismiss-T-alpha"');
    expect(out).toContain('data-testid="mini-stuck-dismiss-T-beta"');
  });

  it("has the outer wrapper data-testid", () => {
    const out = html({
      reports: [makeReport()],
      onDismiss: vi.fn(),
    });
    expect(out).toContain('data-testid="mini-stuck-banner"');
  });
});
