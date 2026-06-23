//! The bounded inner tool-burst loop (L2.2).
//!
//! Devboule is conversational-first. The OUTER loop (L2.1, in `main.rs`) is the
//! UNBOUNDED human conversation. This module adds the BOUNDED INNER burst that
//! runs between human turns: per human message the model emits actions, the loop
//! executes them and feeds the results back, until the model emits a TERMINAL
//! action (`done` / `ask_user` / `escalate`) or a STOP CONDITION fires — then
//! control returns to the human.
//!
//! Stop conditions (Goose-style loop bounds, reimplemented):
//! * three consecutive format errors,
//! * a repeated `(tool, target)` (no progress),
//! * the round cap ([`MAX_ROUNDS`] executed tool actions),
//! * the wall-clock deadline (via an INJECTED [`Clock`] so it is testable).
//!
//! Seams kept minimal so L2.2 is fully exercisable with NO model and NO MCP:
//! * [`CoderModel::next_output`] yields the next raw output ([`ScriptedModel`]).
//! * [`ToolExecutor`] dispatches a validated action to a [`ToolResult`]
//!   ([`StubExecutor`] returns canned results; L2.3 supplies the MCP-backed one).
//!
//! [`CoderModel::next_output`]: crate::model::CoderModel::next_output
//! [`ScriptedModel`]: crate::model::ScriptedModel

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::action::{parse_action_with_servers, AgentAction, FormatError};
use crate::model::CoderModel;

/// Max number of EXECUTED tool actions in one burst before the loop gives up.
/// Owner-tunable in 12–16; format errors do NOT count toward it (a format error
/// is not progress). Default 14.
pub const MAX_ROUNDS: usize = 14;

/// Consecutive format errors that abort the burst. A model that cannot emit a
/// valid action three times running will not on the fourth; bail instead of
/// burning the conversation.
pub const MAX_FORMAT_ERRORS: usize = 3;

/// Default wall-clock budget for one burst. The deadline is enforced via the
/// injected [`Clock`] so the cap is testable without real time.
pub const DEFAULT_BURST_BUDGET: Duration = Duration::from_secs(120);

/// Size of the no-progress window: the last N EXECUTED `(tool, target)` pairs are
/// remembered so the guard catches not just an immediate repeat but a short
/// A→B→A→B oscillation. Small on purpose — the goal is to break a tight cycle,
/// not to forbid legitimately revisiting a file later in a long burst.
pub const NO_PROGRESS_WINDOW: usize = 4;

/// Size of the INVALID-output hash window (Phase 11.4 watchdog refinement). The last
/// N format-error outputs' content hashes are remembered so a model repeating the SAME
/// unparseable output — even when interleaved with valid actions that reset the
/// consecutive-format-error counter — is caught as a loop. The `(tool, target)` guard
/// only covers DISPATCHED actions, and [`MAX_FORMAT_ERRORS`] only catches CONSECUTIVE
/// failures; this closes the interleaved-repeat gap between them. A touch wider than
/// [`NO_PROGRESS_WINDOW`] so a longer invalid↔valid oscillation still trips.
pub const OUTPUT_HASH_WINDOW: usize = 6;

/// Max length (chars) of a single tool result stored in the transcript. A
/// runaway tool (a giant file read, a huge fetch) would otherwise blow up the
/// transcript that is re-fed to the model every round; we truncate at a char
/// boundary with a marker so the model still sees the head and knows it was cut.
pub const MAX_RESULT_LEN: usize = 16_384;

/// How a burst concluded. This IS the assistant turn's conclusion: the human
/// regains control afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurstOutcome {
    /// The model produced a final answer for the human.
    Done(String),
    /// The model needs the human; the question is shown and the loop hands back
    /// (the conversational hand-back).
    AskUser(String),
    /// The burst gave up — model-requested (`escalate`) or a stop condition
    /// fired. The reason is surfaced to the human.
    Escalated(String),
}

/// A tool dispatch result, fed back into the transcript for the next round.
///
/// `ok == false` marks a tool-level failure (the action was well-formed and
/// dispatched, but the tool reports an error). The loop still treats a dispatch
/// as a completed round either way — a failing tool is progress, not a format
/// error — so `ok` is informational for the transcript / TUI, not a control
/// signal here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            ok: true,
            output: output.into(),
        }
    }

    pub fn err(output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
        }
    }
}

/// Dispatches a validated [`AgentAction`] to a [`ToolResult`]. The ONLY seam the
/// real MCP-backed executor (L2.3, [`crate::executor::RealExecutor`]) needs to
/// implement; L2.2 ships [`StubExecutor`].
///
/// `#[async_trait]` (L2.3): `execute` is `async` because the real executor does
/// genuine I/O — MCP `call_tool` over a child-process transport, an Exa HTTP
/// egress call — none of which may block the tokio reactor. The trait stays
/// object-safe so the burst still holds `&dyn ToolExecutor`. `Send + Sync` keeps
/// the burst future `Send` (it holds the executor across the progress-send
/// awaits) so the TUI can `tokio::spawn` it.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, action: &AgentAction) -> ToolResult;

    /// The CONFIGURED user-MCP server names this executor can route an
    /// [`AgentAction::McpTool`] to. The burst passes these to
    /// [`crate::action::parse_action_with_servers`] so an `mcp_tool` naming an
    /// unknown server is rejected at PARSE time (immediate model feedback) rather
    /// than reaching dispatch as a late error. The default is EMPTY: an executor
    /// with no user servers (the Stub, the test mocks, the plain `RmcpBackend`
    /// path) rejects every `mcp_tool`, which is the correct no-user-servers
    /// behavior. [`crate::executor::RealExecutor`] overrides this with the names
    /// its `MultiMcpBackend` knows.
    fn known_mcp_servers(&self) -> &[String] {
        &[]
    }

    /// Drain any LIVE steer messages the app sent to this running orchestrator since
    /// the last round (the burst injects them as human turns). Default EMPTY: only
    /// [`crate::executor::RealExecutor`] with a `DEVBOULE_STEER_FILE` returns messages;
    /// every other executor (stub, mocks) never has a steer inbox.
    fn drain_steer(&self) -> Vec<String> {
        Vec::new()
    }

    /// Emit ONE conversational chat turn to the planner chat (role = "assistant" for the
    /// orchestrator's own words, "user" for an echoed steer). Default NO-OP: only
    /// [`crate::executor::RealExecutor`] with a live activity bridge surfaces it.
    fn emit_chat(&self, _role: &str, _text: &str) {}
}

