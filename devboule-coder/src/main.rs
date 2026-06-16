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

mod app;
mod conversation;
mod model;
mod terminal;

use std::sync::Arc;
use std::time::Duration;

use futures::future::OptionFuture;
use futures::StreamExt;
use ratatui::crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;
use tokio::time::interval;
use tui_textarea::Input;

use app::{render, Activity, App};
use model::{CoderModel, MockModel};
use terminal::TerminalGuard;

/// Redraw cadence; also paces the spinner animation.
const TICK: Duration = Duration::from_millis(60);
/// Bound on in-flight reply chunks. Backpressures a fast model against the UI.
const CHUNK_BUFFER: usize = 64;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let model: Arc<dyn CoderModel> = Arc::new(MockModel::new());
    let mut guard = TerminalGuard::enter()?;
    let result = run(&mut guard, model).await;
    // `guard` drops here -> terminal restored before we surface any error.
    result
}

async fn run(guard: &mut TerminalGuard, model: Arc<dyn CoderModel>) -> std::io::Result<()> {
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
                        handle_event(&mut app, event, &model, &mut chunk_rx);
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
    model: &Arc<dyn CoderModel>,
    chunk_rx: &mut Option<mpsc::Receiver<String>>,
) {
    if let Event::Key(key) = event {
        // crossterm reports both Press and Release on some platforms; act on
        // Press only so a single keystroke is not handled twice.
        if key.kind != KeyEventKind::Press {
            return;
        }
        if handle_key(app, key, model, chunk_rx) {
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
    model: &Arc<dyn CoderModel>,
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
            submit(app, model, chunk_rx);
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

/// Push the human turn, open the assistant turn, and spawn the streaming model
/// task. No-op if the input is blank or a reply is already streaming (the model
/// is single-flight in L2.1).
fn submit(app: &mut App, model: &Arc<dyn CoderModel>, chunk_rx: &mut Option<mpsc::Receiver<String>>) {
    // Single-flight: guard at BOTH layers — the channel (a reply task is live)
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
    let model = Arc::clone(model);
    tokio::spawn(async move {
        model.reply(prompt, tx).await;
        // `tx` drops here -> the loop's chunk arm sees the channel close.
    });
}
