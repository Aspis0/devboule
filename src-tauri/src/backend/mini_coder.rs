//! Mini-coder: data contracts, directive state machine, and the PURE executor
//! core (clock-split, IO-free) for the one-shot helper a fleet coder delegates
//! cheap sub-tasks to.
//!
//! This file is the HEADLESS core (Phase MC-P1). It deliberately contains NO
//! process spawn, NO MCP, NO Tauri command, and NO UI — those arrive in P2+.
//! What lives here:
//!
//!   * The serde contracts crossing two boundaries: the directive queue inside
//!     `.aspis-agents.json` (coder's MCP tool writes it, the Rust executor drains
//!     it) and the result file (the mini writes it, the executor reads it). Both
//!     are camelCase + every optional/list field `#[serde(default)]` + read
//!     leniently, mirroring `backend/model.rs` and `backend/censor/schema.rs`:
//!     one malformed entry must never brick the whole-state read.
//!   * The directive lifecycle `pending -> launching -> running ->
//!     (done | needs_clarification | aborted_by_human | failed | timeout)` as a
//!     set of pure transition helpers (no double-claim).
//!   * `plan_tick` — the pure scheduler (clock-split like `fs_watch.rs`: the
//!     caller supplies `now`, there is no `Instant::now()`/`Utc::now()` inside),
//!     deciding which pending directive to claim next and which running directive
//!     has blown its wall-clock cap.
//!   * `cap_directives` — the bounded queue eviction (oldest TERMINAL first;
//!     never evict an active directive).
//!   * `read_result_file` — the lenient, path-confined result reader.
//!   * The `MiniLauncher` trait the impure P2 executor implements with a real PTY
//!     spawn; tests here implement it with a fake.
//!
//! Most of the pure API below has NO non-test caller yet: the impure executor
//! loop, the MCP `spawn_mini_coder` tool, and the real PTY launcher that consume
//! it all land in Phase MC-P2. The serde contracts ARE already live (model.rs
//! embeds `MiniCoderDirective`/`MiniCoderOutcome` in `AgentLiveState`), but the
//! scheduler/transition/reader functions are intentionally ahead of their first
//! caller. A crate-level `dead_code` allow on this module keeps the P1 skeleton
//! warning-clean without scattering per-item attributes. MC-P2 removed the blanket
//! allow once the executor (`mini_coder_executor.rs`) wired up `plan_tick` +
//! `apply_claim`/`apply_launched`/`apply_result`/`apply_timeout`/`apply_failed` +
//! `cap_directives` + `read_result_file`. MC-P5 wired `apply_aborted` into the
//! executor's human-kill finalize path (`finalize_finished_mini` honoring
//! `kill_requested`). The one item still ahead of its first caller carries a TARGETED
//! `#[allow(dead_code)]`: the `MiniLauncher` trait (the P1 test seam; the P2 executor
//! spawns the PTY directly rather than through it).

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use chrono::DateTime;
use serde::{Deserialize, Serialize};

/// Maximum number of directives retained in the in-file queue. Beyond this,
/// `cap_directives` evicts the oldest TERMINAL directives (never an active one).
/// Bounded so a long-lived `.aspis-agents.json` cannot grow without limit.
pub const MAX_DIRECTIVES: usize = 50;

/// Default wall-clock cap (seconds) for a single running mini before the executor
/// kills it and synthesizes `timeout`. Mirrors the plan's ~10-minute hard cap.
/// Lives here so the pure `plan_tick` and the P2 executor agree on one constant.
pub const DEFAULT_WALL_CLOCK_CAP_SECS: i64 = 600;

/// P6: shorter wall-clock cap (seconds) for a RETRY attempt (`attempt >= 1`). A retry
/// re-does a bounded fix the prior attempt already scoped (it carries the predecessor's
/// files + the Censor feedback), so it should not get the full fresh-task budget — a
/// stuck retry is reaped sooner so the whole chain stays inside the Python poll budget
/// (1 + MAX_MINI_RETRIES attempts). Applied by `plan_tick` to `attempt >= 1` directives;
/// `attempt == 0` (the root) keeps `DEFAULT_WALL_CLOCK_CAP_SECS`.
pub const DEFAULT_RETRY_WALL_CLOCK_CAP_SECS: i64 = 300;

/// Max seconds a directive may sit in `launching` before the executor gives up on
/// it and synthesizes `failed`, RELEASING the single concurrency slot (WARNING 4).
/// A directive is `launching` only between the claim and the `apply_launched` that
/// follows the PTY spawn — normally sub-second. A directive still `launching` after
/// this cap means the launch bookkeeping never completed (app crash mid-launch, or
/// a spawn that wedged), so the slot would otherwise be held forever. Measured from
/// `claimed_at`. Generous vs the real ~sub-second launch so a slow disk write of
/// the running transition is never mistaken for a stuck launch.
pub const DEFAULT_LAUNCH_CAP_SECS: i64 = 60;

/// Hard cap on the bytes `read_result_file` will read from a mini's result file.
/// A legitimate result (status + short output + a handful of touched paths) is a
/// few KiB; 1 MiB is generous headroom. Beyond it the file is treated as hostile/
/// runaway and degrades to `failed` rather than reading unbounded (OOM guard).
pub(crate) const MAX_RESULT_BYTES: u64 = 1 << 20; // 1 MiB

/// B1 (agentic-iterative): retry budget for an `AgenticIterative` WRITE directive on a
/// language WITH deterministic Tier-A coverage. The agentic loop is just the existing P6
/// retry chain run for more rounds — each round re-emits edits, Rust applies them, the
/// deterministic Censor verdict gates, and a dirty verdict appends the next round (up to
/// this budget) before `Escalated`. Raising the budget IS the loop; nothing else changes.
///
/// VALUE = 2, justified against the POLL BUDGET (this is the load-bearing bound). The
/// poll timeout is a HARD failure, NOT a soft "still in progress" signal: when the Python
/// `spawn_mini_coder` poll (`MINI_CODER_POLL_TIMEOUT_SECS` in `oracle/server/aspis_mcp.py`)
/// hits its 1800s cap it STAMPS the directive `status="timeout"` + a terminal `result` on
/// disk (and returns that `timeout` outcome to the coder). Two consequences make this
/// non-recoverable: (1) the coder acts on the terminal `timeout`; (2) the on-disk terminal
/// stamp POISONS the still-live retry chain — the executor's later real `apply_result`
/// sees a non-active (terminal) directive and is REFUSED (`apply_outcome` Err), so the real
/// verdict is lost and the chain strands. The worst-case therefore MUST fit with headroom.
///
/// The chain runs `1 + budget` sequential attempts. Each attempt is gated by `plan_tick`:
/// it first sits in `Launching` (capped at [`DEFAULT_LAUNCH_CAP_SECS`] = 60s) and then runs
/// (capped at [`DEFAULT_WALL_CLOCK_CAP_SECS`] = 600s for attempt 0, [`DEFAULT_RETRY_WALL_CLOCK_CAP_SECS`]
/// = 300s for every retry `attempt >= 1`). Launch precedes Run and is sequential, so the
/// caps-only worst-case for a budget of N is:
///   600 + N*300 + (N+1)*60 seconds.
///     N = 2 -> 600 + 600 + 180  = 1380s  (420s headroom)
///     N = 3 -> 600 + 900 + 240  = 1740s  (only 60s headroom)
/// The earlier formula (`600 + N*300`) OMITTED the per-attempt launch caps and so
/// understated the worst case. Beyond the caps, the deferred verdict thread runs the FINE
/// linters AFTER the process exits, OUTSIDE the wall-clock caps (the directive is excluded
/// from `plan_tick` while the verdict is in-flight); the Fine set includes semgrep (up to a
/// ~300s ceiling) per inter-round gap in the pathological case. At N = 3 the caps-only 1740s
/// leaves only 60s — a single slow verdict pass blows past 1800s and HARD-times-out the
/// poll. At N = 2 the 420s headroom comfortably absorbs the per-round verdict passes (in
/// practice the Fine pass on a <=10-file directive is seconds, not minutes).
///
/// We LOWER N to 2 rather than extend the shared, cross-language `MINI_CODER_POLL_TIMEOUT_SECS`
/// constant for a (currently GPU-deferred) feature: keeping the Python poll contract
/// untouched is the conservative choice, and 2 fix rounds already cover the vast majority of
/// deterministic-Censor convergence. If a future need raises N back to >= 3, the Python poll
/// constant MUST be extended in lockstep with a verdict-thread allowance (and the math above
/// re-checked); the test `budget_max_agentic_fix_rounds_fits_the_python_poll_budget` pins it.
pub const MAX_AGENTIC_FIX_ROUNDS: u32 = 2;

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Directive lifecycle status. snake_case over the wire to match the plan's
/// status strings exactly (`pending|launching|running|done|needs_clarification|
/// aborted_by_human|failed|timeout`) and the Python MCP writer.
///
/// `Default` is `Pending` so a directive missing the key (hand-edited / older
/// writer) deserializes to the queue's entry state rather than hard-erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MiniCoderStatus {
    #[default]
    Pending,
    Launching,
    Running,
    Done,
    NeedsClarification,
    AbortedByHuman,
    Failed,
    Timeout,
}

impl MiniCoderStatus {
    /// `true` once the directive has reached a terminal state (no further
    /// transition). Only terminal directives are eligible for eviction.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            MiniCoderStatus::Done
                | MiniCoderStatus::NeedsClarification
                | MiniCoderStatus::AbortedByHuman
                | MiniCoderStatus::Failed
                | MiniCoderStatus::Timeout
        )
    }

    /// `true` while the directive is live (claimed/launching/running) and must
    /// NEVER be evicted nor re-claimed.
    pub fn is_active(self) -> bool {
        matches!(self, MiniCoderStatus::Launching | MiniCoderStatus::Running)
    }
}

// ---------------------------------------------------------------------------
// Result file (mini -> app) vs synthesized outcome (app-owned)
// ---------------------------------------------------------------------------

/// What the mini writes to its result file as its FINAL action, then exits. The
/// mini can only legitimately report `done` or `needs_clarification` — the other
/// terminal states (`aborted_by_human`/`failed`/`timeout`) are SYNTHESIZED by the
/// executor and the mini never writes them. Read leniently: every field defaults,
/// so a partial object (e.g. just `{"status":"done"}`) still parses; a malformed
/// or missing file is handled by `read_result_file` (-> `failed`), never a panic.
/// P4: one structured edit the mini EMITS instead of touching disk. Exact-match
/// contract: `old_string` must occur EXACTLY ONCE in the target file (the
/// executor rejects 0 or >1 matches); an EMPTY `old_string` means "create the
/// file with `new_string` as its full content". `path` is project-root-relative
/// and must name a file in the directive's allowlist — the executor enforces
/// all of this in `apply_emitted_edits`, the model is never trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MiniEdit {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub old_string: String,
    #[serde(default)]
    pub new_string: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MiniCoderResult {
    /// "done" | "needs_clarification" (any other value is treated as invalid by
    /// `read_result_file`). Kept as a raw string here (not the enum) so a typo'd
    /// status degrades to `failed` in the reader rather than failing the parse.
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    /// P4: structured edits for a WRITE directive. Applied (after validation)
    /// by the executor; cleared before the outcome is persisted to state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<MiniEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// SANDBOX broker: set by the agentic worker when `looks_network_blocked` fired
    /// during a `run` call while `net == NetPolicy::None`. Propagated into
    /// `MiniCoderOutcome` so `finalize_finished_mini` can emit the consent-request
    /// event.  NO-CHURN: omitted from the JSON when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub net_blocked: bool,
    /// SANDBOX broker Slice 2: set by the agentic worker when a write attempt targeted a
    /// path outside (root + working_set). Contains the canonicalized PARENT FOLDER of the
    /// denied write. Propagated into `MiniCoderOutcome` so `finalize_finished_mini` can
    /// emit a `FolderWrite` consent-request event.  NO-CHURN: omitted from the JSON when
    /// `None` so existing result files that pre-date Slice 2 continue to deserialize cleanly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_write_blocked: Option<String>,
}

/// The app-owned terminal payload stored in `MiniCoderDirective.result`. A
/// SUPERSET of `MiniCoderResult`: it carries the resolved `MiniCoderStatus`
/// (which may be a synthesized `aborted_by_human`/`failed`/`timeout` the mini
/// never wrote) plus an `error` string the executor fills in for those
/// synthesized states (e.g. "result file missing", "wall-clock cap exceeded").
///
/// Keeping this DISTINCT from `MiniCoderResult` is intentional: the result file
/// is the mini's narrow self-report, while the outcome is the authoritative
/// app-side verdict the coder's `spawn_mini_coder` call returns. Conflating them

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MiniCoderOutcome {
    pub status: MiniCoderStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_touched: Vec<String>,
    /// P4: the emitted edits, carried from the result file to the apply step in
    /// finalize. The apply step CLEARS this before the outcome is stamped onto
    /// the directive, so edit bodies never bloat the persisted agent state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<MiniEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// App-supplied reason for a SYNTHESIZED terminal state. None for a clean
    /// `done`/`needs_clarification` reported by the mini.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SANDBOX broker: true when the agentic worker set `net_blocked` in its result
    /// JSON (network-blocked failure detected while `net == NetPolicy::None`).
    /// `finalize_finished_mini` uses this to emit the `sandbox://consent-request`
    /// event.  NO-CHURN: omitted from serialized state when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub net_blocked: bool,
    /// SANDBOX broker Slice 2: canonicalized parent folder of a write attempt that was
    /// blocked because it targeted a path outside (root + working_set).
    /// `finalize_finished_mini` uses this to emit the `FolderWrite` consent-request event.
    /// NO-CHURN: omitted when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_write_blocked: Option<String>,
    /// Phase A Censor findings (fast runners, ≤3s). None = no issues or timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub censor_findings: Option<Vec<crate::backend::censor::schema::Finding>>,
}

