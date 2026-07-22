// @vitest-environment jsdom
//
// AgentConsole render + interaction tests. Renders the five mock states and asserts
// the load-bearing elements appear; drives the expand/collapse interaction on a
// real DOM (createRoot + act + click); and mounts useAgentConsole to verify it
// degrades gracefully to the empty state when no backend exists. Mirrors the repo's
// renderToStaticMarkup + createRoot/act idiom (PlanApprovalCard / MiniWriteBehaviorCard).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { AgentConsole } from "./AgentConsole";
import { useAgentConsole, type AgentConsoleDeps } from "./useAgentConsole";
import type { ConsoleActivity } from "./agentConsoleModel";
import {
  STATE_CLEAN,
  STATE_DIRTY,
  STATE_EMPTY,
  STATE_ESCALATED,
  STATE_RUNNING,
} from "./__fixtures__/agentConsoleStates";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function markup(activity: ConsoleActivity, agentRole?: string | null): string {
  return renderToStaticMarkup(
    createElement(AgentConsole, { activity, agentRole }),
  );
}

describe("AgentConsole — A: Running", () => {
  it("renders the coder chip, mini card, round marker, and working shimmer", () => {
    const html = markup(STATE_RUNNING, "coder");
    expect(html).toContain("Coder");
    expect(html).toContain("Mini");
    expect(html).toContain("mini · sonnet-4");
    expect(html).toContain("Round 1");
    expect(html).toContain("working — compiling edits…");
    // scope chips
    expect(html).toContain("auth.rs");
    expect(html).toContain("login.ts");
    // a running action shows the neutral running pill
    expect(html).toContain("running");
    // the emit-edits pill
    expect(html).toContain("emit-edits");
    // a coder milestone with a <span class="mono"> marker → mono text, NOT raw HTML
    expect(html).toContain("T-12 · harden auth flow");
    expect(html).not.toContain('<span class="mono">');
  });

  it("labels tool rows with the session role (not always Coder)", () => {
    expect(markup(STATE_RUNNING, "orchestrator")).toContain("Orchestrator");
    expect(markup(STATE_RUNNING, "verifier")).toContain("Verifier");
    expect(markup(STATE_RUNNING, "mini")).toContain("Mini");
    // Unknown / missing role → generic Agent (never a false "Coder").
    expect(markup(STATE_RUNNING)).toContain("Agent");
    expect(markup(STATE_RUNNING)).not.toContain("Coder");
  });
});

describe("AgentConsole — B: Dirty → fix", () => {
  it("renders a DIRTY verdict with severity-coded findings and two rounds", () => {
    const html = markup(STATE_DIRTY);
    expect(html).toContain("DIRTY");
    expect(html).toContain("findings across");
    expect(html).toContain("Round 1");
    expect(html).toContain("Round 2");
    // severity labels: high/low verbatim, med → "medium"
    expect(html).toContain(">high<");
    expect(html).toContain(">medium<");
    expect(html).toContain(">low<");
    // severity color classes (coral high, amber med, neutral low)
    expect(html).toContain("text-coral-dark");
    expect(html).toContain("text-amber-dark");
    // finding location + message
    expect(html).toContain("auth.rs:42");
    expect(html).toContain("Token accepted when signature header is absent");
  });
});

