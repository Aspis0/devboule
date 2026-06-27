// @vitest-environment jsdom
//
// Tests for StageDesign:
//  - Phase 5: onArtifactActiveChange lifecycle.
//  - Fix 2+3: isLinking guard (select disabled during in-flight change) and
//    linkError display on failure.
//
// The CRITICAL invariant: unmount-while-active MUST emit false so the parent's
// rotation hold can never get permanently stuck when the Design stage is rotated
// away while an interactive artifact is live.

import { describe, it, expect, vi, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ArtifactKind } from "../../../types/design";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

// jsdom has no ResizeObserver. ArtifactFrame's fixed-dimension skins (ios/android) construct
// one on mount; the Fix 2 tests generate an inferred `ios` frame and then auto-show it, so we
// stub a no-op observer. Component/web skins never construct one, so the other tests here are
// unaffected.
if (!(globalThis as { ResizeObserver?: unknown }).ResizeObserver) {
  class ResizeObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  (globalThis as unknown as { ResizeObserver: typeof ResizeObserverStub }).ResizeObserver =
    ResizeObserverStub;
}

// ---------------------------------------------------------------------------
// Mocks — must precede the component import (vitest hoists vi.mock calls)
// ---------------------------------------------------------------------------

// generateAndRegisterDesign calls Tauri backend — stub it out entirely.
vi.mock("../../design/generation/generateAndRegister", () => ({
  generateAndRegisterDesign: vi.fn(async () => "generated-id"),
}));

import { StageDesign } from "./StageDesign";
import { generateAndRegisterDesign } from "../../design/generation/generateAndRegister";

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

const interactiveDesign = {
  name: "Test Design",
  version: "v1",
  ago: "1m ago",
  thumbnailUri: null,
  kind: "interactive" as ArtifactKind,
  artifactId: "test-artifact-id",
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function mountStageDesign(
  onArtifactActiveChange: (active: boolean) => void,
): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);

  await act(async () => {
    root.render(
      createElement(StageDesign, {
        design: interactiveDesign,
        linkedTask: null,
        onOpenInDesign: vi.fn(),
        projectRoot: "/test/root",
        onGenerated: vi.fn(),
        onArtifactActiveChange,
      }),
    );
  });

  return { container, root };
}

/** Find the artifact-toggle button when the artifact is closed (title = "Open interactive artifact"). */
function openBtn(container: HTMLElement): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>(
    'button[title="Open interactive artifact"]',
  );
}

/** Find the artifact-toggle button when the artifact is open (title = "Hide artifact"). */
function hideBtn(container: HTMLElement): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>(
    'button[title="Hide artifact"]',
  );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Fix 4 / Fix 1 TDD: static generation must NOT set artifactId
// This describe block MUST FAIL before Fix 1 is applied (buttons are rendered
// because localArtifactId was wrongly set), and PASS after.
// ---------------------------------------------------------------------------
describe("StageDesign — static design false-positive (TDD for Fix 1)", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("static generation does not set artifactId, show artifact buttons, or call onArtifactActiveChange(true)", async () => {
    const spy = vi.fn();
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        createElement(StageDesign, {
          design: null,
          linkedTask: null,
          onOpenInDesign: vi.fn(),
          projectRoot: "/test/root",
          onGenerated: vi.fn(),
          onArtifactActiveChange: spy,
        }),
      );
    });

    try {
      // Switch to static mode
      const staticBtn = container.querySelector<HTMLButtonElement>(
        'button[title="Generate a static mockup (DOMPurify-sanitized)"]',
      );
      expect(staticBtn).not.toBeNull();
      await act(async () => {
        staticBtn!.click();
      });

      // Fill in the prompt — React controlled input requires the native setter + input event
      const promptInput = container.querySelector<HTMLInputElement>(
        'input[aria-label="Design prompt"]',
      )!;
      const nativeInputSetter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )!.set!;
      await act(async () => {
        nativeInputSetter.call(promptInput, "a test screen");
        promptInput.dispatchEvent(new Event("input", { bubbles: true }));
      });

      // Generate and wait for the mock async to resolve
      const generateBtn = container.querySelector<HTMLButtonElement>(
        'button[aria-label="Generate"]',
      );
      expect(generateBtn).not.toBeNull();
      await act(async () => {
        generateBtn!.click();
      });

      // No rotation-hold signal must have fired
      expect(spy).not.toHaveBeenCalledWith(true);

      // No Maximize button in the left panel top bar (artifact toggle)
      expect(
        container.querySelector('button[title="Open interactive artifact"]'),
      ).toBeNull();

      // No "Open artifact" button in the right meta panel
      const allBtns = Array.from(container.querySelectorAll("button"));
      const openArtifactBtn = allBtns.find((b) =>
        b.textContent?.trim().startsWith("Open artifact"),
      );
      expect(openArtifactBtn).toBeUndefined();
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });
});

