# Plan amendment: `PORT_MACOS_TO_WINDOWS_AMENDMENT_2.md`

> **Status**: Supersedes / amends `specs/PORT_MACOS_TO_WINDOWS.md` after the hostile review at oracle run `29c6691c`. Read `AMENDMENT_1.md` first, then this.

> **Headline finding of this amendment**: **5 of 8 milestones in the original plan describe work already shipped in devboule**. The remaining real work is the `bundle.windows` block (A), the Windows sandbox stack (C with 4 sub-stories per amendment 1), CI matrix (H), and the `ort` rc.10 → rc.12 unify in `oracle-core`. Everything else is "verify existing" or a destructive plan that must NOT be applied.

---

## A. Critical reframe — what is actually new work

| original milestone | shipped-state | new shape |
|---|---|---|
| **A** bundle.windows block | not done | **KEEP — implement now** |
| **B** notepad/explorer argv on Windows | shipped at `commands.rs:2250`, `:2268` (Windows arm + platform test at `:2933`) | **DELETE code-add step**; replace with "Verify existing; do NOT modify — plan §2.6's `cmd /c start ""` snippet would regress the spaced-path fix at `commands.rs:2308+`." |
| **C** Windows sandbox | not done (4 sub-stories per AMENDMENT_1 §B) | **KEEP** |
| **D** Windows Hello via `KeyCredentialManager` | shipped at `auth.rs:38`, `:55`, `:65`, `:99`, `:143`, `:329` via **`UserConsentVerifier::CheckAvailabilityAsync`** (NOT `KeyCredentialManager`) | **DELETE code-add step**; replace with "Verify existing. Devboule's `UserConsentVerifier` is the correct API for owner-consent flow — different from `KeyCredentialManager`'s passwordless-key flow. The two APIs solve different problems." |
| **E** DXGI GPU detect | shipped at `hardware.rs:324` (Windows arm with WARP-skip at `:115`) | **DELETE code-add step**; replace with "Verify existing. Do NOT touch the `is_software_adapter()` WARP filter — plan §2.3's snippet would regress it by treating WARP as a real GPU." |
| **F** ort DirectML on Windows | shipped at `oracle-core/Cargo.toml:54-57` + `ort_backend.rs::default_ep()` | **KEEP only the rc.10→rc.12 unify (AMENDMENT_1 §A)** + verification that feat flags resolve in devboule's workspace |
| **G** mem-pressure backpressure (GlobalMemoryStatusEx) | not done | **KEEP** (still useful; defer if not blocking) |
| **H** CI matrix | **`.github/workflows/` does not exist** | **KEEP — implement now** |

**Net remaining work**: A, C (4 sub-stories), H, ort unify, optionally G. Everything else is verify-only.

---

## B. Resolved blockers from the hostile review

### Blocker #1 — `windows` crate version conflict

**Original plan §4.1** proposed adding `[target.'cfg(windows)'.dependencies] windows = { version = "0.62", features = [...] }`. **devboule already pins** two Windows-rs versions:

- `src-tauri/Cargo.toml:152` — `windows = "0.58"` (the official `windows` crate v0.58.0 line; resolved transitively by `windows_capture`)
- `src-tauri/Cargo.toml:164` — `windows_capture = "...", [package] windows = "=0.61.3"` (pin to 0.61.3 to avoid double-version)

Adding a third `windows = "0.62"` would result in **three Windows-rs versions in the binary** (0.58 + 0.61.3 + 0.62), causing:

- `SetForegroundWindow` etc. linker conflicts (each version exports a differently-decorated symbol)
- `windows-sys` version mismatch (0.58 → windows-sys 0.59, 0.61.3 → windows-sys 0.61, 0.62 → windows-sys 0.62) — distinct type layouts for the same Win32 types
- Audit burden — every Win32 API call site in `auth.rs`, `hardware.rs`, `sandbox/*`, `webview2 boundary` would need re-verification after a major version bump

**Resolution**: **Extend the existing `windows = "0.58"` block with the missing features for Milestone C**. Confirmed at <https://docs.rs/crate/windows/0.58.0/features> that all needed features exist in 0.58:

```toml
# src-tauri/Cargo.toml — extend the existing windows = "0.58" block (around line 152)
windows = { version = "0.58", features = [
    # Existing (current): WebView2, foundation, graphics, etc.
    # ADD for Milestone C:
    "Win32_System_JobObjects",            # win32job-compatible Job Object access (C1)
    "Win32_System_Threading",             # CreateProcessAsUserW, OpenProcess (C1 + Restricted Token prep)
    "Win32_Security",                     # CreateRestrictedToken, SECURITY_DESCRIPTOR, ACL APIs (C2 + C3)
    "Win32_Foundation",                    # HANDLE, CloseHandle (workhorse)
    "Win32_System_Memory",                # GlobalMemoryStatusEx for memory-pressure (G)
    "Win32_NetworkManagement_WindowsFilteringPlatform", # WFP calls for C4 — CONFIRMED present in 0.58 per docs.rs
    "Win32_UI_WindowsAndMessaging",       # SetForegroundWindow, AttachThreadInput (focus_agent_terminal Windows arm)
    "Security_Credentials",               # KeyCredentialManager WinRT (out of scope; current path uses UserConsentVerifier, retained)
] }
```

