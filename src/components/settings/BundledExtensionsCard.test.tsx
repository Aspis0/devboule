// Tests for BundledExtensionsCard — the settings card showing status and basic
// config for bundled pi extensions.
//
// Four npm-installed rows: Subagents (with agent list panel), pi-lens,
// Compactor, Web search (status-only + hint to Web search settings above).

import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// ---------------------------------------------------------------------------
// Mocks (must precede component import)
// ---------------------------------------------------------------------------

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: async (..._args: unknown[]) => undefined,
}));

import { BundledExtensionsCard } from "./BundledExtensionsCard";

// ---------------------------------------------------------------------------
// Static render — loading state (default before effects run in SSR)
// ---------------------------------------------------------------------------

describe("BundledExtensionsCard", () => {
  it("shows loading text before data loads", () => {
    const html = renderToStaticMarkup(<BundledExtensionsCard />);
    expect(html).toContain("Loading extension info");
  });

  it("renders the header and subtitle", () => {
    const html = renderToStaticMarkup(<BundledExtensionsCard />);
    expect(html).toContain("Bundled extensions");
    expect(html).toContain("Extensions that ship with the app");
    expect(html).toContain("installed automatically on first launch");
  });

  it("has data-help-title and data-help-lines", () => {
    const html = renderToStaticMarkup(<BundledExtensionsCard />);
    expect(html).toContain("data-help-title=");
    expect(html).toContain("data-help-lines=");
    expect(html).toContain("Bundled extensions");
  });

  it("uses the puzzle icon", () => {
    const html = renderToStaticMarkup(<BundledExtensionsCard />);
    expect(html).toContain("lucide-puzzle");
  });

  it("help-lines mention Web search links to settings above", () => {
    const html = renderToStaticMarkup(<BundledExtensionsCard />);
    expect(html).toContain("Web search links to the Web search settings section above");
  });
});
