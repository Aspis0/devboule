//! Agent Activity Console — the LIVE backend (Step B) that makes the already-shipped
//! frontend Console light up. The wire CONTRACT is owned by
//! `src/components/agents/agentConsoleModel.ts` (the data shapes) and
//! `src/components/agents/useAgentConsole.ts` (the transport): the Rust structs here
//! mirror that TS 1:1 (camelCase JSON, skip-empty no-churn), the `mini_activity_snapshot`
//! command returns a `ConsoleActivity`, and the per-agent `mini-activity://<agentId>`
//! channel emits `MiniActivityEvent` payloads.
//!
//! SNAPSHOT-ONLY (DELIBERATE): we emit ONLY the full `snapshot` variant — never the
//! frontend's append/set deltas. The hook subscribes BEFORE fetching the command snapshot
//! and BUFFER-AND-REPLAYS every event that arrives during the await. Append deltas are NOT
//! idempotent under that replay (a delta already reflected in the command snapshot would be
//! double-applied); a full `snapshot` IS idempotent (replace), so replaying any number of
//! them converges to the latest state with zero double-application. The hook's contract
//! blesses this explicitly: "The backend may emit ONLY `snapshot` events and this hook is
//! fully correct." Mini runs are low-frequency (rounds ≥1.5s apart, a handful of actions),
//! so full snapshots are cheap.
//!
//! SNAPSHOT/STREAM CONSISTENCY BY CONSTRUCTION: the store is monotonic per agent — every
//! mutation runs under the lock, stores the resulting FULL state, and emits THAT full state
//! UNDER the same lock (FIX 4: emit order == mutation order by construction, so two
//! concurrent same-agent updates can never deliver out of order). The command reads the
//! current full state. So any state the command missed (a mutation that landed during the
//! subscribe→fetch window) is captured by a buffered event, and replace is idempotent → the
//! hook always converges. Preserve that invariant: never emit a partial update, never mutate
//! the store without emitting.
//!
//! PRIVACY: every string surfaced here is already a redacted, human-readable summary the
//! engine produced (model label, file targets, verb labels, finding titles). No raw
//! transcript / token / secret crosses this channel.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::mini_coder::EscalationFinding;

// ---- wire structs (mirror agentConsoleModel.ts 1:1) -------------------------

/// One line of a unified-diff hunk shown under an expanded write action. `t` drives the
/// row color (meta/add/del/ctx); `s` is the line text WITHOUT the leading sigil. Modeled
/// for completeness; the first cut emits no diffs (action rows are detail-free).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub t: DiffLineKind,
    pub s: String,
}

/// `DiffLine.t` — the lowercase string enum the view switches on for the row color.
// CONTRACT COMPLETENESS: this first cut emits no diffs, so the variants are not yet
// constructed — but they mirror the TS `DiffLine.t` 1:1 (verified by serde) so a later cut
// that surfaces unified-diff hunks needs no new type. `dead_code` allowed for that reason.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Meta,
    Add,
    Del,
    Ctx,
}

/// `Action.kind` — what an action did, for icon mapping. Anything else renders `run`.
// CONTRACT COMPLETENESS: this cut only emits `Write` action rows (the applied edits);
// read/run/search are part of the wire contract for a richer later cut. `dead_code` allowed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    Read,
    Write,
    Run,
    Search,
}

/// A single tool action inside a round (read/write/run/search). Collapsed by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub kind: ActionKind,
    /// Short capitalized label, e.g. "Read" / "Write" / "Run" / "Search".
    pub verb: String,
    /// Optional indigo pill, e.g. "emit-edits", shown before the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit: Option<String>,
    /// The file path / command / query the action operated on (monospace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Terminal success: `false` => coral "fail" pill. `None` (default) => sage "ok".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// `Some(Run)` => a neutral "running" pill (action still in flight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ActionStatus>,
    /// A unified-diff hunk to reveal on expand (write actions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff: Vec<DiffLine>,
    /// Generic monospace output to reveal on expand (read/search/run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// `Action.status` — only `"run"` exists (a running action shows the neutral pill).
// CONTRACT COMPLETENESS: write rows are stamped terminal (`ok`), never in-flight, so this
// is not constructed yet — kept to mirror the TS `Action.status`. `dead_code` allowed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Run,
}

/// `Finding.sev` — note `med` (NOT "medium"); the view renders the label "medium".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    High,
    Med,
    Low,
}

/// One Censor finding under a DIRTY verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub sev: FindingSeverity,
    /// A monospace teal location, e.g. "auth.rs:42".
    pub loc: String,
    /// The human-readable finding message.
    pub msg: String,
}

