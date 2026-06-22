# Audit & Simplify — Findings (2026-06-22)

Whole-app pass via **8 background plugin agents** (code-simplifier + cross-cutting
security + bug root-cause), covering: backend orchestration, censor, oracle/MCP/projects,
resource-broker/data, devboule-coder crate, the Oracle Python server, a cross-cutting
security/privacy lens, and the frontend. Plus a `cargo clippy` sweep and a diff code-review.

**Nothing risky was changed.** Only safe, test-verified cleanup was applied (§1). Every
bug/risk below is REPORT-ONLY for owner review (§2–§3). Risk tags: `SAFE-MECHANICAL`
(behavior-preserving) / `NEEDS-REVIEW` (judgement/behavioral) / `RISKY`.

---

## STATUS — updated 2026-06-22 (after "attacca tutto")

This file is the original audit dump; current disposition:

**✅ Fixed & verified (committed `0bdf2a1` + the recommended-tier follow-up):**
per-model sampling (`SamplingParams::from_registry`), `normalize_lexical` `..` fail-closed,
`training_export` denylist + atomic blob write, `model_registry` first-run, `multi_mcp`
per-server timeout, and the `#[cfg(windows)]` `process_creation_time` restore — plus the
clippy/frontend cleanup. All tests green.

**❌ Refuted as FALSE POSITIVES (verified, NOT changed):**
- YAML-title "data loss" (§2): `parse_simple_yaml` uses `split_once(':')`, which keeps the
  full value after the first colon — `title: Fix: bug` round-trips intact.
- runner attempt-cap (§ devboule): the cap is checked for every task in the batch, and an
  over-cap task blocks + breaks before dispatch.
- `agent_state` leak to `mini` (§3.1): the `mini` role's `allowedTools` is
  `[agent_register, oracle_context, project_structure]` — it CANNOT call `agent_state`.

**✅ Resolved by design / earlier work:**
- B18 (orphan manual task): R1 already binds the working root; per owner decision the agent
  must NEVER auto-run — the runner correctly ignores manual tasks (they launch on click).
