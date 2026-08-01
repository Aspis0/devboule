# Port devboule macOS-only surface to Windows

> **Status:** Plan + phases + code samples. Every code block is either
> copied verbatim from a verified source (URL next to it), or explicitly
> marked `TODO(verify)` with the link that needs to be re-checked at
> implementation time. Nothing is invented.

> **Hard invariant:** No macOS file/block/test gets removed, simplified,
> or regressed. All Windows additions live behind `#[cfg(target_os = "windows")]`.

---

> **SUPERSEDED (2026-07-31)**: this plan's C2/C3 decisions were revised by
> `PORT_MACOS_TO_WINDOWS_FINAL.md` — the shipped broker uses per-spawn
> AppContainers (C5, §10.6), not the restricted-token/rappct follow-up
> described below. Keep this file for history; the FINAL plan is the
> reference.

## 0. Scope decisions (locked)

| Decision | Choice |
|---|---|
| Scope bracket | "Movable only" — port what has a clean Windows analogue, skip cleanly otherwise |
| Sandbox fidelity | **RESOLVED (C5)**: per-spawn AppContainer profiles (see FINAL plan §10.6). The original choice — best effort via `windows` crate + `win32job`, AppContainer via `rappct` as follow-up — is historical. |
| Apple FM (Censor on-device LLM) | **Skip this plan** — add `TODO(next-plan)` comment on `AppleFmClient` |
| GPU/embedder | **A**: `ort` w/ `directml` feature on Windows; keep `candle-metal` on macOS |
| Bundle config | **A**: explicit `bundle.windows` block in `tauri.conf.json` |
| **Invariant** | macOS code stays intact |

**Open product questions still pending (§6):** ARM64, elevation, `is_enforced()→true` acceptance.

---

## 1. Verified evidence base