/// `Verdict.state` — clean (sage shield) / dirty (coral shield + findings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictState {
    Clean,
    Dirty,
}

/// The Censor verdict that closes a round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub state: VerdictState,
    /// A human files summary, e.g. "2 files". ABSENT => the view omits the files clause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<String>,
    /// Only meaningful (and only rendered) when `state == Dirty`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

/// One fix-loop round inside a mini run: a "ROUND n" marker, its actions, and an optional
/// closing Censor verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Round {
    pub n: u32,
    // WIRE CONTRACT: `actions` is a REQUIRED array in agentConsoleModel.ts (`actions: Action[]`,
    // no `?`). It MUST always serialize — even empty — or the frontend gets `undefined` and
    // crashes on `.map`/`.length`. This is the COMMON case (`build_initial` opens a round with
    // no actions). `serde(default)` stays for deser robustness; NO `skip_serializing_if`.
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
}

/// `Banner.kind` — the terminal status of a mini run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BannerKind {
    Done,
    Esc,
    Stop,
}

/// The terminal status banner of a mini run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Banner {
    pub kind: BannerKind,
    /// Override the default title ("Done" / "Escalated" / "Stopped").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A muted trailing sub-line, e.g. "2 files · 1 round · edits applied".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
}

/// A delegated mini-coder run, nested under a `spawn` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MiniRun {
    /// Monospace model label, e.g. "mini · sonnet-4".
    pub model: String,
    /// The scope files the mini was given (right-aligned chips).
    // WIRE CONTRACT: `scope` is a REQUIRED array in agentConsoleModel.ts (`scope: string[]`,
    // no `?`) — always serialize, even empty (a scopeless mini), or the frontend crashes on
    // `.map`/`.length`. `serde(default)` stays for deser robustness; NO `skip_serializing_if`.
    #[serde(default)]
    pub scope: Vec<String>,
    // WIRE CONTRACT: `rounds` is a REQUIRED array in agentConsoleModel.ts (`rounds: Round[]`,
    // no `?`) — always serialize, even empty, or the frontend crashes on `.map`/`.length`.
    // `serde(default)` stays for deser robustness; NO `skip_serializing_if`.
    #[serde(default)]
    pub rounds: Vec<Round>,
    /// A live shimmer line shown after the last round while mid-flight. Absent once terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working: Option<String>,
    /// The terminal status banner. Absent while still in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<Banner>,
}

/// Timeline node style: ""=hollow teal, dot=filled teal, sage/terra=colored ring.
// CONTRACT COMPLETENESS: the live mini spawn row uses `Dot`; the others mirror the TS union
// for coder-milestone rows a later cut adds. `dead_code` allowed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStyle {
    /// The empty-string variant `""` (hollow teal). serde renames the unit field below.
    #[serde(rename = "")]
    Hollow,
    Dot,
    Sage,
    Terra,
}

/// A single top-level row of the timeline — the TS `ConsoleEntry` tagged union
/// (`{type:"coder"|"spawn"}`). A coder milestone row, or a spawn row that OWNS a `MiniRun`.
// CONTRACT COMPLETENESS: the live mini is a single `Spawn` entry; the `Coder` milestone row
// mirrors the TS union for a later cut that surfaces standalone coder milestones.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConsoleEntry {
    /// A coder milestone row (teal chip + text + time).
    Coder {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<NodeStyle>,
        text: String,
        time: String,
    },
    /// A spawn row: a coder milestone that owns a nested `MiniRun` card.
    Spawn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<NodeStyle>,
        text: String,
        time: String,
        mini: MiniRun,
    },
}

/// The whole console state for ONE agent — the exact shape `mini_activity_snapshot` returns
/// and the shape every `MiniActivityEvent` is applied INTO. All fields optional so an
/// absent/partial snapshot degrades to the calm empty state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleActivity {
    /// A run is in flight => the Console tab shows a spinner + `runCount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// How many mini runs are active (shown in the tab pill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_count: Option<u32>,
    /// Explicit calm resting state: render the centered empty state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<bool>,
    /// The timeline, oldest-first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<ConsoleEntry>>,
}

impl ConsoleActivity {
    /// The calm resting state — serializes to exactly `{"empty":true}` (every other field
    /// skips when `None`). Returned by the command for an unknown/never-run agent, and the
    /// base every live mutation builds on.
    pub fn empty() -> Self {
        Self {
            empty: Some(true),
            ..Self::default()
        }
    }

    /// Whether THIS activity represents an in-flight run. Used by the store's CAP eviction
    /// to keep running entries pinned (only finished entries are evictable).
    fn is_running(&self) -> bool {
        self.running == Some(true)
    }
}

