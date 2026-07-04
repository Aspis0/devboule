//! devboule-coder L2.1 — conversational TUI shell driven by a MockModel.
//!
//! Scope (L2.1 ONLY): a terminal REPL where the user chats with a model behind
//! the [`CoderModel`] seam. The bundled [`MockModel`] streams a canned reply so
//! the streaming render + spinner are exercised with NO real LLM, NO MCP, and
//! NO action protocol. Those land in later steps.
//!
//! Architecture: a tokio runtime; the model call runs in a spawned task that
//! sends reply chunks over an `mpsc` channel; the main loop `select!`s over
//! { terminal input (`EventStream`), the chunk channel, a ~60ms tick } and
//! redraws on tick or on a channel message. The terminal is restored on every
//! exit path (including panics) by [`TerminalGuard`].

mod action;
mod activity;
mod agent_loop;
mod app;
mod config;
mod conversation;
mod doubt_sensor;
mod executor;
mod model;
mod model_client;
mod multi_mcp;
mod planner;
mod preplan;
mod prompt;
mod reply_stream;
mod rmcp_backend;
mod runner;
mod session_persist;
mod skills;
mod steer;
mod terminal;

use std::sync::Arc;
use std::time::Duration;

use futures::future::OptionFuture;
use futures::StreamExt;
use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use tokio::sync::mpsc;
use tokio::time::interval;
use tui_textarea::Input;

use agent_loop::{run_burst, burst_budget, BurstOutcome, SystemClock};
use app::{render, Activity, App};
use config::Runtime;
use terminal::TerminalGuard;

/// Redraw cadence; also paces the spinner animation.
const TICK: Duration = Duration::from_millis(60);
/// Bound on in-flight reply chunks. Backpressures a fast model against the UI.
const CHUNK_BUFFER: usize = 64;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Headless smoke / automation surface: `--once "<prompt>"` (alias `--print`)
    // runs ONE burst with NO terminal raw-mode and NO TUI, prints the streamed
    // transcript + the terminal `BurstOutcome` to stdout, and exits. This is a
    // genuinely useful non-interactive entry point (CI smoke, scripted runs) AND
    // the only way to exercise the real model+MCP loop without a TTY. Anything
    // else falls through to the interactive ratatui TUI.
    if let Some(prompt) = parse_once_prompt(std::env::args().skip(1)) {
        return run_once(prompt).await;
    }

    // Orchestrator composer: when the app launched us with a seeded GOAL (`DEVBOULE_GOAL`),
    // run it HEADLESS as a CONVERSATION-NATIVE session (`run_session`) — a persistent
    // back-and-forth that survives recoverable burst limits and waits for the user's steers,
    // optionally biased plan-first by `DEVBOULE_PLAN_FIRST`. This is how the typed goal reaches
    // the planner (the TUI would otherwise wait for the operator to type it). The app streams
    // this stdout + the activity file into the live Projects view. Absent ⇒ the interactive TUI.
    if let Some(goal) = config::seeded_goal() {
        return run_session(goal).await;
    }

    // Resolve the model + executor from env BEFORE entering raw mode, so any
    // fallback note (oMLX/MCP disabled) prints to a normal terminal rather than
    // the alternate screen. With no config this yields the Mock + Stub so
    // `cargo run` works without a server.
    let runtime = Arc::new(config::build_runtime().await);
    let mut guard = TerminalGuard::enter()?;
    let result = run(&mut guard, runtime).await;
    // `guard` drops here -> terminal restored before we surface any error.
    result
}

/// Parse a `--once <prompt>` / `--print <prompt>` flag out of the CLI args
/// (already past argv[0]). Returns the prompt when present, else `None` (the
/// interactive TUI path). PURE over an iterator so it is unit-testable without a
/// process. The prompt is the SINGLE argument following the flag; a flag with no
/// following argument yields `None` (treated as no headless request) so a bare
/// `--once` cannot silently run an empty burst.
fn parse_once_prompt(mut args: impl Iterator<Item = String>) -> Option<String> {
    while let Some(arg) = args.next() {
        if arg == "--once" || arg == "--print" {
            return args.next().filter(|p| !p.trim().is_empty());
        }
    }
    None
}

