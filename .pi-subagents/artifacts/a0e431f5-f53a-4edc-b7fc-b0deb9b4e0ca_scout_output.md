# macOS-Specific Code & Configuration Inventory

**Project:** devboule (`C:/Users/gualt/Desktop/devboule`)  
**Date:** 2025-07-16  

---

## Category A — Compile-time macOS-only files (truly macOS-only)

No files are entirely macOS-only. All Rust source files that contain `#[cfg(target_os = "macos")]` also contain the non-macOS fallback in the same file. However, the following modules are **effectively macOS-only** because their entire public surface is cfg-gated:

| # | File | Purpose | Verdict |
|---|------|---------|---------|
| A1 | `src-tauri/src/backend/sandbox/seatbelt.rs` (entire file) | SBPL profile builder (`build_profile`) + SBPL escape utilities + macOS-integration tests that require `/usr/bin/sandbox-exec`. The `build_profile` fn is uncfg'd for testability but is only *called* from the macOS `wrap` branch. The non-test functions have `#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]` — they compile but are dead on non-macOS. | cross-with-branch (macOS tests are cfg-gated; prod fn usable off-macOS for testing only) |
| A2 | `src-tauri/src/backend/sandbox/mod.rs` — `macos_sandbox_exec_argv()` (line 108) | Builds `sandbox-exec -p <profile> -- <program>` argv. Only called from the `#[cfg(target_os = "macos")]` branch of `wrap()`. | cross-with-branch |
| A3 | `src-tauri/src/backend/sandbox/mod.rs` — `is_enforced()` (line 229) | Returns `true` only on macOS. Gates Unattended autonomy. | cross-with-branch |

---

## Category B — Cross-platform files with macOS branches

### B1: `src-tauri/src/backend/auth.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 82-91 | `#[cfg(target_os = "macos")] fn hello_available()` (line 82) | Checks Touch ID availability via `objc2_local_authentication::LAContext::canEvaluatePolicy_error`. Returns `bool`. |
| 93-95 | `#[cfg(not(any(windows, macos)))] fn hello_available()` | Returns `false` on Linux. |
| 360-414 | `#[cfg(target_os = "macos")] fn verify_user()` (line 360) | macOS device-owner authentication (Touch ID + password fallback). Uses `LAContext::evaluatePolicy_localizedReason_reply` with `block2` callback on a dedicated thread. UNVERIFIED on hardware. |
| 407-414 | `#[cfg(not(any(windows, macos)))] fn verify_user()` | Returns `Err("Biometric unlock is only available on Windows and macOS.")` on Linux. |

### B2: `src-tauri/src/backend/agent_spawn.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 85-97 | `#[cfg(target_os = "macos")]` inside `fn kill_spawned_agent_on_record_failure` | Closes Terminal.app windows matching the agent title via `osascript`. Sweeps stale launch script dirs via `sweep_stale_macos_launch_script_dirs`. |
| 282-348 | `#[cfg(target_os = "macos")] fn spawn_agent_terminal_app_impl()` | macOS app-hosted PTY spawn: builds a shell script, runs it via `zsh -ic <script>` under portable-pty, with provider_env injected as env vars. |
| 342-348 | `#[cfg(not(any(windows, macos)))] fn spawn_agent_terminal_app_impl()` | Returns `Err("not supported on Linux")` on Linux. |
| 677-690 | `#[cfg(target_os = "macos")] fn build_macos_agent_script()` | Generates a POSIX shell script for launching agent processes under macOS Terminal.app / app-hosted PTY. Handles prompt-file, env exports, self-delete, MCP client config. |
| 933-1037+ | `#[cfg(target_os = "macos")] fn spawn_agent_terminal_impl()` | External Terminal.app spawn path. Writes a restricted script to temp dir, runs via `open -a Terminal`. |
| 1166-1259 | `#[cfg(target_os = "macos")]` constants and helper fns | `MACOS_LAUNCH_SCRIPT_STALE_SECS` (300s), `sweep_stale_macos_launch_script_dirs`, `write_restricted_script_file`, `sh_single_quote`, `applescript_quote`, `shell_env_name`. |
| 1415-1459 | `#[cfg(target_os = "macos")] fn macos_codex_launch_line()` | Builds the codex CLI invocation line for the macOS launch script (POSIX shell, prompt piped via STDIN). |
| 1732-1785 | `#[cfg(target_os = "macos")]` test blocks | HE-2: `temp_script_removed_on_simulated_launch_failure`, `sweep_stale_launch_script_dirs_respects_age`, HE-3: `macos_script_clipboard_payload_has_no_token`, H-1: `macos_custom_claude_oauth_hint_from_unfiltered_env`. |
| 2023-2088 | `#[cfg(target_os = "macos")]` test: orchestrator | `macos_orchestrator_script_no_stdin_prompt` — verifies orchestrator binary gets no prompt via STDIN. |
| 2193+ | `#[cfg(all(test, target_os = "macos"))] mod f36_isolation_tests` | Tests for Claude config dir exports, orchestrator launch line, credential file handling. |

