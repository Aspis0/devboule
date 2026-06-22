# Local-coder bug ledger (2026-06)
> Real bugs from local-model-generated code caught in sonnet review. Training data (buggy → bug → fix). UNTRACKED — do not commit.

## Phase 1 — `src-tauri/src/backend/budget.rs` (coder: gemma-4-26B-A4B, tuned)

### Bug 1 — blocking call in async [HIGH]
Buggy (model output): inside `#[tauri::command] pub async fn poll_backend_memory`, the model called the synchronous, subprocess-spawning `collect_hardware()` bare:
```rust
let hardware = crate::backend::hardware::collect_hardware();
```
Bug: `collect_hardware()` shells out to `system_profiler`/DXGI (blocking) — calling it bare on the Tokio async worker blocks the thread.
Fix:
```rust
let hardware = tauri::async_runtime::spawn_blocking(crate::backend::hardware::collect_hardware)
    .await
    .map_err(|e| format!("hardware probe failed: {e}"))?;
```

### Bug 2 — narrow integer type → silent data loss [MEDIUM]
Buggy: oMLX `engine_pool` counts deserialized as `Option<u32>` then `unwrap_or(0)`. A value outside u32 range deserializes to `None` → silently 0.
Fix: wire-type as `Option<u64>`, narrow with a saturating cast: `pool.loaded_count.unwrap_or(0).min(u32::MAX as u64) as u32`.

### Bug 3 — empty value leaks across IPC [MEDIUM]
Buggy: `name: m.name.unwrap_or_default()` — a missing Ollama model name becomes `""` and is sent to the UI as a blank-named model.
Fix: `filter_map` skipping empties: `let name = m.name.filter(|n| !n.is_empty())?;`

### Bug 4 — duplicate import (integration) [LOW]
Buggy: a follow-up chunk re-emitted `use serde::Serialize;` although the file already had `use serde::{Deserialize, Serialize};` (would not compile). Caught at integration.

### Note — ungated command [INFO]
`poll_backend_memory` had no auth gate; judged acceptable (non-secret backend status, like `detect_hardware`/`detect_providers`) and documented as intentionally ungated.

