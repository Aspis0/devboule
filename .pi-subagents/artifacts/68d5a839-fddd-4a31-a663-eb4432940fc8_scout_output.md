# Censor & Sandbox Subsystems — Deep Reconnaissance

---

## 1. CENSOR SUBSYSTEM

### 1.1 File Tree (2 levels)

```
src-tauri/src/backend/censor/
├── mod.rs               — module root, exports, pub const CENSOR_DIR, now_stamp()
├── catalog.rs           — curated Ollama model catalog for Censor local-AI tier
├── commands.rs          — Tauri command surface, CensorState, start_watch, review_now, dispose
├── detect.rs            — project kind detection (Cargo.toml→Rust, etc.), FileLang enum
├── extract.rs           — tree-sitter per-item extraction + finding GROUNDING (anti-hallucination)
├── gemma.rs             — local-AI tier (Ollama/oMLX client, prompt building, parse)
├── ledger.rs            — on-disk shard IO, lock, supersede merge, dispose_finding
├── live_e2e.rs          — end-to-end integration test (133 lines)
├── orchestrator.rs      — engine STEP: plan_fine, coarse_runners, run_fine_batch, run_coarse_pass, run_review_now
├── schema.rs            — serde types: Finding, CensorShard, FindingBatch, Severity, Category, etc.
├── severity.rs          — pure normalizers mapping tool-native severity → (Severity, Category)
├── votes.rs             — k-sample self-consistency voting for LLM findings
└── runners/
    ├── mod.rs           — RunnerId enum (33 variants), RawFinding, RunnerOutcome, build_command,
    │                      run_capture, applicable_runners, redact_secrets, cap
    ├── actionlint.rs, bandit.rs, cargo_audit.rs, cargo_check.rs, cargo_deny.rs,
    │   cargo_fmt.rs, clippy.rs, cppcheck.rs, eslint.rs, gitleaks.rs, go_vet.rs,
    │   gofmt.rs, hadolint.rs, jscpd.rs, knip.rs, ktlint.rs, lizard.rs, npm_audit.rs,
    │   oxlint.rs, pip_audit.rs, prettier.rs, pyright.rs, ruff.rs, ruff_format.rs,
    │   semgrep.rs, shellcheck.rs, sqlfluff.rs, stylelint.rs, tidy.rs, tsc.rs,
    │   vulture.rs, yamllint.rs, zizmor.rs
    └── (each has: pure `parse_<tool>` + thin `run(root, target)`)
```

### 1.2 Public Entry Points

#### Tauri Commands (`commands.rs`)
| Function | Line | Purpose |
|---|---|---|
| `censor_review_now(project_id, file?)` | 260 | On-demand review (single file or whole-project). Spawns detached thread. |
| `censor_dispose_finding(project_id, file, id, disposition)` | 599 | Set a finding's disposition (fp/wontfix/fixed/reopen). |
| `censor_get_findings(project_id)` | — | Read all shards, return findings. |
| `censor_count_open(project_id)` | — | Count open findings (lock-free, used by UI chip). |
| `censor_start_watch(project_id)` | ~163 | Start per-project worker (fine/coarse debounce). |
| `censor_stop_watch(project_id)` | — | Stop per-project worker. |

#### Orchestrator (`orchestrator.rs`)
| Function | Line | Purpose |
|---|---|---|
| `plan_fine(kinds, files) -> Vec<FilePlan>` | 220 | Pure: decide which FINE runners apply per file. |
| `coarse_runners(kinds) -> Vec<RunnerId>` | 253 | Pure: union of COARSE runners for project kinds. |
| `group_by_file(raw) -> BTreeMap<String, Vec<RawFinding>>` | 280 | Bucket flat findings by file path. |
| `run_fine_batch(app, project_id, root, files, gemma?, running)` | 621 | IO: run FINE pass, write shards, emit event. |
| `run_fine_batch_no_rail(app, ...)` | 639 | Like above but skips training-rail write (verdict gate). |
| `run_coarse_pass(app, project_id, root, running)` | 883 | IO: run COARSE pass, scoped merge, emit event. |
| `run_review_now(app, project_id, root, file?, gemma?, running)` | 993 | On-demand: file→FINE, None→COARSE + FINE refresh. |

