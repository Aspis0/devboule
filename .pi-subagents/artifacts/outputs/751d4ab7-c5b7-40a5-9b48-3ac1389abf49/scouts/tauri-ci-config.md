# Tauri / Frontend / CI / Config — Surface Map

## Files Retrieved

1. `src-tauri/tauri.conf.json` (lines 1-57) — Tauri 2 config: app metadata, build hooks, window, CSP, bundle
2. `package.json` (lines 1-80) — npm scripts, dependencies, devDependencies
3. `src-tauri/Cargo.toml` (lines 1-120+) — Rust crate config: binaries, features, deps, Windows cfg-gated deps
4. `vite.config.ts` (lines 1-56) — Vite bundler config (port 1420, rollup manualChunks)
5. `vitest.config.ts` (lines 1-27) — Vitest runner config (pool: forks, excludes design/flaky by default)
6. `src-tauri/src/main.rs` (lines 1-17) — `windows_subsystem = "windows"` cfg, headless CLI bridges
7. `src-tauri/src/lib.rs` (lines 1-500+) — Tauri builder, 150+ commands, `run()` setup
8. `src-tauri/build.rs` (lines 1-2) — `tauri_build::build()` only
9. `src-tauri/tauri.pilot.conf.json` (lines 1-8) — Pilot capability (dev-only, .gitignored)
10. `src-tauri/capabilities/default.json` (lines 1-13) — Permissions: core, dialog, notification
11. `src-tauri/.taurignore` (lines 1-5) — Watcher exclusions for config.json, projects/, gen/schemas/
12. `src-tauri/binaries/README.md` — placeholder (no staged binaries committed)
13. `scripts/stage-oracle-embedder.sh` (lines 1-180+) — Model staging for bundle (lite/full)
14. `scripts/stage-devboule-mcp.sh` (lines 1-103) — MCP binary staging for externalBin
15. `oracle-core/Cargo.toml` (lines 1-70) — Oracle Engine: Windows `ort` with `directml` feature
16. `devboule-mcp/Cargo.toml` (lines 1-30) — MCP server: no Windows-specific deps
17. `.gitignore` (lines 1-100+) — Ignores: node_modules, target, .github (no CI!), .pi/* except agents
18. `.oracleignore` (lines 1-50+) — Oracle indexer ignore policy
19. `rig/README.md` (lines 1-50) — Self-test rig docs: pytest Layer A + Rust Layer B
20. `README.md` (lines 1-40) — Project README: macOS primary, "Windows supported; being tested shortly"

## Key Code

### tauri.conf.json — Bundle section (no Windows installer format specified)
```json
"bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
    "resources": ["../oracle", "resources/censor/semgrep-rules.yml", "resources/oracle-models", "../pi-sidecar/*"],
    "externalBin": ["binaries/devboule-mcp"]
}
```
Line 43-52 — No `windows` sub-key (no wix/nsis/msi config)

### Build command chain (tauri.conf.json lines 21-24)
```json
"beforeBuildCommand": "bash -c 'if [ \"${DEVBOULE_BUNDLE_ORACLE_EMBEDDER:-}\" = 1 ]; then bash scripts/stage-oracle-embedder.sh --full; else bash scripts/stage-oracle-embedder.sh --lite; fi' && bash scripts/stage-devboule-mcp.sh && npm run build"
```
Runs three stages: (1) Oracle model staging (full/lite), (2) MCP binary build+stage, (3) Frontend build

### Windows cfg in main.rs (line 1)
```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
```
Suppresses console window on packaged Windows builds.

### Windows cfg-gated dependencies (Cargo.toml lines 105-113)
```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = ["Foundation", "Security_Credentials_UI", ...] }
webview2-com = "=0.38.2"
windows_capture = { package = "windows", version = "=0.61.3", features = ["Win32_Foundation", "Win32_System_Com", "Win32_System_Com_StructuredStorage"] }
```
Also `oracle-core/Cargo.toml` line 52-55: `ort` with `directml` for Windows GPU.

### Windows test modules (key locations)
- `backend/auth.rs:417` — `#[cfg(all(test, target_os = "windows"))] mod tests` (Windows Hello)
- `backend/agent_spawn.rs:1696` — `#[cfg(test)] mod he_security_tests` (includes Windows tests)
- `backend/agent_spawn.rs:1846-1937` — 3 `#[cfg(windows)] #[test]` for script safety
- `backend/agent_pty.rs:914-1026` — 2 `#[cfg(windows)] #[test] #[ignore]` for PTY
- `backend/agents.rs:1452` — `#[cfg(any(windows, test))]` helper for title matching

### Package scripts for test/build
```json
"test": "vitest run",
"test:design": "VITEST_INCLUDE_DESIGN=1 vitest run src/components/design",
"test:all": "VITEST_INCLUDE_DESIGN=1 vitest run",
"rig:smoke": "RIG=1 oracle-data/venv/bin/python -m pytest rig/ -v",
"rig:rust": "cargo test --manifest-path src-tauri/Cargo.toml rig_tests -- --ignored",
"build": "tsc && vite build",
"tauri": "tauri"
```
Lines 11-27 — No `cargo check`, `cargo clippy`, `cargo test --all`, or `cargo deny` scripts.

## Architecture

```
[ Developer ]
    │
    ├── npm run dev          → vite dev server (:1420) — hot-reload frontend
    ├── npm run build        → tsc + vite build → dist/
    ├── npm run tauri dev    → tauri dev (cargo run from src-tauri/ + vite)
    ├── npm run tauri build  → tauri build (oracle staging → MCP build → frontend build → rust build → bundle)
    ├── npm test             → vitest (frontend unit tests, excludes design by default)
    └── npm run rig          → pytest Layer A + cargo test rig_tests -- --ignored Layer B

[ Packaging (tauri build) ]
    Scripts called by beforeBuildCommand:
    1. scripts/stage-oracle-embedder.sh [--lite|--full]   → src-tauri/resources/oracle-models/
    2. scripts/stage-devboule-mcp.sh                       → src-tauri/binaries/devboule-mcp-<triple>
    3. npm run build                                       → dist/

[ Rust test surface ]
    ├── cargo test          → inline #[cfg(test)] modules (~100+ test modules across backend/, polis/, oracle/)
    ├── cargo test -- --ignored → slow/live/integration tests
    └── cargo test rig_tests -- --ignored → Layer B rig integration tests

[ CI ] → NONE. No .github/ directory, no GitLab CI, no Jenkinsfile, no Buildkite.

[ Windows cfg surface ]
    - main.rs: windows_subsystem (release)
    - Cargo.toml: cfg(windows) deps (windows 0.58, webview2-com, windows_capture)
    - oracle-core: cfg(windows) ort+directml
    - 80+ #[cfg(windows)] sites in: auth.rs, agents.rs, agent_spawn.rs, agent_pty.rs,
      agentic_tools.rs, api_fuzz.rs, changes.rs, cloud_duplex.rs, design_generate.rs,
      oracle/python_oracle.rs, polis/commands.rs, polis/scanner.rs
```

## Gaps (Lacune)

### A (Auto/CI) — CRITICAL
- **No CI/CD configuration exists at all**: no `.github/`, no GitLab CI, no Buildkite, no Jenkinsfile
- No automated test runner on push/PR
- No automated build verification
- No automated tauri build validation
- No dependency vulnerability scanning (`cargo deny`, `npm audit`)
- No clippy check gate
- No MSRV check

### H (Hardening) — MODERATE
- No `src-tauri/tests/` integration test directory (missing at project root too)
- No `cargo clippy` script in package.json
- No `cargo deny` / advisory checking
- No MSRV (minimum supported Rust version) documented
- No pre-commit hooks (`husky`, `lint-staged`, etc.)
- Bundle targets `"all"` — on Windows, Tauri defaults to MSI/NSIS depending on Tauri version; no explicit config
- No Windows-specific installer customization (WiX config, signing, etc.)
- No cross-platform build matrix validation
- `npm test` excludes design tests by default (VITEST_INCLUDE_DESIGN not set) and excludes flaky RolesTableCard

### Ort (Orchestration/Release) — LOW-MODERATE
- No release workflow (`tauri build` must be run manually)
- No CHANGELOG or automated version management
- No Docker/container config
- No automated bundle upload/distribution
- No smoke test after build

## Already Shipped / Working

- **Windows-specific Rust code**: extensive `#[cfg(windows)]` surface already implemented and compiling
- **Windows Hello authentication**: full implementation in `backend/auth.rs` with helper binary pattern
- **Windows process management**: taskkill /T, process verification, PTY, window focus via EnumWindows
- **Windows GPU support**: DirectML via ort in oracle-core
- **Windows console suppression**: `windows_subsystem = "windows"` on release builds
- **Bundle resource staging**: oracle embedder (lite/full modes) + MCP binary auto-staging in beforeBuildCommand
- **Frontend test framework**: Vitest configured with jsdom, pool: forks, 20s timeout, global teardown
- **Rust test framework**: extensive inline tests, separate rig integration test files
- **Tailwind CSS**: configured via tailwind.config.js + postcss.config.js
- **TypeScript strict mode**: enabled in tsconfig.json
- **CSP**: configured in tauri.conf.json with allowlist for external hosts in lib.rs

## Start Here

Open `src-tauri/Cargo.toml` to understand the Rust crate layout (two binaries, one lib, Windows cfg deps). Then `package.json` for build/test scripts. Then `src-tauri/tauri.conf.json` for bundle/build config. The CI gap is the single most important finding — there is zero automation.

## Verification Commands

| Command | What it checks | Reliable? |
|---------|---------------|-----------|
| `cargo check --manifest-path src-tauri/Cargo.toml` | Rust compiles | ✅ |
| `cargo clippy --manifest-path src-tauri/Cargo.toml` | Rust lint | ✅ |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Rust unit tests | ✅ |
| `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored` | Rust slow/live tests | ✅ |
| `npm test` | Frontend vitest | ✅ |
| `npm run test:all` | Frontend vitest (incl. design) | ✅ (may hang see vitest.config notes) |
| `npm run rig:rust` | Rust rig integration | ✅ |
| `npm run build` | Frontend typecheck + bundle | ✅ |
| `npx tauri build` | Full Tauri build + bundle | ✅ (needs all deps) |
| `ls .github/` | CI config exists | ✅ (`No such file or directory` = lacuna) |
| `ls src-tauri/tests/` | Integration tests exist | ✅ (`No such file or directory` = lacuna) |

## Residual Risks

1. **No CI = any PR merges untested**. Single highest priority gap.
2. **Bundle.targets = "all"**: ambiguous on Windows (Tauri 2 defaults to MSI but NSIS is common); no explicit windows config means installer format is whatever Tauri version defaults to. Could break on Tauri minor updates.
3. **No MSRV policy**: `oracle-core` uses `ort = "2.0.0-rc.10"` which may require recent Rust; `windows = "0.58"` may also require specific rustc. A CI runner with stable but old rustc could fail to build.
4. **beforeBuildCommand is bash-only**: does not run on Windows unless Git Bash or WSL is available. `npm run build` is the only Windows-safe segment. The oracle/MCP staging scripts assume bash.
5. **npm run rig:smoke hardcodes Python path**: `oracle-data/venv/bin/python` — does not exist without previous `install_oracle_runtime` setup. Will fail on a fresh checkout or CI runner.
