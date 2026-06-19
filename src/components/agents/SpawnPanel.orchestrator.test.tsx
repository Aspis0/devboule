// @vitest-environment jsdom
//
// L2 — proves the launcher SURFACES the local Devboule main-coder model when the
// "Local (Devboule)" CLI is selected (so the model is NOT empty), and that selecting it
// prefills the advisory model field with the configured model. Uses jsdom + a real click
// because the orchestrator branch only renders for the selected client (the static-render
// SpawnPanel.test.tsx covers the always-present option list).

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { SpawnPanel } from "./SpawnPanel";
import type { SpawnLaunchInput } from "./agentRowModel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLDivElement;
let root: Root;
// Captures every SpawnLaunchInput the panel emits, so a test can assert the exact
// value (incl. planFirst) that reaches the launch IPC boundary.
let launches: SpawnLaunchInput[];

async function mount(localCoderModel: string | null) {
  launches = [];
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      createElement(SpawnPanel, {
        projects: [{ id: "p1", title: "Proj" }],
        selectedProjectId: "p1",
        tasks: [],
        projectActive: true,
        isBusy: false,
        message: null,
        localCoderModel,
        onLaunch: (input) => launches.push(input),
        onCopyPrompt: () => undefined,
      }),
    );
  });
}

// Click a CLI radio button by its visible label text.
async function selectClient(label: string) {
  const buttons = Array.from(container.querySelectorAll("button"));
  const btn = buttons.find((b) => b.textContent?.trim() === label);
  if (!btn) throw new Error(`CLI button not found: ${label}`);
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

// The plan-first checkbox (orchestrator-only), or null when not rendered.
function planFirstToggle(): HTMLInputElement | null {
  return container.querySelector<HTMLInputElement>(
    '[data-testid="plan-first-toggle"]',
  );
}

// Click "Launch in app" so onLaunch fires with the built SpawnLaunchInput.
async function clickLaunchInApp() {
  const buttons = Array.from(container.querySelectorAll("button"));
  const btn = buttons.find((b) => b.textContent?.includes("Launch in app"));
  if (!btn) throw new Error("Launch in app button not found");
  await act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

// Click the "Local (Devboule)" CLI radio button, found by its label text.
async function selectOrchestrator() {
  const buttons = Array.from(container.querySelectorAll("button"));
  const orchestratorBtn = buttons.find(
    (b) => b.textContent?.trim() === "Local (Devboule)",
  );
  if (!orchestratorBtn) throw new Error("orchestrator CLI button not found");
  await act(async () => {
    orchestratorBtn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

beforeEach(() => {
  // no-op
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("SpawnPanel — orchestrator model surfacing", () => {
  it("prefills the model field and names the configured model in the note", async () => {
    await mount("qwen2.5-coder");
    await selectOrchestrator();

    const modelInput = container.querySelector(
      'input[type="text"]',
    ) as HTMLInputElement | null;
    expect(modelInput).not.toBeNull();
    // The advisory model field is no longer empty — it is prefilled with the configured
    // local-coder model.
    expect(modelInput!.value).toBe("qwen2.5-coder");
    // And the note names the configured model + points at Settings.
    expect(container.innerHTML).toContain("qwen2.5-coder");
    expect(container.innerHTML).toContain("Settings → Local main coder");
  });

  it("tells the user to configure a model when none is set", async () => {
    await mount(null);
    await selectOrchestrator();

    const modelInput = container.querySelector(
      'input[type="text"]',
    ) as HTMLInputElement | null;
    // No configured model => field stays empty (nothing to prefill) ...
    expect(modelInput!.value).toBe("");
    // ... and the note guides the user to Settings.
    expect(container.innerHTML).toContain("no model configured");
    expect(container.innerHTML).toContain("Settings → Local main coder");
  });
});

describe("SpawnPanel — Plan first toggle (3b)", () => {
  it("is hidden for codex and claude (default client is codex)", async () => {
    await mount("qwen2.5-coder");
    // codex is the default selected client.
    expect(planFirstToggle()).toBeNull();

    await selectClient("claude");
    expect(planFirstToggle()).toBeNull();
  });

  it("shows for the orchestrator and is ON by default", async () => {
    await mount("qwen2.5-coder");
    await selectOrchestrator();

    const toggle = planFirstToggle();
    expect(toggle).not.toBeNull();
    expect(toggle!.checked).toBe(true);
  });

  it("threads planFirst:true into the launch input when ON", async () => {
    await mount("qwen2.5-coder");
    await selectOrchestrator();
    await clickLaunchInApp();

    expect(launches).toHaveLength(1);
    expect(launches[0].client).toBe("orchestrator");
    expect(launches[0].planFirst).toBe(true);
  });

  it("sends planFirst=false when toggled OFF (the user's OFF choice is respected)", async () => {
    await mount("qwen2.5-coder");
    await selectOrchestrator();

    const toggle = planFirstToggle();
    expect(toggle).not.toBeNull();
    // React tracks the checkbox's value internally, so a plain `checked = false`
    // assignment is invisible to onChange. Use the prototype setter React installs
    // on so its change-tracker fires, then dispatch the native `click` (which flips
    // `checked` and emits the React-visible change). Clicking is the simplest path.
    await act(async () => {
      toggle!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(toggle!.checked).toBe(false);
    await clickLaunchInApp();

    expect(launches).toHaveLength(1);
    // A-F1: OFF now sends an EXPLICIT false (Rust defaults an ABSENT value to plan-first,
    // so omitting it would let that default override the user's OFF choice).
    expect(launches[0].planFirst).toBe(false);
  });

  it("never carries planFirst for a non-orchestrator client", async () => {
    await mount("qwen2.5-coder");
    // Select the orchestrator first (toggle defaults ON), then switch to codex:
    // the flag must NOT leak into the codex launch.
    await selectOrchestrator();
    await selectClient("codex");
    expect(planFirstToggle()).toBeNull();
    await clickLaunchInApp();

    expect(launches).toHaveLength(1);
    expect(launches[0].client).toBe("codex");
    expect(launches[0].planFirst).toBeUndefined();
  });
});