#### Ledger IO (`ledger.rs`)
| Function | Line | Purpose |
|---|---|---|
| `censor_dir(root) -> PathBuf` | 109 | `.aspis-censor/` path. |
| `validate_rel_path(rel) -> io::Result<()>` | 140 | Reject `..`, absolute, `-`-leading components. |
| `shard_path(root, file_rel_path) -> PathBuf` | 222 | `<sha256(fileRelPath)>.json` |
| `read_shard(root, file_rel_path) -> Option<CensorShard>` | 302 | Read-only (no lock). |
| `list_shards(root) -> Vec<CensorShard>` | 313 | Enumerate all shards in `.aspis-censor/`. |
| `write_shard(root, shard)` | 364 | Atomic write under lock. |
| `read_supersede_write_shard(root, new, hash, sources, rel, now) -> CensorShard` | 386 | TOCTOU-free read-modify-write with source-scoped merge. |
| `dispose_finding(root, file, id, disposition, actor, now)` | 426 | Set finding disposition + append provenance (under lock). |
| `supersede(old, new, hash, rel, now) -> CensorShard` | 507 | Whole-file merge primitive. |
| `supersede_sources(old, new, hash, sources, rel, now) -> CensorShard` | 537 | Source-scoped merge (preserves non-refreshed sources). |

#### Schema Types (`schema.rs`)
| Type | Purpose |
|---|---|
| `Severity { High, Medium, Low }` | Finding severity (with `rank()`). `Default = Medium`. |
| `Category { Security, Correctness, Complexity, Duplication, DeadCode, Style }` | Finding category. `Default = Correctness`. |
| `Verdict { Suspected, Confirmed }` | Confidence. `Default = Suspected`. |
| `Disposition { Open, Fixed, Fp, Wontfix }` | Lifecycle. `Default = Open`. |
| `ProvenanceEntry { actor, action, role, at }` | Audit trail entry. |
| `Finding { id, file, content_hash, line, severity, category, source, title, body, verdict, disposition, provenance, created_at, commit }` | One finding. All fields `#[serde(default)]` for forward compat. |
| `CensorShard { file_rel_path, content_hash, updated_at, findings }` | Per-file shard on disk. |
| `FindingBatch { batch_id, timestamp, pass_type, files, findings }` | Queue entry. |
| `Finding::compute_id(file, line, category, source, title) -> String` | Stable sha256 hex id. |

#### Runners (`runners/mod.rs`)
| Type/Function | Line | Purpose |
|---|---|---|
| `RunnerId` enum (33 variants) | 192 | Stable runner identifier. |
| `RunnerId::ALL` | 206 | Exhaustive compile-time-pinned list of all 33 runners. |
| `RawFinding { file, line, severity, category, source, title, body }` | 90 | Lightweight tool finding before persistence. |
| `RunnerOutcome { Ok(Vec<RawFinding>), Skipped, Failed }` | 147 | Dispatch result (fail-closed: only `Ok` refreshes source). |
| `Granularity { Fine, Coarse }` | 175 | Per-file vs project-wide trigger. |
| `RunTarget { file_rel_path }` | 282 | Fine target for a runner. |
| `applicable_runners(kinds, lang) -> Vec<RunnerId>` | 345 | Deterministic runner selection. |
| `build_command(program) -> Command` | 428 | Spawn helper with augmented PATH + CREATE_NO_WINDOW. |
| `run_capture(program, args, root) -> Option<String>` | 531 | Run piped command with default 120s timeout. |
| `run_capture_with_timeout(program, args, root, timeout) -> Option<String>` | 573 | Full spawn with capped stdout (16 MiB), kill on timeout/overrun, privacy. |
| `run_capture_stderr_with_timeout(...)` | 614 | Like above but captures stderr (for cargo-deny). |
| `redact_secrets(s) -> String` | 464 | Redact secret-looking tokens from finding body (AWS keys, base64 blobs). |
| `cap(s, max) -> String` | 450 | Truncate title/body with ellipsis. |
| `CROSS_CUTTING` const (5 runners) | 294 | Gitleaks, Jscpd, Lizard, Semgrep, Zizmor (applied to every file). |