/// The result every [`StubExecutor`] tool dispatch returns: an UNMISTAKABLE
/// not-connected ERROR. This is the no-server fallback (`config::build_runtime`
/// drops to the stub when the MCP backend can't be reached, in dev OR on a real
/// production misconfig). A plausible-looking success here is DANGEROUS: a live
/// model would treat fabricated "[stub: 2 snippets]" output as real and confidently
/// hallucinate a working-looking answer that is entirely fake. So every stub result
/// is an error whose text tells the model, in no uncertain terms, to stop and report
/// the backend is offline rather than invent an answer. The system prompt
/// (`crate::prompt`) carries the matching rule.
pub const STUB_NOT_CONNECTED: &str = "TOOL UNAVAILABLE: the local coder is NOT connected to its backend (oracle/spawn/project). Do NOT fabricate an answer — tell the user the local coder backend is offline and stop.";

/// MCP-free executor: every non-terminal action returns the SAME not-connected
/// ERROR ([`STUB_NOT_CONNECTED`]). Terminal actions never reach an executor (the
/// loop returns before dispatching them), so they are not represented here.
/// Retained as the no-config / `cargo run`-without-a-server default and as the
/// test stub — but it NEVER returns a plausible success, so a disconnected run
/// cannot be mistaken for a working agent.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubExecutor;

#[async_trait]
impl ToolExecutor for StubExecutor {
    async fn execute(&self, action: &AgentAction) -> ToolResult {
        match action {
            // Every tool the stub can be asked to run is unavailable: there is no
            // backend behind it. Return the same loud not-connected error for ALL
            // of them so the model cannot mistake any of them for real output.
            AgentAction::OracleAsk { .. }
            | AgentAction::OracleContext { .. }
            | AgentAction::Plan { .. }
            | AgentAction::RunPlan {}
            | AgentAction::SpawnMini { .. }
            | AgentAction::Read { .. }
            | AgentAction::Grep { .. }
            | AgentAction::Glob { .. }
            | AgentAction::LoadSkill { .. }
            | AgentAction::Fetch { .. }
            | AgentAction::Websearch { .. }
            | AgentAction::McpTool { .. } => ToolResult::err(STUB_NOT_CONNECTED),
            // Terminal actions are handled by the loop before dispatch; if one
            // ever reaches here it is a logic error, so report it rather than
            // silently succeed.
            AgentAction::AskUser { .. }
            | AgentAction::Done { .. }
            | AgentAction::Escalate { .. } => {
                ToolResult::err("[stub: terminal action must not be dispatched]")
            }
        }
    }
}

/// A monotonic elapsed-time source, injected so the wall-clock cap is testable
/// without real time. Returns the time elapsed since the burst began.
///
/// `Send + Sync` for the same reason as [`ToolExecutor`]: the burst future holds
/// `&dyn Clock` across awaits and must stay `Send` for `tokio::spawn`.
pub trait Clock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

/// Real clock: anchored at construction, reports true elapsed time.
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn start_now() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// One entry in the burst transcript.
// `Eq` is not derived: the `Action` arm holds an `AgentAction`, which dropped `Eq`
// (its `McpTool.params` is a non-`Eq` `serde_json::Value`). `PartialEq` is all the
// transcript tests need.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEntry {
    /// The model's parsed action for a round.
    Action(AgentAction),
    /// The tool result fed back for the preceding action.
    Result(ToolResult),
    /// Format-error feedback pushed after an unparseable model turn.
    FormatFeedback(String),
    /// A LIVE human message injected mid-burst (a steer from the app). Rendered to the
    /// model as a `user` turn so it can course-correct on its next call.
    Human(String),
}

/// The running record of one burst: the human message plus, in order, each
/// action / tool-result / format-feedback. Fed to the model every round so it
/// sees the full local context.
#[derive(Debug, Clone)]
pub struct Transcript {
    human: String,
    entries: Vec<TranscriptEntry>,
}

impl Transcript {
    pub fn new(human: String) -> Self {
        Self {
            human,
            entries: Vec::new(),
        }
    }

    /// The human message that opened this burst.
    pub fn human_message(&self) -> &str {
        &self.human
    }

    /// The ordered entries accumulated so far. Read by the tests today and by
    /// L2.3's real model (to render the transcript into a prompt); unwired in the
    /// L2.2 binary, so silenced there rather than deleted.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    fn push(&mut self, entry: TranscriptEntry) {
        self.entries.push(entry);
    }

    /// Inject a LIVE human message (an app steer) mid-burst. Surfaces to the model as a
    /// `user` turn on its next call.
    pub fn push_human(&mut self, body: impl Into<String>) {
        self.entries.push(TranscriptEntry::Human(body.into()));
    }

    /// Test-only entry push, so sibling modules (e.g. the model-client transcript
    /// eviction test) can build a transcript with arbitrary entries without
    /// driving a whole burst. NOT compiled into the binary.
    #[cfg(test)]
    pub(crate) fn push_entry_for_test(&mut self, entry: TranscriptEntry) {
        self.entries.push(entry);
    }
}

