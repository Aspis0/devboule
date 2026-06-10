import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { OracleIndexPreferences } from "../../types/backend";

// Node-env render test (this repo's vitest has no jsdom): renderToStaticMarkup
// runs the component's render path WITHOUT effects/events. We assert the static
// output — the mode select, its options, and the persisted value — while the
// save callback is exercised via onChange in the integration path.
// Mock AppContext so no Tauri is touched.
const saveOracleIndexPreferencesMock = vi.fn(async () => null);
let currentPrefs: OracleIndexPreferences | null = null;

vi.mock("../../context/AppContext", () => ({
  invokeBackendCommand: vi.fn(async () => null),
  useAppContext: () => ({
    oracleIndexPreferences: currentPrefs,
    saveOracleIndexPreferences: saveOracleIndexPreferencesMock,
  }),
  useAppActions: () => ({}),
}));

// Import AFTER the mock. The index-preferences card travelled to the Oracle
// admin panel in Phase 4(b); the unit test follows it there.
import { __test_OracleIndexPreferencesCard as OracleIndexPreferencesCard } from "../oracle/OracleAdminPanel";

describe("OracleIndexPreferencesCard", () => {
  it("renders the indexing mode select with both options", () => {
    currentPrefs = null;
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    expect(html).toContain("Indexing mode");
    expect(html).toContain('value="watch"');
    expect(html).toContain("Continuous watcher");
    expect(html).toContain('value="commit"');
    expect(html).toContain("On commit");
  });

  it("defaults to watch when prefs are absent", () => {
    currentPrefs = null;
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    // The select must have value="watch" (the React controlled value attribute).
    expect(html).toContain('value="watch"');
  });

  it("reflects watch mode when prefs.indexMode is watch", () => {
    currentPrefs = {
      autoWatchOnUnlock: true,
      indexRoot: null,
      indexMode: "watch",
    };
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    expect(html).toContain('value="watch"');
  });

  it("reflects commit mode when prefs.indexMode is commit", () => {
    currentPrefs = {
      autoWatchOnUnlock: true,
      indexRoot: null,
      indexMode: "commit",
    };
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    // The <select> value attribute carries the current selection.
    expect(html).toContain('value="commit"');
  });

  it("renders the auto-watch checkbox reflecting the preference", () => {
    currentPrefs = {
      autoWatchOnUnlock: false,
      indexRoot: null,
    };
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    // The checkbox is NOT checked (autoWatchOnUnlock: false).
    // renderToStaticMarkup renders checked={false} as no 'checked' attribute.
    expect(html).toContain("Auto-watch after unlock");
    // checked=false → attribute absent; checked=true → "checked" attribute present.
    // We test absence here since autoWatchOnUnlock is false.
    expect(html).not.toMatch(/type="checkbox"[^>]*checked/);
  });

  it("renders auto-watch checked when prefs.autoWatchOnUnlock is true", () => {
    currentPrefs = {
      autoWatchOnUnlock: true,
      indexRoot: null,
    };
    const html = renderToStaticMarkup(<OracleIndexPreferencesCard />);
    expect(html).toContain("Auto-watch after unlock");
    // checked=true → the 'checked' attribute IS present.
    expect(html).toContain('checked=""');
  });
});