### B3: `src-tauri/src/polis/commands.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 2250-2260 | `#[cfg(target_os = "macos")]` inside `notepad_argv()` | Returns `("open", vec!["-t", path])` — opens TextEdit via the macOS `open -t` command. |
| 2258-2260 | `#[cfg(all(not(macos), not(windows)))]` | Returns `("xdg-open", vec![path])` on Linux. |
| 2270-2280 | `#[cfg(target_os = "macos")]` inside `explorer_argv()` | Returns `("open", vec!["-R", path])` — reveals in Finder via `open -R`. |
| 2936-2950 | `#[cfg(target_os = "macos")]` inside test `native_editor_argv_is_platform_aware` | Asserts macOS-specific argv for notepad/explorer. |

### B4: `src-tauri/src/backend/devices.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 598-600 | `cfg!(target_os = "macos")` in `platform_label()` | Returns `"macos"` as the platform label. |
| 610-612 | `cfg!(target_os = "macos")` in `vault_backend_label()` | Returns `"macOS Keychain"` for the OS credential store name. |
| 622-624 | `cfg!(target_os = "macos")` in `biometric_label()` | Returns `"Touch ID or macOS password"` for the biometric auth label. |

### B5: `src-tauri/src/backend/github.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 30 | `cfg!(target_os = "macos")` in `vault_store_label()` | Returns `"macOS Keychain"` as the OS credential store name. |

### B6: `src-tauri/src/backend/hardware.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 384-408 | `#[cfg(target_os = "macos")] fn detect_gpu()` | Runs `system_profiler SPDisplaysDataType -json` and parses GPU info. |
| 408-410 | `#[cfg(not(any(windows, macos)))] fn detect_gpu()` | Returns `("unknown", None, "unknown")` on Linux. |

### B7: `src-tauri/src/backend/projects.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 49-53 | `#[cfg(target_os = "macos")] pub(crate) use ...` | Re-exports `build_macos_agent_script`, `macos_codex_launch_line`, `macos_orchestrator_launch_line`, `macos_claude_launch_line` from `agent_spawn`. |
| 9378 | `#[cfg(target_os = "macos")]` | macOS branch in a command handler. |
| 9421 | `#[cfg(target_os = "macos")]` | macOS branch in device registration. |
| 9487 | `#[cfg(target_os = "macos")]` | macOS-specific path in resource resolution. |
| 9912 | `#[cfg(target_os = "macos")]` | macOS code within agent workspace setup. |
| 10242 | `#[cfg(target_os = "macos")]` | macOS branch in scripting helpers. |
| 10295 | `#[cfg(target_os = "macos")]` | macOS agent config path. |
| 10319 | `#[cfg(target_os = "macos")]` | macOS-specific file operations. |
| 10381 | `#[cfg(target_os = "macos")]` | macOS path in project import. |
| 10440 | `#[cfg(target_os = "macos")]` | macOS code execution path. |

### B8: `src-tauri/src/backend/provider_detect.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 32 | `#[cfg(target_os = "macos")] use std::io::Read` | Imports for macOS Apple FM detection. |
| 54 | `#[cfg(target_os = "macos")] const CLI_HELP_PROBE_MAX_BYTES` | 64KB limit for `--help` probe output. |
| 408-423 | `#[cfg(target_os = "macos")] fn read_probe_pipe()` | Threaded pipe reader bounded to `CLI_HELP_PROBE_MAX_BYTES`. |
| 423-486 | `#[cfg(target_os = "macos")] fn apple_fm_help_probe_matches()` | Runs `fm --help` and checks output for Apple Foundation Model markers. |
| 473-486 | `#[cfg(target_os = "macos")] fn detect_apple_fm()` | Returns `DetectedProvider` for Apple `fm` CLI when found. |
| 486-488 | `#[cfg(not(target_os = "macos"))] fn detect_apple_fm()` | Returns `None` on non-macOS. |
| 1187 | `#[cfg(not(target_os = "macos"))]` | Fallback for Apple FM detection on other platforms. |