/// What the orchestrator just did, carried across the steer wait so the next
/// burst is framed correctly when the user replies.
enum PriorTurn {
    /// Replied (Done) or just started — a plain continuation.
    Reply,
    /// Asked the user a question (AskUser) — the answer continues from there.
    Question(String),
    /// Paused on a RECOVERABLE burst limit (Escalated). NOT a failure that ends
    /// the session — the user's reply resumes it. This is the B1/B15a fix: the
    /// orchestrator is conversation-native, so hitting a per-burst time/round cap
    /// (or any escalation) pauses and waits for the human instead of dying.
    Paused(String),
}

/// Fold the user's steer reply into the running conversation string, given what
/// the orchestrator did on the prior turn. Pure (no I/O) so it is unit-testable.
fn fold_user_reply(conversation: &str, prior: &PriorTurn, answer: &str) -> String {
    match prior {
        PriorTurn::Reply => format!(
            "{conversation}\n\n[User says: {answer}]\n\nContinue the conversation."
        ),
        PriorTurn::Question(q) => format!(
            "{conversation}\n\n[You asked the user: {q}]\n[User answered: {answer}]\n\nContinue from here."
        ),
        PriorTurn::Paused(reason) => format!(
            "{conversation}\n\n[You paused: {reason}]\n[User says: {answer}]\n\nResume from here."
        ),
    }
}

/// The chat turn emitted when a burst escalates on a recoverable limit, so the
/// user SEES the pause and can steer. Must read as a pause, not a fatal error.
fn paused_note(reason: &str) -> String {
    format!(
        "I paused this turn ({reason}). I can keep going — tell me how you'd like to continue, or say \"go on\"."
    )
}

/// Dynamic budget for the cumulative session conversation. Uses 30% of the
/// model's context window (in tokens→chars) with a hard floor of 48_000 chars
/// (back-compat with the old fixed value); the remaining 70% is for the
/// within-burst transcript + system prompt + model output.
fn conversation_budget_chars() -> usize {
    const ENV_CONTEXT_WINDOW: &str = "DEVBOULE_CONTEXT_WINDOW";
    let window = std::env::var(ENV_CONTEXT_WINDOW)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&w| w >= 1024)
        .unwrap_or(12_000); // 48K chars / 4 = 12K tokens (old default)
    (window * 30 / 100 * 4).max(48_000)
}

/// Bound the running conversation to `max` CHARS (marker included), keeping the head
/// (original goal + early framing) and the recent tail with a trim marker between.
/// Splits on char boundaries so it never cuts a UTF-8 codepoint. Pure → unit-testable.
fn trim_conversation(conversation: String, max: usize) -> String {
    const TRIM_MARKER: &str = "\n\n…[earlier conversation trimmed to fit context]…\n\n";
    let total = conversation.chars().count();
    if total <= max {
        return conversation;
    }
    // Reviewer max-recall: if the budget can't even fit the marker, just hard-truncate to
    // `max` (degenerate; never hit in practice since MAX_CONVERSATION_CHARS ≫ marker).
    if max <= TRIM_MARKER.chars().count() {
        return conversation.chars().take(max).collect();
    }
    // The marker counts toward `max` so the result never exceeds it.
    let content_budget = max.saturating_sub(TRIM_MARKER.chars().count());
    let head_budget = (content_budget / 8).min(2_000);
    let tail_budget = content_budget.saturating_sub(head_budget);
    let chars: Vec<char> = conversation.chars().collect();
    let head: String = chars.iter().take(head_budget).collect();
    let tail: String = chars.iter().skip(total - tail_budget).collect();
    format!("{head}{TRIM_MARKER}{tail}")
}

/// Headless ONE-SHOT (`--once` / `--print`): build the runtime, run EXACTLY ONE
/// `run_burst` for `prompt`, print the streamed transcript + the terminal
/// `BurstOutcome`, and exit. NO raw mode, NO TUI, NO keep-alive — pure stdout for
/// CI smoke / scripted runs. (The app-launched orchestrator uses `run_session`.)
async fn run_once(prompt: String) -> std::io::Result<()> {
    let runtime = Arc::new(config::build_runtime().await);
    eprintln!("devboule --once: egress_enabled={}", runtime.allow_egress);
    println!("=== devboule headless burst ===");
    println!("prompt: {prompt}");
    println!("--- transcript ---");
    runtime.executor.emit_chat("user", &prompt);

    let outcome = run_one_burst(&runtime, prompt).await?;
    println!("--- outcome ---");
    match outcome {
        BurstOutcome::Done(reply) => println!("DONE: {reply}"),
        BurstOutcome::AskUser(question) => println!("ASK_USER: {question}"),
        BurstOutcome::Escalated(reason) => println!("ESCALATED: {reason}"),
    }
    Ok(())
}

