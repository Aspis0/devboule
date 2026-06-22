# Reviewer findings — Devboule local-coder effort (2026-06-17) — TRAINING PAIRS

> Purpose: real hostile-reviewer findings across the whole 11.3→11.5-B session, distilled as
> bug→fix pairs for the **review-experts** training (per-language code-review LoRA). Only
> **CONFIRMED + PLAUSIBLE** findings are kept; **REFUTED / false-positive** verdicts are
> EXCLUDED per owner instruction (≈ 30+ items the reviewers ruled out — available as
> "looks-suspicious-but-correct" NEGATIVES if ever wanted, but NOT listed here as bugs).
>
> Legend — **⊕ = SEMANTIC** (logic / correctness / security / concurrency — the high-value
> training signal); **○ = structural/cosmetic/test-coverage** (lower value). Lang: RS=Rust,
> PY=Python, TS=TypeScript. All bugs below were FIXED + verified this session (commits
> 88128ba · a3ad321 · dc07e91 · 27d2928 on `mac-platform-fixes`); line numbers refer to the
> PRE-fix state, so each entry describes the PATTERN (robust) not just a line.

---

## BLOCKER — would break correctness or security (all ⊕ SEMANTIC)

### B-1 ⊕ RS · serde · `action.rs::AgentAction`
- **Bug:** a fieldless action variant declared as a UNIT variant (`RunPlan,`) in an internally-tagged enum (`#[serde(tag="tool", deny_unknown_fields)]`). `deny_unknown_fields` is SILENTLY NOT enforced for a unit variant → `{"tool":"run_plan","steps":[...]}` parses OK and the extra field is swallowed (no FORMAT ERROR fed back to the model).
- **Why:** the strict-parse contract (every stray key is a hard error the model self-corrects from) is the load-bearing safety of the agent loop; a unit variant silently breaks it for that one tool.
- **Fix:** make it an EMPTY STRUCT variant (`RunPlan {}`); serde enforces `deny_unknown_fields` on struct variants. Add a test that `{"tool":"run_plan","junk":1}` is rejected.

### B-2 ⊕ RS · control-flow / error-masking · `runner.rs::run_tasks`
- **Bug:** when no active plan exists, the function returns `Ok(RunReport{completed:0,total:0,blocked:None})` → the executor maps it to a SUCCESS "Ran 0/0; all finished", so the orchestrator believes the plan ran when nothing happened.
- **Why:** "nothing to do" must not be indistinguishable from "succeeded"; a missing precondition silently reported as success hides the real state from the caller.
- **Fix:** distinguish "no plan-tagged tasks at all" → `Err("no active plan; run \`plan\` first")` from "plan exists but already all finished" → `Ok` with the REAL finished count.

### B-3 ⊕ PY · wrong durable verdict · `aspis_mcp.py::_await_mini_directive`
- **Bug:** a shared poll helper hardcodes the string `"spawn_mini_coder poll timed out..."` in its synthesized timeout result, but the helper is also called from `mini_coder_result` → the directive's persisted `result.error` lies about which tool timed out.
- **Why:** the synthesized result is the caller's durable verdict; a wrong attribution misleads retry logic (a supervising coder may re-`spawn` thinking the original spawn timed out when the mini actually completed).
- **Fix:** parameterize the helper with `caller_tool: str` and interpolate it into the error strings.

### B-4 ⊕ PY · security / authz bypass · `aspis_mcp.py::_mini_directive_parent_agent_id`
- **Bug:** the climb-to-root branch (for a non-root directive id) re-does `by_id.get(directive_id)` — the SAME lookup that already returned `None` — instead of following `parentDirectiveId`. So it returns `None`, and the caller's ownership check (`if owner is not None and owner != agent_id`) is SILENTLY SKIPPED → any registered agent can read any directive's result.
- **Why:** an ownership guard that no-ops for non-root ids is no guard; this is a real authz bypass introduced by a copy-paste/typo in a previously-added "fix".
- **Fix:** WALK UP `parentDirectiveId` (loop, cycle/depth-guarded) to the true root and return its `parentAgentId`. (Lesson: a security check needs a test that a NON-owner is actually rejected, not just that the owner is allowed.)

### B-5 ⊕ PY · security / authz bypass · `aspis_mcp.py::steer_mini_coder`
- **Bug:** ownership guard `if root_owner and root_owner != agent_id: raise` only fires when `root_owner` is NON-EMPTY; and the chain-resolution picked the directive matching `id==directive_id` directly, so a RETRY-CHILD id (which has no `parentAgentId`) resolves to an empty owner → check skipped. A coder who learns any child id (readable via `agent_state`) can steer or `"stop"`-KILL another agent's running mini.
- **Why:** an empty/absent owner must be treated as "not yours" (fail-closed), not "anyone's"; and ownership must resolve the TRUE chain root, not whichever chain member the id happens to name.
- **Fix:** resolve the true root (shared root-walk helper) + raise when `owner != agent_id` INCLUDING empty owner (fail-closed).

### B-6 ⊕ PY · security / missing trust gate · `aspis_mcp.py::project_create_plan_tasks`
- **Bug:** the tool bulk-creates up to 40 auto-executable Kanban tasks for a given `plan_id`, but NEVER verifies the plan is approved. Any registered coder/orchestrator can pass a fabricated / rejected / never-submitted `plan_id` and the runner will auto-execute the tasks. Enforcement was purely an LLM instruction ("wait for approval"), with zero code gate.
- **Why:** a human-approval gate enforced only by prompt is not enforced; a privileged side-effect (creating work the runner auto-runs) needs a code check of the approval state.
- **Fix:** validate `plan_id` against the id regex, and INSIDE the lock check the plan's approval status == "approved" (using a non-locking in-state reader to avoid re-entrant deadlock), else raise.

### B-7 ⊕ RS · concurrency / resource leak + gate bypass · `projects.rs::apply_plan_task_control`
- **Bug:** the `skip` action only rejected `done`; a `wip` task (mini actively running) was stamped `done`. The mini keeps running (no PTY kill), then its later `project_update_status("review")` hits the done-lock and fails → the runner reports a misleading "blocked", the verifier gate is bypassed, and the PTY is a zombie. Downstream DAG tasks also start (done satisfies deps) and may write the same files in parallel.
- **Why:** a status mutation that bypasses the claim gate must not be applied to a LIVE task; "skip" is for non-running tasks (stopping a live mini is a different, agent-scoped action).
- **Fix:** reject `wip` in the skip arm ("stop the mini first"); mirror in the frontend `canSkip`.

---

## WARNING — real bug, narrower blast radius

### W-1 ⊕ RS · timeout mismatch / false failure · `rmcp_backend.rs::CALL_TOOL_TIMEOUT`
- **Bug:** a single flat client `call_tool` timeout (120s) below the SERVER's blocking poll for slow tools (`spawn_mini_coder` server poll = 1800s; human-gated `plan_submit`/`request_git_push` = 600s). The client gives up while the server is still working → a mini taking >120s (a real coding task easily takes minutes) is falsely reported "blocked" while it is still running; human approval is effectively capped at 120s.
- **Why:** a client timeout MUST exceed the server-side blocking-poll deadline of the tool it calls, or every legitimately-slow call is cut into a false error.
- **Fix:** per-tool timeouts > the server poll (`spawn_mini_coder`=1920s, gated tools=720s, etc.); pin the invariant with a test.

