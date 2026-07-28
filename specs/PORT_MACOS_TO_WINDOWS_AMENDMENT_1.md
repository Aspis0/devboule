# Plan amendment: `PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md`

> **Status**: This amends `specs/PORT_MACOS_TO_WINDOWS.md`. Read it after that doc.

> All citations now web-search-verified with the `EXA_API_KEY` enabled. Anything previously tagged `TODO(verify)` is either cleared below or remains an explicit `TODO(verify)`.

---

## A. `ort` version coexistence — RESOLVED

**Finding**: `ort 2.0.0-rc.12` declares both `directml` and `coreml` as features (`directml = ["ort-sys/directml"]`, `coreml = ["ort-sys/coreml"]`). Both RCs target the same ONNX Runtime 1.24 underneath.

**Implication**: we do NOT need two RCs. We can use `ort = "=2.0.0-rc.12"` (single version) in one workspace, gated per-target:

```toml
# oracle-core/Cargo.toml — single RC, single feature per target
[dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray", "coreml"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray", "directml"] }
```

**Caveats (verified by cited sources)**:
- Cargo feature unification in workspaces can cause issues for target-conditional features. Mitigation: `default-features = false` + explicit per-target re-enables.
- Cargo issue #11779 (open): target-specific features don't always resolve in workspace deps. **Verify with `cargo metadata` in F1 milestone prep before committing the snippet.**

**Old plan to delete**: the F1 prep claimed "fix the Windows RC to rc.12, leave macOS on rc.10". Replace with: migrate everything to `=2.0.0-rc.12` with feature flags.

**Sources**:
- <https://docs.rs/crate/ort/latest/source/Cargo.toml>
- <https://docs.rs/crate/ort/latest/features>
- <https://github.com/pykeio/ort/blob/main/Cargo.toml>
- <https://github.com/pykeio/ort/discussions/588>
- <https://doc.rust-lang.org/cargo/reference/features.html>
- <https://nickb.dev/blog/cargo-workspace-and-the-feature-unification-pitfall/>
- <https://github.com/rust-lang/cargo/issues/11779>

---

## B. srt-win security model — RESHAPES Milestone C

**Finding**: Anthropic's srt-win (the closest production Windows sandbox reference) uses a 4-component stack:

1. **Dedicated `srt-sandbox` local user account** — provisioned by the broker, every sandboxed child runs as that user.
2. **Two-hop launch via `CreateProcessWithLogonW`** — broker (real user) → runner (srt-sandbox user, builds lockdown) → child (srt-sandbox user + restricted token).
3. **Restricted token** via `CreateRestrictedToken` with `SidsToDisable = [BUILTIN\Administrators, ...]` (logon SID strip is the load-bearing step).
4. **Job Object** for lifecycle (kill-on-close, memory, CPU).
5. **WFP filters** keyed on `srt-sandbox`'s SID for network confinement (loopback-only by default).
6. **Filesystem ACLs** via `srt-win acl grant|stamp|restore|revoke` — `allowRead`, `allowWrite`, `denyRead`, `denyWrite`. `denyWrite` takes precedence over `allowWrite` (write is allow-only with explicit denies); `allowRead` takes precedence over `denyRead` (read is broad-allow with explicit denies).

**Implication for devboule** (which has its own `SandboxPolicy { readonly_root, writable_paths, net, rlimits }`):

| devboule policy field | srt-win equivalent |
|---|---|
| `readonly_root` (no writes) | `denyWrite` ACE on the path (file: `MODIFY` DENY; parent: `FILE_DELETE_CHILD` DENY) |
| `writable_paths` | `allowWrite` ACE on each path |
| `writable_paths` (read) | `allowRead` covers it implicitly (read is broad-default) |
| `net::None` | no WFP filter = full block, OR `deny network*` ACL on the proxy port range |
| `net::Loopback` | WFP loopback PERMIT filter (port range like `60080–60089`) |
| `net::Enabled` | no WFP filter = full network |
| `rlimits` | win32job ExtendedLimitInfo |

**What's actually new (changes the plan)**:

- **The plan's C1+C2 ("Job Object + Restricted Token") is INSUFFICIENT for honest `is_enforced() -> true`.**
- A Windows backend in devboule MUST include a filesystem-ACL layer (`srt-win`-style or `rappct` AppContainer or both) AND a network-egress layer (WFP filter or Job Object + `deny network*` ACL or AppContainer capability gating).
- **New milestone introduced: C0 — Filesystem ACL layer (the missing piece)**. Without C0, `readonly_root` isn't enforced on Windows. The macOS code is correct because Seatbelt's deny-by-default filesystem rule covers it; on Windows, Job Object + Restricted Token don't see file permissions the same way.

**New milestone structure for C**:

- **C1**: Job Object (kill-on-close + memory/CPU limits)
- **C2**: Restricted Token (`CreateRestrictedToken` with `DISABLE_MAX_PRIVILEGE | 4` + logon SID strip)
- **C3 (NEW, formerly implied)**: Filesystem ACL layer — `allowWrite` on `policy.writable_paths`, `denyWrite` on `policy.readonly_root`, `allowRead`/`denyRead` follow devboule's policy structure. Use raw `windows` crate + the `grantWindowsAcl` pattern from srt-win.
- **C4 (NEW)**: Network-egress layer — WFP filter for `srt-sandbox`-style SID keying OR simpler: `deny network*` for `NetPolicy::None`, loopback PERMIT for `Loopback`, no filter for `Enabled`.
- **`is_enforced() -> true` on Windows is now gated on C1+C2+C3+C4.** Until C3 lands, the docstring for `is_enforced()` says explicitly that file-deny coverage is partial (Job Object + Restricted Token are not Seatbelt-equivalent on filesystem rules).