#### Key Constants
| Constant | Value | Location |
|---|---|---|
| `CENSOR_DIR` | `".aspis-censor"` | `mod.rs:25` |
| `FINE_DEBOUNCE_MS` | `400` | `orchestrator.rs:126` |
| `COARSE_DEBOUNCE_MS` | `4000` | `orchestrator.rs:128` |
| `MAX_HASH_BYTES` | `8 * 1024 * 1024` | `orchestrator.rs:382` |
| `HASH_BUF_BYTES` | `64 * 1024` | `orchestrator.rs:385` |
| `CENSOR_PROVENANCE_MAX` | `50` | `ledger.rs:35` |
| `LOCK_ATTEMPTS` | `100` | `ledger.rs:30` |
| `LOCK_SPIN_INTERVAL` | `50ms` | `ledger.rs:31` |
| `DEFAULT_RUNNER_TIMEOUT` | `120s` | `runners/mod.rs:445` |
| `MAX_STDOUT_BYTES` | `16 MiB` | `runners/mod.rs:449` |
| `GEMMA_GENERATE_TIMEOUT` | `60s` | `gemma.rs:74` |
| `GEMMA_PROBE_TIMEOUT` | `5s` | `gemma.rs:79` |
| `MAX_GEMMA_FINDINGS` | — | `gemma.rs` (cap on LLM findings per file) |

### 1.3 Data Flow

```
mini_coder_executor.rs
  │  triggers FINE on task completion
  │  triggers COARSE on cooldown timer
  ▼
orchestrator::run_fine_batch / run_coarse_pass
  │
  ├── detect::detect_project_kinds(root)      — probe manifest files
  ├── runners::applicable_runners(kinds, lang) — pick runner set
  ├── runners/<tool>::run(root, target)         — spawn tool, parse output
  │   └── runners::run_capture_with_timeout(...)  — bounded spawn
  ├── ledger::read_supersede_write_shard(...)   — TOCTOU-free shard write
  │   └── ledger::supersede_sources(...)         — source-scoped merge
  ├── gemma::run_gemma(...)                     — optional local-AI tier
  │   └── votes::cluster_and_vote(...)           — k-sample voting
  └── emit("censor://findings-updated")         — Tauri event
       │
       ▼
  commands.rs Tauri commands                     — frontend reads shards
  devboule-mcp/src/tools/censor.rs               — Python MCP agent tools
```

**Callers** (non-test):
1. `mini_coder_executor.rs:1130` — `run_coarse_pass` on cooldown timer
2. `mini_coder_executor.rs:2068` — `run_fine_batch_no_rail` on task completion (verdict gate)
3. `censor_review.rs:225` — `run_fine_batch_no_rail` via Pigeon async review queue
4. `commands.rs:260` — `censor_review_now` Tauri command → detached thread → `run_review_now`
5. `commands.rs:599` — `censor_dispose_finding` → `ledger::dispose_finding`
6. `devboule-mcp/src/tools/censor.rs:554` — Python MCP `censor_dispose` → Rust `censor_dispose`
7. `agent_prompt.rs:156,162` — prompt instructions cite `censor_findings`/`censor_dispose`
8. `projects.rs:11616,11657` — coder/verifier persona prompts embed censor_dispose instructions

### 1.4 Configuration / Policy Constants

- **No env-var-driven config** for the core engine. Behavior is hardcoded in constants listed above.
- **CensorLocalAi** config (Gemma model, samples, base URL) is read from `projects.rs` project config and resolved in `commands.rs`/`gemma.rs`. OPT-IN only — no model runs unless user explicitly configures one.
- **Project trust gate**: `project_censor_trusted(app, project_id)` checked at every spawn entry (`orchestrator.rs:232-244`). Untrusted projects cannot run any linters.
- **Gemma availability**: cached per `(provider|base|model)` identity, probed once per session (`commands.rs`).
- **VoteParams**: `n_samples`, `min_votes_block`, `min_votes_verify`, `line_tolerance` — resolved from `CensorLocalAi` config.
- **No role_rules.json analogue**: Censor's command surface is registered in `lib.rs:696-699` and `user_mcp_config.rs:96-97`.

### 1.5 Test Coverage