### W-2 ⊕ RS · concurrency / lost signal · `mini_coder_executor.rs::mark_steer_requested`
- **Bug:** steer targeting used `status.is_active()` (= {launching, running} only) then fell back to matching `agent_id`. In the retry-handoff window (predecessor = AwaitingRetry, retry = Pending with no agent_id), the Tauri (agent_id-keyed) steer resolved the DEAD predecessor → the steer was appended to a directive that never re-runs → SILENTLY LOST. The Python (directive-id-keyed, includes `pending`) path handled it — a cross-language divergence.
- **Why:** "the attempt that will run" is not the same as "the active attempt"; targeting must prefer the live OR highest-attempt non-terminal chain member, and must match the Python semantics.
- **Fix:** target an active member else the highest-attempt non-terminal chain member (matching Python's "active preference"); do NOT widen `is_active()` globally.

### W-3 ⊕ RS · concurrency / unhonored kill · `mini_coder_executor.rs` (AwaitingRetryWith branch)
- **Bug:** the `live_kill_override` (honor a human Stop landing in the gate window) was applied to the Escalate + StampTerminal branches but NOT the AwaitingRetryWith branch. A Stop / `"stop"`-steer landing in that window spawns the retry anyway (and the carry-forward carried `steer_queue` but not `kill_requested`), so the run continues despite the human asserting Stop.
- **Why:** a kill signal must be honored at EVERY round-boundary transition, not a subset; an inconsistent guard is a leak.
- **Fix:** consult the live kill in the AwaitingRetryWith branch too → abort (no retry) when set.

### W-4 ⊕ RS · error-swallow / lost result · `runner.rs` (set_review after a successful mini)
- **Bug:** `set_review(...).await?` after a mini returned `done`: if the status update fails (lease expired / transient), the `?` propagates as a transport error, leaving the task `wip` → on retry the runner re-delegates and the mini re-does already-completed work (non-idempotent side effects).
- **Why:** a post-success bookkeeping failure must not discard the mini's completed work nor masquerade as a transport error that triggers re-execution.
- **Fix:** on a `set_review` failure, return a `Blocked` report ("mini finished but could not set review; update manually") — preserve the work, surface to the human.

### W-5 ⊕ RS · stale snapshot / wrong count · `runner.rs::run_tasks`
- **Bug:** `total` was snapshotted from the FIRST board read; the loop re-reads each iteration, so `completed` (from a fresh read) could exceed `total`, or `AllDone` could fire after a task was removed mid-run.
- **Why:** when a derived count and its denominator come from DIFFERENT reads of mutable shared state, the invariant `completed ≤ total` breaks.
- **Fix:** compute `completed` AND `total` from the SAME latest read when building the report.

### W-6 ⊕ PY/RS · cross-language serialization mismatch (content churn) · `aspis_mcp.py::project_create_plan_tasks` vs `model.rs`
- **Bug:** Python wrote empty optional fields (`"dependsOn":[]`, `"scope":[]`, `"acceptance":""`) while the Rust struct uses `skip_serializing_if` (omits them). On the first Rust re-serialize the fields vanish → the content hash changes → spurious git-dirty + Oracle re-index.
- **Why:** two writers of the same on-disk structure must agree on the serialization of empty values, or every cross-writer round-trip churns.
- **Fix:** Python omits the empty optional fields too (mirror `skip_serializing_if`).

### W-7 ⊕ PY/RS · cross-language validation gap · `aspis_mcp.py::validate_plan_scope_path` vs `action.rs::check_rel_path`
- **Bug:** the Python rel-path validator omitted the 1024-char length cap that the Rust `check_rel_path` enforces. A >1024-char scope path passes Python, is stored, then the Rust runner's `check_rel_path` rejects it at execution → the task blows up live instead of at creation.
- **Why:** a mirrored validator that's weaker on one side lets bad data through the lenient gate and fail at the strict one (worse, far from the cause).
- **Fix:** add the matching length cap to the Python validator (and ideally a cross-language test pinning the mirror).

### W-8 ⊕ PY · injection / control-char hygiene · `aspis_mcp.py::project_create_plan_tasks` (acceptance)
- **Bug:** the `acceptance` field was sanitized with bare `str(...).strip()[:4000]`, bypassing the `strip_invisible_and_bidi` pipeline every other user-facing string uses → a U+202E-obfuscated acceptance survives into a field the runner later folds into the mini's prompt + the human reads.
- **Why:** any string that reaches an LLM prompt or a human display must go through the bidi/invisible-char stripper; a one-off bare sanitize is a hole.
- **Fix:** `strip_invisible_and_bidi(...)` before the length cap (without the non-empty requirement, since acceptance may be empty).

### W-9 ⊕ RS · cross-language hygiene parity · `mini_coder_executor.rs::mini_coder_steer`
- **Bug:** the Rust steer command did `message.trim().chars().take(CAP)` with NO invisible/bidi stripping, while the Python `steer_mini_coder` routed through `clean_text`→`strip_invisible_and_bidi`. The steer text is folded into the running mini's prompt + shown to the human → a parity hole letting a bidi-obfuscated steer through one path.
- **Why:** the same field reached via two entry points must get the same sanitization on both.
- **Fix:** apply the existing invisible/bidi stripper on the Rust path too.

### W-10 ⊕ PY · concurrency / TOCTOU · `aspis_mcp.py::dispatch_mini_coder_result`
- **Bug:** the not-found check, the ownership check, and the result read were done as SEPARATE lock acquisitions. The directive can be evicted (capped) between them → `_mini_directive_parent_agent_id` returns `None` → the ownership check passes for a non-owner (TOCTOU window).
- **Why:** a multi-step authz+read on mutable shared state must be atomic under ONE lock, or the gap is exploitable.
- **Fix:** do the not-found + ownership + read under a single lock acquisition.

### W-11 ⊕ PY · DoS / unbounded block · `aspis_mcp.py::dispatch_mini_coder_result`
- **Bug:** `mini_coder_result(wait=true)` on an unknown/evicted/mistyped `directive_id` entered the full poll and blocked the MCP thread for ~1800s before synthesizing a misleading "failed".
- **Why:** a blocking wait on a non-existent target should short-circuit, not hold a server thread for 30 minutes (a cheap DoS / footgun).
- **Fix:** a single preliminary read; if not found → return `not_found` immediately; only poll when the directive exists.

### W-12 ⊕ RS · security / authz scope (design) · `aspis_mcp.py` + runner (folded prompt) — injection boundary
- **Bug:** model-generated `title`/`acceptance` (stored on the board) and steer text are folded VERBATIM into the mini's prompt with no explicit "this is untrusted supervisor input" boundary. A compromised planner (e.g. an injection in an oracle response that reaches the plan) becomes an injection vector into every mini.
- **Why:** content that flows model→storage→another-model's-prompt should be clearly delimited/labeled as data, not instructions (defense in depth behind the mini's firewall).
- **Fix:** keep the folded content in clearly-labeled delimited sections; document the trust tier (the mini's prompt-injection firewall is the backstop).

### W-13 ⊕ PY · security / weak id validation · `aspis_mcp.py::project_create_plan_tasks` (plan_id)
- **Bug:** `plan_id` was only length-capped (`clean_text(...,200)`), not validated as the canonical 32-hex form (unlike `plan_status`). The runner auto-selects the active plan by this tag → an arbitrary/forged tag is accepted.
- **Why:** an id that gates a side-effect should be validated to its canonical shape, not just length.
- **Fix:** `_PLAN_ID_RE.fullmatch` before use (pairs with B-6).

### W-14 ⊕ RS · observability / silent failure · `runner.rs` (set_blocked)
- **Bug:** `let _ = set_blocked(...)` swallowed a failed block-transition → the task stays `wip` with NO signal, and the RunReport could claim `blocked` while disk shows otherwise.
- **Why:** a best-effort write may stay best-effort, but its FAILURE must be observable (a milestone / the report reason), not invisible.
- **Fix:** keep it best-effort but emit a "block-write failed" milestone + fold the error into the reason + report the real status.

### W-15 ⊕ TS · React lifecycle / stale closure · `PlanExecutionView.tsx`
- **Bug:** a single shared `mountedRef` across effect lifecycles: on `projectId` change, an in-flight fetch from the OLD projectId resolves after the new effect set `mountedRef=true` → it applies the OLD project's tasks + sets the OLD revision → a later control call gets a spurious "project changed on disk" conflict.
- **Why:** `mountedRef` answers "is the component mounted", not "is THIS fetch still current"; a per-effect cancellation token is needed to discard stale async results.
- **Fix:** a per-effect `let cancelled=false` (cleanup sets it true); ignore the fetch result when cancelled (or track+match the issuing projectId).

### W-16 ⊕ TS · React lifecycle / setState-after-unmount + timer leak · `MiniSteerBar.tsx`
- **Bug:** the async steer callback had no mounted guard; unmounting mid-call → `setBusy/setError/flashStatus` on an unmounted component AND a leaked `setTimeout(setStatus(null))`.
- **Why:** async callbacks that setState must guard on mount + cancel scheduled timers on unmount.
- **Fix:** a `mountedRef` guard around the post-await setState + timer-cancel on unmount.

---

## NITPICK — structural / cosmetic / cross-language drift / test-coverage (lower training value)

- **N-1 ○ RS** `runner.rs::next_runnable` — stall report named the first non-ready task, not the BLOCKED root cause (semi-semantic, clarity). Fix: prefer naming a `blocked` task.
- **N-2 ○ RS** `runner.rs` attempts cap `> MAX` allowed MAX+1 delegations (off-by-one vs the name). Fix: `>=` / fix the increment/check order.
- **N-3 ○ RS** `MAX_DELEGATED_TASK_CHARS=3000` < title(2000)+acceptance(2000) → the acceptance check could be truncated out of the mini's task. Fix: cap ≥ 4200.
- **N-4 ○ RS** `validate_plan_structure` accepted any `status` string → a hand-edited foreign status produced a misleading stall instead of a clear error. Fix: validate the status vocabulary.
- **N-5 ○ RS** `unwrap_or(0)` on an "impossible" None silently returned `Stalled(0)` instead of failing loud. Fix: `expect(invariant)`.
- **N-6 ○ RS** `planner.rs` planId from `plan_submit` not trimmed before the non-empty filter (a whitespace-only id passed). Fix: `.trim()` first.
- **N-7 ○ RS** `apply_emitted_edits` return-type refactor lacked a test for the in-run attempt-cap branch / the cap path; `render_action_block` lacked a `RunPlan` round-trip case (test-coverage gaps that would let a future break go unnoticed).
- **N-8 ○ RS/TS** cross-language tie-break drift: `selectActivePlanId` (TS, insertion-order) vs `select_active_plan` (Rust, lexicographic planId) → the UI could show a different active plan than the runner executes. Fix: align the tie-break direction (match Rust `max_by` → greater planId).
- **N-9 ○ RS** ASYNC steer empty-message status diverged (Rust `noop` success vs Python `McpError`). Fix: align (Rust rejects empty too).
- **N-10 ○ RS** folded steer block had a per-message cap but no AGGREGATE cap → 8×2000 = 16KB could bloat the task. Fix: cap the folded block.
- **N-11 ○ PY** redundant `ROLE_ALLOWED_TOOLS` check after `require_agent_tool` already gates it (dead code, false sense of a second layer). Fix: remove the redundant check.
- **N-12 ○ PY** supervise-workflow guidance placed in the role's `forbidden` array (semantically misplaced; LLM could misread). Fix: a dedicated guidance key.
- **N-13 ○ RS (display)** Console diff `ctx_after` read from `old_lines` not `new_lines` (identical by construction, but suffix context can be cut when the cap fires); trailing-newline-only / CRLF diffs render empty (`str::lines()` strips). Cosmetic display gaps on a GPU-deferred feature.

---

## PLAUSIBLE — lower-confidence (kept, flagged)

- **P-1 ⊕ RS** non-ISO-8601 / timezone-variant `updatedAt` strings compared lexicographically → wrong "most recent" plan picked (only via a hand-edited board). Fix: normalize timestamps or document the ISO-UTC assumption.
- **P-2 ⊕ RS** multi-plan coexistence: a `plan_task_control(retry)` on an OLDER plan bumps its recency → the NEXT run may auto-select it over the intended plan. Operational footgun. Fix: an explicit active-plan selection rather than recency.
- **P-3 ○ RS** consequence of B-7 (now fixed): a skip on a `wip` task racing the runner's `set_review` yields a false "blocked" report.

---


## Patterns worth weighting in training (meta — the recurring semantic classes)
1. **Authz that no-ops / fails-open** (B-4, B-5, W-10): an ownership/approval check that returns "allow" on a lookup miss or empty owner. Test the NEGATIVE.
2. **Cross-language mirror drift** (W-2, W-6, W-7, W-9, N-8, N-9, M-6): the same field/logic implemented twice (Py/Rust/TS) and one side weaker/different → data passes the lenient gate, fails the strict one, or churns. No test pins them in sync.
3. **Success/error conflation** (B-2, W-4, W-14, M-2): "nothing to do" / a post-success bookkeeping failure reported as success, or a real failure swallowed.
4. **Client/server deadline mismatch** (W-1, W-11): a wait that's shorter than (or unbounded vs) the work it waits on → false failure or thread starvation.
5. **Live-state mutation without atomicity / stale snapshots** (W-5, W-10, W-15): reading mutable shared state across steps/reads without a consistent snapshot or cancellation.
6. **strict-parse / fail-closed contract holes** (B-1, B-7): a variant/branch that silently bypasses the strict default (serde unit variant; a status mutation skipping a gate).
7. **Stale guards (cleanup-less lifecycle)** (M-4): a guard flag (mounted, ref) without cleanup that persists after unmount → post-unmount side effects execute.
8. **Sync reentrancy (async flags)** (M-5): an async state flag used to guard reentrancy → a second entry occurs before re-render → duplicate side effects.
9. **Config/parsing injection** (M-1, M-7): raw keys/args interpolated into output (TOML, env, command) without escaping; or ambiguous parsing rules that let certain inputs silently misformat.
10. **Reserved-name / tool-name shadowing** (M-3): a list of reserved identifiers that hand-maintainers forget to sync with a downstream source (Oracle tools, etc.) → dispatch/shadowing risk.

---

## Positives — confirmed bugs (real code, before→fix)

Confirmed real bugs found by review this session. Training label = FLAG. Each shows the buggy BEFORE (supplied — not on disk, the fix overwrote it) and the real FIX from disk.

### P-F1 — Whitespace-only oldString inserts at EOF (silent corruption)
`src-tauri/src/backend/mini_coder_executor.rs` (`find_whitespace_span`)

BEFORE: (reconstruct from fix — show the fix only; the guard `if target.is_empty() { return None; }` was ABSENT)

FIX (on disk):
```rust
fn find_whitespace_span(text: &str, old: &str) -> Option<std::ops::Range<usize>> {
    let target = normalize_ws_block(old);
    // A whitespace-only `old` normalizes to "" — and so does the phantom trailing empty
    // line of a `\n`-terminated file. Without this guard Tier 2 would "match" that empty
    // line and return an EMPTY span at EOF, turning the splice into an INSERT of
    // `new_string` at end-of-file (silent corruption: the real target is left untouched).
    // An all-whitespace anchor is never a valid locator, so decline and let Tier 3 (which
    // also cannot confidently match) produce the correct "no confident match" error.
    if target.is_empty() {
        return None;
    }
```

BUG: a whitespace-only oldString normalizes to "" and matches the phantom trailing empty line → an empty span at EOF → replace_range inserts at end-of-file instead of editing the target · FIX: reject an empty normalized target so it falls through to a proper no-match error.

### P-F2 — Overlapping windows defeat the fuzzy ambiguity margin
`src-tauri/src/backend/mini_coder_executor.rs` (`find_fuzzy_span` / `spans_overlap`)

BEFORE: (reconstruct from fix — show the fix only; the second-best was populated via span inequality `span != *best_span` instead of a non-overlap test)

FIX (on disk):
```rust
fn spans_overlap(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}
```

And in `find_fuzzy_span`:
```rust
for first in 0..=(line_count - win) {
    let span = window_span(&starts, text, first, win, ends_nl);
    let ratio = similar::TextDiff::configure()
        .timeout(FUZZY_DIFF_TIMEOUT)
        .diff_chars(&text[span.clone()], old)
        .ratio();
    match best.as_ref() {
        Some((best_ratio, best_span)) if *best_ratio >= ratio => {
            if !spans_overlap(&span, best_span) && second.map(|s| ratio > s).unwrap_or(true)
            {
                second = Some(ratio);
            }
        }
```

BUG: base/base±1 windows over the SAME region were treated as competing locations → a single genuine match was wrongly refused · FIX: only a NON-overlapping span counts toward the ambiguity margin (spans_overlap helper).

### P-F3 — Unbounded fuzzy scan (DoS)
`src-tauri/src/backend/mini_coder_executor.rs` (`find_fuzzy_span`)

BEFORE: (reconstruct from fix — show the fix only; there was no file-size cap and `TextDiff::from_chars(...)` had no timeout)

FIX (on disk):
```rust
fn find_fuzzy_span(text: &str, old: &str) -> Option<(std::ops::Range<usize>, f32)> {
    // DoS guard: never run the O(windows x Myers) fuzzy scan over a large file. Exact and
    // whitespace tiers already ran; a large file needs a precise anchor, not a guess.
    if text.len() > FUZZY_MAX_FILE_BYTES {
        return None;
    }
    let starts = line_start_offsets(text);
    let line_count = starts.len() - 1;
    let (base, ends_nl) = old_block_shape(old);
    if base == 0 || line_count == 0 {
        return None;
    }
    // Candidate window sizes: base, then +/-1 (clamped to >=1 and <= line_count).
    let lo = base.saturating_sub(FUZZY_WINDOW_LINE_DELTA).max(1);
    let hi = (base + FUZZY_WINDOW_LINE_DELTA).min(line_count);
    // Track best and second-best across ALL windows of ALL sizes. `second` only counts
    // a ratio whose SPAN does NOT OVERLAP the current best's span: an overlapping window
    // is the SAME physical region rescored at a different size (base / base+/-1 over the
    // same start line), so letting it feed the margin would make a single genuine match
    // defeat its own ambiguity guard. Only a disjoint region is a competing location.
    let mut best: Option<(f32, std::ops::Range<usize>)> = None;
    let mut second: Option<f32> = None;
    for win in lo..=hi {
        if win > line_count {
            continue;
        }
        for first in 0..=(line_count - win) {
            let span = window_span(&starts, text, first, win, ends_nl);
            let ratio = similar::TextDiff::configure()
                .timeout(FUZZY_DIFF_TIMEOUT)
                .diff_chars(&text[span.clone()], old)
                .ratio();
```

BUG: O(windows × Myers-diff) on a large file blocks the thread · FIX: skip the fuzzy tier above a 256 KiB file cap + a TextDiff timeout.

### P-C1 — SSRF: leading-zero/numeric IPv4 literals bypass the "no IP" rule
`devboule-coder/src/model_client.rs` (`validate_cloud_base_url`)

BEFORE (buggy):
```rust
// only this check rejected IP literals:
if host.parse::<std::net::Ipv4Addr>().is_ok() {
    return Err("Cloud base URL must be a hostname, not an IP literal.".into());
}
// "01.02.03.04" / "010.0.0.1" FAIL Ipv4Addr::parse (leading zeros) -> fall through to the
// hostname/label check -> all labels alphanumeric -> ACCEPTED.
```

FIX (on disk):
```rust
if host.parse::<std::net::Ipv4Addr>().is_ok() {
    return Err("Cloud base URL must be a hostname, not an IP literal.".into());
}
// Rust's `Ipv4Addr` parser REJECTS leading-zero dotted-quads (`01.02.03.04`,
// `010.0.0.1`, `0177.0.0.1`) and out-of-range quads (`999.999.999.999`), so those
// slip past the parse above and look like a hostname (all labels are alphanumeric).
// Reject any host that is exactly 4 dot-separated all-ASCII-digit labels: a numeric
// dotted-quad is always an IP-literal-disguised target, never a real provider host.
let numeric_quad: Vec<&str> = host.split('.').collect();
if numeric_quad.len() == 4
    && numeric_quad
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
{
    return Err("Cloud base URL must be a hostname, not an IP literal.".into());
}
```

BUG: leading-zero/octal dotted-quads slip past the IP-literal reject and are treated as hostnames → SSRF to an internal address via a hand-edited config · FIX: after the parse, reject any host of exactly 4 all-ASCII-digit dot-separated labels.

### P-C2 — TS/Rust validation drift on IP rejection
`src/components/agents/miniCoderBackend.ts` (`isIpv4`)

BEFORE: (reconstruct from fix — show the fix only; the all-numeric-4-label reject `isNumericQuad` was absent, so "999.999.999.999" passed as a hostname)

FIX (on disk):
```typescript
function isIpv4(host: string): boolean {
  const parts = host.split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^[0-9]+$/.test(p) && Number(p) <= 255);
}

// Is `host` exactly 4 dot-separated all-ASCII-digit labels (regardless of leading zeros
// or range)? Mirrors the Rust all-numeric-4-label fallback in `validate_cloud_base_url`:
// it catches IP literals that Rust's strict `Ipv4Addr` parser rejects (`01.02.03.04`,
// `010.0.0.1`, `0177.0.0.1`, `999.999.999.999`) and which `isIpv4` therefore misses.
function isNumericQuad(host: string): boolean {
  const parts = host.split(".");
  return parts.length === 4 && parts.every((p) => /^[0-9]+$/.test(p));
}
```

BUG: the frontend accepted IP-shaped hosts the backend should reject → UI/backend drift on a security validator · FIX: add an all-numeric-4-label reject mirroring Rust.

### P-C3 — API key accepts non-whitespace control chars
`src-tauri/src/backend/vault.rs` (`save_cloud_llm_key`)

BEFORE (buggy):
```rust
if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
    return Err(/* ... */);
}
// no check for other ASCII control chars (\x01, \x7f)
```

FIX (on disk):
```rust
if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
    return Ok(AuxCredentialStatus {
        id: CLOUD_LLM_KEY_ID.into(),
        label: CLOUD_LLM_KEY_LABEL.into(),
        configured: false,
        status: "error".into(),
        last_checked_at: Some(now()),
        message: Some("Cloud API key is too short or contains whitespace.".into()),
    });
}
// Reject other ASCII control characters (`\x01`, `\x7f`, …) the whitespace check above
// misses. reqwest would later refuse such a header value gracefully (no leak, no panic),
// but the user only sees confusing repeated "request failed" with no diagnostic — fail
// LOUD at save time instead. A real bearer key is never control-bearing.
if cleaned.chars().any(|c| c.is_control()) {
    return Ok(AuxCredentialStatus {
        id: CLOUD_LLM_KEY_ID.into(),
        label: CLOUD_LLM_KEY_LABEL.into(),
        configured: false,
        status: "error".into(),
        last_checked_at: Some(now()),
        message: Some("Cloud API key must not contain control characters.".into()),
    });
}
```

BUG: a key with control chars is stored, then fails opaquely in reqwest's header build · FIX: also reject `cleaned.chars().any(|c| c.is_control())` at save time.

### P-C4 — SSRF: metadata/intranet hostnames not blocked
`devboule-coder/src/model_client.rs` (`validate_cloud_base_url`)

BEFORE: (reconstruct from fix — show the fix only; there was no denylist for `metadata.google.internal` / `*.internal` / `*.local`)

FIX (on disk):
```rust
// PARTIAL SSRF mitigation: deny the well-known cloud-metadata FQDN and the conventional
// intranet suffixes `.internal` / `.local`. This is NOT complete SSRF protection —
// COMPLETE protection requires post-DNS-resolution IP filtering (reject RFC1918 /
// link-local / loopback RESOLVED IPs) in the HTTP client's connect layer (a custom
// reqwest resolver). That is a deliberate follow-up and is intentionally NOT done here.
if host_lower == "metadata.google.internal"
    || host_lower.ends_with(".internal")
    || host_lower.ends_with(".local")
{
    return Err("Cloud base URL host must be a public provider host, not an intranet/metadata name.".into());
}
```

BUG: FQDN-shaped metadata hosts passed validation → the cloud request body could exfiltrate instance credentials · FIX: exact+suffix denylist (partial; full SSRF = post-DNS-resolution IP filter, deferred).

### P-C5 — Passive cloud consent (no acknowledgment gate)
`src/components/settings/LocalCoderBackendCard.tsx`

BEFORE: (reconstruct from fix — show the fix only; Save was gated only on key presence, the consent was a passive paragraph with no checkbox)

FIX (on disk):
```typescript
// checkbox is the explicit acknowledgement that content LEAVES the machine. Save is gated on
// it for the cloud kind so a user cannot enable Cloud by passively ignoring the disclosure.
// Reset to false whenever the kind is not "cloud" (mount, current-load, or a switch away)
// so re-entering Cloud always re-requires a fresh acknowledgement.
const [cloudConsentAck, setCloudConsentAck] = useState(false);
useEffect(() => {
  if (kind !== "cloud") setCloudConsentAck(false);
}, [kind]);
```

And the save is gated:
```typescript
disabled={kind === "cloud" && (!hasCloudKey || !cloudConsentAck)}
```

BUG: the user could enable cloud egress without actively acknowledging it · FIX: a consent checkbox (`cloudConsentAck`) that must be ticked to enable Save for the cloud kind, reset on kind change.

### P-C6 — Empty model id to agent_register in cloud mode
`devboule-coder/src/config.rs` (`resolve_register_model`)

BEFORE (buggy):
```rust
model: std::env::var(ENV_OMLX_MODEL).unwrap_or_default(),
// empty in cloud mode -> empty model string registered
```

FIX (on disk):
```rust
fn resolve_register_model() -> String {
    env_nonempty(ENV_OMLX_MODEL)
        .or_else(|| env_nonempty(ENV_CLOUD_MODEL))
        .unwrap_or_default()
}
```

BUG: the model id sent to the Oracle agent_register is empty in cloud mode · FIX: fall back to ENV_CLOUD_MODEL (`env_nonempty(ENV_OMLX_MODEL).or_else(|| env_nonempty(ENV_CLOUD_MODEL)).unwrap_or_default()`).

### P-A1 — Config injection via env KEY
`src-tauri/src/backend/projects.rs` (`codex_user_server_config_settings`)

BEFORE (buggy):
```rust
for (key, value) in &server.env {
    settings.push(format!("mcp_servers.{name}.env.{key}={}", toml_string(value)));
}
// the VALUE is toml-escaped but the KEY is interpolated RAW into the dotted TOML key path
```

FIX (on disk):
```rust
fn codex_user_server_config_settings(server: &user_mcp_config::UserMcpServer) -> Vec<String> {
    let name = &server.name;
    // `command` is always emitted. `args` is emitted ONLY when non-empty — matching the
    // Oracle tokens (which never emit an empty `args=[]`) and keeping the launch line
    // smaller; codex defaults a missing `args` to no arguments, same as `args=[]`.
    let mut settings = vec![format!(
        "mcp_servers.{name}.command={}",
        toml_string(&server.command)
    )];
    if !server.args.is_empty() {
        let arg_refs: Vec<&str> = server.args.iter().map(|s| s.as_str()).collect();
        settings.push(format!("mcp_servers.{name}.args={}", toml_array(&arg_refs)));
    }
    // env keys come from the (deterministically-ordered) BTreeMap so the token order is stable.
    for (key, value) in &server.env {
        settings.push(format!(
            "mcp_servers.{name}.env.{key}={}",
            toml_string(value)
        ));
    }
```

And the validation:
```rust
fn validate_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        if key.is_empty() {
            return Err("environment variable name must not be empty".to_string());
        }
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "environment variable name '{key}' may only contain ASCII letters, digits and '_'"
            ));
        }
        // Mirror validate_args: control chars (including \r and \n) are never
        // legitimate in an env value and indicate a hand-edited hostile payload.
        if value.chars().any(|c| c.is_control()) {
            return Err(format!(
                "environment variable '{key}' value must not contain control characters or newlines"
            ));
        }
    }
    Ok(())
}
```

BUG: an env key containing `.`/`=`/newline mis-nests or injects codex config (exploitable via a committed config) · FIX: validate env keys to `[A-Za-z0-9_]` (and reject control chars in args) in validate_env/validate_server.

### P-A2 — Claude launches with no MCP on serialization failure
`src-tauri/src/backend/projects.rs` (`mcp_client_config_json`)

BEFORE (buggy):
```rust
serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": servers })).unwrap_or_default()
// "" on failure -> claude gets --mcp-config "" -> launches with no Oracle/tools
```

FIX (on disk):
```rust
// Worst-case fallback: serializing this `serde_json::Map` of string-keyed JSON values
// effectively never fails, but if it ever did, `.unwrap_or_default()` would yield `""` —
// and claude launched with `--mcp-config ""` starts with NO MCP servers AT ALL (no
// Oracle), silently losing every tool. Fall back to a VALID-JSON empty config instead so
// the worst case is "claude launches degraded (no servers)" rather than a malformed flag
// value. (This builder returns String and threads through several launch-line callers; a
// Result would cascade into a large signature change for a path that cannot realistically
// fail, so the valid-JSON fallback is the proportionate fix.)
serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": servers }))
    .unwrap_or_else(|_| "{\"mcpServers\":{}}".to_string())
```

BUG: a serde failure silently drops ALL MCP (no Oracle) · FIX: fall back to valid JSON `{"mcpServers":{}}`.

### P-A3 — Oracle tool-name reserved-list drift
`src-tauri/src/backend/user_mcp_config.rs` (`ORACLE_TOOL_NAMES`)

BEFORE: (reconstruct from fix — show the fix only; there was no test asserting the hand-maintained list stays in sync with aspis_mcp.py)

FIX (on disk):
```rust
#[test]
fn oracle_tools_drift_test() {
    // `TOOLS` without being added here (the Rust `ORACLE_TOOL_NAMES` list) AND without a
    // reserved prefix, its bare name (e.g. `visual_check`) would be free for a user server
    // to claim and shadow in dispatch. This test reads the authoritative Python source and
    // fails if any registered tool name is neither in the static list nor caught by a prefix.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR (src-tauri) must have a parent (repo root)");
    let py_path = repo_root.join("oracle").join("server").join("aspis_mcp.py");
    let src = std::fs::read_to_string(&py_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", py_path.display()));
    let registered = oracle_tool_names_from_python(&src);
    assert!(
        !registered.is_empty(),
        "parsed ZERO tool names from {} — the parser or the file shape changed",
        py_path.display()
    );
    let mut missing: Vec<String> = Vec::new();
    for tool in &registered {
        let in_list = ORACLE_TOOL_NAMES.iter().any(|t| t.eq_ignore_ascii_case(tool));
        if !in_list && !caught_by_reserved_prefix(tool) {
            missing.push(tool.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "Oracle tool(s) {missing:?} are registered in aspis_mcp.py but NOT in \
         ORACLE_TOOL_NAMES and NOT caught by a reserved prefix — a user server could \
         claim those names and shadow the Oracle. Add them to ORACLE_TOOL_NAMES."
    );
}
```

BUG: a future Oracle tool added without updating the list becomes hijackable as a user-server name · FIX: a drift test parsing aspis_mcp.py TOOLS that fails if any tool isn't covered.

### P-A4 — Dead unmount guard
`src/components/settings/UserMcpConsentDialog.tsx`

BEFORE (buggy):
```tsx
const mountedRef = useRef(true);
// NO cleanup effect -> the guard `if (mountedRef.current)` is permanently true
```

FIX (on disk):
```typescript
// F1: proper cleanup so mountedRef is false after unmount. Without this the
// ref stays true permanently and onAdded() can fire on an unmounted parent
// if the invoke resolves after unmount.
const mountedRef = useRef(true);
useEffect(() => {
  mountedRef.current = true;
  return () => {
    mountedRef.current = false;
  };
}, []);
```

BUG: callbacks fire on the unmounted dialog (stale load) · FIX: add `useEffect(() => { mountedRef.current = true; return () => { mountedRef.current = false; }; }, [])`.

### P-A5 — Double-submit via async useState guard
`src/components/settings/UserMcpConsentDialog.tsx` + `UserMcpServersCard.tsx`

BEFORE: (reconstruct from fix — show the fix only; the reentrancy guard used the async `useState` `busy` flag)

FIX (on disk):
```typescript
// F2: synchronous reentrancy guard. useState `busy` is async — two rapid
// clicks both observe busy===false before the first setState flush. The ref
// is set synchronously at the top of the handler, so the second click is
// dropped immediately. Keep the useState busy for the visual disabled state.
const busyRef = useRef(false);

const nameClean = name.trim();
const commandClean = command.trim();
const canAdd =
  consentAck && nameClean.length > 0 && commandClean.length > 0 && !busy;

// F6: useCallback with parsedArgs/parsedEnv as deps never memoizes (new
// refs each render). The dialog passes onAdd only to a button onClick — no
// memo-sensitive child — so removing useCallback is the cleanest fix.
async function onAdd() {
  if (!canAdd) return;
  // F2: synchronous guard checked before any await.
  if (busyRef.current) return;
  busyRef.current = true;
  setBusy(true);
  setError(null);
  const server: UserMcpServer = {
    name: nameClean,
    transport: "stdio",
    command: commandClean,
    args: parsedArgs,
    env: parsedEnv,
    enabled: true,
  };
  const args: Record<string, unknown> = { scope, server };
  if (scope === "project" && projectRoot) {
    args.projectRoot = projectRoot;
  }
  try {
    await invokeBackendCommand<void>("user_mcp_add", args);
    if (mountedRef.current) onAdded();
  } catch (e) {
    if (mountedRef.current) {
      setError(
        e instanceof Error
          ? e.message
          : "Failed to add the MCP server. Check the name and command and try again.",
      );
      setBusy(false);
    }
  } finally {
    busyRef.current = false;
  }
}
```

BUG: two rapid clicks both see busy===false → duplicate user_mcp_add/set_enabled/remove · FIX: a `useRef(false)` guard checked+set synchronously at the top of each handler.

### P-A6 — CRLF/control chars in env values
`src/components/settings/UserMcpConsentDialog.tsx` (`parseEnv`)

BEFORE (buggy):
```ts
const idx = line.indexOf("=");
const val = line.slice(idx + 1); // no \r strip; lines split on "\n"
```

FIX (on disk):
```typescript
// Parse env lines (K=V). Split on \r?\n so a Windows CRLF paste does not
// store a trailing \r in the value. Lines that don't contain "=" are ignored.
// Returns a record; env VALUES are never shown back to the user (only keys).
function parseEnv(raw: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of raw.split(/\r?\n/)) {
    const idx = line.indexOf("=");
    if (idx <= 0) continue; // skip blank, malformed, or valueless lines
    const key = line.slice(0, idx).trim();
    // Strip a stray trailing \r from the value (defence: split above already
    // handles \r\n, but a bare \r at the end of a value is equally wrong).
    const val = line.slice(idx + 1).replace(/\r$/, "");
    if (key) result[key] = val;
  }
  return result;
}
```

BUG: a Windows-pasted `KEY=val\r` stores `val\r` → corrupt child env (and the backend validate_env checked only KEYS) · FIX: split on `/\r?\n/` + strip trailing `\r`; backend rejects control chars in env VALUES too.

### P-A7 — parseArgs mixed-mode silently wrong
`src/components/settings/UserMcpConsentDialog.tsx` (`parseArgs`)

BEFORE: (reconstruct from fix — show the fix only; it split on "\n" if a newline was present, else on ",")

FIX (on disk):
```typescript
function parseArgs(raw: string): string[] {
  return raw.split(/[\n,]/).map((p) => p.trim()).filter(Boolean);
}
```

BUG: `-m\nmydb,--debug` → `["-m","mydb,--debug"]` (wrong) so the server fails to start · FIX: split on BOTH delimiters `/[\n,]/`.

---

### P-B1 — user-MCP egress conflated with web egress
`devboule-coder/src/action.rs` (`is_egress`)

FIX (on disk):
```rust
pub fn is_egress(&self) -> bool {
    matches!(
        self,
        AgentAction::Fetch { .. } | AgentAction::Websearch { .. }
    )
}
```

BUG: McpTool returned `is_egress()=true`, so the gate `is_egress() && !allow_egress` blocked user-MCP whenever there was no Exa web key — even a purely local tool was unusable · FIX: decouple — McpTool is not web-egress; it is gated only by being a known configured server.

### P-B2 — Serial connect of user servers (startup hang)
`devboule-coder/src/multi_mcp.rs` (`connect`)

FIX (on disk):
```rust
let connects = specs.into_iter().map(|spec| async move {
    match RmcpBackend::connect_generic(&spec.command, &spec.args, &spec.env).await {
        Ok(backend) => {
            // Fetch the tool catalog WHILE we still hold the concrete backend
            // (the trait object does not expose list_tools). A failure here is
            // non-fatal: the server is still wired; it just lists no tools.
            let tools = match backend.list_tools().await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!(
                        "devboule: user MCP server '{}' list_tools failed ({e}); \
                         wired with no advertised tools",
                        spec.name
                    );
                    Vec::new()
                }
            };
            Some((spec.name, Arc::new(backend) as Arc<dyn McpBackend>, tools))
        }
        Err(e) => {
            eprintln!(
                "devboule: user MCP server '{}' failed to connect ({e}); skipping",
                spec.name
            );
            None
        }
    }
});

let results = match tokio::time::timeout(
    Self::CONNECT_PHASE_DEADLINE,
    futures::future::join_all(connects),
)
.await
{
    Ok(results) => results,
    Err(_) => {
        eprintln!(
            "devboule: user MCP servers did not all connect within {}s; \
             proceeding without the user servers (Oracle still serves)",
            Self::CONNECT_PHASE_DEADLINE.as_secs()
        );
        Vec::new()
    }
};
```

BUG: user servers were connected in a serial loop (each awaiting connect+list_tools) → up to N×~60s of blocked startup that also stalled the Oracle/burst; one hanging server froze everything · FIX: connect concurrently under a startup deadline; failed/timed-out servers are logged and skipped.

### P-B3 — `command` has no content validation
`src-tauri/src/backend/user_mcp_config.rs` (`validate_command`)

FIX (on disk):
```rust
fn validate_command(command: &str) -> Result<(), String> {
    if command.chars().any(|c| c.is_control()) {
        return Err("server command must not contain control characters or newlines".to_string());
    }
    Ok(())
}
```

BUG: `command` was only checked for emptiness (unlike args/env which rejected control chars), so a committed config's command string was spawned verbatim · FIX: reject control chars/newlines in `command` too.

### P-B4 — Serializer emits all servers (no cap)
`src-tauri/src/backend/user_mcp_config.rs` (`orchestrator_env_json`)

FIX (on disk):
```rust
pub(crate) fn orchestrator_env_json(servers: &[UserMcpServer]) -> String {
    if servers.is_empty() {
        return String::new();
    }
    // Cap the emitted set at exactly what the binary consumes (MAX_ORCHESTRATOR_SERVERS,
    // matching the binary's MAX_USER_MCP_SERVERS): emitting more would only bloat the env
    // value toward the OS E2BIG limit and be dropped by the binary regardless. Truncate
    // (never fail the launch) and log so an oversized list is visible.
    if servers.len() > MAX_ORCHESTRATOR_SERVERS {
        eprintln!(
            "[user-mcp] {} enabled servers exceeds the {MAX_ORCHESTRATOR_SERVERS}-server \
             launch cap; emitting only the first {MAX_ORCHESTRATOR_SERVERS}",
            servers.len()
        );
    }
    let payload: Vec<OrchestratorServerPayload<'_>> = servers
        .iter()
        .take(MAX_ORCHESTRATOR_SERVERS)
        .map(|s| OrchestratorServerPayload {
            name: &s.name,
            command: &s.command,
            args: &s.args,
            env: &s.env,
        })
        .collect();
```

BUG: it serialized ALL enabled servers while the consumer caps at 20 → a huge `DEVBOULE_USER_MCP_SERVERS` env value could exceed exec limits (E2BIG) and break the launch · FIX: cap the serializer at the same limit + log truncation.

### P-B5 — Binary doesn't re-guard Oracle/reserved names
`devboule-coder/src/config.rs` (`user_server_name_ok`)

FIX (on disk):
```rust
fn user_server_name_ok(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty name".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name has characters outside [A-Za-z0-9_-]".to_string());
    }
    let lower = name.to_ascii_lowercase();
    if let Some(prefix) = RESERVED_NAME_PREFIXES
        .iter()
        .find(|p| lower.starts_with(**p))
    {
        return Err(format!("reserved name prefix '{prefix}'"));
    }
    if ORACLE_TOOL_NAMES.iter().any(|t| t.eq_ignore_ascii_case(name)) {
        return Err("collides with a reserved Oracle tool name".to_string());
    }
    Ok(())
}
```

BUG: the binary accepted any non-empty server name from `DEVBOULE_USER_MCP_SERVERS`, so a crafted env with `name:"oracle_ask"` registered a user backend under an Oracle name · FIX: re-reject reserved/Oracle names + the charset in the binary (don't trust the env).

### P-B6 — Mini inherits DEVBOULE_USER_MCP_SERVERS from host env
`src-tauri/src/backend/mini_coder_executor.rs` (`build_mini_command_impl`)

FIX (on disk):
```rust
cmd.cwd(project_root);
// MINI-EXCLUSION (design §6): scrub the orchestrator-only user-MCP env var so the mini
// child can NEVER inherit it from the host process env (CommandBuilder snapshots it).
cmd.env_remove(FORBIDDEN_USER_MCP_ENV);
```

BUG: `CommandBuilder::new()` snapshots the host process env, so if the app was launched from a shell with the var set the mini inherited it (violating mini-exclusion) · FIX: `env_remove("DEVBOULE_USER_MCP_SERVERS")` in the mini spawn — runtime-enforced exclusion.

### P-B7 — Prompt sanitizer misses unicode/alt-fence
`devboule-coder/src/prompt.rs` (`sanitize_metadata`)

FIX (on disk):
```rust
fn sanitize_metadata(value: &str) -> String {
    value
        .replace(
            ['\n', '\r', '\u{2028}', '\u{2029}', '\u{0085}', '\u{000B}', '\u{000C}'],
            " ",
        )
        .replace("```", "`\u{200b}``")
        .replace("~~~", "~\u{200b}~~")
}
```

BUG: it collapsed only `\n`/`\r` and neutralized only triple-backtick, so U+2028/U+2029 line separators and the `~~~` alternate fence could escape the untrusted external-tools block (prompt injection) · FIX: also collapse unicode line/para separators and neutralize `~~~`.

### P-B8 — No kill_on_drop + unbounded list_tools
`devboule-coder/src/rmcp_backend.rs` (`connect_generic`, `list_tools`)

FIX (on disk):
```rust
let cmd = Command::new(command).configure(|c| {
    for a in args {
        c.arg(a);
    }
    // HARDENING: reap the user-server child if THIS process exits abnormally (a
    // panic / abort that skips `Drop`). `kill_on_drop` makes tokio send SIGKILL when
    // the `Child` handle drops, so a semi-untrusted user server can never outlive us
    // as an orphan. The normal teardown path is still the transport's cancellation
    // token in `Drop` (below); this is the belt-and-suspenders for the abnormal path.
    c.kill_on_drop(true);
```

```rust
pub async fn list_tools(&self) -> Result<Vec<(String, Option<String>)>, String> {
    let fut = self.service.peer().list_all_tools();
    match timeout(CONNECT_TIMEOUT, fut).await {
        Ok(Ok(tools)) => Ok(tools
            .into_iter()
            // Cap the COUNT first so we never even materialize an unbounded list.
            .take(MAX_TOOLS_PER_SERVER)
            // Drop a pathologically long tool NAME (it is the routing key; an absurd
            // length is never a real tool and would only bloat the prompt).
            .filter(|t| t.name.chars().count() <= MAX_TOOL_NAME_LEN)
            .map(|t| {
                let desc = t.description.map(|d| truncate_chars(&d, MAX_TOOL_DESC_LEN));
                (t.name.to_string(), desc)
            })
            .collect()),
        Ok(Err(e)) => Err(format!("list_tools failed: {e}")),
        Err(_) => Err(format!(
            "list_tools timed out after {}s",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}
```

BUG: the spawned user-server child had no kill_on_drop (orphan on abnormal parent exit) and list_tools was uncapped (a hostile server's huge catalog bloats the prompt/memory) · FIX: `kill_on_drop(true)` + cap tools-per-server and truncate descriptions.

---

## Negatives — suspicious-but-correct (false positives, for FPR/calibration)

Negative training pairs: real code that looks suspicious but is correct. Label = DO NOT FLAG.

**N1 — UTF-8 char-boundary safety in the fuzzy splice**
`src-tauri/src/backend/mini_coder_executor.rs` (`line_start_offsets` + `window_span` + `apply_emitted_edits` splice site)
```rust
fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts.push(text.len());
    starts
}
```
```rust
// in apply_emitted_edits — the splice:
let (span, tier) = locate_edit_span(text, &edit.old_string)
    .map_err(|e| format!("edit {i}: {e} in {rel}"))?;
text.replace_range(span, &edit.new_string);
```
NON-BUG: every splice offset comes from a line start (the byte after `\n`, a single-byte codepoint) or `str::find` (char-boundary by contract), so `replace_range` can never land mid-codepoint.

**N2a — Cloud key not in config.json**
`src-tauri/src/backend/local_coder.rs` (`LocalCoderBackend` struct)
```rust
pub struct LocalCoderBackend {
    pub kind: LocalCoderBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The CLOUD API KEY is deliberately NOT a field here — it never touches config.json.
    /// It lives ONLY in the OS vault and is read at launch into `DEVBOULE_CLOUD_API_KEY` (env).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}
```
NON-BUG: the key is not a field of the serialized backend struct; it lives only in the OS vault.

**N2b — Cloud key not on the launch line**
`src-tauri/src/backend/projects.rs` (`orchestrator_env_pairs` + cloud key injection into `provider_env`)
```rust
fn orchestrator_env_pairs(config: &OrchestratorLaunchConfig) -> Vec<(&'static str, String)> {
    let mut pairs: Vec<(&'static str, String)> = vec![
        ("DEVBOULE_OMLX_BASE_URL", config.omlx_base_url.to_string()),
        // ... other non-secret vars ...
    ];
    // CLOUD (opt-in) NON-SECRET vars only.
    // The cloud API KEY is NEVER here — it rides via `provider_env`
    // (DEVBOULE_CLOUD_API_KEY), off argv (B1 invariant).
    if !config.cloud_base_url.trim().is_empty() {
        pairs.push(("DEVBOULE_CLOUD_BASE_URL", config.cloud_base_url.clone()));
        pairs.push(("DEVBOULE_CLOUD_MODEL", config.cloud_model.clone()));
    }
    pairs
}
```
```rust
// key pushed into provider_env (process env), never into the rendered script:
if !cloud_base_url.trim().is_empty() {
    if let Some(cloud_key) = vault::read_cloud_llm_key()? {
        provider_env.push(AgentLaunchEnv {
            name: "DEVBOULE_CLOUD_API_KEY".into(),
            value: cloud_key,
        });
    }
}
```
NON-BUG: the key rides the process env (`provider_env`), never the rendered launch script (`orchestrator_env_pairs`).

**N2c — Errors don't log the key**
`devboule-coder/src/model_client.rs` (`run_completion` error path) + `devboule-coder/src/config.rs` (build_model cloud error)
```rust
// run_completion — HTTP error arm:
if !status.is_success() {
    // Status code only — never the body, which a provider may echo the prompt
    // or key fragment into.
    return Err(format!("HTTP {}", status.as_u16()));
}
```
```rust
// config.rs build_model — cloud error arm:
Err(e) => {
    // `e` is a validation message (scheme/host/empty-field); it NEVER
    // contains the key value.
    eprintln!("devboule: Cloud model disabled ({e}); using MockModel{plan_note}");
    return Arc::new(MockModel::new());
}
```
NON-BUG: run errors print only the HTTP status, build errors only the validation message; neither interpolates the key.

**N2d — Debug redacts the key**
`devboule-coder/src/model_client.rs` (custom `impl fmt::Debug for CloudModel`)
```rust
impl std::fmt::Debug for CloudModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the key so a `{:?}` of the model (or anything holding it) can never
        // print the credential to a log/diagnostic.
        f.debug_struct("CloudModel")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .field("plan_first", &self.plan_first)
            .finish()
    }
}
```
NON-BUG: the custom Debug impl prints `<redacted>` for the key.

**N2e — No unauthenticated request when key empty**
`devboule-coder/src/model_client.rs` (`CloudModel::new` empty-key check)
```rust
pub fn new(
    base_url: &str,
    model: impl Into<String>,
    api_key: impl Into<String>,
    plan_first: bool,
) -> Result<Self, String> {
    let base_url = validate_cloud_base_url(base_url)?;
    let model = model.into();
    if model.trim().is_empty() {
        return Err("Cloud model id must not be empty.".into());
    }
    let api_key = api_key.into();
    if api_key.trim().is_empty() {
        return Err("Cloud API key must not be empty.".into());
    }
```
NON-BUG: `CloudModel::new` rejects an empty key and falls back to Mock, so no request is sent without auth.

**N2f — Local mode never reads the key**
`src-tauri/src/backend/projects.rs` (`cloud_base_url` gate before `read_cloud_llm_key`)
```rust
if !cloud_base_url.trim().is_empty() {
    if let Some(cloud_key) = vault::read_cloud_llm_key()? {
        provider_env.push(AgentLaunchEnv {
            name: "DEVBOULE_CLOUD_API_KEY".into(),
            value: cloud_key,
        });
    }
}
```
NON-BUG: the key is read+injected only when `cloud_base_url` is non-empty; local kinds resolve it empty, so the key is never read for a local launch.

**N2g — bearer_auth Err not panic**
`devboule-coder/src/model_client.rs` (`run_completion` request build)
```rust
async fn run_completion(&self, transcript: &Transcript) -> Result<String, String> {
    let body = self.build_request_body(transcript);
    let resp = self
        .client
        .post(self.endpoint())
        .bearer_auth(&self.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
```
NON-BUG: an invalid header value returns a handled `Err` (not a panic) whose message omits the key.

**N2h — key_status has no value field**
`src-tauri/src/backend/vault.rs` (`cloud_llm_key_status`) + `src-tauri/src/backend/model.rs` (`AuxCredentialStatus` type)
```rust
pub struct AuxCredentialStatus {
    pub id: String,
    pub label: String,
    pub configured: bool,
    pub status: String,
    pub last_checked_at: Option<String>,
    pub message: Option<String>,
}
```
```rust
pub fn cloud_llm_key_status() -> Result<AuxCredentialStatus, String> {
    match read_cloud_llm_key() {
        Ok(Some(_)) => Ok(AuxCredentialStatus {
            configured: true,
            status: "configured".into(),
            // ... id/label/timestamp — no value field
        }),
        Ok(None) => Ok(AuxCredentialStatus { configured: false, status: "missing".into(), .. }),
        Err(e) => Ok(AuxCredentialStatus { configured: false, status: "error".into(), .. }),
    }
}
```
NON-BUG: the status type has no value field; it reports present/absent only.

**N2i — CRLF rejected at save**
`src-tauri/src/backend/vault.rs` (`save_cloud_llm_key` whitespace rejection)
```rust
pub fn save_cloud_llm_key(key: &str) -> Result<AuxCredentialStatus, String> {
    let cleaned = key.trim();
    // Same minimum-length + no-whitespace guard the Exa key uses: a too-short /
    // whitespace-bearing value is a paste error, not a real bearer key.
    if cleaned.len() < 8 || cleaned.contains(char::is_whitespace) {
        return Ok(AuxCredentialStatus {
            configured: false,
            status: "error".into(),
            message: Some("Cloud API key is too short or contains whitespace.".into()),
            ..
        });
    }
    // Reject other ASCII control characters (`\x01`, `\x7f`, …) the whitespace check above misses.
    if cleaned.chars().any(|c| c.is_control()) {
        return Ok(AuxCredentialStatus {
            configured: false,
            status: "error".into(),
            message: Some("Cloud API key must not contain control characters.".into()),
            ..
        });
    }
```
NON-BUG: the save rejects whitespace (incl. `\r\n`) and control chars before storing.

**N2j — Local/loopback path untouched**
`devboule-coder/src/model_client.rs` (`validate_omlx_base_url`) + `src-tauri/src/backend/local_coder.rs` (ollama/omlx arms)
```rust
pub fn validate_omlx_base_url(base_url: &str) -> Result<String, String> {
    // ...
    // Scheme: http only (loopback, like Ollama). `https://` is rejected.
    let rest = match trimmed.strip_prefix("http://") {
        Some(r) => r,
        None => return Err("oMLX base URL must start with http:// (loopback, http only).".into()),
    };
    // [loopback validation continues — unchanged by cloud addition]
}
```
```rust
// omlx arm of validate_local_coder_backend (unchanged):
LocalCoderBackendKind::Omlx => {
    // ...
    let normalized_base = validate_omlx_base_url(&base_url)?;
    // ... no cloud path involved
}
```
NON-BUG: the loopback validator and the ollama/omlx arms are unchanged; cloud is a separate validator/path.

**N3a — Add double-locked**
`src/components/settings/UserMcpConsentDialog.tsx` (Add button `disabled` + `onAdd` guard)
```tsx
const canAdd =
  consentAck && nameClean.length > 0 && commandClean.length > 0 && !busy;

async function onAdd() {
  if (!canAdd) return;
  // F2: synchronous guard checked before any await.
  if (busyRef.current) return;
  busyRef.current = true;
  setBusy(true);
```
NON-BUG: Add is double-locked (disabled attribute AND an early-return guard).

**N3b — No form**
`src/components/settings/UserMcpConsentDialog.tsx` (top-level container element)
```tsx
return (
  <div
    role="dialog"
    aria-modal="true"
    aria-label="Add MCP server"
    className="fixed inset-0 z-50 flex items-center justify-center bg-cream-900/40 p-4"
  >
    <div className="w-full max-w-lg rounded-2xl border border-cream-200 bg-white shadow-xl">
      {/* Header */}
```
NON-BUG: there is no `<form>`, so Enter does not submit.

**N3c — Unmounts on close**
`src/components/settings/UserMcpServersCard.tsx` (conditional render of `UserMcpConsentDialog`)
```tsx
{showDialog && (
  <UserMcpConsentDialog
    scope={scope}
    projectRoot={projectRoot}
    onAdded={() => void onAdded()}
    onCancel={() => setShowDialog(false)}
  />
)}
```
NON-BUG: the dialog unmounts on close, so consent state is fresh on every reopen.

**N3d — Cancel writes nothing**
`src/components/settings/UserMcpConsentDialog.tsx` (Cancel and X button onClick)
```tsx
// X button:
<button type="button" onClick={onCancel} disabled={busy} aria-label="Cancel">
  <X className="h-4 w-4" />
</button>

// Cancel button:
<button type="button" onClick={onCancel} disabled={busy}>
  Cancel
</button>
```
NON-BUG: Cancel/X call only `onCancel`; no backend command runs.

**N3e — Only env keys rendered**
`src/components/settings/UserMcpConsentDialog.tsx` (review block env rendering)
```tsx
const envKeys = Object.keys(parsedEnv);
// ...
{envKeys.length > 0 && (
  <div className="flex gap-2">
    <dt className="shrink-0 text-cream-400">env keys</dt>
    <dd className="min-w-0 break-all">
      {envKeys.join(", ")}{" "}
      <span className="text-cream-400">(values redacted)</span>
    </dd>
  </div>
)}
```
NON-BUG: only `Object.keys(env)` is rendered; values never reach the DOM.

**N3f — Oracle excluded from list**
`src/components/settings/UserMcpServersCard.tsx` (list render mapping backend result)
```tsx
const [servers, setServers] = useState<UserMcpServer[]>([]);
// ...
const result = await invokeBackendCommand<UserMcpServer[]>("user_mcp_list", baseArgs());
setServers(Array.isArray(result) ? result : []);
// ...
{servers.map((server) => (
  <li key={server.name} data-testid={`server-row-${server.name}`} ...>
```
NON-BUG: the list renders the backend result, which already excludes the Oracle.

**N3g — loadSeqRef guard**
`src/components/settings/UserMcpServersCard.tsx` (`load` function with sequence guard)
```tsx
const load = useCallback(async () => {
  const seq = loadSeqRef.current + 1;
  loadSeqRef.current = seq;
  setLoading(true);
  try {
    const result = await invokeBackendCommand<UserMcpServer[]>("user_mcp_list", baseArgs());
    if (!mountedRef.current || loadSeqRef.current !== seq) return;
    setServers(Array.isArray(result) ? result : []);
  } catch (e) {
    if (!mountedRef.current || loadSeqRef.current !== seq) return;
    setServers([]);
  } finally {
    if (mountedRef.current && loadSeqRef.current === seq) setLoading(false);
  }
}, [baseArgs]);
```
NON-BUG: a sequence ref increments before the await; stale responses are discarded.

**N3h — projectRoot threaded**
`src/components/settings/UserMcpServersCard.tsx` (`baseArgs()`)
```tsx
const baseArgs = useCallback((): Record<string, unknown> => {
  const a: Record<string, unknown> = { scope };
  if (scope === "project" && projectRoot) a.projectRoot = projectRoot;
  return a;
}, [scope, projectRoot]);
```
NON-BUG: `baseArgs` threads `projectRoot` to every command.

**N3i — Dock tabs consistent**
`src/components/projects/projectWorkspaceModel.ts` (`DOCK_TABS` array)
```typescript
export const DOCK_TABS: { id: DockTab; label: string }[] = [
  { id: "censor", label: "Censor" },
  { id: "activity", label: "Activity" },
  { id: "git", label: "Git" },
  { id: "plans", label: "Plans" },
  { id: "console", label: "Console" },
  // Project-scoped user MCP servers (Phase A.3).
  { id: "mcp", label: "MCP" },
];
```
NON-BUG: `DOCK_TABS` is extended consistently and the count test matches.

**N3j — No stale closure on onAdd**
`src/components/settings/UserMcpConsentDialog.tsx` (`parsedArgs`/`parsedEnv` + `onAdd`)
```tsx
const parsedArgs = parseArgs(argsRaw);
const parsedEnv = parseEnv(envRaw);
const envKeys = Object.keys(parsedEnv);
// ...
// F6: useCallback with parsedArgs/parsedEnv as deps never memoizes (new
// refs each render). onAdd is a plain async function reading them directly.
async function onAdd() {
  if (!canAdd) return;
  if (busyRef.current) return;
  busyRef.current = true;
  setBusy(true);
  const server: UserMcpServer = {
    name: nameClean,
    command: commandClean,
    args: parsedArgs,   // latest value, read directly
    env: parsedEnv,     // latest value, read directly
    // ...
  };
```
NON-BUG: parsed args/env are new refs each render and are read directly in `onAdd`, so the latest values are captured.

**N3k — busy moot on unmount**
`src/components/settings/UserMcpConsentDialog.tsx` (`onAdd` success path)
```tsx
async function onAdd() {
  // ...
  try {
    await invokeBackendCommand<void>("user_mcp_add", args);
    if (mountedRef.current) onAdded();   // → parent sets showDialog=false → unmount
  } catch (e) {
    // ...
  }
}
```
NON-BUG: on success the dialog unmounts, so a latched `busy=true` is moot.

**N4 — codex value injection safe**
`src-tauri/src/backend/projects.rs` (`codex_user_server_config_settings` + `sh_single_quote` wrapping)
```rust
fn codex_user_server_config_settings(server: &user_mcp_config::UserMcpServer) -> Vec<String> {
    let name = &server.name;
    let mut settings = vec![format!(
        "mcp_servers.{name}.command={}",
        toml_string(&server.command)   // VALUE goes through toml_string
    )];
    for (key, value) in &server.env {
        settings.push(format!(
            "mcp_servers.{name}.env.{key}={}",
            toml_string(value)         // VALUE goes through toml_string
        ));
    }
    settings
}
```
```rust
// the whole -c <setting> token is single-quoted for the shell:
for config in &config_args {
    line.push_str(" -c ");
    line.push_str(&sh_single_quote(config));
}
```
NON-BUG: codex values go through `toml_string` and the whole `-c` arg is single-quoted for the shell (the env-KEY path was the real bug, fixed separately).

**N5 — disjoint dispatch**
`devboule-coder/src/multi_mcp.rs` (`call_tool` vs `call_user_tool`)
```rust
async fn call_tool(&self, name: &str, params: serde_json::Value) -> Result<String, String> {
    self.oracle.call_tool(name, params).await
}

/// Route a user-MCP call to the named backend.
async fn call_user_tool(
    &self,
    server: &str,
    tool: &str,
    params: serde_json::Value,
) -> Result<String, String> {
    match self.user.iter().find(|(name, _)| name == server) {
        Some((_, backend)) => backend.call_tool(tool, params).await,
        None => Err(format!("unknown user MCP server `{server}`")),
    }
}
```
NON-BUG: Oracle and user dispatch are disjoint trait methods.

**N6 — child env isolation**
`devboule-coder/src/rmcp_backend.rs` (`connect_generic` `env_clear()` + allowlist re-add)
```rust
const SYSTEM_ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "LANG", "LC_ALL", "TZ",
    "SYSTEMROOT", "SystemRoot", "SystemDrive", "TEMP", "TMP", "PATHEXT", "WINDIR",
];
let cmd = Command::new(command).configure(|c| {
    // SECURITY: drop the inherited (secret-bearing) orchestrator env entirely…
    c.env_clear();
    // …re-add only the system baseline that is actually set in our env…
    for key in SYSTEM_ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(key) {
            c.env(key, val);
        }
    }
    // …then the user's OWN declared env (wins over a same-named baseline key).
    for (k, v) in env {
        c.env(k, v);
    }
});
```
NON-BUG: `connect_generic` `env_clear()`s before re-adding a minimal allowlist; no Devboule secret matches it.

**N7 — dropped Eq harmless**
`devboule-coder/src/action.rs` (`AgentAction` derive line)
```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
```
NON-BUG: nothing uses `Eq`/`Hash` on these types; the loop-detector uses `(String,String)` tuples and `u64` hashes.

**N8 — params in target intentional**
`devboule-coder/src/action.rs` (`target()` arm for `McpTool`)
```rust
AgentAction::McpTool {
    server,
    tool,
    params,
} => format!("{server}.{tool} {params}"),
```
NON-BUG: including params is intentional (paginated tools should advance); `MAX_ROUNDS` is the backstop.

**N9 — fence parsing robust**
`devboule-coder/src/action.rs` (`fence_info` inside `count_action_fences`)
```rust
fn count_action_fences(input: &str) -> (usize, usize) {
    fn fence_info(line: &str) -> Option<&str> {
        let t = line.trim_end_matches(['\r', ' ', '\t']);
        t.strip_prefix("```")
            .map(|rest| rest.split([' ', '\t']).next().unwrap_or(""))
    }
    let mut inside_fence = false;
    let mut top_level = 0usize;
    let mut total = 0usize;
    for line in input.lines() {
        let Some(info) = fence_info(line) else { continue };
        if inside_fence {
            if info.is_empty() { inside_fence = false; }
            else if info == "action" { total += 1; }
        } else {
            if info == "action" { top_level += 1; total += 1; }
            // non-action opener → inside_fence = true (skipped for brevity)
        }
    }
```
NON-BUG: `fence_info` strips exactly three backticks and the nested-block rejection catches alternate/nested fences.

**N10 — claude config serde-safe**
`src-tauri/src/backend/projects.rs` (`mcp_client_config_json` serde_json build)
```rust
for server in user_servers {
    if servers.contains_key(&server.name) { continue; }
    let env: serde_json::Map<String, serde_json::Value> = server
        .env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    servers.insert(
        server.name.clone(),
        serde_json::json!({
            "command": server.command,
            "args": server.args,
            "env": env,
        }),
    );
}
```
NON-BUG: the claude config is built with `serde_json`; all values are JSON-encoded.

**N11 — path traversal rejected**
`src-tauri/src/backend/user_mcp_config.rs` (`canonical_project_root`)
```rust
fn canonical_project_root(project_root: &str) -> Result<PathBuf, String> {
    if project_root.trim().is_empty() {
        return Err("project root path must not be empty".to_string());
    }
    let raw = PathBuf::from(project_root);
    if raw
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("project root path must not contain '..'".to_string());
    }
    let canonical = std::fs::canonicalize(&raw).map_err(|e| { ... })?;
    // containment assert follows ...
```
NON-BUG: it rejects `..` then canonicalizes + containment-asserts.

**N12 — unicode name rejected first**
`src-tauri/src/backend/user_mcp_config.rs` (`validate_name` charset check before reserved check)
```rust
fn validate_name(name: &str) -> Result<(), String> {
    // ...length checks...
    // CONFIG-INJECTION GUARD: restrict to safe charset FIRST (ASCII alphanumeric, `-`, `_`)
    // so non-ASCII is rejected before the reserved-prefix / Oracle-name check.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "server name '{name}' may only contain ASCII letters, digits, '-' and '_'"
        ));
    }
    let lower = name.to_ascii_lowercase();
    for prefix in RESERVED_NAME_PREFIXES {
        if lower.starts_with(prefix) {
            return Err(format!("server name '{name}' is reserved ..."));
        }
```
NON-BUG: the ASCII charset check runs before the reserved/Oracle check, so non-ASCII is rejected first.

**N13 — TOCTOU not exploitable**
`src-tauri/src/backend/user_mcp_config.rs` (`read_config_file` — the metadata-then-open pattern)
```rust
fn read_config_file(path: &Path) -> UserMcpConfig {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => { return UserMcpConfig::default(); }
        Err(_) => return UserMcpConfig::default(),
    }
    let mut handle = match std::fs::File::open(path) {
        Ok(f) => f.take(MAX_CONFIG_BYTES + 1),
        Err(e) => { return UserMcpConfig::default(); }
    };
    // ...read and parse...
}
```
NON-BUG: exploiting the read race needs local write access — strictly weaker than poisoning the JSON directly.

**N14 — byte-identical when empty**
`src-tauri/src/backend/user_mcp_config.rs` (`orchestrator_env_json` empty path) + `devboule-coder/src/prompt.rs` (`build_system_prompt` empty-slice path)
```rust
pub(crate) fn orchestrator_env_json(servers: &[UserMcpServer]) -> String {
    if servers.is_empty() {
        return String::new();   // omitted entirely — not an empty string value in the env
    }
    // ...
}
```
```rust
pub fn build_system_prompt(plan_first: bool, user_mcp: &[UserMcpServerTools]) -> String {
    let mut out = String::from(PROMPT_BODY);
    if plan_first { out.push_str(PLAN_FIRST_DIRECTIVE); }
    // Only render when there is at least one server — otherwise stay byte-identical.
    if user_mcp.iter().any(|s| !s.tools.is_empty()) {
        out.push_str(&render_user_mcp_section(user_mcp));
    }
    out
}
```
NON-BUG: with an empty server slice the env var is omitted and the prompt is byte-identical.

**N15 — filter-before-take correct**
`devboule-coder/src/config.rs` (`parse_user_mcp_servers` pipeline)
```rust
for e in entries {
    let name = e.name.trim().to_string();
    let command = e.command.trim().to_string();
    if command.is_empty() {
        eprintln!("devboule: user MCP server '{name}' has an empty command; skipping");
        continue;   // filter(non-empty) FIRST
    }
    if let Err(reason) = user_server_name_ok(&name) {
        eprintln!("devboule: user MCP server name '{name}' rejected ({reason}); skipping");
        continue;   // filter(invalid name) SECOND
    }
    if specs.len() == MAX_USER_MCP_SERVERS {
        eprintln!("devboule: more than {MAX_USER_MCP_SERVERS} user MCP servers ...");
        break;       // take(cap) LAST — applies to valid entries only
    }
    specs.push(...);
}
```
NON-BUG: the pipeline is `filter(non-empty)→take(cap)`; the cap applies to valid entries.

**N16 — serde shapes agree**
`src-tauri/src/backend/user_mcp_config.rs` (producer `OrchestratorServerPayload`) + `devboule-coder/src/config.rs` (consumer `Entry`)
```rust
// Producer (src-tauri):
#[serde(rename_all = "camelCase")]
struct OrchestratorServerPayload<'a> {
    name: &'a str,
    command: &'a str,
    args: &'a [String],
    env: &'a BTreeMap<String, String>,
}
```
```rust
// Consumer (devboule-coder):
#[derive(serde::Deserialize)]
struct Entry {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}
```
NON-BUG: all fields are single-word, so camelCase == snake on the wire; producer and consumer agree.

**N17 — McpTool round-trip**
`devboule-coder/src/action.rs` (`McpTool` variant definition with serde attrs)
```rust
// enum tag: #[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
McpTool {
    server: String,
    // The wire key is `name` (NOT `tool`): the enum is internally tagged on
    // `"tool"` (the action TYPE), so the tool-to-call must use a different key or
    // serde rejects the `tool` field as colliding with the tag.
    #[serde(rename = "name")]
    tool: String,
    params: serde_json::Value,
},
```
NON-BUG: `serde rename "name"` + tag `"tool"` yields a clean round-trip (verified by test).

**N18 — no identity smuggling**
`devboule-coder/src/rmcp_backend.rs` (`connect_generic` `inject_identity = false` + guarded branch)
```rust
// connect_generic sets:
inject_identity: false,
```
```rust
// identity-injection branch — only reached when inject_identity=true (Oracle path):
if self.inject_identity {
    args.insert("role".into(), json!(self.role));
    args.insert("agent_id".into(), json!(self.agent_id));
    if !self.session_token.is_empty() {
        args.insert("session_token".into(), json!(self.session_token));
    }
}
```
NON-BUG: `connect_generic` sets `inject_identity=false`, so no `role`/`agent_id`/`token` is added to user-server params.

**N19 — oversized config safe**
`src-tauri/src/backend/user_mcp_config.rs` (`MAX_CONFIG_BYTES` + regular-file gate + fail-open read)
```rust
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

fn read_config_file(path: &Path) -> UserMcpConfig {
    // Regular-file gate: a FIFO/device at the path would BLOCK File::open forever.
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => { return UserMcpConfig::default(); }  // not a regular file → fail open
        Err(_) => return UserMcpConfig::default(),     // missing → fail open (normal case)
    }
    let mut handle = match std::fs::File::open(path) {
        Ok(f) => f.take(MAX_CONFIG_BYTES + 1),   // size-bounded read
        Err(e) => { return UserMcpConfig::default(); }
    };
    let mut buf = Vec::new();
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        return UserMcpConfig::default();   // oversized → fail open
    }
```
NON-BUG: the read is size-bounded, regular-file-gated, and fails open per-entry.
