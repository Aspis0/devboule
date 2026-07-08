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
use tauri::{AppHandle, Emitter, Manager};

// (was: use super::mini_coder::EscalationFinding; — deleted with verdict gate)
use crate::backend::censor::schema::Finding as CensorFinding;

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
// NOTE: `Eq` is intentionally NOT derived — the `Question` variant (Kairion) carries `f32`
// doubt fields, which are not `Eq`. Only `PartialEq` is needed (the store compares activities
// with `PartialEq`).
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// A websearch row: the query + the REAL pages the orchestrator read (the planner
    /// panel's Websearch view renders these as live sources + distilled findings).
    WebSearch {
        query: String,
        pages: Vec<PageEntry>,
        time: String,
    },
    /// A standalone notice banner (e.g. a web search that completed but returned
    /// no extractable results). Rendered as a muted system line in the timeline.
    Banner {
        text: String,
        time: String,
    },
    /// A model thinking block (pi sessions). Rendered COLLAPSED (one muted row);
    /// the UI expands it on click. `text` is the full thinking content.
    Thinking {
        text: String,
        time: String,
    },
    /// A conversational chat turn (the planner chat): `role` is "assistant" (the
    /// orchestrator talking) or "user" (a steer echoed back) + the message text.
    Chat {
        role: String,
        text: String,
        time: String,
        /// D3 (planner-chat demolition): the client-generated send id, echoed back
        /// through the bridge when the writer knew it (the app's cloud-duplex user
        /// echo). The frontend drains its optimistic pending list BY this id; absent
        /// for local-binary echoes and historical lines (those fall back to text
        /// matching). Omitted from the wire when None (compat with old readers).
        #[serde(rename = "msgId", default, skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
    },
    /// KAIRION (orchestrator-only): a clarification question the orchestrator raised, carrying
    /// the text-doubt signal (unrest / candidates / lean / directionConfidence) so the UI can
    /// render the doubt without a percentage. Tagged `type:"question"`; field names are camelCase
    /// to match the frozen wire shape. `time` is the host clock stamp.
    Question {
        id: String,
        text: String,
        options: Vec<QOption>,
        unrest: f32,
        candidates: Vec<Cand>,
        /// The leaning option (or `null` when genuinely torn). Always serialized.
        lean: Option<String>,
        // The enum-level `rename_all` renames VARIANTS, not struct-variant FIELDS — so this
        // multi-word field needs an explicit camelCase rename to match the frozen wire shape
        // (`directionConfidence`). Every other field here is already single-word.
        #[serde(rename = "directionConfidence")]
        direction_confidence: f32,
        /// "open" | "reopened".
        status: String,
        affects: Vec<String>,
        time: String,
    },
}

/// One selectable option of a Kairion question (`{ "id", "label" }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QOption {
    pub id: String,
    pub label: String,
}

/// One doubt candidate (`{ "label", "pull" }`) — the orchestrator's pull toward an option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cand {
    pub label: String,
    pub pull: f32,
}

/// One web page surfaced by a `websearch` bridge event: source url + title + a
/// distilled summary (the "finding"). Mirrors the orchestrator's `ExaPage` wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageEntry {
    pub url: String,
    pub title: String,
    pub summary: String,
}

/// The whole console state for ONE agent — the exact shape `mini_activity_snapshot` returns
/// and the shape every `MiniActivityEvent` is applied INTO. All fields optional so an
/// absent/partial snapshot degrades to the calm empty state.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
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
    /// Fix2: stable base offset = how many timeline entries have been front-evicted
    /// from the live mapper's history. The frontend computes a STABLE React key =
    /// `entriesBase + i` so a row keeps its identity across FIFO eviction (a plain
    /// 0-based `i` would shift after eviction and bleed per-row state, e.g. an
    /// expanded ThinkingRow adopting a different block). Omitted when 0/absent.
    #[serde(rename = "entriesBase", default, skip_serializing_if = "Option::is_none")]
    pub entries_base: Option<u64>,
    /// Estimated USD cost for the current task (P2 cost tracking).
    /// `None` when the model is unpriced/free or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_cost_estimate_usd: Option<f64>,
    /// B14b: the assistant reply CURRENTLY streaming, if any. A SEPARATE live tail — NOT a
    /// timeline entry — so it is immune to FIFO eviction, interleaved milestone/websearch
    /// events (cloud turns interleave text↔tool), and orphaning. Each `chat-delta` replaces it
    /// with the cumulative reply-so-far; the final `chat` turn lands the real entry and clears
    /// it. The frontend renders it as the last (in-progress) assistant bubble.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_chat: Option<StreamingChat>,
}

/// The live, in-progress assistant reply tail (B14b). `seq` ties it to one turn (for the
/// producer's bookkeeping); `text` is the cumulative reply-so-far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamingChat {
    pub seq: u64,
    pub text: String,
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
#[derive(Debug, Clone, PartialEq, Serialize)]
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
pub async fn mini_activity_snapshot(app: tauri::AppHandle, agent_id: String) -> ConsoleActivity {
    let Some(store) = app.try_state::<MiniActivityStore>() else {
        return ConsoleActivity::empty();
    };
    let snap = store.snapshot(&agent_id);
    if !is_console_blank(&snap) {
        return snap;
    }
    // D2 HYDRATE-ON-MISS: nothing in the render cache (app restart, CAP eviction, or no
    // session this app-run) but the agent may have a durable bridge file — replay its
    // tail so the conversation survives WITHOUT a live session and without the frontend
    // needing a separate durable-read path. Async + spawn_blocking because this is real
    // disk I/O (up to HYDRATE_WINDOW_BYTES) — a burst of snapshot polls must not stall
    // IPC dispatch (hostile-review finding). `mark_running=false`: with no live tail
    // there is no process to spin for; a live tail's own pushes re-assert running within
    // a tick (and its launch-time reset atomically replaces this state anyway, so the
    // benign race between this write and a concurrent tail hydration converges). Ids
    // with no bridge file (minis, coders) fall through untouched — a plain blank snapshot.
    let Ok(projects_dir) = crate::backend::projects::ensure_projects_dir(&app) else {
        return snap;
    };
    let Some(name) = activity_file_name(&agent_id) else {
        return snap;
    };
    let path = projects_dir.join(ACTIVITY_SUBDIR).join(name);
    let hydrated = tauri::async_runtime::spawn_blocking(move || {
        hydrate_from_bridge_file(&path, HYDRATE_WINDOW_BYTES, false)
    })
    .await;
    match hydrated {
        Ok(Some((activity, _))) if !is_console_blank(&activity) => {
            let out = activity.clone();
            store.update(&app, &agent_id, move |a| *a = activity);
            out
        }
        _ => snap,
    }
}

/// One durable chat turn read back from an agent's on-disk activity bridge.
/// Field names match the frontend planner message shape (`role` / `text`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatTurn {
    pub role: String,
    pub text: String,
    /// D3: the echoed client send id, when the bridge line carried one.
    #[serde(rename = "msgId", skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
}

/// B15b: read an agent's DURABLE chat transcript directly from its on-disk
/// `.jsonl` bridge, independent of the in-memory store. Lets the planner chat
/// survive the orchestrator process ending, a store eviction, or an app restart.
/// Best-effort: returns an empty Vec on any resolution/read error (a missing
/// transcript is "no history", never a hard failure).
#[tauri::command]
pub fn read_activity_chat(app: tauri::AppHandle, agent_id: String) -> Vec<ChatTurn> {
    let Ok(projects_dir) = crate::backend::projects::ensure_projects_dir(&app) else {
        return Vec::new();
    };
    // Reviewer max-recall: resolve the path WITHOUT creating the .devboule-activity dir
    // (a read must not mutate the filesystem). `activity_file_name` is the same
    // traversal-safe basename `activity_file_path` uses; we just skip the create_dir_all.
    let Some(name) = activity_file_name(&agent_id) else {
        return Vec::new();
    };
    let path = projects_dir.join(ACTIVITY_SUBDIR).join(name);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(parse_chat_line)
        .map(|chat| ChatTurn {
            role: chat.role,
            text: chat.text,
            msg_id: chat.msg_id,
        })
        .collect()
}

