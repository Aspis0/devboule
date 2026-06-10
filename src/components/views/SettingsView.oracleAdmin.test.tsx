// @vitest-environment jsdom
//
// The Oracle ADMIN surface was moved OFF Settings → Workspace and onto the
// restored standalone Oracle page (OracleView). This test asserts the de-
// duplication: switching to the Workspace tab renders WorkspaceView but does
// NOT mount OracleAdminPanel anymore. Child views are mocked to lightweight
// markers so this only proves the WIRING.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// ---- AppContext mock (minimal surface SettingsView reads) -----------------
const consumePendingTab = vi.fn(() => null as string | null);
vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({ roleStatus: { role: "admin", isAdmin: true }, pendingTab: 0 }),
  useAppActions: () => ({ consumePendingTab, lock: vi.fn() }),
}));

// ---- Child views mocked to markers ----------------------------------------
vi.mock("./SecretsView", () => ({ SecretsView: () => createElement("div") }));
vi.mock("./DevicesView", () => ({ DevicesView: () => createElement("div") }));
vi.mock("./WorkspaceView", () => ({
  WorkspaceView: () =>
    createElement("div", { "data-testid": "workspace-view" }, "workspace"),
}));
// If SettingsView still imported the admin panel, this marker would surface it.
vi.mock("../oracle/OracleAdminPanel", () => ({
  OracleAdminPanel: () =>
    createElement("div", { "data-testid": "oracle-admin-panel" }, "oracle admin"),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let SettingsView: typeof import("./SettingsView").SettingsView;

beforeEach(async () => {
  consumePendingTab.mockReturnValue(null);
  ({ SettingsView } = await import("./SettingsView"));
});

function render(): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(SettingsView));
  });
  return { container, root };
}

describe("SettingsView → Workspace tab", () => {
  it("renders WorkspaceView but NOT OracleAdminPanel (admin moved to OracleView)", () => {
    const { container } = render();
    // Switch to the Workspace tab.
    const tab = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Workspace & Index",
    )!;
    act(() => tab.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(
      container.querySelector('[data-testid="workspace-view"]'),
    ).not.toBeNull();
    // The admin panel must NOT appear in Settings anymore.
    expect(
      container.querySelector('[data-testid="oracle-admin-panel"]'),
    ).toBeNull();
  });
});