// ---- event ------------------------------------------------------------------

/// The per-agent channel payload. ONLY the `snapshot` variant is emitted (see the file
/// header): a full replace is idempotent under the hook's buffer-and-replay, append deltas
/// are not. Tagged exactly like the TS `MiniActivityEvent` snapshot member:
/// `{"type":"snapshot","activity":{...}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MiniActivityEvent {
    Snapshot { activity: ConsoleActivity },
}

/// The channel name for an agent's console stream — MUST match the TS `miniActivityChannel`.
pub fn mini_activity_channel(agent_id: &str) -> String {
    format!("mini-activity://{agent_id}")
}

// ---- store ------------------------------------------------------------------

/// Max distinct agents the console store retains. When exceeded, the OLDEST evictable
/// (non-running) entry is dropped so a long-lived app never grows the map unbounded. A
/// generous cap: the timeline of a finished mini stays selectable long after it ends.
const CAP: usize = 256;

/// In-memory, per-agent monotonic console store (a Tauri `.manage`d type). Holds the latest
/// FULL `ConsoleActivity` for each agent id; mutations replace the stored value and emit it.
/// Terminal states are KEPT (re-selecting a finished mini shows its final timeline) — the
/// CAP eviction is the only bound.
#[derive(Default)]
pub struct MiniActivityStore {
    inner: Mutex<StoreInner>,
}

#[derive(Default)]
struct StoreInner {
    /// agent_id -> latest full activity.
    map: HashMap<String, ConsoleActivity>,
    /// Insertion order of the keys, oldest-first, for CAP eviction. Kept in lockstep with
    /// `map`: an existing key is NOT re-ordered on update (its original insertion position
    /// is preserved), so eviction is true oldest-first.
    order: Vec<String>,
}

impl StoreInner {
    /// The mutate-and-return-clone CORE, factored out so it needs NO `AppHandle` and is
    /// directly unit-testable: get-or-create the entry, run `f`, evict if over CAP, and
    /// return the resulting full activity to emit. The `update` wrapper does the emit.
    fn mutate(&mut self, agent_id: &str, f: impl FnOnce(&mut ConsoleActivity)) -> ConsoleActivity {
        if !self.map.contains_key(agent_id) {
            self.order.push(agent_id.to_string());
            self.map.insert(agent_id.to_string(), ConsoleActivity::empty());
        }
        // Unwrap is safe: just inserted above if it was absent.
        let entry = self.map.get_mut(agent_id).expect("entry present");
        f(entry);
        let result = entry.clone();
        self.evict_if_needed();
        result
    }

    /// CAP eviction: while over the cap, drop the OLDEST NON-running entry. A running entry
    /// is pinned (never evicted mid-flight) — so if every entry is running we keep them all
    /// (the cap is a soft bound that never drops live state). Scans `order` oldest-first.
    fn evict_if_needed(&mut self) {
        while self.map.len() > CAP {
            let victim = self
                .order
                .iter()
                .position(|id| self.map.get(id).map(|a| !a.is_running()).unwrap_or(true));
            match victim {
                Some(idx) => {
                    let id = self.order.remove(idx);
                    self.map.remove(&id);
                }
                // Every remaining entry is running: stop (never evict a live run).
                None => break,
            }
        }
    }

    fn snapshot(&self, agent_id: &str) -> ConsoleActivity {
        self.map
            .get(agent_id)
            .cloned()
            .unwrap_or_else(ConsoleActivity::empty)
    }
}

impl MiniActivityStore {
    /// Mutate `agent_id`'s activity under the lock, then emit the resulting FULL snapshot on
    /// the per-agent channel — STILL UNDER the lock. Panic-poison-resilient: a poisoned mutex
    /// is recovered via `into_inner`.
    pub fn update(&self, app: &AppHandle, agent_id: &str, f: impl FnOnce(&mut ConsoleActivity)) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let activity = inner.mutate(agent_id, f);
        // FIX 4 (ordering guarantee): emit UNDER the lock so emit order == mutation order by
        // construction — the monotonic-per-agent invariant the hook relies on (two concurrent
        // same-agent updates can never emit out of order). SAFE: Tauri v2 `app.emit` is
        // fire-and-forget to the webview, non-reentrant into this store — no deadlock. A failed
        // emit (no listener / torn-down runtime) is non-fatal — the command snapshot still
        // serves the stored state.
        let _ = app.emit(
            &mini_activity_channel(agent_id),
            MiniActivityEvent::Snapshot { activity },
        );
        drop(inner);
    }

    /// The current full activity for `agent_id`, or the empty resting state if unknown.
    pub fn snapshot(&self, agent_id: &str) -> ConsoleActivity {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.snapshot(agent_id)
    }
}

