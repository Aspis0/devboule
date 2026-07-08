//! B14b: incremental extraction of a terminal action's user-facing text (`done.reply` or
//! `ask_user.question`) from the orchestrator's RAW model output AS IT STREAMS, so the planner
//! chat can render the reply token-by-token before the full action block is complete.
//!
//! The model emits ONE fenced ```action block whose body is a JSON object, e.g.
//! `{"tool":"done","reply":"…"}`. While streaming we only have a PREFIX of that text. This
//! extractor is fed the CUMULATIVE raw-so-far on each chunk and returns the partial, JSON-
//! unescaped value of the terminal field — or `None` when the turn is not (yet, or not at all)
//! a streamable terminal reply. It is deliberately tolerant: anything ambiguous yields `None`
//! and the UI simply falls back to the final bubble. The authoritative parse still happens
//! once, at the end, via `parse_action_with_servers` — this is UX-only and must never panic.

/// Stateful, monotonic extractor for ONE assistant turn. Create one per turn; feed it the
/// cumulative raw output repeatedly. Once it decides the turn is non-streamable (a non-terminal
/// tool, e.g. `read`/`spawn_mini`) it latches to inert and returns `None` forever for that turn.
#[derive(Debug, Default)]
pub struct ReplyStreamExtractor {
    state: State,
}

#[derive(Debug, Default, PartialEq)]
enum State {
    /// Tool not yet determined from the prefix.
    #[default]
    Undecided,
    /// Streaming a known terminal field (`"reply"` for done, `"question"` for ask_user). The
    /// field is cached so later calls skip re-deriving the tool (no O(n) re-scan, and the latch
    /// can never flip Streaming→Inert on a growing prefix).
    Streaming { field: &'static str },
    /// A non-terminal tool — never stream this turn.
    Inert,
}

impl ReplyStreamExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the cumulative raw output so far. Returns `Some(partial_text)` (JSON-unescaped,
    /// possibly empty, possibly growing) when the turn is a streamable terminal reply and the
    /// field has begun; `None` otherwise. Never panics.
    pub fn feed(&mut self, cumulative: &str) -> Option<String> {
        // Anchor to the action JSON object so a `"tool"`/`"reply"` substring in any preamble or
        // thinking text BEFORE the ```action fence cannot mis-trigger extraction.
        let body = action_body(cumulative)?;
        match self.state {
            State::Inert => None,
            // Tool already known: stream its field directly (no re-derivation).
            State::Streaming { field } => extract_field_value(body, field),
            State::Undecided => {
                let tool = extract_tool_name(body)?; // None until the name's closing quote arrives
                let field = match tool.as_str() {
                    "done" => "reply",
                    "ask_user" => "question",
                    _ => {
                        self.state = State::Inert; // a non-terminal tool — never stream this turn
                        return None;
                    }
                };
                self.state = State::Streaming { field };
                extract_field_value(body, field)
            }
        }
    }
}

/// Return the slice starting at the action JSON object's opening `{`. Prefers the position after
/// the ```action fence the orchestrator emits; falls back to the first `{` in `s` (covers a bare
/// object). None when no `{` has been emitted yet. Anchoring here means a `"tool"`/`"reply"`
/// substring in plain preamble/thinking text before the object can never mis-trigger us.
fn action_body(s: &str) -> Option<&str> {
    const FENCE: &str = "```action";
    let start = match s.find(FENCE) {
        Some(f) => &s[f + FENCE.len()..],
        None => s,
    };
    let brace = start.find('{')?;
    Some(&start[brace..])
}

/// Scan `s` for the first complete `"tool"\s*:\s*"<name>"` and return `<name>`, or None if the
/// value's closing quote has not arrived yet (or there is no `"tool"` key). Tolerant of
/// whitespace around the colon.
fn extract_tool_name(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let key = s.find("\"tool\"")?;
    let mut j = key + 6;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b':' {
        return None;
    }
    j += 1;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b'"' {
        return None;
    }
    j += 1;
    let start = j;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2, // skip an escape (tool names never contain one, but stay safe)
            b'"' => return Some(s[start..j].to_string()),
            _ => j += 1,
        }
    }
    None // closing quote not yet streamed
}