impl MiniCoderOutcome {
    /// Outcome synthesized from a valid mini-written `MiniCoderResult` reporting
    /// `done`. Carries the mini's output/filesTouched verbatim, no error.
    pub fn done(result: MiniCoderResult) -> Self {
        Self {
            status: MiniCoderStatus::Done,
            output: result.output,
            files_touched: result.files_touched,
            edits: result.edits,
            question: None,
            partial: result.partial,
            error: None,
            net_blocked: result.net_blocked,
            folder_write_blocked: result.folder_write_blocked,
            censor_findings: None,
        }
    }
    /// App-synthesized `failed` (no/invalid result file, parent gone, etc).
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: MiniCoderStatus::Failed,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// App-synthesized `timeout` (wall-clock cap exceeded; mini was killed).
    pub fn timeout(error: impl Into<String>) -> Self {
        Self {
            status: MiniCoderStatus::Timeout,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// App-synthesized `aborted_by_human` (the human hit Stop).
    pub fn aborted(error: impl Into<String>) -> Self {
        Self {
            status: MiniCoderStatus::AbortedByHuman,
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// App-synthesized `needs_clarification`. Carries the mini's question.
    pub fn needs_clarification(result: MiniCoderResult) -> Self {
        Self {
            status: MiniCoderStatus::NeedsClarification,
            output: result.output,
            files_touched: result.files_touched,
            edits: result.edits,
            question: result.question.clone(),
            partial: result.partial,
            error: None,
            net_blocked: result.net_blocked,
            folder_write_blocked: result.folder_write_blocked,
            censor_findings: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Directive (coder -> app, lifecycle owned by the executor)
// ---------------------------------------------------------------------------

/// One mini-coder spawn directive in the `.aspis-agents.json` queue. The coder's
/// MCP `spawn_mini_coder` tool appends it as `status:"pending"`; the headless
/// executor (P2) drives the lifecycle and stamps `agent_id`/`started_at`/`result`.
///
/// camelCase over the wire; every optional/list field `#[serde(default)]` so a
/// partial / hand-edited / older-writer directive still deserializes (the
/// session-level `lenient_mini_coder_directives` then drops only an entry that
/// fails entirely, never bricking the whole state read).
/// `skip_serializing_if` predicate for a `bool` that is false-by-default, so a
/// directive whose Python writer omitted the key round-trips through Rust without
/// gaining a `"...": false` we'd otherwise inject (no-churn co-ownership).
fn is_false(b: &bool) -> bool {
    !*b
}

/// How a WRITE mini-coder applies its changes. camelCase over the wire to match
/// the owning [`MiniCoderDirective`]'s `#[serde(rename_all = "camelCase")]`
/// (the struct's other enum-typed field `status` is snake_case only because it
/// must mirror the plan's literal status strings — `WriteMode` has no such
/// external contract, so it follows the struct's own convention).
///
/// `Default` is [`WriteMode::EmitEdits`] so a directive missing the key
/// (today's writers, hand-edited, older) deserializes to the current behavior.
/// NO-CHURN: paired with [`is_emit_edits`] as the `skip_serializing_if`
/// predicate so an `EmitEdits` directive serializes BYTE-IDENTICALLY to today
/// (no `"writeMode"` key injected).
///
/// PLUMBING ONLY: nothing branches on this yet — a later workstream reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WriteMode {
    /// The mini returns a structured JSON edit list; the Rust executor validates
    /// it against `files` and applies it (today's only behavior). Wire: `"emitEdits"`.
    #[default]
    EmitEdits,
    /// The mini iterates agentically, writing files itself (gated by language
    /// coverage — behavior wired later). Wire: `"agenticIterative"`.
    AgenticIterative,
}

/// `skip_serializing_if` for a [`WriteMode`] that is `EmitEdits`-by-default
/// (NO-CHURN co-ownership, like [`is_false`] for bools): a writer that omitted
/// `writeMode` round-trips through Rust without gaining a `"writeMode":"emitEdits"`.
fn is_emit_edits(m: &WriteMode) -> bool {
    matches!(m, WriteMode::EmitEdits)
}

/// ROLE UNTANGLE Phase 3 — which TIER runs a directive. `Mini` (the default) is
/// the delegated sub-task worker exactly as today. `Main` is the FIRST-CLASS
/// MAIN CODER: the promoted agentic engine — always the multi-turn sandboxed
/// tool loop, never the one-shot PTY (the executor FAILS a Main directive that
/// cannot run agentic rather than silently downgrading it). Wire strings:
/// `"mini"` / `"main"`; the Python co-writer (`spawn_main_coder` in
/// aspis_mcp.py) emits `"tier": "main"`. NO-CHURN: paired with [`is_mini_tier`]
/// so every existing directive (no `tier` key) serializes byte-identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DirectiveTier {
    /// The delegated sub-task worker (one-shot emit-edits or agentic by the S2
    /// capability policy). Wire: `"mini"`.
    #[default]
    Mini,
    /// The first-class Main coder: always agentic + Seatbelt-sandboxed. Wire:
    /// `"main"`.
    Main,
}

/// `skip_serializing_if` for a [`DirectiveTier`] that is `Mini`-by-default
/// (NO-CHURN co-ownership): a writer that omitted `tier` round-trips through
/// Rust without gaining a `"tier":"mini"`.
fn is_mini_tier(t: &DirectiveTier) -> bool {
    matches!(t, DirectiveTier::Mini)
}

/// E1 — the USER-FACING write-behavior POLICY (the ceiling the coder's per-task
/// A3 `write_mode` decision must respect). Persisted in config.json under
/// `miniWriteBehavior`; absent ⇒ [`MiniWriteBehavior::Auto`] (the current behavior).
///
/// This is NOT the same axis as [`WriteMode`]: `WriteMode` is the per-directive
/// mechanism the coder picks for a single delegation; `MiniWriteBehavior` is the
/// global policy that BOUNDS what the coder is allowed/encouraged to pick. The
/// coder reads it through the A3 launch-prompt guidance (`projects.rs`).
///
/// camelCase over the wire to match the TS `MiniWriteBehavior` + the config.json
/// discriminator (e.g. `"agenticAllowed"`), exactly like [`MiniCoderBackendKind`].
/// `Default` is [`MiniWriteBehavior::Auto`] so a config missing the key (today's
/// configs, hand-edited, older) resolves to the unchanged Auto guidance — paired
/// with [`is_auto_write_behavior`] as the `skip_serializing_if` predicate so the
/// Auto default serializes BYTE-IDENTICALLY to today (no `"miniWriteBehavior"` key
/// injected → NO-CHURN).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum MiniWriteBehavior {
    /// The user forbids agentic-iterative entirely: the coder must delegate with
    /// emit-edits ONLY (one write + one fix), regardless of language coverage or
    /// model capability. Wire: `"safe"`.
    Safe,
    /// The default: the coder decides per task (agentic where the language is gate-
    /// covered AND the model is capable, emit-edits otherwise). This is today's
    /// behavior. Wire: `"auto"`.
    #[default]
    Auto,
    /// The user opts in to agentic-iterative for capable models: it is ENCOURAGED on
    /// gate-covered languages (the coder still falls back to emit-edits for uncovered
    /// languages or a weak model). Wire: `"agenticAllowed"`.
    AgenticAllowed,
}

/// `skip_serializing_if` for a [`MiniWriteBehavior`] that is `Auto`-by-default
/// (NO-CHURN co-ownership, like [`is_emit_edits`] for [`WriteMode`]): a config that
/// never set / left the policy at Auto round-trips through Rust without gaining a
/// `"miniWriteBehavior":"auto"` key, so an existing config.json is byte-identical.
pub fn is_auto_write_behavior(m: &MiniWriteBehavior) -> bool {
    matches!(m, MiniWriteBehavior::Auto)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniCoderDirective {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The coder that requested the mini; also what nests the mini under it in the
    /// rail and is the sole human-contact point on escalation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_agent_id: String,
    #[serde(default)]
    pub status: MiniCoderStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// P4: a WRITE directive — the mini is expected to EMIT structured edits
    /// (HTTP backends) that the executor validates against `files` and applies.
    /// NO-CHURN: omitted when false, like `allow_oracle` below.
    #[serde(default, skip_serializing_if = "is_false")]
    pub write: bool,
    /// How a WRITE mini applies its changes (emit-edits vs agentic-iterative).
    /// Only meaningful when `write` is true. PLUMBING ONLY: carried end-to-end,
    /// nothing branches on it yet (a later workstream reads it). NO-CHURN:
    /// omitted when `EmitEdits` (the default), so a directive that did not set it
    /// serializes byte-identically to today, exactly like `write`/`allow_oracle`.
    #[serde(default, skip_serializing_if = "is_emit_edits")]
    pub write_mode: WriteMode,
    /// ROLE UNTANGLE Phase 3: which TIER runs this directive — `Mini` (default,
    /// the delegated sub-task worker, byte-identical to today) or `Main` (the
    /// first-class Main coder: always agentic + sandboxed; the executor FAILS a
    /// Main directive that cannot run agentic instead of downgrading it to the
    /// one-shot path). NO-CHURN: omitted when `Mini` (see [`is_mini_tier`]); the
    /// Python co-writer preserves it verbatim like every passthrough field.
    #[serde(default, skip_serializing_if = "is_mini_tier")]
    pub tier: DirectiveTier,
    /// ROLE UNTANGLE Phase 3: EXPLICIT project scope for an APP-AUTHORED directive
    /// (one whose `parent_agent_id` is the "app-user" sentinel, i.e. appended by
    /// `append_main_coder_directive`, today called from `polis_fix_sin`). MCP-dispatched directives derive
    /// their project from the live parent session (`snapshot_parent_project`) and
    /// leave this None; when Some, the executor prefers it and skips the
    /// parent-liveness auto-kill (the human IS the supervisor of an app-authored
    /// directive — there is no parent session to die). NO-CHURN: omitted when None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Backend override (ollama|api|codex); None -> the global configured backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Whether the mini may consult the Oracle (default-deny). NO-CHURN: a Python
    /// MCP-written directive omits this when false; we must not re-inject
    /// `"allowOracle": false` on a Rust round-trip, so skip it when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_oracle: bool,
    /// P5 SAFETY BRAKE: set true by `mini_coder_kill` (the human Stop button) the
    /// instant BEFORE the PTY is killed, so the EOF-driven `finalize_finished_mini`
    /// path sees the human's intent and synthesizes `aborted_by_human` rather than
    /// `done`/`failed`/`timeout`. The human assertion of control WINS any racing
    /// terminal outcome (enforced in the executor's `finalize_finished_mini`). NO-CHURN:
    /// a Python MCP-written directive omits this when false; we must not re-inject
    /// `"killRequested": false` on a Rust round-trip, so skip it when false. The Python
    /// co-owner preserves it verbatim (passthrough, like scratchPath/claimedAt).
    #[serde(default, skip_serializing_if = "is_false")]
    pub kill_requested: bool,
    /// ASYNC STEERING (piece a): a FIFO of mid-flight supervisor corrections, the
    /// SAME external-signal-to-a-running-directive channel as `kill_requested`
    /// generalized from one bool to a queue. A supervising coder / the human appends a
    /// guidance message (Python `steer_mini_coder` MCP tool or the `mini_coder_steer`
    /// Tauri command); the executor DRAINS it at the next round boundary and folds it
    /// into the retry's task as a `SUPERVISOR STEERING:` block (the same append path
    /// Censor feedback uses — see `build_retry_directive`). The reserved sentinel
    /// [`STEER_STOP_SENTINEL`] maps to the kill path (sets `kill_requested`). NO-CHURN:
    /// a directive that was never steered omits this entirely (empty Vec skipped), so a
    /// Python-written directive round-trips byte-identically — exactly like
    /// `killRequested` skips when false. The Python co-owner preserves it verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steer_queue: Vec<String>,
    /// Relative path (under the project scratch root) the mini writes its result
    /// JSON to. `..`/absolute is rejected by `read_result_file`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_path: String,
    /// The spawned mini's `AgentSession` id, set at launch by the executor. None
    /// until launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// RFC3339 wall-clock time the executor CLAIMED the directive (pending ->
    /// launching), stamped in `apply_claim`. This is the `Launching` anchor: the
    /// executor fails a directive stuck `launching` longer than the launch cap
    /// (WARNING 4) measured from here. None until claimed. Rust-set only; the
    /// Python co-owner preserves it verbatim (passthrough).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<String>,
    /// Absolute path to the project scratch root (`<project_root>/.aspis-mini`)
    /// the mini writes its result file under, RESOLVED + PERSISTED at launch time
    /// by the executor (WARNING/BLOCKER 3). Finalization reads the result from
    /// HERE rather than re-resolving the parent's live `current_project_id`, so a
    /// parent that switched projects between launch and the mini's EOF cannot
    /// redirect the read to the wrong tree. None until launched. Rust-set only; the
    /// Python co-owner preserves it verbatim (passthrough).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch_path: Option<String>,
    /// RFC3339 wall-clock launch time, set when the directive enters `running`.
    /// The wall-clock-cap check in `plan_tick` measures from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// The app-owned terminal verdict, set once the directive reaches a terminal
    /// state. None while pending/launching/running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<MiniCoderOutcome>,
    /// v6 Phase 5: structured stuck report emitted on a terminal failure/timeout
    /// (the mini exhausted its budget). Durable fleet-state mirror of the fire-and-
    /// forget `mini://stuck` event — survives app restarts where the live event
    /// would be lost to a missing listener. None until a stuck report is emitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck_report: Option<crate::backend::stuck_report::StuckReport>,
    /// P6 retry lineage. Retry attempt number: the ROOT directive (the one the
    /// coder's `spawn_mini_coder` originally appended and whose id the blocking poll
    /// watches) is attempt 0; each Censor-driven retry increments it. NO-CHURN: a
    /// Python-written directive omits this when 0; we must not re-inject `0`.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub attempt: u32,
    /// P6: id of the ROOT directive of this retry chain. None on the root itself;
    /// Some(root_id) on every retry. The terminal-propagation walk uses this to find
    /// all `AwaitingRetry` predecessors in the lineage and stamp them with the leaf's
    /// outcome, so the poll watching the ROOT id unblocks. NO-CHURN: omitted when None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_directive_id: Option<String>,
    /// Slice 3 (Pigeon transport): the Pigeon mailbox ticket this directive arrived on,
    /// stamped by the executor's INGEST when it drains `mini-pool` (seam B). The EGRESS
    /// (seam C) posts the terminal outcome back to THIS ticket (`/pigeon/done`) so the
    /// Python `spawn_mini_coder` wait, which polls `/pigeon/status/{ticket}`, unblocks.
    /// NO-CHURN: a directive that never went through Pigeon (the default file path, AND
    /// every Python-written directive) omits this entirely (None), so the on-disk JSON is
    /// byte-identical to before this slice. The Python co-owner preserves it verbatim
    /// (passthrough, like `claimedAt`/`scratchPath`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pigeon_ticket: Option<i64>,
    /// F-E hardening: stamped `Some(true)` by the Python `mini_coder_result` MCP
    /// tool once it has successfully handed this directive's TERMINAL outcome
    /// back to its caller (the async `spawn_mini_coder(wait=false)` +
    /// `mini_coder_result` pattern). `cap_mini_coder_directives` (Python)
    /// evicts COLLECTED terminal directives before UNCOLLECTED ones. Rust-side
    /// this is PASSTHROUGH ONLY (nothing here sets or reads it) — but without
    /// `#[serde(default)]` this struct's `deserialize` would hard-fail on a
    /// Python-written directive carrying the key, and without the field at all
    /// the Rust round-trip would SILENTLY DROP it on every re-serialize (the co-
    /// writer hazard this field exists to close). NO-CHURN: omitted when None,
    /// like `pigeon_ticket`/`claimed_at`, so a directive that was never
    /// collected stays byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collected: Option<bool>,
}

/// `skip_serializing_if` for a `u32` that is zero-by-default (NO-CHURN co-ownership,
/// like [`is_false`] for bools).
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// ASYNC STEERING (a) cap: max chars of a SINGLE steer message folded into the next
/// round's task. CO-WRITER PARITY: the Python `MINI_CODER_MAX_STEER_LEN` must match —
/// both writers (`mini_coder_steer` Tauri command + `steer_mini_coder` MCP tool) trim to
/// this so a pathological message can't bloat the prompt.
pub const MAX_STEER_MESSAGE_LEN: usize = 2000;
/// ASYNC STEERING (a) cap: max queued steer messages on one directive (a bounded FIFO).
/// A steerer that floods past this is silently dropping the OLDEST entries is wrong — we
/// REFUSE the append instead (the caller gets a clear "queue full" status), so an
/// already-queued correction is never lost. CO-WRITER PARITY with Python.
pub const MAX_STEER_QUEUE_LEN: usize = 8;
/// ASYNC STEERING (a) — P2 (AGGREGATE BLOAT CAP): the per-message [`MAX_STEER_MESSAGE_LEN`]
/// cap does NOT bound the SUM folded into one fix-pass task. A full queue
/// ([`MAX_STEER_QUEUE_LEN`] messages × [`MAX_STEER_MESSAGE_LEN`] chars) is 16 000 chars of
/// steering on top of the original task + Censor feedback — enough to balloon the retry
/// prompt. So [`fold_steer_block`] caps the ASSEMBLED steer body to this total budget (chars),
/// truncating at a message boundary and appending a clear `[… steering truncated]` marker so
/// the model knows corrections were dropped. Generous enough that a realistic handful of
/// corrections always fits unchanged; it only bites a pathological flood.
pub const MAX_STEER_BLOCK_LEN: usize = 4000;

// ---------------------------------------------------------------------------
// Pure state transitions (no double-claim)
// ---------------------------------------------------------------------------

/// Claim a `pending` directive: move it to `launching`, stamping `claimed_at`
/// (the `Launching` anchor the executor uses to fail a directive stuck launching
/// past the launch cap — WARNING 4). Claiming a non-pending directive is an ERROR
/// (the no-double-claim guard): two executor passes (or an executor racing a stale
/// view) must never both claim the same directive. Clock-split: the caller supplies
/// `claimed_at` (RFC3339) so this stays IO-free and clock-free like `plan_tick`.
pub fn apply_claim(
    directive: &MiniCoderDirective,
    claimed_at: impl Into<String>,
) -> Result<MiniCoderDirective, String> {
    if directive.status != MiniCoderStatus::Pending {
        return Err(format!(
            "cannot claim directive {} in status {:?} (only pending is claimable)",
            directive.id, directive.status
        ));
    }
    let mut next = directive.clone();
    next.status = MiniCoderStatus::Launching;
    next.claimed_at = Some(claimed_at.into());
    Ok(next)
}

/// Record the spawned mini's agent id and move `launching -> running`, stamping
/// `started_at` (the wall-clock-cap anchor). Only a `launching` directive may
/// transition; anything else is an error (an out-of-order/duplicate launch).
pub fn apply_launched(
    directive: &MiniCoderDirective,
    agent_id: impl Into<String>,
    started_at: impl Into<String>,
) -> Result<MiniCoderDirective, String> {
    if directive.status != MiniCoderStatus::Launching {
        return Err(format!(
            "cannot mark directive {} running from status {:?} (expected launching)",
            directive.id, directive.status
        ));
    }
    let mut next = directive.clone();
    next.agent_id = Some(agent_id.into());
    next.started_at = Some(started_at.into());
    next.status = MiniCoderStatus::Running;
    Ok(next)
}

/// Apply a terminal `MiniCoderOutcome` (done/needs_clarification/failed/timeout/
/// aborted) to a directive. Only an ACTIVE directive (`Launching | Running`) may
/// reach a terminal state: a `Pending` directive must go through the launch flow
/// first (claim -> launch -> run) and an already-terminal directive must not be
/// clobbered (idempotence guard: a late result must not overwrite a kill that
/// already won). This still allows the spawn-error path `Launching -> failed` and
/// `Running -> done/needs_clarification/failed/timeout/aborted`, but blocks both
/// `Pending -> *` (bypassing launch) and terminal-overwrite.
fn apply_outcome(
    directive: &MiniCoderDirective,
    outcome: MiniCoderOutcome,
) -> Result<MiniCoderDirective, String> {
    if !directive.status.is_active() {
        return Err(format!(
            "cannot apply outcome to directive {} in status {:?} \
             (only launching/running directives can reach a terminal state)",
            directive.id, directive.status
        ));
    }
    let mut next = directive.clone();
    next.status = outcome.status;
    next.result = Some(outcome);
    Ok(next)
}

/// Apply the mini's reported result (`done`/`needs_clarification`) — or any
/// synthesized outcome — to a non-terminal directive.
pub fn apply_result(
    directive: &MiniCoderDirective,
    outcome: MiniCoderOutcome,
) -> Result<MiniCoderDirective, String> {
    apply_outcome(directive, outcome)
}

/// Synthesize `timeout` (wall-clock cap exceeded; the executor killed the mini).
pub fn apply_timeout(
    directive: &MiniCoderDirective,
    reason: impl Into<String>,
) -> Result<MiniCoderDirective, String> {
    apply_outcome(directive, MiniCoderOutcome::timeout(reason))
}

/// Synthesize `aborted_by_human` (the human hit Stop). P5's human-kill finalize
/// path (`finalize_finished_mini` when `kill_requested`) is the production caller.
pub fn apply_aborted(
    directive: &MiniCoderDirective,
    reason: impl Into<String>,
) -> Result<MiniCoderDirective, String> {
    apply_outcome(directive, MiniCoderOutcome::aborted(reason))
}

/// Synthesize `failed` (no/invalid result file, parent gone, spawn error).
pub fn apply_failed(
    directive: &MiniCoderDirective,
    reason: impl Into<String>,
) -> Result<MiniCoderDirective, String> {
    apply_outcome(directive, MiniCoderOutcome::failed(reason))
}

/// ASYNC STEERING (piece a): the reserved `steer_queue` message that maps to the kill
/// path instead of a queued correction. A steerer who sends this (case-insensitive,
/// after trimming) is asking to STOP the mini — handled by `mini_coder_steer` /
/// `dispatch_steer_mini_coder` by setting `kill_requested` (reusing the Stop path)
/// rather than queueing prose. Documented + shared so both writers and the executor
/// agree on the one sentinel.
pub const STEER_STOP_SENTINEL: &str = "stop";

/// ASYNC STEERING (piece a): true iff `message` is the reserved stop sentinel
/// ([`STEER_STOP_SENTINEL`], case-insensitive, trimmed). A steer that is a stop must
/// route to the kill path, not the queued-correction path.
pub fn is_steer_stop(message: &str) -> bool {
    message.trim().eq_ignore_ascii_case(STEER_STOP_SENTINEL)
}

/// ASYNC STEERING (piece a) — C2 (BIDI/INVISIBLE PARITY): sanitize a human steer message
/// before it is queued, folded into the fix-pass prompt AND shown in the Console. CO-WRITER
/// PARITY with the Python `steer_mini_coder` tool, which routes the message through
/// `clean_text` -> `strip_invisible_and_bidi` (drop invisible/bidi/control chars) followed by
/// a whitespace collapse. Without this the Tauri command only trimmed, so a steer could
/// smuggle a right-to-left override / zero-width joiner / BOM into the prompt or a toast
/// (spoofing or hiding text). We REUSE the canonical [`is_forbidden_command_char`] blocklist
/// (the SAME control + Cf bidi/zero-width/invisible set the `api`/oMLX validators reject) as
/// the drop predicate, then collapse runs of whitespace to single spaces and trim — mirroring
/// `clean_text`'s `" ".join(text.split()).strip()`. Note U+2028/U+2029 (line/paragraph
/// separator) are whitespace, so the collapse step removes them exactly as Python's regex does.
/// Pure + privacy-safe (operates on the supervisor's own prose). The caller still applies the
/// [`MAX_STEER_MESSAGE_LEN`] char cap AFTER this (so the cap counts only retained chars).
pub fn sanitize_steer_message(message: &str) -> String {
    let stripped: String = message
        .chars()
        // Drop the invisible/bidi/control blocklist, but DELIBERATELY keep whitespace
        // control chars (`\t`/`\n`/`\r`/…): Python strips only the invisible/bidi SET and
        // then `.split()` collapses whitespace to single spaces — so tabs/newlines must
        // survive this step to become word separators, not vanish (which would glue words
        // together). Whitespace is never a smuggling vector (it renders as space).
        .filter(|c| !(is_forbidden_command_char(*c) && !c.is_whitespace()))
        .collect();
    // Collapse any run of (remaining) whitespace to a single space, trim the ends — the
    // SAME normalization as Python's `clean_text` (`" ".join(text.split())`). `split_whitespace`
    // also folds U+2028/U+2029 (line/paragraph separator), exactly as Python's regex does.
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ASYNC STEERING (piece a): fold a DRAINED steer queue into a single
/// `SUPERVISOR STEERING:` block appended to the next round's task — REUSING the exact
/// `\n\n<HEADER>:\n<body>` append shape `build_retry_directive` uses for Censor
/// feedback. Returns None when the queue holds no actionable correction (empty, or only
/// blank/stop entries — a stop is handled by the kill path, never folded into prose), so
/// the caller can skip the append entirely (no spurious header). One line per message,
/// in FIFO order. Pure + privacy-safe (the messages are the supervisor's own prose, the
/// same trust tier as the task itself).
///
/// W3 (TRUST BOUNDARY): the steer text is supervisor/untrusted input folded into the
/// mini's next-round prompt. Its boundary is the explicit, labelled
/// `SUPERVISOR STEERING (apply these corrections this round):` HEADER below — the mini
/// reads the block as bounded supervisor guidance, not as a fresh instruction stream, and
/// each message is already sanitized (`sanitize_steer_message`: invisible/BiDi strip +
/// whitespace collapse) and per-message + aggregate length-capped. The mini's own
/// prompt-injection firewall (it treats the whole delegated task as untrusted) is the
/// BACKSTOP; this header is the labelling/bounding layer.
pub fn fold_steer_block(messages: &[String]) -> Option<String> {
    let lines: Vec<&str> = messages
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty() && !is_steer_stop(m))
        .collect();
    if lines.is_empty() {
        return None;
    }
    // P2 (AGGREGATE BLOAT CAP): assemble the body FIFO but stop at a MESSAGE BOUNDARY once
    // the running char count would exceed MAX_STEER_BLOCK_LEN, so the per-message cap can't
    // sum into a multi-thousand-char prompt balloon. Whole corrections are kept (never a
    // half-sentence); a dropped tail is flagged so the model knows steering was truncated.
    let mut body = String::new();
    let mut truncated = false;
    for m in &lines {
        let line = format!("- {m}");
        // +1 for the joining newline once the body is non-empty.
        let added = line.chars().count() + if body.is_empty() { 0 } else { 1 };
        if !body.is_empty() && body.chars().count() + added > MAX_STEER_BLOCK_LEN {
            truncated = true;
            break;
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&line);
    }
    if truncated {
        body.push_str("\n- [… additional steering corrections truncated]");
    }
    Some(format!(
        "SUPERVISOR STEERING (apply these corrections this round):\n{body}"
    ))
}

/// P6: build the PENDING retry directive appended when a predecessor goes
/// `AwaitingRetry`. Inherits the chain's root id (so the whole lineage shares one
/// root the poll watches), bumps `attempt`, unions the predecessor's declared
/// `files` with the files it actually touched (so the retry sees the full surface),
/// appends the Censor feedback to the task, and inherits backend/allowOracle/
/// parentAgentId. Fresh id/result_path/created_at supplied by the (impure) caller.
///
/// ASYNC STEERING (piece a): the predecessor's `steer_queue` is DRAINED here and folded
/// into the SAME task via [`fold_steer_block`] (the same `\n\n<HEADER>:\n<body>` append
/// shape the Censor feedback uses) — the retry STARTS with `steer_queue` empty (consumed),
/// so a steer takes effect exactly once, at this round boundary.
#[allow(clippy::too_many_arguments)]
// ---------------------------------------------------------------------------
// Pure scheduler (clock-split: caller supplies `now`)
// ---------------------------------------------------------------------------

/// The decision a single executor pass should enact, computed PURELY from the
/// current directive snapshot and a caller-supplied `now`. The impure P2
/// executor calls `plan_tick`, then (under lock) applies `apply_claim` to the
/// claimed id and `apply_timeout` to each timed-out id, then spawns/kills.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TickPlan {
    /// P6: ids of the `pending` directives to claim this pass, oldest-first. Multiple
    /// claims are allowed up to `max_concurrent`, but ONLY for directives whose declared
    /// `files[]` are mutually disjoint AND disjoint from every already-active
    /// (`Launching|Running`) directive's files — two minis must never edit the same file
    /// concurrently. An overlapping pending candidate is left unclaimed (re-evaluated next
    /// tick; oldest-first bounds its starvation). At most `max_concurrent - active` claims.
    pub claims: Vec<String>,
    /// Ids of `running` directives whose wall-clock cap has been exceeded and
    /// must be killed + transitioned to `timeout`.
    pub timeouts: Vec<String>,
    /// Ids of `launching` directives stuck past the launch cap (WARNING 4): their
    /// launch bookkeeping never completed (crash mid-launch / wedged spawn), so the
    /// executor synthesizes `failed` to release the single concurrency slot. Counted
    /// against `max_concurrent` like any other active directive, but reaped here so a
    /// stuck launch never holds the slot forever.
    pub stuck_launching: Vec<String>,
}

/// P6: normalize a file path for the multi-claim disjointness comparison. Forward-slash
/// every separator so `src\a.rs` and `src/a.rs` compare equal, and lowercase ONLY on
/// Windows (its filesystem is case-insensitive, so `SRC/A.RS` collides with `src/a.rs`
/// there; on a case-sensitive OS those are genuinely different files and must NOT be
/// folded). Used both by `plan_tick` (pass-snapshot disjointness) and by the executor's
/// under-lock re-check (`files_disjoint_from_active`), so the two never disagree.
pub(crate) fn normalize_path_for_compare(p: &str) -> String {
    let slashed = p.replace('\\', "/");
    if cfg!(windows) {
        slashed.to_lowercase()
    } else {
        slashed
    }
}

/// P6: the normalized file set of every ACTIVE (`Launching|Running`) directive — the
/// files a new claim must NOT overlap. Deliberately EXCLUDES `AwaitingRetry`: that limbo
/// state holds no concurrency slot and a retry directive legitimately shares files with
/// its AwaitingRetry predecessor, so folding AwaitingRetry into the active set would make
/// the retry permanently unclaimable.
fn active_file_set(directives: &[MiniCoderDirective]) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for d in directives.iter().filter(|d| d.status.is_active()) {
        for f in &d.files {
            set.insert(normalize_path_for_compare(f));
        }
    }
    set
}

/// P6 defense-in-depth (executor under-lock re-check): are `candidate`'s files disjoint
/// from the files of every ACTIVE (`Launching|Running`) directive in the live snapshot?
/// `AwaitingRetry` is excluded (see [`active_file_set`]). A candidate with no files
/// overlaps nothing → `true`. The executor calls this inside the locked claim closure so
/// a stale pass-snapshot can't double-claim overlapping files against now-live state.
pub(crate) fn files_disjoint_from_active(
    candidate: &MiniCoderDirective,
    directives: &[MiniCoderDirective],
) -> bool {
    let active = active_file_set(directives);
    candidate
        .files
        .iter()
        .all(|f| !active.contains(&normalize_path_for_compare(f)))
}

/// Pure scheduler. Given the directive snapshot, the current time `now`
/// (RFC3339), the per-mini wall-clock cap in seconds, and the max number of
/// concurrently-active minis, decide:
///   * which pending directive to claim (the OLDEST pending by `created_at`,
///     lexicographic tie-break on `id` for determinism); never re-claims a
///     launching/running/terminal one. NO claim is returned when the count of
///     already-active (`launching|running`) directives is already at or above
///     `max_concurrent` — the pure scheduler OWNS the one-at-a-time invariant so
///     the impure P2 caller can't accidentally over-spawn (one-at-a-time =
///     `max_concurrent: 1`, but it is a param so the executor can raise it).
///   * which running directives have exceeded the cap (measured from
///     `started_at`) and must be timed out. Timeouts are returned REGARDLESS of
///     `max_concurrent` — concurrency throttles new spawns, never the reaping of
///     blown-cap minis.
///
/// `cap_secs`/`retry_cap_secs` are clamped to at least 1: a misconfigured `0`/negative
/// cap must NOT instant-timeout a just-started directive. `cap_secs` applies to a root
/// attempt (`attempt == 0`); `retry_cap_secs` (typically the shorter
/// [`DEFAULT_RETRY_WALL_CLOCK_CAP_SECS`]) applies to a retry (`attempt >= 1`) so a stuck
/// retry is reaped sooner and the chain stays inside the poll budget (P6).
///
/// MULTI-CLAIM (P6): up to `max_concurrent - active` pending directives are claimed per
/// pass, oldest-first, but only while each candidate's `files[]` are DISJOINT from the
/// union of every already-active (`Launching|Running`) directive's files AND every claim
/// already selected this pass. `AwaitingRetry` is NOT counted as active and its files are
/// NOT in the union — so a retry directive that shares files with its AwaitingRetry
/// predecessor stays claimable. An overlapping candidate is left Pending (re-evaluated
/// next tick; oldest-first bounds its starvation).
///
/// IO-free and clock-free (the `fs_watch.rs` clock-split pattern): the caller
/// owns the real clock, so this is fully unit-testable with crafted states. A
/// running directive with a missing/unparseable `started_at` or a `now` that
/// won't parse is NOT timed out here (fail-open — the executor's own kill path
/// remains the backstop; we never time out on a clock we can't read).
///
/// BLOCKER 2: the `verdict_inflight` parameter EXCLUDES directive ids from the
/// wall-clock timeout. A directive whose deferred Censor-verdict thread is in
/// flight is still `Running` with an OLD `started_at` (its PTY is gone, but we
/// keep it Running so the verdict thread can apply its decision) — it is NOT
/// stuck, so it must never be wall-cap-timed-out. The verdict thread is itself
/// bounded (the fine-pass runner timeouts), so the exclusion can't leak. The
/// executor passes its live in-flight set; callers with no in-flight verdicts
/// pass an empty `HashSet`.
#[allow(clippy::too_many_arguments)]
pub fn plan_tick(
    directives: &[MiniCoderDirective],
    now: &str,
    cap_secs: i64,
    retry_cap_secs: i64,
    launch_cap_secs: i64,
    max_concurrent: usize,
    verdict_inflight: &std::collections::HashSet<String>,
) -> TickPlan {
    let now_dt = DateTime::parse_from_rfc3339(now.trim()).ok();
    // A 0/negative cap would instant-timeout every running directive; clamp it.
    let cap_secs = cap_secs.max(1);
    let retry_cap_secs = retry_cap_secs.max(1);
    let launch_cap_secs = launch_cap_secs.max(1);

    // Concurrency ceiling: only consider new claims while below it. Counted on the live
    // snapshot's active (`Launching|Running`) directives (NOT AwaitingRetry).
    let active_count = directives.iter().filter(|d| d.status.is_active()).count();

    // MULTI-CLAIM: walk pending directives oldest-first; claim each whose files are
    // disjoint from the active set AND every claim already chosen this pass. The union
    // seeds from the active set (Launching|Running only — see `active_file_set`).
    let mut claims: Vec<String> = Vec::new();
    if active_count < max_concurrent {
        let remaining = max_concurrent - active_count;
        let mut union = active_file_set(directives);
        // Pending candidates ordered by (created_at, id) for a deterministic pass.
        let mut pending: Vec<&MiniCoderDirective> = directives
            .iter()
            .filter(|d| d.status == MiniCoderStatus::Pending)
            .collect();
        pending.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        for d in pending {
            if claims.len() >= remaining {
                break;
            }
            let normalized: Vec<String> = d
                .files
                .iter()
                .map(|f| normalize_path_for_compare(f))
                .collect();
            // Disjoint from the union (active ∪ already-claimed-this-pass files)?
            if normalized.iter().any(|f| union.contains(f)) {
                continue; // overlaps — leave Pending, re-evaluate next tick.
            }
            claims.push(d.id.clone());
            for f in normalized {
                union.insert(f);
            }
        }
    }

    // Running directives past their wall-clock cap (root vs retry cap by `attempt`), and
    // launching directives stuck past the launch cap. Both measured against the same
    // parsed `now`; if `now` won't parse we fail-open (reap nothing — the executor's
    // startup crash-sweep is the backstop for stuck launches).
    let mut timeouts = Vec::new();
    let mut stuck_launching = Vec::new();
    if let Some(now_dt) = now_dt {
        for d in directives.iter() {
            match d.status {
                MiniCoderStatus::Running => {
                    // BLOCKER 2: a directive awaiting its deferred Censor-verdict thread is
                    // Running with a stale `started_at` but legitimately NOT stuck — never
                    // wall-cap-timeout it (the bounded verdict thread will finalize it).
                    if verdict_inflight.contains(&d.id) {
                        continue;
                    }
                    let Some(started) = d.started_at.as_deref() else {
                        continue; // no anchor -> cannot judge; fail-open.
                    };
                    let Ok(started_dt) = DateTime::parse_from_rfc3339(started.trim()) else {
                        continue; // unparseable anchor -> fail-open.
                    };
                    let elapsed = now_dt.signed_duration_since(started_dt).num_seconds();
                    // P6: a retry (attempt >= 1) is held to the shorter retry cap.
                    let effective_cap = if d.attempt >= 1 {
                        retry_cap_secs
                    } else {
                        cap_secs
                    };
                    if elapsed >= effective_cap {
                        timeouts.push(d.id.clone());
                    }
                }
                MiniCoderStatus::Launching => {
                    let Some(claimed) = d.claimed_at.as_deref() else {
                        continue; // no anchor -> the startup sweep is the backstop.
                    };
                    let Ok(claimed_dt) = DateTime::parse_from_rfc3339(claimed.trim()) else {
                        continue; // unparseable anchor -> fail-open.
                    };
                    let elapsed = now_dt.signed_duration_since(claimed_dt).num_seconds();
                    if elapsed >= launch_cap_secs {
                        stuck_launching.push(d.id.clone());
                    }
                }
                _ => {}
            }
        }
    }

    TickPlan {
        claims,
        timeouts,
        stuck_launching,
    }
}

// ---------------------------------------------------------------------------
// Bounded queue eviction
// ---------------------------------------------------------------------------

/// Cap the directive queue at `max`, evicting the OLDEST TERMINAL directives
/// first. Active directives (pending/launching/running) are NEVER evicted — they
/// represent work in flight or queued, and losing them would orphan a mini or a
/// coder's pending request. If the queue is over `max` but every excess slot is
/// active, the queue is left larger than `max` rather than dropping live work.
///
/// "Oldest" is by `created_at` (lexicographic on the RFC3339 string, which sorts
/// chronologically), tie-broken on `id` for determinism.
pub fn cap_directives(directives: &mut Vec<MiniCoderDirective>, max: usize) {
    cap_directives_protecting(directives, max, &[]);
}

/// WARNING 5 (P6 PROPAGATED-THEN-EVICTED): like [`cap_directives`] but NEVER evicts a
/// directive whose id is in `protect`, even if it is terminal and oldest. The executor
/// passes the ids it stamped terminal in THIS SAME mutate (a freshly-propagated chain
/// root + its AwaitingRetry ancestors): without this grace, a full queue could evict a
/// just-finalized root before the blocking `spawn_mini_coder` poll reads its outcome —
/// the coder would see the directive vanish (a misleading `failed`/timeout) and the
/// escalation payload would be lost. Granting one pass of grace lets the poll observe
/// the terminal result; the next cap pass (where it is no longer "just-finalized") may
/// evict it normally.
pub fn cap_directives_protecting(
    directives: &mut Vec<MiniCoderDirective>,
    max: usize,
    protect: &[String],
) {
    if directives.len() <= max {
        return;
    }
    let mut to_remove = directives.len() - max;

    // Indices of terminal directives, oldest first — EXCLUDING any protected this pass.
    let mut terminal_idx: Vec<usize> = directives
        .iter()
        .enumerate()
        .filter(|(_, d)| d.status.is_terminal() && !protect.iter().any(|p| p == &d.id))
        .map(|(i, _)| i)
        .collect();
    terminal_idx.sort_by(|&a, &b| {
        directives[a]
            .created_at
            .cmp(&directives[b].created_at)
            .then_with(|| directives[a].id.cmp(&directives[b].id))
    });

    // Mark the oldest terminal entries for removal until we've shed enough (or
    // run out of terminal entries — we never touch an active one).
    let mut remove_flags = vec![false; directives.len()];
    for &idx in terminal_idx.iter() {
        if to_remove == 0 {
            break;
        }
        remove_flags[idx] = true;
        to_remove -= 1;
    }

    // Retain by explicit index into `remove_flags` (one flag per original entry,
    // in order). `retain` visits entries front-to-back exactly once, so `i`
    // advances in lockstep with the visited element — clearer and less fragile
    // than threading an iterator through the closure.
    let mut i = 0;
    directives.retain(|_| {
        let keep = !remove_flags[i];
        i += 1;
        keep
    });
}

// ---------------------------------------------------------------------------
// Result-file reader (lenient + path-confined)
// ---------------------------------------------------------------------------

/// Reject a result path that is absolute or contains a `..` traversal component.
/// Mirrors `backend::censor::ledger::validate_rel_path`: slash-normalize first so
/// a backslash-separated `..` is caught on every OS, then walk components. We do
/// NOT reject a `-`-leading component here (unlike the censor variant) because
/// this path is never handed to a linter as an argv positional — it is only ever
/// joined under the scratch root and opened for read.
pub(crate) fn validate_result_rel_path(rel: &str) -> Result<(), String> {
    let normalized = rel.replace('\\', "/");
    let path = Path::new(&normalized);
    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(format!("result path must not contain '..': {rel}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("result path must be relative, got absolute: {rel}"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Read the result file the mini wrote, returning a terminal `MiniCoderOutcome`.
///
/// `scratch_root` is the directory the mini's `result_rel_path` is confined to;
/// the relative path is validated (`..`/absolute rejected) BEFORE any IO, and the
/// final read target is required to stay under the (lexically resolved) root.
///
/// CONTAINMENT IS LEXICAL ONLY: the `..`/absolute rejection and the
/// `starts_with(scratch_root)` check operate on the textual path. A SYMLINK that
/// lives inside `scratch_root` but points OUTSIDE it is NOT caught here — opening
/// such a path would follow the link off-root. P2 (the impure executor that owns
/// the real spawn) MUST canonicalize-after-open (compare `File`'s real path /
/// `fstat` to the root) or open with `O_NOFOLLOW` before trusting the read. The
/// `MAX_RESULT_BYTES` cap below narrows the blast radius (a symlinked huge file
/// still can't OOM us) but does NOT close the symlink-escape hole.
///
/// Lenient by contract: a valid `done`/`needs_clarification` JSON maps to that
/// outcome; a MISSING file, an UNREADABLE file, an OVERSIZED file (> 1 MiB),
/// MALFORMED/partial JSON, or an unrecognized status string ALL degrade to a
/// synthesized `failed` outcome — never a panic, never an `Err` (the executor
/// always gets a terminal verdict to stamp). A traversal/absolute path also
/// degrades to `failed` WITHOUT touching the filesystem.
pub fn read_result_file(scratch_root: &Path, result_rel_path: &str) -> MiniCoderOutcome {
    if let Err(e) = validate_result_rel_path(result_rel_path) {
        return MiniCoderOutcome::failed(format!("invalid result path: {e}"));
    }

    let normalized = result_rel_path.replace('\\', "/");
    let target: PathBuf = scratch_root.join(&normalized);

    // Defense in depth: after joining, the target's lexical prefix must still be
    // the scratch root (a symlink/component we missed cannot escape silently).
    if !target.starts_with(scratch_root) {
        return MiniCoderOutcome::failed(format!(
            "result path escapes scratch root: {result_rel_path}"
        ));
    }

    // Bounded read: open then `.take(MAX_RESULT_BYTES + 1)` so a runaway/hostile
    // result file can never OOM us. Reading one extra byte lets us distinguish a
    // file exactly at the cap (fine) from one over it (-> failed). Opening THEN
    // reading (rather than metadata-then-read) also tightens the validate->read
    // window: we read from the same handle, not a path re-resolved after a stat.
    let file = match std::fs::File::open(&target) {
        Ok(f) => f,
        Err(_) => {
            // Missing or unreadable -> the mini never produced a result.
            return MiniCoderOutcome::failed("result file missing or unreadable".to_string());
        }
    };
    let mut buf = Vec::new();
    if file
        .take(MAX_RESULT_BYTES + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return MiniCoderOutcome::failed("result file unreadable".to_string());
    }
    if buf.len() as u64 > MAX_RESULT_BYTES {
        return MiniCoderOutcome::failed("result file too large".to_string());
    }
    let raw = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return MiniCoderOutcome::failed("result file is not valid UTF-8".to_string()),
    };

    let parsed: MiniCoderResult = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(_) => {
            return MiniCoderOutcome::failed("result file is malformed JSON".to_string());
        }
    };

    match parsed.status.as_str() {
        "done" => MiniCoderOutcome::done(parsed),
        "needs_clarification" => MiniCoderOutcome::needs_clarification(parsed),
        other => {
            MiniCoderOutcome::failed(format!("result file has unrecognized status: {other:?}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Backend config (P4): the single global mini-coder backend
// ---------------------------------------------------------------------------

/// Max length of a model tag (ollama/codex). Keeps `ollama run <tag>` /
/// `codex exec -m <tag>` argv sane. Mirrors the TS `MINI_MODEL_MAX_LENGTH`.
///
/// `pub(crate)` so the design-LLM backend validator (`backend::design_llm::
/// validate_design_llm_backend`) applies the IDENTICAL cap — its provider kinds are a
/// 1:1 mirror of the mini-coder's, so the two model caps must never drift.
pub(crate) const MINI_MODEL_MAX_LEN: usize = 80;
/// Max length of the `api` CLI command line. Shares the custom-client cap; the
/// command is embedded verbatim into the launch script and fed the prompt over
/// stdin. Mirrors the TS `MINI_COMMAND_MAX_LENGTH`.
///
/// `pub(crate)` so the design-LLM backend validator reuses the same command cap.
pub(crate) const MINI_COMMAND_MAX_LEN: usize = 400;
/// Max length of the `omlx` base URL. A loopback `http://localhost:<port>/v1` is a
/// few dozen chars; 200 is generous headroom while still bounding the field that is
/// later embedded into the launch script. Mirrors the TS `MINI_BASE_URL_MAX_LENGTH`.
///
/// `pub(crate)` so the design-LLM backend validator reuses the same base-URL cap.
pub(crate) const MINI_BASE_URL_MAX_LEN: usize = 200;

/// The kind of runtime a mini-coder runs on. snake/lower over the wire to match
/// the TS `MiniCoderBackendKind` and the config.json discriminator exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MiniCoderBackendKind {
    /// Local `ollama run <model>` — text-only, no MCP/tools. `model` REQUIRED.
    Ollama,
    /// A user-provided cheap-API CLI — `command` REQUIRED (run verbatim, prompt
    /// over stdin). The API key MUST come from the CLI's own env, never argv.
    Api,
    /// The user's codex subscription via `codex exec` (one-shot, rides local
    /// auth — NOT an API key). `model` OPTIONAL; MAY get a bounded oracle grant.
    Codex,
    /// OpenAI via the hosted OpenAI API (rides a local API key from env — NOT
    /// local auth). `model` OPTIONAL; `command`/`base_url` are unused.
    Openai,
    /// A local oMLX (MLX) server exposing an OpenAI-compatible HTTP API. The mini
    /// (P2) POSTs chat-completions to `<baseUrl>/chat/completions`. `model` AND
    /// `base_url` REQUIRED; `command` is unused. The base URL is constrained to a
    /// LOOPBACK http origin (http only; privacy: the prompt never leaves the device).
    Omlx,
    /// Apple Foundation Models via the macOS `fm respond` CLI. `command`/`base_url` are
    /// ignored and `model` is optional; execution resolves the trusted `fm` binary and
    /// feeds the prompt over stdin.
    #[serde(rename = "appleFm")]
    AppleFm,
    /// CLOUD (opt-in): an HTTPS OpenAI-compatible endpoint reachable from a PUBLIC
    /// provider host (e.g. OpenRouter). `model` AND `base_url` REQUIRED; `command`
    /// is unused; the base URL is constrained to an HTTPS, NON-loopback public
    /// host (the inverse of the loopback rule — the two paths must NEVER
    /// accept/reject the same set, so the URL validation is REUSED FROM
    /// `super::local_coder::validate_cloud_base_url`, NOT duplicated here, to
    /// guarantee no drift between the orchestrator's cloud kind and this
    /// per-role/mini cloud kind).
    ///
    /// PRIVACY CONTRACT: like the orchestrator's cloud kind, Cloud sends the
    /// prompt — which may carry file content — OFF the machine to the
    /// configured provider. The host UI shows a mandatory consent disclosure
    /// before this kind can be saved.
    ///
    /// EXECUTOR STATUS: this kind is NOT yet executable by the directive
    /// executor (it runs via the pi engine, see
    /// `pi_sidecar::map_mini_coder_backend_to_sidecar_env`); the executor's
    /// `match backend.kind` arms return a clear "directive executor does not
    /// support it yet" error so a misconfigured spawn fails LOUDLY rather than
    /// silently downgrading. Future work will wire the cloud launch path.
    Cloud,
}

impl MiniCoderBackendKind {
    /// Lowercase wire token (`ollama`, `api`, `codex`, `openai`, `omlx`,
    /// `appleFm`, `cloud`). Mirrors the serde rename and the TS discriminator
    /// so the ledger / rail / log labels can show the runtime verbatim.
    /// Used by `pi_sidecar::resolve_coder_env_for_sidecar`'s
    /// non-sidecar-capable-kind diagnostic.
    pub fn kind_label(self) -> &'static str {
        match self {
            MiniCoderBackendKind::Ollama => "ollama",
            MiniCoderBackendKind::Api => "api",
            MiniCoderBackendKind::Codex => "codex",
            MiniCoderBackendKind::Openai => "openai",
            MiniCoderBackendKind::Omlx => "omlx",
            MiniCoderBackendKind::AppleFm => "appleFm",
            MiniCoderBackendKind::Cloud => "cloud",
        }
    }
}

/// The single, global mini-coder backend config persisted in config.json under
/// `miniCoderBackend`. A discriminated struct: `kind` picks the runtime and the
/// relevant field is required per kind (validated by `validate_mini_coder_backend`).
///
/// camelCase + every optional field `#[serde(default)]`/`skip_serializing_if` so a
/// config.json written by the UI (only the fields its kind uses) round-trips
/// without churn and an older/hand-edited config still parses leniently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniCoderBackend {
    pub kind: MiniCoderBackendKind,
    /// Model tag/name. Required for `ollama`, optional for `codex`, unused for
    /// `api`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The CLI command line. Required for `api`; unused for `ollama`/`codex`/`omlx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The oMLX server base URL (e.g. `http://localhost:8000/v1`). Required for
    /// `omlx`; unused for the other kinds. Validated to a LOOPBACK http origin (http only)
    /// and STORED NORMALIZED (no trailing slash) so later `<baseUrl>/chat/completions`
    /// never double-slashes (see `validate_omlx_base_url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// P6: max number of minis the executor may run CONCURRENTLY. Clamped to `1..=4`;
    /// `None` (the common case) means the default [`DEFAULT_MAX_CONCURRENT`]. NO-CHURN: a
    /// config that never set it (older/UI-omitted) round-trips without gaining a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u8>,
}

/// P6: default concurrency when `MiniCoderBackend.max_concurrent` is unset. Two minis is
/// a conservative parallelism bump over the original one-at-a-time without overwhelming a
/// local backend or the single executor loop.
pub const DEFAULT_MAX_CONCURRENT: usize = 2;

/// P6: clamp a configured `max_concurrent` into the valid `1..=4` band, preserving `None`
/// as `None` (so the default applies at read and no value is injected — no-churn). A
/// hand-edited `0` floors to 1; a `9` ceils to 4.
pub(crate) fn clamp_max_concurrent(v: Option<u8>) -> Option<u8> {
    v.map(|n| n.clamp(1, 4))
}

/// P6: the effective concurrency the executor should pass to `plan_tick` for a backend:
/// the clamped configured value, or [`DEFAULT_MAX_CONCURRENT`] when unset.
pub fn effective_max_concurrent(backend: &MiniCoderBackend) -> usize {
    backend
        .max_concurrent
        .map(|n| n.clamp(1, 4) as usize)
        .unwrap_or(DEFAULT_MAX_CONCURRENT)
}

/// Validate + normalize a backend config. Mirrors the TS `validateMiniBackend`
/// rules so the UI and backend never disagree. Trims fields and keeps ONLY the
/// field(s) the kind uses (so a kind switch never leaves a stale model/command).
/// Returns the normalized backend or a human error string.
///
/// TRUST MODEL (read before "hardening" this): the `api` command is an
/// OPERATOR-CONFIGURED, TRUSTED shell command LINE — the SAME trust model as a
/// `customAgentClients` command. It is run as a shell command line WITH THE USER'S
/// OWN PRIVILEGES, so it legitimately needs to contain arguments and may use shell
/// features (pipes, `;` on Windows, `$()`/backticks on macOS sh). We DELIBERATELY do
/// NOT block shell metacharacters here: doing so would break legitimate commands
/// (e.g. `mycli chat --json`, a wrapper that pipes/quotes) and would NOT add real
/// safety, because anyone who can set this config can already run code as the user.
/// This is by design and consistent with `customAgentClients`; reviewers should not
/// re-flag the lack of metachar filtering as an injection bug.
///
/// SECURITY (what we DO enforce): the command is embedded VERBATIM into the launch
/// script, so a control char (< 0x20), DEL (0x7f), or a Unicode bidi/invisible/format
/// char would split it into extra statements or hide its true semantics — those ARE
/// rejected here (mirrors `validate_custom_agent_client`). The model tag is
/// constrained to a bare token so it is safe as a single `ollama run <tag>` /
/// `-m <tag>` argv positional (no whitespace/metachar injection).
/// WARNING 6: a char that must NEVER appear in the verbatim `api` command line.
/// Covers Unicode Cc controls (`is_control()`, including 0x00-0x1f and DEL 0x7f)
/// PLUS the bidi-control / zero-width / invisible-format blocklist (category Cf and
/// related), which `is_control()` does NOT catch. Embedding any of these into the
/// launch line could split it into extra statements or hide its true semantics.
///
/// `pub(crate)` so Censor's oMLX base validator (`censor::gemma::validate_censor_local_ai`)
/// can apply the IDENTICAL char blocklist to its oMLX base URL — the two oMLX entry
/// points must reject exactly the same set of dangerous characters.
pub(crate) fn is_forbidden_command_char(ch: char) -> bool {
    if ch.is_control() {
        return true;
    }
    matches!(ch,
        '\u{00ad}'             // SOFT HYPHEN
        | '\u{061c}'           // ARABIC LETTER MARK
        | '\u{180e}'           // MONGOLIAN VOWEL SEPARATOR
        | '\u{200b}'..='\u{200f}' // ZERO WIDTH SPACE..RIGHT-TO-LEFT MARK
        | '\u{202a}'..='\u{202e}' // bidi embeddings + LEFT/RIGHT-TO-LEFT OVERRIDE
        | '\u{2060}'..='\u{2064}' // WORD JOINER..INVISIBLE PLUS
        | '\u{2066}'..='\u{2069}' // bidi isolates (LRI/RLI/FSI/PDI)
        | '\u{feff}'           // ZERO WIDTH NO-BREAK SPACE / BOM
    )
}

/// A model tag must be a bare token: first char alnum, rest in `[A-Za-z0-9._:/-]`. No
/// whitespace/control/metachars, so it is safe as a single argv positional. Mirrors the
/// TS MODEL_PATTERN.
///
/// `pub(crate)` so Censor's oMLX model validator (`censor::gemma::validate_censor_local_ai`)
/// can apply the IDENTICAL char-class — every oMLX model id on this machine (mini Rust /
/// Censor Rust / both TS) must satisfy the same bare-token rule.
pub(crate) fn is_valid_model(model: &str) -> bool {
    let mut chars = model.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    model
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '/' | '-'))
}

/// Is an optional `:port` component valid? `None` (no port) is fine; a present port must
/// be 1-5 digits and parse to <= 65535. An EMPTY port (`host:`) is rejected. Shared by
/// BOTH oMLX validators (this module's [`validate_omlx_base_url`] and Censor's
/// `censor::gemma::is_loopback_omlx_base`) so the two oMLX paths on this machine apply the
/// SAME port rule. Mirrors the TS `isValidOptionalPort`; keep all three byte-for-byte
/// equivalent. Deliberately NOT used on the Ollama path (`is_loopback_base`), which keeps
/// its long-standing port-agnostic behavior.
pub(crate) fn is_valid_optional_port(port: Option<&str>) -> bool {
    match port {
        None => true,
        Some(p) => {
            !p.is_empty()
                && p.len() <= 5
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().map(|n| n <= 65535).unwrap_or(false)
        }
    }
}

/// Validate + NORMALIZE an oMLX base URL, returning the normalized form (trailing
/// slash stripped) or a human error string.
///
/// LOOPBACK-ONLY by design (privacy): the mini POSTs the prompt — which may carry
/// file content — to this URL, so any non-loopback host could route it off the
/// machine. The loopback notion is the SAME as Censor's `backend::censor::gemma::
/// is_loopback_base` (localhost / 127.0.0.0/8 parsed as Ipv4Addr / `[::1]`; userinfo
/// `user@host` and the `127.0.0.1.evil.com` suffix trick are both rejected because the
/// host must PARSE as a loopback addr, not merely start with `127.`). oMLX is HTTP-ONLY
/// on loopback (like Ollama): a self-signed TLS cert on a loopback oMLX server would make
/// reqwest's default verification reject the connection and silently disable the tier, so
/// `https://` is rejected here. This validator MIRRORS `censor::gemma::is_loopback_omlx_base`
/// (same host rule, same http-only scheme, same `:port` rule via [`is_valid_optional_port`]).
/// If the two ever diverge, reconcile them — every oMLX base on this machine must agree.
///
/// Also enforced: a length cap, and rejection of any control/invisible/bidi char
/// (mirrors the `api` command check) since the URL is later embedded into the launch
/// script verbatim. The returned URL is the trimmed input with any trailing `/`
/// stripped so `<baseUrl>/chat/completions` never double-slashes.
///
/// `pub(crate)` so the design-LLM backend validator (`backend::design_llm`) reuses the
/// EXACT same loopback/http-only/port/char rules for its oMLX base URL — the design and
/// mini-coder oMLX surfaces must accept/reject precisely the same set, no drift. The
/// only difference is the design validator prefixes the returned error with "design"
/// wording; the accept/reject SET is identical because it routes through this fn.
pub(crate) fn validate_omlx_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("oMLX mini-coder backend requires a base URL.".into());
    }
    // Byte length (NOT `chars().count()`) so the accept/reject boundary matches the
    // TS validator's `.length` for ASCII loopback URLs (F4 parity). Non-ASCII can never
    // reach here as a valid URL (the host check rejects it), so byte/UTF-16 divergence
    // is moot; byte length keeps the two sides genuinely consistent.
    if trimmed.len() > MINI_BASE_URL_MAX_LEN {
        return Err(format!(
            "oMLX base URL must be at most {MINI_BASE_URL_MAX_LEN} characters."
        ));
    }
    // The URL is embedded verbatim into the launch line; reject the SAME control/
    // bidi/invisible blocklist as the `api` command (WARNING 6).
    if trimmed.chars().any(is_forbidden_command_char) {
        return Err("oMLX base URL must not contain control, bidi or invisible characters.".into());
    }

    // Scheme: http only (loopback, like Ollama). `https://` is rejected: a self-signed
    // TLS cert on a loopback oMLX server would fail reqwest's default verification and
    // silently disable the tier. Everything after the scheme is the authority
    // (+ optional path/query/fragment).
    let rest = if let Some(r) = trimmed.strip_prefix("http://") {
        r
    } else {
        return Err("oMLX base URL must start with http:// (loopback, http only)".into());
    };

    // Authority = everything up to the first path/query/fragment delimiter. The host
    // rule below MIRRORS `censor::gemma::is_loopback_base`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("oMLX base URL must include a loopback host.".into());
    }

    let is_loopback = if let Some(after) = authority.strip_prefix("[::1]") {
        // IPv6 loopback `[::1]` optionally followed by `:port`. Reject a userinfo trick
        // (`[::1]:8000@evil.com` / `[::1]:@evil.com`): an `@` in the remainder means the
        // real host is after the `@`, not the loopback addr (F1).
        !after.contains('@')
            && (after.is_empty() || after.starts_with(':'))
            // `after` is either "" or ":<port>"; validate the port when present (F2).
            && is_valid_optional_port(after.strip_prefix(':'))
    } else if authority.contains('@') {
        // Reject a userinfo trick (`127.0.0.1@evil.com`): real host is after the `@`.
        false
    } else {
        // Split off an optional `:port`; IPv4/hostname hosts have no `:` in the host.
        let mut parts = authority.splitn(2, ':');
        let host = parts.next().unwrap_or("");
        let host_is_loopback = host == "localhost"
            || host
                .parse::<std::net::Ipv4Addr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false);
        // `parts.next()` is the port when an `:` was present; validate it (F2).
        host_is_loopback && is_valid_optional_port(parts.next())
    };
    if !is_loopback {
        return Err(
            "oMLX base URL host must be loopback (localhost, 127.0.0.1 or [::1]) with a valid optional :port.".into(),
        );
    }

