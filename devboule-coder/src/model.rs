//! The coder model abstraction and GPU-free mock implementations.
//!
//! L2.1 has NO real LLM. [`CoderModel`] is the seam a real streaming model will
//! plug into later. It now serves two callers:
//!
//! * The L2.1 streaming path ([`CoderModel::reply`]) — used only by the legacy
//!   single-reply test surface and kept so those tests stay green. [`MockModel`]
//!   returns a canned, chunked reply so the streaming render path and the
//!   spinner are genuinely exercised.
//! * The L2.2 burst loop ([`CoderModel::next_output`]) — given the running
//!   [`Transcript`], produce the model's next RAW output for [`parse_action`] to
//!   interpret. [`ScriptedModel`] returns a fixed sequence of raw outputs so a
//!   test can script `[oracle_ask, read, done]` and assert the loop runs them in
//!   order. The real model (L2.3) generates this from the transcript.
//!
//! [`parse_action`]: crate::action::parse_action
//! [`Transcript`]: crate::agent_loop::Transcript

use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::agent_loop::Transcript;

/// A streaming chat model.
///
/// `reply` consumes the conversation context (here: just the latest human
/// prompt) and pushes the response to `tx` in chunks, as a real token stream
/// would. The implementation returns when the reply is fully sent; dropping
/// `tx` signals end-of-stream to the consumer.
///
/// `#[async_trait]` (L2.3): both methods are `async fn` but the trait stays
/// object-safe, so the binary keeps holding `Arc<dyn CoderModel>`. The async
/// shape is what lets the real L2.3 model ([`crate::model_client::OmlxModel`])
/// do non-blocking loopback inference inside `next_output` without blocking the
/// tokio runtime — the prior sync seam would have stalled the reactor.
#[async_trait]
pub trait CoderModel: Send + Sync {
    /// Stream a reply to `prompt` into `tx`. Awaits until the full reply has
    /// been sent.
    ///
    /// L2.1 path, retained for the streaming-render tests. The L2.2 burst loop
    /// uses [`CoderModel::next_output`] instead, so in the non-test binary this
    /// method is currently unwired (silenced there, still live in test builds).
    #[cfg_attr(not(test), allow(dead_code))]
    async fn reply(&self, prompt: String, tx: mpsc::Sender<String>);

    /// Produce the model's next RAW output for the current burst, given the full
    /// running [`Transcript`] (the human message + every prior action and its
    /// tool result). The burst loop feeds the returned string to
    /// [`parse_action`](crate::action::parse_action).
    ///
    /// ASYNC (L2.3): the real model awaits a loopback chat-completions call
    /// here. The burst loop ([`run_burst`]) `.await`s it. Tests still drive it
    /// trivially under `#[tokio::test]`; [`ScriptedModel`] guards its cursor with
    /// a `Mutex` so the `&self` shared borrow is fine.
    ///
    /// [`run_burst`]: crate::agent_loop::run_burst
    async fn next_output(&self, transcript: &Transcript) -> String;
}

/// Canned model: acknowledges the user input and emits a short markdown blob in
/// a few chunks with small delays, so streaming is observable.
///
/// The L2.1 streaming surface ([`MockModel::reply`] / [`MockModel::chunks_for`] /
/// `chunk_delay`) is RETAINED — the existing L2.1 streaming-render tests still
/// exercise it, and it is the reference shape the real L2.3 streaming model will
/// implement. The L2.2 burst loop drives the model via [`CoderModel::next_output`]
/// instead, so in the (non-test) binary the `reply` path is currently unwired;
/// the `cfg_attr` below silences dead-code there WITHOUT hiding a real unused-code
/// regression in test builds.
#[derive(Debug, Clone, Default)]
pub struct MockModel {
    /// Per-chunk delay. Defaults to a small visible value; tests override it to
    /// zero to stay fast.
    #[cfg_attr(not(test), allow(dead_code))]
    chunk_delay: Duration,
}

impl MockModel {
    pub fn new() -> Self {
        Self::with_delay(Duration::from_millis(120))
    }

    /// Construct with an explicit per-chunk delay (tests use `Duration::ZERO`).
    pub fn with_delay(chunk_delay: Duration) -> Self {
        Self { chunk_delay }
    }

    /// The exact chunk sequence this model would stream for `prompt`.
    ///
    /// Exposed (and used by `reply`) so a test can assert that concatenating the
    /// streamed chunks equals the full reply without driving timers.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn chunks_for(prompt: &str) -> Vec<String> {
        // Trim so an echoed prompt does not carry a trailing newline from the
        // input pane into the rendered markdown.
        let prompt = prompt.trim();
        vec![
            "**Devboule (mock)**\n\n".to_string(),
            "You said:\n\n".to_string(),
            format!("> {prompt}\n\n"),
            "_This is a canned, streamed reply_ ".to_string(),
            "— no model is running yet.".to_string(),
        ]
    }
}

#[async_trait]
impl CoderModel for MockModel {
    async fn reply(&self, prompt: String, tx: mpsc::Sender<String>) {
        let delay = self.chunk_delay;
        for chunk in Self::chunks_for(&prompt) {
            // Receiver gone (TUI quit mid-stream) -> stop cleanly.
            if tx.send(chunk).await.is_err() {
                return;
            }
            if !delay.is_zero() {
                sleep(delay).await;
            }
        }
    }

