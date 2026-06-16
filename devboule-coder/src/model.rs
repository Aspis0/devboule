//! The coder model abstraction and a GPU-free mock implementation.
//!
//! L2.1 has NO real LLM. [`CoderModel`] is the seam a real streaming model will
//! plug into later; [`MockModel`] returns a canned, chunked reply so the
//! streaming render path and the spinner are genuinely exercised end-to-end.

use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

/// A streaming chat model.
///
/// `reply` consumes the conversation context (here: just the latest human
/// prompt) and pushes the response to `tx` in chunks, as a real token stream
/// would. The implementation returns when the reply is fully sent; dropping
/// `tx` signals end-of-stream to the consumer.
///
/// Object-safe on purpose: the async work is expressed via the channel rather
/// than an `async fn`, so the trait stays `dyn`-compatible and the call site can
/// hold a `Box<dyn CoderModel>` without extra machinery.
pub trait CoderModel: Send + Sync {
    /// Stream a reply to `prompt` into `tx`. Awaits until the full reply has
    /// been sent.
    fn reply<'a>(
        &'a self,
        prompt: String,
        tx: mpsc::Sender<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// Canned model: acknowledges the user input and emits a short markdown blob in
/// a few chunks with small delays, so streaming is observable.
#[derive(Debug, Clone, Default)]
pub struct MockModel {
    /// Per-chunk delay. Defaults to a small visible value; tests override it to
    /// zero to stay fast.
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

impl CoderModel for MockModel {
    fn reply<'a>(
        &'a self,
        prompt: String,
        tx: mpsc::Sender<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        let delay = self.chunk_delay;
        Box::pin(async move {
            for chunk in Self::chunks_for(&prompt) {
                // Receiver gone (TUI quit mid-stream) -> stop cleanly.
                if tx.send(chunk).await.is_err() {
                    return;
                }
                if !delay.is_zero() {
                    sleep(delay).await;
                }
            }
        })
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
}
