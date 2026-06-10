import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// Node-env render test (this repo's vitest has no jsdom): renderToStaticMarkup
// runs the render path WITHOUT effects/events, so the mount status-load never
// fires and the card renders in its initial (status === null) state. We assert
// the static structure here; the status->pill mapping, the import/remove gates,
// and the exact IPC command names/args are covered as pure helpers in
// githubCardModel.test.ts. Mock AppContext so no Tauri is touched.
const invokeMock = vi.fn(async () => null);

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: (...args: unknown[]) => invokeMock(...(args as [])),
}));

import { GithubProviderCard } from "./GithubProviderCard";

describe("GithubProviderCard", () => {
  it("renders the GitHub label and the keychain disclosure", () => {
    const html = renderToStaticMarkup(<GithubProviderCard />);
    expect(html).toContain("GitHub");
    expect(html).toContain("OS keychain");
    expect(html).toContain("Clone, Pull, and Push");
  });

  it("renders a write-only password token field with no value bound", () => {
    const html = renderToStaticMarkup(<GithubProviderCard />);
    // The token input is a password field (never shows the secret) and starts
    // empty — its value is local draft state, never read back from status.
    expect(html).toContain('type="password"');
    expect(html).toContain("Paste GitHub fine-grained token");
    // A write-only field renders with an empty value attribute (or none). It
    // must never echo a token; assert no value="ghp_..."-style binding exists.
    expect(html).not.toMatch(/value="gh[ps]_/);
  });

  it("shows the 'Checking auth' pill before the first status load", () => {
    // With effects skipped, status is null -> the checking pill is shown and the
    // gated Import/Disconnect buttons are absent.
    const html = renderToStaticMarkup(<GithubProviderCard />);
    expect(html).toContain("Checking auth");
    expect(html).not.toContain("Import from GitHub CLI");
    expect(html).not.toContain("Disconnect");
  });
});