    /// The mock has no action policy: it immediately ends the burst with a
    /// canned `done` referencing the human message, so a burst driven by
    /// [`MockModel`] terminates in one round. (The scripted action sequences live
    /// in [`ScriptedModel`].)
    async fn next_output(&self, transcript: &Transcript) -> String {
        let human = transcript.human_message();
        let reply = format!("Mock reply to: {}", human.trim());
        let json = serde_json::json!({ "tool": "done", "reply": reply });
        format!("```action\n{json}\n```")
    }
}

/// A deterministic, GPU-free model that replays a FIXED sequence of raw outputs,
/// one per `next_output` call. Lets a burst test script
/// `[oracle_ask, read, done]` and assert the loop runs them in order.
///
/// The cursor is behind a `Mutex` so `next_output(&self, …)` can advance it
/// through the trait's shared borrow. Once the script is exhausted, every
/// further call returns [`ScriptedModel::exhausted_output`] — a `done` that ends
/// the burst — so a misconfigured test cannot spin forever waiting on a model
/// that has nothing left to say.
///
/// Test-only in L2.2 (the binary uses [`MockModel`]); silenced for dead-code in
/// the non-test build, exercised by the burst-loop tests.
#[cfg_attr(not(test), allow(dead_code))]
pub struct ScriptedModel {
    outputs: Vec<String>,
    cursor: Mutex<usize>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ScriptedModel {
    /// Build from a sequence of raw model outputs (each typically one fenced
    /// ```action``` block, but a malformed string is allowed so the loop's
    /// format-error feedback path can be exercised).
    pub fn new(outputs: Vec<String>) -> Self {
        Self {
            outputs,
            cursor: Mutex::new(0),
        }
    }

    /// What the model emits once its script is exhausted: a terminal `done`, so
    /// the burst cannot hang on an over-short script.
    fn exhausted_output() -> String {
        let json = serde_json::json!({
            "tool": "done",
            "reply": "scripted model exhausted",
        });
        format!("```action\n{json}\n```")
    }
}

#[async_trait]
impl CoderModel for ScriptedModel {
    /// Unused by the burst loop; the L2.1 streaming path is `MockModel`'s. A
    /// minimal honest implementation: stream the next scripted output as one
    /// chunk so the trait stays fully usable, then return.
    async fn reply(&self, _prompt: String, tx: mpsc::Sender<String>) {
        let next = self.next_output(&Transcript::new(String::new())).await;
        let _ = tx.send(next).await;
    }

    async fn next_output(&self, _transcript: &Transcript) -> String {
        // Take the cursor index inside a TIGHT block so the `MutexGuard` is DROPPED
        // before this `async fn` returns. A guard held across the function body
        // would be live across any future `.await` added here, making the returned
        // future `!Send` and breaking `tokio::spawn` of the burst. Poisoned only if
        // a prior call panicked under the guard (it cannot — no panic point here);
        // recover defensively. This block has no `.await`, so the guard never
        // crosses a suspend point.
        let next: Option<String> = {
            let mut cursor = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
            let idx = *cursor;
            if idx < self.outputs.len() {
                *cursor += 1;
                self.outputs.get(idx).cloned()
            } else {
                None
            }
        };
        next.unwrap_or_else(Self::exhausted_output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streamed_chunks_concatenate_to_full_reply() {
        let model = MockModel::with_delay(Duration::ZERO);
        let (tx, mut rx) = mpsc::channel(16);

        model.reply("ping".to_string(), tx).await;

        let mut got = String::new();
        while let Some(chunk) = rx.recv().await {
            got.push_str(&chunk);
        }

        let expected: String = MockModel::chunks_for("ping").concat();
        assert_eq!(got, expected);
        assert!(got.contains("ping"), "reply should echo the prompt");
        assert!(got.contains("Devboule (mock)"));
    }

    #[test]
    fn chunks_are_split_into_several_pieces() {
        // Streaming is only meaningful if there is more than one chunk.
        assert!(MockModel::chunks_for("x").len() >= 3);
    }

    #[tokio::test]
    async fn send_after_receiver_dropped_does_not_panic() {
        let model = MockModel::with_delay(Duration::ZERO);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        // Must return cleanly rather than panic when the consumer is gone.
        model.reply("q".to_string(), tx).await;
    }

    #[tokio::test]
    async fn mock_next_output_is_a_terminal_done() {
        // The mock's burst policy is a single canned `done` echoing the human.
        let model = MockModel::with_delay(Duration::ZERO);
        let transcript = Transcript::new("hello there".to_string());
        let raw = model.next_output(&transcript).await;
        let action = crate::action::parse_action(&raw).expect("mock emits a valid action");
        match action {
            crate::action::AgentAction::Done { reply } => {
                assert!(reply.contains("hello there"), "echoes the human: {reply}");
            }
            other => panic!("mock should emit done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn scripted_model_replays_in_order_then_exhausts_to_done() {
        let model = ScriptedModel::new(vec!["first".to_string(), "second".to_string()]);
        let t = Transcript::new(String::new());
        assert_eq!(model.next_output(&t).await, "first");
        assert_eq!(model.next_output(&t).await, "second");
        // Past the script: a terminal `done` so a burst cannot hang.
        let exhausted = model.next_output(&t).await;
        let action = crate::action::parse_action(&exhausted)
            .expect("the exhausted output is a valid action");
        assert!(matches!(action, crate::action::AgentAction::Done { .. }));
    }
}
