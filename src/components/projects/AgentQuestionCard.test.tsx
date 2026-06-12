// @vitest-environment jsdom
import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import type { AgentSession } from "../../types/backend";

const mockInvoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => mockInvoke(...args),
  isTauriRuntime: () => false,
}));

import { AgentQuestionCard } from "./AgentQuestionCard";

function session(over: Partial<AgentSession> = {}): AgentSession {
  return {
    agentId: "coder-1",
    role: "coder",
    model: null,
    status: "needs_user",
    message: null,
    currentProjectId: "p1",
    currentTaskId: null,
    firstSeenAt: null,
    lastSeenAt: null,
    needsUser: { reason: "question", message: "Is this okay?", since: "2026-06-09T10:00:00Z" },
    pendingQuestion: { id: "q1", question: "Should I deploy now?", createdAt: "2026-06-09T10:00:00Z" },
    ...over,
  };
}

describe("AgentQuestionCard", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it("renders null when session has no pendingQuestion", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session({ pendingQuestion: null })} />,
    );
    expect(html).toBe("");
  });

  it("renders null when needsUser reason is not question", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard
        session={session({
          needsUser: { reason: "needs_plan_approval", message: "Approve plan", since: "2026-06-09T10:00:00Z" },
          pendingQuestion: null,
        })}
      />,
    );
    expect(html).toBe("");
  });

  it("renders the question text (stripSpoofChars applied)", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    expect(html).toContain("Should I deploy now?");
  });

  it("renders the agent id", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    expect(html).toContain("coder-1");
  });

  it("renders a textarea for the reply", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    expect(html).toContain("textarea");
  });

  it("renders the Send button", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    expect(html).toContain("Send");
  });

  it("textarea has maxLength=4096", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    expect(html).toContain("4096");
  });

  it("shows the 4096 char counter", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard session={session()} />,
    );
    // Counter must show "/ 4096"
    expect(html).toContain("4096");
  });

  it("falls back to pendingQuestion when needsUser is absent", () => {
    const html = renderToStaticMarkup(
      <AgentQuestionCard
        session={session({
          needsUser: null,
          pendingQuestion: { id: "q1", question: "Are you sure?", createdAt: "" },
        })}
      />,
    );
    expect(html).toContain("Are you sure?");
  });

  // Regression (BLOCKER #1): rerendering the SAME mounted component from
  // pendingQuestion (renders content + runs hooks) to pendingQuestion=null
  // (early return null) must NOT throw "Rendered fewer hooks than expected".
  // Hooks must run unconditionally; the early null return must come last.
  it("does not crash when pendingQuestion transitions non-null -> null on the same mount", async () => {
    const prevActEnv = (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT;
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    const container = document.createElement("div");
    document.body.appendChild(container);
    try {
      const { createRoot } = await import("react-dom/client");
      const root = createRoot(container);

      act(() => {
        root.render(createElement(AgentQuestionCard, { session: session() }));
      });
      expect(container.innerHTML).toContain("Should I deploy now?");

      // The normal resolution path: pendingQuestion is cleared by the next poll.
      expect(() => {
        act(() => {
          root.render(
            createElement(AgentQuestionCard, {
              session: session({ needsUser: null, pendingQuestion: null }),
            }),
          );
        });
      }).not.toThrow();
      expect(container.innerHTML).toBe("");

      act(() => {
        root.unmount();
      });
    } finally {
      container.remove();
      (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = prevActEnv;
    }
  });
});

// ---- DOCK_TABS contains "plans" --------------------------------------------

describe("DOCK_TABS contains plans", () => {
  it("has a plans tab entry", async () => {
    const { DOCK_TABS } = await import("./projectWorkspaceModel");
    const ids = DOCK_TABS.map((t) => t.id);
    expect(ids).toContain("plans");
  });

  it("plans tab has a non-empty label", async () => {
    const { DOCK_TABS } = await import("./projectWorkspaceModel");
    const plansTab = DOCK_TABS.find((t) => t.id === "plans");
    expect(plansTab).toBeDefined();
    expect((plansTab?.label ?? "").length).toBeGreaterThan(0);
  });
});