/// CONVERSATION-NATIVE session (B1): the app-launched orchestrator. Runs a burst,
/// then STAYS ALIVE and waits for the user's next message (the steer inbox),
/// folding it into the running conversation. A reply (Done) is just the
/// orchestrator's turn, NOT the end; an Escalated burst is a RECOVERABLE pause —
/// it emits a chat turn and keeps waiting (it no longer kills the session, the
/// B15a bug). The session ends ONLY when the user stops replying (the wait window
/// elapses) or the app stops the process; the transcript persists on disk either
/// way (B15b). There is NO runaway: the loop BLOCKS on `wait_for_steer_reply`
/// between bursts, so another burst runs only after the human sends a message.
async fn run_session(goal: String) -> std::io::Result<()> {
    let runtime = Arc::new(config::build_runtime().await);
    eprintln!("devboule session: egress_enabled={}", runtime.allow_egress);
    println!("=== devboule orchestrator session ===");
    println!("goal: {goal}");
    println!("--- transcript ---");

    // v6 Phase 4 (resume): if the launcher set DEVBOULE_SESSION_FILE and a prior
    // conversation was persisted there, resume from it instead of starting fresh from the
    // goal — so a restarted orchestrator keeps its planning context.
    let session_file = std::env::var("DEVBOULE_SESSION_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from);
    // Only RESUME when the persisted conversation belongs to THIS goal. The conversation
    // always begins with the launch goal (it starts as `goal` and only appends), so a
    // persisted transcript that does NOT start with the current goal is a DIFFERENT run on
    // the same project — ignore it and start fresh (fixes: a new goal must not replay the
    // previous goal's stale conversation). The per-turn save below then overwrites it.
    let resumed = session_file
        .as_deref()
        .and_then(session_persist::load)
        .filter(|prev| prev.starts_with(&goal));

    // Surface the launch goal as the FIRST user chat turn ONLY on a fresh start (on a
    // resume the goal is already the head of the restored conversation).
    if resumed.is_none() {
        runtime.executor.emit_chat("user", &goal);
    } else {
        println!("--- resumed prior session ---");
    }

    let mut conversation = resumed.unwrap_or(goal);
    loop {
        // Reviewer F3: the cumulative conversation is the NON-evictable `human`
        // message of every burst, so it must be bounded or a long planning session
        // overflows the model context. Keep the head (the original goal + early
        // framing) and the recent tail.
        conversation = trim_conversation(conversation, conversation_budget_chars());
        // v6 Phase 4: persist the accumulated conversation each turn (best-effort).
        if let Some(ref f) = session_file {
            let _ = session_persist::save(f, &conversation);
        }
        let outcome = run_one_burst(&runtime, conversation.clone()).await?;
        let prior = match outcome {
            // The reply was already emitted as a chat bubble by the burst — do NOT
            // re-emit it. But DO fold it into the conversation (reviewer F5) so the
            // model keeps its own prior replies in context across turns.
            BurstOutcome::Done(reply) => {
                println!("--- replied; waiting for the user ---");
                conversation = format!("{conversation}\n\n[You replied: {reply}]");
                PriorTurn::Reply
            }
            BurstOutcome::AskUser(question) => {
                println!("--- asked the user; waiting ---");
                PriorTurn::Question(question)
            }
            BurstOutcome::Escalated(reason) => {
                // RECOVERABLE pause — NOT the end. Emit a chat turn so the user sees
                // it and can steer; the session stays alive.
                println!("--- paused ({reason}); waiting for the user ---");
                runtime.executor.emit_chat("assistant", &paused_note(&reason));
                PriorTurn::Paused(reason)
            }
        };
        match wait_for_steer_reply(&runtime).await {
            Some(answer) => {
                runtime.executor.emit_chat("user", &answer);
                conversation = fold_user_reply(&conversation, &prior, &answer);
                println!("--- continuing with the user's reply ---");
            }
            None => {
                println!("(no reply before the wait window elapsed — ending session)");
                break;
            }
        }
    }
    // Clean up session persistence so next launch starts fresh.
    if let Some(ref f) = session_file {
        let _ = std::fs::remove_file(f);
    }
    Ok(())
}

/// Run ONE burst for `human`, draining its progress to stdout live, returning the
/// terminal outcome. Factored out so the conversation loop can run several in sequence.
async fn run_one_burst(
    runtime: &Arc<config::Runtime>,
    human: String,
) -> std::io::Result<BurstOutcome> {
    let (tx, mut rx) = mpsc::channel::<String>(CHUNK_BUFFER);
    let burst_runtime = Arc::clone(runtime);
    let burst = tokio::spawn(async move {
        let clock = SystemClock::start_now();
        run_burst(
            human,
            burst_runtime.model.as_ref(),
            burst_runtime.executor.as_ref(),
            &clock,
            burst_budget(),
            burst_runtime.allow_egress,
            &tx,
        )
        .await
    });
    while let Some(line) = rx.recv().await {
        print!("{line}");
    }
    burst
        .await
        .map_err(|e| std::io::Error::other(format!("burst task failed: {e}")))
}

/// How long the orchestrator stays alive waiting for a steer reply after asking the user
/// a question, before it gives up and ends the session.
const STEER_REPLY_WAIT_SECS: u64 = 1800;

/// Poll the steer inbox until the user sends a reply (or the wait window elapses). The
/// process stays alive the whole time, so the chat composer can deliver the answer. If
/// several lines arrive together they are joined (the user's multi-line answer).
async fn wait_for_steer_reply(runtime: &Arc<config::Runtime>) -> Option<String> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(STEER_REPLY_WAIT_SECS);
    loop {
        let msgs = runtime.executor.drain_steer();
        if !msgs.is_empty() {
            return Some(msgs.join("\n"));
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
}

async fn run(guard: &mut TerminalGuard, runtime: Arc<Runtime>) -> std::io::Result<()> {
    let mut app = App::new();
    let mut events = EventStream::new();
    let mut ticker = interval(TICK);

    // `Some` while a reply is streaming; the loop owns the receiver so a single
    // `select!` arm drains chunks and detects end-of-stream (channel closed).
    let mut chunk_rx: Option<mpsc::Receiver<String>> = None;

    // Initial paint.
    guard.terminal_mut().draw(|f| render(&mut app, f))?;

    loop {
        tokio::select! {
            // Terminal input.
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        handle_event(&mut app, event, &runtime, &mut chunk_rx);
                        app.mark_dirty(); // an input/scroll/submit may have changed state
                        if app.should_quit {
                            // Quit while a reply is still streaming: finalize the
                            // conversation state and drop the receiver so we never
                            // exit with `streaming == true`. Dropping the receiver
                            // (`take`) also closes the channel, so the spawned
                            // model task's next `send` errors and it returns cleanly.
                            if chunk_rx.take().is_some() {
                                app.conversation.end_assistant();
                                app.activity = Activity::Idle;
                            }
                            break;
                        }
                    }
                    Some(Err(_)) => break, // terminal read error -> bail, guard restores
                    None => break,         // EOF on stdin
                }
                guard.terminal_mut().draw(|f| render(&mut app, f))?;
                app.dirty = false;
            }

            // Streamed reply chunks. `OptionFuture` borrows the receiver only
            // when one exists (no `unwrap`), and the `if` guard keeps the arm
            // disabled — so `chunk_rx` is un-borrowed — when no stream is live.
            // With the guard, `chunk_rx` is always `Some` here, so the outer
            // `OptionFuture` layer always yields `Some(_)`.
            chunk = OptionFuture::from(chunk_rx.as_mut().map(|rx| rx.recv())), if chunk_rx.is_some() => {
                match chunk {
                    Some(Some(text)) => {
                        app.conversation.push_assistant_chunk(&text);
                        app.scroll_to_bottom();
                    }
                    Some(None) => {
                        // Sender dropped -> reply complete.
                        app.conversation.end_assistant();
                        app.activity = Activity::Idle;
                        chunk_rx = None;
                    }
                    // Unreachable: the `if` guard ensures `chunk_rx` is `Some`,
                    // so `OptionFuture` never resolves to the outer `None`.
                    None => {}
                }
                app.mark_dirty();
                guard.terminal_mut().draw(|f| render(&mut app, f))?;
                app.dirty = false;
            }

            // Tick: advance the spinner and repaint — but only when something
            // actually changed (`dirty`) or the spinner needs to animate. At
            // idle with nothing pending this is a no-op, so we do NOT re-parse
            // the whole conversation markdown 16×/s for a static screen.
            _ = ticker.tick() => {
                if app.activity == Activity::Thinking {
                    app.throbber_state.calc_next();
                }
                if app.dirty || app.activity == Activity::Thinking {
                    guard.terminal_mut().draw(|f| render(&mut app, f))?;
                    app.dirty = false;
                }
            }
        }
    }

    Ok(())
}

