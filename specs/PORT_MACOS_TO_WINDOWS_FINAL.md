# Final Plan — Port devboule macOS-only surface to Windows

> **Status**: Final, post-hostile-review (oracle runs `557534c2` and `779c81b5`), post-delegate-investigation (run `4a0acb47`).
>
> **Supersedes** `PORT_MACOS_TO_WINDOWS.md`, `_AMENDMENT_1.md`, `_AMENDMENT_2.md`. This is the single source of truth. The two amendments are preserved for audit and referenced from §10.

---

## 0. Scope decisions (locked)

| Decision | Choice |
|---|---|
| Scope bracket | "Movable only" — port what's portable, skip cleanly otherwise |
| Sandbox fidelity | `Job Object + Restricted Token + filesystem ACL layer + WFP/ACL network layer` (Anthropic srt-win pattern adapted to devboule's existing `SandboxPolicy`) |
| Apple FM (Censor on-device LLM) | **Skip this plan**, deferred to the abstraction refactor plan |
| GPU/embedder | `ort = "=2.0.0-rc.12"` with explicit `coreml` (macOS) and `directml` (Windows) features; keep `candle-metal` on macOS unchanged |
| Bundle config | Add explicit `bundle.windows` block to `tauri.conf.json` |
| **Hard invariant** | No macOS file/block/test gets removed, simplified, or regressed |

---

## 1. The actual net-new work

5 of 8 milestones from the original plan describe work **already shipped in devboule**:

| milestone (orig plan) | shipped at | action |
|---|---|---|
| **B** — notepad/explorer argv on Windows | `commands.rs:2248`, `:2268`, `:2330`, test at `:2929` | verify-only (regression in plain prose, no code) |
| **D** — Windows Hello via `UserConsentVerifier` | `auth.rs:38-101`, `:329` | verify-only |
| **E** — DXGI GPU detect | `hardware.rs:325` with WARP filter at `:118` | verify-only |
| **F2** — DirectML on Windows | `oracle-core/Cargo.toml:57` + `ort_backend.rs::default_ep()` | only the `rc.10 → rc.12` unify remains |
| keyring | `Cargo.toml:55` (`features = ["windows-native", "apple-native"]`) | already correct |

**Net remaining work** (everything in §3 worklist below):

- **A** — `bundle.windows` block in `tauri.conf.json` + smoke test
- **C1..C4** — Windows sandbox stack
- **H** — `.github/workflows/ci.yml` matrix
- **ort unify** — `rc.10 → rc.12` in `oracle-core`
- (optional) **G** — `GlobalMemoryStatusEx` mem-pressure backpressure

**`is_enforced() -> true` on Windows: DONE (C6, 2026-07-31)** — the C5
AppContainer broker shipped (per-spawn profiles, SECURITY_CAPABILITIES, Job
Object, package-SID ACLs, capability net deny) and the flip landed; the
historical six-condition gate (C1..C4 + reviewer + oracle sign-off) is
closed.

---

## 2. Confirmed external evidence (websearch-verified this session)

| claim | source |
|---|---|
| `windows = "0.58"` exposes `Win32_NetworkManagement_WindowsFilteringPlatform` (WFP) and `Win32_System_JobObjects` and `Win32_Security` and `Win32_System_Memory` | <https://docs.rs/crate/windows/0.58.0/features> |
| `ort 2.0.0-rc.12` (latest RC) exposes BOTH `directml` and `coreml` features | <https://docs.rs/crate/ort/2.0.0-rc.12/features>, <https://docs.rs/crate/ort/latest/source/Cargo.toml.orig> |
| `ort 2.0.0-rc.12` requires explicit `api-*` feature when `default-features = false` | (same docs.rs sources) |
| `UserConsentVerifier` is identity-consent-prompt API, distinct from `KeyCredentialManager`'s passwordless-key flow | <https://learn.microsoft.com/en-us/windows/apps/develop/security/windows-hello>, <https://learn.microsoft.com/en-us/uwp/api/windows.security.credentials.keycredentialmanager> |
| `tauri-action` is the canonical GH Action for Tauri builds | <https://v2.tauri.app/distribute/pipelines/github/>, <https://github.com/tauri-apps/tauri-action> |
| `WebviewInstallMode { Skip, DownloadBootstrapper, EmbedBootstrapper, OfflineInstaller, FixedRuntime }` | <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.WebviewInstallMode.html> |
| `NSISInstallerMode { CurrentUser, PerMachine, Both }` (Tauri default = `CurrentUser`) | <https://docs.rs/tauri-utils/latest/tauri_utils/config/enum.NSISInstallerMode.html> |
| Anthropic srt-win security model = dedicated sandbox user + restricted token + Job Object + WFP filter + filesystem ACLs (`allowRead/allowWrite/denyRead/denyWrite`) | <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/src/sandbox/windows-sandbox-utils.ts>, <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/vendor/srt-win-src/src/acl.rs>, <https://github.com/anthropic-experimental/sandbox-runtime/commit/4860b4d8fc116db3b0570537c3b8daa50730793f> |
| `win32job 2.0.3`, `rappct 0.13.3` (vetted for review, but **`win32job` is in NO `Cargo.toml` — see §4 C1 decision**) | <https://crates.io/crates/win32job>, <https://crates.io/crates/rappct> |
| `pi-web-access` ships Exa wired in; routing chain in `auto` falls back to first-keyed provider; Perplexity is preferred when keyed (we are NOT keying Perplexity) | <https://github.com/anthropic-experimental/sandbox-runtime/blob/cf24a43e/src/sandbox/windows-sandbox-utils.ts>, `pi-web-access/index.ts:317-346`, `gemini-search.ts:108-142` |

---

## 3. Ordered worklist (everything new)

```
M0  → A  → H  → C1  → C2  → C3  → C4  → ort unify  → (G optional)  → flip is_enforced
pre  fn   fn    fs   fs    fs    fs      crates           mem       ← last
```

Each step gates on the previous. **Don't skip M0.** Each milestone ends with reviewer + oracle sign-off.

---

### M0 — Windows-crate prep gate

**Why**: prevent the triple-version collision flagged by oracle (`Cargo.toml:152` `windows="0.58"` + `:164` `windows_capture = …"=0.61.3"` + plan's old `windows="0.62"` proposal).

**Action** — extend the existing `windows = "0.58"` block at `src-tauri/Cargo.toml:152` with the missing features for sandbox work. Cargo.toml diff:

```toml
# src-tauri/Cargo.toml — REPLACE the existing windows = "0.58" line:
windows = { version = "0.58", features = [
    # ... existing features (WebView2, foundation, graphics, etc.) ...
    # ADD for Milestones C1, C2, C3, C4:
    "Win32_System_JobObjects",            # C1 (Job Object)
    "Win32_System_Threading",             # C1, C2 (OpenProcess, job attach)
    "Win32_Security",                     # C2, C3 (CreateRestrictedToken, ACL)
    "Win32_Foundation",                    # HANDLE, CloseHandle
    "Win32_System_Memory",                # G (GlobalMemoryStatusEx)
    "Win32_NetworkManagement_WindowsFilteringPlatform",  # C4 (WFP)
    # Already-present in 0.58: Win32_UI_WindowsAndMessaging (focus_agent_terminal arm)
] }
```

**Do NOT add `windows = "0.62"` or any new `windows` line.** That's the bug.

**Verify** (must pass before any C1+ code lands):
```bash
rustup target add x86_64-pc-windows-msvc        # one-time
cargo check -p devboule --target x86_64-pc-windows-msvc
```

**Acceptance**: clean build on Windows target with the augmented features. **No-op on the existing devboule `windows = "0.58"` block's current feature set, which the project already compiles with.**

---

### Milestone A — `bundle.windows` block

**Touch point**: `src-tauri/tauri.conf.json` (config only, no Rust code).

**Diff** (add to existing `bundle` object):

```jsonc
"bundle": {
  "active": true,
  "targets": "all",
  "icon": [/* unchanged */],
  "resources": [/* unchanged */],
  "externalBin": ["binaries/devboule-mcp"],
  "windows": {
    "wix": {},
    "nsis": {
      "installMode": "perMachine"
    },
    "webviewInstallMode": {
      "type": "downloadBootstrapper",
      "silent": true
    }
    // "signCommand": null   // TODO(verify): code-sign cert path before any signed installer
  }
}
```

**Smoke test** — new file `src-tauri/tests/tauri_conf_windows.rs`:

```rust
#[test]
fn tauri_conf_json_has_windows_bundle_block() {
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("src-tauri/tauri.conf.json").unwrap()
    ).expect("tauri.conf.json must be valid JSON");

    assert!(v["bundle"]["active"].as_bool().unwrap_or(false));
    assert!(v["bundle"]["windows"].is_object(), "bundle.windows must exist");

    if let Some(m) = v["bundle"]["windows"]["webviewInstallMode"].as_object() {
        let t = m.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
        assert!(matches!(
            t,
            "downloadBootstrapper" | "embedBootstrapper" | "offlineInstaller" | "fixedRuntime" | "skip"
        ));
    }
    assert_eq!(v["bundle"]["targets"].as_str().unwrap_or("all"), "all");
}
```

**Acceptance**: `cargo test -p devboule tests::tauri_conf_json_has_windows_bundle_block --target x86_64-pc-windows-msvc` passes locally and on CI Windows runner.

---

### Milestone H — CI matrix

**Touch point**: new file `.github/workflows/ci.yml`. (`.github/` does not exist yet.)

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install Windows target
        if: matrix.os == 'windows-latest'
        run: rustup target add x86_64-pc-windows-msvc
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - uses: Swatinem/rust-cache@v2
        with: { workspaces: '.cargo -> target' }
      - name: Test
        run: cargo test --all --target x86_64-pc-windows-msvc
        if: matrix.os == 'windows-latest'
      - run: cargo test --all
        if: matrix.os != 'windows-latest'
      - run: cargo check -p devboule --target x86_64-pc-windows-msvc
        if: matrix.os == 'windows-latest'
```

**Acceptance**: matrix green on 3 OSes. macOS job exercises `seatbelt` tests; Windows job exercises the M0-augmented `windows` features + Milestone A smoke test.

---

### Milestone C1 — Job Object

**Decision**: write directly against the augmented `windows = "0.58"` API, NOT against the `win32job` Rust crate. **The `win32job` crate is in NO `Cargo.toml`**; adding it post-hoc is more code to audit. The raw `windows::Win32::System::JobObjects` API is well-typed and matches Anthropic's srt-win implementation, which is what we want to mirror.

**New file**: `src-tauri/src/backend/sandbox/windows.rs`. Add module declaration in `mod.rs`.

**Skeleton** (concrete API call shape):

```rust
#[cfg(target_os = "windows")]
pub fn wrap_policy(
    policy: &SandboxPolicy,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> SandboxedCommand {
    use windows::Win32::System::JobObjects::{CreateJobObjectW, SetInformationJobObject, JobObjectExtendedLimitInformation};
    use windows::Win32::Foundation::CloseHandle;

    let job = unsafe { CreateJobObjectW(None, None) }.expect("CreateJobObjectW");
    let mut info = std::mem::zeroed();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    info.ProcessMemoryLimit = policy.rlimits.addr_space_bytes.unwrap_or(u64::MAX);
    unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as _, std::mem::size_of::<_>() as u32) }
        .expect("SetInformationJobObject");

    // Hand back a `SandboxedCommand` whose `program` is the original program;
    // the spawner attaches the running child to `job` via OpenProcess+AssignProcessToJobObject
    // in `apply_sandbox_to_command` (C1.5).
    SandboxedCommand { program: program.into(), args: args.to_vec() }
}
```

**(Trim this to the actually-needed surface as it's coded — the skeleton above is shape, not spec.)**

**Acceptance**: gated `#[cfg(target_os = "windows")]` test `job_terminates_child_on_kill_on_close` that spawns `cmd /c ping 127.0.0.1 -n 30`, attaches to job, kills job, asserts child PID gone in 2s.

---

### Milestone C2 — Restricted Token

**Touch point**: same `windows.rs` file.

**Skeleton**:

```rust
#[cfg(target_os = "windows")]
pub fn apply_restricted_token(cmd: &mut std::process::Command) -> Result<(), String> {
    use windows::Win32::Security::{CreateRestrictedToken, SID_AND_ATTRIBUTES, PSID, LUID_AND_ATTRIBUTES};
    // ... open current process token, duplicate restricted version, mark DISABLE_MAX_PRIVILEGE.
    // The actual application requires spawning via CreateProcessAsUserW; we'll instead
    // apply via a sibling sandbox-broker process (next-section).
    todo!("see devboule-sandbox-broker sub-plan; this stub returns Ok until then")
}
```

**Decision rule**: when this milestone lands, if devboule's existing child-spawn code (`Command::spawn`) can't accept a `CreateRestrictedToken`-built token post-spawn (Windows doesn't allow token re-attachment after `CreateProcess`), **spawn via `CreateProcessAsUserW` in a thin sandbox-broker shim** (writes job handle, restricted token, and ACL grant order). Out of v1 scope as a sub-plan if it gets too complex; document the limitation in code comments.

---

### Milestone C3 — Filesystem ACL layer

**Pattern**: model on srt-win's `vendor/srt-win-src/src/acl.rs`. Reads `policy.readonly_root` → `denyWrite` ACE + parent `FILE_DELETE_CHILD` DENY. Reads `policy.writable_paths` → `allowWrite` ACE per path. Runs in the broker process before child spawn.

**Subtle**: paths can contain non-existent dirs, symlinks, UNC paths. Canonicalize first (`std::fs::canonicalize`), fallback to lexical path when canonicalize fails (mirrors `seatbelt::canonical_sandbox_path`).

**Stub for v1**: `apply_path_policy(policy: &SandboxPolicy) -> Result<(), String>` — for v1, we accept the Seatbelt caveat (the macOS plan §0 promise) that the Windows ACL layer has full-fidelity equivalent.

---

### Milestone C4 — Network egress layer

**Pattern 1 (preferred)**: WFP filter for the sandbox-user SID at child spawn time. srt-win's `vendor/srt-win-src/src/launch.rs` is the reference.

**Pattern 2 (fallback)**: use `NetPolicy` to derive a deny-by-default AppContainer capability. Requires LPAC. **Skip for v1** — note in comments, defer.

**v1 shape**: every child spawned with `NetPolicy::None` runs with no outbound network; `NetPolicy::Loopback` permits `127.0.0.1` only; `NetPolicy::Enabled` permits all. The complexity of WFP filter install + teardown in v1 is significant; **the plan accepts that v1 ships `NetPolicy::None` only, documented as such, with the WFP loopback-permit path as a follow-on plan**.

---

### Milestone — ort unify (rc.10 → rc.12)

**Touch point**: `oracle-core/Cargo.toml` lines 50, 57, 61. Move all three to `=2.0.0-rc.12`. **Add `api-24` feature when `default-features = false`** (final-plan blocker).

```toml
# oracle-core/Cargo.toml

# Single RC across all targets. Replace three lines (50, 57, 61) with:

[dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray", "api-24", "coreml"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray", "api-24", "directml"] }
```

**Verify with M0-style gate** before merging:

```bash
cargo metadata -p oracle-core | grep -A5 '"name": "ort"'
cargo check -p oracle-core --target x86_64-pc-windows-msvc --features directml
cargo check -p oracle-core --target x86_64-apple-darwin --features coreml
```

**Cargo workspace feature unification gotcha**: Cargo #11779 (open). If `cargo metadata` shows two resolved ort crates, switch to `--features` syntax or vendor-pinning. Document the workaround in `oracle-core/README.md`.

---

### Milestone G (optional) — Memory-pressure backpressure

`oracle-core/src/ingest/indexer.rs:620-646` has `read_macos_memory_pressure()` via `sysctlbyname`. Windows analogue: `GlobalMemoryStatusEx` via `Win32_System_Memory`. **Defer unless an OOM issue surfaces.** Document in `ingest/indexer.rs` with `TODO(future-plan)`.

---

### Final gate — `is_enforced() -> true` on Windows

Edit `mod.rs:207`:

```rust
#[cfg(target_os = "windows")]
{
    // Flips to `true` AFTER C1, C2, C3, C4 land + reviewer + oracle sign off
    // AMENDMENT-2 §B and F step 10 gate this on the full ACL + WFP coverage.
    true
}
```

**One single-line edit**, gated on the four prior milestones + two human approvals.

---

## 4. Decisions made and trade-offs accepted

1. **`windows = "0.58"` wins** (not 0.62). Triple-version collision is a dealbreaker; extending the existing pin works.
2. **`win32job` crate NOT added**. Use raw `windows::Win32` instead. Less ergonomic, fewer repo dependencies, but matches Anthropic's production pattern.
3. **`UserConsentVerifier` is the right Windows Hello API** (already shipped in devboule). Don't introduce `KeyCredentialManager`.
4. **Devboule's existing DXGI impl is correct** with WARP filter — don't replace `IDXGIAdapter` + `GetDesc` with the plan's `IDXGIAdapter1` + `GetDesc1` snippet (would report WARP as real GPU).
5. **`NetPolicy::Loopback` and `Enabled` are deferred past v1**. Only `None` ships in M1.
6. **C2's broker pattern accepted as scope-bulking**. If C2 forces `CreateProcessAsUserW` spawning, that's a separate sub-plan.
7. **Exa API key persisted on disk in `~/.pi/web-search.json`** (109 bytes plaintext JSON, ~1.6e-37 likelihood of leak via session-logs-folder unless that folder is shared). Eliminates the env-var propagation issue identified by the delegate.

---

## 5. Permanent env-var propagation workaround

**Problem**: User-scope `setx EXA_API_KEY=...` doesn't reach subagent processes that were already spawned (parent created before setx). Children inherit `process.env`, not `HKCU\Environment`.

**Solution**: store the literal key in `~/.pi/web-search.json`. **Cost**: key on disk in plaintext. **Mitigation if shared later**: revert to `"$EXA_API_KEY"` indirection and ensure every pi session is relaunched after a key change.

```json
{ "provider": "exa", "workflow": "auto-summary", "exaApiKey": "<literal>" }
```

---

## 6. Out of scope (deferred)

| item | reason |
|---|---|
| Apple FM (Censor on-device LLM) | Aion 1.0 / Phi Silica / Apple Foundation Model → needs abstraction plan |
| `NetPolicy::Loopback` and `Enabled` on Windows | WFP filter install + teardown; deferred past v1 |
| Aion 1.0 Windows AI Foundry integration | No Rust SDK ships today |
| MSIX packaging | Deferred — needs `bundle.windows.msix` block + sign tooling |
| ARM64 Windows support | `wry#1665` WebView2 deadlock + `tauri#13084` ARM64 issues; not in v1 |
| `keyring 4.x` ecosystem migration | Current `keyring 3.6 + ["windows-native", "apple-native"]` is correct |
| Elevation-required features | `tauri#13926` WebView2 fails under elevation; devboule must run unprivileged |
| `kt` / Python `.py` snippet that loads `candle-metal` for macOS | Unchanged — Apple FM defer |
| `cargo metadata --manifest-path` for the wix bundle | Post-MH; not v1 |

---

## 7. Verification discipline (re-stated for every milestone)

Before each milestone lands in code, **one fresh websearch** confirms the load-bearing claim (M0, A, C1, C2, C3, C4, ort unify, final gate). Anything unverifiable becomes `TODO(verify)` with a URL.

The reviewer loop gates every milestone:

```
worker writes → reviewer (DeepSeek V4 Pro) reviews → oracle (GLM-5.2 max) reviews for sensitive areas → I (planner, MiniMax-M3) read final diff.
```

For backend-Rust/security milestones, **GLM-5.2 max sign-off is required** before merge.

---

## 8. Honest reset of where we started vs where we ended

- **Original plan**: 8 milestones, mostly duplicated shipped work, with a `windows = "0.62"` proposal that would have broken devboule's existing Windows auth + GPU paths.
- **Amendment 1**: Milestone C restructured into 4 sub-stories (sand-win reality for `is_enforced() -> true`).
- **Amendment 2**: trimmed dead/redundant milestones; extended existing `windows = "0.58"` block; explicit already-shipped map.
- **Final plan**: single coherent document, one blocker for `ort` snippet (`api-*` feature), env-var fix documented, every Milestone's commands and acceptance ready.

**Time-to-execute for a fresh worker pass**: M0 (½ day) → A (½ day) → H (½ day) → C1 (1 day) → C2 (1-3 days depending on broker shim) → C3 (1-2 days) → C4 (1-2 days, possibly deferred to follow-on) → ort unify (½ day) → flip gate (½ day). Realistic: **2 weeks with full reviewer + oracle loop**, 4 weeks if any C-milestone hits a Windows-API surprise.

---

## 9. Permanent config artifacts to remember

| file | why |
|---|---|
| `~/.pi/web-search.json` | hard-pinned to Exa, literal API key (109 bytes plaintext) |
| `~/.pi/agent/agents/oracle.md`, `advisor.md`, `researcher.md` | ejected; `inheritSkills: true` + `defaultContext: fresh` + `skills: delegate-task, request-review, diagnose-stall` |
| `~/.pi/agent/settings.json` | `subagents.agentOverrides` for 7 agents (deepseek-v4-flash/pro, glm-5.2, qwen3.7-plus, hy3) |
| `C:\Users\gualt\AppData\Roaming\npm\pi.ps1` | fixed bug; `setx EXA_API_KEY` env-var promotion restored |
| `devboule/specs/PORT_MACOS_TO_WINDOWS_FINAL.md` | this document |

If npm reinstall of `pi-coding-agent` runs again, **line 6 of `pi.ps1` will be re-broken** — re-apply the `pi.ps1` fix. **Agent frontmatter edits survive** because they're user-scope (`.pi/agent/agents/*`).

---

## 10.5 Implementation status (2026-07-31, post-execution pass)

> **UPDATE (2026-07-31, C5 landed): the elevation blocker below is RESOLVED** —
> the sandbox now uses AppContainers (§10.6), verified by the broker
> integration test on a non-elevated host. **FINAL UPDATE (C6 landed): the
> flip is DONE** — `is_enforced()` is `true` on Windows; the interactive-agent
> PTY and one-shot mini paths are sandboxed via the ConPTY broker. The
> historical note below (flip still false, hostile review on 479e355) is
> preserved as the C5-era status; Unattended is now honoured app-hosted and
> rejected on the external conhost path.


| item | status | evidence |
|---|---|---|
| M0 — windows=0.58 features | ✅ shipped | commit `c1144fd` |
| A — `bundle.windows` block | ✅ shipped | `tauri.conf.json` (wix/nsis perMachine, downloadBootstrapper) |
| H — 3-OS CI matrix | ✅ shipped | `.github/workflows/ci.yml` |
| C1 — Job Object | ✅ shipped | `sandbox/windows.rs` `create_job_object` (KILL_ON_JOB_CLOSE, PROCESS_MEMORY, ACTIVE_PROCESS, PROCESS_TIME) |
| C2 — Restricted Token + broker spawn | ✅ shipped, **SUPERSEDED by C5** | commit `840d142` (historical restricted-token path) replaced by per-spawn AppContainer profiles (C5, §10.6): `create_appcontainer_profile` + SECURITY_CAPABILITIES via PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, CreateProcessAsUserW, CREATE_SUSPENDED, proc-thread-attr list, ResumeThread |
| C3 — filesystem ACL layer | ✅ shipped | commit `840d142` — restricted-SID DACL grants via SetNamedSecurityInfoW, SD save/restore, .git/.devboule deny-write, rollback guards |
| C4 — network egress layer | ✅ shipped | commit `840d142` (netsh layer, historical) → C5 rewrite: per-token capability SIDs; Loopback added round-30 via NetworkIsolationSetAppContainerConfig (e2e-tested) |
| ort unify rc.12 + api-24 | ✅ shipped | `oracle-core/Cargo.toml:50,57,61`; vendored esaxx-rs `/MD` CRT |
| G — memory backpressure | ⏸ deferred | per plan (optional) |
| **Flip `is_enforced()` → true** | ✅ **DONE (C6)** | `mod.rs` = `true`; PTY + one-shot paths sandboxed via ConPTY broker; `is_enforced_true_on_windows` |

### Why the flip was BLOCKED at C5 (historical; resolved by C6)

All 18 sandbox tests pass on a real Windows host (`cargo test --lib backend::sandbox`),
but the suite exposed a hard conflict:

- The broker's C2/C3 path **must** grant the restricted SID (S-1-5-12) read/execute on
  `C:\Windows` and `C:\Windows\System32` — system DLLs are otherwise unreachable for a
  restricted token (system roots carry no S-1-5-12 ACE). `SetNamedSecurityInfoW` on those
  roots requires **SeRestorePrivilege / elevation**.
- devboule must run **unprivileged** (tauri#13926 — WebView2 fails under elevation; §6).

So on a normal (non-elevated) dev machine every broker spawn fails with
`WIN32_ERROR(5)` on the system-root ACE grant. `is_enforced() -> true` would claim OS
confinement that the shipped broker cannot deliver in the supported run mode.

**Options (out of this plan's scope, pick one for a follow-on plan):**
1. Elevate a thin broker *service* once (install-time), children spawned via the service
   token; app stays unprivileged. Matches srt-win's sandbox-user pattern.
2. Replace the restricted-token path with an AppContainer (LPAC) whose system access is
   granted by package SID ACLs instead of touching `C:\Windows` — plan §C4 pattern 2,
   previously deferred.
3. Accept admin-required sandboxing (documented UX wall; contradicts §6).

**RESOLVED 2026-07-31 (C6)**: option 2 landed — the per-spawn AppContainer
broker (§10.6). `is_enforced()` is `true` on Windows; Unattended is honoured
for app-hosted (broker-gated) launches and rejected on the legacy external
conhost path (projects.rs `unattended_external_is_rejected`).

---

## 10.6 RESOLUTION — AppContainer (LPAC-style) replaces the S-1-5-12 restricted token

**Decision (2026-07-31, follow-on pass):** option 2 from §10.5 — replace the
`CreateRestrictedToken` S-1-5-12 path with a **per-spawn AppContainer profile**.
This removes the elevation requirement entirely and strengthens the sandbox.

**IMPLEMENTED and VERIFIED 2026-07-31.** The broker now: creates a per-spawn
profile via `CreateAppContainerProfile` (pid+seq moniker), passes
`SECURITY_CAPABILITIES` (package SID + `internetClient` capability when
`NetPolicy::Enabled`) via `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` to
`CreateProcessAsUserW`, grants the package SID read/exec on
readonly_root/cwd/exe-parent and modify on writable_paths, reroutes
LOCALAPPDATA/TEMP/TMP to the profile AC folder (required — spawn fails with
0x800700CB otherwise), deletes the profile in `wait_and_restore`. Verified
end-to-end on a non-elevated host: `TokenIsAppContainer=1`, stdin pipe works,
exit 0, host ACLs restored. Three API pitfalls found and fixed on the way:
(i) `CreateRestrictedToken` with a package SID fails ERROR_INVALID_PARAMETER —
the MS pattern is SECURITY_CAPABILITIES, not a restricted token; (ii) a bare
derived SID without a registered profile fails CreateProcess* with
ERROR_FILE_NOT_FOUND; (iii) JOB_OBJECT_LIMIT_PROCESS_TIME pairs with
PerProcessUserTimeLimit (per-job is JOB_OBJECT_LIMIT_JOB_TIME + PerJobUserTimeLimit).

### Why it works unprivileged

- System roots (`C:\Windows`, `System32`) already carry `ALL APPLICATION PACKAGES:(RX)`
  ACEs on a stock Windows install — an AppContainer token reads system DLLs **without
  any ACL modification**. The broker's system-root grant (the elevation blocker) is
  deleted, not made conditional.
- Network is deny-by-default for AppContainers: no `internetClient` capability → no
  outbound sockets (kernel-enforced via WFP ALE, see Project Zero's analysis). The
  `netsh advfirewall` rule (admin-required) is deleted; `NetPolicy::None` = no
  capability SIDs, `NetPolicy::Enabled` = `internetClient` capability
  (`DeriveCapabilitySidsFromName`). **Loopback IMPLEMENTED (round 30)**:
  `NetPolicy::Loopback` now calls `NetworkIsolationSetAppContainerConfig`
  (resolved dynamically from `firewallapi.dll` — no import-lib needed,
  verified HRESULT 0x0 from a NON-elevated process) with the per-spawn
  package SID right after profile creation. This unblocks the pi sidecar's
  local Ollama/oMLX sessions. e2e-tested:
  `loopback_policy_allows_localhost_connection` (sandboxed PowerShell
  Test-NetConnection reaches a host-side 127.0.0.1 listener, exit 0).
  Fail-closed: an exemption error aborts the spawn.
- Package SID comes from a **per-spawn registered profile**
  (`CreateAppContainerProfile` with a pid+seq moniker — no admin needed, lands
  under `%LOCALAPPDATA%\Packages`). NOTE: a bare derived SID
  (`DeriveAppContainerSidFromAppContainerName` alone, Chromium's pattern) makes
  CreateProcess* fail with ERROR_FILE_NOT_FOUND — the registered profile is
  REQUIRED (verified 2026-07-31; spec corrected post-oracle).
- File ACL layer (C3) keeps its exact snapshot/restore machinery but targets the
  **package SID** instead of S-1-5-12. Grants are now required for EVERY path the
  child needs (deny-by-default), which is stricter than the S-1-5-12 double-check.

### Concrete changes (all in `src-tauri`, milestone "C5")

1. `Cargo.toml`: add `"Win32_Security_Isolation"` to the `windows 0.58` features.
2. `sandbox/windows.rs`:
   - `create_restricted_token` → `create_appcontainer_profile()` +
     `build_capability_sids(policy)`: registered per-spawn profile
     (`devboule.sandbox.<pid>.<seq>`), `SECURITY_CAPABILITIES` (package SID +
     `internetClient` capability with `SE_GROUP_ENABLED` iff `NetPolicy::Enabled`)
     passed via `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` to
     `CreateProcessAsUserW` — NOT `CreateRestrictedToken`, which rejects a
     package SID with ERROR_INVALID_PARAMETER (verified 2026-07-31).
   - `apply_restricted_sid_policy` → package-SID grants; **delete the SystemRoot/
     System32 grant block**; add the user home as a READ-ONLY root (npm/git/python
     read `~/.npmrc`, `~/.gitconfig`, `~/.config`; matches macOS seatbelt broad reads).
   - `apply_net_policy`/`restore_net_policy`: delete netsh rule + journal + orphan
     cleanup (no longer needed; capability SIDs are per-token and die with the token).
   - Broker spawn: unchanged flow (CREATE_SUSPENDED → AssignProcessToJobObject →
     ResumeThread), token is the AppContainer token.
3. Tests: the broker integration test can now run **non-elevated** — remove the
   `process_is_elevated()` skip and assert the child really is in an AppContainer
   (check `TokenIsAppContainer` on the child token, or verify a blocked-outbound
   socket). icacls roundtrip tests keep their skip (icacls /restore still needs
   SeRestorePrivilege — they cover the legacy path, not the broker).
4. Final gate: flip `is_enforced()` → `true` on Windows AFTER reviewer + oracle
   sign-off on the C5 diff.

### C6 (DONE 2026-07-31) — sandbox the PTY paths

Hostile review on `479e355` (blocker) is resolved: `agent_pty.rs` now routes
the interactive agent terminal AND the one-shot mini path through the broker
on Windows. Implementation:

- `PtyCommand { program, args, cwd, env }` replaces the opaque
  portable_pty::CommandBuilder in `spawn_agent_pty` (builder is converted via
  `from_command_builder`, which reads get_argv/get_cwd/iter_extra_env_as_str).
- `create_conpty()` (sandbox/windows.rs) creates the pseudoconsole + the two
  host pipes; `spawn_sandboxed_pty` passes HPCON via
  `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` (0x20016) alongside SECURITY_CAPABILITIES.
- `SandboxedChild` implements portable_pty `Child`/`ChildKiller`;
  `WindowsConPtyMaster` implements `MasterPty` (resize via ResizePseudoConsole,
  duplicate_reader/duplicate_writer for the app's reader/writer threads).
- `is_enforced()` → `true` on Windows: every unattended spawn path (agentic,
  sidecar, cloud duplex, censor, PTY, one-shot mini) now runs in an
  AppContainer with Job Object + ACL grants.

Verified on a non-elevated host: `real_pty_echo_is_captured_and_child_reaped`
(PTY echo roundtrip inside the AppContainer, TokenIsAppContainer=1) and
`fast_exit_child_does_not_orphan_session_in_map` both pass.

**C6 review fixes (2026-07-31, hostile review on 49146ae):**
- Blocker fixed: the sandboxed child could NOT read the per-launch prompt file
  or session gitconfig (user-only %TEMP% dirs, AppContainer deny-by-default) —
  real agent launches would have failed inside the sandbox. `SandboxPolicy`
  gained `readonly_paths` (builder `.readonly()`), `PtyCommand` gained
  `extra_read_roots`, and agent_spawn/mini_command_build grant the prompt dir
  + gitconfig dir as read roots. The PTY integration test now spawns a child
  that `type`s the prompt file from the granted root and asserts the marker
  arrives (end-to-end, not just an echo).
- Documented, not changed: the external conhost terminal (host="external") is
  the legacy ATTENDED path (full-token, window title = kill handle), parity
  with macOS Terminal.app; host="app" (default) is the sandboxed PTY. The
  "every unattended path" claim in 49146ae refers to app-hosted paths.
- Known limitation: `writable(cwd)` ACL propagation is O(files in cwd) — a
  node_modules-heavy repo can take seconds at spawn+restore. Accepted for v1;
  a file-count guard is a follow-up.
- **Centralized read roots (review round 3)**: `agent_sandbox_read_roots()`
  (agent_spawn.rs) computes prompt dir + session gitconfig dir + real
  gitconfig file(s) and is applied by ALL sandboxed agent builders — app-hosted
  PTY, one-shot mini, cloud duplex. The real gitconfig grant is level-1 only
  (includeIf targets not covered) and is a documented widening (users may store
  PATs in http.extraHeader; copying identity keys only is a follow-up).
- **Consent hook (review rounds 5-6)**: the Claude PreToolUse hook binary is
  granted read+exec and the ledger dir + ledger FILE (exists pre-spawn, so
  OI/CI inheritance alone would not cover it) are writable. Known limits,
  documented as accountability-grade: (a) the agent itself shares the sandbox
  with the hook and knows ASPIS_CONSENT_BRIDGE, so the ledger is forgeable by
  the gated entity — consent is accountability, not a security boundary;
  (b) if the hook ships in Program Files (perMachine install), the package-SID
  ACE fails (user not owner) and the spawn is refused — run the hook from a
  user-writable path (dev layout: target/debug) until a degradation path lands;
  (c) the --settings scan grants only %TEMP%-prefixed paths (no arbitrary
  self-grants);
  (d) CROSS-AGENT ledger write (review rounds 7-9): writable grants include
  DELETE/FILE_DELETE_CHILD, so an agent CAN rewrite the shared
  .aspis-agents.json in place — forge verdicts for OTHER agents' pending
  requests or DoS the queue for the session duration (ACLs restore at child
  exit). This is a real widening beyond self-forgery; accepted for v1 with the
  same accountability-grade caveat — the consent bridge is a UX gate, not a
  security boundary; a per-agent ledger or signed-verdict follow-up is
  required before Unattended is trusted with multiple concurrent agents.
- **REPLACE-LEG FAILURE (review rounds 8-11, e2e-proven)**: MoveFileExW
  REPLACE_EXISTING fails ACCESS_DENIED inside the AppContainer even with
  DELETE + FILE_DELETE_CHILD granted — verified three ways: cmd `move /y`,
  PowerShell `Move-Item -Force` and `[IO.File]::Replace` (the sandbox
  double-check rejects the atomic-replace access path; the ACE grant alone
  cannot fix it). `fs_replace::replace_existing` tries the atomic move first
  and falls back to copy+delete, but ONLY (a) for ERROR_ACCESS_DENIED
  (FACILITY_WIN32 validated), (b) through the explicit
  `replace_file_with_backup_with_fallback` capability AND (c) when the
  calling process is ITSELF inside an AppContainer (TokenIsAppContainer
  checked — round-12 hostile review: a bare boolean is not enough, the
  execution context is load-bearing). Host-side shared callers
  (design/projects/config/oracle saves, and today ALL production call sites
  of fs_replace — agent ledger/state, censor shard are host-process writers)
  keep strictly atomic semantics: 0x80070005 from a plain host (broken ACLs)
  is indistinguishable from the AppContainer double-check, so without BOTH
  gates the original error is returned. The fallback is wired into the ONE
  genuinely-sandboxed production writer: `write_agent_live_state`
  (agents.rs:1009) — it is also the consent-hook BINARY's writer
  (`mutate_agent_live_state_at_path`, called from the standalone
  `src/bin/claude_consent_hook.rs` that runs INSIDE the cloud-duplex
  AppContainer; there MoveFileExW REPLACE_EXISTING fails in the double-check,
  so without the fallback the hook's ledger update fails closed and Unattended
  cloud cannot function — round-13 review). The TokenIsAppContainer gate keeps
  the host-side launch-time callers strictly atomic. Unit tests simulate the
  AppContainer context to exercise the fallback. The SAME gap existed in the
  MCP servers that run INSIDE the AppContainer (mcpServers entries of the
  sandboxed codex/claude child): `devboule-mcp` (Rust, default backend) and
  `oracle/server/aspis_mcp.py` (Python, packaged fallback) both wrote
  `.aspis-agents.json`/project files with a strict atomic replace
  (MoveFileExW / os.replace) that fails ACCESS_DENIED in the double-check —
  registration/heartbeat/claim writes would fail closed and Unattended cloud
  could not function (round-14 review). Both now fall back to copy+delete on
  ERROR_ACCESS_DENIED (winerror 5) only; other errors keep original
  semantics. Round 15 hardened the MCP rollback the same way as fs_replace:
  had_backup is tracked BEFORE the replace, a failed fallback copy (target
  truncated but still present) restores the backup UNCONDITIONALLY (keeping
  the .bak if the restore itself fails), and first-write failures remove the
  partially-created target. Round 16 closed the last rollback hole: the BACKUP
  copy itself can fail mid-way (partial .bak) — `backup_created` is set only
  after a SUCCESSFUL copy, a partial .bak is removed (never restored over the
  target then deleted), and the no-valid-backup case is reported explicitly
  (`backup_copy_failure_reports_no_valid_backup` test); restore failure keeps
  the .bak and names it in the error (`restore_failure_keeps_backup` test).
  The Python backend mirrors the same logic and reads the winerror via
  getattr (Windows-only attribute, round-16 review). Round 17-18: restore
  failure in Python now RAISES with the retained .bak path (no silent
  swallow); the MCP test seams compile under cfg(all(test, windows)) so
  `cargo test` works on Linux/macOS; new Python parity tests
  (oracle/server/tests/test_write_crash_safe_fallback.py — 4 tests, mocks
  os.replace winerror=5) cover fallback success, unconditional restore,
  first-write cleanup and .bak retention on restore failure. New Windows
  tests use fault seams MOVE_FAULT / COPY_FALLBACK_FAULT / BACKUP_COPY_FAULT
  / RESTORE_FAULT; all discriminating. Every seam reference in fs_replace.rs
  and devboule-mcp carries the SAME guard as its definition
  (cfg(all(test, target_os = "windows"))) — uniform by audit (round 21). The
  polis meta_store / augure ledger writers use strict rename but are HOST-side
  only (never inside an AppContainer child), so they stay atomic and are
  outside the sandbox-writer scope. Round 26: the MCP fallbacks are now
  context-gated like fs_replace — devboule-mcp checks TokenIsAppContainer via
  raw FFI (OpenProcessToken/GetTokenInformation class 29) and aspis_mcp.py
  via ctypes, so a HOST-side MCP server hitting an ordinary ACL ACCESS_DENIED
  keeps strictly atomic semantics (new host-denial tests in both backends;
  sandboxed-writer tests simulate the AppContainer context).
- **UNATTENDED MUST BE APP-HOSTED (review round 12, enforced in code)**: with
  `is_enforced()=true`, Unattended autonomy is unlocked — but the legacy
  EXTERNAL conhost path launches raw `conhost.exe` OUTSIDE the broker (no Job
  Object, no package-SID ACLs, no net deny). `prepare_or_launch_project_agent`
  now rejects external+Unattended on Windows fail-closed
  (`unattended_external_is_rejected`, unit-tested); the in-app PTY path
  (host="app", fully broker-gated) is the only Unattended carrier. Ask and
  AutoAccept remain supervised by the broker consent flow and are unaffected.
  On macOS/Linux the gate compiles out (cfg! constant) — zero behaviour
  change.
- **MACOS SCOPE NOTE (round-13 hostile review, deferred by DESIGN)**: the
  reviewer correctly notes that macOS also reports `is_enforced()==true`
  while BOTH its carriers are unconfined: the external path spawns
  Terminal.app and the app-hosted PTY spawns `portable_pty` directly (no
  seatbelt wrap — verified in `agent_pty.rs::to_command_builder`). Fixing
  that (gating Unattended on macOS too, or flipping macOS `is_enforced()` to
  false) would REGRESS the historical macOS behaviour. The port's hard
  invariant — "no macOS file/block/test may be removed or regressed" — is an
  explicit user constraint, so this is left as a DOCUMENTED, PRE-EXISTING
  gap with a follow-up: macOS needs either seatbelt-wrapped PTY/external
  spawns or a platform-specific Unattended gate before its invariant is
  sound. The Windows gate introduced here is strictly tighter than the
  macOS status quo. Rollback
  semantics: on copy failure the backup is restored UNCONDITIONALLY (even if
  the target still exists — round-8 bug), the .bak is KEPT if restoration
  itself fails, a committed target with a leaked temp is preserved and only
  the leak reported (round-10 bug), and first-write failures clean up
  partially-created targets. Round 9-11 regression tests (fault-injection
  seams: arm_move_fault/arm_copy_fault/arm_restore_fault) cover all paths on
  the non-elevated host.

API pitfalls found and fixed (documented for the next engineer):
- `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` takes the **HPCON value itself** as
  lpValue (like a scalar), NOT a pointer to it — unlike SECURITY_CAPABILITIES.
- `CREATE_NO_WINDOW` **breaks ConPTY output** (child never renders into the
  pseudoconsole; master reads nothing). Removed in ConPty mode only.
- `bInheritHandles` must be FALSE in ConPty mode (no HANDLE_LIST attribute).

### Known trade-offs (accepted for v1)

- AppContainer children CANNOT reach any user path without an explicit ACE.
  v1 grants only `readonly_root` + `cwd` + exe parent + `writable_paths` — the
  user home (~/.ssh, ~/.npmrc, ~/.gitconfig) is NOT granted, so the child is
  **more locked down than macOS seatbelt** (broad reads). Tools that need user
  config must have those paths added per-project. (A home-wide read grant was
  tried and dropped: setting an inheritable ACE on the user Known Folder
  triggers a long Windows propagation pass over every file — multi-minute
  hang.)
- Some tools that write to `AppData` (npm cache) need their cache dir in
  `writable_paths`; pi-sidecar already passes a writable home for now (reviewer
  CONCERN on 840d142 — tracked, same policy on macOS).
- Loopback-only policy remains unsupported on Windows (v1 None/Enabled only).

---

## 10. Audit trail (pointers)

- `PORT_MACOS_TO_WINDOWS.md` — original plan (kept for diff visibility)
- `PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md` — first amendment (Milestone C reshape, ort coexistence, keyring resolution, bundle.windows correction)
- `PORT_MACOS_TO_WINDOWS_AMENDMENT_2.md` — second amendment (Milestones B/D/E/F2 trim, `windows = "0.62"` → extend `0.58`, worklist)
- This file — final consolidated plan, includes everything from all three

**Hostile-review runs** (oracles):
- `1af3d46d` — first attempt, stalled at 98k tokens due to fork-context inheritance, stopped
- `dbbb5f86` — second attempt, stalled again, stopped
- `557534c2` — third attempt, fresh context, returned with 5-blocker + already-shipped map
- `779c81b5` — final review of Amendment 2 + this final plan; returned with one new blocker (`api-*` feature), now patched in §3

**Investigation runs**:
- `4a0acb47` (delegate, HY3) — env-var propagation root cause identified; led to literal-key fix in `~/.pi/web-search.json`

**Ejected agent edits** (`.pi/agent/agents/oracle.md`, `advisor.md`, `researcher.md`):
- `tools:` line — added `web_search, fetch_content`
- `inheritSkills: false` → `true`
- `defaultContext: fork` → `fresh`
- Added `skills: delegate-task, request-review, diagnose-stall` (advisor and oracle); researcher includes `research-first` too

Standing by. Next step: cut **M0** when you give the green light.