| File | Test focus | Notable |
|---|---|---|
| `schema.rs` | Serde round-trips, forward-compat, `compute_id` determinism, category ID token match | Excellent coverage of serde contract |
| `severity.rs` | Every normalizer function tested exhaustively | ~400 lines of tests covering every tool's severity mapping |
| `ledger.rs` | shard_path, validate_rel_path, supersede, supersede_sources, lock-write-read round trips, dispose_finding, provenance dedup/cap, corrupt shard handling | ~500 lines, very thorough |
| `runners/mod.rs` | `RunnerId::ALL` exhaustiveness, `applicable_runners` for every lang/kind combo, `build_command`, `runner_outcome`, `redact_secrets` | Cross-cutting + per-language mapping tests |
| `detect.rs` | `detect_project_kinds` for each marker, polyglot, `FileLang::from_path` all extensions, Dockerfile/GHA detection | Comprehensive |
| `orchestrator.rs` | `plan_fine`, `coarse_runners`, `group_by_file`, `fine_batch_collect`, `coarse_pass_collect`, trust gate | Pure functions tested against tempdirs |
| `votes.rs` | Clustering algorithm, Jaccard merge, edge cases | Pure, well-covered |
| `extract.rs` | Tree-sitter parsing for each grammar, item extraction, grounding | Dark (no production callers yet) |
| `gemma.rs` | Prompt building, parse_gemma, parse_censor_v2, client construction, split-brain guard | ~4300 lines, very thorough |
| `live_e2e.rs` | End-to-end: detect → plan → run → merge in tempdir | 133 lines, smoke test |
| `commands.rs` | Status formatting, cache_identity probe dedup | Moderate |

**What's not covered**: No load tests for concurrent shard writers. No adversarial testing of runner output (malformed JSON from tools). No stress tests for large file counts in fine_batch_collect.

### 1.6 Notable Risks / Gaps

1. **Concurrent shard writes**: The per-shard lock serializes writes within one process, but the Python MCP writer contends on the same lock sidecar. Lock spin is up to 5s (100×50ms). No timeout escalation path — a stuck lock blocks the watcher for that file.
2. **Gemma stale findings (BLOCKER 3)**: If Gemma goes offline, the `refreshed_sources` set still includes "gemma" when a client context exists, so findings are cleared. But if the client context itself is `None` (non-config), gemma findings from a prior session survive indefinitely. Addressed by always refreshing when a context exists.
3. **TOCTOU in steer/queue write** (`orchestrator.rs:1303-1305`): Between collecting open findings and writing the steer file, a concurrent Python MCP `censor_dispose` can change dispositions. Documented as accepted best-effort.
4. **Over-size file handling**: `MAX_HASH_BYTES = 8 MiB` — files over this are skipped entirely, not reviewed. A CI-generated bundle could evade review.
5. **Runner timeout default 120s**: Some project-wide tools (semgrep, cargo-audit) may exceed this on large repos. `run_capture_with_timeout` accepts a per-runner override (clippy et al. use default).
6. **No runner retry**: If a runner fails transiently (network timeout for npm_audit, OOM for clippy), the finding source is NOT refreshed — stale open findings survive until the next successful run. This is by design (fail-closed) but could leave stale noise.
7. **Advisory severity caps**: Multiple severity normalizers cap at Medium (not High) pending FP-rate measurement (go_vet, cppcheck, tidy, shellcheck, yamllint, hadolint, actionlint, stylelint, cargo_deny). Until promoted, real High-severity issues from these tools appear as Medium.
8. **No rate limit on censor_review_now**: Any frontend/agent can call `censor_review_now` arbitrarily, spawning threads. The CENSOR_REVIEW_MAX_INFLIGHT cap (4) limits concurrent LLM reviews, but deterministic-only calls have no back-pressure.
9. **extract.rs is dark**: Tree-sitter grounding is fully implemented but has no production caller. If wired later, any bugs in grounding (false drops) would silently suppress real findings.

---

## 2. SANDBOX SUBSYSTEM

### 2.1 File Tree

```
src-tauri/src/backend/sandbox/
├── mod.rs       — NetPolicy, SandboxPolicy, ResourceLimits, SandboxedCommand,
│                  wrap(), apply_rlimits(), is_enforced()
└── seatbelt.rs  — SBPL profile builder (macOS Seatbelt), sbpl_escape, canonical_sandbox_path
```

### 2.2 Public Entry Points