## Phase 1 — rejected model output: Qwen3.6-27B-OptiQ (dense), same task
Two compile errors the MoE coders did NOT make (kept as training data — a capable model's typical slips):
- Wrong module path: emitted `mod provider_detect;` + `use provider_detect::{...}` instead of `use crate::backend::provider_detect::{...}`.
- Missing `.await`: `let body = probe_get(client, "...")?;` — `probe_get` is async, the `.await` was omitted.
(Also: at a too-low token budget the dense model failed entirely, writing a file-reading "exploration script" instead of the requested module — fixed by raising thinking_budget/max_tokens.)

## Phase 2 — `src-tauri/src/backend/budget.rs` global budget accountant (coder: gemma-4-26B-A4B, tuned)

### Bug 1 — GiB-vs-GB unit mismatch [BLOCKER]
Buggy (model output): RAM total computed with SI 1e9 while the source value is in binary GiB:
```rust
let total_ram_bytes = (hardware.ram_total_gb * 1_000_000_000.0) as u64;
```
Bug: `hardware.ram_total_gb` is GiB (sysinfo bytes / 1024^3), so multiplying by 1e9 undercounts RAM by ~6.87% (64 GiB → 64.0e9 instead of 68_719_476_736). Every displayed/derived number is wrong.
Fix:
```rust
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
let total_ram_bytes = if hardware.ram_total_gb.is_finite() && hardware.ram_total_gb >= 0.0 {
    (hardware.ram_total_gb * GIB) as u64
} else { 0 };
```

### Bug 2 — sum::<u64>() overflow panic on hostile input [BLOCKER]
Buggy: `o.models.iter().map(|m| m.size_bytes).sum::<u64>()` — a malicious/buggy local Ollama returning `size: u64::MAX` overflows the sum, which PANICS in debug builds (DoS the command).
Fix: saturating fold — `o.models.iter().fold(0u64, |acc, m| acc.saturating_add(m.size_bytes))`.

### Bug 3 — reserve constant in SI units, inconsistent with GiB codebase [WARNING]
Buggy: `const DEFAULT_RESERVE_BYTES: u64 = 8_000_000_000;` (SI). Fix: `8 * 1024 * 1024 * 1024` (8 GiB), consistent with sysinfo/oMLX/Ollama binary units.

### Bug 4 — float→int cast trap [WARNING]
`f64::INFINITY as u64` yields `u64::MAX` (not 0). Unguarded today (safe because ram_total_gb is always finite from sysinfo) but a footgun once the value can come from Settings (Phase 3). Fix: the `is_finite() && >= 0.0` guard shown in Bug 1's fix.

### Bug 5 — unit tests validated the WRONG arithmetic [WARNING]
The model's tests hard-coded expected values derived from the 1e9 bug (e.g. `budget_bytes == 56_000_000_000`), so they passed while the code was wrong. Fix: assert `total_ram_bytes == 64 * GIB` (binary) — this assertion is now the GiB-vs-GB regression guard.

### Note — Ollama VRAM accounting [NITPICK, deferred]
Reviewer suggested subtracting `size_vram_bytes` from Ollama `size_bytes` for system-RAM pressure. NOT applied: on UNIFIED memory (Apple Silicon) VRAM IS system RAM, so subtracting would undercount. Regime-dependent — belongs in the placement phase (4/5), not here.

## Phase 3a — `src-tauri/src/backend/model_registry.rs` (coder: gemma-4-26B-A4B, tuned, with a provided template)

### Bug 1 — wrong import path for a repo-specific helper [MEDIUM]
Buggy (model output): imported all three config helpers from one module:
```rust
use crate::backend::projects::{config_write_lock, locate_config_path, replace_file_with_backup};
```
Bug: `replace_file_with_backup` lives in `crate::backend::fs_replace`, not `projects` — the model can't see the repo layout and guessed. (Classic local-model failure: repo-specific helper location.)
Fix:
```rust
use crate::backend::fs_replace::replace_file_with_backup;
use crate::backend::projects::{config_write_lock, locate_config_path};
```

### Bug 2 — GET all-or-nothing parse → data-loss loop [MEDIUM]
Buggy: `serde_json::from_value::<Vec<ModelRegistryEntry>>(registry_val.clone())` — one malformed stored entry makes the WHOLE deserialize fail, so GET silently returns an empty list; a subsequent SET with that empty list then wipes the user's registry.
Fix: per-entry tolerance (mirrors read_custom_agent_clients):
```rust
if let Some(arr) = value.get("modelRegistry").and_then(|v| v.as_array()) {
    return Ok(arr.iter().filter_map(|e| serde_json::from_value::<ModelRegistryEntry>(e.clone()).ok()).collect());
}
```

### Bug 3 — tuned sampling params persisted unvalidated [WARNING]
Buggy: temperature/top_p/top_k/thinking_budget stored with zero bounds checks → a negative temperature / top_p>1 / top_k=0 / huge thinking_budget would be forwarded verbatim to the oMLX/Ollama inference call.
Fix: validate at the config boundary — temperature in [0,2], top_p in [0,1], top_k>=1, thinking_budget<=32768.

### Nit — unused `mut` [LOW]
`let mut filtered_roles = ...` was never mutated; dropped the `mut` at integration.

Note: this chunk was given the proven set_custom_agent_clients RMW code as a template, and the model adapted it correctly EXCEPT for the helper import path (Bug 1) — i.e. providing the template prevented the harder RMW/atomicity mistakes.

## Phase 3c — `src/components/settings/ModelRegistryCard.tsx` Settings UI (coder: gemma-4-26B-A4B, tuned)

### Bug 1 — unused destructure → tsc noUnusedLocals error [MEDIUM]
Buggy: `const { config } = useAppContext();` — `config` was never used (the card loads via get_model_registry, not config). With noUnusedLocals this fails the build.
Fix: dropped the `useAppContext` import + the `config` line; kept `useAppActions` for refreshConfig.

### Bug 2 — UI rows keyed by id alone → wrong-row mutation across backends [MEDIUM]
Buggy: `updateEntry(id)`/`removeEntry(id)` and React `key={e.id}` used id only — but the registry dedupes by (backend,id), so the same id can exist on omlx AND ollama; editing one would mutate both / collide React keys.
Fix: composite key `keyOf(e)=`${e.backend}:${e.id}`` for React keys; update/remove match on `e.id===id && e.backend===backend`.

### Bug 3 — `as any` on a select value [LOW]
Buggy: `tier: ev.target.value as any`. Fix: `as ModelRegistryEntry["tier"]`.

### Bug 4 — double-save race (sonnet review) [WARNING]
Buggy: `saveRegistry` guarded only by `setBusy(true)` (async) — a fast double-click runs two concurrent set_model_registry writes.
Fix: synchronous `savingRef` guard — `if (savingRef.current) return; savingRef.current = true;` ... reset in finally.

### Bug 5 — leaked "Saved" timer (sonnet review) [WARNING]
Buggy: `window.setTimeout(...2000)` for the Saved flash never cleared on unmount.
Fix: store the handle in `savedTimerRef` and `clearTimeout` it in the unmount cleanup.

### Bug 6 — weak TS type [NITPICK]
`roles: string[]` → `Array<"mainCoder"|"miniCoder"|"censor">`.

### Test fallout — new card not mocked broke the tab test [MEDIUM]
The ProvidersModelsTab test mocks every child card AND mocks AppContext to ONLY export invokeBackendCommand (not useAppActions). The new ModelRegistryCard wasn't mocked, so it hit the incomplete AppContext mock (useAppActions undefined) and crashed the whole render → all 5 tab tests failed. Fix: `vi.mock("./ModelRegistryCard", ...)` to a marker + assert its testid (mirrors the other card mocks). LESSON: when adding a child card to a composed tab, also add it to the tab test's card-mock set.

## Phase 4 — `src-tauri/src/backend/budget.rs` spawn-gate (coder: gemma-4-26B-A4B, tuned)

The admission LOGIC (`admit_local_spawn` → Admit/Queue/RouteToCloud) was correct first try — good. Only one integration bug:

### Bug 1 — test referenced a const from a sibling test module [MEDIUM]
Buggy: gemma's `mod admission_tests` used `GIB_U64`, but that const is defined inside the OTHER `mod tests` — `use super::*` imports the PARENT module's items, NOT a sibling test mod's. So it would not compile.
Fix: declare a local `const GIB_U64: u64 = 1024 * 1024 * 1024;` inside `admission_tests`.

### Design note — gate placement (not a model bug) [INFO]
The plan was a HARD gate inside the live `mini_coder_executor::claim_and_launch`. But that fn is SYNC and already holds the agent-state lock, so an inline async budget probe (HTTP to oMLX/Ollama) can't go there safely. Phase 4 shipped the gate as an async COMMAND (`evaluate_local_spawn`) on live budget data; the in-executor HARD enforcement (threading a per-pass budget snapshot in) is a documented follow-up. Lesson: a local model's pure logic is reliable; the risky part is always the live-path WIRING, which a human/careful pass should own.

## Phase 5 — `src-tauri/src/backend/budget.rs` recommended-config tiering (coder: gemma-4-26B-A4B, tuned)

The tiering logic (`recommend_config` → minimal/low/mid/high placement) was correct first try. One integration bug:

### Bug 1 — test constructed a struct with missing fields [MEDIUM]
Buggy: gemma's tests built `BudgetSummary { total_ram_bytes, reserve_bytes, budget_bytes }` — only 3 of the struct's 7 fields, so the tests would not compile (Rust requires all fields in a struct literal).
Fix: added a `bud(total_gib, budget_gib)` test helper that fills all 7 fields (omlx_used_bytes/ollama_used_bytes/used_bytes/free_bytes too).

Recurring pattern across phases: the local model writes correct LOGIC but slips on (a) repo-specific helper import paths, (b) struct field completeness / sibling-module const scope in tests. The sonnet review + cargo catch these every time.

## Phase 6c-i — `src-tauri/src/backend/agentic_tools.rs` SANDBOX (coder: gemma-4-26B-A4B, tuned)

⭐ HIGH-VALUE: real SECURITY bugs in local-model-generated sandbox code, caught by a hostile sonnet review. The pure logic was mostly right; the filesystem confinement had escape holes.

### Bug 1 — SANDBOX ESCAPE: grep follows symlinks out of scope [BLOCKER]
Buggy: the recursive `walk_grep` used `path.is_dir()` (follows symlinks) and `fs::read_dir` with NO per-entry root check — a symlink inside the repo (e.g. `node_modules/.bin`, a monorepo cross-link) pointing to `/etc` lets `grep` read the whole filesystem (exfiltration). The canonicalize check existed ONLY for the starting path, not the walked entries.
Fix: skip symlinks (`entry.file_type().is_symlink()` = lstat, doesn't follow → `continue`, fail-safe on error) AND canonicalize each walked path, requiring it `starts_with(canon_root)`; pass `canon_root` down.

### Bug 2 — OOM: grep has no per-file size cap [BLOCKER]
Buggy: `read_file` capped reads at 256KB, but `walk_grep`'s `fs::read_to_string` had no cap — a single multi-GB file (build artifact, db dump) in scope → heap blow-up / OOM-kill. The 2000-file limit doesn't help (one file suffices).
Fix: `fs::metadata(&path).len() > 4MB` → skip the file before reading.

### Bug 3 — stack overflow: unbounded recursion depth [WARNING]
Buggy: `walk_grep` recursed with no depth counter → a deep tree overflows the 8MB stack. Fix: `depth` param, cap at 50.

### Bug 4 — absolute-path check before normalization [WARNING]
Buggy: `safe_rel_path` checked `starts_with('/')` on the raw string BEFORE `\\`→`/` normalization, so `\foo` → `/foo` slipped past it (was only caught incidentally by the empty-component check). Fix: re-check leading `/` AFTER normalization.

### Bug 5 — UTF-8 panic in truncation [MEDIUM]
Buggy: `&content[..max_read_bytes]` slices at a raw byte index → panics if it lands mid-UTF-8-char. Fix: walk `end` down to the nearest `is_char_boundary`.

### Bug 6 — over-rejected valid paths [LOW]
`safe_rel_path` rejected any `.` component, so `././x` and the `list_dir`/`grep` default `"."` failed. Fix: DROP `.`/empty components (and treat all-dropped as the scope root `"."`).

### Note for 6c-ii (write tools) [INFO]
`resolve()` returns the lexical join WITHOUT a canonical check when neither the path nor its parent exists — fine for reads (a missing file just fails to open) but a WRITE must NOT trust it (use cap-std or refuse). Documented in the file.

LESSON: local models write plausible sandbox code with REAL escape holes (symlink-follow, missing caps). A hostile security review is non-negotiable for sandbox/security code.

## Phase 6c-ii — `src-tauri/src/backend/agentic_tools.rs` WRITE tools (coder: gemma-4-26B-A4B, tuned)

⭐ HIGH-VALUE security entry: write/edit sandbox. The local model's write_resolve was good (parent-canon + symlink refusal), but a hostile sonnet review found a deeper escape it missed.

### Bug 1 — HARDLINK escape [BLOCKER]
Buggy: write_resolve refused symlinks and canonicalized the target, but a HARDLINK (nlink>1) shares an inode with a possibly out-of-scope file; `canonicalize` resolves directory symlinks, NOT inode aliases, so `scope/x` hardlinked to `~/.ssh/id_rsa` passes every check and `fs::write` clobbers the outside file. (Pre-condition: an externally-planted hardlink in the scope.)
Fix (unix): after the canon check, `use std::os::unix::fs::MetadataExt; if fs::metadata(&full)?.nlink() > 1 { return Err("refusing to write to a hardlinked file") }`. Added a hardlink-escape regression test.

### Bug 2 — edit_file unbounded read → OOM [WARNING]
Buggy: write_file capped INPUT at 1MB, but edit_file did `fs::read_to_string` on the on-disk file with NO size cap → a multi-GB in-scope file OOMs. Fix: `fs::metadata(&p).len() > MAX_WRITE_BYTES` → refuse before reading.

### Bug 3 — raw model path echoed in success message [NITPICK]
Buggy: `format!("wrote N bytes to {path}")` used the raw model-supplied path ("a/./b/../c"), fed back to the model as tool output (could mislead its next step). Fix: use the resolved `p.display()`.

### Documented residual — TOCTOU [INFO]
Check→fs::write has a time-of-check/time-of-use window (a concurrent rename of the verified parent could defeat the scope). Low risk in a single-user desktop app + outside the semi-trusted-LLM threat model; cap-std / openat(O_NOFOLLOW) is the structural fix — documented as a follow-up before multi-tenant use.

LESSON (repeat of 6c-i): local models produce plausible sandbox code that passes the OBVIOUS checks (symlinks) but misses the non-obvious inode-level escape (hardlinks). Security-code review by a strong model is non-negotiable.

---

## Final MAX-RECALL (cross-phase, 3-reviewer) — agentic + budget

⭐ The whole-diff max-recall caught CROSS-PHASE bugs that the per-phase reviews structurally could not (each phase's code was individually fine; the bugs were in the INTERACTION). High-value training signal: local models build plausible per-file code that's wrong at the protocol/cross-module level.

### Bug 1 — PROTOCOL BLOCKER: agentic loop never emitted a tool_calls array [BLOCKER]
Buggy: `run_agent_loop` recorded the assistant's tool-call turn as a TEXT SUMMARY (`format!("{name}({args})")`) in `ChatMsg.content`, and `ChatMsg` had no `tool_calls` field. So `build_chat_request` sent the assistant turn as plain text — the following `tool` role messages had a `tool_call_id` with NO preceding `tool_calls` array. A REAL oMLX/Ollama server rejects this (HTTP 400) → multi-turn tool-calling never works. The unit tests passed because MockLlm bypasses the wire. (Classic: tests-green-but-broken-on-real-server.)
Fix: add `tool_calls: Option<Vec<ToolCall>>` to ChatMsg; store the real calls on the assistant turn; `build_chat_request` serializes `{role:assistant, content:null, tool_calls:[{id,type:function,function:{name,arguments}}]}`. New serialization test.

### Bug 2 — placement ignored live usage [BLOCKER]
Buggy: `plan_placement` started its budget counter at `used=0`, ignoring `budget.used_bytes` (what the backends already hold) — INCONSISTENT with `admit_local_spawn`, which uses `free_bytes`. Two functions used different denominators for the same RAM → over-commit. Fix: `used = budget.used_bytes`; live-usage test.

### Bug 3 — admit ordering [WARNING]
`admit_local_spawn` checked the compute cap BEFORE never-fits, so a model too big for the whole budget got `Queue` (retry→cap→re-queue forever) instead of `RouteToCloud`. Fix: never-fits → cloud FIRST.

### Bug 4 — parse aborts on malformed turn [WARNING]
`parse_llm_turn` returned Err (→ loop Abort) when a turn had null content + only empty-named tool calls (a quantized-model glitch). Fix: return an empty Message (graceful); Err reserved for "no choices/message".

### Bug 5/6 — edit_file 'too large' on missing file; sequential probes [WARNING/perf]
edit_file's `metadata().unwrap_or(true)` reported "too large" for a NON-existent file → fix: explicit "file not found" first. poll_backend_memory awaited the two independent probes sequentially → `tokio::join!`.

LESSON: per-file review can't catch protocol/cross-module bugs — the END max-recall is essential. The single most dangerous class for local-model code: "compiles + unit-tests-green but wrong against the real wire protocol" (Bug 1).
