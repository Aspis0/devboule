// @vitest-environment jsdom
//
// Tests for useStageRotation's `hold` parameter (Phase 5).
//
// Covers:
//   - view advances after intervalMs when auto && enabled && !hold
//   - view does NOT advance when hold is true
//   - rotation resumes once hold goes false (auto stays true throughout)
//   - hold never mutates the auto flag; pick/toggleAuto semantics unaffected

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useStageRotation } from "./useStageRotation";
import type { StageRotation } from "./useStageRotation";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// Short interval so tests don't wait 3800 ms.
const INTERVAL = 100;

// ---------------------------------------------------------------------------
// Minimal renderHook for useStageRotation
// ---------------------------------------------------------------------------

async function mountRotation(
  initialHold: boolean,
  enabled = true,
): Promise<{
  result: () => StageRotation;
  rerender: (newHold: boolean) => Promise<void>;
  unmount: () => void;
  container: HTMLDivElement;
  root: Root;
}> {
  let latest!: StageRotation;
  let currentHold = initialHold;

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);

  function Hook() {
    latest = useStageRotation(INTERVAL, enabled, currentHold);
    return null;
  }

  await act(async () => {
    root.render(createElement(Hook, null));
  });

  return {
    container,
    root,
    result: () => latest,
    rerender: async (newHold: boolean) => {
      currentHold = newHold;
      await act(async () => {
        root.render(createElement(Hook, null));
      });
    },
    unmount: () => {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("useStageRotation — hold parameter", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("advances the view after intervalMs when auto && enabled && !hold", async () => {
    const { result, unmount } = await mountRotation(false);
    try {
      expect(result().view).toBe("exa"); // starts at 'exa'

      await act(async () => {
        vi.advanceTimersByTime(INTERVAL);
      });

      expect(result().view).toBe("plan"); // exa → plan
    } finally {
      unmount();
    }
  });

  it("does NOT advance the view when hold is true", async () => {
    const { result, unmount } = await mountRotation(true);
    try {
      expect(result().view).toBe("exa");

      await act(async () => {
        // 2 intervals → would reach 'design' if rotation were active.
        // (3 intervals would cycle back to 'exa' and pass for the wrong reason.)
        vi.advanceTimersByTime(INTERVAL * 2);
      });

      expect(result().view).toBe("exa"); // view must not have moved
    } finally {
      unmount();
    }
  });

  it("resumes rotation after hold goes false (auto stays true)", async () => {
    const { result, rerender, unmount } = await mountRotation(true);
    try {
      // With hold on, timer fires but view should not advance.
      await act(async () => {
        vi.advanceTimersByTime(INTERVAL);
      });
      expect(result().view).toBe("exa");

      // Release hold — rotation must resume.
      await rerender(false);

      await act(async () => {
        vi.advanceTimersByTime(INTERVAL);
      });
      expect(result().view).toBe("plan");
    } finally {
      unmount();
    }
  });

  it("hold never mutates the auto flag", async () => {
    const { result, rerender, unmount } = await mountRotation(false);
    try {
      expect(result().auto).toBe(true);

      // Activating hold must not flip auto.
      await rerender(true);
      expect(result().auto).toBe(true);

      // Releasing hold must not flip auto.
      await rerender(false);
      expect(result().auto).toBe(true);

      // pick() still sets auto=false as before.
      await act(async () => {
        result().pick("design");
      });
      expect(result().auto).toBe(false);

      // toggleAuto() still flips it back.
      await act(async () => {
        result().toggleAuto();
      });
      expect(result().auto).toBe(true);
    } finally {
      unmount();
    }
  });
});