    // Normalize: strip a single trailing slash so `<baseUrl>/chat/completions` is
    // clean. Only the trailing slash on the whole URL is stripped (e.g.
    // `http://localhost:8000/v1/` -> `http://localhost:8000/v1`); a bare
    // `http://localhost:8000/` -> `http://localhost:8000`.
    let normalized = trimmed.strip_suffix('/').unwrap_or(trimmed).to_string();
    Ok(normalized)
}

pub fn validate_mini_coder_backend(backend: &MiniCoderBackend) -> Result<MiniCoderBackend, String> {
    let model = backend
        .model
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let command = backend
        .command
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    match backend.kind {
        MiniCoderBackendKind::Ollama => {
            if model.is_empty() {
                return Err("Ollama mini-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Ollama,
                model: Some(model),
                command: None,
                base_url: None,
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::Api => {
            if command.is_empty() {
                return Err("API mini-coder backend requires a command line.".into());
            }
            if command.chars().count() > MINI_COMMAND_MAX_LEN {
                return Err(format!(
                    "Mini-coder command must be at most {MINI_COMMAND_MAX_LEN} characters."
                ));
            }
            // WARNING 6: the command is embedded VERBATIM into a PowerShell/sh launch
            // line, so any control or invisible/format char must be rejected at the
            // boundary. `is_control()` covers 0x00-0x1f, DEL (0x7f) and the Unicode Cc
            // category — but bidi overrides and zero-width separators are category Cf
            // (NOT Cc), so `is_control()` alone misses them. We additionally reject the
            // standard bidi/invisible/format blocklist so a right-to-left override or a
            // zero-width char cannot smuggle hidden semantics into the launch line.
            if command.chars().any(is_forbidden_command_char) {
                return Err(
                    "Mini-coder command must not contain control, bidi or invisible characters."
                        .into(),
                );
            }
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Api,
                model: None,
                command: Some(command),
                base_url: None,
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::Codex => {
            // model is OPTIONAL for codex; validate only if provided.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Codex,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::Openai => {
            // model is OPTIONAL for openai; validate only if provided.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Openai,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::AppleFm => {
            // AppleFm keeps the same model guardrail as oMLX (bare token), but is OPTIONAL.
            // Any configured command/base_url are dropped because execution always resolves
            // the fixed Apple `fm respond` CLI on macOS.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::AppleFm,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::Omlx => {
            // omlx requires BOTH a model (a bare tag, same rule as ollama) and a
            // loopback http (only) base URL. `command` is ignored/dropped.
            if model.is_empty() {
                return Err("oMLX mini-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("oMLX mini-coder backend requires a base URL.".into());
            }
            let normalized_base = validate_omlx_base_url(&base_url)?;
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Omlx,
                model: Some(model),
                command: None,
                base_url: Some(normalized_base),
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
        MiniCoderBackendKind::Cloud => {
            // Cloud mini-coder backend: same HTTPS-NON-loopback rule as the
            // orchestrator's `LocalCoderBackendKind::Cloud` — REUSE the SAME
            // validator (`super::local_coder::validate_cloud_base_url`) so the
            // two surfaces can NEVER drift on accept/reject. `command` is
            // ignored/dropped (cloud is HTTP only, no shell command line).
            //
            // PRIVACY: cloud sends the prompt OFF the machine — the host UI
            // surfaces a consent disclosure before this kind can be saved (a
            // separate UI concern, not enforced here).
            //
            // API KEY: deliberately NOT a field on this struct — it lives
            // ONLY in the OS vault under `provider:cloud_llm` (the SAME vault
            // key the orchestrator's cloud kind uses) and is read at the
            // sidecar spawn site into the `OPENROUTER_API_KEY` env var. Key
            // PRESENCE is enforced at the spawn layer (where the vault is
            // reachable), not in this pure config-shape validator.
            if model.is_empty() {
                return Err("Cloud mini-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Mini-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Mini-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("Cloud mini-coder backend requires a base URL.".into());
            }
            // REUSE — NOT duplicate — the local_coder cloud validator. The
            // accept/reject set MUST be byte-for-byte identical so the two
            // surfaces never disagree on what is a public-provider host vs a
            // loopback / IP literal / intranet.
            let normalized_base = super::local_coder::validate_cloud_base_url(&base_url)?;
            Ok(MiniCoderBackend {
                kind: MiniCoderBackendKind::Cloud,
                model: Some(model),
                command: None,
                base_url: Some(normalized_base),
                max_concurrent: clamp_max_concurrent(backend.max_concurrent),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Launcher trait (real impl supplied by P2; tests supply a fake)
// ---------------------------------------------------------------------------

/// Spawns the actual one-shot mini process and returns the spawned agent id. P1
/// tests implement it with a fake that records the call and returns a canned id (or
/// an error to exercise the failure path). NOTE (MC-P2): the real executor in
/// `mini_coder_executor.rs` spawns the PTY DIRECTLY (it needs the AppHandle +
/// AgentPtySessions, which this trait deliberately keeps out), so this trait stays a
/// test seam for the pure-glue lifecycle tests rather than the production launcher.
#[allow(dead_code)] // test seam; the P2 executor spawns the PTY directly (see module doc).
pub trait MiniLauncher {
    /// Launch the mini for `directive`. On success returns the spawned mini's
    /// `agent_id`; on failure returns a human-readable error the executor turns
    /// into a `failed` outcome.
    fn launch(&self, directive: &MiniCoderDirective) -> Result<String, String>;
}

#[cfg(test)]
mod tests {
    use super::MiniCoderOutcome;

    #[test]
    fn outcome_censor_findings_defaults_to_none() {
        let outcome = MiniCoderOutcome::default();
        assert!(outcome.censor_findings.is_none());
    }

    #[test]
    fn outcome_censor_findings_skips_when_none_in_json() {
        let outcome = MiniCoderOutcome::default();
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(
            !json.contains("censorFindings"),
            "None must be omitted from JSON"
        );
    }

    #[test]
    fn outcome_censor_findings_some_round_trips() {
        let outcome = MiniCoderOutcome {
            censor_findings: Some(vec![crate::backend::censor::schema::Finding {
                id: "abc".into(),
                file: "src/x.rs".into(),
                title: "unused var".into(),
                severity: crate::backend::censor::schema::Severity::High,
                category: crate::backend::censor::schema::Category::Correctness,
                source: "clippy".into(),
                disposition: crate::backend::censor::schema::Disposition::Open,
                ..Default::default()
            }]),
            ..Default::default()
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("censorFindings"), "Some must appear in JSON");
        assert!(json.contains("unused var"), "finding title must be in JSON");

        let back: MiniCoderOutcome = serde_json::from_str(&json).unwrap();
        let findings = back.censor_findings.unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "unused var");
    }

    #[test]
    fn outcome_censor_findings_respects_skip_serializing_when_none() {
        let outcome = MiniCoderOutcome::default();
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(
            !json.contains("censorFindings"),
            "None must be absent from JSON"
        );
    }

    use super::*;
    use std::cell::RefCell;
    use std::io::Write;

    // E1 — the user-facing write-behavior policy.
    #[test]
    fn mini_write_behavior_default_is_auto() {
        assert_eq!(MiniWriteBehavior::default(), MiniWriteBehavior::Auto);
    }

    #[test]
    fn mini_write_behavior_wire_tokens_are_camel_case() {
        // The TS `MiniWriteBehavior` + config.json discriminator must match these.
        assert_eq!(
            serde_json::to_string(&MiniWriteBehavior::Safe).unwrap(),
            "\"safe\""
        );
        assert_eq!(
            serde_json::to_string(&MiniWriteBehavior::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&MiniWriteBehavior::AgenticAllowed).unwrap(),
            "\"agenticAllowed\""
        );
    }

    #[test]
    fn mini_write_behavior_round_trips() {
        for v in [
            MiniWriteBehavior::Safe,
            MiniWriteBehavior::Auto,
            MiniWriteBehavior::AgenticAllowed,
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: MiniWriteBehavior = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v, "round-trip for {v:?}");
        }
    }

    #[test]
    fn mini_write_behavior_no_churn_predicate() {
        // `is_auto_write_behavior` is the skip_serializing_if guard: ONLY Auto omits.
        assert!(is_auto_write_behavior(&MiniWriteBehavior::Auto));
        assert!(!is_auto_write_behavior(&MiniWriteBehavior::Safe));
        assert!(!is_auto_write_behavior(&MiniWriteBehavior::AgenticAllowed));
    }

    fn directive(id: &str, status: MiniCoderStatus, created_at: &str) -> MiniCoderDirective {
        MiniCoderDirective {
            id: id.into(),
            parent_agent_id: "coder-1".into(),
            status,
            write: false,
            write_mode: WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: None,
            task: "docstring foo()".into(),
            files: vec!["src/a.rs".into()],
            backend: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: Vec::new(),
            result_path: format!("mini/{id}.json"),
            agent_id: None,
            created_at: created_at.into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            stuck_report: None,
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        }
    }

    // -- serde --------------------------------------------------------------

    #[test]
    fn directive_round_trip_uses_camel_case() {
        let d = MiniCoderDirective {
            id: "d1".into(),
            parent_agent_id: "coder-1".into(),
            status: MiniCoderStatus::Running,
            task: "t".into(),
            write: false,
            write_mode: WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: None,
            files: vec!["src/a.rs".into()],
            backend: Some("ollama".into()),
            allow_oracle: true,
            kill_requested: true,
            steer_queue: Vec::new(),
            result_path: "mini/d1.json".into(),
            agent_id: Some("mini-coder1-abcd1234".into()),
            created_at: "2026-06-06T00:00:00Z".into(),
            claimed_at: Some("2026-06-06T00:00:00Z".into()),
            scratch_path: Some("/proj/.aspis-mini".into()),
            started_at: Some("2026-06-06T00:00:01Z".into()),
            result: None,
            stuck_report: None,
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"claimedAt\""), "json: {json}");
        assert!(json.contains("\"scratchPath\""), "json: {json}");
        assert!(json.contains("\"parentAgentId\""), "json: {json}");
        assert!(json.contains("\"allowOracle\""), "json: {json}");
        assert!(json.contains("\"resultPath\""), "json: {json}");
        assert!(json.contains("\"agentId\""), "json: {json}");
        assert!(json.contains("\"createdAt\""), "json: {json}");
        assert!(json.contains("\"startedAt\""), "json: {json}");
        assert!(!json.contains("parent_agent_id"), "snake leaked: {json}");
        // status snake_case over the wire.
        assert!(json.contains("\"status\":\"running\""), "json: {json}");
        // Slice 3 NO-CHURN: a directive that never went through Pigeon (pigeon_ticket None)
        // OMITS the field entirely — byte-identical to before this slice.
        assert!(
            !json.contains("pigeonTicket"),
            "None ticket must be omitted: {json}"
        );
        // F-E NO-CHURN: `collected: None` (never stamped by the Python
        // `mini_coder_result` co-writer) OMITS the field entirely.
        assert!(
            !json.contains("collected"),
            "None collected must be omitted: {json}"
        );

        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn collected_round_trips_as_camel_case_when_present() {
        // F-E hardening: the Python `mini_coder_result` co-writer stamps
        // `collected: true` on a directive once its terminal outcome has been
        // successfully handed back — the Rust struct must round-trip it
        // verbatim (this is the CO-WRITER hazard: an unknown field silently
        // dropped by a Rust re-serialize would erase the Python stamp).
        let mut d = directive("d-collected", MiniCoderStatus::Done, "2026-07-03T00:00:00Z");
        d.collected = Some(true);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"collected\":true"), "json: {json}");
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back.collected, Some(true));
        assert_eq!(d, back);
    }

    #[test]
    fn python_shaped_directive_with_collected_key_round_trips_without_dropping_it() {
        // CO-WRITER WARNING guard: deserialize a directive shaped exactly like the
        // Python `.aspis-agents.json` writer emits it (camelCase, `collected` present)
        // and confirm a Rust round-trip preserves the key rather than silently
        // dropping it (the exact hazard `deny_unknown_fields`-free structs create
        // when a NEW co-writer field has no matching Rust field).
        let raw = serde_json::json!({
            "id": "d-py",
            "parentAgentId": "codex",
            "status": "done",
            "task": "x",
            "files": ["src/a.rs"],
            "resultPath": "mini/d-py.json",
            "createdAt": "2026-07-03T00:00:00Z",
            "collected": true,
        });
        let d: MiniCoderDirective = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(d.collected, Some(true));
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains("\"collected\":true"),
            "the stamp must survive a Rust round-trip: {json}"
        );
    }

    #[test]
    fn pigeon_ticket_round_trips_as_camel_case_when_present() {
        // Slice 3: when the directive arrived via Pigeon, the ticket is carried as the
        // camelCase `pigeonTicket` and survives a round-trip (the Python co-owner reads it).
        let mut d = directive("d-pigeon", MiniCoderStatus::Pending, "2026-06-26T00:00:00Z");
        d.pigeon_ticket = Some(42);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"pigeonTicket\":42"), "json: {json}");
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pigeon_ticket, Some(42));
        assert_eq!(d, back);
    }

    #[test]
    fn status_snake_case_strings_match_plan() {
        assert_eq!(
            serde_json::to_string(&MiniCoderStatus::NeedsClarification).unwrap(),
            "\"needs_clarification\""
        );
        assert_eq!(
            serde_json::to_string(&MiniCoderStatus::AbortedByHuman).unwrap(),
            "\"aborted_by_human\""
        );
        for (s, tok) in [
            (MiniCoderStatus::Pending, "pending"),
            (MiniCoderStatus::Launching, "launching"),
            (MiniCoderStatus::Running, "running"),
            (MiniCoderStatus::Done, "done"),
            (MiniCoderStatus::Failed, "failed"),
            (MiniCoderStatus::Timeout, "timeout"),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{tok}\""));
            let back: MiniCoderStatus = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn directive_partial_json_defaults_everything() {
        // The leanest pending directive a writer could emit.
        let json = r#"{ "id": "d1", "task": "x", "resultPath": "mini/d1.json" }"#;
        let d: MiniCoderDirective = serde_json::from_str(json).unwrap();
        assert_eq!(d.id, "d1");
        assert_eq!(d.status, MiniCoderStatus::Pending); // enum default
        assert!(d.files.is_empty());
        assert_eq!(d.backend, None);
        assert!(!d.allow_oracle);
        assert_eq!(d.agent_id, None);
        assert_eq!(d.started_at, None);
        assert_eq!(d.result, None);
    }

    #[test]
    fn directive_missing_status_defaults_pending() {
        let json = r#"{ "id": "d1" }"#;
        let d: MiniCoderDirective = serde_json::from_str(json).unwrap();
        assert_eq!(d.status, MiniCoderStatus::Pending);
    }

    #[test]
    fn empty_pending_collections_not_serialized_no_churn() {
        let d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        let mut d = d;
        d.files.clear();
        let json = serde_json::to_string(&d).unwrap();
        // Empty files + None backend/agentId/startedAt/result must be absent.
        assert!(!json.contains("\"files\""), "json: {json}");
        assert!(!json.contains("\"backend\""), "json: {json}");
        assert!(!json.contains("\"agentId\""), "json: {json}");
        assert!(!json.contains("\"startedAt\""), "json: {json}");
        assert!(!json.contains("\"result\""), "json: {json}");
    }

    #[test]
    fn write_mode_default_emit_edits_omitted_no_churn() {
        // A1 NO-CHURN: an EmitEdits (default) directive must serialize BYTE-
        // IDENTICALLY to today — no `"writeMode"` key injected — and must
        // deserialize back to EmitEdits when the key is absent.
        let d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        assert_eq!(d.write_mode, WriteMode::EmitEdits, "default is EmitEdits");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains("writeMode"),
            "default writeMode must be omitted (no churn): {json}"
        );

        // Absent in JSON -> EmitEdits.
        let from_absent: MiniCoderDirective =
            serde_json::from_str(r#"{ "id": "d1", "task": "x", "resultPath": "mini/d1.json" }"#)
                .unwrap();
        assert_eq!(from_absent.write_mode, WriteMode::EmitEdits);
    }

    #[test]
    fn directive_tier_default_mini_omitted_no_churn() {
        // ROLE UNTANGLE Phase 3 NO-CHURN: a Mini (default) directive serializes
        // BYTE-IDENTICALLY to today — no `"tier"` key injected — and an absent
        // key deserializes back to Mini.
        let d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        assert_eq!(d.tier, DirectiveTier::Mini, "default tier is Mini");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains("tier"),
            "default tier must be omitted (no churn): {json}"
        );
        let from_absent: MiniCoderDirective =
            serde_json::from_str(r#"{ "id": "d1", "task": "x", "resultPath": "mini/d1.json" }"#)
                .unwrap();
        assert_eq!(from_absent.tier, DirectiveTier::Mini);
    }

    #[test]
    fn directive_tier_main_round_trips_with_wire_string() {
        // The Python co-writer (spawn_main_coder) emits `"tier": "main"`.
        let mut d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        d.write = true;
        d.write_mode = WriteMode::AgenticIterative;
        d.tier = DirectiveTier::Main;
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains("\"tier\":\"main\""),
            "expected the main wire string: {json}"
        );
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tier, DirectiveTier::Main);
        assert_eq!(d, back);
        // Standalone wire strings — the exact contract the Python co-writer mirrors.
        assert_eq!(
            serde_json::to_string(&DirectiveTier::Mini).unwrap(),
            "\"mini\""
        );
        assert_eq!(
            serde_json::to_string(&DirectiveTier::Main).unwrap(),
            "\"main\""
        );
    }

    #[test]
    fn write_mode_agentic_iterative_round_trips_with_camel_case_wire() {
        // A1 wire string: camelCase to match the directive's serde convention.
        let mut d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        d.write = true;
        d.write_mode = WriteMode::AgenticIterative;
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            json.contains("\"writeMode\":\"agenticIterative\""),
            "expected camelCase wire string: {json}"
        );
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back.write_mode, WriteMode::AgenticIterative);
        assert_eq!(d, back);

        // The standalone enum wire strings are exactly the contract A2 mirrors.
        assert_eq!(
            serde_json::to_string(&WriteMode::EmitEdits).unwrap(),
            "\"emitEdits\""
        );
        assert_eq!(
            serde_json::to_string(&WriteMode::AgenticIterative).unwrap(),
            "\"agenticIterative\""
        );
    }

    #[test]
    fn outcome_round_trip_and_error_field() {
        let o = MiniCoderOutcome::failed("result file missing");
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.contains("\"status\":\"failed\""), "json: {json}");
        assert!(
            json.contains("\"error\":\"result file missing\""),
            "json: {json}"
        );
        let back: MiniCoderOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);

        // A clean done outcome has NO error key (no churn).
        let done = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            output: Some("ok".into()),
            files_touched: vec!["src/a.rs".into()],
            ..Default::default()
        });
        let djson = serde_json::to_string(&done).unwrap();
        assert!(!djson.contains("\"error\""), "json: {djson}");
        assert!(djson.contains("\"filesTouched\""), "json: {djson}");
    }

    // -- transitions --------------------------------------------------------

    #[test]
    fn lifecycle_pending_to_done() {
        let d = directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        let launching = apply_claim(&d, "2026-06-06T00:00:00Z").unwrap();
        assert_eq!(launching.status, MiniCoderStatus::Launching);
        assert_eq!(
            launching.claimed_at.as_deref(),
            Some("2026-06-06T00:00:00Z")
        );

        let running =
            apply_launched(&launching, "mini-coder1-abcd1234", "2026-06-06T00:00:01Z").unwrap();
        assert_eq!(running.status, MiniCoderStatus::Running);
        assert_eq!(running.agent_id.as_deref(), Some("mini-coder1-abcd1234"));
        assert_eq!(running.started_at.as_deref(), Some("2026-06-06T00:00:01Z"));

        let outcome = MiniCoderOutcome::done(MiniCoderResult {
            status: "done".into(),
            output: Some("done it".into()),
            files_touched: vec!["src/a.rs".into()],
            ..Default::default()
        });
        let done = apply_result(&running, outcome).unwrap();
        assert_eq!(done.status, MiniCoderStatus::Done);
        assert_eq!(done.result.as_ref().unwrap().status, MiniCoderStatus::Done);
    }

    #[test]
    fn claim_of_non_pending_is_err_no_double_claim() {
        let launching = directive("d1", MiniCoderStatus::Launching, "t");
        assert!(apply_claim(&launching, "t").is_err());
        let running = directive("d1", MiniCoderStatus::Running, "t");
        assert!(apply_claim(&running, "t").is_err());
        let done = directive("d1", MiniCoderStatus::Done, "t");
        assert!(apply_claim(&done, "t").is_err());
    }

    #[test]
    fn launched_requires_launching() {
        let pending = directive("d1", MiniCoderStatus::Pending, "t");
        assert!(apply_launched(&pending, "x", "t").is_err());
        let running = directive("d1", MiniCoderStatus::Running, "t");
        assert!(apply_launched(&running, "x", "t").is_err());
    }

    #[test]
    fn synthesized_terminal_paths() {
        let running = directive("d1", MiniCoderStatus::Running, "t");
        assert_eq!(
            apply_timeout(&running, "cap exceeded").unwrap().status,
            MiniCoderStatus::Timeout
        );
        assert_eq!(
            apply_aborted(&running, "human stop").unwrap().status,
            MiniCoderStatus::AbortedByHuman
        );
        assert_eq!(
            apply_failed(&running, "no file").unwrap().status,
            MiniCoderStatus::Failed
        );
    }

    #[test]
    fn terminal_directive_cannot_be_overwritten() {
        // A kill that already won (aborted_by_human) must not be clobbered by a
        // late result. apply_* refuses to transition a terminal directive.
        let aborted = directive("d1", MiniCoderStatus::AbortedByHuman, "t");
        assert!(
            apply_result(&aborted, MiniCoderOutcome::done(MiniCoderResult::default())).is_err()
        );
        assert!(apply_timeout(&aborted, "x").is_err());
        assert!(apply_failed(&aborted, "x").is_err());
    }

    #[test]
    fn outcome_on_pending_is_err() {
        // BLOCKER 1: a Pending directive must NOT be transitioned straight to a
        // terminal state — it has to go through claim -> launch -> run first.
        // Every apply_* terminal helper must reject a Pending directive.
        let pending = directive("d1", MiniCoderStatus::Pending, "t");
        assert!(
            apply_result(&pending, MiniCoderOutcome::done(MiniCoderResult::default())).is_err(),
            "Pending -> done must be rejected"
        );
        assert!(
            apply_timeout(&pending, "x").is_err(),
            "Pending -> timeout must be rejected"
        );
        assert!(
            apply_aborted(&pending, "x").is_err(),
            "Pending -> aborted must be rejected"
        );
        assert!(
            apply_failed(&pending, "x").is_err(),
            "Pending -> failed must be rejected"
        );

        // But the legitimate active->terminal paths still succeed:
        // spawn-error path Launching -> failed.
        let launching = directive("d2", MiniCoderStatus::Launching, "t");
        assert_eq!(
            apply_failed(&launching, "spawn error").unwrap().status,
            MiniCoderStatus::Failed
        );
        // Running -> done.
        let running = directive("d3", MiniCoderStatus::Running, "t");
        assert_eq!(
            apply_result(&running, MiniCoderOutcome::done(MiniCoderResult::default()))
                .unwrap()
                .status,
            MiniCoderStatus::Done
        );
    }

    #[test]
    fn stale_snapshot_double_claim_is_rejected() {
        // Two executor passes both observe the SAME directive as Pending in their
        // (stale) snapshots. Executor A claims it (-> Launching) against live
        // state. Executor B then applies its claim against the NOW-Launching live
        // state and must fail: no double-claim, no double-spawn.
        let live_pending = directive("d1", MiniCoderStatus::Pending, "t");

        // Both snapshots see Pending.
        let snapshot_a = live_pending.clone();
        let snapshot_b = live_pending.clone();
        assert_eq!(snapshot_a.status, MiniCoderStatus::Pending);
        assert_eq!(snapshot_b.status, MiniCoderStatus::Pending);

        // A claims against live Pending -> succeeds, live becomes Launching.
        let live_after_a = apply_claim(&live_pending, "2026-06-06T00:00:00Z").unwrap();
        assert_eq!(live_after_a.status, MiniCoderStatus::Launching);

        // B applies its claim against the NOW-live (Launching) state -> Err.
        assert!(
            apply_claim(&live_after_a, "2026-06-06T00:00:01Z").is_err(),
            "second claim against now-Launching live state must be rejected"
        );
    }

    // -- no-churn serde co-ownership ----------------------------------------

    #[test]
    fn python_shaped_directive_round_trips_without_extra_keys() {
        // A directive the Python MCP writer emits carries ONLY the keys it sets;
        // a Rust (de)serialize cycle must NOT inject defaults (allowOracle:false,
        // empty strings, etc) that would churn the file under the co-owner.
        let python_json = r#"{
            "id": "d1",
            "parentAgentId": "coder-1",
            "status": "pending",
            "task": "docstring foo()",
            "files": ["src/a.rs"],
            "resultPath": "mini/d1.json",
            "createdAt": "2026-06-06T00:00:00Z"
        }"#;
        let parsed: serde_json::Value = serde_json::from_str(python_json).unwrap();
        let original_keys: std::collections::BTreeSet<String> =
            parsed.as_object().unwrap().keys().cloned().collect();

        let d: MiniCoderDirective = serde_json::from_str(python_json).unwrap();
        let reser: serde_json::Value = serde_json::to_value(&d).unwrap();
        let reser_keys: std::collections::BTreeSet<String> =
            reser.as_object().unwrap().keys().cloned().collect();

        assert_eq!(
            original_keys, reser_keys,
            "Rust round-trip changed the key set (churn): {reser_keys:?}"
        );
        // Specifically: the injected-default offenders must be absent.
        assert!(
            !reser_keys.contains("allowOracle"),
            "allowOracle injected: {reser_keys:?}"
        );
        assert!(!reser_keys.contains("agentId"));
        assert!(!reser_keys.contains("startedAt"));
        assert!(!reser_keys.contains("result"));
        assert!(!reser_keys.contains("backend"));
    }

    #[test]
    fn kill_requested_round_trips_camel_case_and_skips_when_false() {
        // P5: killRequested round-trips as camelCase when true; is OMITTED (no churn)
        // when false so a Python-written directive that never set it round-trips clean.
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.kill_requested = true;
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"killRequested\":true"), "json: {json}");
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert!(back.kill_requested);

        // False -> skipped (no-churn), and a directive missing the key defaults false.
        let mut d2 = directive("d2", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        d2.kill_requested = false;
        let json2 = serde_json::to_string(&d2).unwrap();
        assert!(!json2.contains("killRequested"), "json2: {json2}");
        let from_python: MiniCoderDirective =
            serde_json::from_str(r#"{ "id": "d3", "task": "x", "resultPath": "m.json" }"#).unwrap();
        assert!(!from_python.kill_requested);
    }

    #[test]
    fn steer_queue_round_trips_camel_case_and_skips_when_empty() {
        // ASYNC STEERING (a): steerQueue round-trips as camelCase when non-empty; is
        // OMITTED (no churn) when empty so a Python-written directive that was never
        // steered round-trips byte-identically — exactly the kill_requested pattern.
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.steer_queue = vec!["use a HashMap not a Vec".into(), "add a test".into()];
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"steerQueue\""), "json: {json}");
        assert!(json.contains("use a HashMap not a Vec"), "json: {json}");
        let back: MiniCoderDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steer_queue, d.steer_queue);

        // Empty -> skipped (no-churn), and a directive missing the key defaults empty.
        let d2 = directive("d2", MiniCoderStatus::Pending, "2026-06-06T00:00:00Z");
        assert!(d2.steer_queue.is_empty());
        let json2 = serde_json::to_string(&d2).unwrap();
        assert!(!json2.contains("steerQueue"), "json2: {json2}");
        let from_python: MiniCoderDirective =
            serde_json::from_str(r#"{ "id": "d3", "task": "x", "resultPath": "m.json" }"#).unwrap();
        assert!(from_python.steer_queue.is_empty());
    }

    #[test]
    fn fold_steer_block_builds_supervisor_block_or_none() {
        // Non-empty actionable messages -> a single SUPERVISOR STEERING block, FIFO order.
        let block = fold_steer_block(&["first fix".into(), "second fix".into()]).unwrap();
        assert!(block.starts_with("SUPERVISOR STEERING"), "block: {block}");
        assert!(block.contains("- first fix"), "block: {block}");
        assert!(block.contains("- second fix"), "block: {block}");
        // first must precede second (FIFO).
        assert!(
            block.find("first fix").unwrap() < block.find("second fix").unwrap(),
            "FIFO order: {block}"
        );

        // Empty queue, blank-only, and stop-only all fold to None (no spurious header;
        // the stop sentinel routes to the kill path, never into the prose).
        assert!(fold_steer_block(&[]).is_none());
        assert!(fold_steer_block(&["   ".into(), "".into()]).is_none());
        assert!(fold_steer_block(&["stop".into(), "  STOP  ".into()]).is_none());
        // A mix keeps the real correction, drops the stop.
        let mixed = fold_steer_block(&["stop".into(), "real correction".into()]).unwrap();
        assert!(mixed.contains("- real correction"), "mixed: {mixed}");
        assert!(
            !mixed.contains("- stop"),
            "stop must not be folded: {mixed}"
        );
    }
    #[test]
    fn fold_steer_block_caps_aggregate_size() {
        // P2 (AGGREGATE BLOAT CAP): a full queue of max-length messages must NOT fold into a
        // multi-thousand-char block. With MAX_STEER_QUEUE_LEN messages each at
        // MAX_STEER_MESSAGE_LEN, the naive sum is ~16 000 chars; the cap bounds the body.
        let big = "x".repeat(MAX_STEER_MESSAGE_LEN);
        let queue: Vec<String> = (0..MAX_STEER_QUEUE_LEN).map(|_| big.clone()).collect();
        let block = fold_steer_block(&queue).unwrap();
        // The body (block minus the header line) stays within budget + the short marker.
        assert!(
            block.chars().count()
                <= "SUPERVISOR STEERING (apply these corrections this round):\n"
                    .chars()
                    .count()
                    + MAX_STEER_BLOCK_LEN
                    + 64,
            "folded block must be capped, got {} chars",
            block.chars().count()
        );
        assert!(
            block.contains("truncated"),
            "a dropped tail must be flagged: {block}"
        );

        // A realistic handful of short corrections is NOT truncated (whole messages kept).
        let small = fold_steer_block(&[
            "use a HashMap".into(),
            "add a bounds check".into(),
            "rename foo to bar".into(),
        ])
        .unwrap();
        assert!(
            !small.contains("truncated"),
            "small block untouched: {small}"
        );
        assert!(small.contains("- use a HashMap"));
        assert!(small.contains("- rename foo to bar"));
    }

    #[test]
    fn is_steer_stop_matches_sentinel_case_insensitively() {
        assert!(is_steer_stop("stop"));
        assert!(is_steer_stop("  STOP  "));
        assert!(is_steer_stop("Stop"));
        assert!(!is_steer_stop("stop the build")); // only the exact sentinel
        assert!(!is_steer_stop("please stop"));
        assert!(!is_steer_stop(""));
    }

    #[test]
    fn sanitize_steer_message_strips_invisible_bidi_and_collapses_whitespace() {
        // C2 (BIDI/INVISIBLE PARITY): the Tauri steer message must be cleaned like Python's
        // clean_text -> strip_invisible_and_bidi + whitespace collapse before it is queued /
        // folded into the prompt / shown in a toast.

        // Zero-width joiner, RTL override and BOM are STRIPPED (every char is in the
        // is_forbidden_command_char blocklist).
        let dirty = "use a \u{202e}HashMap\u{200d}\u{feff} not a Vec";
        assert_eq!(
            sanitize_steer_message(dirty),
            "use a HashMap not a Vec",
            "invisible/bidi chars must be dropped"
        );

        // Whitespace runs (incl. tabs/newlines and U+2028 line separator) collapse to single
        // spaces and the ends trim — mirroring Python's `\" \".join(text.split())`.
        let spacey = "  first\t\tline\n\nsecond\u{2028}third  ";
        assert_eq!(sanitize_steer_message(spacey), "first line second third");

        // A message made ENTIRELY of invisible/bidi/whitespace sanitizes to empty (the
        // command then rejects it — C3 parity with Python's required-field error).
        assert_eq!(sanitize_steer_message("\u{200b}\u{feff}  \u{202a}"), "");
        assert_eq!(sanitize_steer_message("   "), "");

        // Ordinary prose with internal single spaces round-trips unchanged.
        assert_eq!(
            sanitize_steer_message("prefer iterators over loops"),
            "prefer iterators over loops"
        );

        // The stop sentinel survives sanitize so the kill path still recognizes it.
        assert!(is_steer_stop(&sanitize_steer_message("  STOP  ")));
    }

    #[test]
    fn minimal_pending_directive_omits_empty_strings() {
        // The leanest directive Python might write (only id + task + resultPath):
        // a Rust round-trip must not inject empty-string parentAgentId/createdAt
        // nor allowOracle:false.
        let json = r#"{ "id": "d1", "task": "x", "resultPath": "mini/d1.json" }"#;
        let d: MiniCoderDirective = serde_json::from_str(json).unwrap();
        let out = serde_json::to_string(&d).unwrap();
        assert!(!out.contains("parentAgentId"), "out: {out}");
        assert!(!out.contains("createdAt"), "out: {out}");
        assert!(!out.contains("allowOracle"), "out: {out}");
        // status:pending is the enum default and IS emitted (no skip) — that is
        // acceptable since Python always writes status on a new directive.
    }

    // -- plan_tick ----------------------------------------------------------

    /// One-at-a-time concurrency the original tests assumed implicitly.
    const MC1: usize = 1;

    #[test]
    fn plan_tick_empty_is_empty() {
        let plan = plan_tick(
            &[],
            "2026-06-06T00:10:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan, TickPlan::default());
    }

    #[test]
    fn plan_tick_claims_oldest_pending() {
        let directives = vec![
            directive("d2", MiniCoderStatus::Pending, "2026-06-06T00:00:02Z"),
            directive("d1", MiniCoderStatus::Pending, "2026-06-06T00:00:01Z"),
            directive("d3", MiniCoderStatus::Pending, "2026-06-06T00:00:03Z"),
        ];
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.claims, vec!["d1".to_string()]);
        assert!(plan.timeouts.is_empty());
    }

    #[test]
    fn plan_tick_does_not_reclaim_active() {
        let directives = vec![
            directive("d1", MiniCoderStatus::Launching, "2026-06-06T00:00:01Z"),
            directive("d2", MiniCoderStatus::Running, "2026-06-06T00:00:02Z"),
        ];
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert!(plan.claims.is_empty());
    }

    #[test]
    fn plan_tick_flags_running_past_cap() {
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.started_at = Some("2026-06-06T00:00:00Z".into());
        let directives = vec![d];
        // 700s elapsed > 600s cap.
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:11:40Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.timeouts, vec!["d1".to_string()]);
    }

    #[test]
    fn plan_tick_does_not_flag_running_within_cap() {
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.started_at = Some("2026-06-06T00:00:00Z".into());
        let directives = vec![d];
        // 60s elapsed < 600s cap.
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert!(plan.timeouts.is_empty());
    }

    #[test]
    fn plan_tick_fail_open_on_bad_clock() {
        // No started_at, unparseable started_at, and unparseable now: none time out.
        let d_no_anchor = directive("d1", MiniCoderStatus::Running, "t");
        let mut d_bad_anchor = directive("d2", MiniCoderStatus::Running, "t");
        d_bad_anchor.started_at = Some("not-a-date".into());
        let directives = vec![d_no_anchor, d_bad_anchor];
        assert!(plan_tick(
            &directives,
            "2026-06-06T01:00:00Z",
            1,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        )
        .timeouts
        .is_empty());

        let mut d_ok = directive("d3", MiniCoderStatus::Running, "t");
        d_ok.started_at = Some("2026-06-06T00:00:00Z".into());
        assert!(plan_tick(
            &[d_ok],
            "not-a-date",
            1,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        )
        .timeouts
        .is_empty());
    }

    #[test]
    fn plan_tick_cap_zero_does_not_instant_timeout() {
        // A misconfigured cap of 0 (or negative) must be clamped to >= 1 so a
        // just-started directive is NOT instantly timed out.
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.started_at = Some("2026-06-06T00:00:00Z".into());
        let directives = vec![d];
        // 0s elapsed: now == started_at. With clamp, elapsed(0) < cap(1).
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:00:00Z",
            0,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert!(plan.timeouts.is_empty(), "cap=0 must not instant-timeout");
        let plan_neg = plan_tick(
            &directives,
            "2026-06-06T00:00:00Z",
            -5,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            MC1,
            &std::collections::HashSet::new(),
        );
        assert!(
            plan_neg.timeouts.is_empty(),
            "negative cap must not instant-timeout"
        );
    }

    #[test]
    fn plan_tick_max_concurrent_throttles_new_claims() {
        // One active + one pending (DISJOINT files, so only the concurrency ceiling — not
        // the file-disjointness rule — gates the claim). With max_concurrent=1 no new
        // claim; with max_concurrent=2 the pending one is claimed.
        let mut active = directive("active", MiniCoderStatus::Running, "2026-06-06T00:00:01Z");
        active.files = vec!["src/active.rs".into()];
        let directives = vec![
            active,
            pending_with_files("pending", "2026-06-06T00:00:02Z", &["src/pending.rs"]),
        ];
        let plan1 = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            1,
            &std::collections::HashSet::new(),
        );
        assert!(plan1.claims.is_empty(), "at ceiling -> no claim");

        let plan2 = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            2,
            &std::collections::HashSet::new(),
        );
        assert_eq!(
            plan2.claims,
            vec!["pending".to_string()],
            "below ceiling -> claim"
        );
    }

    #[test]
    fn plan_tick_timeouts_returned_regardless_of_concurrency() {
        // Even at the concurrency ceiling, blown-cap running directives are reaped.
        let mut d = directive("d1", MiniCoderStatus::Running, "2026-06-06T00:00:00Z");
        d.started_at = Some("2026-06-06T00:00:00Z".into());
        let pending = directive("d2", MiniCoderStatus::Pending, "2026-06-06T00:00:02Z");
        let directives = vec![d, pending];
        // active_count(1) >= max_concurrent(1) -> no claim, but timeout still fires.
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:11:40Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            1,
            &std::collections::HashSet::new(),
        );
        assert!(plan.claims.is_empty());
        assert_eq!(plan.timeouts, vec!["d1".to_string()]);
    }

    #[test]
    fn plan_tick_flags_launching_past_launch_cap() {
        // WARNING 4: a directive stuck `launching` (claimed but never marked running)
        // past the launch cap must be flagged so the executor frees the slot. A fresh
        // launching directive is NOT flagged.
        let mut stuck = directive("stuck", MiniCoderStatus::Launching, "2026-06-06T00:00:00Z");
        stuck.claimed_at = Some("2026-06-06T00:00:00Z".into());
        let mut fresh = directive("fresh", MiniCoderStatus::Launching, "2026-06-06T00:00:50Z");
        fresh.claimed_at = Some("2026-06-06T00:00:50Z".into());
        let directives = vec![stuck, fresh];
        // now = claimed("stuck")+90s (> 60s cap) but claimed("fresh")+40s (< cap).
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:30Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            1,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.stuck_launching, vec!["stuck".to_string()]);
        // A launching directive with NO claimed_at anchor is NOT flagged here (the
        // executor's startup crash-sweep is the backstop for that case).
        let no_anchor = directive("noanchor", MiniCoderStatus::Launching, "t");
        let plan2 = plan_tick(
            &[no_anchor],
            "2026-06-06T05:00:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            1,
            &std::collections::HashSet::new(),
        );
        assert!(plan2.stuck_launching.is_empty());
    }

    // -- P6: multi-claim + retry cap + lost-child + gate decision ------------

    /// Build a pending directive with a custom file set (for disjointness tests).
    fn pending_with_files(id: &str, created_at: &str, files: &[&str]) -> MiniCoderDirective {
        let mut d = directive(id, MiniCoderStatus::Pending, created_at);
        d.files = files.iter().map(|f| f.to_string()).collect();
        d
    }

    #[test]
    fn plan_tick_multi_claim_respects_concurrency_and_oldest_first() {
        // 3 disjoint pending + max_concurrent=2 -> exactly 2 claimed, oldest-first.
        let directives = vec![
            pending_with_files("d2", "2026-06-06T00:00:02Z", &["src/b.rs"]),
            pending_with_files("d1", "2026-06-06T00:00:01Z", &["src/a.rs"]),
            pending_with_files("d3", "2026-06-06T00:00:03Z", &["src/c.rs"]),
        ];
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            2,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.claims, vec!["d1".to_string(), "d2".to_string()]);
    }

    #[test]
    fn plan_tick_multi_claim_skips_overlapping_files() {
        // d1 and d2 both touch src/a.rs; even with headroom only the oldest (d1) is
        // claimed — d2 overlaps the just-claimed file set and stays Pending.
        let directives = vec![
            pending_with_files("d1", "2026-06-06T00:00:01Z", &["src/a.rs"]),
            pending_with_files("d2", "2026-06-06T00:00:02Z", &["src/a.rs", "src/z.rs"]),
            pending_with_files("d3", "2026-06-06T00:00:03Z", &["src/c.rs"]),
        ];
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            4,
            &std::collections::HashSet::new(),
        );
        // d1 (oldest) + d3 (disjoint); d2 skipped (overlaps d1's src/a.rs).
        assert_eq!(plan.claims, vec!["d1".to_string(), "d3".to_string()]);
    }

    #[cfg(windows)]
    #[test]
    fn plan_tick_windows_path_normalization_treats_backslash_and_case_equal() {
        // On Windows `src\a.rs` and `src/A.RS` are the SAME file; the second candidate
        // must be skipped as overlapping the first.
        let directives = vec![
            pending_with_files("d1", "2026-06-06T00:00:01Z", &["src\\a.rs"]),
            pending_with_files("d2", "2026-06-06T00:00:02Z", &["src/A.RS"]),
        ];
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:01:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            4,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.claims, vec!["d1".to_string()]);
    }

    /// BLOCKER 1 (second sweep rule): an AwaitingRetry predecessor whose retry child is
    /// now TERMINAL (not absent) but which is itself still un-stamped must be caught and
    /// re-propagated from the child's terminal outcome — closing the STRANDED-ROOT window

    // -- P6: backend max_concurrent config -----------------------------------

    #[test]
    fn max_concurrent_round_trips_camel_case_and_skips_when_none() {
        let mut b = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: None,
            max_concurrent: Some(3),
        };
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("\"maxConcurrent\":3"), "json: {json}");
        let back: MiniCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_concurrent, Some(3));

        // None -> key omitted (no churn).
        b.max_concurrent = None;
        let json2 = serde_json::to_string(&b).unwrap();
        assert!(!json2.contains("maxConcurrent"), "json2: {json2}");
    }

    #[test]
    fn effective_max_concurrent_default_and_clamp() {
        let base = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: Some("m".into()),
            command: None,
            base_url: None,
            max_concurrent: None,
        };
        assert_eq!(effective_max_concurrent(&base), DEFAULT_MAX_CONCURRENT);
        assert_eq!(effective_max_concurrent(&base), 2);

        let mut b = base.clone();
        b.max_concurrent = Some(0);
        assert_eq!(effective_max_concurrent(&b), 1, "0 clamps to 1");
        b.max_concurrent = Some(9);
        assert_eq!(effective_max_concurrent(&b), 4, "9 clamps to 4");
        b.max_concurrent = Some(3);
        assert_eq!(effective_max_concurrent(&b), 3);
    }

    #[test]
    fn validate_clamps_max_concurrent_into_band() {
        let bad_low = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: Some("m".into()),
            command: None,
            base_url: None,
            max_concurrent: Some(0),
        };
        let n = validate_mini_coder_backend(&bad_low).unwrap();
        assert_eq!(n.max_concurrent, Some(1));

        let bad_high = MiniCoderBackend {
            max_concurrent: Some(9),
            ..bad_low.clone()
        };
        let n2 = validate_mini_coder_backend(&bad_high).unwrap();
        assert_eq!(n2.max_concurrent, Some(4));

        let none = MiniCoderBackend {
            max_concurrent: None,
            ..bad_low
        };
        let n3 = validate_mini_coder_backend(&none).unwrap();
        assert_eq!(n3.max_concurrent, None, "None preserved (no-churn)");
    }

    #[test]
    fn budget_max_agentic_fix_rounds_fits_the_python_poll_budget() {
        // POLL-BUDGET INVARIANT (mirrors the doc on MAX_AGENTIC_FIX_ROUNDS): the worst-case
        // chain runs `1 + budget` SEQUENTIAL attempts. Each attempt is capped by `plan_tick`
        // in TWO consecutive phases: it sits in `Launching` (DEFAULT_LAUNCH_CAP_SECS) BEFORE
        // it runs (DEFAULT_WALL_CLOCK_CAP_SECS for attempt 0, DEFAULT_RETRY_WALL_CLOCK_CAP_SECS
        // for every retry). The previous formula omitted the (N+1) launch caps and so
        // understated the worst case. The poll timeout is HARD (it stamps the directive
        // terminal on disk + poisons the live retry chain — see the const doc), so the
        // CORRECTED caps-only worst-case MUST fit STRICTLY inside the Python
        // `MINI_CODER_POLL_TIMEOUT_SECS` (1800s) with headroom for the out-of-cap fine-verdict
        // passes between rounds. A regression that raises the budget without extending the
        // poll constant trips here.
        const PYTHON_POLL_TIMEOUT_SECS: i64 = 1800; // oracle/server/aspis_mcp.py
        let n = MAX_AGENTIC_FIX_ROUNDS as i64;
        // attempt 0 runs at the fresh cap; the N retries at the shorter retry cap; EVERY one
        // of the (N+1) attempts also pays its launch cap before running.
        let worst_case = DEFAULT_WALL_CLOCK_CAP_SECS
            + n * DEFAULT_RETRY_WALL_CLOCK_CAP_SECS
            + (n + 1) * DEFAULT_LAUNCH_CAP_SECS;
        assert_eq!(
            worst_case, 1380,
            "600 + 2*300 + 3*60 (caps-only, incl. launch caps)"
        );
        // Strictly under, with headroom: the deferred fine-verdict pass between rounds runs
        // OUTSIDE these caps, so a thin margin (the old 1500s == 300s) is NOT enough — assert
        // a generous slack (>= 300s) absorbs those out-of-cap passes.
        assert!(
            worst_case <= PYTHON_POLL_TIMEOUT_SECS - 300,
            "agentic caps-only worst-case {worst_case}s must leave >=300s headroom under the \
             {PYTHON_POLL_TIMEOUT_SECS}s poll budget for the out-of-cap fine-verdict passes; \
             raising MAX_AGENTIC_FIX_ROUNDS requires extending MINI_CODER_POLL_TIMEOUT_SECS"
        );
    }

    #[test]
    fn python_shaped_directive_with_new_fields_round_trips() {
        // BLOCKER/WARNING 3+4 parity: a directive carrying the Rust-set scratchPath +
        // claimedAt round-trips through Rust serde without churn (camelCase, present).
        let json = r#"{
            "id": "d1",
            "parentAgentId": "coder-1",
            "status": "running",
            "task": "t",
            "resultPath": "d1.json",
            "createdAt": "2026-06-06T00:00:00Z",
            "claimedAt": "2026-06-06T00:00:01Z",
            "scratchPath": "/proj/.aspis-mini",
            "agentId": "mini-coder1-abcd1234",
            "startedAt": "2026-06-06T00:00:02Z"
        }"#;
        let d: MiniCoderDirective = serde_json::from_str(json).unwrap();
        assert_eq!(d.claimed_at.as_deref(), Some("2026-06-06T00:00:01Z"));
        assert_eq!(d.scratch_path.as_deref(), Some("/proj/.aspis-mini"));
        let out = serde_json::to_string(&d).unwrap();
        assert!(
            out.contains("\"claimedAt\":\"2026-06-06T00:00:01Z\""),
            "out: {out}"
        );
        assert!(
            out.contains("\"scratchPath\":\"/proj/.aspis-mini\""),
            "out: {out}"
        );
    }

    // -- cap_directives -----------------------------------------------------

    #[test]
    fn cap_evicts_oldest_terminal_keeps_active() {
        let mut directives = vec![
            directive("old1", MiniCoderStatus::Done, "2026-06-06T00:00:01Z"),
            directive("old2", MiniCoderStatus::Failed, "2026-06-06T00:00:02Z"),
            directive("active", MiniCoderStatus::Running, "2026-06-06T00:00:03Z"),
            directive("new", MiniCoderStatus::Done, "2026-06-06T00:00:04Z"),
        ];
        cap_directives(&mut directives, 2);
        // Must shed 2; only terminal ones, oldest first -> old1, old2 gone.
        let ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["active", "new"]);
    }

    #[test]
    fn cap_never_evicts_active_even_over_max() {
        // All active, over cap: nothing is dropped (live work preserved).
        let mut directives = vec![
            directive("a", MiniCoderStatus::Running, "2026-06-06T00:00:01Z"),
            directive("b", MiniCoderStatus::Launching, "2026-06-06T00:00:02Z"),
            directive("c", MiniCoderStatus::Pending, "2026-06-06T00:00:03Z"),
        ];
        cap_directives(&mut directives, 1);
        assert_eq!(directives.len(), 3);
    }

    #[test]
    fn cap_noop_under_max() {
        let mut directives = vec![directive("a", MiniCoderStatus::Done, "t")];
        cap_directives(&mut directives, 50);
        assert_eq!(directives.len(), 1);
    }

    /// WARNING 5 (PROPAGATED-THEN-EVICTED): a full queue plus a freshly-propagated
    /// terminal root must NOT evict that root in the cap pass it was stamped in. The
    /// protected id survives even though it is the OLDEST terminal directive (the one
    /// cap would normally shed first), so the blocking poll can read its outcome.
    #[test]
    fn cap_protecting_spares_just_finalized_root_even_when_oldest() {
        // `root` is the oldest terminal directive — without protection cap would evict
        // it first. We protect it; an OTHER terminal directive is shed instead.
        let mut directives = vec![
            directive("root", MiniCoderStatus::Failed, "2026-06-06T00:00:01Z"),
            directive("other-old", MiniCoderStatus::Done, "2026-06-06T00:00:02Z"),
            directive("active", MiniCoderStatus::Running, "2026-06-06T00:00:03Z"),
            directive("newest", MiniCoderStatus::Done, "2026-06-06T00:00:04Z"),
        ];
        cap_directives_protecting(&mut directives, 2, &["root".to_string()]);
        let ids: std::collections::HashSet<&str> =
            directives.iter().map(|d| d.id.as_str()).collect();
        assert!(
            ids.contains("root"),
            "protected just-finalized root must survive the cap"
        );
        assert!(ids.contains("active"), "active is never evicted");
        // We had to shed 2 terminal slots; with root protected, other-old + newest go.
        assert!(
            !ids.contains("other-old"),
            "an unprotected terminal is shed instead"
        );
        assert_eq!(directives.len(), 2);
    }

    /// Control: WITHOUT protection the same full queue DOES evict the oldest root (the
    /// exact bug WARNING 5 fixes) — proving the protect set is load-bearing.
    #[test]
    fn cap_without_protection_evicts_oldest_root() {
        let mut directives = vec![
            directive("root", MiniCoderStatus::Failed, "2026-06-06T00:00:01Z"),
            directive("other-old", MiniCoderStatus::Done, "2026-06-06T00:00:02Z"),
            directive("active", MiniCoderStatus::Running, "2026-06-06T00:00:03Z"),
            directive("newest", MiniCoderStatus::Done, "2026-06-06T00:00:04Z"),
        ];
        cap_directives(&mut directives, 2);
        let ids: std::collections::HashSet<&str> =
            directives.iter().map(|d| d.id.as_str()).collect();
        assert!(
            !ids.contains("root"),
            "unprotected oldest root IS evicted (the bug)"
        );
    }

    // -- read_result_file ---------------------------------------------------

    fn write_scratch(dir: &Path, rel: &str, body: &str) {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(target).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn read_result_valid_done() {
        let dir = std::env::temp_dir().join(format!("mc_done_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_scratch(
            &dir,
            "mini/d1.json",
            r#"{"status":"done","output":"ok","filesTouched":["src/a.rs"]}"#,
        );
        let outcome = read_result_file(&dir, "mini/d1.json");
        assert_eq!(outcome.status, MiniCoderStatus::Done);
        assert_eq!(outcome.output.as_deref(), Some("ok"));
        assert_eq!(outcome.files_touched, vec!["src/a.rs".to_string()]);
        assert_eq!(outcome.error, None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_valid_needs_clarification() {
        let dir = std::env::temp_dir().join(format!("mc_clar_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_scratch(
            &dir,
            "r.json",
            r#"{"status":"needs_clarification","question":"which foo?"}"#,
        );
        let outcome = read_result_file(&dir, "r.json");
        assert_eq!(outcome.status, MiniCoderStatus::NeedsClarification);
        assert_eq!(outcome.question.as_deref(), Some("which foo?"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_missing_is_failed() {
        let dir = std::env::temp_dir().join(format!("mc_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let outcome = read_result_file(&dir, "nope.json");
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        assert!(outcome.error.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_malformed_is_failed() {
        let dir = std::env::temp_dir().join(format!("mc_bad_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_scratch(&dir, "bad.json", r#"{"status":"done", "output": "#);
        let outcome = read_result_file(&dir, "bad.json");
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_unknown_status_is_failed() {
        let dir = std::env::temp_dir().join(format!("mc_unk_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_scratch(&dir, "u.json", r#"{"status":"aborted_by_human"}"#);
        // The mini may NOT self-report a synthesized status; treat as failed.
        let outcome = read_result_file(&dir, "u.json");
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_oversized_is_failed() {
        // BLOCKER 2: a result file larger than MAX_RESULT_BYTES must degrade to
        // `failed` (OOM guard) instead of being read unbounded.
        let dir = std::env::temp_dir().join(format!("mc_huge_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Valid JSON, but padded past the cap so the size check trips first.
        let pad = "x".repeat((MAX_RESULT_BYTES as usize) + 16);
        let body = format!(r#"{{"status":"done","output":"{pad}"}}"#);
        write_scratch(&dir, "big.json", &body);
        let outcome = read_result_file(&dir, "big.json");
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        assert!(
            outcome.error.as_deref().unwrap().contains("too large"),
            "error: {:?}",
            outcome.error
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_at_cap_still_parses() {
        // A file at (not over) the cap must still be read. Build a valid JSON
        // payload padded to be just under MAX_RESULT_BYTES.
        let dir = std::env::temp_dir().join(format!("mc_atcap_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = r#"{"status":"done","output":""#;
        let suffix = r#""}"#;
        let pad_len = (MAX_RESULT_BYTES as usize) - prefix.len() - suffix.len();
        let body = format!("{prefix}{}{suffix}", "x".repeat(pad_len));
        assert_eq!(body.len() as u64, MAX_RESULT_BYTES);
        write_scratch(&dir, "atcap.json", &body);
        let outcome = read_result_file(&dir, "atcap.json");
        assert_eq!(
            outcome.status,
            MiniCoderStatus::Done,
            "error: {:?}",
            outcome.error
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_result_rejects_traversal_and_absolute_without_io() {
        let dir = std::env::temp_dir().join(format!("mc_trav_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for bad in ["../escape.json", "a/../../etc.json", "..\\escape.json"] {
            let outcome = read_result_file(&dir, bad);
            assert_eq!(outcome.status, MiniCoderStatus::Failed, "path {bad}");
            assert!(outcome.error.unwrap().contains("result path"));
        }
        // Absolute path rejected too.
        let abs = if cfg!(windows) {
            "C:\\evil.json"
        } else {
            "/evil.json"
        };
        let outcome = read_result_file(&dir, abs);
        assert_eq!(outcome.status, MiniCoderStatus::Failed);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- fake launcher driving the full lifecycle ---------------------------

    struct FakeLauncher {
        next_id: String,
        fail: bool,
        calls: RefCell<Vec<String>>, // directive ids it was asked to launch
    }

    impl MiniLauncher for FakeLauncher {
        fn launch(&self, directive: &MiniCoderDirective) -> Result<String, String> {
            self.calls.borrow_mut().push(directive.id.clone());
            if self.fail {
                Err("fake spawn failed".into())
            } else {
                Ok(self.next_id.clone())
            }
        }
    }

    #[test]
    fn fake_launcher_drives_one_directive_full_lifecycle() {
        let dir = std::env::temp_dir().join(format!("mc_fake_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_scratch(
            &dir,
            "mini/d1.json",
            r#"{"status":"done","output":"done it","filesTouched":["src/a.rs"]}"#,
        );

        let launcher = FakeLauncher {
            next_id: "mini-coder1-abcd1234".into(),
            fail: false,
            calls: RefCell::new(Vec::new()),
        };

        let mut directives = vec![directive(
            "d1",
            MiniCoderStatus::Pending,
            "2026-06-06T00:00:00Z",
        )];

        // 1) plan_tick claims d1.
        let plan = plan_tick(
            &directives,
            "2026-06-06T00:00:00Z",
            DEFAULT_WALL_CLOCK_CAP_SECS,
            DEFAULT_RETRY_WALL_CLOCK_CAP_SECS,
            DEFAULT_LAUNCH_CAP_SECS,
            1,
            &std::collections::HashSet::new(),
        );
        assert_eq!(plan.claims, vec!["d1".to_string()]);
        let claim_id = plan.claims[0].clone();
        assert_eq!(claim_id, "d1");
        let idx = directives.iter().position(|d| d.id == claim_id).unwrap();
        directives[idx] = apply_claim(&directives[idx], "2026-06-06T00:00:00Z").unwrap();
        assert_eq!(directives[idx].status, MiniCoderStatus::Launching);

        // 2) launcher spawns the mini.
        let agent_id = launcher.launch(&directives[idx]).unwrap();
        directives[idx] =
            apply_launched(&directives[idx], &agent_id, "2026-06-06T00:00:01Z").unwrap();
        assert_eq!(directives[idx].status, MiniCoderStatus::Running);
        assert_eq!(
            directives[idx].agent_id.as_deref(),
            Some("mini-coder1-abcd1234")
        );

        // 3) mini exited; executor reads its result file -> done.
        let outcome = read_result_file(&dir, &directives[idx].result_path);
        directives[idx] = apply_result(&directives[idx], outcome).unwrap();
        assert_eq!(directives[idx].status, MiniCoderStatus::Done);
        let res = directives[idx].result.as_ref().unwrap();
        assert_eq!(res.output.as_deref(), Some("done it"));
        assert_eq!(res.files_touched, vec!["src/a.rs".to_string()]);

        assert_eq!(*launcher.calls.borrow(), vec!["d1".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fake_launcher_failure_yields_failed_outcome() {
        let launcher = FakeLauncher {
            next_id: String::new(),
            fail: true,
            calls: RefCell::new(Vec::new()),
        };
        let claimed = apply_claim(&directive("d1", MiniCoderStatus::Pending, "t"), "t").unwrap();
        let err = launcher.launch(&claimed).unwrap_err();
        // Executor turns a launch error into a failed outcome on the launching dir.
        let failed = apply_failed(&claimed, err).unwrap();
        assert_eq!(failed.status, MiniCoderStatus::Failed);
        assert!(failed
            .result
            .unwrap()
            .error
            .unwrap()
            .contains("fake spawn failed"));
    }

    // -- backend config (P4) -------------------------------------------------

    #[test]
    fn backend_kind_serializes_lowercase_matching_ts() {
        for (kind, tok) in [
            (MiniCoderBackendKind::Ollama, "ollama"),
            (MiniCoderBackendKind::Api, "api"),
            (MiniCoderBackendKind::Codex, "codex"),
            (MiniCoderBackendKind::Openai, "openai"),
            (MiniCoderBackendKind::AppleFm, "appleFm"),
            (MiniCoderBackendKind::Cloud, "cloud"),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{tok}\""));
            let back: MiniCoderBackendKind = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn applefm_accepts_optional_model_and_rejects_bad_model() {
        let no_model = MiniCoderBackend {
            kind: MiniCoderBackendKind::AppleFm,
            model: None,
            command: Some("dropped".into()),
            base_url: Some("dropped".into()),
            max_concurrent: Some(0),
        };
        let n = validate_mini_coder_backend(&no_model).unwrap();
        assert_eq!(n.model, None);
        assert_eq!(n.command, None);
        assert_eq!(n.base_url, None);
        assert_eq!(n.max_concurrent, Some(1));

        let bad = MiniCoderBackend {
            kind: MiniCoderBackendKind::AppleFm,
            model: Some("bad model".into()),
            command: None,
            base_url: None,
            max_concurrent: Some(9),
        };
        assert!(validate_mini_coder_backend(&bad).is_err());
        assert_eq!(
            validate_mini_coder_backend(&bad).unwrap_err(),
            "Mini-coder model must be a bare tag (letters, digits, . _ : / -)."
        );
    }

    #[test]
    fn applefm_keeps_optional_model() {
        let with_model = MiniCoderBackend {
            kind: MiniCoderBackendKind::AppleFm,
            model: Some("gpt-5".into()),
            command: Some("dropped".into()),
            base_url: Some("dropped".into()),
            max_concurrent: None,
        };
        let n = validate_mini_coder_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("gpt-5"));
        assert_eq!(n.command, None);
        assert_eq!(n.base_url, None);
        assert_eq!(n.max_concurrent, None);
    }

    #[test]
    fn backend_round_trips_camel_case_and_skips_unused_fields() {
        let ollama = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: None,
            max_concurrent: None,
        };
        let json = serde_json::to_string(&ollama).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "json: {json}");
        assert!(json.contains("\"model\":\"qwen2.5-coder\""), "json: {json}");
        assert!(!json.contains("command"), "unused command leaked: {json}");
        let back: MiniCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(ollama, back);

        // codex with no model: the model key is absent (no churn).
        let codex_bare = MiniCoderBackend {
            kind: MiniCoderBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            max_concurrent: None,
        };
        let cj = serde_json::to_string(&codex_bare).unwrap();
        assert_eq!(cj, r#"{"kind":"codex"}"#);
    }

    #[test]
    fn validate_ollama_requires_model_and_keeps_only_model() {
        let bad = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: None,
            command: Some("ignored".into()),
            base_url: None,
            max_concurrent: None,
        };
        assert!(validate_mini_coder_backend(&bad).is_err());

        let ok = MiniCoderBackend {
            kind: MiniCoderBackendKind::Ollama,
            model: Some("  qwen2.5-coder  ".into()),
            command: Some("dropped".into()),
            base_url: None,
            max_concurrent: None,
        };
        let n = validate_mini_coder_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder")); // trimmed
        assert_eq!(n.command, None); // command dropped for ollama
    }

    #[test]
    fn validate_api_requires_command_rejects_control_chars() {
        let no_cmd = MiniCoderBackend {
            kind: MiniCoderBackendKind::Api,
            model: None,
            command: None,
            base_url: None,
            max_concurrent: None,
        };
        assert!(validate_mini_coder_backend(&no_cmd).is_err());

        let ctrl = MiniCoderBackend {
            kind: MiniCoderBackendKind::Api,
            model: None,
            command: Some("mycli chat\nrm -rf /".into()),
            base_url: None,
            max_concurrent: None,
        };
        assert!(
            validate_mini_coder_backend(&ctrl).is_err(),
            "a newline in the command must be rejected (script-injection guard)"
        );

        let ok = MiniCoderBackend {
            kind: MiniCoderBackendKind::Api,
            model: Some("dropped".into()),
            command: Some("  mycli chat --json  ".into()),
            base_url: None,
            max_concurrent: None,
        };
        let n = validate_mini_coder_backend(&ok).unwrap();
        assert_eq!(n.command.as_deref(), Some("mycli chat --json"));
        assert_eq!(n.model, None); // model dropped for api
    }

    #[test]
    fn validate_api_rejects_del_and_unicode_control_chars() {
        // WARNING 6: `is_control()` must reject DEL (0x7f) and Unicode Cc controls
        // (e.g. the right-to-left override U+202E) that the old `< 0x20` check let
        // through — they would be embedded verbatim into the launch line.
        for bad in [
            "mycli chat\u{7f}--json",   // DEL (0x7f)
            "mycli chat\u{202e}--json", // RIGHT-TO-LEFT OVERRIDE (bidi, Unicode Cc)
        ]
        .iter()
        {
            let b = MiniCoderBackend {
                kind: MiniCoderBackendKind::Api,
                model: None,
                command: Some((*bad).into()),
                base_url: None,
                max_concurrent: None,
            };
            assert!(
                validate_mini_coder_backend(&b).is_err(),
                "control/invisible char in command {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_codex_ok_bare_and_with_optional_model() {
        let bare = MiniCoderBackend {
            kind: MiniCoderBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            max_concurrent: None,
        };
        let n = validate_mini_coder_backend(&bare).unwrap();
        assert_eq!(n.model, None);
        assert_eq!(n.command, None);

        let with_model = MiniCoderBackend {
            kind: MiniCoderBackendKind::Codex,
            model: Some("gpt-5-codex".into()),
            command: Some("dropped".into()),
            base_url: None,
            max_concurrent: None,
        };
        let n = validate_mini_coder_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(n.command, None);
    }

    #[test]
    fn validate_rejects_model_with_whitespace_or_metachars() {
        for bad_model in ["qwen coder", "model;rm", "$(evil)", "-leadinghyphen.. ok"] {
            let b = MiniCoderBackend {
                kind: MiniCoderBackendKind::Ollama,
                model: Some(bad_model.into()),
                command: None,
                base_url: None,
                max_concurrent: None,
            };
            assert!(
                validate_mini_coder_backend(&b).is_err(),
                "model {bad_model:?} must be rejected"
            );
        }
    }

    // -- oMLX backend (oMLX-P1) ---------------------------------------------

    fn omlx(model: Option<&str>, base_url: Option<&str>) -> MiniCoderBackend {
        MiniCoderBackend {
            kind: MiniCoderBackendKind::Omlx,
            model: model.map(|s| s.to_string()),
            command: Some("dropped".into()), // omlx ignores command
            base_url: base_url.map(|s| s.to_string()),
            max_concurrent: None,
        }
    }

    #[test]
    fn omlx_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&MiniCoderBackendKind::Omlx).unwrap(),
            "\"omlx\""
        );
        let back: MiniCoderBackendKind = serde_json::from_str("\"omlx\"").unwrap();
        assert_eq!(back, MiniCoderBackendKind::Omlx);
    }

    #[test]
    fn omlx_round_trips_camel_case_baseurl_and_drops_command() {
        // A validated omlx backend serializes `baseUrl` (camelCase) and has NO
        // `command` key (no churn; command is dropped for omlx).
        let n = validate_mini_coder_backend(&omlx(
            Some("qwen2.5-coder"),
            Some("http://localhost:8000/v1"),
        ))
        .unwrap();
        assert_eq!(n.command, None, "command must be dropped for omlx");
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("\"kind\":\"omlx\""), "json: {json}");
        assert!(
            json.contains("\"baseUrl\":\"http://localhost:8000/v1\""),
            "json: {json}"
        );
        assert!(!json.contains("command"), "command leaked: {json}");
        assert!(!json.contains("base_url"), "snake_case leaked: {json}");
        let back: MiniCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn omlx_requires_model_and_base_url() {
        // Missing model.
        assert!(
            validate_mini_coder_backend(&omlx(None, Some("http://localhost:8000/v1"))).is_err(),
            "missing model must be rejected"
        );
        // Missing base_url.
        assert!(
            validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), None)).is_err(),
            "missing base url must be rejected"
        );
        // Empty/whitespace base_url.
        assert!(
            validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some("   "))).is_err(),
            "blank base url must be rejected"
        );
    }

    #[test]
    fn omlx_accepts_loopback_http_and_trims_model() {
        for url in [
            "http://localhost:8000/v1",
            "http://127.0.0.1:8000/v1",
            "http://127.5.4.3:8000/v1", // anywhere in 127.0.0.0/8
            "http://[::1]:8000/v1",
            "http://localhost/v1", // no port
            "http://127.0.0.1",    // bare loopback, no path
        ] {
            let n = validate_mini_coder_backend(&omlx(Some("  qwen2.5-coder  "), Some(url)))
                .unwrap_or_else(|e| panic!("url {url:?} should be accepted, got: {e}"));
            assert_eq!(n.kind, MiniCoderBackendKind::Omlx);
            assert_eq!(n.model.as_deref(), Some("qwen2.5-coder")); // trimmed
        }
    }

    #[test]
    fn omlx_rejects_https_scheme() {
        // F3: oMLX is http-only on loopback (like Ollama). A self-signed TLS cert on a
        // loopback oMLX server would fail reqwest's default verification and silently
        // disable the tier, so `https://` must be REJECTED even for a loopback host.
        for bad in [
            "https://localhost:8000/v1",
            "https://127.0.0.1:8000/v1",
            "https://[::1]:8000/v1",
            "https://localhost",
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(bad))).is_err(),
                "https oMLX base url {bad:?} must be REJECTED (http only)"
            );
        }
    }

    #[test]
    fn omlx_rejects_non_loopback_userinfo_and_suffix_trick() {
        for bad in [
            "http://evil.com/v1",            // non-loopback host
            "http://192.168.0.1:8000/v1",    // LAN, not loopback
            "http://127.0.0.1.evil.com/v1",  // suffix trick (must NOT match 127.)
            "http://127.0.0.1@evil.com/v1",  // userinfo trick
            "http://localhost.evil.com/v1",  // localhost suffix trick
            "ftp://localhost:8000/v1",       // bad scheme
            "localhost:8000/v1",             // missing scheme
            "http://[::1]extra/v1",          // malformed ipv6 authority
            "http://[::1]:8000@evil.com/v1", // F1: ipv6 userinfo bypass
            "http://[::1]:@evil.com/v1",     // F1: minimal ipv6 userinfo bypass
            "http://[::1]@evil.com/v1",      // F1: ipv6 userinfo, no port
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(bad))).is_err(),
                "base url {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn omlx_rejects_ipv6_userinfo_loopback_bypass() {
        // F1 regression: `[::1]:port@evil.com` would route to evil.com if the `[::1]`
        // branch only checked `starts_with(':')`. The `@` in the remainder MUST reject.
        for bad in [
            "http://[::1]:8000@evil.com",
            "http://[::1]:@evil.com",
            "http://[::1]@evil.com",
            "https://[::1]:8000@evil.com/v1",
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(bad))).is_err(),
                "ipv6 userinfo bypass {bad:?} must be REJECTED"
            );
        }
    }

    #[test]
    fn omlx_validates_optional_port() {
        // F2: a present port must be 1-5 digits and <= 65535; empty port rejected.
        for ok in [
            "http://localhost:8000/v1",
            "http://127.0.0.1:1/v1",
            "http://127.0.0.1:65535/v1",
            "http://[::1]:8000/v1",
            "http://[::1]:65535",
            "http://localhost/v1", // no port at all is fine
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(ok))).is_ok(),
                "valid-port url {ok:?} must be accepted"
            );
        }
        for bad in [
            "http://localhost:abc/v1",   // non-numeric
            "http://localhost:65536/v1", // > 65535
            "http://localhost:999999",   // > 5 digits and out of range
            "http://localhost:/v1",      // empty port
            "http://localhost:",         // empty port, bare
            "http://[::1]:abc",          // ipv6 non-numeric port
            "http://[::1]:65536/v1",     // ipv6 out of range
            "http://[::1]:",             // ipv6 empty port
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(bad))).is_err(),
                "invalid-port url {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn omlx_rejects_control_and_invisible_chars_in_base_url() {
        for bad in [
            "http://localhost:8000/v1\nrm -rf /", // newline
            "http://localhost:8000/v1\u{7f}",     // DEL
            "http://localhost:8000/\u{202e}v1",   // RIGHT-TO-LEFT OVERRIDE (bidi)
            "http://localhost:8000/\u{200b}v1",   // ZERO WIDTH SPACE
        ] {
            assert!(
                validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(bad))).is_err(),
                "control/invisible char in base url {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn omlx_normalizes_trailing_slash() {
        // A trailing slash is stripped so `<baseUrl>/chat/completions` never
        // double-slashes. Stored normalized.
        let n = validate_mini_coder_backend(&omlx(
            Some("qwen2.5-coder"),
            Some("http://localhost:8000/v1/"),
        ))
        .unwrap();
        assert_eq!(n.base_url.as_deref(), Some("http://localhost:8000/v1"));

        // A bare-root URL strips its slash too.
        let n2 = validate_mini_coder_backend(&omlx(
            Some("qwen2.5-coder"),
            Some("http://localhost:8000/"),
        ))
        .unwrap();
        assert_eq!(n2.base_url.as_deref(), Some("http://localhost:8000"));

        // No trailing slash -> unchanged.
        let n3 = validate_mini_coder_backend(&omlx(
            Some("qwen2.5-coder"),
            Some("http://localhost:8000/v1"),
        ))
        .unwrap();
        assert_eq!(n3.base_url.as_deref(), Some("http://localhost:8000/v1"));
    }

    #[test]
    fn omlx_rejects_overlong_base_url() {
        let long = format!(
            "http://localhost:8000/{}",
            "a".repeat(MINI_BASE_URL_MAX_LEN)
        );
        assert!(
            validate_mini_coder_backend(&omlx(Some("qwen2.5-coder"), Some(&long))).is_err(),
            "overlong base url must be rejected"
        );
    }

    #[test]
    fn omlx_rejects_bad_model_tag() {
        assert!(
            validate_mini_coder_backend(&omlx(
                Some("qwen coder"),
                Some("http://localhost:8000/v1")
            ))
            .is_err(),
            "model with whitespace must be rejected"
        );
    }

    // -- A1 cloud kind ---------------------------------------------------------
    // Cloud mini-coder backend mirrors the orchestrator's
    // `LocalCoderBackendKind::Cloud` accept/reject set via the SHARED
    // `super::local_coder::validate_cloud_base_url` validator (the validator is
    // REUSED, NOT duplicated, so the two surfaces can never drift). These tests
    // pin the integration: https+FQDN+model accepted; loopback / http / bad-model
    // / empty-url rejected. They complement (NOT replace) the local_coder unit
    // tests for the shared validator itself.

    fn cloud(model: Option<&str>, base_url: Option<&str>) -> MiniCoderBackend {
        MiniCoderBackend {
            kind: MiniCoderBackendKind::Cloud,
            model: model.map(|s| s.to_string()),
            command: Some("dropped".into()), // cloud ignores command
            base_url: base_url.map(|s| s.to_string()),
            max_concurrent: None,
        }
    }

    #[test]
    fn cloud_kind_serializes_lowercase_matching_ts() {
        assert_eq!(
            serde_json::to_string(&MiniCoderBackendKind::Cloud).unwrap(),
            "\"cloud\""
        );
        let back: MiniCoderBackendKind = serde_json::from_str("\"cloud\"").unwrap();
        assert_eq!(back, MiniCoderBackendKind::Cloud);
    }

    #[test]
    fn cloud_kind_label_returns_wire_token() {
        assert_eq!(MiniCoderBackendKind::Cloud.kind_label(), "cloud");
        // Belt-and-suspenders: every variant returns its own wire token.
        assert_eq!(MiniCoderBackendKind::Ollama.kind_label(), "ollama");
        assert_eq!(MiniCoderBackendKind::AppleFm.kind_label(), "appleFm");
    }

    #[test]
    fn cloud_accepts_https_with_model_and_normalizes() {
        // Standard happy path: https + FQDN + model.
        let n = validate_mini_coder_backend(&cloud(
            Some("gpt-4o-mini"),
            Some("https://openrouter.ai/api/v1"),
        ))
        .expect("https + FQDN + model must be accepted");
        assert_eq!(n.kind, MiniCoderBackendKind::Cloud);
        assert_eq!(n.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(n.base_url.as_deref(), Some("https://openrouter.ai/api/v1"));
        // Trailing slash must be stripped (matches the orchestrator's local_coder
        // cloud rule — validator is shared, so behavior is identical).
        let trimmed = validate_mini_coder_backend(&cloud(
            Some("m"),
            Some("https://example.com/v1/"),
        ))
        .unwrap();
        assert_eq!(trimmed.base_url.as_deref(), Some("https://example.com/v1"));
        // model trimmed.
        let trimmed_model = validate_mini_coder_backend(&cloud(
            Some("  gpt-4o-mini  "),
            Some("https://example.com/v1"),
        ))
        .unwrap();
        assert_eq!(trimmed_model.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn cloud_drops_command_and_keeps_only_model_and_base_url() {
        let n = validate_mini_coder_backend(&cloud(
            Some("m"),
            Some("https://example.com/v1"),
        ))
        .unwrap();
        assert_eq!(n.command, None, "command must be dropped for cloud");
        // JSON serialization round-trip: only kind + model + baseUrl present.
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("\"kind\":\"cloud\""), "json: {json}");
        assert!(json.contains("\"model\":\"m\""), "json: {json}");
        assert!(json.contains("\"baseUrl\""), "json: {json}");
        assert!(!json.contains("command"), "command leaked: {json}");
        let back: MiniCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn cloud_rejects_loopback_http_and_bad_scheme() {
        // The SAME accept/reject set as the orchestrator's Cloud kind (shared
        // validator). Loopback must be rejected: the local kinds (ollama/omlx)
        // are the loopback path; cloud is intentionally remote-only.
        //
        // NOT in the reject list (deliberate, by design of the shared validator):
        //   - `https://127.0.0.1.evil.com/v1`  — a LEGITIMATE public FQDN owned
        //     by evil.com. The host string parses as a hostname (label-separator
        //     `.`, not a loopback IPv4Addr), so the shared validator accepts it.
        //     The threat model blocks loopback / intranet / IP-literals / userinfo,
        //     NOT lookalike public suffixed hosts (those are indistinguishable
        //     from any other public domain at the URL-string level).
        //   - `https://localhost.evil.com/v1` — same reasoning; a public FQDN.
        // Complete SSRF protection requires post-DNS-resolution IP filtering
        // (reject RFC1918 / link-local / loopback RESOLVED IPs in the HTTP
        // client's connect layer — a custom reqwest resolver). The shared
        // validator documents this as a deliberate PARTIAL SSRF mitigation
        // and the post-DNS follow-up is a separate workstream (see the
        // `PARTIAL SSRF mitigation` comment in `local_coder::validate_cloud_base_url`).
        // The cloud mini backend MUST agree with the orchestrator's accept set
        // here (per-role parity) — adding them to the reject list would make
        // the mini and orchestrator disagree on what is a valid provider host.
        for bad in [
            "http://localhost:8000/v1",       // http (cleartext)
            "http://127.0.0.1:8000/v1",       // http + loopback
            "http://openrouter.ai/api/v1",    // http (a real provider would never accept http)
            "https://localhost:8000/v1",      // https + loopback is still loopback
            "https://127.0.0.1/v1",           // bare loopback IP
            "ftp://example.com/v1",           // bad scheme
            "example.com/v1",                 // missing scheme
            "https://192.168.0.1/v1",         // intranet IP literal
            "https://example.com@evil.com",   // userinfo trick
        ] {
            assert!(
                validate_mini_coder_backend(&cloud(Some("m"), Some(bad))).is_err(),
                "cloud base url {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn cloud_rejects_ip_literals_and_intranet_names() {
        // The shared `validate_cloud_base_url` rejects IP literals + `.internal` /
        // `.local` / `metadata.google.internal` + bare hostnames (no dot). These
        // cover the SSRF / privacy hardening set; the cloud mini backend MUST
        // accept the same set the orchestrator accepts, so the rejection surface
        // must agree.
        for bad in [
            "https://1.2.3.4/v1",
            "https://[::1]/v1",
            "https://metadata.google.internal/v1",
            "https://service.internal/v1",
            "https://nas.local/v1",
            "https://example", // bare hostname, no dot
        ] {
            assert!(
                validate_mini_coder_backend(&cloud(Some("m"), Some(bad))).is_err(),
                "cloud base url {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn cloud_rejects_empty_model_and_bad_model() {
        // Empty model.
        assert!(
            validate_mini_coder_backend(&cloud(None, Some("https://example.com/v1"))).is_err(),
            "missing model must be rejected"
        );
        assert!(
            validate_mini_coder_backend(&cloud(Some("   "), Some("https://example.com/v1")))
                .is_err(),
            "blank model must be rejected"
        );
        // Bad model (whitespace / metachars / leading dash).
        for bad_model in ["qwen coder", "model;rm", "$(evil)", "-leadinghyphen"] {
            assert!(
                validate_mini_coder_backend(&cloud(Some(bad_model), Some("https://example.com/v1")))
                    .is_err(),
                "model {bad_model:?} must be rejected"
            );
        }
    }

    #[test]
    fn cloud_rejects_empty_base_url() {
        // Missing base_url.
        assert!(
            validate_mini_coder_backend(&cloud(Some("m"), None)).is_err(),
            "missing base url must be rejected"
        );
        // Blank base url.
        assert!(
            validate_mini_coder_backend(&cloud(Some("m"), Some("   "))).is_err(),
            "blank base url must be rejected"
        );
    }

    #[test]
    fn cloud_requires_model_and_base_url_via_helper() {
        // One consolidated test that exercises every "missing" sub-case the
        // executor would surface as a launch-time error. The two prior tests
        // cover each axis in isolation; this one asserts the combined
        // contract: a valid Cloud backend is BOTH (model, https non-loopback).
        for (model, base_url, expect_ok) in [
            (Some("m"), Some("https://example.com/v1"), true),
            (None, Some("https://example.com/v1"), false), // missing model
            (Some("m"), None, false),                      // missing base_url
            (Some("m"), Some("http://example.com/v1"), false), // http (not https)
            (Some("m"), Some("https://localhost/v1"), false),  // loopback
            (Some("qwen coder"), Some("https://example.com/v1"), false), // bad model
            (Some(""), Some("https://example.com/v1"), false),  // blank model
            (Some("m"), Some(""), false),                         // blank base_url
        ] {
            let res = validate_mini_coder_backend(&cloud(model, base_url));
            assert_eq!(
                res.is_ok(),
                expect_ok,
                "cloud(model={model:?}, base_url={base_url:?}) → ok?={expect_ok}, got={res:?}"
            );
        }
    }

    // ── FIX 4: is_false helper — NO-CHURN round-trip for net_blocked ─────────

    /// `net_blocked = false` must NOT appear in the serialized JSON (no-churn).
    #[test]
    fn net_blocked_false_omitted_from_result_json() {
        let r = MiniCoderResult {
            status: "done".into(),
            output: None,
            files_touched: vec![],
            edits: vec![],
            question: None,
            partial: None,
            net_blocked: false,
            folder_write_blocked: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            !json.contains("netBlocked"),
            "net_blocked=false must be omitted: {json}"
        );
        // Round-trip: absent key deserializes back to false.
        let back: MiniCoderResult = serde_json::from_str(&json).unwrap();
        assert!(!back.net_blocked);
    }

    /// `net_blocked = true` must appear in the serialized JSON.
    #[test]
    fn net_blocked_true_present_in_result_json() {
        let r = MiniCoderResult {
            status: "done".into(),
            output: None,
            files_touched: vec![],
            edits: vec![],
            question: None,
            partial: None,
            net_blocked: true,
            folder_write_blocked: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            json.contains("\"netBlocked\":true"),
            "net_blocked=true must be present: {json}"
        );
        let back: MiniCoderResult = serde_json::from_str(&json).unwrap();
        assert!(back.net_blocked);
    }

    /// Same no-churn guarantee on MiniCoderOutcome.
    #[test]
    fn net_blocked_false_omitted_from_outcome_json() {
        let o = MiniCoderOutcome::failed("something went wrong");
        assert!(!o.net_blocked);
        let json = serde_json::to_string(&o).unwrap();
        assert!(
            !json.contains("netBlocked"),
            "net_blocked=false must be omitted from outcome: {json}"
        );
        let back: MiniCoderOutcome = serde_json::from_str(&json).unwrap();
        assert!(!back.net_blocked);
    }

    /// net_blocked=true on an outcome serialises and round-trips.
    #[test]
    fn net_blocked_true_present_in_outcome_json() {
        let mut o = MiniCoderOutcome::default();
        o.net_blocked = true;
        let json = serde_json::to_string(&o).unwrap();
        assert!(
            json.contains("\"netBlocked\":true"),
            "net_blocked=true must be present in outcome: {json}"
        );
        let back: MiniCoderOutcome = serde_json::from_str(&json).unwrap();
        assert!(back.net_blocked);
    }
}