/// Run ONE bounded tool-burst for `human_msg`, streaming a progress line per
/// action + result over `progress_tx`, and returning the terminal [`BurstOutcome`].
///
/// `clock` is injected (see [`Clock`]); the deadline is `budget` of elapsed time.
/// The model is queried for its raw next output each round, parsed under
/// mini-swe-agent format discipline, and either fed back (format error) or
/// dispatched (valid action). See the module docs for the full stop-condition set.
///
/// `allow_egress` STRUCTURALLY gates network actions (`is_egress()`): when
/// `false`, such an action is NEVER handed to the executor — the loop pushes a
/// disabled-result the model can recover from (answer via the Oracle), counts it
/// as a round so it cannot be retried forever, and continues. L2.2's binary call
/// site passes `false` (no web provider is wired until L2.3).
///
/// Progress sends use `.await` for natural backpressure against the bounded TUI
/// channel; if the receiver is gone (the TUI quit mid-burst) sends silently fail
/// and the loop still runs to a clean terminal outcome.
pub async fn run_burst(
    human_msg: String,
    model: &dyn CoderModel,
    executor: &dyn ToolExecutor,
    clock: &dyn Clock,
    budget: Duration,
    allow_egress: bool,
    progress_tx: &mpsc::Sender<String>,
) -> BurstOutcome {
    let mut transcript = Transcript::new(human_msg);
    let mut rounds: usize = 0;
    let mut consecutive_format_errors: usize = 0;
    // A sliding window of the last EXECUTED (tool_name, target) pairs, for the
    // no-progress guard. Catches not just an immediate repeat but a short
    // A→B→A→B oscillation. Only a dispatched action pushes here; format errors,
    // terminal actions, and egress-blocked actions do not.
    let mut executed_window: VecDeque<(String, String)> =
        VecDeque::with_capacity(NO_PROGRESS_WINDOW);
    // A sliding window of the content hashes of recent INVALID (format-error) outputs,
    // for the 11.4 loop-detector. Only format-error outputs push here; a valid action
    // is governed by the (tool, target) guard, not this one.
    let mut invalid_output_hashes: VecDeque<u64> = VecDeque::with_capacity(OUTPUT_HASH_WINDOW);
    // Time spent inside `run_plan` dispatches, EXCLUDED from the wall-clock budget. The
    // 11.3 runner executes a human-APPROVED plan deterministically — many minis, each
    // legitimately minutes long — as ONE `executor.execute` call. That is real work, not
    // the model "wandering", and it is already bounded independently (task count ×
    // attempts, each mini by its own server poll). So the burst budget governs the
    // MODEL's exploration only; we subtract run_plan's duration so a long, successful run
    // does not get a spurious "time cap reached" the instant it returns.
    let mut excluded_elapsed = Duration::ZERO;

    loop {
        // Wall-clock cap: check BEFORE asking the model so an already-overrun
        // burst stops promptly rather than doing one more (possibly slow) round.
        // `run_plan` execution time is excluded (see `excluded_elapsed`).
        if clock.elapsed().saturating_sub(excluded_elapsed) >= budget {
            return BurstOutcome::Escalated("time cap reached".to_string());
        }

        // Live steer: drain any messages the app sent to this RUNNING orchestrator and
        // inject them as human turns so the model sees them on THIS round (mid-plan
        // course-correction). Empty for every non-steered burst (no DEVBOULE_STEER_FILE).
        for msg in executor.drain_steer() {
            emit(progress_tx, format!("💬 steer: {}", elide(&msg))).await;
            executor.emit_chat("user", &msg);
            transcript.push_human(msg);
        }

        let raw = model.next_output(&transcript).await;

        // Validate against the executor's configured user-MCP server names so an
        // `mcp_tool` naming an unknown server is an immediate format error (the
        // default empty set rejects every `mcp_tool` when no user servers exist).
        match parse_action_with_servers(&raw, executor.known_mcp_servers()) {
            Err(fe) => {
                // 11.4 output-hash loop-detector: a model re-emitting the SAME invalid
                // output — even interleaved with valid actions that reset the
                // consecutive-error counter below — is stuck in a way neither the
                // consecutive-format-error guard nor the (tool, target) guard catches.
                // Hash this invalid output; a repeat within the window escalates.
                let h = hash_output(&raw);
                if invalid_output_hashes.contains(&h) {
                    let reason = "no progress: repeated invalid model output".to_string();
                    emit(progress_tx, format!("⚠ {reason}")).await;
                    return BurstOutcome::Escalated(reason);
                }
                invalid_output_hashes.push_back(h);
                if invalid_output_hashes.len() > OUTPUT_HASH_WINDOW {
                    invalid_output_hashes.pop_front();
                }

                // A format error is NOT progress: feed precise guidance back and
                // do NOT consume the round budget. Three in a row aborts.
                let feedback = fe.feedback();
                emit(
                    progress_tx,
                    format!("⚠ format error: {}", short_reason(&fe)),
                )
                .await;
                transcript.push(TranscriptEntry::FormatFeedback(feedback));
                consecutive_format_errors += 1;
                if consecutive_format_errors >= MAX_FORMAT_ERRORS {
                    return BurstOutcome::Escalated(format!(
                        "{MAX_FORMAT_ERRORS} consecutive format errors"
                    ));
                }
            }
            Ok(action) => {
                consecutive_format_errors = 0;

                // Terminal actions end the burst immediately (never dispatched).
                match &action {
                    AgentAction::Done { reply } => {
                        emit(progress_tx, format!("✓ done: {}", elide(reply))).await;
                        // The orchestrator's closing words → an assistant chat bubble.
                        executor.emit_chat("assistant", reply);
                        return BurstOutcome::Done(reply.clone());
                    }
                    AgentAction::AskUser { question } => {
                        emit(progress_tx, format!("❓ {}", elide(question))).await;
                        // The orchestrator's question → an assistant chat bubble (the user
                        // answers it via the chat composer → steer → continues).
                        executor.emit_chat("assistant", question);
                        return BurstOutcome::AskUser(question.clone());
                    }
                    AgentAction::Escalate { reason } => {
                        emit(progress_tx, format!("⚠ escalated: {}", elide(reason))).await;
                        return BurstOutcome::Escalated(reason.clone());
                    }
                    _ => {}
                }

                // No-progress guard: an identical (tool, target) anywhere in the
                // recent-executed window means the model is spinning — catches an
                // immediate repeat AND a short A→B→A→B oscillation. Check BEFORE
                // dispatch so we never re-run the same side-effect.
                let this = (action.tool_name().to_string(), action.target());
                if executed_window.contains(&this) {
                    let reason = format!("no progress: cycling on {} {}", this.0, elide(&this.1));
                    emit(progress_tx, format!("⚠ {reason}")).await;
                    return BurstOutcome::Escalated(reason);
                }

                // Egress gate (W7): a network action is dispatched ONLY when egress
                // is allowed. When disabled, do NOT call the executor — push a
                // disabled-result the model can recover from (answer via the
                // Oracle), count it as a round so it can't be retried forever, and
                // continue. The (tool, target) IS recorded in the no-progress window
                // (like a dispatched action): otherwise a model repeating the same
                // blocked `fetch(url)` would never trip the no-progress guard and
                // would burn every remaining round on an action that can never run.
                if action.is_egress() && !allow_egress {
                    emit(
                        progress_tx,
                        format!("⨯ {} [egress disabled]", action.tool_name()),
                    )
                    .await;
                    let result = ToolResult::err(
                        "egress disabled (no web provider configured / opt-in off); \
                         answer from the Oracle instead",
                    );
                    transcript.push(TranscriptEntry::Action(action));
                    transcript.push(TranscriptEntry::Result(cap_result(result)));
                    executed_window.push_back(this);
                    if executed_window.len() > NO_PROGRESS_WINDOW {
                        executed_window.pop_front();
                    }
                    rounds += 1;
                    if rounds >= MAX_ROUNDS {
                        return BurstOutcome::Escalated("round cap reached".to_string());
                    }
                    continue;
                }

                // Dispatch and feed the result back. Egress actions are tagged so
                // the human sees, in the live stream, when a turn leaves the
                // machine (oracle_* is private/grounded and never tagged).
                let egress = if action.is_egress() { " [egress]" } else { "" };
                emit(
                    progress_tx,
                    format!(
                        "→ {}{egress}: {}",
                        action.tool_name(),
                        elide(&action.target())
                    ),
                )
                .await;
                // Measure `run_plan`'s deterministic execution time so it is EXCLUDED
                // from the wall-clock budget (human-approved real work, not the model
                // wandering — see `excluded_elapsed`). Every other tool counts normally.
                let is_run_plan = matches!(action, AgentAction::RunPlan {});
                let dispatch_start = if is_run_plan {
                    Some(clock.elapsed())
                } else {
                    None
                };
                let result = cap_result(executor.execute(&action).await);
                if let Some(start) = dispatch_start {
                    excluded_elapsed =
                        excluded_elapsed.saturating_add(clock.elapsed().saturating_sub(start));
                }
                emit(progress_tx, format!("   {}", elide(&result.output))).await;

                transcript.push(TranscriptEntry::Action(action));
                transcript.push(TranscriptEntry::Result(result));

                // Record this executed pair in the sliding window, evicting the
                // oldest when over capacity.
                executed_window.push_back(this);
                if executed_window.len() > NO_PROGRESS_WINDOW {
                    executed_window.pop_front();
                }
                rounds += 1;

                if rounds >= MAX_ROUNDS {
                    return BurstOutcome::Escalated("round cap reached".to_string());
                }
            }
        }
    }
}

