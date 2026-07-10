import { describe, it, expect } from "vitest";
import { ALPHA_HIDDEN_PROVIDERS } from "./alphaHidden";

describe("ALPHA_HIDDEN_PROVIDERS", () => {
  it("hides scaleway and cloudflare", () => {
    expect(ALPHA_HIDDEN_PROVIDERS.has("scaleway")).toBe(true);
    expect(ALPHA_HIDDEN_PROVIDERS.has("cloudflare")).toBe(true);
  });

  it("does not hide unrelated providers", () => {
    expect(ALPHA_HIDDEN_PROVIDERS.has("overview")).toBe(false);
    expect(ALPHA_HIDDEN_PROVIDERS.has("budget")).toBe(false);
  });

  it("filters a tabs array so hidden ids are dropped", () => {
    const tabs = [
      { id: "overview", label: "Overview" },
      { id: "cloudflare", label: "Cloudflare" },
      { id: "scaleway", label: "Scaleway / Compute" },
      { id: "budget", label: "Budget" },
    ];
    const visible = tabs.filter((t) => !ALPHA_HIDDEN_PROVIDERS.has(t.id));
    expect(visible.map((t) => t.id)).toEqual(["overview", "budget"]);
  });
});