/// D-resume: the chat turns inside a bridge file's HYDRATION WINDOW — a bounded tail
/// read, never the whole file (max-recall: an unbounded read on the launch path means
/// ever-growing relaunch latency for a months-old project). Empty on a missing file.
pub fn recent_chat_turns(path: &Path) -> Vec<ChatTurn> {
    let Some((activity, _)) = hydrate_from_bridge_file(path, HYDRATE_WINDOW_BYTES, false) else {
        return Vec::new();
    };
    activity
        .entries
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| match entry {
            ConsoleEntry::Chat {
                role,
                text,
                msg_id,
                ..
            } => Some(ChatTurn { role, text, msg_id }),
            _ => None,
        })
        .collect()
}

/// D-resume (planner-chat demolition follow-on): format the TAIL of a project's durable
/// chat transcript as a context block for a RELAUNCHED orchestrator's first turn, so a
/// new process (relaunch, app restart, backend switch) resumes the conversation instead
/// of starting amnesiac. Deterministic and zero-LLM, reusing the compact.rs primitives:
/// turns are taken from the END (recency beats lexical relevance for a dialogue — BM25
/// deliberately NOT used here) until `max_turns` or the `budget_tokens` estimate is
/// exhausted; each line is truncated to a per-turn ceiling so one giant paste cannot
/// evict the rest. `None` for an empty history — the caller then sends the goal alone,
/// byte-identical to a first launch.
pub fn format_chat_resume_block(
    turns: &[ChatTurn],
    max_turns: usize,
    budget_tokens: usize,
) -> Option<String> {
    use crate::backend::compact::{estimate_tokens, truncate_to_token_budget};
    /// One turn may not eat more than this many tokens of the resume budget.
    const PER_TURN_TOKEN_CAP: usize = 400;
    if turns.is_empty() || max_turns == 0 || budget_tokens == 0 {
        return None;
    }
    let mut kept: Vec<String> = Vec::new();
    let mut spent = 0usize;
    for turn in turns.iter().rev().take(max_turns) {
        let line = format!(
            "{}: {}",
            turn.role,
            truncate_to_token_budget(&turn.text, PER_TURN_TOKEN_CAP)
        );
        let cost = estimate_tokens(&line);
        if !kept.is_empty() && spent + cost > budget_tokens {
            break;
        }
        spent += cost;
        kept.push(line);
    }
    kept.reverse();
    Some(format!(
        "<conversation-so-far>\n{}\n</conversation-so-far>\n(You are resuming this project's planning conversation after a relaunch or backend switch. The transcript above is what the user already discussed with the previous orchestrator. Continue from there — do not re-ask answered questions, do not re-introduce yourself.)",
        kept.join("\n"),
    ))
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

/// Append a passive ANNOTATION row — identical wire shape to [`push_coder_milestone`]
/// (`ConsoleEntry::Coder { node, text, time }`) but does NOT touch `running`, `run_count`,
/// or `empty`.
///
/// Use this for terminal log notes that must not alter the agent's live/stopped status.
/// The canonical use-case is the Slice 3 Unattended denial note written AFTER the agent
/// has finished: calling `push_coder_milestone` there would set `running = Some(true)` and
/// leave the tab showing a spinner for a completed agent until `spawn_verdict_thread`
/// clears it (up to 30 s), producing a zombie spinner in the console.
///
/// Respects [`MAX_ENTRIES_PER_AGENT`] with the same FIFO drop as `push_coder_milestone`.
pub fn push_coder_note(
    activity: &mut ConsoleActivity,
    label: &str,
    style: Option<NodeStyle>,
    ts: &str,
) {
    let entries = activity.entries.get_or_insert_with(Vec::new);
    entries.push(ConsoleEntry::Coder {
        node: style,
        text: label.to_string(),
        time: ts.to_string(),
    });
    if entries.len() > MAX_ENTRIES_PER_AGENT {
        let overflow = entries.len() - MAX_ENTRIES_PER_AGENT;
        entries.drain(0..overflow);
    }
    // Deliberately NOT touching `running`, `run_count`, or `empty` — passive annotation.
}

/// Append ONE `WebSearch` timeline entry (the query + the real pages just read). Same
/// FIFO cap + live-state bookkeeping as [`push_coder_milestone`].
pub fn push_websearch(
    activity: &mut ConsoleActivity,
    query: &str,
    pages: Vec<PageEntry>,
    time: &str,
) {
    let entries = activity.entries.get_or_insert_with(Vec::new);
    entries.push(ConsoleEntry::WebSearch {
        query: query.to_string(),
        pages,
        time: time.to_string(),
    });
    if entries.len() > MAX_ENTRIES_PER_AGENT {
        let overflow = entries.len() - MAX_ENTRIES_PER_AGENT;
        entries.drain(0..overflow);
    }
    activity.running = Some(true);
    activity.run_count = Some(1);
    activity.empty = None;
}

/// Append ONE `Chat` timeline entry (a conversational turn). Same FIFO cap + live-state
/// bookkeeping as [`push_coder_milestone`].
pub fn push_chat(
    activity: &mut ConsoleActivity,
    role: &str,
    text: &str,
    time: &str,
    msg_id: Option<&str>,
) {
    // B14b: the final assistant turn lands as a real timeline entry — clear the live
    // streaming tail so the in-progress bubble is replaced by the finalized one (no
    // duplicate, no leftover preview). A user turn never clears an in-flight reply.
    if role == "assistant" {
        activity.streaming_chat = None;
    }
    let entries = activity.entries.get_or_insert_with(Vec::new);
    entries.push(ConsoleEntry::Chat {
        role: role.to_string(),
        text: text.to_string(),
        time: time.to_string(),
        msg_id: msg_id.map(str::to_string),
    });
    if entries.len() > MAX_ENTRIES_PER_AGENT {
        let overflow = entries.len() - MAX_ENTRIES_PER_AGENT;
        entries.drain(0..overflow);
    }
    activity.running = Some(true);
    activity.run_count = Some(1);
    activity.empty = None;
}

/// KAIRION: append (or UPSERT) a `Question` timeline entry. If an entry with the same `id`
/// already exists, it is REPLACED in place — so a `"reopened"` question overwrites the prior
/// `"open"` one rather than stacking a duplicate row. A genuinely new id is appended (with the
/// same FIFO cap as the other pushers). Same live-state bookkeeping as [`push_coder_milestone`].
pub fn push_question(activity: &mut ConsoleActivity, q: ParsedQuestionLine, time: &str) {
    let entries = activity.entries.get_or_insert_with(Vec::new);
    let qid = q.id.clone();
    let entry = ConsoleEntry::Question {
        id: q.id,
        text: q.text,
        options: q.options,
        unrest: q.unrest,
        candidates: q.candidates,
        lean: q.lean,
        direction_confidence: q.direction_confidence,
        status: q.status,
        affects: q.affects,
        time: time.to_string(),
    };
    // UPSERT by id: a reopened question replaces the open one in place (no duplicate).
    let existing = entries
        .iter_mut()
        .find(|e| matches!(e, ConsoleEntry::Question { id, .. } if *id == qid));
    if let Some(slot) = existing {
        *slot = entry;
    } else {
        entries.push(entry);
        if entries.len() > MAX_ENTRIES_PER_AGENT {
            let overflow = entries.len() - MAX_ENTRIES_PER_AGENT;
            entries.drain(0..overflow);
        }
    }
    activity.running = Some(true);
    activity.run_count = Some(1);
    activity.empty = None;
}

/// Replace the live STREAMING assistant reply tail with the cumulative reply-so-far for turn
/// `seq` (B14b). This is a SEPARATE slot, NOT a timeline entry: it is immune to FIFO eviction,
/// to interleaved milestone/websearch events (a cloud turn interleaves text↔tool within one
/// reply), and to orphaning. Each delta carries the full reply-so-far, so a replace can never
/// lose or reorder tokens. The final [`push_chat`] lands the real entry and clears this tail.
pub fn push_chat_delta(activity: &mut ConsoleActivity, seq: u64, text: &str) {
    activity.streaming_chat = Some(StreamingChat {
        seq,
        text: text.to_string(),
    });
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
        task_cost_estimate_usd: None,
        streaming_chat: None,
        entries_base: None,
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
    // B14b: drop any stale streaming tail inherited from the predecessor run so a resumed
    // run never shows a half-finished reply that will never be finalized.
    a.streaming_chat = None;

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

/// Convert Phase A Censor findings into a console `Verdict`. CLEAN when there are no
/// findings (sage shield), DIRTY otherwise (coral shield + the findings list).
pub fn verdict_from_findings(findings: &[CensorFinding], file_count: usize) -> Verdict {
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
        findings: findings.iter().map(finding_from_censor).collect(),
    }
}

/// Project ONE `EscalationFinding` onto the console `Finding`: severity "medium"→`Med`
/// (anything not high/low maps to med — the view's middle tier); `loc` = `file[:line]`;
/// `msg` = the finding title.
fn finding_from_censor(f: &CensorFinding) -> Finding {
    use crate::backend::censor::schema::Severity;
    let sev = match f.severity {
        Severity::High => FindingSeverity::High,
        Severity::Low => FindingSeverity::Low,
        _ => FindingSeverity::Med,
    };
    let loc = match f.line {
        Some(line) if !f.file.is_empty() => format!("{}:{line}", f.file),
        Some(line) if f.file.is_empty() => format!("(unknown):{line}"),
        _ if !f.file.is_empty() => f.file.clone(),
        _ => "(unknown)".to_string(),
    };
    Finding { sev, loc, msg: f.title.clone() }
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

/// The `websearch` bridge event wire shape (separate from [`BridgeEvent`]).
#[derive(Deserialize)]
struct WsEvent {
    kind: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    pages: Vec<PageEntry>,
}

/// Parse ONE file line into a `(query, pages)` websearch event, or `None` to SKIP
/// (blank, oversized, non-JSON, or not `kind == "websearch"`). Pure + total. Re-caps
/// the untrusted query/title/summary lengths and the page count on the read side.
fn parse_websearch_line(line: &str) -> Option<(String, Vec<PageEntry>)> {
    let line = line.trim();
    if line.is_empty() || line.len() > MAX_LINE_BYTES {
        return None;
    }
    let event: WsEvent = serde_json::from_str(line).ok()?;
    if event.kind != "websearch" {
        return None;
    }
    let query: String = event.query.chars().take(MILESTONE_TEXT_CAP).collect();
    let pages: Vec<PageEntry> = event
        .pages
        .into_iter()
        .take(6)
        .map(|mut p| {
            p.url = p.url.chars().take(500).collect();
            p.title = p.title.chars().take(160).collect();
            p.summary = p.summary.chars().take(400).collect();
            p
        })
        .collect();
    Some((query, pages))
}

/// The `chat` bridge event wire shape.
#[derive(Deserialize)]
struct ChatEvent {
    kind: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    text: String,
    /// D3: the client-generated send id the writer echoed back (optional).
    #[serde(rename = "msgId", default)]
    msg_id: Option<String>,
}

/// One parsed chat bridge line: normalized role + capped text + the optional echoed
/// send id (D3). A struct, not a widening tuple, so call sites stay readable.
pub(crate) struct ParsedChatLine {
    pub(crate) role: String,
    pub(crate) text: String,
    pub(crate) msg_id: Option<String>,
}

/// Parse ONE file line into a chat turn, or `None` to SKIP (blank, oversized,
/// non-JSON, not `kind == "chat"`, or an unknown role). Pure + total. Role is
/// normalized to "assistant"/"user" (anything else is dropped); text re-capped;
/// the untrusted `msgId` is trimmed, capped, and blank ⇒ absent.
fn parse_chat_line(line: &str) -> Option<ParsedChatLine> {
    let line = line.trim();
    if line.is_empty() || line.len() > MAX_LINE_BYTES {
        return None;
    }
    let event: ChatEvent = serde_json::from_str(line).ok()?;
    if event.kind != "chat" {
        return None;
    }
    let role = match event.role.as_str() {
        "assistant" => "assistant",
        "user" => "user",
        _ => return None,
    };
    let text: String = event.text.chars().take(CHAT_TEXT_CAP).collect();
    if text.trim().is_empty() {
        return None;
    }
    let msg_id = event
        .msg_id
        .map(|id| id.trim().chars().take(128).collect::<String>())
        .filter(|id| !id.is_empty());
    Some(ParsedChatLine {
        role: role.to_string(),
        text,
        msg_id,
    })
}

/// The `chat-delta` bridge event wire shape (B14b): a cumulative snapshot of an assistant
/// turn's reply as it streams. `seq` ties the deltas of one turn together.
#[derive(Deserialize)]
struct ChatDeltaEvent {
    kind: String,
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    text: String,
}

/// Parse ONE file line into a `(seq, text)` streaming chat delta, or `None` to SKIP (blank,
/// oversized, non-JSON, or not `kind == "chat-delta"`). Pure + total. Unlike `parse_chat_line`
/// this does NOT reject empty/whitespace text — a delta may legitimately be short or empty as
/// the reply grows; the host coalesces same-`seq` deltas into one live row. Text is re-capped.
fn parse_chat_delta_line(line: &str) -> Option<(u64, String)> {
    let line = line.trim();
    if line.is_empty() || line.len() > MAX_LINE_BYTES {
        return None;
    }
    let event: ChatDeltaEvent = serde_json::from_str(line).ok()?;
    if event.kind != "chat-delta" {
        return None;
    }
    let text: String = event.text.chars().take(CHAT_TEXT_CAP).collect();
    Some((event.seq, text))
}

/// The `question` bridge event wire shape (Kairion). Mirrors the frozen QUESTION wire line the
/// cloud duplex assembles. `directionConfidence` is camelCase on the wire (via `rename_all`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuestionEvent {
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    options: Vec<QOption>,
    #[serde(default)]
    unrest: f32,
    #[serde(default)]
    candidates: Vec<Cand>,
    #[serde(default)]
    lean: Option<String>,
    #[serde(default)]
    direction_confidence: f32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    affects: Vec<String>,
}

/// A `question` line parsed off the bridge, pre-`time` (the tail adds the host clock stamp).
/// Same shape as the [`ConsoleEntry::Question`] payload minus `time`.
pub struct ParsedQuestionLine {
    pub id: String,
    pub text: String,
    pub options: Vec<QOption>,
    pub unrest: f32,
    pub candidates: Vec<Cand>,
    pub lean: Option<String>,
    pub direction_confidence: f32,
    pub status: String,
    pub affects: Vec<String>,
}

/// Clamp a wire float into [0,1], mapping a non-finite (NaN/Inf) value to 0.0 — untrusted
/// file input must never inject a NaN into the store.
fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Parse ONE file line into a [`ParsedQuestionLine`], or `None` to SKIP (blank, oversized,
/// non-JSON, not `kind == "question"`, or an empty question text). Pure + total, with the same
/// caps/guards as [`parse_chat_delta_line`]: re-caps every untrusted string + count, clamps the
/// floats, and normalizes the status. Mirrors the frozen QUESTION wire shape.
fn parse_question_line(line: &str) -> Option<ParsedQuestionLine> {
    let line = line.trim();
    if line.is_empty() || line.len() > MAX_LINE_BYTES {
        return None;
    }
    let event: QuestionEvent = serde_json::from_str(line).ok()?;
    if event.kind != "question" {
        return None;
    }
    let text: String = event.text.chars().take(CHAT_TEXT_CAP).collect();
    if text.trim().is_empty() {
        return None;
    }
    let id: String = event.id.chars().take(MILESTONE_TEXT_CAP).collect();
    let id = if id.trim().is_empty() {
        "q".to_string()
    } else {
        id
    };
    let options: Vec<QOption> = event
        .options
        .into_iter()
        .take(12)
        .map(|o| QOption {
            id: o.id.chars().take(MILESTONE_TEXT_CAP).collect(),
            label: o.label.chars().take(MILESTONE_TEXT_CAP).collect(),
        })
        .filter(|o| !o.label.trim().is_empty())
        .collect();
    let candidates: Vec<Cand> = event
        .candidates
        .into_iter()
        .take(12)
        .map(|c| Cand {
            label: c.label.chars().take(MILESTONE_TEXT_CAP).collect(),
            pull: clamp01(c.pull),
        })
        .filter(|c| !c.label.trim().is_empty())
        .collect();
    let lean = event
        .lean
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(MILESTONE_TEXT_CAP).collect::<String>());
    let status = match event.status.as_str() {
        "reopened" => "reopened",
        _ => "open",
    }
    .to_string();
    let affects: Vec<String> = event
        .affects
        .into_iter()
        .take(40)
        .map(|a| a.chars().take(MILESTONE_TEXT_CAP).collect::<String>())
        .filter(|s| !s.trim().is_empty())
        .collect();
    Some(ParsedQuestionLine {
        id,
        text,
        options,
        unrest: clamp01(event.unrest),
        candidates,
        lean,
        direction_confidence: clamp01(event.direction_confidence),
        status,
        affects,
    })
}

/// Host-side cap on a milestone label (chars). Matches the writer's cap; re-applied here
/// because the file is untrusted input the host reads back.
const MILESTONE_TEXT_CAP: usize = 200;

/// Host-side cap for a `chat` turn's text (chars) — matches the writer's larger chat cap.
/// A chat reply is prose, not a basename+verb label, so 200 would truncate it mid-sentence.
const CHAT_TEXT_CAP: usize = 2000;

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
    /// Returns `(stop flag, generation, had_predecessor)` — the last tells the caller
    /// whether an entry was actually replaced (a same-id relaunch), so the new tail
    /// can skip its stray-push grace sleep on a genuinely fresh first registration.
    fn register(&self, agent_id: &str) -> (Arc<AtomicBool>, u64, bool) {
        let flag = Arc::new(AtomicBool::new(false));
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // The new generation is one past the predecessor's (or 0 for a first registration).
        let generation = map.get(agent_id).map(|e| e.generation.wrapping_add(1)).unwrap_or(0);
        let entry = TailEntry {
            stop: Arc::clone(&flag),
            generation,
        };
        let had_predecessor = match map.insert(agent_id.to_string(), entry) {
            Some(old) => {
                old.stop.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        };
        (flag, generation, had_predecessor)
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

/// D1 (planner-chat demolition): best-effort purge of an agent's bridge + steer files.
/// Used by project DELETE — project ids are title slugs and can be REUSED by a later
/// unrelated project; the stable orchestrator id (`orchestrator-<project id>`) would
/// then hydrate the dead project's conversation into the namesake. Never creates the
/// holding dir; missing files are the normal case.
pub fn purge_agent_bridge_files(projects_dir: &Path, agent_id: &str) {
    if let Some(name) = activity_file_name(agent_id) {
        let dir = projects_dir.join(ACTIVITY_SUBDIR);
        let _ = std::fs::remove_file(dir.join(&name));
        let _ = std::fs::remove_file(dir.join(format!("{name}.steer")));
    }
}

/// Resolve the per-agent STEER inbox path (app → running orchestrator), mirroring
/// [`activity_file_path`] but with a distinct extension so the two bridge files never
/// collide. The app APPENDS one message per line here; the orchestrator drains it via
/// `DEVBOULE_STEER_FILE`. Deterministic from (projects_dir, agent_id) so the launch and
/// the `orchestrator_steer` command resolve the SAME path without a ledger. `None` when
/// the id can't be made a safe filename or the dir can't be created.
pub fn steer_file_path(projects_dir: &Path, agent_id: &str) -> Option<PathBuf> {
    let name = activity_file_name(agent_id)?;
    let dir = projects_dir.join(ACTIVITY_SUBDIR);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir.join(format!("{name}.steer")))
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
    let (stop, generation, had_predecessor) = registry.register(agent_id);
    let app = app.clone();
    let agent_id = agent_id.to_string();

    tauri::async_runtime::spawn(async move {
        // D2 RESET + HYDRATE: from here on this tail OWNS the console entry for its id.
        // On a same-id RELAUNCH, wait one poll tick first: the just-stopped predecessor
        // (register() flipped its flag under the lock) may still be inside its final
        // synchronous chunk-processing; landing the atomic replace AFTER one tick lets
        // those stray pushes finish so the replace wipes them (their lines are re-read
        // from disk) instead of leaving them stacked as duplicates on top of the
        // hydrated state. A fresh first registration has no predecessor — skip the
        // grace sleep so the console's first paint isn't needlessly delayed.
        if had_predecessor {
            tokio::time::sleep(std::time::Duration::from_millis(TAIL_POLL_MS)).await;
        }
        // Byte offset already consumed from the file. Persists across ticks so we only
        // ever read NEW bytes (true tail, not re-read). Starts where hydration ended.
        let mut offset: u64 = 0;
        if !stop.load(Ordering::SeqCst) {
            let path = file_path.clone();
            let hydrated = tauri::async_runtime::spawn_blocking(move || {
                hydrate_from_bridge_file(&path, HYDRATE_WINDOW_BYTES, true)
            })
            .await;
            // Re-check AFTER the await (same discipline as FIX 2(b) below): if a
            // relaunch superseded us while hydrating, the successor owns the reset —
            // fall through, the loop's first stop-check exits straight to teardown.
            if !stop.load(Ordering::SeqCst) {
                if let Ok(hydrated) = hydrated {
                    // Absent file → plain reset (empty console): stale same-id state
                    // from a previous generation must not leak into this launch.
                    let (activity, end) =
                        hydrated.unwrap_or_else(|| (ConsoleActivity::empty(), 0));
                    offset = end;
                    if let Some(store) = app.try_state::<MiniActivityStore>() {
                        // ATOMIC REPLACE, not a push: reset + hydration land as one
                        // mutation so no observer ever sees stale + new state mixed.
                        store.update(&app, &agent_id, |a| *a = activity);
                    }
                }
            }
        }
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
                    } else if let Some((query, pages)) = parse_websearch_line(&line) {
                        if let Some(store) = app.try_state::<MiniActivityStore>() {
                            let time = now_clock();
                            store.update(&app, &agent_id, |a| {
                                push_websearch(a, &query, pages, &time)
                            });
                        }
                    } else if let Some(chat) = parse_chat_line(&line) {
                        if let Some(store) = app.try_state::<MiniActivityStore>() {
                            let time = now_clock();
                            store.update(&app, &agent_id, |a| {
                                push_chat(a, &chat.role, &chat.text, &time, chat.msg_id.as_deref())
                            });
                        }
                    } else if let Some((seq, text)) = parse_chat_delta_line(&line) {
                        if let Some(store) = app.try_state::<MiniActivityStore>() {
                            store.update(&app, &agent_id, |a| {
                                push_chat_delta(a, seq, &text)
                            });
                        }
                    } else if let Some(question) = parse_question_line(&line) {
                        // KAIRION: a clarification question (with its doubt signal) → UPSERT a
                        // Question timeline entry by id (a reopened question replaces the open one).
                        if let Some(store) = app.try_state::<MiniActivityStore>() {
                            let time = now_clock();
                            store.update(&app, &agent_id, |a| {
                                push_question(a, question, &time)
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

/// D2 (planner-chat demolition): bytes of bridge-file TAIL replayed when a console
/// hydrates from disk — both the tail-start reset and the snapshot-on-miss rebuild the
/// in-memory render cache from the last window of the durable file. ~200 chat turns fit
/// comfortably; anything older stays on disk for `read_activity_chat` (the full-history
/// escape hatch). Bounded so a months-old project file can never stall a project open.
const HYDRATE_WINDOW_BYTES: u64 = 256 * 1024;

/// D2: whether a stored console has NO renderable content — used to decide that a disk
/// hydration is warranted (store miss, CAP eviction, app restart). Resting flags don't
/// count; only timeline entries or a live streaming tail do.
fn is_console_blank(activity: &ConsoleActivity) -> bool {
    activity.entries.as_deref().unwrap_or(&[]).is_empty() && activity.streaming_chat.is_none()
}

/// D2: replay the TAIL of an on-disk bridge file into a FRESH `ConsoleActivity`.
/// Returns the rebuilt activity plus the byte offset consumed through the last COMPLETE
/// line — the live tail continues from exactly there, so hydration + tailing partition
/// the file with no overlap and no gap. `None` only when the file is absent/unreadable
/// (callers treat that as "no history", never an error).
///
/// Semantics:
/// - Reads at most the trailing `window` bytes; a window that starts mid-file begins
///   mid-LINE, so replay starts at the first line boundary inside the window (a whole
///   window with no newline is one giant unterminated fragment — consume nothing and
///   hand the live tail EOF; a later completion parses as malformed and is skipped).
/// - `chat-delta` lines are deliberately NOT replayed: a stale delta would resurrect a
///   ghost streaming bubble for a session that is not streaming.
/// - Replayed rows carry an EMPTY time stamp: bridge lines have no timestamps and
///   stamping the hydration clock would fabricate arrival times.
/// - The pushers stamp `running=true` as they replay; the caller knows better, so the
///   final flag is forced to `mark_running` (launch-time reset ⇒ `true`, a snapshot of
///   a session with no live tail ⇒ `false`) — only when anything was replayed. An empty
///   replay stays the plain resting state.
///
/// Blocking I/O — call on a blocking thread from async contexts.
fn hydrate_from_bridge_file(
    path: &Path,
    window: u64,
    mark_running: bool,
) -> Option<(ConsoleActivity, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(window);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    // Cap the read at the metadata length: the file may grow while we read; growth
    // belongs to the live tail, which continues from the offset we return.
    file.take(len - start).read_to_end(&mut buf).ok()?;
    let mut pos = 0usize;
    if start > 0 {
        match buf.iter().position(|&b| b == b'\n') {
            Some(nl) => pos = nl + 1,
            None => return Some((ConsoleActivity::empty(), len)),
        }
    }
    let mut activity = ConsoleActivity::empty();
    let mut consumed = pos;
    while let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') {
        let line_end = pos + nl;
        let line = String::from_utf8_lossy(&buf[pos..line_end]);
        if let Some((text, node)) = parse_milestone_line(&line) {
            push_coder_milestone(&mut activity, &text, node, "");
        } else if let Some((query, pages)) = parse_websearch_line(&line) {
            push_websearch(&mut activity, &query, pages, "");
        } else if let Some(chat) = parse_chat_line(&line) {
            push_chat(&mut activity, &chat.role, &chat.text, "", chat.msg_id.as_deref());
        } else if let Some(question) = parse_question_line(&line) {
            push_question(&mut activity, question, "");
        }
        pos = line_end + 1;
        consumed = pos;
    }
    activity.streaming_chat = None;
    if !activity.entries.as_deref().unwrap_or(&[]).is_empty() {
        activity.running = Some(mark_running);
    }
    Some((activity, start + consumed as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value, Value};

    fn censor_finding(sev_str: &str, file: &str, line: Option<u32>, title: &str) -> CensorFinding {
        use crate::backend::censor::schema::Severity;
        let severity = match sev_str {
            "high" => Severity::High,
            "low" => Severity::Low,
            _ => Severity::Medium,
        };
        CensorFinding {
            file: file.to_string(),
            title: title.to_string(),
            source: "test".into(),
            line,
            severity,
            ..Default::default()
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
            &[censor_finding("medium", "auth.rs", Some(42), "unchecked unwrap")],
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
        assert!(true);
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

    /// B15b: the durable chat reader keeps only valid chat lines from the on-disk
    /// `.jsonl`, dropping milestones and malformed lines. (Tests the parse path the
    /// `read_activity_chat` command relies on — a tauri::AppHandle can't be built in
    /// a unit test, so we exercise the same `lines().filter_map(parse_chat_line)`.)
    #[test]
    fn read_activity_chat_parses_chat_lines_from_disk() {
        let path = std::env::temp_dir().join(format!(
            "devboule-b15-{}-{}.jsonl",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let content = "{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"ciao\"}\n\
{\"kind\":\"milestone\",\"text\":\"x\"}\n\
{\"kind\":\"chat\",\"role\":\"assistant\",\"text\":\"hello\"}\n\
not json\n";
        std::fs::write(&path, content).unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        let turns: Vec<ChatTurn> = on_disk
            .lines()
            .filter_map(parse_chat_line)
            .map(|c| ChatTurn { role: c.role, text: c.text, msg_id: c.msg_id })
            .collect();
        assert_eq!(turns.len(), 2, "only the two chat lines survive");
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].text, "ciao");
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(turns[1].text, "hello");
        std::fs::remove_file(&path).ok();
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
                censor_finding("high", "a.rs", Some(1), "h"),
                censor_finding("medium", "b.rs", None, "m"),
                censor_finding("low", "c.rs", Some(3), "l"),
            ],
            2,
        );
        assert_eq!(dirty.state, VerdictState::Dirty);
        assert_eq!(dirty.files, Some("2 files".to_string()));
        assert_eq!(dirty.findings.len(), 3);
        assert!(true);
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
            verdict_from_findings(&[censor_finding("high", "auth.rs", Some(7), "unchecked unwrap")], 1),
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
    fn parse_and_push_websearch_round_trips() {
        let line = r#"{"kind":"websearch","query":"stripe usage billing","pages":[{"url":"https://stripe.com/docs","title":"Usage","summary":"meter via UsageRecord"}]}"#;
        let (query, pages) = parse_websearch_line(line).expect("parses a websearch line");
        assert_eq!(query, "stripe usage billing");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].url, "https://stripe.com/docs");
        assert_eq!(pages[0].summary, "meter via UsageRecord");
        // wrong kind / bad json -> None (never panics)
        assert!(parse_websearch_line(r#"{"kind":"milestone","text":"x"}"#).is_none());
        assert!(parse_websearch_line("not json").is_none());
        // push appends a WebSearch entry + marks the stream live
        let mut a = ConsoleActivity::empty();
        push_websearch(&mut a, &query, pages, "14:00:00");
        assert_eq!(a.running, Some(true));
        assert!(a.empty.is_none());
        match &a.entries.as_ref().unwrap()[0] {
            ConsoleEntry::WebSearch { query: q, pages: p, .. } => {
                assert_eq!(q, "stripe usage billing");
                assert_eq!(p.len(), 1);
            }
            _ => panic!("expected a websearch entry"),
        }
    }

    #[test]
    fn parse_and_push_chat_round_trips() {
        let line = r#"{"kind":"chat","role":"assistant","text":"I drafted a 6-task plan."}"#;
        let parsed = parse_chat_line(line).expect("parses a chat line");
        assert_eq!(parsed.role, "assistant");
        assert_eq!(parsed.text, "I drafted a 6-task plan.");
        assert_eq!(parsed.msg_id, None, "no msgId on the wire ⇒ None");
        // wrong kind / unknown role / blank text / bad json -> None
        assert!(parse_chat_line(r#"{"kind":"milestone","text":"x"}"#).is_none());
        assert!(parse_chat_line(r#"{"kind":"chat","role":"system","text":"x"}"#).is_none());
        assert!(parse_chat_line(r#"{"kind":"chat","role":"user","text":"   "}"#).is_none());
        assert!(parse_chat_line("not json").is_none());
        // push appends a Chat entry + marks live
        let mut a = ConsoleActivity::empty();
        push_chat(&mut a, &parsed.role, &parsed.text, "14:00:00", None);
        assert_eq!(a.running, Some(true));
        match &a.entries.as_ref().unwrap()[0] {
            ConsoleEntry::Chat { role: r, text: t, .. } => {
                assert_eq!(r, "assistant");
                assert_eq!(t, "I drafted a 6-task plan.");
            }
            _ => panic!("expected a chat entry"),
        }
    }

    #[test]
    fn parse_chat_line_carries_the_msg_id_through_the_bridge() {
        // D3 (planner-chat demolition): the client-generated send id, echoed back on the
        // wire as `msgId`, is what lets the frontend drain its optimistic pending list BY
        // IDENTITY instead of by counting user rows.
        let line = r#"{"kind":"chat","role":"user","text":"go on","msgId":"m-42"}"#;
        let parsed = parse_chat_line(line).expect("parses");
        assert_eq!(parsed.msg_id.as_deref(), Some("m-42"));
        // Untrusted read side: an absurd msgId is capped, a blank one is dropped to None.
        let big = format!(
            r#"{{"kind":"chat","role":"user","text":"x","msgId":"{}"}}"#,
            "i".repeat(500)
        );
        let capped = parse_chat_line(&big).expect("parses");
        assert!(capped.msg_id.as_deref().unwrap_or("").len() <= 128);
        let blank = parse_chat_line(r#"{"kind":"chat","role":"user","text":"x","msgId":"  "}"#)
            .expect("parses");
        assert_eq!(blank.msg_id, None, "a blank msgId reads back as absent");
        // The id survives push → entry → snapshot serialization as `msgId`.
        let mut a = ConsoleActivity::empty();
        push_chat(&mut a, &parsed.role, &parsed.text, "14:00:00", parsed.msg_id.as_deref());
        let json = to_value(&a).expect("serializes");
        assert_eq!(json["entries"][0]["msgId"], json!("m-42"));
        // And an id-less push serializes WITHOUT the key (wire compat with old readers).
        let mut b = ConsoleActivity::empty();
        push_chat(&mut b, "user", "plain", "14:00:01", None);
        let json_b = to_value(&b).expect("serializes");
        assert!(json_b["entries"][0].get("msgId").is_none());
    }

    #[test]
    fn parse_chat_delta_line_accepts_and_rejects() {
        let (seq, text) =
            parse_chat_delta_line(r#"{"kind":"chat-delta","seq":3,"text":"Hel"}"#).expect("parses");
        assert_eq!(seq, 3);
        assert_eq!(text, "Hel");
        // a delta may be empty as it grows — NOT rejected on blank text
        let (s, t) = parse_chat_delta_line(r#"{"kind":"chat-delta","seq":0,"text":""}"#)
            .expect("empty delta still parses");
        assert_eq!(s, 0);
        assert_eq!(t, "");
        // wrong kind / non-json rejected
        assert!(parse_chat_delta_line(r#"{"kind":"chat","role":"assistant","text":"x"}"#).is_none());
        assert!(parse_chat_delta_line(r#"{"kind":"milestone","text":"x"}"#).is_none());
        assert!(parse_chat_delta_line("not json").is_none());
    }

    #[test]
    fn parse_question_line_accepts_and_rejects() {
        let line = r#"{"kind":"question","id":"q1","text":"Which DB?","options":[{"id":"pg","label":"Postgres"}],"unrest":0.6,"candidates":[{"label":"Postgres","pull":0.7}],"lean":"Postgres","directionConfidence":0.3,"status":"open","affects":["schema.rs"]}"#;
        let q = parse_question_line(line).expect("parses a question line");
        assert_eq!(q.id, "q1");
        assert_eq!(q.text, "Which DB?");
        assert_eq!(q.options[0].label, "Postgres");
        assert_eq!(q.candidates[0].pull, 0.7);
        assert_eq!(q.lean.as_deref(), Some("Postgres"));
        assert_eq!(q.status, "open");
        assert_eq!(q.affects, vec!["schema.rs".to_string()]);
        // wrong kind / blank text / non-json -> None
        assert!(parse_question_line(r#"{"kind":"chat","role":"assistant","text":"x"}"#).is_none());
        assert!(parse_question_line(r#"{"kind":"question","text":"   "}"#).is_none());
        assert!(parse_question_line("not json").is_none());
    }

    #[test]
    fn question_entry_serializes_to_the_frozen_camelcase_wire() {
        let q = parse_question_line(
            r#"{"kind":"question","id":"q1","text":"Which DB?","options":[{"id":"pg","label":"Postgres"}],"unrest":0.6,"candidates":[{"label":"Postgres","pull":0.7}],"lean":null,"directionConfidence":0.25,"status":"open","affects":["a.rs"]}"#,
        )
        .unwrap();
        let mut a = ConsoleActivity::empty();
        push_question(&mut a, q, "14:00:00");
        assert_eq!(a.running, Some(true));
        assert!(a.empty.is_none());
        let v = to_value(&a).unwrap();
        let entry = &v["entries"][0];
        assert_eq!(entry["type"], json!("question"));
        assert_eq!(entry["id"], json!("q1"));
        assert_eq!(entry["text"], json!("Which DB?"));
        assert_eq!(entry["status"], json!("open"));
        // f32 → f64 widening means an exact `0.6` literal won't match; compare with tolerance.
        assert!((entry["unrest"].as_f64().unwrap() - 0.6).abs() < 1e-6);
        assert!((entry["directionConfidence"].as_f64().unwrap() - 0.25).abs() < 1e-6);
        assert_eq!(entry["lean"], Value::Null, "lean is required, null when torn");
        assert_eq!(entry["options"][0]["label"], json!("Postgres"));
        assert!((entry["candidates"][0]["pull"].as_f64().unwrap() - 0.7).abs() < 1e-6);
        assert_eq!(entry["affects"][0], json!("a.rs"));
        assert_eq!(entry["time"], json!("14:00:00"));
    }

    #[test]
    fn push_question_upserts_a_reopened_question_by_id() {
        let mut a = ConsoleActivity::empty();
        let open = parse_question_line(
            r#"{"kind":"question","id":"q1","text":"open?","status":"open","options":[],"candidates":[]}"#,
        )
        .unwrap();
        push_question(&mut a, open, "14:00:00");
        let reopened = parse_question_line(
            r#"{"kind":"question","id":"q1","text":"reopened?","status":"reopened","options":[],"candidates":[]}"#,
        )
        .unwrap();
        push_question(&mut a, reopened, "14:00:05");
        // exactly ONE question row (upserted), now showing the reopened state
        let entries = a.entries.as_ref().unwrap();
        let questions: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Question { .. }))
            .collect();
        assert_eq!(questions.len(), 1, "reopened replaces open — no duplicate");
        match questions[0] {
            ConsoleEntry::Question { status, text, time, .. } => {
                assert_eq!(status, "reopened");
                assert_eq!(text, "reopened?");
                assert_eq!(time, "14:00:05");
            }
            _ => unreachable!(),
        }
        // a DIFFERENT id appends a second row
        let other = parse_question_line(
            r#"{"kind":"question","id":"q2","text":"other?","options":[],"candidates":[]}"#,
        )
        .unwrap();
        push_question(&mut a, other, "14:00:10");
        let count = a
            .entries
            .as_ref()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ConsoleEntry::Question { .. }))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn parse_question_line_clamps_floats_and_caps_lists() {
        // NaN / out-of-range floats are sanitized; lists are capped.
        let line = r#"{"kind":"question","text":"x","unrest":5.0,"directionConfidence":-1.0,"candidates":[{"label":"A","pull":9.0}]}"#;
        let q = parse_question_line(line).unwrap();
        assert_eq!(q.unrest, 1.0, "unrest clamped to 1");
        assert_eq!(q.direction_confidence, 0.0, "negative clamped to 0");
        assert_eq!(q.candidates[0].pull, 1.0);
        assert_eq!(q.id, "q", "missing id defaults to q");
    }

    #[test]
    fn oversized_question_line_is_capped_under_max_and_round_trips() {
        // A pathological orchestrator question (huge text + a long affects list) must NOT produce
        // a line over MAX_LINE_BYTES — otherwise parse_question_line silently DROPS it, losing the
        // whole question. build_question_line caps it; the capped line still round-trips here.
        use crate::backend::doubt_sensor_text::{build_question_line, ParsedQuestion};
        let q = ParsedQuestion {
            id: "q1".to_string(),
            text: "x".repeat(40_000),
            options: vec![("a".to_string(), "Alpha".to_string())],
            affects: (0..200).map(|i| format!("path/to/file_{i}.rs")).collect(),
            status: "open".to_string(),
        };
        let line = build_question_line("maybe Alpha", &q);
        assert!(
            line.len() <= MAX_LINE_BYTES,
            "capped line {} must be <= MAX_LINE_BYTES {}",
            line.len(),
            MAX_LINE_BYTES
        );
        let parsed = parse_question_line(&line).expect("capped line still round-trips");
        assert_eq!(parsed.id, "q1");
        assert!(!parsed.text.trim().is_empty(), "truncated text survived");
        assert_eq!(parsed.status, "open");
    }

    #[test]
    fn chat_delta_is_a_separate_live_tail_finalized_by_push_chat() {
        let mut a = ConsoleActivity::empty();
        push_chat_delta(&mut a, 1, "He");
        push_chat_delta(&mut a, 1, "Hello");
        // The streaming tail holds the cumulative reply; NO timeline entry yet.
        assert!(a.entries.as_ref().map(|e| e.is_empty()).unwrap_or(true));
        assert_eq!(
            a.streaming_chat,
            Some(StreamingChat { seq: 1, text: "Hello".to_string() })
        );
        assert_eq!(a.running, Some(true));
        // the final turn lands ONE real entry and clears the tail (no duplicate, no leftover)
        push_chat(&mut a, "assistant", "Hello there", "10:00:01", None);
        assert_eq!(a.streaming_chat, None, "tail cleared on finalize");
        let entries = a.entries.as_ref().unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ConsoleEntry::Chat { role, text, .. } => {
                assert_eq!(role, "assistant");
                assert_eq!(text, "Hello there");
            }
            _ => panic!("expected a Chat entry"),
        }
    }

    #[test]
    fn chat_delta_new_seq_replaces_the_tail_no_orphan() {
        let mut a = ConsoleActivity::empty();
        push_chat_delta(&mut a, 1, "first");
        // a new turn's delta replaces the tail outright — the old partial can't orphan,
        // even if the previous turn was never finalized (e.g. interrupted).
        push_chat_delta(&mut a, 2, "second");
        assert_eq!(
            a.streaming_chat,
            Some(StreamingChat { seq: 2, text: "second".to_string() })
        );
        assert!(a.entries.as_ref().map(|e| e.is_empty()).unwrap_or(true));
    }

    #[test]
    fn streaming_chat_serializes_camelcase_and_omits_when_none() {
        let mut a = ConsoleActivity::empty();
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("streamingChat").is_none(), "absent tail is omitted");
        push_chat_delta(&mut a, 4, "partial");
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["streamingChat"]["seq"], 4);
        assert_eq!(v["streamingChat"]["text"], "partial");
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

    // ── push_coder_note ───────────────────────────────────────────────────────

    #[test]
    fn push_coder_note_appends_row_does_not_touch_running_or_run_count() {
        // Case A: activity in the resting empty state (None/None) — note must NOT flip them.
        let mut a = ConsoleActivity::empty();
        push_coder_note(
            &mut a,
            "Network access denied (Unattended mode)",
            Some(NodeStyle::Terra),
            "14:55:01",
        );
        assert_eq!(a.running, None, "push_coder_note must leave running=None when it was None");
        assert_eq!(a.run_count, None, "push_coder_note must leave run_count=None when it was None");
        // The entry itself must have the correct wire shape.
        let entries = a.entries.as_ref().expect("entries vec must be present");
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            ConsoleEntry::Coder { node, text, time } => {
                assert!(
                    matches!(node, Some(NodeStyle::Terra)),
                    "node must be Terra"
                );
                assert_eq!(text, "Network access denied (Unattended mode)");
                assert_eq!(time, "14:55:01");
            }
            other => panic!("expected ConsoleEntry::Coder, got {other:?}"),
        }

        // Case B: activity already marked stopped (running=Some(false)) — note must not
        // re-enable the spinner.
        let mut b = ConsoleActivity::empty();
        push_coder_milestone(&mut b, "working", Some(NodeStyle::Dot), "00:00");
        mark_coder_stopped(&mut b);
        assert_eq!(b.running, Some(false));
        push_coder_note(&mut b, "denial note after stop", Some(NodeStyle::Terra), "00:01");
        assert_eq!(
            b.running,
            Some(false),
            "push_coder_note must not re-enable the spinner on a stopped agent"
        );
        assert_eq!(
            b.run_count,
            Some(1),
            "push_coder_note must not alter run_count"
        );
        let entries_b = b.entries.as_ref().unwrap();
        assert_eq!(entries_b.len(), 2, "note appended after the milestone");

        // Case C: FIFO cap is still respected.
        let mut c = ConsoleActivity::empty();
        for i in 0..(MAX_ENTRIES_PER_AGENT + 5) {
            push_coder_note(&mut c, &format!("n{i}"), None, "00:00");
        }
        assert_eq!(
            c.entries.as_ref().unwrap().len(),
            MAX_ENTRIES_PER_AGENT,
            "push_coder_note respects MAX_ENTRIES_PER_AGENT FIFO cap"
        );
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

    // ---- D-resume: transcript tail → relaunched orchestrator's first-turn context ------

    #[test]
    fn format_chat_resume_block_is_none_for_empty_history() {
        assert!(format_chat_resume_block(&[], 12, 2000).is_none());
    }

    #[test]
    fn format_chat_resume_block_keeps_order_roles_and_frames_the_resume() {
        let turns = vec![
            ChatTurn { role: "user".into(), text: "build OAuth".into(), msg_id: None },
            ChatTurn { role: "assistant".into(), text: "drafting a plan".into(), msg_id: None },
        ];
        let block = format_chat_resume_block(&turns, 12, 2000).expect("non-empty");
        let u = block.find("user: build OAuth").expect("user turn present");
        let a = block.find("assistant: drafting a plan").expect("assistant turn present");
        assert!(u < a, "chronological order preserved");
        assert!(block.starts_with("<conversation-so-far>"));
        assert!(block.contains("</conversation-so-far>"));
        assert!(
            block.to_lowercase().contains("resuming"),
            "the block explains WHY the history is there (relaunch/backend switch)"
        );
    }

    #[test]
    fn format_chat_resume_block_takes_from_the_end_within_turn_and_token_budgets() {
        // Turn cap: only the LAST max_turns survive, still in order.
        let many: Vec<ChatTurn> = (0..20)
            .map(|i| ChatTurn {
                role: "user".into(),
                text: format!("turn-{i}"),
                msg_id: None,
            })
            .collect();
        let block = format_chat_resume_block(&many, 3, 2000).expect("non-empty");
        assert!(!block.contains("turn-16 "), "older than the last 3 dropped");
        assert!(block.contains("turn-17") && block.contains("turn-19"));
        assert!(
            block.find("turn-17").unwrap() < block.find("turn-19").unwrap(),
            "kept turns stay chronological"
        );
        // Token budget (compact.rs estimate: ~4 chars/token): turns are taken from
        // the END until the budget is exhausted — the newest always survives.
        let big: Vec<ChatTurn> = (0..5)
            .map(|i| ChatTurn {
                role: "assistant".into(),
                text: format!("{}-{i}", "x".repeat(100)),
                msg_id: None,
            })
            .collect();
        // Each line ≈ (11 + 100 + 2)/4 ≈ 28 tokens; budget 60 fits exactly the last two.
        let capped = format_chat_resume_block(&big, 12, 60).expect("non-empty");
        assert!(capped.contains("-4"), "the newest turn always survives");
        assert!(!capped.contains("-0"), "the oldest is dropped once over budget");
    }

    // ---- D2 hydration (planner-chat demolition): the bridge FILE is the one source of
    // truth; the store is a render cache rebuilt from the file's tail. ------------------

    #[test]
    fn hydrate_from_bridge_file_absent_file_is_none() {
        let dir = TestDir::new("hydrate-absent");
        assert!(hydrate_from_bridge_file(&dir.path().join("nope.jsonl"), 1024, true).is_none());
    }

    #[test]
    fn hydrate_from_bridge_file_replays_whole_lines_in_order() {
        let dir = TestDir::new("hydrate-order");
        let path = dir.path().join("a.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"ciao\"}\n",
                "{\"kind\":\"milestone\",\"text\":\"Reading files\",\"node\":\"dot\"}\n",
                "{\"kind\":\"chat\",\"role\":\"assistant\",\"text\":\"hello back\"}\n",
            ),
        )
        .unwrap();
        let (activity, end) =
            hydrate_from_bridge_file(&path, 64 * 1024, true).expect("file exists");
        let entries = activity.entries.as_deref().unwrap_or(&[]);
        assert_eq!(entries.len(), 3, "chat + milestone + chat replayed");
        assert!(
            matches!(&entries[0], ConsoleEntry::Chat { role, text, .. } if role == "user" && text == "ciao")
        );
        assert!(
            matches!(&entries[1], ConsoleEntry::Coder { text, .. } if text == "Reading files")
        );
        assert!(
            matches!(&entries[2], ConsoleEntry::Chat { role, .. } if role == "assistant")
        );
        assert_eq!(
            end,
            std::fs::metadata(&path).unwrap().len(),
            "every complete line consumed"
        );
    }

    #[test]
    fn hydrate_from_bridge_file_skips_deltas_and_the_partial_tail() {
        // A process that died mid-stream leaves chat-delta lines and possibly a partial
        // final line. Replaying a stale delta would resurrect a ghost streaming bubble
        // on a session that is not streaming; the partial line belongs to the LIVE tail
        // (it may still be completed by a writer) so hydration must not consume it.
        use std::io::Write;
        let dir = TestDir::new("hydrate-delta");
        let path = dir.path().join("a.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            "{{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"go\"}}\n{{\"kind\":\"chat-delta\",\"seq\":1,\"text\":\"half a rep\"}}\n{{\"kind\":\"chat\",\"role\":\"assist" // dangling partial line
        )
        .unwrap();
        drop(f);
        let (activity, end) =
            hydrate_from_bridge_file(&path, 64 * 1024, false).expect("file exists");
        let entries = activity.entries.as_deref().unwrap_or(&[]);
        assert_eq!(entries.len(), 1, "only the complete chat line lands");
        assert!(activity.streaming_chat.is_none(), "no ghost streaming tail");
        let full = std::fs::metadata(&path).unwrap().len();
        assert!(end < full, "the dangling partial line is NOT consumed");
        let consumed = std::fs::read(&path).unwrap()[..end as usize].to_vec();
        assert!(
            consumed.ends_with(b"\n"),
            "hydration stops exactly after the last complete line"
        );
    }

    #[test]
    fn hydrate_from_bridge_file_running_follows_mark_running() {
        let dir = TestDir::new("hydrate-running");
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, "{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"hi\"}\n").unwrap();
        let (live, _) = hydrate_from_bridge_file(&path, 1024, true).unwrap();
        assert_eq!(live.running, Some(true), "launch-time hydration marks live");
        let (dead, _) = hydrate_from_bridge_file(&path, 1024, false).unwrap();
        assert_eq!(
            dead.running,
            Some(false),
            "snapshot-on-miss hydration must NOT resurrect a spinner for a dead session"
        );
    }

    #[test]
    fn hydrate_from_bridge_file_empty_file_yields_blank_console() {
        let dir = TestDir::new("hydrate-empty");
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, "").unwrap();
        let (activity, end) = hydrate_from_bridge_file(&path, 1024, true).unwrap();
        assert!(activity.entries.as_deref().unwrap_or(&[]).is_empty());
        assert!(
            activity.running.is_none(),
            "an empty file must not set running at all (fresh-launch reset)"
        );
        assert_eq!(end, 0);
    }

    #[test]
    fn hydrate_from_bridge_file_window_starts_at_a_line_boundary() {
        // A window smaller than the file must start replaying at the FIRST line boundary
        // inside the window — never mid-line (a mid-line start would assemble garbage).
        let dir = TestDir::new("hydrate-window");
        let path = dir.path().join("a.jsonl");
        let old = "{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"OLD OLD OLD OLD\"}\n";
        let recent = "{\"kind\":\"chat\",\"role\":\"assistant\",\"text\":\"recent\"}\n";
        std::fs::write(&path, format!("{old}{recent}")).unwrap();
        // Window covers `recent` plus the TAIL of `old` (cuts `old` mid-line).
        let window = (recent.len() + 10) as u64;
        let (activity, end) = hydrate_from_bridge_file(&path, window, false).unwrap();
        let entries = activity.entries.as_deref().unwrap_or(&[]);
        assert_eq!(entries.len(), 1, "the cut line is skipped, not garbled");
        assert!(
            matches!(&entries[0], ConsoleEntry::Chat { text, .. } if text == "recent")
        );
        assert_eq!(end, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn hydrate_from_bridge_file_window_without_newline_skips_to_len() {
        // Pathological: the whole window is the tail of ONE giant unterminated line.
        // Hydration must consume nothing and hand the live tail an offset at EOF (the
        // garbled fragment is unrecoverable; a later completion parses as malformed
        // and is skipped — same class as the MAX_LINE_BYTES guard).
        use std::io::Write;
        let dir = TestDir::new("hydrate-giant");
        let path = dir.path().join("a.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{{\"kind\":\"chat\",\"role\":\"user\",\"text\":\"{}", "x".repeat(4000)).unwrap();
        drop(f);
        let (activity, end) = hydrate_from_bridge_file(&path, 1024, false).unwrap();
        assert!(activity.entries.as_deref().unwrap_or(&[]).is_empty());
        assert_eq!(end, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn is_console_blank_distinguishes_content_from_resting_state() {
        assert!(is_console_blank(&ConsoleActivity::empty()));
        let mut with_chat = ConsoleActivity::empty();
        push_chat(&mut with_chat, "user", "hi", "10:00:00", None);
        assert!(!is_console_blank(&with_chat));
        let mut with_stream = ConsoleActivity::empty();
        push_chat_delta(&mut with_stream, 1, "partial");
        assert!(!is_console_blank(&with_stream));
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
            "{{\"kind\":\"milestone\",\"text\":\"old complete\",\"node\":\"dot\"}}\n{{\"kind\":\"milestone\",\"text\":\"old PARTIAL fr" // no closing / newline
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
                ConsoleEntry::WebSearch { query, .. } => query.as_str(),
                ConsoleEntry::Chat { text, .. } => text.as_str(),
                ConsoleEntry::Question { text, .. } => text.as_str(),
                ConsoleEntry::Banner { text, .. } => text.as_str(),
                ConsoleEntry::Thinking { text, .. } => text.as_str(),
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
        let (flag, gen0, had_pred0) = reg.register("orch-1");
        assert!(!flag.load(Ordering::SeqCst), "fresh flag starts un-stopped");
        assert_eq!(gen0, 0, "first registration is generation 0");
        assert!(!had_pred0, "a first registration reports no predecessor (no grace sleep)");

        // Re-registering the same id flips the PREDECESSOR's flag (clean relaunch) and bumps
        // the generation.
        let (flag2, gen1, had_pred1) = reg.register("orch-1");
        assert!(flag.load(Ordering::SeqCst), "predecessor task is told to stop");
        assert!(!flag2.load(Ordering::SeqCst));
        assert_eq!(gen1, 1, "relaunch bumps the generation");
        assert!(had_pred1, "a same-id relaunch reports the replaced predecessor");

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
        let (_flag_old, gen_old, _) = reg.register("orch-1");
        // Relaunch: a fresh tail registers under a NEW generation (predecessor flag flipped).
        let (_flag_new, gen_new, _) = reg.register("orch-1");
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
        let (_flag, gen, _) = reg.register("orch-1");
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
        let (flag, _gen, _) = reg.register("orch-1");
        reg.stop("orch-1");
        assert!(flag.load(Ordering::SeqCst), "the predecessor flag is set by stop()");
        // The entry is removed: a fresh register() sees NO predecessor → generation resets to 0.
        let (_flag2, gen2, had_pred2) = reg.register("orch-1");
        assert_eq!(gen2, 0, "after stop() removed the entry, the next register is generation 0");
        assert!(
            !had_pred2,
            "a cleanly-stopped entry is gone — the re-register reports no predecessor"
        );
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
        let (flag_a, _, _) = reg.register("orch-a");
        let (flag_b, _, _) = reg.register("orch-b");
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

    #[test]
    fn task_cost_estimate_usd_serializes_camel_case() {
        let activity = ConsoleActivity {
            task_cost_estimate_usd: Some(0.03),
            ..Default::default()
        };
        let json = serde_json::to_string(&activity).unwrap();
        assert!(
            json.contains("\"taskCostEstimateUsd\""),
            "expected camelCase key in: {json}"
        );
        assert!(!json.contains("task_cost_estimate_usd"));
    }

    #[test]
    fn task_cost_estimate_usd_omitted_when_none() {
        let activity = ConsoleActivity::default();
        let json = serde_json::to_string(&activity).unwrap();
        assert!(!json.contains("taskCostEstimateUsd"));
    }
}
