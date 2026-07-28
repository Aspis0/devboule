# Oracle Consultation — Milestone A (`bundle.windows` block + smoke test)

**Verdict**: GREEN with ONE mandatory correction (test path bug) + 2 optional hardening suggestions. The pre-approved JSON block is correct and minimal; no fields are missed. Milestone A requires **zero** `Cargo.toml` edits (config + test file only).

**Output path**: `...\a3a645e7-...\oracle\decision-milestone-a.md`

## Inherited decisions (baseline contract)
- **SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone A; Amendment 1 §C cleared the `TODO(verify)` on the `bundle.windows` schema.
- **Pre-approved shape**: `wix:{}`, `nsis.installMode:"perMachine"`, `webviewInstallMode:{type:"downloadBootstrapper",silent:true}`.
- **Locked (untouched by A)**: `windows = "0.58"`, raw `JobObjects` API, Windows Hello `UserConsentVerifier`, DXGI+WARP GPU filter.
- **Hard routing**: coding → `worker` (hy3); A is config + test (code), no setup/install → hy3 envelope is fine.
- **A scope**: `tauri.conf.json` edit + one new test file. No Rust source, no `Cargo.toml`.

## Repo facts verified (read-only)
- `src-tauri/tauri.conf.json`: `bundle` block present (`active:true`, `targets:"all"`, `icon[]`, `resources[]`, `externalBin:["binaries/devboule-mcp"]`) — **NO** `windows` subkey. Matches plan.
- `src-tauri/tests/` **does not exist** → created by adding the test file (no `.gitkeep` needed).
- `src-tauri` is **lib+bin**: `[lib] name="devboule_lib"` (crate-type `lib`/`cdylib`/`staticlib`) + `[[bin]] devboule` (src/main.rs) + `[[bin]] claude_consent_hook`. → `tests/` integration tests compile against `devboule_lib` — fully supported.
- `serde_json = { version="1", features=["preserve_order"] }` is in `[dependencies]`; **no `[dev-dependencies]`** section exists. → the smoke test uses `serde_json` with **zero** `Cargo.toml` changes.
- **No root workspace** (`Cargo.toml` absent at repo root). → `src-tauri` is the package root; **integration-test CWD = `src-tauri/`**.
- `@tauri-apps/cli ^2.0.0` is a `package.json` devDependency → `npx tauri info` available for schema validation.

## Drift / contradiction check — ONE mandatory correction

### BLOCKER — test file-path bug (severity: high; fails the test at runtime)
The plan's test (`PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone A snippet) reads:
    std::fs::read_to_string("src-tauri/tauri.conf.json").unwrap()
For an integration test in package `devboule`, Cargo sets the working directory to the **package root** = `src-tauri/` (no root workspace to anchor higher). The relative path `"src-tauri/tauri.conf.json"` therefore resolves to `src-tauri/src-tauri/tauri.conf.json` → **does not exist** → `read_to_string(...).unwrap()` panics → the test fails at runtime even with a valid config. The JSON is fine; only the test path is wrong.