/// Scan `s` for `"<field>"\s*:\s*"` and return the JSON-unescaped string value SO FAR, up to the
/// first unescaped `"` or end-of-input. Returns None if the field's opening quote hasn't arrived.
/// Decodes escapes progressively; an incomplete trailing escape (lone `\`, short `\u`) is dropped.
/// UTF-8 safe: literal runs are copied as `&str` slices, never byte-by-byte.
fn extract_field_value(s: &str, field: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let key = format!("\"{field}\"");
    let pos = s.find(&key)?;
    let mut j = pos + key.len();
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b':' {
        return None;
    }
    j += 1;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b'"' {
        return None;
    }
    j += 1;

    let mut out = String::new();
    let mut lit_start = j; // start of the current literal (unescaped) run
    while j < bytes.len() {
        match bytes[j] {
            b'"' => {
                out.push_str(&s[lit_start..j]); // closing quote: flush the literal run
                return Some(out);
            }
            b'\\' => {
                out.push_str(&s[lit_start..j]); // flush the literal run before the escape
                if j + 1 >= bytes.len() {
                    // lone trailing backslash — incomplete escape, drop it
                    return Some(out);
                }
                match bytes[j + 1] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'u' => {
                        let hs = j + 2;
                        if hs + 4 > bytes.len() {
                            return Some(out); // incomplete \uXXXX — drop it
                        }
                        // Slice the 4 hex chars from BYTES (never panics on a char boundary the
                        // way `&s[hs..hs+4]` could if malformed input put a multibyte char here);
                        // valid \uXXXX is ASCII hex, anything else decodes to None and is skipped.
                        if let Ok(hex) = std::str::from_utf8(&bytes[hs..hs + 4]) {
                            if let Ok(code) = u32::from_str_radix(hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    out.push(c);
                                }
                            }
                        }
                        j = hs + 4;
                        lit_start = j;
                        continue;
                    }
                    other => out.push(other as char), // unknown escape: keep the char as-is
                }
                j += 2;
                lit_start = j;
            }
            _ => j += 1, // part of a literal run (incl. multibyte UTF-8 continuation bytes)
        }
    }
    // value not closed yet — flush whatever literal run we have
    out.push_str(&s[lit_start..j]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_a_growing_done_reply() {
        let mut x = ReplyStreamExtractor::new();
        // tool not fully known yet -> None
        assert_eq!(x.feed("```action\n{\"tool\":\"do"), None);
        // reply field begun -> partial
        assert_eq!(
            x.feed(r#"```action
{"tool":"done","reply":"Hel"#),
            Some("Hel".to_string())
        );
        assert_eq!(
            x.feed(r#"```action
{"tool":"done","reply":"Hello wor"#),
            Some("Hello wor".to_string())
        );
        // closed value + closing fence -> full reply
        assert_eq!(
            x.feed(r#"```action
{"tool":"done","reply":"Hello world"}
```"#),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn empty_reply_value_is_some_empty_once_the_quote_opened() {
        let mut x = ReplyStreamExtractor::new();
        // tool known, field present but no opening quote yet -> None
        assert_eq!(x.feed(r#"{"tool":"done","#), None);
        // opening quote, empty value -> Some("")
        assert_eq!(x.feed(r#"{"tool":"done","reply":""#), Some(String::new()));
    }

    #[test]
    fn decodes_json_escapes_progressively() {
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(
            x.feed(r#"{"tool":"done","reply":"line1\nline2"#),
            Some("line1\nline2".to_string())
        );
        // an embedded escaped quote decodes to a literal quote
        let mut y = ReplyStreamExtractor::new();
        assert_eq!(
            y.feed(r#"{"tool":"done","reply":"say \"hi\" now"}"#),
            Some(r#"say "hi" now"#.to_string())
        );
    }

    #[test]
    fn drops_an_incomplete_trailing_escape() {
        let mut x = ReplyStreamExtractor::new();
        // a lone trailing backslash is an incomplete escape — drop it, don't panic
        assert_eq!(
            x.feed(r#"{"tool":"done","reply":"abc\"#),
            Some("abc".to_string())
        );
    }

    #[test]
    fn streams_an_ask_user_question() {
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(
            x.feed(r#"{"tool":"ask_user","question":"Which fil"#),
            Some("Which fil".to_string())
        );
    }

    #[test]
    fn non_terminal_tool_latches_inert_forever() {
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(x.feed(r#"{"tool":"read","path":"a"#), None);
        // even if later text superficially looks like a reply, stay inert for this turn
        assert_eq!(
            x.feed(r#"{"tool":"read","path":"a","reply":"x"}"#),
            None
        );
    }

    #[test]
    fn tolerates_whitespace_around_colons() {
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(
            x.feed(r#"{ "tool" : "done", "reply" : "hi"#),
            Some("hi".to_string())
        );
    }

    #[test]
    fn preserves_multibyte_utf8_in_the_reply() {
        // The orchestrator chat is often Italian — a byte-by-byte copy would corrupt "è"/emoji.
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(
            x.feed(r#"{"tool":"done","reply":"perché 你好 🚀 ok"#),
            Some("perché 你好 🚀 ok".to_string())
        );
    }

    #[test]
    fn decodes_a_unicode_escape_and_keeps_following_text() {
        // Catches the off-by-one that would skip the char right after \uXXXX.
        let mut x = ReplyStreamExtractor::new();
        // A == 'A'; the "BC done" right after the escape must NOT be skipped.
        let input = "{\"tool\":\"done\",\"reply\":\"\\u0041BC done\"}";
        assert_eq!(x.feed(input), Some("ABC done".to_string()));
    }

    #[test]
    fn ignores_a_tool_substring_in_preamble_before_the_action() {
        // Finding 1: thinking text mentioning a tool must NOT mis-trigger; we anchor on the
        // action object (here via the ```action fence) and read the REAL done.reply.
        let mut x = ReplyStreamExtractor::new();
        let input = "I'll use the \"tool\":\"read\" idea first.\n```action\n{\"tool\":\"done\",\"reply\":\"Hello\"}";
        assert_eq!(x.feed(input), Some("Hello".to_string()));
    }

    #[test]
    fn nothing_before_any_action_is_none() {
        let mut x = ReplyStreamExtractor::new();
        assert_eq!(x.feed(""), None);
        assert_eq!(x.feed("let me think about this first"), None);
    }
}