describe("AgentConsole — verdict files clause (no fabricated count)", () => {
  // FIX 2: when verdict.files is ABSENT the meta line must NOT assert a count — no
  // hardcoded "2 files" anywhere; the "… reviewed" / "across …" clause is omitted.
  function withVerdict(verdict: ConsoleActivity["entries"]): ConsoleActivity {
    return { entries: verdict };
  }

  it("CLEAN without files renders just 'No policy violations' (no count, no '2 files')", () => {
    const html = markup(
      withVerdict([
        {
          type: "spawn",
          text: "spawned mini-coder",
          time: "00:00",
          mini: {
            model: "mini · sonnet-4",
            scope: [],
            rounds: [{ n: 1, actions: [], verdict: { state: "clean" } }],
          },
        },
      ]),
    );
    expect(html).toContain("CLEAN");
    expect(html).toContain("No policy violations");
    expect(html).not.toContain("reviewed");
    expect(html).not.toContain("2 files");
  });

  it("CLEAN WITH files appends the '… reviewed' clause with the real count", () => {
    const html = markup(
      withVerdict([
        {
          type: "spawn",
          text: "spawned mini-coder",
          time: "00:00",
          mini: {
            model: "mini · sonnet-4",
            scope: [],
            rounds: [
              { n: 1, actions: [], verdict: { state: "clean", files: "3 files" } },
            ],
          },
        },
      ]),
    );
    expect(html).toContain("3 files");
    expect(html).toContain("reviewed");
    expect(html).not.toContain("2 files");
  });

  it("DIRTY without files renders 'N finding(s)' without the 'across …' clause", () => {
    const html = markup(
      withVerdict([
        {
          type: "spawn",
          text: "spawned mini-coder",
          time: "00:00",
          mini: {
            model: "mini · sonnet-4",
            scope: [],
            rounds: [
              {
                n: 1,
                actions: [],
                verdict: {
                  state: "dirty",
                  findings: [{ sev: "high", loc: "a.rs:1", msg: "bad" }],
                },
              },
            ],
          },
        },
      ]),
    );
    expect(html).toContain("DIRTY");
    expect(html).toContain("finding");
    expect(html).not.toContain("across");
    expect(html).not.toContain("2 files");
  });
});

describe("AgentConsole — C: Clean → Done", () => {
  it("renders a CLEAN verdict and a Done banner", () => {
    const html = markup(STATE_CLEAN);
    expect(html).toContain("CLEAN");
    expect(html).toContain("No policy violations");
    expect(html).toContain("Done");
    expect(html).toContain("2 files · 1 round · edits applied");
    // sage banner class
    expect(html).toContain("text-sage-dark");
    // Output is COLLAPSED by default (matching the mock): the ok-line text is not
    // in the static markup, and the raw HTML marker is never emitted as markup.
    expect(html).not.toContain("test result: ok. 41 passed; 0 failed");
    expect(html).not.toContain('<span class="ok-ln">');
  });
});

describe("AgentConsole — D: Escalated", () => {
  it("renders the amber Escalated banner with its budget sub-line", () => {
    const html = markup(STATE_ESCALATED);
    expect(html).toContain("Escalated");
    expect(html).toContain("hit 3-round fix budget · handed back to coder");
    expect(html).toContain("Round 3");
    expect(html).toContain("text-amber-dark");
  });
});

describe("AgentConsole — E: Idle / empty", () => {
  it("renders the centered empty state copy + mono hint", () => {
    const html = markup(STATE_EMPTY);
    expect(html).toContain("No agent activity yet");
    expect(html).toContain(
      "When a coder claims a task or spawns a mini-coder",
    );
    expect(html).toContain("waiting on orchestrator…");
  });

  it("an activity with no entries also renders empty", () => {
    expect(markup({})).toContain("No agent activity yet");
    expect(markup({ entries: [] })).toContain("No agent activity yet");
  });
});

// ---- interaction: expand / collapse on a real DOM ---------------------------