| Function/Type | File:Line | Purpose |
|---|---|---|
| `NetPolicy { None, Loopback, Enabled }` | `mod.rs:10-14` | Network egress policy for child processes. |
| `ResourceLimits { cpu_secs, addr_space_bytes, max_procs }` | `mod.rs:20-24` | OS resource caps. Default: 600s CPU, no ASAN, 256 procs. |
| `SandboxPolicy { readonly_root, writable_paths, net, rlimits }` | `mod.rs:44-49` | Complete policy: what child may read/write/reach. |
| `SandboxPolicy::deny(readonly_root) -> Self` | `mod.rs:63-68` | Default-deny constructor. |
| `SandboxPolicy::writable(self, path) -> Self` | `mod.rs:71-73` | Builder: add writable path. |
| `SandboxPolicy::net(self, net) -> Self` | `mod.rs:76-77` | Builder: set network policy. |
| `SandboxPolicy::rlimits(self, rlimits) -> Self` | `mod.rs:80-81` | Builder: set resource limits. |
| `SandboxedCommand { program, args }` | `mod.rs:89-92` | Rewritten command for sandboxed execution. |
| `wrap(policy, program, args, cwd) -> SandboxedCommand` | `mod.rs:124` | Wrap command in `<sandbox-exec -p profile -- <program> <args>`. On non-macOS: passthrough. |
| `apply_rlimits(cmd, limits)` | `mod.rs:159` (unix) / `mod.rs:196` (non-unix) | Set rlimits via `pre_exec`. No-op on non-unix. |
| `is_enforced() -> bool` | `mod.rs:217` | `true` on macOS, `false` on Windows/Linux (until phase 3). |
| `seatbelt::build_profile(policy) -> String` | `seatbelt.rs:27` | Build SBPL profile string from SandboxPolicy. |
| `seatbelt::sbpl_escape(value) -> String` | `seatbelt.rs:9` | Escape string for SBPL double-quoted literal. |
| `seatbelt::canonical_sandbox_path(path) -> PathBuf` | `seatbelt.rs:19` | Canonicalize path for SBPL subpath rule (falls back lexically). |

### 2.3 Key Data Structures

| Type | Fields | Purpose |
|---|---|---|
| `NetPolicy` | `None | Loopback | Enabled` | Network confinement level |
| `ResourceLimits` | `cpu_secs: u64, addr_space_bytes: Option<u64>, max_procs: u64` | OS resource caps on the child |
| `SandboxPolicy` | `readonly_root, writable_paths, net, rlimits` | Complete permission boundary |
| `SandboxedCommand` | `program: String, args: Vec<String>` | Rewritten argv for sandbox-exec |

### 2.4 Callers (Data Flow)

```
agentic_tools.rs
  ├── agentic_run_policy(...) [line 1126]        → builds SandboxPolicy::deny(root).writable(root)
  ├── agentic_run_policy_with_working_set(...) [line 1138] → builds policy with working set paths
  └── sandbox_exec(...) [line 1016]              → calls sandbox::wrap() + sandbox::apply_rlimits()
       │
pi_sidecar.rs
  ├── pi_sandbox_policy(...) [line 1356]          → builds SandboxPolicy with writable root + tmpdir
  └── pi_sidecar_exec(...) [line 1508]            → calls sandbox::wrap() + sandbox::apply_rlimits()
       │
broker::effective_sandbox_mode(mod.rs line 132)  → gates Unattended mode on sandbox::is_enforced()
       │
mini_coder_executor.rs [lines 1672, 2198]         → calls sandbox::is_enforced() for mode gating
```

**Policy construction pattern**: All callers start from `SandboxPolicy::deny(root)`, then add `.writable(root)` and possibly `.writable(tmpdir)` and `.net(some)`.

### 2.5 Platform Status

| Platform | `wrap()` behavior | `is_enforced()` | Notes |
|---|---|---|---|
| macOS (target) | Real `sandbox-exec` with Seatbelt profile | `true` | Full OS confinement |
| Windows | Passthrough (no-op) | `false` | Phase 3 planned: Restricted Token + WFP + Job Object |
| Linux | Passthrough (no-op) | `false` | Landlock stub; not implemented |
| Other | Passthrough (no-op) | `false` | N/A |

On non-macOS, a one-time warning is emitted: `"[sandbox] wrap: NO OS confinement on this platform — children run UNRESTRICTED"`.

### 2.6 Seatbelt Profile Contents (`build_profile`)