// ---- command ----------------------------------------------------------------

/// `mini_activity_snapshot({ agentId }) -> ConsoleActivity` — the hook's initial hydration.
/// NO vault-unlock gate: it reads only in-memory store state (no secrets, no protected
/// config), mirroring `mini_coder_kill`'s deliberate gate-skip. Tauri maps the camelCase
/// `agentId` arg onto the snake_case `agent_id` param, exactly like `get_agent_token_usage`.
#[tauri::command]
pub fn mini_activity_snapshot(
    agent_id: String,
    store: State<'_, MiniActivityStore>,
) -> ConsoleActivity {
    store.snapshot(&agent_id)
}

// ---- builder / mutator helpers (keep the executor diff tiny) ----------------

/// The live "working" shimmer line shown after the last round while a mini run is mid-flight.
/// Shared by `build_initial` (fresh launch) and `resume_retry_round` (retry resume) so the two
/// shimmer strings are provably IDENTICAL — a retry that resumes the same console must not show
/// a different shimmer than the original launch did.
const WORKING_SHIMMER: &str = "working — running…";

/// Build the INITIAL live activity for a freshly-launched mini round. The whole run is a
/// single `Spawn` entry (the live mini is the only entry a stream mutates): a short
/// task/scope label as the row text, a filled-dot node, and a `MiniRun` carrying the model
/// label + scope + the first empty round + the "working" shimmer. `running=true`,
/// `run_count=1`.
///
/// `round_n` is the 1-based round number for the first round (the executor passes
/// `directive.attempt + 1` since `attempt` is 0-based). For a launch this is normally 1,
/// but a directive whose first finalize already retried could re-build at a higher round —
/// the helper stays general.
pub fn build_initial(model: &str, label: &str, scope: &[String], round_n: u32) -> ConsoleActivity {
    let mini = MiniRun {
        model: model.to_string(),
        scope: scope.to_vec(),
        rounds: vec![Round {
            n: round_n,
            actions: Vec::new(),
            verdict: None,
        }],
        working: Some(WORKING_SHIMMER.to_string()),
        banner: None,
    };
    ConsoleActivity {
        running: Some(true),
        run_count: Some(1),
        empty: None,
        entries: Some(vec![ConsoleEntry::Spawn {
            node: Some(NodeStyle::Dot),
            text: label.to_string(),
            time: String::new(),
            mini,
        }]),
    }
}

/// The live mini run (the last `Spawn` entry), mutable. None if the activity has no entries
/// yet or no spawn entry — every mutator below is then a harmless no-op (the launch builds
/// the spawn entry first, so in practice it is always present once a run started).
fn live_mini_mut(activity: &mut ConsoleActivity) -> Option<&mut MiniRun> {
    let entries = activity.entries.as_mut()?;
    for entry in entries.iter_mut().rev() {
        if let ConsoleEntry::Spawn { mini, .. } = entry {
            return Some(mini);
        }
    }
    None
}

/// Set the verdict on the CURRENT (last) round of the live mini run. No-op if there is no
/// live mini / no round yet.
pub fn set_current_round_verdict(activity: &mut ConsoleActivity, verdict: Verdict) {
    if let Some(mini) = live_mini_mut(activity) {
        if let Some(round) = mini.rounds.last_mut() {
            round.verdict = Some(verdict);
        }
    }
}

/// Append a `write` action (the applied-edit row) to the CURRENT (last) round of the live
/// mini run. kind=write, verb="Write", emit="emit-edits", target=path, ok=true. No diff hunk
/// in this first cut — the contract allows detail-free action rows; a real unified-diff is a
/// follow-up (the executor knows old/new edit bodies and could populate `diff` later).
pub fn push_write_action(activity: &mut ConsoleActivity, path: &str) {
    if let Some(mini) = live_mini_mut(activity) {
        if let Some(round) = mini.rounds.last_mut() {
            round.actions.push(Action {
                kind: ActionKind::Write,
                verb: "Write".to_string(),
                emit: Some("emit-edits".to_string()),
                target: Some(path.to_string()),
                ok: Some(true),
                status: None,
                diff: Vec::new(),
                output: None,
            });
        }
    }
}

/// Append a NEW round to the live mini run (the dirty→retry path: a verdict closed the
/// previous round, the next round begins). The shimmer stays on (the run is still in
/// flight). No-op if there is no live mini.
pub fn append_round(activity: &mut ConsoleActivity, round_n: u32) {
    if let Some(mini) = live_mini_mut(activity) {
        mini.rounds.push(Round {
            n: round_n,
            actions: Vec::new(),
            verdict: None,
        });
    }
}

