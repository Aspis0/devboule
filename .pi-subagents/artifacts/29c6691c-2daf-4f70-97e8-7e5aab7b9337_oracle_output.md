I have all the evidence needed. Every external claim verified against crates.io/docs.rs APIs, and the codebase cross-checks reveal that 5 of the plan's 8 milestones describe work already shipped in devboule. Here is the structured report.

---

## A. Verdict
**GO-WITH-AMENDMENTS.** The plan's external research is accurate and the amendment is sound, but 5 of 8 milestones (B, D, E, F2, keyring) duplicate already-shipped code, two propose conflicting/worse APIs, and the plan entirely misses an existing `windows = 0.58` pin that its `0.62` snippet would collide with.

## B. Blockers (must-fix before code)
1. **`windows` crate version conflict — unaddressed** — `src-tauri/Cargo.toml:152` (`windows = "0.58"`) + `:164` (`windows_capture` = package `windows` `=0.61.3`) vs plan §4.1 proposing a fresh `windows = "0.62"` block. Decide: bump 0.58→0.62 (re-audit every call site in `auth.rs`/`hardware.rs`/webview2 boundary) or extend the existing 0.58 block with the missing features (`Win32_System_JobObjects`, `Win32_Security`, `Win32_NetworkManagement_WindowsFilteringPlatform`). A blind merge introduces a 3rd `windows` version and breaks shipped Windows auth/GPU.
2. **Milestone B proposes a *different, worse* editor API than shipped** — `src-tauri/src/polis/commands.rs:2250` & `:2268` already ship Windows arms (`notepad`, `explorer /select,"..."` with the `raw_arg` quoting FIX 5 at `:2308+`); plan §2.6 snippet (`cmd /c start ""` + unquoted `/select,`) would **regress** the spaced-path fix. Trim Milestone B to "verify existing".
3. **Milestone D proposes a *conflicting* Windows Hello API than shipped** — `src-tauri/src/backend/auth.rs:38-101` ships `hello_available`/`verify_user`/`WinRtGuard` (`:329`) via `UserConsentVerifier::CheckAvailabilityAsync()` (`:67`); plan §2.4 proposes `KeyCredentialManager::IsSupportedAsync()` — a different API (passwordless-key creation, not owner-consent). Milestone D is duplicate + conflict; delete or rewrite as "verify existing".
4. **Milestone E duplicates shipped DXGI code and drops the WARP-skip** — `src-tauri/src/backend/hardware.rs:324` already ships `#[cfg(windows)] detect_gpu()` (incl. `is_software_adapter()` WARP filter at `:115`); plan §2.3 snippet (`IDXGIAdapter1`/`GetDesc1`, no WARP filter) regresses the deliberate base-variant design. Trim to "verify existing".
5. **Plan §9 line citations are stale** — `is_enforced()` is at `mod.rs:207` not `:217`; plan cites `auth.rs:78-100` as the *insertion* point for code that already lives at `:38`. Indicates the plan was not re-synced to HEAD before sign-off.

## C. Already-shipped in devboule (plan = duplication)
1. **`notepad_argv`/`explorer_argv` Windows arms + platform test** at `src-tauri/src/polis/commands.rs:2250`, `:2268`, `:2933` — Milestone B is done (and better than the plan's snippet).
2. **Windows Hello `hello_available`/`verify_user`/`check_hello_available`/`verify_user_inner`/`WinRtGuard`** at `src-tauri/src/backend/auth.rs:38`, `:55`, `:65`, `:99`, `:143`, `:329` — Milestone D is done.
3. **DXGI `detect_gpu` (Windows)** at `src-tauri/src/backend/hardware.rs:324` (WARP-skip at `:115`) — Milestone E is done.
4. **DirectML EP selection + ort Windows-target dep** at `oracle-core/Cargo.toml:54-57` and `oracle-core/src/embed/ort_backend.rs` `default_ep()` (`EpArg::Directml`) — Milestone F2 is done; only the rc.10→rc.12 bump (amendment §A) is real work.
5. **`keyring` with `windows-native`** at `src-tauri/Cargo.toml:55` — amendment §D "no change needed" is correct; the plan never had a keychain milestone anyway.

## D. Confirmed via websearch/curl
1. **`ort 2.0.0-rc.12` is the latest RC and declares BOTH `directml` and `coreml` features** (`coreml=["ort-sys/coreml"]`, `directml=["ort-sys/directml"]`) — amendment §A single-RC premise VERIFIED — https://docs.rs/crate/ort/2.0.0-rc.12/features + https://docs.rs/crate/ort/latest/source/Cargo.toml
2. **`win32job 2.0.3`** (latest stable) — https://crates.io/api/v1/crates/win32job
3. **`rappct 0.13.3`** (latest) — https://crates.io/api/v1/crates/rappct
4. **`windows` crate 0.62.x exists** (latest 0.62.2; code pins 0.58 + 0.61.3) — https://crates.io/api/v1/crates/windows
5. **`WebviewInstallMode` = {Skip, DownloadBootstrapper, EmbedBootstrapper, OfflineInstaller, FixedRuntime}**; **`NSISInstallerMode` = {CurrentUser, PerMachine, Both}** — amendment §C enum lists VERIFIED — https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html, https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.NSISInstallerMode.html
6. **`keyring` latest = 4.1.5 (3.6.x line valid)**; **`windows-native-keyring-store` 1.1.0** — amendment §D "hold 3.6" is sound — https://crates.io/api/v1/crates/keyring, https://crates.io/api/v1/crates/windows-native-keyring-store

## E. Spot-checks performed
- Files read: `specs/PORT_MACOS_TO_WINDOWS.md`, `specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/backend/sandbox/mod.rs`, `src-tauri/src/backend/sandbox/seatbelt.rs`, `oracle-core/Cargo.toml`, `oracle-core/src/embed/ort_backend.rs`, `src-tauri/src/backend/auth.rs` (grep + read offset 38), `src-tauri/src/backend/hardware.rs` (grep + read offset 310), `src-tauri/src/polis/commands.rs` (grep + read offsets 2240, 2925); plus `.github/workflows` existence check (absent).
- URLs fetched: crates.io API (`ort`, `win32job`, `rappct`, `windows`, `keyring`, `windows-native-keyring-store`), docs.rs (`ort` features + source Cargo.toml, `tauri-utils` WebviewInstallMode + NSISInstallerMode). (web_search proper failed — Exa credential-empty; substituted `curl` read-only inspection.)

**Bottom line:** Genuine remaining work = Milestone A (`bundle.windows`), Milestone C (sandbox C1–C4, per amendment), Milestone H (CI matrix, `.github/workflows` absent), and the ort rc.10→rc.12 unify (amendment §A). Everything else (B, D, E, F2, keyring) is already shipped and should be marked "verify-only", with the plan's conflicting snippets (§2.4 KeyCredentialManager, §2.6 `cmd /c start`) explicitly **not** applied.