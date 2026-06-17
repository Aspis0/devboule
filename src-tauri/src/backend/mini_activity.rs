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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

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
// NOW LIVE: the live mini spawn row uses `Dot`; `push_coder_milestone` (the orchestrator
// milestone stream) constructs Hollow/Dot/Sage/Terra from the file-bridge events, so every
// variant is reachable.
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
// NOW LIVE: the live mini is a single `Spawn` entry; the `Coder` milestone row is
// constructed by `push_coder_milestone` for the ORCHESTRATOR's coder-tier milestone stream
// (the file-bridge tail task), so both variants are reachable.
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

/// Hard cap on the number of timeline ENTRIES kept for ONE agent. The orchestrator's
/// coder-milestone stream (`push_coder_milestone`) APPENDS a row per milestone, so over a
/// long session it must be bounded or the per-agent state grows without limit. When the cap
/// is exceeded the OLDEST entries are dropped (FIFO) — the live tail of the timeline is what
/// the user watches; ancient milestones scroll off. Generous: a whole plan run is a handful
/// of milestones, so the cap only ever trims a pathological flood.
const MAX_ENTRIES_PER_AGENT: usize = 200;

/// Append a coder-tier MILESTONE row (the orchestrator's own timeline, fed by the file
/// bridge). Constructs `ConsoleEntry::Coder { node, text, time }`, marks the agent
/// `running` (a milestone means the orchestrator is actively working), clears the `empty`
/// resting flag, and bounds the entries list to [`MAX_ENTRIES_PER_AGENT`] (oldest dropped).
///
/// `time` is a short local clock stamp (`HH:MM:SS`) so the live stream shows WHEN each
/// milestone arrived — the only host-synthesized field; `text` + `node` come verbatim from
/// the (already redacted, label-only) bridge event. Coexists with the mini `Spawn` path:
/// this only appends Coder rows and never touches a mini run, so an agent that has BOTH a
/// spawn card and orchestrator milestones renders them in arrival order.
pub fn push_coder_milestone(
    activity: &mut ConsoleActivity,
    text: &str,
    node: Option<NodeStyle>,
    time: &str,
) {
    let entries = activity.entries.get_or_insert_with(Vec::new);
    entries.push(ConsoleEntry::Coder {
        node,
        text: text.to_string(),
        time: time.to_string(),
    });
    // FIFO bound: drop the oldest entries if we exceed the cap. `drain(..n)` keeps the
    // newest `MAX_ENTRIES_PER_AGENT` rows (the live tail the user is watching).
    if entries.len() > MAX_ENTRIES_PER_AGENT {
        let overflow = entries.len() - MAX_ENTRIES_PER_AGENT;
        entries.drain(0..overflow);
    }
    // A milestone means the orchestrator is live; reflect that in the tab (spinner + pill)
    // and leave the resting/empty state. `run_count` mirrors the mini path's "one active".
    activity.running = Some(true);
    activity.run_count = Some(1);
    activity.empty = None;
}

/// Mark an agent's orchestrator stream as no longer running (the PTY session ended). Flips
/// `running=false` so the Console tab spinner stops, WITHOUT touching the timeline (the
/// final milestones stay visible). A no-op-safe terminal the tail task calls on teardown.
pub fn mark_coder_stopped(activity: &mut ConsoleActivity) {
    activity.running = Some(false);
}

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

/// Hard cap on the number of `DiffLine`s emitted per write action. A 5000-line generated
/// file must not flood the Console; everything beyond this limit is replaced by a single
/// truncation marker so the frontend renders a bounded card.
const DIFF_LINE_CAP: usize = 200;