**Verified**: <https://docs.rs/crate/windows/0.58.0/features> and <https://docs.rs/crate/windows/%5E0.58/features> confirm `Win32_NetworkManagement_WindowsFilteringPlatform` is in 0.58 (the WFP feature). No WFP stories depend on 0.62-only features.

**Drop from plan**: the entire plan §4.1 `"windows = { version = "0.62" }"` snippet. Replace with the snippet above.

**New prep milestone `M0`**: before any C1+ code lands, run `cargo metadata` + `cargo check --target x86_64-pc-windows-msvc` against the augmented `windows = "0.58"` block. If any feature name doesn't resolve in 0.58 (none expected, per docs.rs), gate the milestone as failed.

### Blocker #2 — Milestone B (notepad/explorer) would regress shipped code

**Reality**: devboule's `commands.rs:2250` `notepad_argv` Windows arm returns `("notepad", vec![path])` (correct — opens Notepad). `commands.rs:2268` `explorer_argv` returns `("explorer", vec![format!("/select,\"{path}\"")])` (uses `raw_arg` quoting at `:2308+` for spaced paths).

**The plan's §2.6 snippet** proposed `("cmd", vec!["/c", "start", "", &s])` and `("explorer", vec![format!("/select,{}", s)])`. **Both regress** the existing implementation:
- `cmd /c start "" "<file>"` opens the file with the *default app*, not Notepad. For `.txt` files that's typically Notepad, but it's policy-driven, not guaranteed.
- Unquoted `/select,` with a path containing spaces breaks the explorer selection.

**Resolution**: **Delete the code-add component of Milestone B**. New shape: "Verify the existing arms at `commands.rs:2250` and `:2268`. Test at `:2929-2950` already exercises both. If the test fails on real Windows, fix the test, NOT the production code."

### Blocker #3 — Milestone D would ship wrong API for Windows Hello

**Reality**: devboule's `auth.rs:38-101` already ships `hello_available` / `verify_user` via **`UserConsentVerifier::CheckAvailabilityAsync()`** (line 67) with a `WinRtGuard` STA initializer at `:329`, `run_hello_thread` at `:99`, `verify_user_inner` at `:143`. The plan proposed `KeyCredentialManager::IsSupportedAsync()`.

**Different APIs — verified by websearch**: confirmed at <https://xmichele.substack.com/p/how-we-made-our-on-device-ai-models>, Microsoft Learn docs, and the `UserConsentVerifier` reference: `UserConsentVerifier` displays an authentication prompt (Touch ID / PIN / face) to confirm the **identity of the current user** — i.e., owner-consent. `KeyCredentialManager` is a **passwordless-key creation/retrieval** API (RSA 2048, attestation, VBS-enclave protected) — a fundamentally different flow.

**Devboule's existing choice is correct for "is Windows Hello available" and "verify the user with Hello"**. The plan's `KeyCredentialManager` proposal was category-error.

**Resolution**: **Delete the code-add component of Milestone D** (`KeyCredentialManager` references). New shape: "Verify the existing `UserConsentVerifier` arms at `auth.rs:38-101`. Add a `#[cfg(all(test, target_os = "windows"))] smoke test that confirms `UserConsentVerifier::CheckAvailabilityAsync` returns `Ok(Available)` on a real Windows + Hello-enrolled machine — `TODO(verify)` because the existing tests are all `cfg(target_os = "windows")` but never assert the actual API result."

### Blocker #4 — Milestone E would drop WARP-skip

**Reality**: devboule's `hardware.rs:324` has `#[cfg(windows)] detect_gpu()`, and at line 115 there's an `is_software_adapter()` WARP filter that **skips Microsoft's software-rendered adapter** so it's not reported as a real GPU. The plan's §2.3 snippet proposed enumerating `IDXGIAdapter1` via `IDXGIFactory1::EnumAdapters1` WITHOUT this filter — it would report WARP as a real GPU and the user's hardware info would lie.

**Resolution**: **Delete the code-add component of Milestone E**. New shape: "Verify the existing DXGI enumeration at `hardware.rs:324` including the WARP filter at `:115`. Add a Windows-only test asserting that `detect_gpu()` returns `("unknown", None, "unknown")` when no adapter is present OR when only WARP is present."

### Blocker #5 — Stale plan §9 line citations