### B9: `src-tauri/src/backend/design_preview.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 538 | `#[cfg(target_os = "macos")] const MACOS_CAPTURE_VERIFIED: bool = false` | Flag controlling whether macOS webview capture is enabled. Currently `false` (capture not verified on real hardware). |
| 739-752 | `#[cfg(target_os = "macos")] async fn capture_webview_png()` | Stub for macOS webview capture — returns error because `MACOS_CAPTURE_VERIFIED` is false. |
| 752-755 | `#[cfg(not(any(windows, macos)))] async fn capture_webview_png()` | Returns `Err("unsupported platform")` on Linux. |

### B10: `src-tauri/src/backend/mini_command_build.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 814 | `#[cfg(target_os = "macos")] pub(crate) fn build_mini_command_impl()` | macOS mini-coder command builder: wraps with `/bin/sh -c`, applies sandbox-exec + Seatbelt for local-loopback backends using `build_seatbelt_profile()`. |
| 1138 | `#[cfg(target_os = "macos")] pub(crate) const MACOS_RESULT_EXTRACTOR_PY` | Inline Python script for extracting JSON results from raw backend stdout (OSC/CSI strip, multi-JSON parse). |
| 1187 | `#[cfg(target_os = "macos")] pub(crate) fn macos_stdout_to_result_wrapper()` | Generates shell wrapper that runs backend, captures stdout to file, invokes Python extractor. |
| 1208 | `#[cfg(target_os = "macos")] pub(crate) fn sh_single_quote_local()` | macOS-specific single-quote escaping for shell embedding. |
| 1215 | `#[cfg(not(any(windows, macos)))] pub(crate) fn build_mini_command_impl()` | Returns `Err("Mini-coder is supported on Windows and macOS only.")` on Linux. |

### B11: `src-tauri/src/backend/mini_coder_executor_tests.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 2427 | `#[cfg(target_os = "macos")]` | macOS-specific test: seatbelt profile syntax acceptance. |
| 2475 | `#[cfg(target_os = "macos")]` | macOS test: seatbelt deny-write-to-readonly. |
| 3520 | `#[cfg(target_os = "macos")]` | macOS test: sandbox profile loopback network. |
| 3529 | `#[cfg(target_os = "macos")]` | macOS test: sandbox profile network enabled. |
| 3594 | `#[cfg(target_os = "macos")]` | macOS test: `.git` write denial in sandbox. |
| 3647 | `#[cfg(target_os = "macos")]` | macOS test: sandbox-exec token/profile acceptance. |
| 3698 | `#[cfg(target_os = "macos")]` | macOS test: scratch dir writable in sandbox. |
| 3755 | `#[cfg(target_os = "macos")]` | macOS test: sandbox rlimit CPU. |
| 3787 | `#[cfg(target_os = "macos")]` | macOS test: sandbox rlimit address space. |
| 3827 | `#[cfg(target_os = "macos")]` | macOS test: sandbox with read-only project root. |
| 3865 | `#[cfg(target_os = "macos")]` | macOS test: result extraction from Python. |
| 4020 | `#[cfg(target_os = "macos")]` | macOS test: editor argv (open -t / open -R). |
| 4057 | `#[cfg(target_os = "macos")]` | macOS test: macos_stdout_to_result_wrapper. |
| 4095 | `#[cfg(target_os = "macos")]` | macOS test: shell script escaping. |
| 4123 | `#[cfg(target_os = "macos")]` | macOS test: applescript quoting. |
| 4182 | `#[cfg(target_os = "macos")]` | macOS test: full mini-coder seatbelt profile build. |

### B12: `src-tauri/src/backend/censor/gemma.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 1235-1254 | `#[cfg(target_os = "macos")] pub struct AppleFmClient` | Apple Foundation Model client for Censor (local AI guardrails). Implements `GemmaClient` trait. |
| 1763-1768 | `#[cfg(target_os = "macos")]` in `build_gemma_client()` | Returns `AppleFmClient` for `CensorAiProvider::AppleFm` on macOS. |
| 1768 | `#[cfg(not(target_os = "macos"))]` | Returns `Err("Apple on-device requires macOS 27+.")` on non-macOS. |
| 4003 | `#[cfg(not(target_os = "macos"))]` | Non-macOS test for Apple FM error. |

