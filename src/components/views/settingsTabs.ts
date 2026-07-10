// Pure, DOM-free mapping from a LEGACY Settings sub-tab id to the new Phase-5
// tab id. Phase 5 collapsed the five Settings tabs (account / secrets / devices /
// workspace / oracle) into four (account / providers / workspace / security), so
// every persisted deep-link or code path that still requests an old tab id must
// land on its canonical successor:
//
//   - "secrets"  → "security"   (Secrets moved under the Security tab)
//   - "devices"  → "security"   (Devices moved under the Security tab, admin-only)
//   - "oracle"   → "providers"  (FALLBACK for stale deep-links. Oracle LLM config
//                                 now lives on the standalone Oracle page inside
//                                 OracleAdminPanel. Direct callers like AskErrorCard
//                                 navigate to requestView("oracle") instead of
//                                 going through Settings; this mapping only handles
//                                 legacy persisted links that still target
//                                 settings#oracle.)
//   - "workspace"→ "workspace"  (workspace settings tab — unchanged)
//   - "account"  → "account"    (unchanged)
//
// NOTE: The Oracle ADMIN surface (index-root picker, doctor) is on the standalone
// Oracle VIEW — reachable via requestView("oracle"), NOT via Settings→workspace.
// The AskErrorCard "admin" action (goToAdmin in OracleAskPanel) calls
// requestView("oracle") directly. The legacy mapLegacyViewTarget redirect that
// used to map the "oracle" view to settings#workspace has been removed; "oracle"
// is a real top-level view again.
//
// Any unknown / empty id falls back to "account" (the default tab) so a stale or
// hand-built link can never leave Settings on no tab at all.
//
// This is applied INSIDE SettingsView's pendingTab effect, AFTER consumePendingTab.
// Pure (no React, no DOM) — unit-tested in settingsTabs.test.ts.

export type SettingsTabId =
  | "account"
  | "providers"
  | "workspace"
  | "security"
  | "dependencies";

// The legacy → new map. Listed explicitly (not derived) so a reviewer can read the
// full contract at a glance and a new legacy id is a deliberate addition.
const LEGACY_SETTINGS_TAB_MAP: Record<string, SettingsTabId> = {
  account: "account",
  secrets: "security",
  devices: "security",
  workspace: "workspace",
  oracle: "providers",
  // Phase-5 ids also map to themselves so a fresh link (already using the new ids)
  // is a no-op rather than falling through to the default.
  providers: "providers",
  security: "security",
  // TASK #13: the Dependencies tab also maps to itself (it is a genuine tab id, not
  // a legacy alias) so a deep-link using it is a no-op.
  dependencies: "dependencies",
};

export function mapLegacySettingsTab(old: string): SettingsTabId {
  const key = (old ?? "").trim();
  return LEGACY_SETTINGS_TAB_MAP[key] ?? "account";
}