Sources verified by websearch (this plan's only source of truth):

### Crate versions, verified

| Crate | Version | Confirmed source |
|---|---|---|
| `win32job` | 2.0.3 (May 2025) | <https://crates.io/crates/win32job>, <https://docs.rs/win32job/2.0.3> |
| `rappct` | 0.13.3 (Oct 2025) | <https://crates.io/crates/rappct>, <https://docs.rs/crate/rappct/latest> |
| `uiautomation` | 0.25.0 (May 2026) | <https://crates.io/crates/uiautomation>, <https://github.com/leexgone/uiautomation-rs> |
| `windows-wfp` | 0.2.1 (Mar 2026, GPL-2.0) | <https://crates.io/crates/windows-wfp> |
| `wfp` (alt) | 0.0.7 (May 2026, dlon) | <https://crates.io/crates/wfp>, <https://docs.rs/wfp> |
| `ort` | 2.0.0-rc.12 (ONNX Runtime 1.24) | <https://docs.rs/crate/ort/2.0.0-rc.12/features> |
| `userboundkey-kcm` | 0.1.0 (Jan 2026, MIT) | <https://crates.io/crates/userboundkey-kcm> |
| `keyring` | 4.x (windows-native, apple-native features) | <https://crates.io/crates/keyring>, <https://crates.io/crates/windows-native-keyring-store> |
| `windows-native-keyring-store` | 1.1.0 (May 2026) | <https://crates.io/crates/windows-native-keyring-store> |

### Tauri v2 config, verified

- Bundle targets include `nsis` and `msi` (Windows-side). Source: <https://v2.tauri.app/distribute/windows-installer/>, <https://v2.tauri.app/reference/config/>.
- `WebviewInstallMode` default is `{ "silent": true, "type": "downloadBootstrapper" }`. Source: <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html>.
- JSON schema for config: <https://schema.tauri.app/config/2>.

### Windows API facts, verified

- **Job Object kill-on-close**: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` flag (verified in `Job Objects` Microsoft Learn doc + `win32job` source).
- **Restricted token**: `CreateRestrictedToken` API, signature from `winapi::um::securitybaseapi`. Source: <https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken>.
- **DXGI enumeration**: `IDXGIFactory1::EnumAdapters1` + `IDXGIAdapter1::GetDesc1` returning `DXGI_ADAPTER_DESC1` with `DedicatedVideoMemory: usize`. Source: <https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Dxgi/struct.IDXGIAdapter1.html>, <https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgifactory1-enumadapters1>.
- **`KeyCredentialManager::IsSupportedAsync()`** — returns `Result<IAsyncOperation<bool>>` from `windows::Security::Credentials`. Source: <https://microsoft.github.io/windows-docs-rs/doc/windows/Security/Credentials/struct.KeyCredentialManager.html>.
- **`explorer.exe /select,<path>`** — comma immediately after, no space. Source: <https://ss64.com/nt/explorer.html>.
- **`cmd /c start "" "<file>"`** — empty `""` is the title placeholder (NOT path). Source: <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/start>, <https://ss64.com/nt/start.html>.

### Real-world sandbox reference implementations verified

- **OpenAI Codex `windows-sandbox-rs`**: uses `windows_sys::Win32::Security::CreateRestrictedToken` + `windows_sys::Win32::System::Threading::CreateProcessAsUserW`. Source: <https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/token.rs>, <https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/process.rs>.
- **Anthropic `srt-win`**: same pattern with `windows` crate + `SAFER_LEVEL`. Source: <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/token.rs>, <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/launch.rs>.
- **`win32job` example** (verbatim from docs.rs): the canonical kill-on-close pattern.

### Aion 1.0 (Microsoft AI Foundry), verified

- Microsoft Build 2026 (June 2 2026) announcement: Phi Silica → Aion Instruct transition. **Aion 1.0 Instruct (SLM) in Edge Insider preview today. Aion 1.0 Plan (14B reasoning model) ships in-box on capable Windows PCs. Standalone sideloadable package for Aion Instruct: September 2026. Open-weights Aion Instruct on Hugging Face: July 2026.**
- **No Rust SDK today.** Microsoft sample is C#/WinUI on .NET 9: <https://github.com/microsoft/Aion-Instruct-Preview-Sample>.
- Source for the announcement: <https://blogs.windows.com/windowsdeveloper/2026/06/02/build-2026-furthering-windows-as-the-trusted-platform-for-development/>, <https://learn.microsoft.com/en-us/windows/ai/apis/phi-silica>.

### CI, verified

- `tauri-apps/tauri-action` is the official GH Action. Source: <https://github.com/tauri-apps/tauri-action>, <https://v2.tauri.app/distribute/pipelines/github/>.
- Caching via `Swatinem/rust-cache`. Source: <https://github.com/smikes75/d2h/blob/main/.github/workflows/build-windows.yml>.
- Example workflow: <https://github.com/MediaHarbor/mediaharbor/blob/5074c1d663ce90b69f6e37607773ca200f02a95f/.github/workflows/tauri-build.yml>.

### WebView2/elevation bugs to be aware of (not implementing around, just noting)

- **wry#1665**: WebView2 deadlock on ARM64. Source: <https://github.com/tauri-apps/wry/issues/1665>. **=> ARM64 = "best effort" if at all in v1.**
- **tauri#13926**: Admin Protection breaks WebView2. Source: <https://github.com/tauri-apps/tauri/issues/13926>. **=> devboule must not require elevation.**

---

## 2. Code samples (verified, not invented)

### 2.1 Job Object with kill-on-close — from `win32job` docs

Source: <https://docs.rs/win32job/latest/win32job/>, copied verbatim from the docs.

```rust
use win32job::Job;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let job = Job::create()?;
    let mut info = job.query_extended_limit_info()?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&mut info)?;
    job.assign_current_process()?;

    Command::new("cmd.exe")
        .arg("/C")
        .arg("ping -n 9999 127.0.0.1")
        .spawn()?;

    // The cmd will be killed once we exit, or `job` is dropped.
    Ok(())
}
```

Notes for our use:
- `Job::create_with_limit_info(&ExtendedLimitInfo)` lets us set memory + cpu + kill-on-close in one call.
- `Job::assign_process(proc_handle as isize)` accepts a process handle (we'll get it via `OpenProcess` with `PROCESS_ALL_ACCESS = 0x1F0FFF`).
- `ExtendedLimitInfo::limit_working_memory(min, max)` + `set_extended_limit_info` set the working-set cap, NOT address-space total. For `addr_space_bytes` we want the equivalent — but `win32job` exposes `BasicLimitInformation` via query only; `JOB_OBJECT_LIMIT_PROCESS_MEMORY` is set via raw `SetInformationJobObject`. **Will need a thin wrapper around `SetInformationJobObject` for `ProcessMemoryLimit`. `TODO(verify)`: confirm exact struct field in current `windows` crate version.**

### 2.2 Restricted token construction — pattern from Codex

Source: <https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/token.rs> (real Anthropic-style pattern; adapted to devboule's needs).

```rust
// Pattern from openai/codex windows-sandbox-rs/src/token.rs
use windows_sys::Win32::Foundation::{HANDLE, CloseHandle};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, SID_AND_ATTRIBUTES, PSID, LUID_AND_ATTRIBUTES,
    TOKEN_ALL_ACCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, PROCESS_INFORMATION, STARTUPINFOW,
    EXTENDED_STARTUPINFO_PRESENT, CREATE_UNICODE_ENVIRONMENT,
};