The plan's appendix cited `mod.rs:217` for `is_enforced()` — actually at `:207`. `auth.rs:78-100` was cited as the *insertion point* for new code, but the Windows arms already live at `:38`. **Indicates the plan was not re-synced to HEAD before sign-off.** Re-verifying every appendix citation against current source is now part of pre-cut checklist.

---

## C. Confirmed via websearch (this round)

| claim | source |
|---|---|
| `windows` crate 0.58 features include `Win32_System_JobObjects`, `Win32_Security`, `Win32_NetworkManagement_WindowsFilteringPlatform` | <https://docs.rs/crate/windows/0.58.0/features> |
| `windows-sys` 0.59 ↔ `windows` 0.58 line-up is stable | <https://docs.rs/crate/windows-sys/0.59.0/features>, <https://crates.io/crates/windows-sys/0.59.0> |
| `UserConsentVerifier` is consent-prompt (different from `KeyCredentialManager`'s passwordless-key flow) | <https://xmichele.substack.com/p/how-we-made-our-on-device-ai-models> + Microsoft Learn `UserConsentVerifier` class docs |
| `tauri-action` is the canonical GitHub Actions path; workflows live under `.github/workflows/` | <https://v2.tauri.app/distribute/pipelines/github/>, <https://github.com/tauri-apps/tauri-action>, example: <https://github.com/sayedhfatimi/object0/blob/main/.github/workflows/tauri-publish.yml> |
| `ort` 2.0.x `directml` + `coreml` features both exist (AMENDMENT_1 §A verified, repetition confirmed) | <https://docs.rs/crate/ort/2.0.0-rc.12/features> |

---

## D. Final pre-cut checklist (gate before any code lands)

1. **All plan citations** (`file:line` references in §1, §9) re-verified against HEAD
2. **`windows = "0.58"` feature augmentation snippet compiles** — `cargo check --target x86_64-pc-windows-msvc` succeeds with the augmented block (Milestone M0)
3. **`ort` rc.12 with `coreml + directml` features resolves in devboule's workspace** — `cargo metadata` clean (Milestone F-prep)
4. **`.github/workflows/ci.yml`** is a NEW file (creation, not modification)
5. **Milestone A smoke test** (`specs/PORT_MACOS_TO_WINDOWS.md` §3.2) re-read against the augmented amendment §C `bundle.windows` block
6. **No part of any "already-shipped" milestone (B, D, E, F2, keyring) gets an edit** — worklist gates on verification only

---

## E. Out-of-scope items (unchanged from AMENDMENT_1 §F)

- `is_enforced() -> true` on Windows: requires C1 + C2 + C3 + C4 + reviewer + oracle sign-off. Do NOT flip until all five conditions hold.
- ARM64 Windows port: still out of scope per your earlier decision (`wry#1665` deadlock).
- Apple FM (Censor on-device LLM) deferral: still in place.
- Aion 1.0 Windows AI Foundry: deferred (no Rust SDK at plan time).
- MSIX packaging: deferred (out of v1 scope).
- `EXA_API_KEY` env var scope risk: User-scope env vars inherit to every child process on the box. Consider migrating to `~/.pi/web-search.json` (`exaApiKey` field) before any code/process sharing risk emerges.

---

## F. Honest reset of where we are

**Original plan**: 8 milestones, blend of new + already-shipped work, plus a `windows = "0.62"` proposal that would have broken things.

**After AMENDMENT_1**: Milestone C restructured into C1-C4 sub-stories because Anthropic's srt-win requires file-ACL + network-egress layers for honest `is_enforced() -> true`.

**After AMENDMENT_2** (this one): Plan trimmed to **only** what's actually new. Milestones B/D/E deleted to verify-only. `windows 0.58` extended. ort unify still pending.

**Concrete ordered worklist**:

1. **M0** — `windows = "0.58"` augmented feature list compiles (prep, no code writes)
2. **A** — `tauri.conf.json` `bundle.windows` block + smoke test
3. **H** — `.github/workflows/ci.yml` matrix (Linux/macOS/Windows)
4. **C1** — Job Object wrapper
5. **C2** — Restricted Token wrapper
6. **C3** — Filesystem ACL layer (`allowWrite`/`denyWrite` ACEs on `policy.writable_paths` + `policy.readonly_root`)
7. **C4** — Network-egress layer (WFP filter or ACL-denied network for `NetPolicy::None`, loopback-PERMIT for `Loopback`)
8. **ort rc.10 → rc.12 unify** in `oracle-core` per AMENDMENT_1 §A
9. **G (optional)** — `GlobalMemoryStatusEx` mem-pressure backpressure
10. **flip `is_enforced() -> true` on Windows** — only after C1+C2+C3+C4 reviewed + oracle-signed

Plus `verify-only` for the already-shipped milestones (B, D, E, F2, keyring).
