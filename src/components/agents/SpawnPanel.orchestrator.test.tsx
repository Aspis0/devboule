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

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLDivElement;
let root: Root;

async function mount(localCoderModel: string | null) {
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
        onLaunch: () => undefined,
        onCopyPrompt: () => undefined,
      }),
    );
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