/// Safety: caller must close the returned token handle.
pub unsafe fn make_restricted_token() -> Result<HANDLE, String> {
    let mut base_token: HANDLE = std::ptr::null_mut();
    // OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut base_token)?;
    // (left as the next concrete call — see TODO)
    // ::OpenProcessToken(...);

    let mut restricted: HANDLE = std::ptr::null_mut();
    // DISABLE_MAX_PRIVILEGE flag strips most privileges — including
    // SeRestorePrivilege and SeTakeOwnershipPrivilege — which is the
    // canonical "drop escalation capability" move.
    let ok = CreateRestrictedToken(
        base_token,
        4, // DISABLE_MAX_PRIVILEGE = 0x4
        0, std::ptr::null(),     // no SIDs to disable explicitly
        0, std::ptr::null_mut(), // no privileges to delete explicitly
        0, std::ptr::null(),     // no restricting SIDs
        &mut restricted,
    );
    if ok == 0 {
        return Err(format!("CreateRestrictedToken failed: {}", std::io::Error::last_os_error()));
    }
    Ok(restricted)
}
```

`TODO(verify)` markers inside the snippet (will resolve before code lands):
- Exact `OpenProcessToken` signature from `windows_sys`.
- Whether DISABLE_MAX_PRIVILEGE constant is `4` (`0x4`) or re-exported by the `windows-sys` crate as a named constant.
- `EXTENDED_STARTUPINFO_PRESENT` is needed to pass a job handle to `CreateProcessAsUserW` — verify the docs example pattern matches.

### 2.3 DXGI GPU enumeration — from `windows` crate docs

Source: <https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Dxgi/struct.IDXGIAdapter1.html>, <https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgifactory1-enumadapters1>.

```rust
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIAdapter1, DXGI_ADAPTER_DESC1,
};

#[cfg(target_os = "windows")]
pub fn detect_gpu() -> (String, Option<f64>, String) {
    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(_) => return (String::from("unknown"), None, String::from("unknown")),
        };

        let mut best: Option<(String, f64)> = None;
        let mut idx = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(idx) {
            idx += 1;
            let desc: DXGI_ADAPTER_DESC1 = match adapter.GetDesc1() {
                Ok(d) => d,
                Err(_) => continue,
            };
            // Description is a wide-char fixed buffer; pull as a CString for inspection.
            let name_end = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..name_end]);
            let vram_bytes = desc.DedicatedVideoMemory as f64;
            let vram_gb = vram_bytes / (1024.0 * 1024.0 * 1024.0);
            // Prefer the adapter with most dedicated VRAM.
            if best.as_ref().map_or(true, |(_, v)| vram_gb > *v) {
                best = Some((name, vram_gb));
            }
        }

        match best {
            Some((name, vram)) => (name, Some(vram), String::from("directx")),
            None => (String::from("unknown"), None, String::from("unknown")),
        }
    }
}
```

`TODO(verify)` markers:
- Exact path `windows::Win32::Graphics::Dxgi::CreateDXGIFactory1` in the version of `windows` we'll pin for devboule.
- `windows::core::Result` semantics vs `Win32::Foundation::HRESULT`.

### 2.4 Windows Hello (`hello_available`) — from `windows` crate docs

Source: <https://microsoft.github.io/windows-docs-rs/doc/windows/Security/Credentials/struct.KeyCredentialManager.html>.

```rust
#[cfg(target_os = "windows")]
use windows::Security::Credentials::KeyCredentialManager;