describe("AgentConsole — expand / collapse", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("a write action with a diff toggles its detail; a static action has no toggle", () => {
    act(() => {
      root.render(createElement(AgentConsole, { activity: STATE_RUNNING }));
    });

    // Toggle buttons are exactly the actions with a diff/output (Read, Search,
    // Write here). The running "Run cargo check" action is STATIC (no toggle).
    const toggles = Array.from(
      container.querySelectorAll<HTMLButtonElement>('button[aria-expanded]'),
    );
    expect(toggles.length).toBeGreaterThan(0);

    // The Write action carries the diff; find it by its verb text.
    const writeBtn = toggles.find((b) =>
      b.textContent?.includes("Write"),
    );
    expect(writeBtn).toBeTruthy();
    expect(writeBtn?.getAttribute("aria-expanded")).toBe("false");
    // Collapsed: the diff hunk meta line is not in the DOM yet.
    expect(container.textContent).not.toContain(
      "@@ src/auth.rs · fn validate @@",
    );

    // Expand.
    act(() => {
      writeBtn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(writeBtn?.getAttribute("aria-expanded")).toBe("true");
    expect(container.textContent).toContain("@@ src/auth.rs · fn validate @@");
    expect(container.textContent).toContain(
      "fn validate(token: &str) -> Result<Claims, AuthError>",
    );

    // Collapse again.
    act(() => {
      writeBtn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(writeBtn?.getAttribute("aria-expanded")).toBe("false");
    expect(container.textContent).not.toContain(
      "@@ src/auth.rs · fn validate @@",
    );

    // The static running action ("Run" / "cargo check") is NOT an aria-expanded
    // button — it renders as a plain row.
    const runButtonIsToggle = toggles.some(
      (b) =>
        b.textContent?.includes("cargo check") &&
        b.textContent?.includes("Run"),
    );
    expect(runButtonIsToggle).toBe(false);
    expect(container.textContent).toContain("cargo check");
  });

  it("expanding an output action reveals the ok-line as a sage span (not raw HTML)", () => {
    act(() => {
      root.render(createElement(AgentConsole, { activity: STATE_CLEAN }));
    });

    // The "Run cargo test && npm test" action carries an output block with the
    // ok-ln marker — collapsed by default, so its text is absent until expanded.
    expect(container.textContent).not.toContain(
      "test result: ok. 41 passed; 0 failed",
    );
    const runBtn = Array.from(
      container.querySelectorAll<HTMLButtonElement>("button[aria-expanded]"),
    ).find((b) => b.textContent?.includes("cargo test && npm test"));
    expect(runBtn).toBeTruthy();

    act(() => {
      runBtn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(container.textContent).toContain(
      "test result: ok. 41 passed; 0 failed",
    );
    // The ok-ln marker is a sage span, NOT raw HTML injected via innerHTML.
    expect(container.innerHTML).not.toContain('<span class="ok-ln">');
    const okSpan = Array.from(container.querySelectorAll("span")).find(
      (s) =>
        s.className.includes("text-sage-dark") &&
        s.textContent === "test result: ok. 41 passed; 0 failed",
    );
    expect(okSpan).toBeTruthy();
  });
});

// ---- hook: graceful degradation when no backend exists ----------------------

describe("useAgentConsole — graceful degradation", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function Harness({ deps, agentId }: { deps: AgentConsoleDeps; agentId: string | null }) {
    const activity = useAgentConsole(agentId, deps);
    return createElement(AgentConsole, { activity });
  }

  it("swallows a failing snapshot and renders the empty state; unlistens on unmount", async () => {
    const unlisten = vi.fn();
    const deps: AgentConsoleDeps = {
      listen: vi.fn(async () => unlisten),
      fetchSnapshot: vi.fn(async () => {
        throw new Error("mini_activity_snapshot not implemented yet");
      }),
    };

    await act(async () => {
      root.render(createElement(Harness, { deps, agentId: "coder-1" }));
    });
    // Subscribed BEFORE the snapshot invoke, then degraded to empty.
    expect(deps.listen).toHaveBeenCalledTimes(1);
    expect(deps.fetchSnapshot).toHaveBeenCalledWith("coder-1");
    expect(container.textContent).toContain("No agent activity yet");

    await act(async () => {
      root.unmount();
    });
    expect(unlisten).toHaveBeenCalledTimes(1);
    // re-create a root so afterEach's unmount is a harmless no-op
    root = createRoot(container);
  });

  it("applies a snapshot delivered over the channel", async () => {
    let emit: ((e: { payload: unknown }) => void) | null = null;
    const deps: AgentConsoleDeps = {
      listen: vi.fn(async (_channel, handler) => {
        emit = handler;
        return () => {};
      }),
      fetchSnapshot: vi.fn(async () => ({ empty: true }) as ConsoleActivity),
    };

    await act(async () => {
      root.render(createElement(Harness, { deps, agentId: "coder-1" }));
    });
    expect(container.textContent).toContain("No agent activity yet");

    // A live snapshot event flips it to the running timeline.
    await act(async () => {
      emit?.({ payload: { type: "snapshot", activity: STATE_RUNNING } });
    });
    expect(container.textContent).toContain("Round 1");
    expect(container.textContent).toContain("working — compiling edits…");
  });

  it("resets to empty when agentId becomes null", async () => {
    const deps: AgentConsoleDeps = {
      listen: vi.fn(async () => () => {}),
      fetchSnapshot: vi.fn(async () => STATE_CLEAN),
    };

    await act(async () => {
      root.render(createElement(Harness, { deps, agentId: "coder-1" }));
    });
    expect(container.textContent).toContain("Done");

    await act(async () => {
      root.render(createElement(Harness, { deps, agentId: null }));
    });
    expect(container.textContent).toContain("No agent activity yet");
  });
});