### B13: `src-tauri/src/backend/sandbox/mod.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 125-132 | `#[cfg(target_os = "macos")]` and `#[cfg(not(target_os = "macos"))]` in `wrap()` | macOS: builds Seatbelt profile and wraps via `sandbox-exec`. Non-macOS: passthrough with one-time warning. |
| 208-217 | `#[cfg(target_os = "macos")]` in `is_enforced()` | Returns `true` on macOS (real sandbox enforcement). |
| 252 | `#[cfg(not(target_os = "macos"))]` | Returns `false` on non-macOS. |
| 288-296 | `#[cfg(target_os = "macos")]` and `#[cfg(not(target_os = "macos"))]` test blocks | macOS test: `macos_apply_rlimits_sets_cpu_limit`; non-macOS test: `wrap_is_passthrough_off_macos`. |

### B14: `src-tauri/src/backend/agents.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 1757 | `#[cfg(target_os = "macos")]` in `focus_agent_terminal()` | Calls `focus_agent_terminal_macos()` — uses `osascript` to activate and raise Terminal.app window. |
| 1762 | `#[cfg(not(any(windows, macos)))]` | Returns `Err("not supported on Linux")`. |
| 1776 | `#[cfg(target_os = "macos")] fn focus_agent_terminal_macos()` | Uses AppleScript to activate Terminal and bring matching window to front. |
| 1979 | `#[cfg(target_os = "macos")]` | macOS branch in terminal polling logic. |
| 1998 | `#[cfg(all(unix, not(macos)))]` | Linux fallback in terminal polling. |

### B15: `src-tauri/src/backend/mcp_backend.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 138-140 | `#[cfg(all(target_os = "macos", target_arch = "aarch64"))]` and `x86_64` | Adds triple-suffixed MCP binary candidates for macOS (`devboule-mcp-aarch64-apple-darwin` and `x86_64-apple-darwin`). |

### B16: `src-tauri/src/backend/pi_sidecar.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 1484 | `cfg!(target_os = "macos")` check | `let sandboxed = sandbox_enabled && cfg!(target_os = "macos");` — sandbox is only applied on macOS (Seatbelt). On other platforms `sandboxed` is always `false`. |

### B17: `src-tauri/src/oracle/rust_oracle.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 86-103 | `#[cfg(target_os = "macos")]` in `select_embed_backend()` | Uses Candle Metal F16 backend on macOS (requires `oracle-core` with `features=["metal"]`). |
| 103+ | `#[cfg(not(target_os = "macos"))]` | Uses ORT int8 CPU on Windows/Linux. |

### B18: `oracle-core/src/embed/ort_backend.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 60-79 | `#[cfg(target_os = "macos")]` in EP auto-selection | Defaults to `EpArg::Cpu` on macOS (CoreML can't run Qwen3 ONNX export). |
| 79 | `#[cfg(not(any(macos, windows)))]` | Defaults to `EpArg::Cpu` on Linux. |

### B19: `oracle-core/src/embed/mod.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 920-921 | `cfg!(all(target_os = "macos", feature = "metal"))` | `metal_available` flag: enables Candle Metal GPU path on macOS with `metal` feature. |

### B20: `oracle-core/src/embedder.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 1 | `#[cfg(all(feature = "metal", not(target_os = "macos")))]` | `compile_error!("the metal feature is macOS-only")` — prevents building `metal` feature on non-macOS. |
| 59-64 | `#[cfg(all(target_os = "macos", feature = "metal"))] fn metal_device()` | Creates Candle Metal device. |
| 64 | `#[cfg(not(all(target_os = "macos", feature = "metal")))] fn metal_device()` | Returns error `"metal not compiled in"`. |

### B21: `oracle-core/src/onnx_embedder.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 65-78 | `#[cfg(target_os = "macos")]` in session builder | Registers CoreML execution provider with MLProgram format. |
| 78 | `#[cfg(not(target_os = "macos"))]` | Bail if `--ep coreml` used on non-macOS. |

### B22: `oracle-core/src/jobs.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 241-245 | `#[cfg(target_os = "macos")] fn default_device()` | Returns `Some("mps")` for MPS device on macOS. |
| 245 | `#[cfg(not(target_os = "macos"))]` | Returns `None` on other platforms. |

