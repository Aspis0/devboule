// @vitest-environment jsdom
//
// The cloud "Providers" area is hidden from the sidebar until the
// provider-agnostic refactor (S1). It must NOT render as a nav button for the
// default config, nor for a stale persisted config that still lists a
// "providers" nav entry (defensive HIDDEN_NAV_IDS filter in Sidebar). The view
// itself stays reachable by deep link (requestView("providers")).

import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { AppConfig, NavItem } from "../types/config";

function makeConfig(navigation: NavItem[]): AppConfig {
  return {
    project: { name: "Devboule", version: "1.0.0" },
    navigation,
    providers: [],
    bookmarks: [],
    secrets: [],
    compute: {
      gpus: { active: 0, total: 0, provider: "Scaleway" },
      cpus: { active: 0, total: 0, provider: "Scaleway" },
      workers: { active: 0, total: 0, provider: "Cloudflare" },
    },
    budget: { monthly_limit: 0, currency: "EUR", categories: [] },
    customAgentClients: [],
  };
}

// Mutable config so individual tests can swap in a stale/persisted navigation.
let mockConfig: AppConfig = makeConfig([
  { id: "projects", label: "Projects", icon: "FolderKanban" } as NavItem,
  { id: "oracle", label: "Oracle", icon: "BrainCircuit" } as NavItem,
]);

const setActiveView = vi.fn();

vi.mock("../context/AppContext", () => ({
  useAppContext: () => ({
    config: mockConfig,
    activeView: "projects",
    roleStatus: { role: "admin", isAdmin: true, provisioned: true },
  }),
  useAppActions: () => ({ setActiveView }),
}));

vi.mock("../utils/roles", () => ({
  navIdsForRole: (_role: unknown, ids: string[]) => ids,
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let container: HTMLElement;

beforeEach(() => {
  setActiveView.mockClear();
  // Reset to the default (providers-less) config before each test.
  mockConfig = makeConfig([
    { id: "projects", label: "Projects", icon: "FolderKanban" } as NavItem,
    { id: "oracle", label: "Oracle", icon: "BrainCircuit" } as NavItem,
  ]);
  container = document.createElement("div");
  document.body.appendChild(container);
});

describe("Sidebar hides the cloud Providers area (S1)", () => {
  it("default config: no Providers button, but renders Projects/Oracle/Polis/Design/Skills/Labs", async () => {
    const { Sidebar } = await import("./Sidebar");
    await act(async () => {
      createRoot(container).render(createElement(Sidebar));
    });

    const text = container.textContent ?? "";
    // Providers must be hidden.
    expect(text).not.toContain("Providers");
    // The expected entries (injected by Sidebar) must still render.
    for (const label of [
      "Projects",
      "Oracle",
      "Polis",
      "Design",
      "Skills",
      "Labs",
    ]) {
      expect(text).toContain(label);
    }

    // No nav button should resolve to the "providers" view.
    const providersBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Providers",
    );
    expect(providersBtn).toBeFalsy();
  });

  it("stale persisted config with a 'providers' entry is filtered out of the nav", async () => {
    // Simulate an old saved config on disk / backend get_config that still
    // lists the providers nav id.
    mockConfig = makeConfig([
      { id: "projects", label: "Projects", icon: "FolderKanban" } as NavItem,
      { id: "providers", label: "Providers", icon: "Boxes" } as NavItem,
      { id: "oracle", label: "Oracle", icon: "BrainCircuit" } as NavItem,
    ]);

    const { Sidebar } = await import("./Sidebar");
    await act(async () => {
      createRoot(container).render(createElement(Sidebar));
    });

    const text = container.textContent ?? "";
    expect(text).not.toContain("Providers");

    // Injected entries still present.
    for (const label of ["Projects", "Oracle", "Polis", "Design", "Skills", "Labs"]) {
      expect(text).toContain(label);
    }

    const providersBtn = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.trim() === "Providers",
    );
    expect(providersBtn).toBeFalsy();
  });
});