- agent_state mini-scoping: moot (mini can't call the tool).

**🔧 In progress:** size-based **"recommended tier"** — `model_param_billions`/`recommended_tier`
classify a model agentic(>20B)/emitEdits(<20B) from its real param size, surfaced as a UI
*recommendation* (the user's manual tier choice still wins; `should_run_agentic` unchanged).
Rationale: the agentic tier grants the powerful tools (`write_file`/`run`), so the size
distinction should be visible, not a blind manual label.

**⏸️ Deferred (design-heavy/risky — exact fix in §2):**
`base_url` use-time validation, B5 orchestrator prompt, B16 project lifecycle,
secret-redaction in `pairs.jsonl`.

---

## 1. Applied & verified (in the working tree, uncommitted)

| Change | Scope | Verification |
|---|---|---|
| clippy idiomatic auto-fixes (no doc-comment cosmetics, no dead-code) | 22 Rust files (src-tauri + devboule-coder) | `cargo test` **2314 + 334 green**, `cargo check` green; each sensitive file hand-reviewed; 2 code-review finders confirmed all semantics-preserving |
| **Windows-build fix** | `projects.rs` | `clippy --fix` had removed `process_creation_time` (used only in a `#[cfg(windows)]` site) → would break the Windows build. Restored as `#[cfg(windows)] use`. mac `cargo check` green; Windows not cross-compilable here |
| Frontend simplifications (2 passes): comment cleanup, `.then/.catch`→`.finally`, nested ternary→`switch`/extracted fns (also fixes project no-nested-ternary rule), dup-JSX→`??` | 11 TS/React files | `tsc` **0 errors** + `vitest` **1955/1955 green** |

Net code diff ≈ 38 files, small. **Left uncommitted for owner review/commit.**

---

## 2. Bugs / Risks to review (NOT changed)

### 🔴 CRITICAL
- **Agentic `base_url` not re-validated as loopback at use-time** — `mini_coder_executor.rs ~1091` (`should_run_agentic`) / `spawn_agentic_worker`. `sanitize_llm_base_url()` (vault.rs) enforces loopback/HTTPS-allowlist but runs **only at vault-write**, not at use. If `backend.base_url` is ever a remote URL, the full `directive.task` (prompt + pasted file contents) is sent to it via the agentic HTTP worker — no sandbox, no loopback check. **Fix:** call `sanitize_llm_base_url` in `should_run_agentic`/`spawn_agentic_worker` before building the client. `NEEDS-REVIEW`

### 🟠 HIGH
- **Secrets can land in training data** — `training_export.rs ~754`: `directive.task` + `outcome.output` written **verbatim** to `pairs.jsonl`, no secret redaction (the `is_sensitive_blob_name` denylist applies only to file blobs). API keys/tokens pasted into a prompt persist on disk. **Fix:** gitleaks-style scan → redact/skip. `NEEDS-REVIEW`
- **Training blocklist gaps** — `training_export.rs ~421`: `is_sensitive_blob_name` is denylist-based and misses `config.json` (holds the registry API keys!), `*.token`, `auth.json`, `*.secret`, framework secret files. **Fix:** extend denylist + content-based skip. `NEEDS-REVIEW`
- **Per-model sampling is DEAD in production** — `mini_coder_executor.rs:1174`: `spawn_agentic_worker` hardcodes `SamplingParams::tuned()` instead of `SamplingParams::from_registry()`. The per-model temp/top-k/top-p the user sets in Settings (gemma 1.0/64, qwen 0.6/20…) are **ignored on every agentic run**. `from_registry` exists, is unit-tested, never called in prod. **Fix:** resolve registry entry by (backend,model), pass `from_registry`, fallback to `tuned()`. `NEEDS-REVIEW`
- **Runner attempt-cap only checks the first batch task** — `runner.rs:257`: under parallel dispatch (`MAX_PARALLEL_TASKS=2`), a non-first task over `MAX_TASK_ATTEMPTS` bypasses the cap and gets re-dispatched. **Fix:** check all batch members before selecting any. `NEEDS-REVIEW`
- **Unquoted YAML title → silent data loss** — `projects.rs:2316`: `replace_frontmatter` emits `title: {value}` unquoted; `parse_simple_yaml` splits on first `:`, so a title with `: ` is **truncated on every read-write cycle**. **Fix:** quote like `root_path` (`yaml_double_quote_inner`). `RISKY` (data loss) but the fix itself is simple/safe.
- **Agentic `edit_file` lacks the whitespace/fuzzy tiers** — `agentic_tools.rs:344`: uses exact-only `replacen`, while the one-shot path has Exact→Whitespace→Fuzzy. Agentic edits fail on minor whitespace drift → avoidable round failures. **Fix:** share `locate_edit_span`. `NEEDS-REVIEW`
- **`write_resolve` TOCTOU** — `agentic_tools.rs:299` (already commented): a concurrent rename of the verified parent between canonicalize and write can escape the scope. **Fix:** `cap-std`/`openat` + `O_NOFOLLOW`. `NEEDS-REVIEW` (structural)

### 🟡 MEDIUM
- **`agent_state` exposes full cross-session state to any role** — `aspis_mcp.py:6502` + `public_agents_state:1982`: a read-only `mini` can read the orchestrator's plan/active task/file/`needsUser` of other sessions. **Fix:** scope to caller's own session or drop from `mini`. `NEEDS-REVIEW`
- **Mini with no `project_id` reads all project `.md`** — `aspis_mcp.py:1493` (`enforce_mini_oracle_project_scope` skips when empty). **Fix:** require scope for mini. `NEEDS-REVIEW`
- **`multi_mcp` global deadline kills already-connected servers** — `multi_mcp.rs:156`: one hanging server drops all working ones. **Fix:** per-server timeout, preserve completed. `NEEDS-REVIEW`
- **`normalize_lexical` doesn't strip `..`** — `oracle_service.rs:784`: on canonicalize failure (non-existent path), `../../` can pass the `starts_with` scope check → commit path escape. **Fix:** reject `ParentDir` components. `NEEDS-REVIEW`
- **`ASPIS_PROJECTS_DIR` accepted unvalidated** — `oracle_service.rs:206`: env override redirects the discovery file (contains the AGENT token) anywhere. **Fix:** validate within `$HOME`/allowlist. `NEEDS-REVIEW`
- **`discover_installed_models` bypasses `MAX_MODELS` caps** — `model_registry.rs:195`: re-parses backend JSON without the `provider_detect` bounds → loopback flood. **Fix:** reuse `parse_omlx_models`/`parse_ollama_tags`. `NEEDS-REVIEW`
- **`set_model_registry` errors on first run** — `model_registry.rs:149`: fails if `config.json` absent (unlike `get_*`). **Fix:** start from `{}` on NotFound. `NEEDS-REVIEW`
- **Non-atomic blob write** — `training_export.rs:537`: `fs::write` (not temp+rename); a partial blob gets permanently deduped. **Fix:** temp+rename. `NEEDS-REVIEW`
- **Missing `validate_rel_path` on `changed_files`** — `training_export.rs:590`: Censor-supplied paths snapshot without the `..` guard applied to `files_touched`. **Fix:** validate in the loop. `NEEDS-REVIEW`
- **`agent_loop` orphan-drop can lose the newest round's assistant context** — `agent_loop.rs:593`. **Fix:** evaluate against any surviving assistant entry. `NEEDS-REVIEW`
- **Double-claim race** — `runner.rs:298`: `claim_task`+`spawn` not atomic; concurrent runners double-delegate. **Fix:** server-side lease (document; no in-proc fix). `NEEDS-REVIEW`
- **rmcp identity-key clobber** — `rmcp_backend.rs:412`: injecting `role/agent_id/session_token` silently overwrites a same-named model param. **Fix:** debug-assert/warn. `NEEDS-REVIEW`
- **`dev` escape hatch** — `aspis_mcp.py` `ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS=1` bypasses launch-token + session-token (not role-gate). Not set by the Rust launcher. **Fix:** startup warning + refuse if cloud token present. `NEEDS-REVIEW`
- **Codex/API agentic path: no OS sandbox** — only argv allowlist + `env_clear`; an allowlisted `cargo test` with a malicious test file can read arbitrary files into context. Architecture trade-off — **document** it. `NEEDS-REVIEW`

### ⚪ LOW / defensive
- `fs_replace.rs:22` backup-removal TOCTOU (`SAFE-MECHANICAL` rewrite to unconditional rename).
- `censor/ledger.rs:115` + `aspis_mcp.py validate_censor_rel_path`: no null-byte check (OS rejects downstream) — add one line. `validate_censor_rel_path` also missing the 1024-char cap `validate_plan_scope_path` has.
- `oracle_service.rs:936` discovery temp-file in `$TMPDIR` (0o600) — same-UID race window; `O_TMPFILE` would close it.
- `changes.rs:258` `open_in_editor` leaks a zombie child (no `wait`/drop).
- `model_client.rs:482` transcript eviction drops more history than necessary (stops at first over-budget entry).
- `budget.rs:551` `plan_placement` vs `admit_local_spawn` use inconsistent free-bytes views.

---

## 3. Open manual-test bugs — root causes located

- **B5 (orchestrator acts as worker)** — `projects.rs:2485` normalizes `orchestrator`→`coder`, so a Claude/Codex orchestrator gets the generic coder prompt with `project_next_task` (task-pull) + unconditional `oracle_context` (`projects.rs:3178-3179`). No "you are the orchestrator → `plan_submit` first" branch for non-devboule clients. **Fix:** add an orchestrator `role_rule` + swap the task-entrypoint to `plan_submit` when `client=="orchestrator"` (skill override already wired at 1537). `NEEDS-REVIEW`
- **B16 (Stop → "Planned")** — `projectStage.ts:146` default fallthrough is `"planned"`; a finished project (empty board from B9 + no active sessions + `status` never set to `done`) falls through. **Fix:** preserve last status before the default + wire `agent_heartbeat status=done` → `project_update_status(done)`. `SAFE` (fallback) / `NEEDS-REVIEW` (completion signal)
- **B18 (orphan manual task)** — `runner.rs read_plan_views()` skips tasks with empty `planId`; manual tasks (`create_project_task` → `plan_id: None`) are stored correctly but invisible to the runner. **Fix (Option A, low-risk):** have `project_next_task` surface manual tasks as ad-hoc single-phase items (no runner change). `NEEDS-REVIEW`
- **B10 (~5-min freeze)** — no single code bug. 300s timers found (`python_oracle.rs:70` hung-child restart, `mini_coder.rs:65` retry cap) but none kills the app. Most likely **RAM exhaustion** (dev build + Vite + oMLX large model). **Fix:** add a default Oracle HTTP client timeout (safe) + investigate RAM in Activity Monitor / capture `tauri dev` stderr for a panic. `SAFE` (timeout) + runtime investigation.

---

## 4. Simplifications available (SAFE-MECHANICAL, not applied)

- `projects.rs ~838-1380`: **7 near-identical config setters** → one `update_config<F>` helper (~300 lines).
- `vault.rs`: 4 near-identical `*_key_status` functions (~120 lines) + double `map/trim/filter` in `save_oracle_llm_key`.
- `aspis_mcp.py`: **~8 redundant allowlist double-checks** after `require_agent_tool` (dead code); `cap_*` ×4 dedup (~90 lines); `validate_censor_rel_path`/`validate_plan_scope_path` 90% dup; `dispatch_project_structure` dead `if not project_id`; `oracle_ask` double `clean_text`.
- `devboule-coder`: `truncate_chars`/`cap_chars` implemented 4× → shared util; `push_capped_window` dup in `agent_loop.rs`; `build_http_client` dup (Omlx/Cloud); `next_runnable`/`ready_batch` overlap; **`Frontier::Ready(usize)` → unit variant** (index confirmed dead by 2 agents).
- `mini_coder_executor.rs`: `interruptible_sleep` helper (2 dup loops); `lock_recover<T>` helper (14 dup poison-recovery); single `try_state` lookup (1619); single `Utc::now()` in `upsert_mini_session` (3339/3362).
- `mini_activity.rs:362`: O(n²) `evict_if_needed` → sort-once/drain.
- `budget.rs:291`: `gib()` shadows `GIB` const with a duplicated literal.

---

## 5. Confirmed CLEAN (security lens)
`provider_detect.rs` (redirects disabled, 127.0.0.1 only, 256 KiB cap, bounded model lists) · `vault.rs` (keyring, raw values never returned, role-gated CF token, `sanitize_llm_base_url`) · `censor/gemma.rs` (no-redirect, loopback-clamped, capped) · `budget.rs` probes (inherit provider_detect guards) · `fs_replace.rs` (atomic rename, backup cleaned on error) · `user_mcp_config.rs` (reserved-prefix, exact Oracle-name block, ASCII charset, fail-secure empty allowlist, mini exclusion hard-wired) · `agentic_tools.rs` hardlink/symlink rejection · `training_export.rs` `files_touched` path validation.

---

## 6. Coverage & honesty
This was a **broad** pass (8 agents over backend + oracle-python + devboule-coder + frontend + a security lens), not the earlier tiny slice. NOT covered deeply: the 2000+-line view files (`ComputeView`, `CloudflareView`, `AppContext`, `DesignView`, `ProjectsView`) — flagged for a dedicated session with the owner. The code-review *plugin* is diff-based (ran on the cleanup diff); whole-codebase auditing here used code-simplifier agents in audit mode. clippy = lint breadth, not a plugin.