/// Best-effort progress send: backpressures against the bounded TUI channel and
/// silently no-ops if the receiver is gone (TUI quit mid-burst). A trailing
/// newline is appended so each progress line renders on its own line in the
/// assistant message (the body is concatenated chunk-by-chunk by the TUI).
async fn emit(tx: &mpsc::Sender<String>, line: String) {
    let _ = tx.send(format!("{line}\n")).await;
}

/// One-line summary of a format error for a progress line (the full feedback
/// goes into the transcript, not the TUI tail).
fn short_reason(fe: &FormatError) -> String {
    match fe {
        FormatError::Missing => "no action block".to_string(),
        FormatError::TooMany(n) => format!("{n} action blocks"),
        FormatError::Invalid(msg) => elide(msg),
    }
}

/// Cap a tool result before it enters the transcript (W6). If `output` exceeds
/// [`MAX_RESULT_LEN`] chars, keep the first `MAX_RESULT_LEN` CHARS (never split a
/// UTF-8 codepoint — we iterate `chars`, not bytes) and append a marker naming
/// how many BYTES were dropped, so the model sees the head and knows it was cut.
fn cap_result(mut result: ToolResult) -> ToolResult {
    if result.output.chars().count() <= MAX_RESULT_LEN {
        return result;
    }
    let kept: String = result.output.chars().take(MAX_RESULT_LEN).collect();
    let dropped_bytes = result.output.len() - kept.len();
    result.output = format!("{kept}\n[…truncated {dropped_bytes} bytes]");
    result
}

/// Truncate a string to a sensible one-line length for progress / reason text.
fn elide(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace(['\n', '\r'], " ");
    let trimmed = one_line.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(MAX).collect();
    format!("{cut}…")
}