### B23: `oracle-core/src/ingest/indexer.rs`

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 620-646 | `#[cfg(target_os = "macos")] fn read_macos_memory_pressure()` | Reads `kern.memorystatus_vm_pressure_level` and `kern.memorystatus_level` via `libc::sysctlbyname` for memory-aware indexing backpressure. |
| 646 | `#[cfg(not(target_os = "macos"))]` | Returns `None`. |

### B24: `src-tauri/src/backend/mini_command_build.rs` — `cfg_attr` dead_code suppression

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 524, 572, 642, 680, 707, 751 | `#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]` on various functions/types | Suppresses dead-code warnings for macOS-only functions when compiling on non-macOS (they are still compiled for testability). |

### B25: `src-tauri/src/backend/sandbox/seatbelt.rs` — `cfg_attr` dead_code suppression

| Lines | macOS branch | What it does |
|-------|-------------|--------------|
| 9, 17 | `#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]` on `sbpl_escape` and `canonical_sandbox_path` | Suppresses dead-code warnings on non-macOS (these are testable but only called from mac code). |

---

## Category C — macOS framework / crate dependencies

### C1: `src-tauri/Cargo.toml` (lines 169-180)

```
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.5"
objc2-foundation = { version = "0.2", features = ["NSString", "NSError"] }
objc2-local-authentication = { version = "0.2", features = ["LAContext", "LAError", "block2"] }
block2 = "0.5"
oracle-core = { path = "../oracle-core", features = ["metal"] }
```

- `objc2` — Raw Objective-C bindings (runtime message sending). Used for macOS system APIs.
- `objc2-foundation` — Foundation framework bindings (NSString, NSError). Used in `auth.rs` for Touch ID.
- `objc2-local-authentication` — LocalAuthentication.framework bindings. Used for Touch ID / device-owner auth.
- `block2` — C block (closure) support for Objective-C async callbacks. Required by `evaluatePolicy_localizedReason_reply`.
- `oracle-core` with `features = ["metal"]` — Enables Candle Metal GPU backend for ONNX embeddings (only valid on macOS; `compile_error!` on other platforms).

**Always-compiled dependencies with macOS-specific features:**
- `keyring` (line 55): `features = ["windows-native", "apple-native"]` — the `apple-native` feature pulls in the macOS Keychain (`security-framework` transitively) but keyring's feature resolution compiles only the matching one.

### C2: `oracle-core/Cargo.toml` (lines 48-51)

```
[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.10", features = ["coreml"] }
libc = "0.2"
```

- `ort` with `features = ["coreml"]` — ONNX Runtime with CoreML execution provider (macOS GPU path).
- `libc` — Used for `sysctlbyname` syscalls (memory pressure reading in `indexer.rs`).

---

## Category D — macOS build & signing config

### D1: `src-tauri/tauri.conf.json`

- **No macOS-specific bundle keys** present (no `macOS` sub-object, no `minimumSystemVersion`, `signingIdentity`, `entitlements`, `hardenedRuntime`, `category`, `fileAssociations`, `urlSchemes`).
- **Bundle targets:** `"all"` — builds dmg, msi, appimage as applicable.
- **Icons:** Includes `icons/icon.icns` (macOS icon format) alongside `icon.ico` (Windows) and PNGs.
- **Identifier:** `"com.devboule.app"` — the macOS app bundle identifier.
- **ExternalBin:** `["binaries/devboule-mcp"]` — Tauri copies this to `MacOS/devboule-mcp` in the `.app` bundle.
- **Resources:** Listed paths are bundled into the app Resources directory.

### D2: `src-tauri/Info.plist`

**Full file content:**
```xml
<plist version="1.0">
<dict>
  <key>NSAppSleepDisabled</key>
  <true/>
</dict>
</plist>
```

Purpose: Disables macOS App Nap (App Sleep) to prevent background throttling. This is macOS-specific and only applies to the macOS .app bundle.

### D3: Entitlements files

**No `.entitlements` files found anywhere in the repository.** No sandbox entitlements, no hardened runtime entitlements.

### D4: Build scripts

- `scripts/stage-devboule-mcp.sh` — Cross-platform: stages MCP binary for Tauri. Contains `install_new_inode` logic that uses `chmod +x` on non-Windows (which covers macOS). No macOS-exclusive tooling.
- `scripts/stage-oracle-embedder.sh` — Cross-platform (stages Python embedder, not macOS-specific).
- No notarization/stapling scripts found.
- No Makefile found that targets macOS specifically.

### D5: CI / GitHub Actions

**No `.github/workflows/` or CI configuration found.** No macOS-specific CI jobs.