| Section | Rule | Notes |
|---|---|---|
| Default | `(deny default)` | Everything denied by default |
| Reads | `(allow file-read*)` broad, `(allow file-read-metadata)`, `(allow sysctl-read)`, `(allow mach-lookup)` | Broad read — security is on writes+network |
| Writes | `(allow file-write* ...)` with explicit paths: `/dev/null`, `$TMPDIR`, policy `writable_paths` | Default-deny writes |
| .git deny | `(deny file-write* (regex #"/\\.git($|/)"))` | **Security**: prevent RCE via planted git hooks |
| .devboule deny | `(deny file-write* (regex #"/\\.devboule($|/)"))` | **Security**: prevent injection into devboule dirs |
| Exec | `(allow process-exec)`, `(allow process-fork)` | Broad — exec is not the boundary; children inherit write+net confinement |
| Network (None) | `(deny network*)` | Total network isolation |
| Network (Loopback) | `(deny network*)` + `(allow network-outbound (remote tcp "localhost:*") (remote udp "localhost:*"))` | Loopback only |
| Network (Enabled) | No deny — full network access | For roles that need egress |

### 2.7 Configuration / Policy Constants

| Constant | Value | Location |
|---|---|---|
| `ResourceLimits::default().cpu_secs` | `600` | `mod.rs:27` |
| `ResourceLimits::default().addr_space_bytes` | `None` | `mod.rs:28` |
| `ResourceLimits::default().max_procs` | `256` | `mod.rs:29` |

- **No env-var configuration** for sandbox behavior. All policy is constructed programmatically.
- **No per-project sandbox config**: The `SandboxMode` (Ask/AutoAcceptInWorkspace/Unattended) is per-project, but the sandbox *policy* is constructed identically for all projects.
- **NetPolicy is caller-dependent**: agentic_tools.rs receives `net` from the caller (agent loop determines per-task); pi_sidecar.rs uses `NetPolicy::Loopback` for the pi sidecar.

### 2.8 Test Coverage

| File | Test focus | Coverage |
|---|---|---|
| `sandbox/mod.rs` (tests) | `macos_argv_wraps_with_sandbox_exec`, `wrap_is_passthrough_off_macos`, `default_policy_is_deny`, `builder_adds_writable_and_sets_net`, `default_rlimits`, `is_enforced` per platform, `macos_apply_rlimits_sets_cpu_limit` | ~15 tests, moderate |
| `sandbox/seatbelt.rs` (tests) | `build_profile` output for each NetPolicy, writable_paths inclusion, non-absolute skip, `.git`/`.devboule` deny regex presence, `macos_enabled_profile_accepted_by_kernel` (real sandbox-exec acceptance), `.git` hook RCE guard | ~10 tests, good for a profile builder |
| `broker/mod.rs` (tests) | `effective_sandbox_mode` — Unattended honoured when enforced, degrades to Ask when not, supervised modes unchanged | ~5 tests |

**What's not covered**: No integration tests that actually run a process under sandbox and verify confinement (except the macOS kernel acceptance test). No test for the `RLIMIT_NPROC` deliberate omission (documented as intentional). No cross-process sandbox contention tests.

### 2.9 Notable Risks / Gaps

1. **Non-macOS platforms are unconfinable**: On Windows/Linux, `wrap()` is a complete passthrough. The `is_enforced()` gate correctly degrades Unattended to Ask, but a user who manually approves execution on an unconfined platform runs code with NO OS isolation.
2. **RLIMIT_NPROC intentionally omitted** (`mod.rs:172-174`): On macOS, `RLIMIT_NPROC` caps the *entire UID*, not just the child, so setting it would starve the Tauri app. This means a fork-bomb in a sandboxed child can exhaust process slots for the whole app.
3. **TMPDIR canonicalization race** (`seatbelt.rs:47-50`): If `$TMPDIR` canonicalize fails, a warning is emitted but the lexical path is used. If `$TMPDIR` contains a symlink not yet resolved, SBPL subpath won't match the real path and the child's temp writes get silently denied.
4. **Writable path canonicalization fallback** (`seatbelt.rs:19-22`): `canonical_sandbox_path` falls back to the lexical path when it doesn't exist yet (not-yet-created scratch dir). If the path later resolves to a symlink target, writes fail silently.
5. **One-time warning only**: The non-macOS passthrough warning fires once per process lifetime via `Once`. If the platform later adds confinement (Windows phase 3), this could mask missing confinement on first run.
6. **No audit log**: There is no record of which commands ran under which policy, making post-hoc forensics impossible.
7. **No timeout on approval**: The `SandboxMode::AutoAcceptInWorkspace` grant has no expiry — once approved, the workspace root is writable for the session without further prompts.

---

## Acceptance Report