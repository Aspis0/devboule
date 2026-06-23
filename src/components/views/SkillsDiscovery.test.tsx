// @vitest-environment jsdom
//
// SkillsDiscovery — the bundled-skills one-click install list + the featured open-source
// marketplaces. `invoke` is a prop (plain vi.fn, no module mock), mirroring MarketplaceInstall.test.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { SkillsDiscovery } from "./SkillsDiscovery";
import type { LibraryCatalogEntry, FeaturedMarketplace } from "../../types/skills";

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
  vi.restoreAllMocks();
});

const LIB: LibraryCatalogEntry[] = [
  { name: "code-review", description: "Systematic diff review." },
  { name: "debugging", description: "Root-cause debugging." },
];
const FEAT: FeaturedMarketplace[] = [
  {
    name: "Anthropic Skills",
    url: "https://github.com/anthropics/skills",
    license: "Apache-2.0",
    description: "Official skills.",
  },
];

function makeInvoke(extra?: (cmd: string, args?: Record<string, unknown>) => unknown) {
  return vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "skills_library_catalog") return LIB;
    if (cmd === "skills_featured_marketplaces") return FEAT;
    return extra ? extra(cmd, args) : "/p/.claude/skills/code-review";
  });
}

async function mount(invoke: ReturnType<typeof makeInvoke>) {
  await act(async () => {
    root.render(createElement(SkillsDiscovery, { folderPath: "/p", invoke }));
  });
  // flush the mount effect's awaited fetches + the re-render they trigger.
  await act(async () => {
    await Promise.resolve();
  });
}

describe("SkillsDiscovery", () => {
  it("lists the bundled skills and featured marketplaces fetched on mount", async () => {
    const invoke = makeInvoke();
    await mount(invoke);
    expect(invoke).toHaveBeenCalledWith("skills_library_catalog");
    expect(invoke).toHaveBeenCalledWith("skills_featured_marketplaces");
    expect(container.textContent).toContain("code-review");
    expect(container.textContent).toContain("Systematic diff review.");
    expect(container.textContent).toContain("Anthropic Skills");
    expect(container.textContent).toContain("Apache-2.0");
  });

  it("installs a bundled skill with the project folder + skill name on click", async () => {
    const invoke = makeInvoke();
    await mount(invoke);
    const btn = [...container.querySelectorAll("button")].find(
      (b) => b.getAttribute("data-skill") === "code-review",
    ) as HTMLButtonElement;
    expect(btn).toBeTruthy();
    await act(async () => {
      btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const call = invoke.mock.calls.find((c) => c[0] === "skills_install_bundled_library");
    expect(call).toBeTruthy();
    expect(call![1]).toMatchObject({ workingFolderPath: "/p", skillName: "code-review" });
  });
});