/// Apply one terminal event to the app. On submit, spawns the model task and
/// installs the chunk receiver.
fn handle_event(
    app: &mut App,
    event: Event,
    runtime: &Arc<Runtime>,
    chunk_rx: &mut Option<mpsc::Receiver<String>>,
) {
    if let Event::Key(key) = event {
        // crossterm reports both Press and Release on some platforms; act on
        // Press only so a single keystroke is not handled twice.
        if key.kind != KeyEventKind::Press {
            return;
        }
        if handle_key(app, key, runtime, chunk_rx) {
            return;
        }
    }
    // Anything not handled as a control key feeds the input pane (text,
    // backspace, arrows, etc.). Paste is delivered char-by-char as Key events
    // because bracketed-paste is not enabled, so there is no Paste arm.
    if let Event::Key(_) = event {
        let input: Input = event.into();
        app.input.input(input);
    }
}

/// Handle control keys. Returns `true` if the key was consumed as a control
/// action (and must NOT also be fed to the input pane).
fn handle_key(
    app: &mut App,
    key: KeyEvent,
    runtime: &Arc<Runtime>,
    chunk_rx: &mut Option<mpsc::Receiver<String>>,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        // Quit: Ctrl-C always; Esc when the input is empty (so Esc can still be
        // a normal editor key while typing).
        KeyCode::Char('c') if ctrl => {
            app.should_quit = true;
            true
        }
        KeyCode::Esc if app.input_is_empty() => {
            app.should_quit = true;
            true
        }
        // Submit on Enter (without modifiers). Shift/Alt+Enter inserts a newline.
        KeyCode::Enter if key.modifiers.is_empty() => {
            submit(app, runtime, chunk_rx);
            true
        }
        KeyCode::PageUp => {
            app.scroll_up(5);
            true
        }
        KeyCode::PageDown => {
            app.scroll_down(5);
            true
        }
        _ => false,
    }
}