/// FIX 3: resume a SHARED console for a retry relaunch WITHOUT wiping the predecessor's
/// history. A retry chain reuses ONE `agent_id` (`mini_agent_id` collapses `{root}-r{N}` to
/// the root's id), so the launch hook must be ADDITIVE on a retry instead of re-running
/// `build_initial` (which would erase round 1 — including the dirty verdict that CAUSED the
/// retry). The predecessor's finalize (`AwaitingRetryWith`) already closed the current round
/// (write rows + dirty verdict) and `append_round`ed the next; this helper just re-arms the
/// run flags + shimmer on top of that preserved history.
///
/// * If `a` has NO entries (predecessor finalize was lost/evicted — unexpected): defensively
///   reseed via `build_initial` and return (a console is better than nothing).
/// * Else (the normal resume): flip the run live again (`running=true`, `run_count=1`,
///   `empty=None`), re-light the shimmer (`working=Some(WORKING_SHIMMER)`, `banner=None`),
///   and open round `round_n` ONLY IF the last round's `n < round_n` — the predecessor's
///   `append_round` normally already opened it, so this is idempotent/defensive, never a
///   duplicate round.
pub fn resume_retry_round(
    a: &mut ConsoleActivity,
    model: &str,
    label: &str,
    scope: &[String],
    round_n: u32,
) {
    // No entries at all, OR entries with no live mini run (e.g. only coder rows) -> the
    // predecessor's console CARD is gone; rebuild from scratch so a resumed run always has a
    // visible mini card + an open round. Defensive: every current write to a mini's agent_id
    // goes through `build_initial` (which seeds a spawn entry), so `live_mini_mut` is normally
    // Some here — this guard keeps `resume_retry_round` robust to any future entry shape and
    // never leaves `running=true` on a card that can't render progress.
    let entries_empty = a.entries.as_ref().map(|e| e.is_empty()).unwrap_or(true);
    if entries_empty || live_mini_mut(a).is_none() {
        *a = build_initial(model, label, scope, round_n);
        return;
    }

    // Re-arm the run as live (the relaunch is in flight again).
    a.running = Some(true);
    a.run_count = Some(1);
    a.empty = None;

    if let Some(mini) = live_mini_mut(a) {
        // Re-light the shimmer, clear any predecessor banner (a resumed run is not terminal).
        mini.working = Some(WORKING_SHIMMER.to_string());
        mini.banner = None;
        // Open round `round_n` only if it is not already open (idempotent vs. the
        // predecessor's `append_round`). Never duplicates the current round.
        let needs_open = mini.rounds.last().map(|r| r.n < round_n).unwrap_or(true);
        if needs_open {
            mini.rounds.push(Round {
                n: round_n,
                actions: Vec::new(),
                verdict: None,
            });
        }
    }
}

/// Stamp the TERMINAL banner on the live mini run: clear the shimmer, set the banner, and
/// flip `running=false` (the tab spinner stops).
///
/// FIX 2 INVARIANT: `running=Some(false)` is set REGARDLESS of whether a live mini exists (a
/// terminal is a terminal — a never-seeded stuck-launching directive must still stop showing
/// the tab spinner). The `working`/`banner` mutation only happens when there IS a live mini —
/// so a never-seeded directive stays an empty/resting console (no phantom timeline), just not
/// "running".
pub fn set_terminal(activity: &mut ConsoleActivity, banner: Banner) {
    if let Some(mini) = live_mini_mut(activity) {
        mini.working = None;
        mini.banner = Some(banner);
    }
    activity.running = Some(false);
}

/// Convert the gate's `EscalationFinding`s into a console `Verdict`. CLEAN when there are no
/// findings (sage shield), DIRTY otherwise (coral shield + the findings list). `files` is a
/// short "N file(s)" summary built from the applied-file count (so the meta line reads e.g.
/// "2 files"); `None` when `file_count == 0` so the view omits the files clause entirely
/// rather than fabricating a "0 files".
pub fn verdict_from_findings(findings: &[EscalationFinding], file_count: usize) -> Verdict {
    let state = if findings.is_empty() {
        VerdictState::Clean
    } else {
        VerdictState::Dirty
    };
    let files = if file_count == 0 {
        None
    } else if file_count == 1 {
        Some("1 file".to_string())
    } else {
        Some(format!("{file_count} files"))
    };
    Verdict {
        state,
        files,
        findings: findings.iter().map(finding_from_escalation).collect(),
    }
}