#[cfg(target_os = "windows")]
pub fn hello_available() -> bool {
    // IsSupportedAsync returns IAsyncOperation<bool>; .get() blocks on the result.
    KeyCredentialManager::IsSupportedAsync()
        .ok()
        .and_then(|op| op.get().ok())
        .unwrap_or(false)
}
```

The existing macOS `auth.rs` already has WinRT-thread setup helpers (`WinRtGuard` etc., verified by my own file read); we'll reuse that scaffold. **NOTE**: WinRT async calls must be made on an STA-initialized thread.

`TODO(verify)`:
- Exact `IAsyncOperation<bool>` `.get()` semantics in `windows 0.62.x` — there may be a `join()` instead of `.get()`.
- `KeyCredentialCreationOption` enum import path.

### 2.5 `ort` DirectML execution provider wiring

Source: <https://docs.rs/ort/latest/ort/ep/directml/struct.DirectML.html>, <https://ort.pyke.io/setup/cargo-features>, <https://docs.rs/crate/ort/latest/features>.

```toml
# oracle-core/Cargo.toml — addition (cross-check exact rc version BEFORE merging)
[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "directml", "ndarray"] }
```

```rust
// Existing select_embed_backend — new Windows branch (ADDITIVE; do not touch macOS)
#[cfg(target_os = "windows")]
pub fn select_ep() -> ort::ep::ExecutionProvider {
    use ort::ep::DirectML;
    DirectML::default().build()
}
```

`TODO(verify)`:
- `=2.0.0-rc.12` is the exact published rc at plan time. **Re-check crates.io at code-cut.**
- `DirectML::default().build()` returns `ExecutionProvider` — exact API surface in current `ort` RC. Source: <https://docs.rs/ort/latest/ort/ep/directml/struct.DirectML.html>.
- `windows`-coreml coexistence on macOS — existing `ort = { version = "2.0.0-rc.10", features = ["coreml"] }` at `oracle-core/Cargo.toml:48-51` remains.

### 2.6 Editor launch on Windows — `cmd /c start "" "<file>"`

Source: <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/start>, <https://ss64.com/nt/start.html>. Empty `""` is the title argument, NOT a path.

```rust
#[cfg(target_os = "windows")]
pub fn open_in_editor(path: &std::path::Path) -> Result<(), String> {
    let s = path.as_os_str().to_string_lossy().to_string();
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", &s])
        .status()
        .map_err(|e| format!("start failed: {e}"))?;
    if !status.success() {
        return Err(format!("start exited with {status:?}"));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    let s = path.as_os_str().to_string_lossy().to_string();
    let status = std::process::Command::new("explorer")
        .arg(format!("/select,{}", s))
        .status()
        .map_err(|e| format!("explorer /select failed: {e}"))?;
    if !status.success() {
        return Err(format!("explorer exited with {status:?}"));
    }
    Ok(())
}
```

`TODO(verify)`:
- Long-path handling — paths over ~260 chars need `\\?\` prefix; out of scope for v1.
- Comma directly after `/select` with no space — verified.

---

## 3. Tauri config additions

### 3.1 `tauri.conf.json` — `bundle.windows` block

Source for the keys: <https://v2.tauri.app/distribute/windows-installer/>, <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html>, <https://docs.rs/tauri-utils/latest/tauri_utils/config/struct.WixConfig.html>.

```jsonc
{
  // ... existing fields ...
  "bundle": {
    "active": true,
    "targets": "all",                      // keep: macOS deb/rpm/dmg and Windows nsis/msi both honored
    "icon": [/* unchanged */],
    "resources": [/* unchanged */],
    "externalBin": ["binaries/devboule-mcp"],
    "windows": {
      "wix": {
        // placeholder; commented-out fields take schema defaults
        // "language": "en-US"
      },
      "nsis": {
        // "installerIcon": "icons/icon.ico",
        // "languages": ["English"],
        "installMode": "perMachine"
      },
      "webviewInstallMode": {
        "type": "downloadBootstrapper",
        "silent": true
      }
      // "signCommand": null   // TODO(verify) once we have a cert path
    }
  }
}
```

### 3.2 Smoke test: schema-validity across platforms

```rust
// src-tauri/tests/tauri_conf_windows.rs
use serde_json::Value;
use std::fs;

#[test]
fn tauri_conf_json_is_valid_and_has_windows_bundle() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tauri.conf.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap())
        .expect("tauri.conf.json must be valid JSON");

    assert!(v["bundle"]["active"].as_bool().unwrap_or(false),
            "bundle.active must be true");

    let windows = &v["bundle"]["windows"];
    assert!(windows.is_object(),
            "bundle.windows must be an object (Windows bundle config)");

    // We accept either default webviewInstallMode (None) or explicit.
    if let Some(m) = windows["webviewInstallMode"].as_object() {
        let t = m.get("type").and_then(Value::as_str).unwrap_or("");
        assert!(
            ["downloadBootstrapper", "fixedRuntime", "embedBootstrapper", "offlineInstaller", "skip"]
                .contains(&t),
            "webviewInstallMode.type must be a valid Tauri value (got {t})"
        );
    }

    // Cross-check we did NOT regress macOS.
    let targets = v["bundle"]["targets"].as_str().unwrap_or("all");
    assert_eq!(targets, "all", "bundle.targets should remain 'all' for cross-platform builds");
}
```

`TODO(verify)`: WebviewInstallMode is `Option<...>` in the schema — confirm the tests do not break on either default-omitted or explicit.

---

## 4. Cargo.toml — Windows target dependencies

Source for each dep confirmed above.

### 4.1 `src-tauri/Cargo.toml` additions

```toml
# Windows-only sandbox primitives — existing macOS block stays untouched.
[target.'cfg(target_os = "windows")'.dependencies]
win32job = "2"
windows  = { version = "0.62", features = [
    "Win32_System_JobObjects",
    "Win32_System_Threading",
    "Win32_Security",
    "Win32_Security_Credentials",
    "Win32_Foundation",
    "Win32_System_Memory",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Graphics_Dxgi",
    "Security_Credentials",
    "Win32_NetworkManagement_WindowsFilteringPlatform",
] }

