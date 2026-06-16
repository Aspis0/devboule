//! Application state + rendering.
//!
//! [`App`] owns the conversation, the input pane ([`tui_textarea::TextArea`]),
//! the spinner state, and the scroll offset. Rendering is a free function
//! [`render`] that takes a `Frame` so it can be driven by both the live
//! crossterm backend and a `TestBackend` in tests with zero terminal I/O.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, ThrobberState, WhichUse, BRAILLE_SIX};
use tui_textarea::TextArea;

use crate::conversation::{Conversation, Message, Role};

/// Whether the app is currently waiting on / streaming a model reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Thinking,
}

pub struct App {
    pub conversation: Conversation,
    pub input: TextArea<'static>,
    pub activity: Activity,
    pub throbber_state: ThrobberState,
    /// Number of lines scrolled UP from the bottom. 0 == pinned to newest
    /// (auto-scroll). Increasing it walks backward through history.
    pub scroll_back: u16,
    /// Set when the user requests quit; the event loop reads this and exits.
    pub should_quit: bool,
    /// Set whenever something changed that the next frame must reflect (input
    /// edit, new chunk, submit, scroll). The idle tick redraws ONLY when this is
    /// set (or the spinner is animating), so a quiet UI burns no CPU re-parsing
    /// markdown 16×/s. Cleared by the loop right after it repaints.
    pub dirty: bool,
    /// Cache of the rendered scrollback. `lines` holds, in conversation order,
    /// the rendered (markdown-parsed, owned) lines of every FINALIZED message;
    /// `count` is how many messages it covers. The in-progress streaming
    /// assistant message is NEVER cached (its body grows each chunk) — it is
    /// rendered live and appended after the cache. Because the conversation is
    /// append-only and only the trailing message mutates, the cache only ever
    /// grows by appending; see [`conversation_text`].
    cache: RenderCache,
    /// Last `max_offset` (content height beyond the viewport) computed by
    /// [`render_messages`]. `scroll_back` is clamped to this so heavy PageUp
    /// cannot accumulate past the top — otherwise PageDown would need many
    /// presses to return to the bottom.
    max_offset: u16,
}

/// Per-message render cache. One `Vec<Line>` block per finalized message, in
/// conversation order. Parsed once, reused every frame.
#[derive(Default)]
struct RenderCache {
    blocks: Vec<Vec<Line<'static>>>,
    /// Number of leading conversation messages whose rendered blocks are stored.
    count: usize,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut input = TextArea::default();
        input.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" message (Enter to send · Esc to quit · Ctrl-C) "),
        );
        input.set_cursor_line_style(Style::default());
        Self {
            conversation: Conversation::new(),
            input,
            activity: Activity::Idle,
            throbber_state: ThrobberState::default(),
            scroll_back: 0,
            should_quit: false,
            dirty: true,
            cache: RenderCache::default(),
            max_offset: 0,
        }
    }

    /// Mark the UI as needing a repaint on the next tick.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The current input pane contents joined into one string.
    pub fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    /// `true` if the input pane has no non-whitespace content.
    pub fn input_is_empty(&self) -> bool {
        self.input.lines().iter().all(|l| l.trim().is_empty())
    }

    /// Replace the input pane with a fresh empty one (after submit).
    pub fn clear_input(&mut self) {
        let block = self.input.block().cloned();
        self.input = TextArea::default();
        if let Some(block) = block {
            self.input.set_block(block);
        }
        self.input.set_cursor_line_style(Style::default());
    }

    pub fn scroll_up(&mut self, lines: u16) {
        // Clamp to the last-known scrollable height so repeated PageUp cannot
        // accumulate past the top; otherwise PageDown would need as many extra
        // presses to walk the overshoot back down.
        self.scroll_back = self
            .scroll_back
            .saturating_add(lines)
            .min(self.max_offset);
        self.dirty = true;
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll_back = self.scroll_back.saturating_sub(lines);
        self.dirty = true;
    }

    /// Re-pin to the newest message (called on submit / new chunk).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_back = 0;
        self.dirty = true;
    }
}