/// Project ONE `EscalationFinding` onto the console `Finding`: severity "medium"→`Med`
/// (anything not high/low maps to med — the view's middle tier); `loc` = `file[:line]`;
/// `msg` = the finding title.
fn finding_from_escalation(f: &EscalationFinding) -> Finding {
    let sev = match f.severity.to_ascii_lowercase().as_str() {
        "high" | "critical" => FindingSeverity::High,
        "low" | "info" => FindingSeverity::Low,
        // "medium"/"med"/anything unknown -> the middle tier (the contract's `"med"`).
        _ => FindingSeverity::Med,
    };
    let loc = match f.line {
        Some(line) if !f.file.is_empty() => format!("{}:{line}", f.file),
        _ => f.file.clone(),
    };
    Finding {
        sev,
        loc,
        msg: f.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value, Value};

    fn esc(severity: &str, file: &str, line: Option<u32>, title: &str) -> EscalationFinding {
        EscalationFinding {
            file: file.to_string(),
            severity: severity.to_string(),
            source: String::new(),
            title: title.to_string(),
            line,
        }
    }

    #[test]
    fn empty_activity_serializes_to_exactly_empty_true() {
        let v = to_value(ConsoleActivity::empty()).unwrap();
        assert_eq!(v, json!({ "empty": true }));
    }

    #[test]
    fn default_activity_serializes_to_empty_object() {
        // A fully-default activity (all None) skips every field -> `{}`, which the hook
        // normalizes to the resting state. (empty() adds the explicit flag.)
        let v = to_value(ConsoleActivity::default()).unwrap();
        assert_eq!(v, json!({}));
    }

    #[test]
    fn populated_activity_has_exact_camelcase_wire_keys() {
        // One spawn entry; a round with a write action + a dirty verdict (one medium
        // finding); a done banner. Assert the EXACT keys + values the frontend expects.
        let mut activity = build_initial(
            "mini · sonnet-4",
            "edit auth.rs",
            &["auth.rs".to_string()],
            1,
        );
        push_write_action(&mut activity, "auth.rs");
        let verdict = verdict_from_findings(
            &[esc("medium", "auth.rs", Some(42), "unchecked unwrap")],
            1,
        );
        set_current_round_verdict(&mut activity, verdict);
        set_terminal(
            &mut activity,
            Banner {
                kind: BannerKind::Done,
                title: None,
                sub: Some("1 file · 1 round · edits applied".to_string()),
            },
        );

        let v = to_value(&activity).unwrap();

        // top-level
        assert_eq!(v["running"], json!(false));
        assert_eq!(v["runCount"], json!(1));
        assert!(v.get("empty").is_none(), "no empty flag on a populated state");
        let entries = v["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 1);

        // the spawn entry (ConsoleEntry tag)
        let entry = &entries[0];
        assert_eq!(entry["type"], json!("spawn"));
        assert_eq!(entry["node"], json!("dot"));
        assert_eq!(entry["text"], json!("edit auth.rs"));

        // the mini run
        let mini = &entry["mini"];
        assert_eq!(mini["model"], json!("mini · sonnet-4"));
        assert_eq!(mini["scope"], json!(["auth.rs"]));
        assert!(mini.get("working").is_none(), "terminal clears working");

        // banner
        assert_eq!(mini["banner"]["kind"], json!("done"));
        assert_eq!(mini["banner"]["sub"], json!("1 file · 1 round · edits applied"));

        // round + action
        let rounds = mini["rounds"].as_array().expect("rounds array");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0]["n"], json!(1));
        let actions = rounds[0]["actions"].as_array().expect("actions array");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["kind"], json!("write"));
        assert_eq!(actions[0]["verb"], json!("Write"));
        assert_eq!(actions[0]["emit"], json!("emit-edits"));
        assert_eq!(actions[0]["target"], json!("auth.rs"));
        assert_eq!(actions[0]["ok"], json!(true));

        // verdict + finding (sev == "med", NOT "medium")
        let verdict = &rounds[0]["verdict"];
        assert_eq!(verdict["state"], json!("dirty"));
        assert_eq!(verdict["files"], json!("1 file"));
        let findings = verdict["findings"].as_array().expect("findings array");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["sev"], json!("med"));
        assert_eq!(findings[0]["loc"], json!("auth.rs:42"));
        assert_eq!(findings[0]["msg"], json!("unchecked unwrap"));
    }

    #[test]
    fn snapshot_event_wire_shape() {
        let event = MiniActivityEvent::Snapshot {
            activity: ConsoleActivity::empty(),
        };
        let v = to_value(&event).unwrap();
        assert_eq!(v, json!({ "type": "snapshot", "activity": { "empty": true } }));
    }

    #[test]
    fn channel_name_matches_frontend() {
        assert_eq!(mini_activity_channel("mini-abc-123"), "mini-activity://mini-abc-123");
    }

    #[test]
    fn store_update_then_snapshot_returns_latest() {
        // Drive the AppHandle-free core directly (the emit path needs a live runtime).
        let mut inner = StoreInner::default();
        inner.mutate("a", |x| *x = build_initial("m1", "t1", &[], 1));
        let after_first = inner.snapshot("a");
        assert_eq!(after_first.run_count, Some(1));

        inner.mutate("a", |x| {
            set_terminal(
                x,
                Banner { kind: BannerKind::Stop, title: None, sub: None },
            )
        });
        let after_second = inner.snapshot("a");
        assert_eq!(after_second.running, Some(false));
        // The last-write wins; the entry is the SAME (mutated in place), not a second one.
        assert_eq!(after_second.entries.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn snapshot_of_unknown_agent_is_empty() {
        let inner = StoreInner::default();
        let v = to_value(inner.snapshot("nope")).unwrap();
        assert_eq!(v, json!({ "empty": true }));
    }

    #[test]
    fn cap_evicts_oldest_non_running_entry_and_pins_running() {
        let mut inner = StoreInner::default();

        // A RUNNING entry inserted FIRST (oldest) — must survive eviction.
        inner.mutate("running-old", |x| *x = build_initial("m", "t", &[], 1));

        // Fill PAST the cap with FINISHED entries. The first finished one ("done-0") is the
        // oldest evictable, so it is the eviction victim once we exceed CAP.
        for i in 0..=CAP {
            let key = format!("done-{i}");
            inner.mutate(&key, |x| {
                *x = build_initial("m", "t", &[], 1);
                set_terminal(x, Banner { kind: BannerKind::Done, title: None, sub: None });
            });
        }

        // Over CAP by exactly one insertion -> exactly one eviction.
        assert_eq!(inner.map.len(), CAP);
        // The running entry is pinned even though it is the oldest key overall.
        assert!(inner.map.contains_key("running-old"), "running entry must survive");
        // The oldest FINISHED entry was the victim.
        assert!(!inner.map.contains_key("done-0"), "oldest completed entry evicted");
        // A recent finished entry is retained.
        assert!(inner.map.contains_key(&format!("done-{CAP}")));
        // order stays in lockstep with map (no stale key left behind).
        assert_eq!(inner.order.len(), inner.map.len());
    }

    #[test]
    fn verdict_from_findings_maps_severity_and_state() {
        // No findings -> CLEAN, no files clause when count is 0.
        let clean = verdict_from_findings(&[], 0);
        assert_eq!(clean.state, VerdictState::Clean);
        assert!(clean.files.is_none());
        assert!(clean.findings.is_empty());

        // Mixed severities -> DIRTY, mapped to the contract's tiers ("medium" -> Med).
        let dirty = verdict_from_findings(
            &[
                esc("high", "a.rs", Some(1), "h"),
                esc("medium", "b.rs", None, "m"),
                esc("low", "c.rs", Some(3), "l"),
            ],
            2,
        );
        assert_eq!(dirty.state, VerdictState::Dirty);
        assert_eq!(dirty.files, Some("2 files".to_string()));
        assert_eq!(dirty.findings.len(), 3);
        assert_eq!(dirty.findings[0].sev, FindingSeverity::High);
        assert_eq!(dirty.findings[0].loc, "a.rs:1");
        // No line -> loc is just the file (no trailing colon).
        assert_eq!(dirty.findings[1].sev, FindingSeverity::Med);
        assert_eq!(dirty.findings[1].loc, "b.rs");
        assert_eq!(dirty.findings[2].sev, FindingSeverity::Low);

        // Serde check: the medium finding's `sev` is the literal "med" on the wire.
        let v: Value = to_value(&dirty.findings[1]).unwrap();
        assert_eq!(v["sev"], json!("med"));
    }

    #[test]
    fn empty_string_node_serializes_to_empty_string() {
        // The hollow node is the empty-string variant `""`, not "hollow".
        let entry = ConsoleEntry::Coder {
            node: Some(NodeStyle::Hollow),
            text: "x".to_string(),
            time: "t".to_string(),
        };
        let v = to_value(&entry).unwrap();
        assert_eq!(v["node"], json!(""));
        assert_eq!(v["type"], json!("coder"));
    }

    #[test]
    fn build_initial_serializes_required_arrays_even_when_empty() {
        // WIRE CONTRACT (FIX 1): `actions`, `scope`, `rounds` are REQUIRED arrays on the TS
        // side. A freshly-built launch snapshot has an EMPTY scope and an EMPTY first-round
        // `actions` — both MUST be present on the wire (`[]`), never omitted, or the frontend
        // gets `undefined` and crashes on `.map`/`.length`. This is the common launch case.
        let v = to_value(build_initial("mini · sonnet-4", "edit auth.rs", &[], 1)).unwrap();
        let mini = &v["entries"][0]["mini"];

        // scope: empty array present (not omitted).
        assert_eq!(mini["scope"], json!([]), "empty scope must serialize as []");
        // rounds: always present (one open round here).
        let rounds = mini["rounds"].as_array().expect("rounds array present");
        assert_eq!(rounds.len(), 1);
        // the open round's actions: empty array present (not omitted).
        assert_eq!(
            rounds[0]["actions"],
            json!([]),
            "empty round actions must serialize as []"
        );
        // Sanity: the genuinely-optional fields ARE still skipped (no regression).
        assert!(rounds[0].get("verdict").is_none(), "absent verdict stays skipped");
    }

    #[test]
    fn resume_retry_round_preserves_predecessor_history() {
        // FIX 3: simulate a retry. Round 1 ran, got a DIRTY verdict (the cause of the retry),
        // and the predecessor's AwaitingRetry finalize already opened round 2 via append_round.
        // The retry relaunch must RESUME this shared console, not wipe round 1.
        let mut a = build_initial("mini · sonnet-4", "edit auth.rs", &["auth.rs".to_string()], 1);
        push_write_action(&mut a, "auth.rs");
        set_current_round_verdict(
            &mut a,
            verdict_from_findings(&[esc("high", "auth.rs", Some(7), "unchecked unwrap")], 1),
        );
        // The predecessor's AwaitingRetry finalize opens the next round.
        append_round(&mut a, 2);

        // The retry relaunch resumes (the launch hook's additive arm).
        resume_retry_round(&mut a, "mini · sonnet-4", "edit auth.rs", &["auth.rs".to_string()], 2);

        // History preserved: round 1 still carries its dirty verdict.
        let mini = match &a.entries.as_ref().unwrap()[0] {
            ConsoleEntry::Spawn { mini, .. } => mini,
            _ => panic!("expected a spawn entry"),
        };
        assert_eq!(mini.rounds.len(), 2, "round 1 must survive; no duplicate round 2");
        assert_eq!(mini.rounds[0].n, 1);
        assert_eq!(
            mini.rounds[0].verdict.as_ref().map(|v| v.state),
            Some(VerdictState::Dirty),
            "round 1's dirty verdict (the retry cause) is preserved"
        );
        // Exactly one round 2 (idempotent vs. the predecessor's append_round — no duplicate).
        assert_eq!(mini.rounds[1].n, 2);
        // The run is live again with the shimmer, no banner.
        assert_eq!(a.running, Some(true));
        assert_eq!(a.run_count, Some(1));
        assert!(a.empty.is_none());
        assert_eq!(mini.working.as_deref(), Some(WORKING_SHIMMER));
        assert!(mini.banner.is_none());
    }

    #[test]
    fn resume_retry_round_rebuilds_when_history_lost() {
        // FIX 3 defensive arm: if the predecessor console was evicted/lost (no entries), the
        // resume must reseed a fresh console rather than leave an empty resting state.
        let mut a = ConsoleActivity::empty();
        resume_retry_round(&mut a, "mini · m", "task", &[], 3);
        assert_eq!(a.running, Some(true));
        let mini = match &a.entries.as_ref().unwrap()[0] {
            ConsoleEntry::Spawn { mini, .. } => mini,
            _ => panic!("expected a spawn entry"),
        };
        assert_eq!(mini.rounds.len(), 1);
        assert_eq!(mini.rounds[0].n, 3, "rebuilt at the resume round number");
        assert_eq!(mini.working.as_deref(), Some(WORKING_SHIMMER));
    }

    #[test]
    fn resume_retry_round_does_not_open_duplicate_when_round_already_present() {
        // If the predecessor did NOT append_round (defensive path), resume opens it once.
        // If it DID, resume must not double it. Here the last round n == round_n -> no open.
        let mut a = build_initial("mini · m", "task", &[], 2);
        // Last round is already n=2; resuming at round_n=2 must not push a second one.
        resume_retry_round(&mut a, "mini · m", "task", &[], 2);
        let mini = match &a.entries.as_ref().unwrap()[0] {
            ConsoleEntry::Spawn { mini, .. } => mini,
            _ => panic!("expected a spawn entry"),
        };
        assert_eq!(mini.rounds.len(), 1, "round_n already open -> no duplicate");
        assert_eq!(mini.rounds[0].n, 2);
    }
}
