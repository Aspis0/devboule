//! Conversation state model.
//!
//! A conversation is an ordered list of turns. Each turn has a [`Role`] and a
//! text body. The body of an assistant turn is built up incrementally while the
//! model streams its reply: [`Conversation::push_assistant_chunk`] appends to
//! the in-progress assistant message, while human turns are pushed whole.
//!
//! This module is pure state — it knows nothing about rendering, async, or the
//! model. That keeps it trivially testable.

/// Who produced a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Human,
    Assistant,
}

/// A single turn in the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub body: String,
}

impl Message {
    fn new(role: Role, body: impl Into<String>) -> Self {
        Self {
            role,
            body: body.into(),
        }
    }
}

/// Ordered conversation history.
#[derive(Debug, Default, Clone)]
pub struct Conversation {
    messages: Vec<Message>,
    /// `true` while the trailing assistant message is still being streamed.
    /// Guards [`Conversation::push_assistant_chunk`] so chunks never leak into a
    /// finalized turn or a human turn.
    streaming: bool,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only view of the turns, oldest first.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// `true` if an assistant reply is currently being streamed in.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Append a completed human turn. Submitting a human turn ends any
    /// dangling stream defensively (the loop always finalizes first, but we do
    /// not want a stray flag to corrupt ordering).
    pub fn push_human(&mut self, body: impl Into<String>) {
        self.streaming = false;
        self.messages.push(Message::new(Role::Human, body));
    }

    /// Open a fresh, empty assistant turn and enter streaming mode. Chunks are
    /// then accumulated into it via [`Conversation::push_assistant_chunk`].
    ///
    /// Re-entrancy guard: if a stream is already open, this is a no-op rather
    /// than pushing a second, empty assistant message. Opening a new turn would
    /// orphan the prior in-progress one as an empty/partial message and break
    /// the single-flight invariant. No-op (not "finalize prior") is the safe
    /// choice: the caller is the single-flight submit path, so a double-begin is
    /// a logic error, and we must not silently finalize a reply that is still
    /// arriving.
    pub fn begin_assistant(&mut self) {
        if self.streaming {
            return;
        }
        self.messages.push(Message::new(Role::Assistant, String::new()));
        self.streaming = true;
    }

    /// Append a streamed chunk to the in-progress assistant turn.
    ///
    /// No-op if no assistant turn is open (defensive: a late chunk arriving
    /// after the turn was finalized must not resurrect or corrupt state).
    pub fn push_assistant_chunk(&mut self, chunk: &str) {
        if !self.streaming {
            return;
        }
        if let Some(last) = self.messages.last_mut() {
            debug_assert_eq!(last.role, Role::Assistant);
            last.body.push_str(chunk);
        }
    }

    /// Mark the in-progress assistant turn as complete.
    pub fn end_assistant(&mut self) {
        self.streaming = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushing_turns_preserves_order_and_roles() {
        let mut c = Conversation::new();
        c.push_human("hello");
        c.begin_assistant();
        c.push_assistant_chunk("hi there");
        c.end_assistant();
        c.push_human("second");

        let m = c.messages();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].role, Role::Human);
        assert_eq!(m[0].body, "hello");
        assert_eq!(m[1].role, Role::Assistant);
        assert_eq!(m[1].body, "hi there");
        assert_eq!(m[2].role, Role::Human);
        assert_eq!(m[2].body, "second");
    }

    #[test]
    fn streamed_chunks_accumulate_into_one_assistant_message() {
        let mut c = Conversation::new();
        c.push_human("q");
        c.begin_assistant();
        for chunk in ["Hello", ", ", "world", "!"] {
            c.push_assistant_chunk(chunk);
        }
        c.end_assistant();

        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].role, Role::Assistant);
        assert_eq!(c.messages()[1].body, "Hello, world!");
        assert!(!c.is_streaming());
    }

    #[test]
    fn chunk_without_open_assistant_is_ignored() {
        let mut c = Conversation::new();
        c.push_human("q");
        // No begin_assistant() — a stray chunk must not corrupt the human turn.
        c.push_assistant_chunk("leak");
        assert_eq!(c.messages().len(), 1);
        assert_eq!(c.messages()[0].body, "q");
    }

    #[test]
    fn double_begin_assistant_does_not_orphan_an_empty_message() {
        let mut c = Conversation::new();
        c.push_human("q");
        c.begin_assistant();
        c.push_assistant_chunk("partial reply");
        // A second begin while still streaming must be a no-op: no extra empty
        // assistant message, and the in-progress body is preserved.
        c.begin_assistant();
        c.push_assistant_chunk(" continues");
        c.end_assistant();

        assert_eq!(c.messages().len(), 2);
        assert_eq!(c.messages()[1].role, Role::Assistant);
        assert_eq!(c.messages()[1].body, "partial reply continues");
    }

    #[test]
    fn streaming_flag_tracks_lifecycle() {
        let mut c = Conversation::new();
        assert!(!c.is_streaming());
        c.begin_assistant();
        assert!(c.is_streaming());
        c.end_assistant();
        assert!(!c.is_streaming());
    }
}