/// Push the human turn, open the assistant turn, and spawn the bounded inner
/// burst (L2.2). No-op if the input is blank or a burst is already streaming (the
/// agent is single-flight). The spawned task runs [`run_burst`], streaming a
/// progress line per action+result over the SAME chunk channel L2.1 uses, then
/// appends the burst CONCLUSION as the final chunk(s) before dropping `tx` (which
/// the loop's chunk arm sees as end-of-stream and finalizes the assistant turn).
fn submit(app: &mut App, runtime: &Arc<Runtime>, chunk_rx: &mut Option<mpsc::Receiver<String>>) {
    // Single-flight: guard at BOTH layers — the channel (a burst task is live)
    // and the conversation state (a turn is still streaming). Either alone is
    // sufficient today; defending both keeps the invariant honest if the
    // plumbing changes.
    if app.input_is_empty() || chunk_rx.is_some() || app.conversation.is_streaming() {
        return;
    }
    // Trim ONCE and use the SAME value for both the stored human turn and the
    // model prompt, so the rendered conversation and what the model sees never
    // diverge (e.g. a trailing newline from the input pane).
    let prompt = app.input_text().trim().to_string();
    app.clear_input();
    app.conversation.push_human(prompt.clone());
    app.conversation.begin_assistant();
    app.activity = Activity::Thinking;
    app.scroll_to_bottom();

    let (tx, rx) = mpsc::channel(CHUNK_BUFFER);
    *chunk_rx = Some(rx);
    let runtime = Arc::clone(runtime);
    tokio::spawn(async move {
        // The model + executor are the runtime-resolved ones (L2.3): real oMLX +
        // MCP/FS/Exa when configured, else Mock + Stub. `allow_egress` is the
        // executor's authoritative gate (true ONLY with a real Exa-backed
        // executor). The wall-clock cap uses the real [`SystemClock`].
        let clock = SystemClock::start_now();
        let outcome = run_burst(
            prompt,
            runtime.model.as_ref(),
            runtime.executor.as_ref(),
            &clock,
            burst_budget(),
            runtime.allow_egress,
            &tx,
        )
        .await;

        // Append the conclusion as the assistant turn's final content. A leading
        // blank line separates it from the streamed progress lines above.
        let conclusion = match outcome {
            BurstOutcome::Done(reply) => format!("\n\n{reply}"),
            BurstOutcome::AskUser(question) => format!("\n\n❓ {question}"),
            BurstOutcome::Escalated(reason) => format!("\n\n⚠ escalated: {reason}"),
        };
        let _ = tx.send(conclusion).await;
        // `tx` drops here -> the loop's chunk arm sees the channel close and
        // finalizes the assistant turn, handing control back to the human. For
        // AskUser this hand-back IS the conversational continuation: the human's
        // next message starts a fresh burst.
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> impl Iterator<Item = String> {
        v.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parse_once_prompt_reads_flag_and_following_value() {
        assert_eq!(
            parse_once_prompt(args(&["--once", "do the thing"])),
            Some("do the thing".to_string())
        );
        // `--print` is an accepted alias.
        assert_eq!(
            parse_once_prompt(args(&["--print", "summarize"])),
            Some("summarize".to_string())
        );
    }

    #[test]
    fn parse_once_prompt_is_none_without_the_flag() {
        // No flag -> interactive TUI path.
        assert_eq!(parse_once_prompt(args(&[])), None);
        assert_eq!(parse_once_prompt(args(&["something", "else"])), None);
    }

    #[test]
    fn parse_once_prompt_rejects_missing_or_blank_value() {
        // A bare flag (no following arg) must NOT run an empty burst.
        assert_eq!(parse_once_prompt(args(&["--once"])), None);
        // A blank/whitespace prompt is treated as absent for the same reason.
        assert_eq!(parse_once_prompt(args(&["--once", "   "])), None);
    }

    #[test]
    fn fold_user_reply_reply_case() {
        let result = fold_user_reply("prev", &PriorTurn::Reply, "hi");
        assert!(result.contains("[User says: hi]"));
        assert!(result.contains("Continue the conversation."));
    }

    #[test]
    fn fold_user_reply_question_case() {
        let result = fold_user_reply("prev", &PriorTurn::Question("why?".into()), "hi");
        assert!(result.contains("[You asked the user: why?]"));
        assert!(result.contains("[User answered: hi]"));
    }

    /// B15a: an escalation folds into a RESUME (not a session end). This is the
    /// regression guard for "the 2nd-turn steer kills the orchestrator".
    #[test]
    fn fold_user_reply_paused_case() {
        let result =
            fold_user_reply("prev", &PriorTurn::Paused("round cap reached".into()), "hi");
        assert!(result.contains("[You paused: round cap reached]"));
        assert!(result.contains("[User says: hi]"));
        assert!(result.contains("Resume"));
    }

    #[test]
    fn paused_note_reads_as_pause_not_failure() {
        let note = paused_note("time cap reached");
        assert!(note.contains("time cap reached"));
        assert!(note.contains("paused"));
        assert!(!note.contains("ESCALATED"));
        assert!(!note.contains("failed"));
    }

    /// Reviewer F3: a short conversation is returned verbatim; a long one is
    /// bounded to ~max chars, keeping the head (goal) and the recent tail.
    #[test]
    fn trim_conversation_bounds_long_and_keeps_short() {
        let short = "the original goal".to_string();
        assert_eq!(trim_conversation(short.clone(), 48_000), short);

        let head = "GOAL: build the thing";
        let long = format!("{head}{}", "x".repeat(60_000));
        let trimmed = trim_conversation(long, 48_000);
        assert!(
            trimmed.chars().count() <= 48_000,
            "must be bounded to max (marker included): {}",
            trimmed.chars().count()
        );
        assert!(trimmed.contains("GOAL: build the thing"), "head preserved");
        assert!(trimmed.contains("trimmed to fit context"), "marker present");
    }
}