// ---------------------------------------------------------------------------
// Fix 2 + Fix 3: isLinking guard and linkError feedback
// ---------------------------------------------------------------------------

const linkedDesign = {
  name: "Design with id",
  version: "v1",
  ago: "2m ago",
  thumbnailUri: null,
  id: "p-test-id",
  kind: "interactive" as ArtifactKind,
  artifactId: "art-test-id",
};

const sampleTasks = [
  { n: 1, title: "Build login" },
  { n: 2, title: "Add dashboard" },
];

describe("StageDesign — task-link isLinking guard and linkError", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("disables the select while an onLinkTask promise is pending, re-enables after resolve", async () => {
    let resolveLink!: () => void;
    const linkPromise = new Promise<void>((res) => {
      resolveLink = res;
    });
    const onLinkTask = vi.fn(() => linkPromise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        createElement(StageDesign, {
          design: linkedDesign,
          linkedTask: null,
          onOpenInDesign: vi.fn(),
          projectRoot: "/test/root",
          onGenerated: vi.fn(),
          tasks: sampleTasks,
          onLinkTask,
        }),
      );
    });

    try {
      const select = container.querySelector<HTMLSelectElement>(
        'select[aria-label="Attach design to task"]',
      );
      expect(select).not.toBeNull();
      expect(select!.disabled).toBe(false);

      // Fire the change — the promise is still pending.
      await act(async () => {
        const event = new Event("change", { bubbles: true });
        Object.defineProperty(event, "target", {
          writable: false,
          value: Object.assign(select!, { value: "1" }),
        });
        select!.dispatchEvent(event);
      });

      // The guard should have been entered: select is disabled while pending.
      expect(select!.disabled).toBe(true);

      // Resolve the promise.
      await act(async () => {
        resolveLink();
        await linkPromise;
      });

      // After resolve the select is re-enabled.
      expect(select!.disabled).toBe(false);
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });

  it("shows a linkError message and re-enables the select when onLinkTask rejects", async () => {
    const onLinkTask = vi.fn(() =>
      Promise.reject(new Error("backend error")),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        createElement(StageDesign, {
          design: linkedDesign,
          linkedTask: null,
          onOpenInDesign: vi.fn(),
          projectRoot: "/test/root",
          onGenerated: vi.fn(),
          tasks: sampleTasks,
          onLinkTask,
        }),
      );
    });

    try {
      const select = container.querySelector<HTMLSelectElement>(
        'select[aria-label="Attach design to task"]',
      );
      expect(select).not.toBeNull();

      // Fire the change — onLinkTask will reject.
      await act(async () => {
        const event = new Event("change", { bubbles: true });
        Object.defineProperty(event, "target", {
          writable: false,
          value: Object.assign(select!, { value: "2" }),
        });
        select!.dispatchEvent(event);
      });

      // Select must be re-enabled after the rejection settles.
      expect(select!.disabled).toBe(false);

      // The inline error alert must be visible.
      const alert = container.querySelector('[role="alert"]');
      expect(alert).not.toBeNull();
      expect(alert!.textContent).toMatch(/try again/i);
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });
});