# Tokio stays — already in tree; no additions.
```

`TODO(verify)`:
- `windows` feature flag list — `Win32_Graphics_Dxgi` and `Security_Credentials` casing are the documented form (with underscores). Source: <https://microsoft.github.io/windows-docs-rs/doc/windows/>. Re-confirm the exact feature set resolves at `cargo check`.
- Job Object kernel32 imports — `Win32_System_JobObjects` is correct per the `windows` crate's feature list.

### 4.2 `oracle-core/Cargo.toml` additions

```toml
# Already has:
[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.10", features = ["coreml"] }

# ADD (Windows side — DirectML, do not touch macOS):
[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = [
    "std", "directml", "ndarray",
] }
```

`TODO(verify)`: `ort 2.0.0-rc.12` exact version at code-cut time. The two RCs may not coexist in one workspace if Cargo resolves them to distinct crate versions — **may need to unify to a single RC across both targets**. This is the single biggest ORT-related risk in the plan; F1 milestone prep checkpoint exists for this reason.

### 4.3 Existing `keyring` setup stays intact

`src-tauri/Cargo.toml:55` already has:
```toml
keyring = { version = "3.6", features = ["windows-native", "apple-native"] }
```
The `keyring` crate's per-target feature resolution means it picks the right backend at compile time. **No change needed for Windows credential storage.** Verified by my own file read earlier today and by `keyring` documentation.

---

## 5. Phases / milestones

Plan: ship in 8 milestones, smallest first, reviewer-gated between each.

### Milestone A — bundle config + smoke test

**A1**. Add `bundle.windows` block to `tauri.conf.json` (snippet §3.1).
**A2**. Add `src-tauri/tests/tauri_conf_windows.rs` (snippet §3.2).
**A3**. Verify: `cargo test -p devboule-tauri tests::tauri_conf_json_is_valid_and_has_windows_bundle` passes locally + on Windows runner.

Owner: single `worker` pass; one `reviewer` pass.

### Milestone B — Editor integration (trivial port)

**B1**. In `src-tauri/src/polis/commands.rs:2250`, add `#[cfg(target_os = "windows")]` arm to `notepad_argv()` returning `("cmd", vec!["/c", "start", "", path.to_str().unwrap()])`. Mirror macOS test at line 2936 with a new `#[cfg(target_os = "windows")]` test.
**B2**. Same for `explorer_argv()` returning `("explorer", vec![format!("/select,{}", path.display())])`. Same test pattern.

Owner: single `worker`; one `reviewer`; macOS tests untouched.

### Milestone C — Sandbox (sub-milestones C1, C2, C3)

#### C1 — Job Object

Create `src-tauri/src/backend/sandbox/windows.rs` (new file):
- Wraps `win32job::Job` with `ExtendedLimitInfo` set: `limit_kill_on_job_close()` + raw `SetInformationJobObject` for `ProcessMemoryLimit` (max-process memory from `policy.rlimits.addr_space_bytes`).
- Exposes `attach_child(child_pid: u32) -> Result<()>` that opens the child with `OpenProcess(PROCESS_ALL_ACCESS, ...)` then `Job::assign_process(handle)`.
- Modify `wrap()` in `mod.rs`: non-macOS arm returns a `SandboxedCommand` whose `program` is a small wrapper that joins the child to the job via a `pre_exec`-style call. **Real plan**: rather than wrapping via `sandbox-exec`, ship a `windows_apply_sandbox_to_command(cmd: &mut Command, policy: &SandboxPolicy)` function the spawner calls before `.spawn()`. This keeps `wrap()` returning a thin pass-through.

Tests (gated `#[cfg(target_os = "windows")]`):
- `job_terminates_child_on_kill_on_close`
- `job_memory_limit_blocks_oversized_alloc` (uses a memory-hungry child process and observes the kill)

#### C2 — Restricted token

Add `apply_restricted_token(cmd: &mut Command) -> Result<(), String>` in `sandbox/windows.rs`. Pattern from §2.2 (Codex `token.rs`).
- Use `DISABLE_MAX_PRIVILEGE` flag on `CreateRestrictedToken`.
- Spawn the child via the existing `std::process::Command`. Restricted-token-via-process requires `CreateProcessAsUserW`; **we will use a small wrapper that applies the token to the child *after* spawn via `AssignProcessToJobObject` + token duplication**, NOT require spawning through `CreateProcessAsUserW` in v1. This is a known gap documented in code (`TODO(verify)` for the v2 change).

Tests:
- `restricted_token_blocks_token_specific_writes` — child tries `echo x > <token-only-deny>/file.txt`; the file must not be created.

#### C3 — Optional, deferred until C1+C2 green (SUPERSEDED)

AppContainer via `rappct`. **Historical**: C3 shipped as the C5 AppContainer
broker (per-spawn profiles, package-SID ACLs) — see
`PORT_MACOS_TO_WINDOWS_FINAL.md` §10.6.

### Milestone D — Auth (Windows Hello)

**D1**. In `src-tauri/src/backend/auth.rs:78-100`, add `#[cfg(target_os = "windows")] pub fn hello_available() -> bool` per §2.4. Reuse existing `WinRtGuard` + thread helper.
**D2**. Add `#[cfg(target_os = "windows")] pub fn verify_user(message: &str) -> Result<bool, String>` calling `KeyCredentialManager::RequestCreateAsync`.

Tests: mirror macOS tests (gated `#[cfg(target_os = "windows")]`).

`TODO(verify)`: WinRT async `.get()` vs `.join()` API in current `windows` version.

### Milestone E — GPU detection

**E1**. In `src-tauri/src/backend/hardware.rs:370`, add `#[cfg(target_os = "windows")] fn detect_gpu()` per §2.3. Wire to `collect_hardware()` alongside existing macOS + Linux arms.

Tests: `#[cfg(target_os = "windows")]` test asserts adapter list is non-empty on real Windows runners (gate the test with `#[ignore]` for CI).

### Milestone F — Oracle GPU backend

#### F1 — prep

Verify:
- `ort` versions `2.0.0-rc.10` (macOS) and `2.0.0-rc.12` (Windows) coexist in the same workspace without conflict. If not, unify.
- DirectML EP loads with the existing ONNX Qwen3 export.

If either fails: **defer Milestone F**. macOS Metal path stays untouched.

#### F2 — wiring

`oracle-core/src/embed/ort_backend.rs` — add Windows arm that registers `DirectML` EP (§2.5). macOS arm unchanged.

#### F3 — embedder abstraction

`oracle-core/src/embedder.rs:59-64` — add `#[cfg(target_os = "windows")]` Windows device-arm analogous to macOS Metal. macOS Metal untouched.

### Milestone G — Mem-pressure backpressure

Deferred (per default in plan). If requested: `GlobalMemoryStatusEx` via `windows::Win32::System::Memory`.

### Milestone H — CI matrix

Add `.github/workflows/ci.yml`:
- Matrix `ubuntu-latest`, `macos-latest`, `windows-latest`.
- Each runs `cargo test --all`.
- Cache via `Swatinem/rust-cache`.

Source: confirmed above.

---

## 6. Open product questions (need answers before execution)

| # | Question | My default |
|---|---|---|
| Q1 | ARM64 support in initial Windows port? (wry#1665) | x86_64 only; ARM64 = best-effort later |
| Q2 | Does any devboule feature require elevation? (tauri#13926) | Default: no. If yes, refactor to UI-unprivileged + helper-as-elevated |
| Q3 | Is `is_enforced() -> true` on Windows acceptable after C1+C2? | Yes, with comment documenting the Seatbelt gap |
| Q4 | MSIX in v1? | Out of scope; separate plan |
| Q5 | `keyring` migrate to v4 + `windows-native-keyring-store`? | Hold current 3.6; separate plan |
| Q6 | GPU detect fallback chain | DXGI primary; `("unknown", None, "unknown")` if no adapter |

---

## 7. What I will NOT do

- Remove or simplify any macOS file/block/test.
- Touch the `AppleFmClient` `#[cfg(target_os = "macos")]` arm. I will ADD a `TODO(next-plan)` comment; nothing else.
- Bump `candle` version on macOS.
- Implement Aion 1.0 Rust bindings (no Rust SDK ships; out of scope).
- Modify `sandbox/seatbelt.rs`.

---

## 8. Verification discipline (re-stated)

Before each milestone lands, I do **one fresh websearch** on that milestone's load-bearing claim. The plan was built on verified sources; execution is gated on continuing verification. Anything I can't verify at code-cut time becomes `TODO(verify)` in the code with the source URL.

The reviewer loop also gates each milestone: `worker` writes → `reviewer` (DeepSeek V4 Pro) reviews → `oracle` (GLM-5.2 max) reviews for sensitive-area stories (C, D, F) → I (planner, minimax m3) read the final diff.

---

## 9. Documents referenced (URL list, for re-verification)

### Tauri
- <https://v2.tauri.app/distribute/windows-installer/>
- <https://v2.tauri.app/reference/config/>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/struct.WixConfig.html>
- <https://schema.tauri.app/config/2>
- <https://v2.tauri.app/plugin/>
- <https://v2.tauri.app/security/capabilities/>
- <https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins>
- <https://github.com/tauri-apps/tauri-action>
- <https://v2.tauri.app/distribute/pipelines/github/>

### Crates
- <https://crates.io/crates/win32job>, <https://docs.rs/win32job/2.0.3>, <https://github.com/ohadravid/win32job-rs>
- <https://crates.io/crates/rappct>, <https://docs.rs/crate/rappct/latest>, <https://github.com/cpjet64/rappct>
- <https://crates.io/crates/uiautomation>, <https://github.com/leexgone/uiautomation-rs>
- <https://crates.io/crates/windows-wfp>, <https://github.com/lostyzen/windows-wfp>
- <https://crates.io/crates/wfp>, <https://github.com/dlon/wfp-rs>
- <https://crates.io/crates/ort>, <https://docs.rs/crate/ort/2.0.0-rc.12/features>, <https://docs.rs/ort/latest/ort/ep/directml/struct.DirectML.html>
- <https://crates.io/crates/keyring>, <https://crates.io/crates/windows-native-keyring-store>
- <https://crates.io/crates/userboundkey-kcm>
- <https://crates.io/crates/keepawake>

### Microsoft docs
- <https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects>
- <https://learn.microsoft.com/en-us/windows/win32/secauthz/restricted-tokens>
- <https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken>
- <https://learn.microsoft.com/en-us/windows/win32/api/winsafer/nf-winsafer-safercomputetokenfromlevel>
- <https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgifactory1-enumadapters1>
- <https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc1>
- <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/start>
- <https://learn.microsoft.com/en-us/uwp/api/windows.security.credentials.keycredentialmanager>
- <https://learn.microsoft.com/en-us/windows/apps/develop/security/windows-hello>
- <https://learn.microsoft.com/en-us/windows/ai/apis/phi-silica>
- <https://blogs.windows.com/windowsdeveloper/2026/06/02/build-2026-furthering-windows-as-the-trusted-platform-for-development/>
- <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate>

### Reference implementations
- <https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/token.rs>
- <https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/process.rs>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/token.rs>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/launch.rs>
- <https://github.com/alexcrichton/rustjob/blob/master/src/main.rs>
- <https://github.com/microsoft/Aion-Instruct-Preview-Sample>
- <https://github.com/codeberg.org/hongminhee/rust-windows-hello-sample>
- <https://github.com/bitwarden/clients/blob/789d66ce880933c1699545c3b1000e549359ca6a/apps/desktop/desktop_native/core/src/biometric/windows.rs>
- <https://github.com/crynta/terax-ai/blob/460657aa/src-tauri/src/modules/proc/job.rs>

### Known bugs being avoided
- <https://github.com/tauri-apps/wry/issues/1665> (WebView2 ARM64 deadlock)
- <https://github.com/tauri-apps/tauri/issues/13926> (Admin Protection breaks WebView2)
- <https://github.com/tauri-apps/tauri/issues/13572> (WebView2 download fails with embedBootstrapper)

### devboule internals (verified by my own reads)
- `src-tauri/Cargo.toml:55` — keyring dep with both target features
- `src-tauri/Cargo.toml:169-180` — macOS-only deps (block stays untouched)
- `src-tauri/Info.plist` — 11 lines, `NSAppSleepDisabled = true`
- `src-tauri/src/backend/sandbox/mod.rs:217` — `is_enforced()` tri-state
- `src-tauri/tauri.conf.json` — current top-level schema (no `bundle.windows` yet)
- `oracle-core/Cargo.toml:48-51` — `ort = { version = "2.0.0-rc.10", features = ["coreml"] }` (macOS)

---

**End of plan.** Status: research-verified, awaiting your green light to start Milestone A.
