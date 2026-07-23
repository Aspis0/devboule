// @vitest-environment jsdom
//
// The standalone Oracle view was RESTORED, so the Sidebar must now RENDER a nav
// button for an "oracle" entry present in the config (the defensive Phase 4(c)
// strip was removed). This test injects a config that includes the oracle entry
// and asserts the nav button is rendered and clickable.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import type { AppConfig, NavItem } from "../types/config";

// A minimal AppConfig that includes the restored "oracle" nav entry.
const config: AppConfig = {
  project: { name: "Devboule", version: "1.0.0" },
  navigation: [
    { id: "projects", label: "Projects", icon: "FolderKanban" } as NavItem,
    { id: "oracle", label: "Oracle", icon: "BrainCircuit" } as NavItem,
  ],
  providers: [],
  bookmarks: [],
  secrets: [],
  compute: {
    gpus: { active: 0, total: 0, provider: "" },
    cpus: { active: 0, total: 0, provider: "" },
    workers: { active: 0, total: 0, provider: "" },
  },
  budget: { monthly_limit: 0, currency: "EUR", categories: [] },
  customAgentClients: [],
};

const setActiveView = vi.fn();

vi.mock("../context/AppContext", () => ({
  useAppContext: () => ({
    config,
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
  container = document.createElement("div");
  document.body.appendChild(container);
});

describe("Sidebar oracle nav (restored standalone view)", () => {
  it("renders a clickable nav button for the oracle item", async () => {
    const { Sidebar } = await import("./Sidebar");
    await act(async () => {
      createRoot(container).render(createElement(Sidebar));
    });

    // The other entries still render.
    expect(container.textContent).toContain("Projects");
    expect(container.textContent).toContain("Oracle");

    // Oracle now appears as a nav button and navigates to the "oracle" view.
    const buttons = Array.from(container.querySelectorAll("button"));
    const oracleBtn = buttons.find((b) => b.textContent?.trim() === "Oracle");
    expect(oracleBtn).toBeTruthy();

    await act(async () => {
      oracleBtn!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(setActiveView).toHaveBeenCalledWith("oracle");
  });
});