describe("StageDesign — onArtifactActiveChange lifecycle", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("emits true when an interactive artifact is opened", async () => {
    const spy = vi.fn();
    const { container, root } = await mountStageDesign(spy);
    try {
      // Before opening: no true call
      expect(spy).not.toHaveBeenCalledWith(true);

      const btn = openBtn(container);
      expect(btn).not.toBeNull();
      await act(async () => {
        btn!.click();
      });

      expect(spy).toHaveBeenCalledWith(true);
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });

  it("emits false when the artifact is closed", async () => {
    const spy = vi.fn();
    const { container, root } = await mountStageDesign(spy);
    try {
      // Open first
      await act(async () => {
        openBtn(container)!.click();
      });
      spy.mockClear();

      // Now close
      const close = hideBtn(container);
      expect(close).not.toBeNull();
      await act(async () => {
        close!.click();
      });

      expect(spy).toHaveBeenCalledWith(false);
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });

  it("emits false on unmount while artifact is active — hold-release safety", async () => {
    const spy = vi.fn();
    const { container, root } = await mountStageDesign(spy);

    // Open the artifact
    await act(async () => {
      openBtn(container)!.click();
    });
    expect(spy).toHaveBeenCalledWith(true);
    spy.mockClear();

    // Unmount WITHOUT closing — this is the dangerous scenario
    await act(async () => {
      root.unmount();
    });
    container.remove();

    // The unmount cleanup MUST emit false to release the parent's hold
    expect(spy).toHaveBeenCalledWith(false);
  });
});

// ---------------------------------------------------------------------------
// Fix 2: handleGenerate must persist the RESOLVED frame (effectiveFrameKind),
// not the raw dropdown value — so a heuristic-inferred skin survives remount.
// ---------------------------------------------------------------------------
describe("StageDesign — Fix 2: persists the resolved (inferred) frame", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  function fillPrompt(container: HTMLElement, value: string): Promise<void> {
    const promptInput = container.querySelector<HTMLInputElement>(
      'input[aria-label="Design prompt"]',
    )!;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )!.set!;
    return act(async () => {
      setter.call(promptInput, value);
      promptInput.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  it("stores the heuristic-inferred frame (not undefined) when no dropdown frame is chosen", async () => {
    const mockGen = vi.mocked(generateAndRegisterDesign);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        createElement(StageDesign, {
          design: null, // no stored frame -> chain falls through to inference
          linkedTask: null,
          onOpenInDesign: vi.fn(),
          projectRoot: "/test/root",
          onGenerated: vi.fn(),
        }),
      );
    });

    try {
      // interactive is the default mode; leave the frame dropdown at "" (Default).
      // "iOS dashboard" -> inferFrameKind -> "ios".
      await fillPrompt(container, "iOS dashboard");

      const generateBtn = container.querySelector<HTMLButtonElement>(
        'button[aria-label="Generate"]',
      )!;
      await act(async () => {
        generateBtn.click();
      });

      expect(mockGen).toHaveBeenCalledTimes(1);
      // The inferred frame ("ios") must be persisted, NOT undefined.
      expect(mockGen.mock.calls[0][0]).toMatchObject({
        mode: "interactive",
        frame: "ios",
        prompt: "iOS dashboard",
      });
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });

  it("explicit dropdown frame still wins over inference", async () => {
    const mockGen = vi.mocked(generateAndRegisterDesign);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        createElement(StageDesign, {
          design: null,
          linkedTask: null,
          onOpenInDesign: vi.fn(),
          projectRoot: "/test/root",
          onGenerated: vi.fn(),
        }),
      );
    });

    try {
      // Pick "android" from the frame dropdown even though the prompt infers ios.
      const select = container.querySelector<HTMLSelectElement>(
        'select[aria-label="Frame"]',
      )!;
      const selectSetter = Object.getOwnPropertyDescriptor(
        window.HTMLSelectElement.prototype,
        "value",
      )!.set!;
      await act(async () => {
        selectSetter.call(select, "android");
        select.dispatchEvent(new Event("change", { bubbles: true }));
      });

      await fillPrompt(container, "iOS dashboard");

      const generateBtn = container.querySelector<HTMLButtonElement>(
        'button[aria-label="Generate"]',
      )!;
      await act(async () => {
        generateBtn.click();
      });

      expect(mockGen).toHaveBeenCalledTimes(1);
      // Explicit dropdown ("android") wins over the inferred ios.
      expect(mockGen.mock.calls[0][0]).toMatchObject({ frame: "android" });
    } finally {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    }
  });
});