---

## Category E — Shell scripts and tooling that target macOS only

### E1: `scripts/stage-devboule-mcp.sh`

| Lines | macOS specificity | Description |
|-------|------------------|-------------|
| 57 | Uses `$(uname -s)` checks for MINGW/MSYS/CYGWIN | Cross-platform logic: `chmod +x` on non-Windows (includes macOS). Portable script with Windows detection. |
| 68, 79 | Same check for executable-ness | Portable. |

**Verdict:** Portable (cross-platform) with uname-based platform detection. No macOS-exclusive tooling.

### E2: `tools/devboule-pilot/env.sh`

| Lines | macOS specificity | Description |
|-------|------------------|-------------|
| 24 | `DEVBOULE_APP_IDENTIFIER="com.devboule.app"` | The `.app` suffix is the macOS bundle identifier convention. Used by the Tauri pilot across platforms. |
| 29 | Same | Same. |

**Verdict:** Cross-platform with macOS-naming convention for app identifier. No macOS-exclusive tooling.

### E3: `tools/devboule-pilot/ensure-devurl.sh`

| Lines | macOS specificity | Description |
|-------|------------------|-------------|
| 104 | `/tmp/tauri-pilot-com.devboule.app.sock` | The socket path uses the macOS `com.devboule.app` bundle ID convention. The `/tmp/` path works on both macOS and Linux; not Windows-specific. |

**Verdict:** Cross-platform, references macOS bundle ID convention but works on Linux too.

**No scripts found containing:** `defaults write/read`, `osascript`, `xattr`, `codesign`, `xcrun`, `ditto`, `launchctl`, hardcoded `.app` paths (beyond the identifier convention), or `Darwin` uname checks.

---

## Category F — Capacities / SecurityPolicy that apply differently on macOS

### F1: `src-tauri/capabilities/default.json`

```json
{
  "identifier": "default",
  "description": "Default capabilities for Devboule",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "dialog:allow-open",
    "notification:allow-is-permission-granted",
    "notification:allow-request-permission",
    "notification:allow-notify"
  ]
}
```

**Verdict:** No platform-specific capabilities. The same permissions apply to all platforms.

### F2: `src-tauri/tauri.conf.json` — `security` block

```json
"security": {
  "csp": {
    "default-src": "'self'",
    "script-src": "'self'",
    "style-src": "'self' 'unsafe-inline'",
    "font-src": "'self'",
    "img-src": "'self' data:",
    "connect-src": "'self' ipc: http://ipc.localhost",
    "frame-src": "'self' artifact: http://artifact.localhost"
  }
}
```

**Verdict:** No security policy differences per platform. CSP is uniform.

### F3: Sandbox / Seatbelt (`src-tauri/src/backend/sandbox/`)

The entire sandbox module is the **primary macOS-specific security enforcement mechanism**:

- **macOS:** `wrap()` builds a Seatbelt SBPL profile and wraps the child via `/usr/bin/sandbox-exec`. `is_enforced()` returns `true`.
- **Windows:** `wrap()` is passthrough (not yet implemented — phase 3 planned). `is_enforced()` returns `false`.
- **Linux:** `wrap()` is passthrough (landlock stub). `is_enforced()` returns `false`.

The `pi_sidecar.rs` Pi session also uses this: `let sandboxed = sandbox_enabled && cfg!(target_os = "macos");` — the sandbox is only *actually* applied on macOS.

---

## Category G — macOS-only test fixtures / mock files

### G1: `src-tauri/src/backend/sandbox/seatbelt.rs` (lines 204-350+)

- `#[cfg(target_os = "macos")] mod tests` contains:
  - `macos_real_parser_regression` — Tests `/usr/bin/sandbox-exec` with real SBPL profiles (write deny, allow, git-write-deny).
  - `macos_enabled_profile_accepted_by_kernel` — Tests network-enabled profile acceptance.
  - `macos_git_dir_denied_even_when_root_writable` — Tests .git write protection.

### G2: `src-tauri/src/backend/sandbox/mod.rs` (lines 288-301)

- `#[cfg(target_os = "macos")] #[test] fn macos_apply_rlimits_sets_cpu_limit()` — Tests that rlimits are actually enforced on macOS.
- `#[cfg(not(target_os = "macos"))] #[test] fn is_enforced_false_off_macos()` — Tests that sandbox is passthrough on non-macOS.