/// Stable content hash of a model output, for the 11.4 loop-detector. Uses the std
/// `DefaultHasher` (SipHash) — no new dependency; we only need equality of identical
/// strings within ONE process run (not a cryptographic or cross-run-stable digest), so
/// the default hasher's per-process seed is fine.
fn hash_output(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ScriptedModel;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A clock that reports a fixed elapsed time (test-controlled).
    struct FixedClock(Duration);
    impl Clock for FixedClock {
        fn elapsed(&self) -> Duration {
            self.0
        }
    }

    /// A clock that returns `before` for the first `trip_after` calls, then
    /// `after`. Lets a test let N rounds run, then push the burst past the
    /// deadline. The counter is an atomic so the clock is `Sync` (the `Clock`
    /// supertrait bound), even though `run_burst` only ever touches it serially.
    struct ArmedClock {
        before: Duration,
        after: Duration,
        trip_after: usize,
        calls: AtomicUsize,
    }
    impl Clock for ArmedClock {
        fn elapsed(&self) -> Duration {
            let n = self.calls.fetch_add(1, Ordering::Relaxed);
            if n >= self.trip_after {
                self.after
            } else {
                self.before
            }
        }
    }

    fn action_block(json: serde_json::Value) -> String {
        format!("```action\n{json}\n```")
    }

    /// Drain whatever progress lines were buffered (non-blocking).
    fn drain(rx: &mut mpsc::Receiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(line) = rx.try_recv() {
            out.push(line);
        }
        out
    }

    #[tokio::test]
    async fn runs_tools_in_order_then_returns_done() {
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"oracle_ask","query":"where"})),
            action_block(serde_json::json!({"tool":"read","path":"src/main.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"found it"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "do the thing".to_string(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;

        assert_eq!(outcome, BurstOutcome::Done("found it".to_string()));

        // The progress stream shows both tool actions in order before done.
        let lines = drain(&mut rx);
        let joined = lines.join("\n");
        let oracle_at = joined.find("oracle_ask").expect("oracle_ask streamed");
        let read_at = joined.find("read:").expect("read streamed");
        let done_at = joined.find("done:").expect("done streamed");
        assert!(oracle_at < read_at, "oracle_ask must precede read");
        assert!(read_at < done_at, "read must precede done");

        // The stub now returns the SAME loud not-connected error for every tool
        // (oracle_ask + read here), so the stream carries the not-connected wording
        // instead of plausible-looking fake output. Truncated for the progress line,
        // so match the unmistakable head ("TOOL UNAVAILABLE").
        assert!(
            joined.matches("TOOL UNAVAILABLE").count() >= 2,
            "both stub tool results signalled not-connected"
        );
    }

    #[tokio::test]
    async fn transcript_accumulates_both_tool_results() {
        // Drive a recording executor so we can prove BOTH tools dispatched in
        // order and the terminal action was NOT dispatched.
        use std::sync::Mutex;
        struct Recorder(Mutex<Vec<String>>);
        #[async_trait]
        impl ToolExecutor for Recorder {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok(format!("ran {}", action.tool_name()))
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"oracle_ask","query":"where"})),
            action_block(serde_json::json!({"tool":"read","path":"src/main.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"ok"})),
        ]);
        let exec = Recorder(Mutex::new(Vec::new()));
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "go".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("ok".into()));
        assert_eq!(
            *exec.0.lock().unwrap(),
            vec!["oracle_ask".to_string(), "read".to_string()],
            "both tools dispatched in order, terminal not dispatched"
        );
    }

    #[test]
    fn transcript_records_action_result_pairs_in_order() {
        // Exercises the Transcript accessor surface directly: two action+result
        // pairs interleaved, plus a format-feedback entry, in push order.
        let mut t = Transcript::new("the human ask".into());
        assert_eq!(t.human_message(), "the human ask");
        assert!(t.entries().is_empty());

        t.push(TranscriptEntry::Action(AgentAction::Read {
            path: "a.rs".into(),
        }));
        t.push(TranscriptEntry::Result(ToolResult::ok("contents of a")));
        t.push(TranscriptEntry::FormatFeedback("emit one block".into()));
        t.push(TranscriptEntry::Action(AgentAction::Read {
            path: "b.rs".into(),
        }));
        t.push(TranscriptEntry::Result(ToolResult::err("missing b")));

        let entries = t.entries();
        assert_eq!(entries.len(), 5);
        assert!(matches!(
            entries[0],
            TranscriptEntry::Action(AgentAction::Read { .. })
        ));
        assert!(matches!(&entries[1], TranscriptEntry::Result(r) if r.ok));
        assert!(matches!(entries[2], TranscriptEntry::FormatFeedback(_)));
        assert!(matches!(&entries[4], TranscriptEntry::Result(r) if !r.ok));
    }

    #[tokio::test]
    async fn never_emitting_done_hits_round_cap() {
        // A script that keeps reading DISTINCT files forever -> the round cap, not
        // the no-progress guard, is what stops it. Build MAX_ROUNDS+5 distinct reads.
        let outputs: Vec<String> = (0..(MAX_ROUNDS + 5))
            .map(|i| action_block(serde_json::json!({"tool":"read","path":format!("f{i}.rs")})))
            .collect();
        let model = ScriptedModel::new(outputs);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(256);

        let outcome = run_burst(
            "loop".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated("round cap reached".to_string())
        );
    }

    #[tokio::test]
    async fn three_format_errors_escalate_without_consuming_rounds() {
        // Three malformed outputs in a row -> escalate. They must NOT count toward
        // the round cap; we prove that by noting MAX_FORMAT_ERRORS < MAX_ROUNDS and
        // the message names the format-error condition, not the round cap.
        let model = ScriptedModel::new(vec![
            "no action here".to_string(),
            "still nothing".to_string(),
            "nope".to_string(),
            // Would be a valid done if reached, but we must escalate before it.
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated(format!("{MAX_FORMAT_ERRORS} consecutive format errors"))
        );
    }

    #[tokio::test]
    async fn malformed_then_valid_recovers() {
        // One malformed output, then a valid done: the loop feeds the error back
        // and proceeds to the done reply. The format-error counter must reset.
        let model = ScriptedModel::new(vec![
            "garbage, no block".to_string(),
            action_block(serde_json::json!({"tool":"done","reply":"recovered"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("recovered".to_string()));
        let joined = drain(&mut rx).join("\n");
        assert!(joined.contains("format error"), "the error was surfaced");
        assert!(joined.contains("done"), "then the burst completed");
    }

    #[tokio::test]
    async fn repeated_invalid_output_escalates_even_when_interleaved() {
        // 11.4 loop-detector: identical INVALID outputs interleaved with a valid action
        // (which resets the consecutive-format-error counter) slip past BOTH the
        // 3-consecutive guard and the (tool,target) guard. The output-hash window catches
        // the repeat: garbage_A, read foo.rs (valid), garbage_A -> escalate on the 2nd A.
        let model = ScriptedModel::new(vec![
            "stuck thought, no action block".to_string(),
            action_block(serde_json::json!({"tool":"read","path":"foo.rs"})),
            "stuck thought, no action block".to_string(), // byte-identical invalid output
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        match outcome {
            BurstOutcome::Escalated(reason) => {
                assert!(
                    reason.contains("repeated invalid"),
                    "reason names the repeated-invalid-output loop: {reason}"
                );
            }
            other => panic!("expected repeated-invalid-output escalation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn distinct_invalid_outputs_are_not_falsely_flagged() {
        // The detector must NOT trip on DISTINCT invalid outputs: three different garbage
        // strings escalate via the existing 3-consecutive rule, not the hash detector.
        let model = ScriptedModel::new(vec![
            "garbage one".to_string(),
            "garbage two".to_string(),
            "garbage three".to_string(),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated(format!("{MAX_FORMAT_ERRORS} consecutive format errors")),
            "distinct invalid outputs escalate via the consecutive guard, not the hash detector"
        );
    }

    #[tokio::test]
    async fn repeated_tool_target_is_no_progress() {
        // The SAME (read, foo.rs) twice -> no-progress escalation on the second.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"read","path":"foo.rs"})),
            action_block(serde_json::json!({"tool":"read","path":"foo.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        match outcome {
            BurstOutcome::Escalated(reason) => {
                assert!(reason.starts_with("no progress"), "reason: {reason}");
                assert!(reason.contains("read"), "reason names the tool: {reason}");
            }
            other => panic!("expected no-progress escalation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn same_tool_different_target_is_progress() {
        // read foo.rs then read bar.rs is NOT a repeat -> proceeds to done.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"read","path":"foo.rs"})),
            action_block(serde_json::json!({"tool":"read","path":"bar.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"ok"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("ok".to_string()));
    }

    #[tokio::test]
    async fn oscillating_tool_targets_escalate_within_window() {
        // W3: read A, read B, read A again -> the second read A is in the recent
        // window even though it is not the IMMEDIATELY preceding action, so the
        // A→B→A oscillation is caught instead of looping forever.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"read","path":"a.rs"})),
            action_block(serde_json::json!({"tool":"read","path":"b.rs"})),
            action_block(serde_json::json!({"tool":"read","path":"a.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        match outcome {
            BurstOutcome::Escalated(reason) => {
                assert!(reason.starts_with("no progress"), "reason: {reason}");
                assert!(reason.contains("cycling"), "names the cycle: {reason}");
                assert!(reason.contains("a.rs"), "names the cycled target: {reason}");
            }
            other => panic!("expected oscillation escalation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn distinct_progress_beyond_window_reaches_round_cap() {
        // W3: a genuinely advancing sequence (all-distinct targets) must NOT be
        // tripped by the window — it runs until the round cap, like before.
        let outputs: Vec<String> = (0..(MAX_ROUNDS + 5))
            .map(|i| action_block(serde_json::json!({"tool":"read","path":format!("f{i}.rs")})))
            .collect();
        let model = ScriptedModel::new(outputs);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(256);

        let outcome = run_burst(
            "loop".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated("round cap reached".to_string())
        );
    }

    #[tokio::test]
    async fn oversized_tool_result_is_truncated_in_transcript() {
        // W6: a tool returning a 50 KB output is stored truncated with the marker;
        // we inspect the transcript via a probe executor + a model that reads the
        // accumulated entries (here we drive one round then a done, and assert on
        // the stored Result entry through a recording wrapper).
        use std::sync::Mutex;
        struct BigExec;
        #[async_trait]
        impl ToolExecutor for BigExec {
            async fn execute(&self, _action: &AgentAction) -> ToolResult {
                ToolResult::ok("x".repeat(50_000))
            }
        }
        // A model that, after the read, captures the transcript's last Result and
        // then emits done. We capture via a shared slot the model writes into.
        struct CapturingModel {
            captured: Mutex<Option<String>>,
        }
        #[async_trait]
        impl CoderModel for CapturingModel {
            async fn reply(&self, _prompt: String, _tx: mpsc::Sender<String>) {}
            async fn next_output(&self, transcript: &Transcript) -> String {
                // First round: no result yet -> emit a read. Second round: capture
                // the stored result, then emit done.
                let last_result = transcript.entries().iter().rev().find_map(|e| match e {
                    TranscriptEntry::Result(r) => Some(r.output.clone()),
                    _ => None,
                });
                if let Some(out) = last_result {
                    *self.captured.lock().unwrap() = Some(out);
                    action_block(serde_json::json!({"tool":"done","reply":"done"}))
                } else {
                    action_block(serde_json::json!({"tool":"read","path":"big.rs"}))
                }
            }
        }
        let model = CapturingModel {
            captured: Mutex::new(None),
        };
        let exec = BigExec;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("done".to_string()));

        let stored = model
            .captured
            .lock()
            .unwrap()
            .clone()
            .expect("a result was stored");
        assert!(
            stored.chars().count() <= MAX_RESULT_LEN + 64,
            "stored output is truncated near the cap, got {} chars",
            stored.chars().count()
        );
        assert!(stored.contains("[…truncated"), "truncation marker present");
        assert!(stored.contains("bytes]"), "truncation marker names bytes");
    }

    #[tokio::test]
    async fn small_tool_result_is_not_truncated() {
        // W6: a small output passes through untouched (no marker).
        struct SmallExec;
        #[async_trait]
        impl ToolExecutor for SmallExec {
            async fn execute(&self, _action: &AgentAction) -> ToolResult {
                ToolResult::ok("just a little output")
            }
        }
        use std::sync::Mutex;
        struct CapturingModel(Mutex<Option<String>>);
        #[async_trait]
        impl CoderModel for CapturingModel {
            async fn reply(&self, _prompt: String, _tx: mpsc::Sender<String>) {}
            async fn next_output(&self, transcript: &Transcript) -> String {
                let last = transcript.entries().iter().rev().find_map(|e| match e {
                    TranscriptEntry::Result(r) => Some(r.output.clone()),
                    _ => None,
                });
                if let Some(out) = last {
                    *self.0.lock().unwrap() = Some(out);
                    action_block(serde_json::json!({"tool":"done","reply":"done"}))
                } else {
                    action_block(serde_json::json!({"tool":"read","path":"a.rs"}))
                }
            }
        }
        let model = CapturingModel(Mutex::new(None));
        let exec = SmallExec;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let _ = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        let stored = model
            .0
            .lock()
            .unwrap()
            .clone()
            .expect("a result was stored");
        assert_eq!(stored, "just a little output");
        assert!(!stored.contains("truncated"), "no marker on a small result");
    }

    #[tokio::test]
    async fn egress_disabled_does_not_dispatch_and_recovers() {
        // W7: with allow_egress=false a fetch is NEVER handed to the executor; the
        // loop pushes a disabled-result, counts the round, and continues so the
        // model can recover (here it follows up with done).
        use std::sync::Mutex;
        struct Recorder(Mutex<Vec<String>>);
        #[async_trait]
        impl ToolExecutor for Recorder {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok("dispatched")
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"fetch","url":"https://example.com"})),
            action_block(serde_json::json!({"tool":"done","reply":"recovered via oracle"})),
        ]);
        let exec = Recorder(Mutex::new(Vec::new()));
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            false,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Done("recovered via oracle".to_string())
        );
        assert!(
            exec.0.lock().unwrap().is_empty(),
            "the egress action must NEVER reach the executor"
        );
        let joined = drain(&mut rx).join("\n");
        assert!(
            joined.contains("egress disabled"),
            "the disabled marker streamed: {joined}"
        );
    }

    #[tokio::test]
    async fn repeated_egress_blocked_action_trips_no_progress() {
        // FIX 5: the SAME blocked fetch(url) twice must trip the no-progress guard
        // on the second attempt. Previously the egress-blocked branch did not push
        // to the executed window, so a model could repeat an action that can NEVER
        // run and burn every round; now the (tool, target) is recorded like a
        // dispatched action, so the second identical egress-blocked fetch escalates.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"fetch","url":"https://example.com"})),
            action_block(serde_json::json!({"tool":"fetch","url":"https://example.com"})),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            false,
            &tx,
        )
        .await;
        match outcome {
            BurstOutcome::Escalated(reason) => {
                assert!(reason.starts_with("no progress"), "reason: {reason}");
                assert!(reason.contains("fetch"), "names the cycled tool: {reason}");
            }
            other => {
                panic!("expected no-progress escalation on repeated blocked egress, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn egress_enabled_dispatches_normally() {
        // W7: with allow_egress=true the same fetch IS dispatched to the executor.
        use std::sync::Mutex;
        struct Recorder(Mutex<Vec<String>>);
        #[async_trait]
        impl ToolExecutor for Recorder {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok("dispatched")
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"fetch","url":"https://example.com"})),
            action_block(serde_json::json!({"tool":"done","reply":"ok"})),
        ]);
        let exec = Recorder(Mutex::new(Vec::new()));
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("ok".to_string()));
        assert_eq!(
            *exec.0.lock().unwrap(),
            vec!["fetch".to_string()],
            "the egress action was dispatched when allowed"
        );
    }

    #[tokio::test]
    async fn deadline_escalates_with_time_cap() {
        // The clock trips past the deadline after the first elapsed() check that
        // lets one round run, then reports over-budget so the next iteration caps.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"read","path":"foo.rs"})),
            action_block(serde_json::json!({"tool":"read","path":"bar.rs"})),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        // First elapsed() (top of round 1) is under budget; the second (top of
        // round 2) is over -> "time cap reached" before any further dispatch.
        let clock = ArmedClock {
            before: Duration::from_secs(0),
            after: Duration::from_secs(999),
            trip_after: 1,
            calls: AtomicUsize::new(0),
        };
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            Duration::from_secs(120),
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated("time cap reached".to_string())
        );
    }

    #[tokio::test]
    async fn run_plan_execution_time_is_excluded_from_the_wall_clock() {
        // run_plan executes a human-approved plan deterministically and can take many
        // minutes; that time must NOT count against the burst budget. The scripted clock
        // simulates run_plan consuming ~1000s. WITHOUT exclusion the round-2 top-of-loop
        // check (elapsed 1010 ≥ budget 120) would escalate "time cap reached"; WITH
        // exclusion (1010 − 1000 = 10 < 120) the burst proceeds to a clean `done`.
        struct ScriptedClock {
            values: Vec<u64>,
            calls: AtomicUsize,
        }
        impl Clock for ScriptedClock {
            fn elapsed(&self) -> Duration {
                let n = self.calls.fetch_add(1, Ordering::Relaxed);
                let secs = self
                    .values
                    .get(n)
                    .copied()
                    .unwrap_or_else(|| self.values.last().copied().unwrap_or(0));
                Duration::from_secs(secs)
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"run_plan"})),
            action_block(serde_json::json!({"tool":"done","reply":"plan executed"})),
        ]);
        let exec = StubExecutor;
        // elapsed() calls: [round1-top=0, dispatch-start=10, post-dispatch=1010,
        // round2-top=1010]. excluded = 1010-10 = 1000; round2 check = 1010-1000 = 10.
        let clock = ScriptedClock {
            values: vec![0, 10, 1010, 1010],
            calls: AtomicUsize::new(0),
        };
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "go".into(),
            &model,
            &exec,
            &clock,
            Duration::from_secs(120),
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Done("plan executed".to_string()),
            "run_plan's long deterministic execution is excluded from the wall-clock cap"
        );
    }

    #[tokio::test]
    async fn mcp_tool_with_no_known_servers_is_a_format_error() {
        // The default executor exposes NO user servers (`known_mcp_servers() == &[]`),
        // so an `mcp_tool` is rejected at PARSE time as a format error — it never
        // reaches the executor. Three such errors escalate via the consecutive guard.
        // Three DISTINCT mcp_tool blocks (different server names) so each is a fresh
        // unknown-server format error — escalating via the consecutive-format-error
        // guard rather than the repeated-identical-output loop detector.
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"mcp_tool","server":"db-a","name":"query","params":{}})),
            action_block(serde_json::json!({"tool":"mcp_tool","server":"db-b","name":"query","params":{}})),
            action_block(serde_json::json!({"tool":"mcp_tool","server":"db-c","name":"query","params":{}})),
            action_block(serde_json::json!({"tool":"done","reply":"unreached"})),
        ]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated(format!("{MAX_FORMAT_ERRORS} consecutive format errors")),
            "an mcp_tool with no configured servers is a format error each time"
        );
        let joined = drain(&mut rx).join("\n");
        assert!(
            joined.contains("format error"),
            "the unknown-server rejection surfaced as a format error: {joined}"
        );
        assert!(
            joined.contains("unknown MCP server"),
            "the format error names the unknown-server cause: {joined}"
        );
    }

    #[tokio::test]
    async fn mcp_tool_with_a_known_server_dispatches() {
        // An executor that reports a known user server lets the model call it: the
        // burst parses the mcp_tool (server is in the set) and dispatches it.
        use std::sync::Mutex;
        struct UserExec(Mutex<Vec<String>>, Vec<String>);
        #[async_trait]
        impl ToolExecutor for UserExec {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok("user tool ran")
            }
            fn known_mcp_servers(&self) -> &[String] {
                &self.1
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"mcp_tool","server":"my-db","name":"query","params":{"q":1}})),
            action_block(serde_json::json!({"tool":"done","reply":"ok"})),
        ]);
        let exec = UserExec(Mutex::new(Vec::new()), vec!["my-db".to_string()]);
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("ok".to_string()));
        assert_eq!(
            *exec.0.lock().unwrap(),
            vec!["mcp_tool".to_string()],
            "the mcp_tool was dispatched to the executor"
        );
    }

    #[tokio::test]
    async fn mcp_tool_to_known_server_is_allowed_with_web_egress_off() {
        // DECOUPLING (design §5.2): a user MCP server is its OWN opt-in capability,
        // separate from the web-search (Exa) opt-in. So a `mcp_tool` naming a KNOWN
        // configured server MUST be dispatched even when web egress is OFF
        // (`allow_egress=false`) — the web-egress gate (is_egress && !allow_egress)
        // only blocks fetch/websearch. The known-server set is the user-MCP gate.
        use std::sync::Mutex;
        struct UserExec(Mutex<Vec<String>>, Vec<String>);
        #[async_trait]
        impl ToolExecutor for UserExec {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok("user tool ran")
            }
            fn known_mcp_servers(&self) -> &[String] {
                &self.1
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"mcp_tool","server":"my-db","name":"query","params":{}})),
            action_block(serde_json::json!({"tool":"done","reply":"ok"})),
        ]);
        let exec = UserExec(Mutex::new(Vec::new()), vec!["my-db".to_string()]);
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            false, // WEB egress OFF — must NOT block a known-server mcp_tool
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("ok".to_string()));
        assert_eq!(
            *exec.0.lock().unwrap(),
            vec!["mcp_tool".to_string()],
            "a known-server mcp_tool must be dispatched even with web egress disabled"
        );
        // The web-egress "disabled" recovery message must NEVER fire for mcp_tool.
        let joined = drain(&mut rx).join("\n");
        assert!(
            !joined.contains("egress disabled"),
            "mcp_tool must not surface the web-egress-disabled message: {joined}"
        );
    }

    #[tokio::test]
    async fn web_fetch_is_still_blocked_when_egress_disabled() {
        // The WEB tools stay gated on allow_egress (UNCHANGED): with egress OFF a fetch
        // is never dispatched and the model gets the recovery message.
        use std::sync::Mutex;
        struct WebExec(Mutex<Vec<String>>);
        #[async_trait]
        impl ToolExecutor for WebExec {
            async fn execute(&self, action: &AgentAction) -> ToolResult {
                self.0.lock().unwrap().push(action.tool_name().to_string());
                ToolResult::ok("should not run")
            }
        }
        let model = ScriptedModel::new(vec![
            action_block(serde_json::json!({"tool":"fetch","url":"https://example.com"})),
            action_block(serde_json::json!({"tool":"done","reply":"recovered"})),
        ]);
        let exec = WebExec(Mutex::new(Vec::new()));
        let clock = FixedClock(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            false, // egress OFF
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::Done("recovered".to_string()));
        assert!(
            exec.0.lock().unwrap().is_empty(),
            "a web fetch must never reach the executor when egress is disabled"
        );
        let joined = drain(&mut rx).join("\n");
        assert!(joined.contains("egress disabled"), "the disabled marker streamed: {joined}");
    }

    #[tokio::test]
    async fn ask_user_hands_back() {
        let model = ScriptedModel::new(vec![action_block(
            serde_json::json!({"tool":"ask_user","question":"which env?"}),
        )]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(outcome, BurstOutcome::AskUser("which env?".to_string()));
    }

    #[tokio::test]
    async fn model_escalate_is_surfaced() {
        let model = ScriptedModel::new(vec![action_block(
            serde_json::json!({"tool":"escalate","reason":"out of my depth"}),
        )]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst(
            "x".into(),
            &model,
            &exec,
            &clock,
            DEFAULT_BURST_BUDGET,
            true,
            &tx,
        )
        .await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated("out of my depth".to_string())
        );
    }

    #[tokio::test]
    async fn stub_executor_signals_not_connected_for_every_tool() {
        // FIX 1 (safety): the no-server fallback must NEVER return a plausible
        // success — a live model would treat it as real and hallucinate. Every
        // non-terminal tool must come back as an ERROR carrying the unmistakable
        // not-connected wording so the model stops instead of fabricating.
        let exec = StubExecutor;
        let actions = [
            AgentAction::OracleAsk { query: "q".into() },
            AgentAction::OracleContext {
                query: "q".into(),
                limit: None,
            },
            AgentAction::Plan {
                steps: vec!["s".into()],
            },
            AgentAction::RunPlan {},
            AgentAction::SpawnMini {
                task: "t".into(),
                files: vec!["a.rs".into()],
                write: true,
            },
            AgentAction::SpawnMini {
                task: "t".into(),
                files: vec!["a.rs".into()],
                write: false,
            },
            AgentAction::Read {
                path: "a.rs".into(),
            },
            AgentAction::Grep {
                pattern: "x".into(),
                glob: None,
            },
            AgentAction::Glob {
                pattern: "*.rs".into(),
            },
            AgentAction::Fetch {
                url: "https://example.com".into(),
            },
            AgentAction::Websearch { query: "q".into() },
            AgentAction::McpTool {
                server: "my-db".into(),
                tool: "query".into(),
                params: serde_json::json!({}),
            },
        ];
        for action in &actions {
            let result = exec.execute(action).await;
            assert!(
                !result.ok,
                "{} stub result must be an error",
                action.tool_name()
            );
            assert_eq!(
                result.output,
                STUB_NOT_CONNECTED,
                "{} stub result must be the not-connected signal",
                action.tool_name()
            );
            assert!(
                result.output.contains("TOOL UNAVAILABLE")
                    && result.output.contains("NOT connected")
                    && result.output.contains("Do NOT fabricate"),
                "{} stub result must carry the not-connected wording",
                action.tool_name()
            );
        }
    }
}
