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

use crate::action::{parse_action, AgentAction, FormatError};
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
}

/// Canned, MCP-free executor for L2.2: every non-terminal action maps to a
/// deterministic stub result. Terminal actions never reach an executor (the loop
/// returns before dispatching them), so they are not represented here. Retained
/// as the no-config / `cargo run`-without-a-server default and as the test stub.
#[derive(Debug, Default, Clone, Copy)]
pub struct StubExecutor;

#[async_trait]
impl ToolExecutor for StubExecutor {
    async fn execute(&self, action: &AgentAction) -> ToolResult {
        match action {
            AgentAction::OracleAsk { .. } => ToolResult::ok("[stub: 2 snippets]"),
            AgentAction::OracleContext { .. } => ToolResult::ok("[stub: context block]"),
            AgentAction::Plan { steps } => {
                ToolResult::ok(format!("[stub: plan accepted, {} step(s)]", steps.len()))
            }
            AgentAction::SpawnMini { write, .. } => ToolResult::ok(format!(
                "[stub: mini {} done]",
                if *write { "write" } else { "read" }
            )),
            AgentAction::Read { path } => ToolResult::ok(format!("[stub file contents: {path}]")),
            AgentAction::Grep { .. } => ToolResult::ok("[stub: 0 matches]"),
            AgentAction::Glob { .. } => ToolResult::ok("[stub: 0 paths]"),
            AgentAction::Fetch { .. } => ToolResult::ok("[stub: fetched 0 bytes]"),
            AgentAction::Websearch { .. } => ToolResult::ok("[stub: 0 results]"),
            // Terminal actions are handled by the loop before dispatch; if one
            // ever reaches here it is a logic error, so report it rather than
            // silently succeed.
            AgentAction::AskUser { .. } | AgentAction::Done { .. } | AgentAction::Escalate { .. } => {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    /// The model's parsed action for a round.
    Action(AgentAction),
    /// The tool result fed back for the preceding action.
    Result(ToolResult),
    /// Format-error feedback pushed after an unparseable model turn.
    FormatFeedback(String),
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
    let mut executed_window: VecDeque<(String, String)> = VecDeque::with_capacity(NO_PROGRESS_WINDOW);

    loop {
        // Wall-clock cap: check BEFORE asking the model so an already-overrun
        // burst stops promptly rather than doing one more (possibly slow) round.
        if clock.elapsed() >= budget {
            return BurstOutcome::Escalated("time cap reached".to_string());
        }

        let raw = model.next_output(&transcript).await;

        match parse_action(&raw) {
            Err(fe) => {
                // A format error is NOT progress: feed precise guidance back and
                // do NOT consume the round budget. Three in a row aborts.
                let feedback = fe.feedback();
                emit(progress_tx, format!("⚠ format error: {}", short_reason(&fe))).await;
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
                        return BurstOutcome::Done(reply.clone());
                    }
                    AgentAction::AskUser { question } => {
                        emit(progress_tx, format!("❓ {}", elide(question))).await;
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
                // continue. The window is NOT updated (nothing was executed).
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
                let result = cap_result(executor.execute(&action).await);
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

        // Transcript holds BOTH tool results (drive a fresh burst to inspect it
        // via a probe executor — here we assert via the stub result strings).
        assert!(joined.contains("2 snippets"), "oracle stub result streamed");
        assert!(joined.contains("stub file contents"), "read stub result streamed");
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

        let outcome = run_burst("go".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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
        assert!(matches!(entries[0], TranscriptEntry::Action(AgentAction::Read { .. })));
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

        let outcome = run_burst("loop".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
        assert_eq!(outcome, BurstOutcome::Done("recovered".to_string()));
        let joined = drain(&mut rx).join("\n");
        assert!(joined.contains("format error"), "the error was surfaced");
        assert!(joined.contains("done"), "then the burst completed");
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("loop".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
        assert_eq!(outcome, BurstOutcome::Done("done".to_string()));

        let stored = model.captured.lock().unwrap().clone().expect("a result was stored");
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

        let _ = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
        let stored = model.0.lock().unwrap().clone().expect("a result was stored");
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

        let outcome =
            run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, false, &tx).await;
        assert_eq!(outcome, BurstOutcome::Done("recovered via oracle".to_string()));
        assert!(
            exec.0.lock().unwrap().is_empty(),
            "the egress action must NEVER reach the executor"
        );
        let joined = drain(&mut rx).join("\n");
        assert!(joined.contains("egress disabled"), "the disabled marker streamed: {joined}");
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

        let outcome =
            run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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
    async fn ask_user_hands_back() {
        let model = ScriptedModel::new(vec![action_block(
            serde_json::json!({"tool":"ask_user","question":"which env?"}),
        )]);
        let exec = StubExecutor;
        let clock = FixedClock(Duration::ZERO);
        let (tx, _rx) = mpsc::channel(64);

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
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

        let outcome = run_burst("x".into(), &model, &exec, &clock, DEFAULT_BURST_BUDGET, true, &tx).await;
        assert_eq!(
            outcome,
            BurstOutcome::Escalated("out of my depth".to_string())
        );
    }
}