/// Render ONE message into owned lines: a styled role header, the body (human
/// plain, assistant markdown-parsed), then a blank spacer line. Self-contained
/// so concatenating every message's block reproduces the full scrollback.
///
/// This is the only place markdown is parsed; the cache calls it once per
/// finalized message, and the live tail calls it for the in-progress message.
fn render_message(msg: &Message) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    match msg.role {
        Role::Human => {
            lines.push(Line::from(Span::styled(
                "you",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            for raw in msg.body.lines() {
                lines.push(Line::from(raw.to_string()));
            }
        }
        Role::Assistant => {
            lines.push(Line::from(Span::styled(
                "devboule",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            // tui-markdown borrows from the input str; clone into owned lines so
            // the returned lines are 'static and can be cached.
            let md = tui_markdown::from_str(&msg.body);
            for line in md.lines {
                lines.push(owned_line(line));
            }
        }
    }
    lines.push(Line::from(String::new()));
    lines
}

/// Build the scrollback [`Text`] using `cache`, parsing markdown at most once per
/// finalized message.
///
/// Invariant kept here: `cache.blocks[i]` is the rendered block of
/// `conv.messages()[i]`, for every `i < cache.count`, and every cached message
/// is FINALIZED (its body is frozen). The only message that can still change is
/// the trailing one while the conversation is streaming; it is rendered live on
/// every frame and never cached. Since the conversation is append-only and only
/// that trailing message mutates, the cache only ever grows.
fn conversation_text(conv: &Conversation, cache: &mut RenderCache) -> Text<'static> {
    let msgs = conv.messages();
    // Number of leading messages that are FINALIZED and therefore cacheable: all
    // of them, minus the in-progress streaming tail (if any).
    let stable = msgs.len() - usize::from(conv.is_streaming());

    // Defensive: the conversation is append-only, so the cache should only ever
    // need to grow. If that assumption is ever violated (cache covers messages
    // that no longer exist, or a cached body could have changed), drop it and
    // re-render from scratch rather than serve stale/misaligned lines.
    if cache.count > stable {
        cache.blocks.clear();
        cache.count = 0;
    }

    // Render & cache any newly-finalized messages (parses markdown ONCE each).
    for msg in &msgs[cache.count..stable] {
        cache.blocks.push(render_message(msg));
    }
    cache.count = stable;

    // Assemble: cached finalized blocks + the live-rendered streaming tail.
    let mut lines: Vec<Line<'static>> = Vec::new();
    for block in &cache.blocks {
        lines.extend(block.iter().cloned());
    }
    if stable < msgs.len() {
        lines.extend(render_message(&msgs[stable]));
    }
    Text::from(lines)
}

/// Deep-clone a borrowed [`Line`] into an owned (`'static`) one. tui-markdown
/// returns a `Text<'a>` borrowing the source string; we own the spans so the
/// scrollback Text can outlive the message borrow within this call.
fn owned_line(line: Line) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    Line::from(spans).alignment_or(line.alignment)
}

/// Small helper to re-apply an optional alignment to a Line.
trait LineAlign {
    fn alignment_or(self, a: Option<ratatui::layout::Alignment>) -> Self;
}
impl LineAlign for Line<'static> {
    fn alignment_or(mut self, a: Option<ratatui::layout::Alignment>) -> Self {
        self.alignment = a;
        self
    }
}

/// Render one frame. Pure function of `app` + `area`, so a `TestBackend` can
/// drive it directly.
pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // messages pane
            Constraint::Length(1), // status line
            Constraint::Length(3), // input pane
        ])
        .split(area);

    render_messages(app, frame, chunks[0]);
    render_status(app, frame, chunks[1]);
    frame.render_widget(&app.input, chunks[2]);
}

fn render_messages(app: &mut App, frame: &mut Frame, area: Rect) {
    // Split-borrow: the cache and the conversation are distinct fields.
    let text = conversation_text(&app.conversation, &mut app.cache);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" conversation ");
    let inner_height = block.inner(area).height;
    // Saturate: a conversation taller than u16::MAX lines must clamp, not wrap —
    // a wrapping cast would corrupt the scroll math silently.
    let total_lines = u16::try_from(text.lines.len()).unwrap_or(u16::MAX);

    // Auto-scroll: bottom-anchored offset, walked up by `scroll_back`.
    let max_offset = total_lines.saturating_sub(inner_height);
    // Record the scrollable height so scroll_up can clamp against it next time.
    app.max_offset = max_offset;
    // Clamp here too: content may have SHRUNK relative to the viewport (e.g. a
    // window resize) since the last scroll, so an old scroll_back could now
    // exceed max_offset and push the view above the top.
    app.scroll_back = app.scroll_back.min(max_offset);
    let offset = max_offset.saturating_sub(app.scroll_back);

    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((offset, 0));
    frame.render_widget(paragraph, area);
}

