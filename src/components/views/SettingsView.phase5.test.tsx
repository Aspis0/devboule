// @vitest-environment jsdom
//
// Phase 5 Settings IA: 4 tabs (account / providers / workspace / security) with
// legacy deep-link redirects. Child views + the Providers tab are mocked to markers
// so this test only proves the WIRING: tab order, legacy pendingTab routing through
// mapLegacySettingsTab, and the Security tab's admin-only Devices gate.

import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

// ---- AppContext mock ------------------------------------------------------
const consumePendingTab = vi.fn(() => null as string | null);
let roleStatus: { role: string; isAdmin: boolean } | null = {
  role: "admin",
  isAdmin: true,
};
vi.mock("../../context/AppContext", () => ({
  useAppContext: () => ({ roleStatus, pendingTab: 0 }),
  useAppActions: () => ({
    consumePendingTab,
    lock: vi.fn(),
    // CliAgentsCard reads these (Account tab); harmless no-ops.
    configureCliAgents: vi.fn(async () => ({})),
    unconfigureCliAgents: vi.fn(async () => ({})),
    cliAgentsStatus: vi.fn(async () => ({ runtimeReady: false })),
  }),
}));

// ---- Child views + tab mocked to markers ----------------------------------
vi.mock("./SecretsView", () => ({
  SecretsView: () =>
    createElement("div", { "data-testid": "secrets-view" }, "secrets"),
}));
vi.mock("./DevicesView", () => ({
  DevicesView: () =>
    createElement("div", { "data-testid": "devices-view" }, "devices"),
}));
vi.mock("./WorkspaceView", () => ({
  WorkspaceView: () =>
    createElement("div", { "data-testid": "workspace-view" }, "workspace"),
}));
// The Oracle admin panel moved OFF Settings onto the standalone OracleView, so
// SettingsView no longer imports it. The mock is kept ONLY so the absence
// assertion below has a stable test-id to look for (it never mounts).
vi.mock("../oracle/OracleAdminPanel", () => ({
  OracleAdminPanel: () =>
    createElement("div", { "data-testid": "oracle-admin-panel" }, "admin"),
}));
vi.mock("../settings/ProvidersModelsTab", () => ({
  ProvidersModelsTab: () =>
    createElement("div", { "data-testid": "providers-tab" }, "providers"),
}));

let SettingsView: typeof import("./SettingsView").SettingsView;

beforeEach(async () => {
  consumePendingTab.mockReturnValue(null);
  roleStatus = { role: "admin", isAdmin: true };
  ({ SettingsView } = await import("./SettingsView"));
});

// Track every mounted root so afterEach can unmount it — the Account tab's
// CliAgentsCard runs an async status fetch whose finally-setState would otherwise
// fire after teardown and surface as an unhandled rejection.
let mountedRoots: Root[] = [];

function render(): { container: HTMLElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root!: Root;
  act(() => {
    root = createRoot(container);
    root.render(createElement(SettingsView));
  });
  mountedRoots.push(root);
  return { container, root };
}

afterEach(() => {
  // Unmount synchronously BEFORE the async CliAgentsCard status fetch resolves:
  // unmount sets the card's mountedRef to false, so its finally-setState guard
  // (`if (mountedRef.current ...)`) no-ops and nothing lands after teardown.
  act(() => {
    for (const root of mountedRoots) root.unmount();
  });
  mountedRoots = [];
});

function tabLabels(container: HTMLElement): string[] {
  // The tab bar is the first flex container of buttons; read its button labels.
  const bar = container.querySelector("div.flex")!;
  return Array.from(bar.querySelectorAll("button")).map((b) =>
    (b.textContent ?? "").trim(),
  );
}

describe("SettingsView Phase 5 — tabs + routing", () => {
  it("renders the four tabs in order account / providers / workspace / security", () => {
    const { container } = render();
    expect(tabLabels(container)).toEqual([
      "Account",
      "Providers & Models",
      "Workspace & Index",
      "Security",
    ]);
  });

  it("legacy settings#oracle deep-link lands on the providers tab", () => {
    consumePendingTab.mockReturnValue("oracle");
    const { container } = render();
    expect(container.querySelector('[data-testid="providers-tab"]')).not.toBeNull();
  });

  it("legacy settings#workspace deep-link lands on the workspace tab (admin moved to OracleView)", () => {
    consumePendingTab.mockReturnValue("workspace");
    const { container } = render();
    expect(container.querySelector('[data-testid="workspace-view"]')).not.toBeNull();
    // The Oracle admin panel is no longer mounted in Settings.
    expect(container.querySelector('[data-testid="oracle-admin-panel"]')).toBeNull();
  });

  it("legacy settings#secrets deep-link lands on the security tab", () => {
    consumePendingTab.mockReturnValue("secrets");
    const { container } = render();
    expect(container.querySelector('[data-testid="secrets-view"]')).not.toBeNull();
  });

  it("legacy settings#devices deep-link lands on the security tab", () => {
    consumePendingTab.mockReturnValue("devices");
    const { container } = render();
    expect(container.querySelector('[data-testid="secrets-view"]')).not.toBeNull();
  });
});

describe("SettingsView Phase 5 — Security tab role gating", () => {
  it("shows Devices under Security for an admin", () => {
    roleStatus = { role: "admin", isAdmin: true };
    consumePendingTab.mockReturnValue("security");
    const { container } = render();
    expect(container.querySelector('[data-testid="secrets-view"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="devices-view"]')).not.toBeNull();
  });

  it("hides Devices for a collaborator but still shows Secrets", () => {
    roleStatus = { role: "collaborator", isAdmin: false };
    consumePendingTab.mockReturnValue("security");
    const { container } = render();
    expect(container.querySelector('[data-testid="secrets-view"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="devices-view"]')).toBeNull();
  });
});