**Fix the worker MUST apply** (do not copy the plan's path verbatim):
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
`CARGO_MANIFEST_DIR` is absolute and set at compile time → robust regardless of where `cargo test` is invoked. (A bare `"tauri.conf.json"` would also work given CWD=`src-tauri`, but the `CARGO_MANIFEST_DIR` form is bulletproof.)

## Answers to the 7 questions

### 1. Validate JSON block — any missed field?
**No mandatory field is missed.** The block is correct and minimal. Optional fields and safe defaults for devboule (single maintainer, no code-sign cert):

| Field | Default if absent | Recommendation |
|---|---|---|
| `certificateThumbprint` | none → unsigned | **Leave absent** (no cert yet) |
| `signCommand` | none | **Leave absent** — plan's `TODO(verify)` is correct; add only when a cert path is decided |
| `digestAlgorithm` | `sha256` | leave absent (default fine) |
| `timestampUrl` | `http://timestamp.digicert.com` | leave absent (no signing) |
| `wix.language` | `["en-US"]` | leave `wix:{}` (default fine); add explicitly only if desired |
| `nsis.languages` | `["English"]` | leave as-is; optional explicit `["English"]` |
| `webviewFixedRuntime` | n/a | only relevant if `type:"fixedRuntime"` — not chosen |

Keep the block exactly as pre-approved. **Confirmed: `signCommand` / `certificateThumbprint` must stay ABSENT (null default = unsigned installer)**, acceptable for the no-cert dev stage. Adding unused optional fields is noise.

### 2. `perMachine` vs `currentUser`
**Keep `perMachine`** (honors the locked plan decision; coherent with `downloadBootstrapper`).
- `tauri#13926` (WebView2 breaks under an **elevated app process**) is **NOT triggered**: install scope and process elevation are independent. The installed app launches non-elevated; only the installer UAC-prompts once. The task's framing is correct.
- Genuine tradeoff: `perMachine` → UAC at install + `Program Files` + heavier SmartScreen on an unsigned installer; `currentUser` (Tauri default) → no UAC, `%LOCALAPPDATA%`, lower dev-iteration friction.
- For a single-maintainer no-cert dev app, `currentUser` is the lower-friction default — BUT Amendment 1 §C explicitly weighed this and still chose `perMachine`. **No evidence to override the locked decision.**
- **Reversibility**: the smoke test does **not** assert `installMode`, so flipping to `currentUser` later is a one-line, test-safe change. Not a blocker either way. Ship `perMachine` now; revisit only if UAC friction during dev reinstalls becomes painful.

### 3. Test file location
**`src-tauri/tests/tauri_conf_windows.rs` works.** `devboule` is lib+bin, so `tests/` integration tests compile against `devboule_lib`. The test imports only `std` + `serde_json` (a normal dep) → no `[dev-dependencies]` addition, no `Cargo.toml` change. No need for a unit test inside `src/`. **Only the path bug above must be fixed.**

Optional hardening (not blocking): the plan validates `webviewInstallMode.type` only *if* the object is present (`if let Some`). Since A1 explicitly adds it, assert presence strictly:
    assert!(v["bundle"]["windows"]["webviewInstallMode"].is_object());

### 4. Verification commands (after the worker lands A)
1. **Primary gate (the smoke test itself)**:
   `cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows`
2. **Host sanity (fast)**:
   `cargo check --manifest-path src-tauri/Cargo.toml`
   (Note: `cargo check` does **not** validate `tauri.conf.json` — near-no-op for config-only changes; still a fine sanity gate that nothing else broke.)
3. **Schema validation (optional — Tauri CLI present as `@tauri-apps/cli ^2.0.0` devDep)**:
   `npx tauri info` — loads and schema-validates `tauri.conf.json` on the host (cross-platform; surfaces any malformed `bundle.windows` field).
   Do **NOT** run `tauri build` as an A gate — that belongs to the milestone that actually produces installers and needs the target-suffixed `binaries/devboule-mcp-x86_64-pc-windows-msvc.exe` (see Risks).

### 5. Commit message shape
**`build(bundle):`** — most semantically accurate: packaging/build config, no app behavior or Rust code change. (`feat(bundle):` is acceptable if you want "Windows installer support" to count as a version-bumping milestone, but for pre-release dev-stage config, `build` avoids a spurious semver bump.)

Title + body (worker to use verbatim):
    build(bundle): add Windows installer config (nsis perMachine + webview2 bootstrapper)

    - Add bundle.windows to tauri.conf.json: nsis perMachine, webviewInstallMode
      downloadBootstrapper/silent, empty wix (defaults). No code-sign fields (no cert yet).
    - Add smoke test src-tauri/tests/tauri_conf_windows.rs asserting bundle.active,
      bundle.windows object, webviewInstallMode.type enum, bundle.targets=="all".
    - Config + test only; no Rust source or Cargo.toml changes. macOS bundling
      unaffected (bundle.windows ignored on non-Windows targets).
    - Milestone A of specs/PORT_MACOS_TO_WINDOWS_FINAL.md.

### 6. Risk — macOS bundle regression
**Low.** `bundle.windows` is read only for Windows targets; ignored on macOS. `targets:"all"` is already present and unchanged. No macOS key is touched.

Pre-existing items **not** caused by A (out of scope, do **not** block A, but worth a glance):
- `bundle.icon` contains `"icons/[EMAIL]"` — and a file literally named `[EMAIL]` exists in `src-tauri/icons/`. Looks like an unsubstituted template placeholder that became a real file. Housekeeping for a future cleanup; A does not touch icons.
- `externalBin: ["binaries/devboule-mcp"]` — on Windows, Tauri requires a target-suffixed `binaries/devboule-mcp-x86_64-pc-windows-msvc.exe`. Will fail `tauri build` on Windows if absent. **Not an A gate** (A does not build); flag for the milestone that runs the actual Windows bundle.
- `resources: ["../oracle", "../pi-sidecar/*"]` — relative paths outside `src-tauri`; must exist at Windows bundle time. Same: not an A gate.

### 7. Gitignore
**No `.gitkeep` needed.** Git tracks files, not directories. Adding `tauri_conf_windows.rs` into `src-tauri/tests/` makes git track the directory. `.gitkeep` is only for intentionally-empty dirs. The test file alone is sufficient.

## Recommendation
Proceed with Milestone A via `worker` (hy3) — pure config + one test file, no setup/install, squarely in hy3's working envelope. **Mandatory**: have the worker apply the test-path fix (`CARGO_MANIFEST_DIR` join) — do **not** copy the plan's `read_to_string("src-tauri/tauri.conf.json")` verbatim or the test will fail at runtime. Optionally apply the strict `webviewInstallMode` presence assertion. Everything else ships as pre-approved.

## Risks (residual)
- If the worker copies the plan's test verbatim → runtime test failure (mitigated by the mandatory path fix above).
- `perMachine` unsigned installer will trigger SmartScreen + UAC on first install — expected for the no-cert dev stage, not a regression.
- `externalBin` target-suffix + out-of-tree resources will block the *later* Windows `tauri build`, not A.
- No schema-validation in CI yet (Milestone H adds CI); until then `npx tauri info` is manual-only.

## Need from main agent
None — all 7 questions are answerable from verified repo facts + the locked plan. No pivot required.

## Suggested execution prompt (worker handoff IS warranted)
    Milestone A — bundle.windows config + smoke test. Config + test file ONLY; no Cargo.toml or Rust source edits.

    1. Edit src-tauri/tauri.conf.json: add a "windows" subkey to the existing "bundle" object (after "externalBin"):
       "windows": {
         "wix": {},
         "nsis": { "installMode": "perMachine" },
         "webviewInstallMode": { "type": "downloadBootstrapper", "silent": true }
       }
       Do NOT add certificateThumbprint/signCommand/timestampUrl (no cert yet). Do NOT touch icons/resources/externalBin.

    2. Create src-tauri/tests/tauri_conf_windows.rs with ONE test tauri_conf_json_has_windows_bundle_block that:
       - reads tauri.conf.json via std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json")
         (NOT "src-tauri/tauri.conf.json" — CWD is already src-tauri; that path double-prefixes and fails)
       - parses with serde_json (already a dependency; no Cargo.toml change)
       - asserts: bundle.active==true; bundle.windows.is_object();
         bundle.windows.webviewInstallMode.is_object() and its "type" in
         {downloadBootstrapper,embedBootstrapper,offlineInstaller,fixedRuntime,skip};
         bundle.targets=="all"

    3. Verify: cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows  (must pass)
               npx tauri info  (must load config without schema error)

    4. Commit with:
       build(bundle): add Windows installer config (nsis perMachine + webview2 bootstrapper)
       (+ the 5-line body from the oracle report)

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete findings with file paths + severities: BLOCKER (high) at PORT_MACOS_TO_WINDOWS_FINAL.md §Milestone A test snippet — relative path 'src-tauri/tauri.conf.json' double-prefixes under CWD=src-tauri (no root workspace) and panics at runtime; fix = CARGO_MANIFEST_DIR join. Verified src-tauri/tauri.conf.json (no windows subkey), src-tauri/tests/ (absent), src-tauri/Cargo.toml ([lib] devboule_lib + 2 [[bin]], serde_json in [dependencies], no [dev-dependencies]), repo root (no Cargo.toml workspace), package.json (@tauri-apps/cli ^2.0.0 devDep), src-tauri/icons/ (anomalous '[EMAIL]' file)."
    }
  ],
  "changedFiles": [],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    { "command": "read src-tauri/tauri.conf.json", "result": "passed", "summary": "bundle block present, no windows subkey; matches plan" },
    { "command": "ls src-tauri/tests/ + src/lib.rs + src/main.rs + src/", "result": "passed", "summary": "tests/ absent; lib.rs(44KB)+main.rs present -> lib+bin package" },
    { "command": "grep specs for Milestone A / bundle.windows / webviewInstallMode", "result": "passed", "summary": "FINAL §Milestone A + Amendment 1 §C cleared block located" },
    { "command": "grep src-tauri/Cargo.toml ([package]/[lib]/[[bin]]/serde_json)", "result": "passed", "summary": "lib+bin, serde_json in [dependencies], no [dev-dependencies] -> zero Cargo.toml edits for A" },
    { "command": "ls repo-root Cargo.toml + grep package.json tauri + grep dev-deps + ls icons", "result": "passed", "summary": "no root workspace (test CWD=src-tauri); @tauri-apps/cli present; [EMAIL] icon file anomaly noted" },
    { "command": "cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows", "result": "not-run", "summary": "worker's post-implementation gate; not executed by oracle (read-only)" }
  ],
  "validationOutput": [
    "JSON block: correct + minimal; no missed mandatory fields. signCommand/certificateThumbprint correctly absent (unsigned, no cert).",
    "Test location src-tauri/tests/tauri_conf_windows.rs valid (lib+bin crate; serde_json available w/o dev-dep).",
    "MANDATORY correction: test must read via env!(CARGO_MANIFEST_DIR).join(tauri.conf.json), NOT 'src-tauri/tauri.conf.json' (CWD=src-tauri -> double-prefix -> runtime panic).",
    "perMachine: keep (locked, tauri#13926 not triggered); reversible, test does not assert installMode.",
    "Commit: build(bundle): (config/packaging, no semver bump). No .gitkeep needed. macOS unaffected."
  ],
  "residualRisks": [
    "Worker copying plan's test verbatim -> runtime test failure (mitigated by mandatory CARGO_MANIFEST_DIR path fix).",
    "perMachine unsigned installer -> SmartScreen + UAC on first install (expected dev-stage, not a regression).",
    "externalBin binaries/devboule-mcp lacks target-suffix (.exe) -> will block later Windows tauri build, not A.",
    "Out-of-tree resources (../oracle, ../pi-sidecar/*) must exist at Windows bundle time -> not an A gate.",
    "No CI schema validation until Milestone H; npx tauri info is manual-only until then.",
    "Pre-existing icon anomaly 'icons/[EMAIL]' (real file) -> housekeeping, out of A scope."
  ],
  "noStagedFiles": true,
  "diffSummary": "No diff — read-only oracle consultation; no repo files changed. One mandatory correction (test path: use CARGO_MANIFEST_DIR, not 'src-tauri/tauri.conf.json') + one optional hardening (strict webviewInstallMode presence assert) identified for the worker. JSON block ships as pre-approved; zero Cargo.toml edits.",
  "reviewFindings": [
    "blocker: PORT_MACOS_TO_WINDOWS_FINAL.md §Milestone A test snippet — read_to_string('src-tauri/tauri.conf.json') double-prefixes under CWD=src-tauri (no root workspace) and panics at runtime. Fix: Path::new(env!(CARGO_MANIFEST_DIR)).join('tauri.conf.json').",
    "suggestion (low): make webviewInstallMode presence a strict assert (plan only checks type if object present).",
    "info: no missed JSON fields; signCommand/certificateThumbprint correctly left absent (no cert).",
    "info: perMachine kept (locked); tauri#13926 not triggered (install scope != process elevation).",
    "info (out-of-scope): icons/[EMAIL] anomaly + externalBin target-suffix will matter for the Windows tauri build milestone, not A."
  ],
  "manualNotes": "Milestone A is config+test only and needs ZERO Cargo.toml edits (serde_json already a normal dep). Hand off to worker(hy3) with the mandatory path fix baked into the prompt. Do NOT run tauri build as an A gate. Commit type build(bundle): preferred over feat to avoid a spurious semver bump for pre-release packaging config."
}
```