/// Build a `Vec<DiffLine>` representing the unified diff of `old` → `new` for `path`.
///
/// Algorithm (no LCS crate in tree — simple, correct for the single-replacement case the
/// mini-coder produces):
///   1. Split both sides into lines (preserving content without trailing newline).
///   2. Trim common prefix lines (Ctx) and common suffix lines (Ctx), isolating the changed
///      middle.
///   3. Emit: a Meta "@@ path @@" header, prefix Ctx lines (up to CONTEXT), Del lines (old
///      middle), Add lines (new middle), suffix Ctx lines (up to CONTEXT).
///   4. Cap at [`DIFF_LINE_CAP`] total lines; append a truncation marker if exceeded.
///
/// A pure create (empty `old`) yields all-Add lines. A pure delete (empty `new`) yields
/// all-Del lines. Identical content yields an empty vec (no diff to show).
pub fn build_file_diff(path: &str, old: &str, new: &str) -> Vec<DiffLine> {
    // Number of unchanged context lines shown above/below the changed hunk.
    const CONTEXT: usize = 3;

    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Identical content → no diff.
    if old_lines == new_lines {
        return Vec::new();
    }

    // Trim common prefix.
    let prefix_len = old_lines
        .iter()
        .zip(new_lines.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Trim common suffix (never overlaps the prefix).
    let old_tail = &old_lines[prefix_len..];
    let new_tail = &new_lines[prefix_len..];
    let suffix_len = old_tail
        .iter()
        .rev()
        .zip(new_tail.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_mid = &old_tail[..old_tail.len() - suffix_len];
    let new_mid = &new_tail[..new_tail.len() - suffix_len];

    // Context slice helpers (bounded).
    let ctx_before_start = prefix_len.saturating_sub(CONTEXT);
    let ctx_before = &old_lines[ctx_before_start..prefix_len];

    let suffix_start = old_lines.len() - suffix_len;
    let ctx_after_end = (suffix_start + CONTEXT).min(old_lines.len());
    let ctx_after = &old_lines[suffix_start..ctx_after_end];

    // Assemble the raw diff lines before capping.
    let mut out: Vec<DiffLine> = Vec::new();

    // Meta header — "@@ path @@" so the view labels each hunk.
    out.push(DiffLine {
        t: DiffLineKind::Meta,
        s: format!("@@ {path} @@"),
    });

    for line in ctx_before {
        out.push(DiffLine { t: DiffLineKind::Ctx, s: line.to_string() });
    }
    for line in old_mid {
        out.push(DiffLine { t: DiffLineKind::Del, s: line.to_string() });
    }
    for line in new_mid {
        out.push(DiffLine { t: DiffLineKind::Add, s: line.to_string() });
    }
    for line in ctx_after {
        out.push(DiffLine { t: DiffLineKind::Ctx, s: line.to_string() });
    }

    // Cap: if we exceed DIFF_LINE_CAP, truncate and append the marker.
    if out.len() > DIFF_LINE_CAP {
        out.truncate(DIFF_LINE_CAP);
        out.push(DiffLine {
            t: DiffLineKind::Meta,
            s: "[… diff truncated]".to_string(),
        });
    }

    out
}

/// Append a `write` action (the applied-edit row) to the CURRENT (last) round of the live
/// mini run. kind=write, verb="Write", emit="emit-edits", target=path, ok=true, diff=the
/// real unified diff of the edit (empty when no diff is available, e.g. for non-write paths).
pub fn push_write_action(activity: &mut ConsoleActivity, path: &str, diff: Vec<DiffLine>) {
    if let Some(mini) = live_mini_mut(activity) {
        if let Some(round) = mini.rounds.last_mut() {
            round.actions.push(Action {
                kind: ActionKind::Write,
                verb: "Write".to_string(),
                emit: Some("emit-edits".to_string()),
                target: Some(path.to_string()),
                ok: Some(true),
                status: None,
                diff,
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

// =============================================================================
// FILE BRIDGE TAIL — turn the orchestrator's activity file into live milestones
// =============================================================================
//
// The local orchestrator (`devboule-coder`) runs as a SEPARATE PTY process with a
// ratatui TUI, so it cannot print activity markers to stdout. Instead it APPENDS one
// JSON event per line to `DEVBOULE_ACTIVITY_FILE` (see `devboule-coder/src/activity.rs`).
// At orchestrator launch the host points that env at a per-agent file AND starts the
// poll-tail task below: it watches the file, parses each new whole line into a
// milestone, and pushes a `ConsoleEntry::Coder` into the store for that `agent_id`.
//
// ROBUSTNESS (all by construction, never a panic):
//  * the file may not exist yet → the tail polls and starts reading once it appears;
//  * only WHOLE newline-terminated lines are consumed (a partial trailing write is
//    left for the next tick), so a mid-write read never yields a half line;
//  * a malformed / non-milestone line is SKIPPED, not fatal;
//  * an oversized line (> MAX_LINE_BYTES) is skipped so a single pathological write
//    cannot blow memory; the read itself is bounded per tick (MAX_READ_PER_TICK);
//  * the task self-terminates when its stop flag is set (the lifecycle teardown).
//
// PRIVACY: the bridge events are label-only (already redacted upstream); this task
// adds only a host clock stamp. No raw transcript / secret crosses it.

/// Poll interval between tail reads. Planner phases are seconds apart, so a sub-second
/// poll keeps the Console feeling live without busy-spinning.
const TAIL_POLL_MS: u64 = 300;

/// Max bytes of a SINGLE event line we will parse. A longer line is skipped (the
/// orchestrator caps its labels far below this; this is the host's belt-and-suspenders).
const MAX_LINE_BYTES: usize = 8 * 1024;

/// Max bytes read from the file in ONE poll tick. Bounds the per-tick work so a huge
/// backlog (or a misbehaving writer) can never stall the reactor or balloon memory; the
/// remainder is consumed on subsequent ticks from the saved offset.
const MAX_READ_PER_TICK: usize = 64 * 1024;

/// One milestone event parsed off the bridge file — mirrors the writer's JSON shape
/// `{ "kind": "milestone", "text": "...", "node": "dot|sage|terra|" }`. Unknown extra
/// fields are ignored (forward-compatible); a missing/extra `kind` that is not
/// `"milestone"` is rejected by [`parse_milestone_line`].
#[derive(Debug, Deserialize)]
struct BridgeEvent {
    kind: String,
    text: String,
    #[serde(default)]
    node: String,
}

/// Map the bridge `node` wire string onto the store's [`NodeStyle`]. The empty string
/// is the hollow node; an unknown value falls back to hollow (never an error — a
/// forward/unknown style degrades to the neutral node rather than dropping the row).
fn node_from_wire(node: &str) -> Option<NodeStyle> {
    match node {
        "" => Some(NodeStyle::Hollow),
        "dot" => Some(NodeStyle::Dot),
        "sage" => Some(NodeStyle::Sage),
        "terra" => Some(NodeStyle::Terra),
        _ => Some(NodeStyle::Hollow),
    }
}

/// Parse ONE file line into a `(text, node)` milestone, or `None` to SKIP it (blank,
/// oversized, non-JSON, or not a `kind == "milestone"` event). Pure + directly testable.
fn parse_milestone_line(line: &str) -> Option<(String, Option<NodeStyle>)> {
    let line = line.trim();
    if line.is_empty() || line.len() > MAX_LINE_BYTES {
        return None;
    }
    let event: BridgeEvent = serde_json::from_str(line).ok()?;
    if event.kind != "milestone" {
        return None;
    }
    // Defensive: re-cap the text length on the READ side too (a hand-crafted file could
    // carry a long label). Char-truncate so we never split a codepoint.
    let text = if event.text.chars().count() > MILESTONE_TEXT_CAP {
        event.text.chars().take(MILESTONE_TEXT_CAP).collect()
    } else {
        event.text
    };
    Some((text, node_from_wire(&event.node)))
}

/// Host-side cap on a milestone label (chars). Matches the writer's cap; re-applied here
/// because the file is untrusted input the host reads back.
const MILESTONE_TEXT_CAP: usize = 200;

/// A short local clock stamp (`HH:MM:SS`) for the milestone's `time` field, so the live
/// timeline shows WHEN each phase arrived. Local time matches the user's wall clock.
fn now_clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// One registered tail: its shared STOP flag plus a monotonic GENERATION stamp. The
/// generation lets a teardown tell whether it is still the CURRENT tail for its id (a
/// same-id relaunch bumps the generation), so a stale predecessor teardown becomes a no-op
/// instead of flipping `running=false` on the live successor.
#[derive(Clone)]
struct TailEntry {
    stop: Arc<AtomicBool>,
    generation: u64,
}

/// The managed registry of running tail tasks, keyed by `agent_id`. Each entry is a
/// shared STOP flag + a per-id generation; the launch path inserts one + spawns the task,
/// the teardown path flips it (the task notices within one poll and exits, then drops
/// itself). Registered in `lib.rs` via `.manage(ActivityTailRegistry::default())`.
#[derive(Default)]
pub struct ActivityTailRegistry {
    inner: Mutex<HashMap<String, TailEntry>>,
}

impl ActivityTailRegistry {
    /// Insert (or replace) the stop flag for `agent_id`, returning the flag the spawned
    /// task watches AND the GENERATION it was registered under. A pre-existing flag for the
    /// same id is FIRST flipped (so a relaunch cleanly stops the predecessor's task) then
    /// replaced under a freshly-incremented generation.
    ///
    /// FIX 2(a): the returned `generation` is captured by the spawned task and re-checked at
    /// teardown via [`is_current_generation`]; a relaunch bumps it, so the OLD task's teardown
    /// recognizes it is no longer current and skips `mark_coder_stopped` (no spinner flip-off
    /// on the live successor).
    fn register(&self, agent_id: &str) -> (Arc<AtomicBool>, u64) {
        let flag = Arc::new(AtomicBool::new(false));
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // The new generation is one past the predecessor's (or 0 for a first registration).
        let generation = map.get(agent_id).map(|e| e.generation.wrapping_add(1)).unwrap_or(0);
        let entry = TailEntry {
            stop: Arc::clone(&flag),
            generation,
        };
        if let Some(old) = map.insert(agent_id.to_string(), entry) {
            old.stop.store(true, Ordering::SeqCst);
        }
        (flag, generation)
    }

    /// FIX 2(a): whether a teardown for `(agent_id, generation)` should run `mark_coder_stopped`.
    /// The ONLY case it must NOT is a same-id RELAUNCH: the entry is still present but under a
    /// DIFFERENT (newer) generation — the live successor owns `running`, so flipping it false
    /// would turn off the spinner on a session that is actively pushing milestones.
    ///
    /// A clean `stop()` REMOVES the entry (absent → returns `true` here): teardown SHOULD still
    /// flip `running=false`, because the spinner-clear on an explicit stop relies on exactly
    /// this teardown (see `agents::mark_agent_session_closed`, which calls `stop()` and counts
    /// on the tail to clear the Console `running`). So: present-and-same OR absent → mark;
    /// present-and-different (relaunch) → skip.
    fn should_mark_stopped(&self, agent_id: &str, generation: u64) -> bool {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(agent_id) {
            Some(entry) => entry.generation == generation,
            None => true,
        }
    }

    /// Stop the tail task for `agent_id` (idempotent): flip + remove its flag. The task
    /// sees the flag on its next tick and exits. A missing id is a no-op.
    ///
    /// FIX 4 (TOCTOU): the flag is flipped WHILE STILL HOLDING the map lock, then the entry
    /// removed. If the store happened AFTER releasing the lock, a racing `register()` could
    /// observe no predecessor (entry already removed) yet the predecessor task would not yet
    /// be told to stop — two live tails for one id. Storing under the lock closes that window.
    pub fn stop(&self, agent_id: &str) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get(agent_id) {
            entry.stop.store(true, Ordering::SeqCst);
            map.remove(agent_id);
        }
    }

    /// FIX 3: signal EVERY registered tail to stop and clear the registry. Called from the
    /// app-exit teardown (alongside the PTY reaper) so quit / dev Ctrl-C never leaves a tail
    /// task spinning. Idempotent and safe when no tails are registered (empty map → no-op).
    pub fn stop_all(&self) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for entry in map.values() {
            entry.stop.store(true, Ordering::SeqCst);
        }
        map.clear();
    }
}

/// Sanitize an `agent_id` into a SINGLE safe filename component for the bridge file.
/// Keeps only `[A-Za-z0-9._-]`, maps everything else to `_`, and rejects a name that
/// reduces to empty / `.` / `..` (returns `None` → the caller skips the bridge). This is
/// the path-traversal guard: the agent_id is app-generated but may be caller-influenced,
/// and it is used to build a filesystem path.
///
/// LEADING-DOT TIGHTENING: a cleaned name that STARTS with `.` (e.g. `.hidden`, `...x`)
/// would produce a hidden / odd dotfile. Separators are already neutralized so this is not
/// a traversal bug, but we replace a leading `.` with `_` so the bridge never creates a
/// hidden/odd file. We REPLACE (not reject) so a legitimate id like `.config-1` still gets a
/// usable, visible file (`_config-1.jsonl`) and observability is not silently disabled.
pub fn activity_file_name(agent_id: &str) -> Option<String> {
    let cleaned: String = agent_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }
    // Bound the length so an absurd id cannot create a pathological filename.
    let bounded: String = cleaned.chars().take(128).collect();
    // Tighten: never emit a leading-dot (hidden / odd) filename. Replace the first char's
    // dot with `_` — the rest of the name keeps its dots (legal in a basename).
    let safe = if bounded.starts_with('.') {
        format!("_{}", &bounded[1..])
    } else {
        bounded
    };
    Some(format!("{safe}.jsonl"))
}

/// The subdir (under the projects dir) that holds per-agent bridge files. Kept out of
/// the project repos themselves so a milestone file is never mistaken for project data.
const ACTIVITY_SUBDIR: &str = ".devboule-activity";

/// Resolve the per-agent bridge file path under `projects_dir`, creating the holding
/// subdir. Returns `None` (bridge disabled) when the id cannot be made a safe filename
/// or the subdir cannot be created — observability never blocks a launch.
pub fn activity_file_path(projects_dir: &Path, agent_id: &str) -> Option<PathBuf> {
    let name = activity_file_name(agent_id)?;
    let dir = projects_dir.join(ACTIVITY_SUBDIR);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir.join(name))
}

/// Start the poll-tail task for `agent_id` reading `file_path`. Registers a stop flag in
/// the managed [`ActivityTailRegistry`] and spawns a tokio task that, until stopped:
///   1. reads any NEW whole lines appended since the last offset (bounded per tick),
///   2. parses each into a milestone (skipping malformed/oversized),
///   3. pushes it into the [`MiniActivityStore`] (which emits the snapshot).
/// On stop it flips `running=false` for the agent so the Console tab spinner clears.
///
/// Best-effort: if the registry/store/runtime is unavailable the task simply does
/// nothing — a missing Console never breaks a launch.
pub fn start_activity_tail(app: &AppHandle, agent_id: &str, file_path: PathBuf) {
    let Some(registry) = app.try_state::<ActivityTailRegistry>() else {
        return;
    };
    // Capture the generation this tail was registered under (FIX 2(a)): teardown only marks
    // the agent stopped if this generation is STILL current (a relaunch bumps it → no-op).
    let (stop, generation) = registry.register(agent_id);
    let app = app.clone();
    let agent_id = agent_id.to_string();

    tauri::async_runtime::spawn(async move {
        // Byte offset already consumed from the file. Persists across ticks so we only
        // ever read NEW bytes (true tail, not re-read).
        let mut offset: u64 = 0;
        // A RAW-BYTE carry for a trailing partial line (a write that landed without its
        // newline yet). We carry BYTES — not a decoded String — so a multi-byte UTF-8
        // codepoint that happens to straddle a per-tick read boundary is reassembled
        // INTACT before decoding (decoding a half-codepoint chunk would corrupt it).
        let mut carry: Vec<u8> = Vec::new();

        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }

            // Read new bytes off the reactor thread (blocking file I/O on spawn_blocking).
            let path = file_path.clone();
            let read = tauri::async_runtime::spawn_blocking(move || read_new_chunk(&path, offset))
                .await;

            // FIX 2(b): re-check the stop flag AFTER the await. A `stop()` that landed while
            // the blocking read was in flight must NOT push one more milestone (which would
            // re-assert `running=true` AFTER teardown set it false → a zombie running state).
            // Break before processing the bytes; the offset advance is irrelevant once stopped.
            if stop.load(Ordering::SeqCst) {
                break;
            }

            if let Ok(Some((bytes, new_offset, was_reset))) = read {
                offset = new_offset;
                // FIX 1: the file was truncated/rotated and we restarted from 0. Any carried
                // partial line is from the OLD file — drop it BEFORE extending, or its stale
                // bytes would be prepended to the new file's first line and assemble a
                // FABRICATED milestone that the orchestrator never wrote.
                if was_reset {
                    carry.clear();
                }
                carry.extend_from_slice(&bytes);
                // Consume WHOLE lines (split on the '\n' byte). Decode each COMPLETE line
                // with from_utf8_lossy — by construction a complete line never splits a
                // codepoint, so the lossy decode is exact for well-formed input.
                while let Some(nl) = carry.iter().position(|&b| b == b'\n') {
                    let line_bytes: Vec<u8> = carry.drain(..=nl).collect();
                    let line = String::from_utf8_lossy(&line_bytes);
                    if let Some((text, node)) = parse_milestone_line(&line) {
                        if let Some(store) = app.try_state::<MiniActivityStore>() {
                            let time = now_clock();
                            store.update(&app, &agent_id, |a| {
                                push_coder_milestone(a, &text, node, &time)
                            });
                        }
                    }
                }
                // Guard `carry` from unbounded growth if the writer never emits a newline
                // (it always does, but be defensive): drop an oversized partial line.
                if carry.len() > MAX_LINE_BYTES {
                    carry.clear();
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(TAIL_POLL_MS)).await;
        }

        // Teardown: mark the agent's stream stopped so the tab spinner clears, then emit
        // the final snapshot. Best-effort.
        //
        // FIX 2(a) — RELAUNCH GUARD: a same-`agent_id` relaunch bumps the registry generation
        // and starts a fresh tail (which may already have pushed its first milestone, marking
        // `running=true`). This OLD task must NOT then flip `running=false` and turn off the
        // spinner on the live successor. `should_mark_stopped` returns false ONLY in that
        // relaunch case (entry present under a newer generation); a clean `stop()` removed the
        // entry, so it returns true and we DO clear the spinner (the explicit-stop path relies
        // on exactly this teardown to clear Console `running`). If the registry is gone
        // (app teardown) we still mark stopped — best-effort, the store may also be gone.
        let mark = match app.try_state::<ActivityTailRegistry>() {
            Some(registry) => registry.should_mark_stopped(&agent_id, generation),
            None => true,
        };
        if mark {
            if let Some(store) = app.try_state::<MiniActivityStore>() {
                store.update(&app, &agent_id, mark_coder_stopped);
            }
        }
    });
}

/// Read up to [`MAX_READ_PER_TICK`] NEW raw BYTES from `path` starting at `offset`.
/// Returns `Some((bytes, new_offset, was_reset))` when there were new bytes, `None` when the
/// file is absent / unchanged / unreadable (the tail simply waits). RAW bytes (not decoded) so
/// the caller can reassemble a codepoint that straddles a read boundary before decoding.
///
/// `was_reset` is `true` when the file SHRANK below the saved offset (truncation/rotation) and
/// we restarted from byte 0. The caller MUST drop any persistent partial-line `carry` in that
/// case BEFORE extending it with the new bytes — otherwise a stale fragment from the OLD file
/// would be prepended to the NEW file's first line and assemble a FABRICATED milestone.
/// Blocking I/O — call on a blocking thread. The offset advances by the bytes actually
/// read so the next tick continues from there.
fn read_new_chunk(path: &Path, offset: u64) -> Option<(Vec<u8>, u64, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // File shrank (truncated/rotated) → reset to its start so we don't read garbage from
    // a stale offset past EOF. Report the reset so the caller drops its stale carry.
    let was_reset = offset > len;
    let start = if was_reset { 0 } else { offset };
    if start >= len {
        return None; // nothing new
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let to_read = std::cmp::min((len - start) as usize, MAX_READ_PER_TICK);
    let mut buf = vec![0u8; to_read];
    let n = file.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    buf.truncate(n);
    Some((buf, start + n as u64, was_reset))
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
        push_write_action(&mut activity, "auth.rs", vec![]);
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
        push_write_action(&mut a, "auth.rs", vec![]);
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
    fn push_coder_milestone_appends_a_coder_row_and_marks_running() {
        // Start from the resting empty state (an orchestrator that has not emitted yet).
        let mut a = ConsoleActivity::empty();
        push_coder_milestone(&mut a, "Planning: 3 spine files", Some(NodeStyle::Dot), "14:22:08");

        // A single Coder entry with the exact wire keys.
        let v = to_value(&a).unwrap();
        assert_eq!(v["running"], json!(true), "a milestone means the orchestrator is live");
        assert_eq!(v["runCount"], json!(1));
        assert!(v.get("empty").is_none(), "empty resting flag cleared on first milestone");
        let entries = v["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["type"], json!("coder"));
        assert_eq!(entries[0]["node"], json!("dot"));
        assert_eq!(entries[0]["text"], json!("Planning: 3 spine files"));
        assert_eq!(entries[0]["time"], json!("14:22:08"));
        // No mini card on a standalone coder row (the frontend only renders MiniCard for spawn).
        assert!(entries[0].get("mini").is_none());

        // A second milestone APPENDS (oldest-first), preserving order.
        push_coder_milestone(&mut a, "exploring main.rs", Some(NodeStyle::Hollow), "14:22:09");
        let v = to_value(&a).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["text"], json!("Planning: 3 spine files"));
        assert_eq!(entries[1]["text"], json!("exploring main.rs"));
        // The hollow node serializes to the empty string (NOT "hollow").
        assert_eq!(entries[1]["node"], json!(""));
    }

    #[test]
    fn push_coder_milestone_coexists_with_a_live_mini_spawn_entry() {
        // An agent that already has a mini Spawn card gets orchestrator milestones APPENDED
        // after it — the mini run is never mutated by the coder path.
        let mut a = build_initial("mini · m", "edit a.rs", &["a.rs".to_string()], 1);
        push_coder_milestone(&mut a, "drafted 2 tasks", Some(NodeStyle::Dot), "00:01");
        let entries = a.entries.as_ref().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], ConsoleEntry::Spawn { .. }), "the mini spawn stays first");
        assert!(matches!(entries[1], ConsoleEntry::Coder { .. }), "the milestone is appended");
    }

    #[test]
    fn push_coder_milestone_bounds_entries_fifo() {
        // Flood well past the cap; only the newest MAX_ENTRIES_PER_AGENT survive (FIFO).
        let mut a = ConsoleActivity::empty();
        for i in 0..(MAX_ENTRIES_PER_AGENT + 25) {
            push_coder_milestone(&mut a, &format!("m{i}"), Some(NodeStyle::Hollow), "00:00");
        }
        let entries = a.entries.as_ref().unwrap();
        assert_eq!(entries.len(), MAX_ENTRIES_PER_AGENT, "capped to the per-agent bound");
        // The OLDEST were dropped; the newest is the last pushed.
        match entries.last().unwrap() {
            ConsoleEntry::Coder { text, .. } => {
                assert_eq!(text, &format!("m{}", MAX_ENTRIES_PER_AGENT + 25 - 1));
            }
            _ => panic!("expected a coder entry"),
        }
        // The first surviving row is the one just past the dropped window (no off-by-one).
        match entries.first().unwrap() {
            ConsoleEntry::Coder { text, .. } => assert_eq!(text, "m25"),
            _ => panic!("expected a coder entry"),
        }
    }

    #[test]
    fn mark_coder_stopped_flips_running_false_keeps_timeline() {
        let mut a = ConsoleActivity::empty();
        push_coder_milestone(&mut a, "plan approved", Some(NodeStyle::Sage), "00:05");
        mark_coder_stopped(&mut a);
        assert_eq!(a.running, Some(false), "the tab spinner stops on teardown");
        // The timeline is preserved (the final milestone stays visible).
        assert_eq!(a.entries.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn store_push_coder_milestone_through_inner_mutate_snapshots() {
        // Drive the AppHandle-free core: a coder milestone via the store's mutate path
        // lands in the snapshot exactly as constructed.
        let mut inner = StoreInner::default();
        inner.mutate("orch-1", |a| {
            push_coder_milestone(a, "plan submitted — awaiting approval", Some(NodeStyle::Terra), "09:00")
        });
        let v = to_value(inner.snapshot("orch-1")).unwrap();
        assert_eq!(v["entries"][0]["type"], json!("coder"));
        assert_eq!(v["entries"][0]["node"], json!("terra"));
        assert_eq!(v["entries"][0]["text"], json!("plan submitted — awaiting approval"));
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

    // ---- FILE BRIDGE TAIL --------------------------------------------------

    #[test]
    fn parse_milestone_line_accepts_a_well_formed_event() {
        let line = r#"{"kind":"milestone","text":"Planning: 3 spine files","node":"dot"}"#;
        let (text, node) = parse_milestone_line(line).expect("a valid milestone parses");
        assert_eq!(text, "Planning: 3 spine files");
        assert_eq!(node, Some(NodeStyle::Dot));
    }

    #[test]
    fn parse_milestone_line_maps_every_node_wire_value() {
        let cases = [
            ("", NodeStyle::Hollow),
            ("dot", NodeStyle::Dot),
            ("sage", NodeStyle::Sage),
            ("terra", NodeStyle::Terra),
            ("future-unknown", NodeStyle::Hollow), // unknown degrades to hollow
        ];
        for (wire, expect) in cases {
            let line = format!(r#"{{"kind":"milestone","text":"x","node":"{wire}"}}"#);
            let (_, node) = parse_milestone_line(&line).expect("parses");
            assert_eq!(node, Some(expect), "node {wire:?} maps correctly");
        }
    }

    #[test]
    fn parse_milestone_line_skips_malformed_blank_and_non_milestone() {
        // Blank / whitespace.
        assert!(parse_milestone_line("").is_none());
        assert!(parse_milestone_line("   ").is_none());
        // Non-JSON garbage.
        assert!(parse_milestone_line("not json at all").is_none());
        // Valid JSON but wrong/absent kind.
        assert!(parse_milestone_line(r#"{"kind":"other","text":"x","node":""}"#).is_none());
        assert!(parse_milestone_line(r#"{"text":"x","node":""}"#).is_none());
        // Oversized line is skipped (no parse, no panic).
        let huge = format!(
            r#"{{"kind":"milestone","text":"{}","node":""}}"#,
            "a".repeat(MAX_LINE_BYTES + 10)
        );
        assert!(parse_milestone_line(&huge).is_none(), "oversized line skipped");
    }

    #[test]
    fn parse_milestone_line_recaps_text_on_the_read_side() {
        // A hand-crafted file could carry a label longer than the writer's cap; the read
        // side re-caps to MILESTONE_TEXT_CAP chars (without splitting a codepoint).
        let long = "é".repeat(MILESTONE_TEXT_CAP + 30);
        let line = format!(r#"{{"kind":"milestone","text":"{long}","node":"dot"}}"#);
        let (text, _) = parse_milestone_line(&line).expect("parses");
        assert_eq!(text.chars().count(), MILESTONE_TEXT_CAP);
        assert!(text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn activity_file_name_sanitizes_and_rejects_traversal() {
        assert_eq!(activity_file_name("coder-123").as_deref(), Some("coder-123.jsonl"));
        // The KEY guarantee: the result is a SINGLE flat filename component — every path
        // SEPARATOR (`/` and `\`) is neutralized to '_', so the name can never be a path.
        // Dots are kept (legal in a basename), so `..` survives only INSIDE a longer flat
        // name, which is harmless (it is not a directory hop).
        let name = activity_file_name("../../etc/passwd").unwrap();
        assert!(!name.contains('/') && !name.contains('\\'), "no path separators survive");
        assert!(name.ends_with(".jsonl"));
        // The cleaned name `.._.._etc_passwd` STARTS with a dot → the leading-dot tightening
        // replaces ONLY the first char's dot with `_` (the rest keep their dots), so no
        // hidden/odd file is produced.
        assert_eq!(name, "_._.._etc_passwd.jsonl");
        assert!(!name.starts_with('.'), "no leading-dot (hidden) filename");
        assert_eq!(activity_file_name("a/b\\c").as_deref(), Some("a_b_c.jsonl"));
        // Degenerate ids that reduce to EXACTLY "." / ".." / empty are rejected (bridge
        // disabled) — those are the only names that would be a real traversal hop.
        assert!(activity_file_name("").is_none());
        assert!(activity_file_name(".").is_none());
        assert!(activity_file_name("..").is_none());
        // A separator-only id collapses to underscores (a flat name), never a hop.
        assert_eq!(activity_file_name("/").as_deref(), Some("_.jsonl"));
        // Leading-dot tightening: a legitimate id that begins with `.` is REPLACED (not
        // rejected) so the bridge still produces a usable, VISIBLE file — never a dotfile.
        assert_eq!(activity_file_name(".hidden").as_deref(), Some("_hidden.jsonl"));
        assert_eq!(activity_file_name("...x").as_deref(), Some("_..x.jsonl"));
        // A non-leading dot is untouched (legal in a basename).
        assert_eq!(activity_file_name("v1.2.3").as_deref(), Some("v1.2.3.jsonl"));
    }

    /// A unique, auto-cleaned temp dir for the file-touching tail tests (the crate has
    /// no `tempfile` dev-dep, so mirror the repo's `std::env::temp_dir()` idiom). The
    /// returned guard removes the dir on drop.
    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "aspis-activity-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TestDir(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn read_new_chunk_tails_only_new_bytes_and_advances_offset() {
        use std::io::Write;
        let dir = TestDir::new("readnew");
        let path = dir.path().join("a.jsonl");

        // Absent file → None.
        assert!(read_new_chunk(&path, 0).is_none());

        // First write, read from 0.
        std::fs::write(&path, "line1\n").unwrap();
        let (chunk, off, reset) = read_new_chunk(&path, 0).expect("reads new bytes");
        assert_eq!(chunk, b"line1\n");
        assert_eq!(off, 6);
        assert!(!reset, "a fresh read from 0 is not a reset");

        // No new bytes since the saved offset → None.
        assert!(read_new_chunk(&path, off).is_none());

        // Append more; reading from the saved offset returns ONLY the new bytes.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"line2\n").unwrap();
        let (chunk2, off2, reset2) = read_new_chunk(&path, off).expect("reads appended bytes");
        assert_eq!(chunk2, b"line2\n");
        assert_eq!(off2, 12);
        assert!(!reset2, "an append within bounds is not a reset");

        // Truncation/rotation (file shrank below offset) → reset to start, was_reset=true.
        std::fs::write(&path, "fresh\n").unwrap();
        let (chunk3, _, reset3) = read_new_chunk(&path, off2).expect("reads from reset start");
        assert_eq!(chunk3, b"fresh\n");
        assert!(reset3, "a shrink below the saved offset reports was_reset");
    }

    #[test]
    fn truncation_with_stale_carry_drops_the_fragment_no_phantom_milestone() {
        // FIX 1: the BLOCKER scenario. The OLD file ended on a PARTIAL line (no trailing
        // newline) so the tail kept it in `carry`. The file is then truncated/rotated and a
        // FRESH file is written. The reset MUST drop the stale carry BEFORE extending, or the
        // old fragment + the new file's first bytes assemble a fabricated milestone.
        use std::io::Write;
        let dir = TestDir::new("trunc-carry");
        let path = dir.path().join("a.jsonl");

        // OLD file: a complete milestone + a DANGLING partial line (no '\n').
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            "{}\n{}",
            r#"{"kind":"milestone","text":"old complete","node":"dot"}"#,
            r#"{"kind":"milestone","text":"old PARTIAL fr"# // no closing / newline
        )
        .unwrap();
        drop(f);

        // Mirror the tail loop's carry handling exactly.
        let mut carry: Vec<u8> = Vec::new();
        let mut inner = StoreInner::default();
        let mut consume = |bytes: &[u8], was_reset: bool, carry: &mut Vec<u8>| {
            if was_reset {
                carry.clear();
            }
            carry.extend_from_slice(bytes);
            while let Some(nl) = carry.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = carry.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                if let Some((text, node)) = parse_milestone_line(&line) {
                    inner.mutate("orch", |a| push_coder_milestone(a, &text, node, "00:00"));
                }
            }
        };

        // Tick 1: read the OLD file. The complete line is consumed; the partial stays in carry.
        let (bytes1, off1, reset1) = read_new_chunk(&path, 0).expect("reads old file");
        assert!(!reset1);
        consume(&bytes1, reset1, &mut carry);
        assert!(!carry.is_empty(), "the dangling partial line is carried");

        // Truncate/rotate: a FRESH file whose first bytes would, if prepended with the stale
        // carry, look JSON-ish. Its real first line is a single legitimate milestone.
        std::fs::write(&path, "agment\"}\n{\"kind\":\"milestone\",\"text\":\"new clean\",\"node\":\"sage\"}\n").unwrap();

        // Tick 2: the read reports was_reset=true. The stale carry MUST be dropped first.
        let (bytes2, _off2, reset2) = read_new_chunk(&path, off1).expect("reads fresh file");
        assert!(reset2, "the shrink is reported as a reset");
        consume(&bytes2, reset2, &mut carry);

        // Only the genuine milestones survive: "old complete" (tick 1) + "new clean" (tick 2).
        // The fabricated `old PARTIAL fragment"}` line is NEVER assembled.
        let snap = inner.snapshot("orch");
        let entries = snap.entries.as_ref().expect("entries");
        let texts: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                ConsoleEntry::Coder { text, .. } => text.as_str(),
                ConsoleEntry::Spawn { text, .. } => text.as_str(),
            })
            .collect();
        assert_eq!(
            texts,
            vec!["old complete", "new clean"],
            "no phantom milestone from the stale carry across truncation"
        );
    }

    #[test]
    fn tail_pipeline_parses_file_into_store_milestones() {
        // End-to-end of the tail's INNER logic without an AppHandle: read the bridge
        // file, parse each whole line, push into a store core, and assert the snapshot
        // carries the coder milestones in order — a malformed middle line is skipped.
        use std::io::Write;
        let dir = TestDir::new("pipeline");
        let path = dir.path().join("activity.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"kind":"milestone","text":"Planning: 2 spine files","node":"dot"}}"#).unwrap();
        writeln!(f, "garbage-not-json").unwrap(); // skipped
        writeln!(f, r#"{{"kind":"milestone","text":"exploring main.rs","node":""}}"#).unwrap();
        writeln!(f, r#"{{"kind":"milestone","text":"plan approved","node":"sage"}}"#).unwrap();

        let (bytes, _off, _reset) = read_new_chunk(&path, 0).expect("reads the file");

        // Mirror the tail loop's byte-split → per-line lossy-decode → parse path.
        let mut inner = StoreInner::default();
        for line_bytes in bytes.split_inclusive(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line_bytes);
            if let Some((text, node)) = parse_milestone_line(&line) {
                inner.mutate("orch", |a| push_coder_milestone(a, &text, node, "00:00"));
            }
        }

        let snap = to_value(inner.snapshot("orch")).unwrap();
        let entries = snap["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3, "the malformed line was skipped");
        assert_eq!(entries[0]["text"], json!("Planning: 2 spine files"));
        assert_eq!(entries[0]["node"], json!("dot"));
        assert_eq!(entries[1]["text"], json!("exploring main.rs"));
        assert_eq!(entries[1]["node"], json!(""));
        assert_eq!(entries[2]["text"], json!("plan approved"));
        assert_eq!(entries[2]["node"], json!("sage"));
        assert_eq!(snap["running"], json!(true), "milestones mark the orchestrator live");
    }

    #[test]
    fn activity_tail_registry_register_and_stop_flip_the_flag() {
        let reg = ActivityTailRegistry::default();
        let (flag, gen0) = reg.register("orch-1");
        assert!(!flag.load(Ordering::SeqCst), "fresh flag starts un-stopped");
        assert_eq!(gen0, 0, "first registration is generation 0");

        // Re-registering the same id flips the PREDECESSOR's flag (clean relaunch) and bumps
        // the generation.
        let (flag2, gen1) = reg.register("orch-1");
        assert!(flag.load(Ordering::SeqCst), "predecessor task is told to stop");
        assert!(!flag2.load(Ordering::SeqCst));
        assert_eq!(gen1, 1, "relaunch bumps the generation");

        // stop() flips the current flag; a second stop / unknown id is a no-op.
        reg.stop("orch-1");
        assert!(flag2.load(Ordering::SeqCst), "stop flips the live flag");
        reg.stop("orch-1");
        reg.stop("never-registered");
    }

    #[test]
    fn relaunch_generation_makes_predecessor_teardown_a_no_op() {
        // FIX 2(a): the predecessor tail's teardown must NOT mark the agent stopped after a
        // same-id relaunch bumped the generation — otherwise it would flip `running=false` on
        // the live successor that already pushed a milestone.
        let reg = ActivityTailRegistry::default();
        let (_flag_old, gen_old) = reg.register("orch-1");
        // Relaunch: a fresh tail registers under a NEW generation (predecessor flag flipped).
        let (_flag_new, gen_new) = reg.register("orch-1");
        assert_ne!(gen_old, gen_new);

        // The PREDECESSOR's teardown checks its own (now stale) generation → must be a no-op.
        assert!(
            !reg.should_mark_stopped("orch-1", gen_old),
            "stale predecessor teardown does NOT mark stopped (would kill the live successor)"
        );
        // The SUCCESSOR's teardown (its generation is current) WOULD mark stopped.
        assert!(
            reg.should_mark_stopped("orch-1", gen_new),
            "the current generation's teardown marks stopped"
        );
    }

    #[test]
    fn clean_stop_lets_teardown_still_mark_stopped() {
        // FIX 2(a) counterpart: an EXPLICIT stop() removes the entry. The teardown must STILL
        // mark stopped (absent entry → true), because the spinner-clear on an explicit stop
        // relies on this teardown (mark_agent_session_closed calls stop() and counts on the
        // tail to flip Console `running=false`).
        let reg = ActivityTailRegistry::default();
        let (_flag, gen) = reg.register("orch-1");
        reg.stop("orch-1");
        assert!(
            reg.should_mark_stopped("orch-1", gen),
            "after a clean stop (no relaunch) the teardown still clears the spinner"
        );
    }

    #[test]
    fn stop_sets_flag_while_holding_the_lock_no_toctou() {
        // FIX 4: stop() must flip the flag BEFORE the entry is observable-as-removed, so a
        // racing register() never sees an absent predecessor whose task was not yet signaled.
        // We can't easily drive the data race in a unit test, but we assert the observable
        // contract: after stop() returns, the predecessor flag is set AND the entry is gone
        // (so a subsequent register() starts a fresh generation 0-relative chain cleanly).
        let reg = ActivityTailRegistry::default();
        let (flag, _gen) = reg.register("orch-1");
        reg.stop("orch-1");
        assert!(flag.load(Ordering::SeqCst), "the predecessor flag is set by stop()");
        // The entry is removed: a fresh register() sees NO predecessor → generation resets to 0.
        let (_flag2, gen2) = reg.register("orch-1");
        assert_eq!(gen2, 0, "after stop() removed the entry, the next register is generation 0");
    }

    #[test]
    fn post_await_stop_check_drops_an_in_flight_tick_without_pushing() {
        // FIX 2(b): if stop() lands while a read is in flight, the loop re-checks the stop flag
        // AFTER the await and breaks BEFORE pushing — so no milestone is pushed (which would
        // re-assert running=true) after stop. We mirror the loop's exact post-await gate.
        use std::io::Write;
        let dir = TestDir::new("postawait");
        let path = dir.path().join("a.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"kind":"milestone","text":"would-be-pushed","node":"dot"}}"#).unwrap();
        drop(f);

        let stop = Arc::new(AtomicBool::new(false));
        let mut inner = StoreInner::default();

        // Simulate one tick: the (blocking) read completed and returned bytes...
        let read = read_new_chunk(&path, 0);

        // ...but a stop() landed during the read. The post-await gate must break first.
        stop.store(true, Ordering::SeqCst);

        if stop.load(Ordering::SeqCst) {
            // break — do NOT process the bytes.
        } else if let Some((bytes, _off, was_reset)) = read {
            let mut carry: Vec<u8> = Vec::new();
            if was_reset {
                carry.clear();
            }
            carry.extend_from_slice(&bytes);
            while let Some(nl) = carry.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = carry.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                if let Some((text, node)) = parse_milestone_line(&line) {
                    inner.mutate("orch", |a| push_coder_milestone(a, &text, node, "00:00"));
                }
            }
        }

        // Nothing was pushed: the agent never became known/running via this tick.
        let snap = inner.snapshot("orch");
        assert!(
            snap.entries.is_none(),
            "a stop during the in-flight read must NOT push a milestone after stop"
        );
        assert_ne!(snap.running, Some(true), "running is not re-asserted after stop");
    }

    #[test]
    fn stop_all_signals_every_tail_and_clears_the_registry() {
        // FIX 3: app-exit teardown. stop_all() flips EVERY registered flag and empties the
        // map. Idempotent + safe when no tails are registered.
        let reg = ActivityTailRegistry::default();
        let (flag_a, _) = reg.register("orch-a");
        let (flag_b, _) = reg.register("orch-b");
        assert!(!flag_a.load(Ordering::SeqCst));
        assert!(!flag_b.load(Ordering::SeqCst));

        reg.stop_all();
        assert!(flag_a.load(Ordering::SeqCst), "every tail flag flipped on app exit");
        assert!(flag_b.load(Ordering::SeqCst));

        // After stop_all the map is empty → a teardown for a now-absent id still marks stopped
        // (absent → true), and a second stop_all on an empty registry is a harmless no-op.
        reg.stop_all();

        // A fresh ActivityTailRegistry with no tails: stop_all is a no-op (no panic).
        let empty = ActivityTailRegistry::default();
        empty.stop_all();
    }

    // ---- build_file_diff tests -----------------------------------------------

    #[test]
    fn diff_single_changed_line_produces_del_then_add() {
        let old = "hello\nworld\nfoo\n";
        let new = "hello\nWORLD\nfoo\n";
        let diff = build_file_diff("src/a.rs", old, new);
        // Should have: Meta header, Ctx "hello" (prefix context), Del "world", Add "WORLD",
        // Ctx "foo" (suffix context).
        let meta = diff.iter().find(|d| d.t == DiffLineKind::Meta).unwrap();
        assert!(meta.s.contains("src/a.rs"), "meta header must include the path");
        let del = diff.iter().find(|d| d.t == DiffLineKind::Del).unwrap();
        assert_eq!(del.s, "world");
        let add = diff.iter().find(|d| d.t == DiffLineKind::Add).unwrap();
        assert_eq!(add.s, "WORLD");
        // Exactly one Del and one Add.
        let del_count = diff.iter().filter(|d| d.t == DiffLineKind::Del).count();
        let add_count = diff.iter().filter(|d| d.t == DiffLineKind::Add).count();
        assert_eq!(del_count, 1);
        assert_eq!(add_count, 1);
    }

    #[test]
    fn diff_pure_create_yields_all_add() {
        let old = "";
        let new = "line1\nline2\nline3\n";
        let diff = build_file_diff("new_file.rs", old, new);
        // Every non-Meta line must be Add.
        let non_meta: Vec<_> = diff.iter().filter(|d| d.t != DiffLineKind::Meta).collect();
        assert!(!non_meta.is_empty(), "pure create must emit lines");
        for line in &non_meta {
            assert_eq!(line.t, DiffLineKind::Add, "pure create lines must all be Add");
        }
        // No Del lines.
        assert!(
            diff.iter().all(|d| d.t != DiffLineKind::Del),
            "pure create must have no Del lines"
        );
    }

    #[test]
    fn diff_pure_delete_yields_all_del() {
        let old = "line1\nline2\n";
        let new = "";
        let diff = build_file_diff("old.rs", old, new);
        let non_meta: Vec<_> = diff.iter().filter(|d| d.t != DiffLineKind::Meta).collect();
        assert!(!non_meta.is_empty(), "pure delete must emit lines");
        for line in &non_meta {
            assert_eq!(line.t, DiffLineKind::Del, "pure delete lines must all be Del");
        }
        assert!(
            diff.iter().all(|d| d.t != DiffLineKind::Add),
            "pure delete must have no Add lines"
        );
    }

    #[test]
    fn diff_identical_content_yields_empty() {
        let content = "alpha\nbeta\ngamma\n";
        let diff = build_file_diff("same.rs", content, content);
        assert!(diff.is_empty(), "identical old and new must produce an empty diff");
    }

    #[test]
    fn diff_large_file_is_capped_with_truncation_marker() {
        // Build a file with 500 lines; change one line in the middle so the diff
        // would emit far more than DIFF_LINE_CAP lines (Del + Add alone is 2, but the
        // unchanged prefix/suffix forces many Ctx lines, and the total could exceed 200).
        // For a more direct test, make old/new differ on EVERY line so we get 500 Del +
        // 500 Add = 1000+ raw lines.
        let old: String = (0..300).map(|i| format!("old line {i}\n")).collect();
        let new: String = (0..300).map(|i| format!("new line {i}\n")).collect();
        let diff = build_file_diff("big.rs", &old, &new);
        assert!(
            diff.len() <= DIFF_LINE_CAP + 1,
            "capped diff must not exceed DIFF_LINE_CAP + 1 (truncation marker): got {}",
            diff.len()
        );
        let last = diff.last().expect("must have at least the marker");
        assert_eq!(last.t, DiffLineKind::Meta, "truncation marker must be Meta kind");
        assert!(
            last.s.contains("truncated"),
            "truncation marker text must mention 'truncated': {:?}",
            last.s
        );
    }

    #[test]
    fn push_write_action_carries_diff_through_to_action() {
        let mut activity = build_initial("mini", "edit x.rs", &["x.rs".to_string()], 1);
        let diffs = vec![
            DiffLine { t: DiffLineKind::Meta, s: "@@ x.rs @@".to_string() },
            DiffLine { t: DiffLineKind::Del, s: "old".to_string() },
            DiffLine { t: DiffLineKind::Add, s: "new".to_string() },
        ];
        push_write_action(&mut activity, "x.rs", diffs.clone());
        let mini = match &activity.entries.as_ref().unwrap()[0] {
            ConsoleEntry::Spawn { mini, .. } => mini,
            _ => panic!("expected spawn entry"),
        };
        let action = &mini.rounds[0].actions[0];
        assert_eq!(action.kind, ActionKind::Write);
        assert_eq!(action.diff, diffs, "diff must be threaded through to the Action");
    }
}