### G3: `src-tauri/src/backend/mini_coder_executor_tests.rs` (lines 2427-4182+)

At least 16 macOS-only tests (see Category B11 above) testing seatbelt profiles, sandbox-exec, launch scripts, result extraction, quoting functions.

### G4: `src-tauri/src/backend/agent_spawn.rs` (lines 1732-2193+)

- `#[cfg(target_os = "macos")] #[test]` blocks (9+ macOS-only tests) testing launch script generation, secrets handling, stale dir sweep, clipboard safety, HE-2/HE-3 invariants.
- `#[cfg(all(test, target_os = "macos"))] mod f36_isolation_tests` — Tests Claude config dir, orchestrator launch, credential isolation.

### G5: `src-tauri/src/backend/censor/gemma.rs` (line 4003)

- `#[cfg(not(target_os = "macos"))] #[test] fn build_gemma_client_applefm_non_macos_clean_error()` — Tests that Apple FM errors cleanly on non-macOS.

### G6: `src-tauri/src/backend/provider_detect.rs` (line 1187)

- `#[cfg(not(target_os = "macos"))]` — Test for Apple FM probe fallback.

### G7: `src-tauri/src/polis/commands.rs` (lines 2936-2950)

- `#[cfg(target_os = "macos")]` assertions in `native_editor_argv_is_platform_aware` test.

---

## Summary Table

| File | Category | Why macOS-specific | Verdict |
|------|----------|-------------------|---------|
| `src-tauri/src/backend/auth.rs` | B | Touch ID (LocalAuthentication) via objc2 | cross-with-branch |
| `src-tauri/src/backend/agent_spawn.rs` | B, G | Terminal.app management, osascript, launch scripts, PTY spawn | cross-with-branch |
| `src-tauri/src/backend/projects.rs` | B | Re-exports macOS agent build functions | cross-with-branch |
| `src-tauri/src/backend/provider_detect.rs` | B | Apple Foundation Model CLI detection | cross-with-branch |
| `src-tauri/src/backend/devices.rs` | B | Platform label strings ("macOS Keychain", "Touch ID") | cross-with-branch |
| `src-tauri/src/backend/github.rs` | B | Vault store label "macOS Keychain" | cross-with-branch |
| `src-tauri/src/backend/hardware.rs` | B | `system_profiler SPDisplaysDataType` GPU detection | cross-with-branch |
| `src-tauri/src/backend/design_preview.rs` | B | Webview capture stub (objc2, unverified) | cross-with-branch |
| `src-tauri/src/backend/mini_command_build.rs` | B | Sandboxed mini-coder, result extractor Python, shell quoting | cross-with-branch |
| `src-tauri/src/backend/mini_coder_executor_tests.rs` | B, G | Seatbelt profile tests, macOS launch tests | cross-with-branch |
| `src-tauri/src/backend/censor/gemma.rs` | B | AppleFmClient (Apple Foundation Model integration) | cross-with-branch |
| `src-tauri/src/backend/sandbox/seatbelt.rs` | B, G | SBPL profile builder, macOS sandbox-exec integration tests | cross-with-branch |
| `src-tauri/src/backend/sandbox/mod.rs` | B, G | `wrap()` with sandbox-exec, `is_enforced()`, rlimit application | cross-with-branch |
| `src-tauri/src/backend/agents.rs` | B | `focus_agent_terminal_macos()` (osascript) | cross-with-branch |
| `src-tauri/src/backend/mcp_backend.rs` | B | Triple-suffixed MCP binary names for macOS | cross-with-branch |
| `src-tauri/src/backend/pi_sidecar.rs` | B | `cfg!(target_os = "macos")` sandbox gating | cross-with-branch |
| `src-tauri/src/oracle/rust_oracle.rs` | B | Candle Metal backend selection | cross-with-branch |
| `src-tauri/src/polis/commands.rs` | B, G | `notepad_argv`/`explorer_argv` with `open -t`/`open -R`, test assertions | cross-with-branch |
| `oracle-core/src/embed/ort_backend.rs` | B | CoreML EP auto-default to CPU on macOS | cross-with-branch |
| `oracle-core/src/embed/mod.rs` | B | `metal_available = cfg!(all(target_os = "macos", feature = "metal"))` | cross-with-branch |
| `oracle-core/src/embedder.rs` | B | `compile_error!` if `metal` feature used on non-macOS; `metal_device()` | cross-with-branch |
| `oracle-core/src/onnx_embedder.rs` | B | CoreML EP registration | cross-with-branch |
| `oracle-core/src/jobs.rs` | B | `default_device()` returning `Some("mps")` | cross-with-branch |
| `oracle-core/src/ingest/indexer.rs` | B | `sysctlbyname` memory pressure reading (macOS kernel) | cross-with-branch |
| `src-tauri/Cargo.toml` | C | `objc2`, `objc2-foundation`, `objc2-local-authentication`, `block2`, `oracle-core` with `metal` feature | cross-with-branch |
| `oracle-core/Cargo.toml` | C | `ort` with `coreml` feature, `libc` | cross-with-branch |
| `src-tauri/Info.plist` | D | `NSAppSleepDisabled` — macOS-only plist | mac-only |
| `src-tauri/tauri.conf.json` | D | `icon.icns`, `com.devboule.app` identifier | cross-platform with macOS elements |
| `scripts/stage-devboule-mcp.sh` | E | `chmod +x` on non-Windows (includes macOS) | cross-platform |
| `tools/devboule-pilot/env.sh` | E | `com.devboule.app` identifier | cross-platform |
| `src-tauri/capabilities/default.json` | F | Generic permissions, no macOS-specific policy | cross-platform |