**Old plan text to delete**: Milestone C's claim that C1+C2 cover the policy.

**Sources**:
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/src/sandbox/windows-sandbox-utils.ts>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/acl.rs>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/token.rs>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/launch.rs>
- <https://github.com/anthropic-experimental/sandbox-runtime/commit/4860b4d8fc116db3b0570537c3b8daa50730793f>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/cli.rs>
- <https://deepwiki.com/anthropic-experimental/sandbox-runtime/6.4.3-windows-acl-filesystem-isolation>
- <https://deepwiki.com/anthropic-experimental/sandbox-runtime/6.4.4-two-hop-launch-and-job-objects>
- <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/Cargo.toml>

---

## C. Tauri v2 `bundle.windows` schema — RESOLVED

**Finding**: schema confirmed at <https://schema.tauri.app/config/2>.

**Concrete Tauri v2 `bundle.windows` block** (cleared `TODO(verify)` from §3.1 of the plan):

```jsonc
"bundle": {
  // ... existing fields ...
  "windows": {
    "wix": {
      // NsisConfig / WixConfig fields per schema. Defaults fine.
      // "language": ["en-US"]
    },
    "nsis": {
      "installMode": "perMachine"   // NSISInstallerMode: CurrentUser | PerMachine | Both. Default = CurrentUser.
      // "installerIcon": "icons/icon.ico",
      // "languages": ["English"]
    },
    "webviewInstallMode": {
      "type": "downloadBootstrapper",   // Skip | DownloadBootstrapper | EmbedBootstrapper | OfflineInstaller | FixedRuntime
      "silent": true
    }
    // "signCommand": null               // TODO(verify): placeholder until code-sign cert path is decided.
  }
}
```

**Old plan to update**: my plan said `"installMode": "perMachine"` without flagging that it overrides the Tauri default of `CurrentUser`. Also the plan said `WebviewInstallMode` was a simple string; it's actually an object `{ type, silent }`. **Corrected here.**

**Sources**:
- <https://v2.tauri.app/distribute/windows-installer/>
- <https://v2.tauri.app/reference/config/>
- <https://schema.tauri.app/config/2>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.NSISInstallerMode.html>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/struct.NsisConfig.html>
- <https://docs.rs/tauri-utils/latest/tauri_utils/config/struct.WixConfig.html>
- <https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi>
- <https://github.com/tauri-apps/tauri-apps/tauri-docs/blob/v2/src/content/docs/distribute/windows-installer.mdx>

---

## D. `keyring` setup — RESOLVED, no change needed

**Finding**: devboule's existing `Cargo.toml:55` declaration is correct:

```toml
keyring = { version = "3.6", features = ["windows-native", "apple-native"] }
```

`keyring 3.6.3`'s `windows-native` feature gates the `windows-sys` backend; `apple-native` gates the macOS Keychain security-framework. Per-target feature resolution handles the rest. **No code change needed for Windows credential storage.**

**Implication**: Milestone "keychain on Windows" was already effectively done. No new work item; just a verification line in the plan.

**Worth considering later (out of scope)**:
- `keyring 4.x` ecosystem migration (separate plan)
- `tauri-plugin-keyring-store v0.2.0` adoption (Stronghold-shaped, different API surface — also a separate plan)
- `windows-native-keyring-store v1.1.0` is already the underlying store for both `keyring` (via feature) and the standalone route. **No new dependency.**

**Sources**:
- <https://docs.rs/crate/keyring/3.6.3>
- <https://docs.rs/crate/keyring/3.6.3/features>
- <https://docs.rs/keyring/latest/x86_64-pc-windows-msvc/keyring/windows/index.html>
- <https://crates.io/crates/windows-native-keyring-store>
- <https://github.com/open-source-cooperative/windows-native-keyring-store/releases>
- <https://crates.io/crates/tauri-plugin-keyring-store>

---

## E. Concrete next-milestone ready check

After this amendment, **Milestone A is well-defined** with the corrected `bundle.windows` block from §C above. F1 (ort prep) is well-defined with the single-RC feature-gated snippet from §A. The plan no longer has "TODO(verify)" gaps on the load-bearing claims for those two milestones.

**Milestone C** now has 4 sub-stories instead of 3 (add C3 file-ACL + C4 network). `is_enforced() -> true` on Windows is strictly gated on all four landing + reviewer + oracle sign-off.

---

## F. Open items kept

- `is_enforced() -> true` on Windows still needs §B's C0+C1+C2+C3+C4 done + reviewed.
- ARM64 Windows port: still out of scope per your earlier decision (`wry#1665` deadlock).
- Apple FM (Censor on-device LLM) deferral: still in place.
- Aion 1.0 Windows AI Foundry: deferred (no Rust SDK at plan time).
- MSIX packaging: deferred (out of v1 scope).
- Cargo workspace feature unification: verify with `cargo metadata` in F1 prep.
