// @vitest-environment jsdom
//
// MarketplaceInstall — the owner-vetting surface for installing an external SKILL.md. Uses the repo's
// jsdom + createRoot + act pattern (mirrors SkillsView.test.tsx). `invoke` is a prop, so we drive the
// backend with a plain vi.fn — no module mock needed.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

import { MarketplaceInstall } from "./MarketplaceInstall";
import type { MarketplacePreview } from "../../types/skills";

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

function dangerPreview(): MarketplacePreview {
  return {
    name: "evil-skill",
    description: "does things",
    allowed_tools: "Bash(rm:*) Read",
    body_excerpt: "the body excerpt",
    findings: [{ code: "SG001", severity: "Danger", title: "Shell / code execution", evidence: "bash -c x" }],
    worst: "Danger",
    source_url: "https://m/SKILL.md",
    sha256: "abc123",
  };
}

function cleanPreview(): MarketplacePreview {
  return {
    name: "nice-skill",
    description: "safe",
    allowed_tools: null,
    body_excerpt: "body",
    findings: [],
    worst: null,
    source_url: "https://m/ok/SKILL.md",
    sha256: "deadbeef",
  };
}

function setUrl(value: string) {
  const input = container.querySelector('input[type="url"]') as HTMLInputElement;
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!;
  act(() => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function clickButton(text: string) {
  const btn = [...container.querySelectorAll("button")].find((b) => b.textContent?.includes(text));
  if (!btn) throw new Error(`button "${text}" not found`);
  return act(async () => {
    btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

function installButton(): HTMLButtonElement {
  return [...container.querySelectorAll("button")].find((b) =>
    b.textContent?.includes("Install skill"),
  ) as HTMLButtonElement;
}

describe("MarketplaceInstall", () => {
  it("previews, shows risk findings + worst severity, and defaults the install name", async () => {
    const invoke = vi.fn(async () => dangerPreview());
    act(() => root.render(createElement(MarketplaceInstall, { folderPath: "/p", invoke })));
    setUrl("https://m/SKILL.md");
    await clickButton("Preview");

    expect(invoke).toHaveBeenCalledWith("skills_marketplace_preview", { url: "https://m/SKILL.md" });
    expect(container.textContent).toContain("Shell / code execution");
    expect(container.textContent).toContain("SG001");
    expect(container.textContent).toContain("worst: Danger");
    expect(container.textContent).toContain("Bash(rm:*) Read"); // allowed-tools permission summary
    const nameInput = container.querySelector('input[aria-label="Install skill name"]') as HTMLInputElement;
    expect(nameInput.value).toBe("evil-skill");
  });

  it("gates a Danger install behind the acknowledgement checkbox", async () => {
    const invoke = vi.fn(async (cmd: string) =>
      cmd === "skills_marketplace_preview" ? dangerPreview() : "/p/.claude/skills/evil-skill",
    );
    act(() => root.render(createElement(MarketplaceInstall, { folderPath: "/p", invoke })));
    setUrl("https://m/SKILL.md");
    await clickButton("Preview");

    expect(installButton().disabled).toBe(true); // Danger ⇒ blocked until ack

    const ack = container.querySelector('input[type="checkbox"]') as HTMLInputElement;
    act(() => {
      ack.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(installButton().disabled).toBe(false);
  });

  it("installs with the previewed sha (so a swapped payload is caught) + a timestamp", async () => {
    const invoke = vi.fn(async (cmd: string, _args?: Record<string, unknown>) =>
      cmd === "skills_marketplace_preview" ? cleanPreview() : "/p/.claude/skills/nice-skill",
    );
    act(() => root.render(createElement(MarketplaceInstall, { folderPath: "/proj", invoke })));
    setUrl("https://m/ok/SKILL.md");
    await clickButton("Preview");
    // No Danger ⇒ install enabled immediately.
    expect(installButton().disabled).toBe(false);
    await clickButton("Install skill");

    const call = invoke.mock.calls.find((c) => c[0] === "skills_marketplace_install");
    expect(call).toBeTruthy();
    const args = call![1] as Record<string, unknown>;
    expect(args.workingFolderPath).toBe("/proj");
    expect(args.skillName).toBe("nice-skill");
    expect(args.expectedSha256).toBe("deadbeef");
    expect(args.url).toBe("https://m/ok/SKILL.md");
    expect(typeof args.fetchedAt).toBe("string");
    expect(container.textContent).toContain("Installed to");
  });

  it("surfaces a backend error from preview", async () => {
    const invoke = vi.fn(async () => {
      throw new Error("refusing to fetch from a private/loopback address");
    });
    act(() => root.render(createElement(MarketplaceInstall, { folderPath: "/p", invoke })));
    setUrl("https://10.0.0.1/SKILL.md");
    await clickButton("Preview");
    expect(container.textContent).toContain("private/loopback");
  });
});