fn render_status(app: &mut App, frame: &mut Frame, area: Rect) {
    match app.activity {
        Activity::Idle => {
            let p = Paragraph::new(Line::from(Span::styled(
                " idle",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(p, area);
        }
        Activity::Thinking => {
            let throbber = Throbber::default()
                .label(" thinking…")
                .style(Style::default().fg(Color::Yellow))
                .throbber_set(BRAILLE_SIX)
                .use_type(WhichUse::Spin);
            frame.render_stateful_widget(throbber, area, &mut app.throbber_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Flatten a TestBackend buffer into a single string for substring asserts.
    fn buffer_to_string(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn renders_conversation_and_input_into_buffer() {
        let mut app = App::new();
        app.conversation.push_human("how do I list files");
        app.conversation.begin_assistant();
        app.conversation
            .push_assistant_chunk("Use the `ls` command.");
        app.conversation.end_assistant();
        app.input.insert_str("my draft reply");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(&mut app, f))
            .expect("render one frame");

        let screen = buffer_to_string(&terminal);
        // Conversation content (both roles) is present.
        assert!(screen.contains("how do I list files"), "human turn missing");
        assert!(screen.contains("Use the"), "assistant turn missing");
        assert!(screen.contains("ls command"), "assistant body missing");
        // Input pane content is present (proves the input pane is wired).
        assert!(screen.contains("my draft reply"), "input text missing");
    }

    #[test]
    fn status_shows_thinking_when_streaming() {
        let mut app = App::new();
        app.activity = Activity::Thinking;
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let screen = buffer_to_string(&terminal);
        assert!(screen.contains("thinking"), "spinner label missing");
    }

    /// Render the full scrollback bypassing the cache (parses every message
    /// fresh each call). The cached path must produce byte-identical `Text`.
    fn uncached_text(conv: &Conversation) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in conv.messages() {
            lines.extend(render_message(msg));
        }
        Text::from(lines)
    }

    #[test]
    fn cached_render_matches_fresh_render() {
        let mut conv = Conversation::new();
        conv.push_human("how do I list files");
        conv.begin_assistant();
        conv.push_assistant_chunk("Use `ls`.\n\n```sh\nls -la\n```\n");
        conv.end_assistant();
        conv.push_human("thanks");

        let mut cache = RenderCache::default();
        // First pass populates the cache; second pass serves entirely from it.
        let first = conversation_text(&conv, &mut cache);
        let second = conversation_text(&conv, &mut cache);
        let fresh = uncached_text(&conv);

        assert_eq!(first, fresh, "first (cache-filling) pass must equal fresh");
        assert_eq!(second, fresh, "second (cache-served) pass must equal fresh");
        // Both finalized messages are cached; nothing is re-parsed live.
        assert_eq!(cache.count, conv.messages().len());
    }

    #[test]
    fn cache_excludes_streaming_tail_then_appends_on_finalize() {
        let mut conv = Conversation::new();
        conv.push_human("q");
        conv.begin_assistant();
        conv.push_assistant_chunk("partial");

        let mut cache = RenderCache::default();
        let streaming = conversation_text(&conv, &mut cache);
        // Only the finalized human turn is cached; the streaming tail is live.
        assert_eq!(cache.count, 1, "streaming assistant tail must NOT be cached");
        assert_eq!(streaming, uncached_text(&conv), "live tail must render");

        // Tail keeps growing while streaming: the cache must not freeze it.
        conv.push_assistant_chunk(" more");
        let grown = conversation_text(&conv, &mut cache);
        assert_eq!(cache.count, 1, "tail stays uncached while streaming");
        assert_eq!(grown, uncached_text(&conv), "grown tail must render fully");

        // Finalize: the tail is now stable and must be appended to the cache.
        conv.end_assistant();
        let finalized = conversation_text(&conv, &mut cache);
        assert_eq!(cache.count, 2, "finalized tail must be appended to the cache");
        assert_eq!(finalized, uncached_text(&conv));
    }

    #[test]
    fn cache_invalidates_when_count_exceeds_messages() {
        // Defensive reset path: a cache claiming more messages than exist must be
        // dropped and rebuilt rather than serving misaligned lines.
        let mut conv = Conversation::new();
        conv.push_human("only one");
        let mut cache = RenderCache::default();
        cache.blocks.push(vec![Line::from("stale-A")]);
        cache.blocks.push(vec![Line::from("stale-B")]);
        cache.count = 2; // > messages().len() == 1

        let text = conversation_text(&conv, &mut cache);
        assert_eq!(cache.count, 1, "cache must reset to match the conversation");
        assert_eq!(text, uncached_text(&conv), "stale lines must not survive");
    }

    #[test]
    fn scroll_up_past_max_then_down_reaches_bottom() {
        // Build a conversation tall enough to scroll within a short viewport.
        let mut app = App::new();
        for i in 0..40 {
            app.conversation.push_human(format!("line {i}"));
        }
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        // Frame 1: establishes max_offset.
        terminal.draw(|f| render(&mut app, f)).unwrap();
        let max = app.max_offset;
        assert!(max > 0, "content must overflow the viewport for this test");

        // Hammer PageUp far past the top: scroll_back must clamp to max_offset.
        for _ in 0..100 {
            app.scroll_up(5);
            terminal.draw(|f| render(&mut app, f)).unwrap();
        }
        assert_eq!(app.scroll_back, max, "scroll_back must clamp to max_offset");

        // A SINGLE PageDown of the same step must move off the top (no overshoot
        // backlog to burn through). Before the fix scroll_back was ~500, so one
        // PageDown left it at ~495 and the view was still pinned to the top.
        app.scroll_down(5);
        assert_eq!(app.scroll_back, max - 5, "one PageDown moves immediately");

        // Enough PageDowns reaches the bottom (scroll_back == 0).
        for _ in 0..(max / 5 + 1) {
            app.scroll_down(5);
        }
        assert_eq!(app.scroll_back, 0, "PageDown must reach the bottom");
    }

    #[test]
    fn dirty_starts_set_and_clears() {
        // The loop clears `dirty` after painting; a fresh App starts dirty so the
        // first tick paints. mark_dirty re-arms it.
        let mut app = App::new();
        assert!(app.dirty, "a fresh App must start dirty for the initial paint");
        app.dirty = false;
        app.mark_dirty();
        assert!(app.dirty, "mark_dirty must re-arm the repaint");
    }
}