---

## Top-Level Answer: Which files would a Windows or Linux developer never need to touch?

### Never need to touch (macOS-only purpose):

1. **`src-tauri/Info.plist`** — Only applies to macOS `.app` bundles. Disables App Nap. Zero relevance on Windows/Linux.

### Rarely need to touch (macOS-heavy but cross-platform file):

2. **`src-tauri/src/backend/sandbox/seatbelt.rs`** — The entire SBPL profile builder and macOS test suite. Windows/Linux devs only encounter this if implementing their own sandbox backend.

3. **`src-tauri/src/backend/sandbox/mod.rs`** — The `macos_sandbox_exec_argv` helper and `is_enforced()`. Windows/Linux only need to stub out.

4. **`src-tauri/src/backend/agent_spawn.rs`** — Contains `build_macos_agent_script`, `spawn_agent_terminal_impl` (macOS Terminal.app), `kill_spawned_agent_on_record_failure` (osascript close), sweep functions, and many macOS-only test modules. This is the most macOS-dense file in the project.

5. **`src-tauri/src/backend/mini_command_build.rs`** — `build_mini_command_impl` for macOS (Seatbelt wrapping), `MACOS_RESULT_EXTRACTOR_PY`, `macos_stdout_to_result_wrapper`, `sh_single_quote_local`.

6. **`src-tauri/src/backend/auth.rs`** — The `verify_user()` macOS implementation using `objc2-local-authentication`. Windows devs only touch the Windows Hello path; Linux devs skip entirely.

7. **`src-tauri/src/backend/design_preview.rs`** — The `capture_webview_png` macOS stub (currently returns error).

8. **`src-tauri/src/backend/hardware.rs`** — `detect_gpu()` macOS branch using `system_profiler`.

9. **`src-tauri/src/backend/provider_detect.rs`** — `detect_apple_fm()` macOS branch.

10. **`src-tauri/src/backend/censor/gemma.rs`** — `AppleFmClient` struct and implementation.

11. **`src-tauri/src/backend/agents.rs`** — `focus_agent_terminal_macos()` using `osascript`.

12. **`src-tauri/src/backend/mcp_backend.rs`** — The `aarch64-apple-darwin`/`x86_64-apple-darwin` triple-suffixed binary names.

13. **`src-tauri/src/oracle/rust_oracle.rs`** — The `#[cfg(target_os = "macos")]` branch in `select_embed_backend()`.

14. **`oracle-core/src/embedder.rs`** — `compile_error!` guard + `metal_device()`. Only the `#[cfg(not(all(target_os = "macos", feature = "metal")))]` fallback is relevant.

15. **`oracle-core/src/ingest/indexer.rs`** — `read_macos_memory_pressure()` using `sysctlbyname`.

16. **`oracle-core/src/jobs.rs`** — `default_device()` returning `Some("mps")`.

17. **`oracle-core/src/onnx_embedder.rs`** — CoreML EP registration block.

The **minimal absolute list** (files a Windows/Linux developer would never need to touch):

```
src-tauri/Info.plist
```

Everything else is a cross-platform file with macOS branches. The macOS branches are well-contained behind `#[cfg(target_os = "macos")]` gates and won't affect Windows/Linux compilation.

---

## Acceptance Report