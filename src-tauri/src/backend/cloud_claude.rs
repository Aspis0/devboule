//! Phase D: normalize Claude Code CLI `--output-format stream-json` events into our activity
//! bridge JSONL lines, so a CLOUD Claude orchestrator drives the SAME planner Stage (chat /
//! token-streaming / websearch) as the local devboule orchestrator. The duplex client feeds each
//! NDJSON line here; the returned bridge lines are appended to the agent's activity file, which
//! `start_activity_tail` already parses into the Console/Stage.
//!
//! Event shapes are taken from REAL captured output (src/backend/testdata/claude_stream_*.jsonl):
//!   - `{"type":"stream_event","event":{"type":"message_start",...}}`         → a new assistant turn
//!   - `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"…"}}}` → a token
//!   - `{"type":"assistant","message":{"content":[{"type":"text","text":"…"},{"type":"tool_use","name":"WebSearch","input":{"query":"…"}}]}}`
//!   - `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"…","content":"Web search results …\n\nLinks: [{\"title\":…,\"url\":…}]"}]}}`
//!
//! Output bridge lines (no trailing newline; the writer appends it), matching `mini_activity`:
//!   chat-delta: {"kind":"chat-delta","seq":N,"text":"<cumulative>"}
//!   chat:       {"kind":"chat","role":"assistant","text":"…"}
//!   websearch:  {"kind":"websearch","query":"…","pages":[{"url":"…","title":"…","summary":""}]}
//!   milestone:  {"kind":"milestone","text":"…","node":"dot"}

use std::collections::HashMap;

use super::doubt_sensor_text;

/// Stateful normalizer for ONE Claude stream-json session. `new(base_seq)` seeds the assistant
/// turn counter (so it never collides with deltas the local path may have emitted on the same
/// agent — in practice base 0).
pub struct ClaudeNormalizer {
    seq: u64,
    cur_text: String,
    /// tool_use_id -> the web-search query, pending its tool_result.
    pending_ws: HashMap<String, String>,
    /// tool_use_id -> tool name, for non-websearch tool_result echo.
    pending_tools: HashMap<String, String>,
    /// Kairion: the (summarized) reasoning trace accumulated from `thinking_delta`s since the
    /// last `message_start`. Fed to the text doubt sensor when an assistant question turn fires.
    trace: String,
}

impl ClaudeNormalizer {
    pub fn new(base_seq: u64) -> Self {
        Self {
            seq: base_seq,
            cur_text: String::new(),
            pending_ws: HashMap::new(),
            pending_tools: HashMap::new(),
            trace: String::new(),
        }
    }

    /// The reasoning trace accumulated for the current turn (Kairion; mostly for tests).
    pub fn trace(&self) -> &str {
        &self.trace
    }

    /// Feed ONE NDJSON line; return 0+ complete bridge JSONL lines (no trailing newline). Pure
    /// w.r.t. I/O and total: a malformed/irrelevant line yields an empty Vec, never panics.
    pub fn feed_line(&mut self, line: &str) -> Vec<String> {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return vec![],
        };
        let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "stream_event" => {
                if let Some(ev) = value.get("event") {
                    let ev_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match ev_type {
                        "message_start" => {
                            self.seq += 1;
                            self.cur_text.clear();
                            // A new turn: drop any web-search queries from the prior turn that never
                            // got a tool_result, so `pending_ws` can't leak across the session.
                            self.pending_ws.clear();
                            // Same for non-websearch tool results.
                            self.pending_tools.clear();
                            // Kairion: the reasoning trace is per-turn — start fresh so this turn's
                            // doubt is computed only from THIS turn's thinking.
                            self.trace.clear();
                            return vec![];
                        }
                        "content_block_delta" => {
                            if let Some(delta) = ev.get("delta") {
                                let dt = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                if dt == "text_delta" {
                                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                        self.cur_text.push_str(text);
                                        return vec![serde_json::json!({"kind":"chat-delta","seq":self.seq,"text":self.cur_text}).to_string()];
                                    }
                                } else if dt == "thinking_delta" {
                                    // Kairion: EXTRACT the (summarized) reasoning trace. We emit no
                                    // bridge line for it (the trace is internal to doubt sensing);
                                    // it is consumed when an assistant question turn fires.
                                    if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                                        self.trace.push_str(t);
                                    }
                                }
                            }
                            return vec![];
                        }
                        _ => {}
                    }
                }
                vec![]
            }
            "assistant" => {
                // This finalizes the turn; reset the streaming accumulator so a second
                // consecutive `assistant` event without an intervening `message_start` (resumed
                // session / injected context) can't prepend the prior turn's text to the next.
                self.cur_text.clear();
                let mut lines = vec![];
                if let Some(content_arr) =
                    value.get("message").and_then(|m| m.get("content")).and_then(|v| v.as_array())
                {
                    for block in content_arr {
                        let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if bt == "text" {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    // Kairion (orchestrator-only): if this turn carries a
                                    // KAIRION_QUESTION marker, assemble + emit the frozen question
                                    // wire line (doubt computed from the accumulated trace) INSTEAD
                                    // of a plain chat bubble. No marker → byte-identical chat turn.
                                    if let Some((preamble, q)) =
                                        doubt_sensor_text::parse_question_marker_with_preamble(text)
                                    {
                                        // Any prose BEFORE the marker line is real reasoning — emit
                                        // it as its own chat bubble first, then the question line,
                                        // so a "preamble\nKAIRION_QUESTION {…}" turn drops nothing.
                                        if let Some(pre) = preamble {
                                            lines.push(serde_json::json!({"kind":"chat","role":"assistant","text":pre}).to_string());
                                        }
                                        lines.push(doubt_sensor_text::build_question_line(
                                            &self.trace,
                                            &q,
                                        ));
                                    } else {
                                        lines.push(serde_json::json!({"kind":"chat","role":"assistant","text":text}).to_string());
                                    }
                                }
                            }
                        } else if bt == "thinking" {
                            // Emit a collapsed thinking row for the activity tail.
                            if let Some(thinking) = block.get("thinking").and_then(|v| v.as_str()) {
                                if !thinking.is_empty() {
                                    // Cap at 2000 chars so the tail's MAX_LINE_BYTES=8192 gate
                                    // doesn't drop the line before its own visual truncation.
                                    lines.push(serde_json::json!({"kind":"thinking","text":truncate_summary(thinking, 2000)}).to_string());
                                }
                            }
                        } else if bt == "tool_use" {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            if name == "WebSearch" || name == "web_search" {
                                if let Some(query) = block
                                    .get("input")
                                    .and_then(|v| v.get("query"))
                                    .and_then(|v| v.as_str())
                                {
                                    self.pending_ws.insert(id.to_string(), query.to_string());
                                }
                            } else {
                                let mut label = name.to_string();
                                if let Some(inp) = block.get("input").and_then(|v| v.as_object()) {
                                    if let Some(val) = inp
                                        .get("file_path")
                                        .or_else(|| inp.get("path"))
                                        .or_else(|| inp.get("command"))
                                        .and_then(|v| v.as_str())
                                    {
                                        // char-safe truncate (a path/command may be multibyte).
                                        let short: String = val.chars().take(60).collect();
                                        label.push(' ');
                                        label.push_str(&short);
                                    }
                                }
                                lines.push(serde_json::json!({"kind":"milestone","text":label,"node":"dot"}).to_string());
                                // Track for tool_result echo (skip empty ids — some tools produce none).
                                if !id.is_empty() {
                                    self.pending_tools.insert(id.to_string(), name.to_string());
                                }
                            }
                        }
                    }
                }
                lines
            }
            "user" => {
                let mut lines = vec![];
                if let Some(content_arr) =
                    value.get("message").and_then(|m| m.get("content")).and_then(|v| v.as_array())
                {
                    for block in content_arr {
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                            let id = block.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                            // 1) Websearch path — already handled by pending_ws.
                            if let Some(query) = self.pending_ws.remove(id) {
                                let content_str = tool_result_text(block);
                                let pages: Vec<serde_json::Value> = extract_links(&content_str)
                                    .into_iter()
                                    .map(|(u, t)| serde_json::json!({"url":u,"title":t,"summary":""}))
                                    .collect();
                                lines.push(serde_json::json!({"kind":"websearch","query":query,"pages":pages}).to_string());
                            // 2) Non-websearch tool_result echo — check pending_tools.
                            } else if let Some(tool_name) = self.pending_tools.remove(id) {
                                let content_str = tool_result_text(block);
                                let is_error = block
                                    .get("is_error")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                let emoji = if is_error { "❌" } else { "✅" };
                                // Cap tool name at 60 chars (the dot-milestone already does this).
                                let safe_name = truncate_summary(&tool_name, 60);
                                let summary = if content_str.is_empty() {
                                    "(no result)".to_string()
                                } else {
                                    truncate_summary(&content_str, 200)
                                };
                                lines.push(serde_json::json!({
                                    "kind": "milestone",
                                    "text": format!("{} {} → {}", emoji, safe_name, summary),
                                    "node": "sage",
                                }).to_string());
                            }
                        }
                    }
                }
                lines
            }
            "result" => {
                // Final turn-summary banner: {"type":"result","subtype":"success",...}
                let subtype = value
                    .get("subtype")
                    .and_then(|v| v.as_str())
                    .unwrap_or("error")
                    .to_string();
                let is_error = value.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                // A result is "failed" if is_error is true OR subtype is present but not "success"
                // (e.g. subtype="error_max_turns" with no is_error flag).
                let failed = is_error || (!subtype.is_empty() && subtype != "success");
                if failed {
                    // Error banner.
                    let reason = value
                        .get("result")
                        .and_then(|v| v.as_str())
                        .map(|s| truncate_summary(s, 160))
                        .unwrap_or_default();
                    let safe_subtype = truncate_summary(&subtype, 40);
                    let text = if reason.is_empty() {
                        format!("Turn failed ({safe_subtype})")
                    } else {
                        format!("Turn failed ({safe_subtype}) — {reason}")
                    };
                    vec![serde_json::json!({"kind":"banner","text":text}).to_string()]
                } else {
                    // Success banner.
                    let mut parts: Vec<String> = Vec::new();
                    if let Some(num_turns) = value.get("num_turns").and_then(|v| v.as_u64()) {
                        parts.push(format!("{num_turns} turns"));
                    }
                    if let Some(duration_ms) = value.get("duration_ms").and_then(|v| v.as_u64()) {
                        let secs = duration_ms as f64 / 1000.0;
                        parts.push(format!("{secs:.1}s"));
                    }
                    if let Some(cost) = value.get("total_cost_usd").and_then(|v| v.as_f64()) {
                        parts.push(format!("${cost:.4}"));
                    }
                    let text = if parts.is_empty() {
                        "Turn done".to_string()
                    } else {
                        format!("Turn done — {}", parts.join(" · "))
                    };
                    vec![serde_json::json!({"kind":"banner","text":text}).to_string()]
                }
            }
            _ => vec![],
        }
    }
}

/// Extract the result text from a tool_result block (string content or array of
/// `{"type":"text","text":"…"}` objects). Used by both websearch and non-websearch
/// tool_result paths.
fn tool_result_text(block: &serde_json::Value) -> String {
    match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        // Future API shape: an array of content blocks — accept bare strings OR
        // `{"type":"text","text":"…"}` objects.
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| {
                v.as_str().map(str::to_string).or_else(|| {
                    v.get("text").and_then(|t| t.as_str()).map(str::to_string)
                })
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Char-cap a string to `max` chars, appending '…' only when truncated.
fn truncate_summary(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}


/// Parse the `Links: [{"title":…,"url":…}, …]` JSON array embedded in a Claude WebSearch
/// tool_result's text content into `(url, title)` pairs (capped to 6). Tolerant: any failure
/// (no `Links:`, truncated array, a `]` inside a title) yields an empty list → a query-only
/// websearch event rather than a panic.
fn extract_links(content: &str) -> Vec<(String, String)> {
    let mut links = Vec::new();
    let Some(start) = content.find("Links:") else {
        return links;
    };
    let after = &content[start..];
    let Some(open) = after.find('[') else {
        return links;
    };
    // Parse the FIRST complete JSON value starting at '[' via a streaming deserializer, so a ']'
    // inside any title/url string can't truncate the array early (serde respects the grammar).
    let mut stream = serde_json::Deserializer::from_str(&after[open..]).into_iter::<serde_json::Value>();
    if let Some(Ok(serde_json::Value::Array(arr))) = stream.next() {
        for item in arr.iter().take(6) {
            let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !url.is_empty() {
                links.push((url, title));
            }
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn one(v: &[String]) -> Value {
        assert_eq!(v.len(), 1, "expected exactly one bridge line, got {v:?}");
        serde_json::from_str(&v[0]).expect("bridge line is valid JSON")
    }

    #[test]
    fn text_deltas_accumulate_into_a_cumulative_chat_delta() {
        let mut n = ClaudeNormalizer::new(0);
        // a new assistant turn
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}"#)
            .is_empty());
        let a = one(&n.feed_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Ciao,"}}}"#,
        ));
        assert_eq!(a["kind"], "chat-delta");
        assert_eq!(a["seq"], 1);
        assert_eq!(a["text"], "Ciao,");
        // the second delta is CUMULATIVE
        let b = one(&n.feed_line(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" sto pianificando."}}}"#,
        ));
        assert_eq!(b["text"], "Ciao, sto pianificando.");
        assert_eq!(b["seq"], 1);
    }

    #[test]
    fn thinking_deltas_are_extracted_into_the_trace_input_json_ignored() {
        // Kairion: a `thinking_delta` no longer just drops — its text accumulates into the
        // reasoning trace (still emitting NO bridge line). `input_json_delta` stays ignored.
        let mut n = ClaudeNormalizer::new(0);
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm, maybe "}}}"#)
            .is_empty());
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Postgres"}}}"#)
            .is_empty());
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q"}}}"#)
            .is_empty());
        // the thinking was EXTRACTED, the input_json was NOT
        assert_eq!(n.trace(), "hmm, maybe Postgres");
    }

    #[test]
    fn assistant_question_marker_emits_the_frozen_question_line_with_doubt() {
        let mut n = ClaudeNormalizer::new(0);
        // a new turn + a torn reasoning trace
        n.feed_line(r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}"#);
        n.feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"maybe Postgres, but perhaps MySQL. however Postgres. not sure."}}}"#);
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"KAIRION_QUESTION {\"id\":\"q1\",\"text\":\"Which DB?\",\"options\":[{\"id\":\"pg\",\"label\":\"Postgres\"},{\"id\":\"my\",\"label\":\"MySQL\"}],\"affects\":[\"schema.rs\"]}"}]}}"#;
        let out = n.feed_line(line);
        let v = one(&out);
        assert_eq!(v["kind"], "question");
        assert_eq!(v["id"], "q1");
        assert_eq!(v["text"], "Which DB?");
        assert_eq!(v["status"], "open");
        assert_eq!(v["options"][0]["label"], "Postgres");
        assert_eq!(v["affects"][0], "schema.rs");
        assert!(v["unrest"].as_f64().unwrap() > 0.0, "torn trace → some unrest");
        assert!(v["directionConfidence"].as_f64().unwrap() <= 0.4, "text-only → low");
        // a question turn must NOT also emit a plain chat bubble
        assert!(out.iter().all(|l| !l.contains("\"kind\":\"chat\"")));
    }

    #[test]
    fn assistant_preamble_before_marker_emits_chat_then_question() {
        // A turn that writes reasoning prose BEFORE the KAIRION_QUESTION marker must NOT drop that
        // preamble: it surfaces as a chat bubble first, then the question line.
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}"#);
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Here is my reasoning so far.\nKAIRION_QUESTION {\"id\":\"q1\",\"text\":\"Which DB?\",\"options\":[{\"id\":\"pg\",\"label\":\"Postgres\"}],\"affects\":[]}"}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 2, "preamble chat + question, got {out:?}");
        let chat: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(chat["kind"], "chat");
        assert_eq!(chat["role"], "assistant");
        assert_eq!(chat["text"], "Here is my reasoning so far.");
        let q: Value = serde_json::from_str(&out[1]).unwrap();
        assert_eq!(q["kind"], "question");
        assert_eq!(q["id"], "q1");
        assert_eq!(q["text"], "Which DB?");
    }

    #[test]
    fn assistant_text_block_finalizes_a_chat_turn() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"1.96.0\n\nSources: ok"}]}}"#;
        let v = one(&n.feed_line(line));
        assert_eq!(v["kind"], "chat");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["text"], "1.96.0\n\nSources: ok");
    }

    #[test]
    fn assistant_with_only_tool_use_emits_no_empty_chat() {
        let mut n = ClaudeNormalizer::new(0);
        // a WebSearch tool_use alone must NOT produce a chat turn (it stores the query)
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"WebSearch","input":{"query":"rust version"}}]}}"#;
        let out = n.feed_line(line);
        assert!(
            out.iter().all(|l| !l.contains("\"kind\":\"chat\"")),
            "no chat turn for a tool-only assistant message: {out:?}"
        );
    }

    #[test]
    fn websearch_tool_use_then_tool_result_emits_a_websearch_event_with_pages() {
        let mut n = ClaudeNormalizer::new(0);
        // 1) the WebSearch tool_use records the query
        n.feed_line(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_X","name":"WebSearch","input":{"query":"latest rust"}}]}}"#);
        // 2) the matching tool_result carries the "Links: [...]" results
        let line = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_X","type":"tool_result","content":"Web search results for query: \"latest rust\"\n\nLinks: [{\"title\":\"Rust Versions\",\"url\":\"https://releases.rs/\"},{\"title\":\"Announcing Rust 1.96.0\",\"url\":\"https://blog.rust-lang.org/releases/latest/\"}]\n\nmore text"}]}}"#;
        let v = one(&n.feed_line(line));
        assert_eq!(v["kind"], "websearch");
        assert_eq!(v["query"], "latest rust");
        let pages = v["pages"].as_array().expect("pages array");
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["url"], "https://releases.rs/");
        assert_eq!(pages[0]["title"], "Rust Versions");
        assert_eq!(pages[1]["url"], "https://blog.rust-lang.org/releases/latest/");
    }

    #[test]
    fn websearch_links_with_a_bracket_in_a_title_still_parse() {
        // F2: a ']' inside a title must NOT truncate the Links array (serde grammar, not first ']').
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"WebSearch","input":{"query":"q"}}]}}"#);
        let line = r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"Links: [{\"title\":\"arr[0] access\",\"url\":\"https://a.test/\"},{\"title\":\"B\",\"url\":\"https://b.test/\"}]"}]}}"#;
        let v = one(&n.feed_line(line));
        assert_eq!(v["kind"], "websearch");
        let pages = v["pages"].as_array().expect("pages");
        assert_eq!(pages.len(), 2, "the ']' in the first title didn't cut the array");
        assert_eq!(pages[0]["title"], "arr[0] access");
        assert_eq!(pages[1]["url"], "https://b.test/");
    }

    #[test]
    fn non_websearch_tool_use_emits_a_milestone() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"/x/README.md"}}]}}"#;
        let v = one(&n.feed_line(line));
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["node"], "dot");
        assert!(
            v["text"].as_str().unwrap().contains("Read"),
            "milestone names the tool: {}",
            v["text"]
        );
    }

    #[test]
    fn replays_the_real_captured_websearch_session() {
        // End-to-end against REAL captured `claude --output-format stream-json` output: the whole
        // session must normalize into a sane bridge stream — a websearch with pages, growing
        // chat-deltas, and at least one finalized chat turn — without panicking on any line.
        let fixture = include_str!("testdata/claude_stream_websearch.jsonl");
        let mut n = ClaudeNormalizer::new(0);
        let mut kinds: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut ws_with_pages = 0usize;
        for line in fixture.lines() {
            if line.trim().is_empty() {
                continue;
            }
            for out in n.feed_line(line) {
                let v: Value = serde_json::from_str(&out).expect("emitted a valid bridge line");
                let kind = v["kind"].as_str().unwrap_or("?").to_string();
                if kind == "websearch" && v["pages"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                    ws_with_pages += 1;
                }
                *kinds.entry(kind).or_insert(0) += 1;
            }
        }
        assert!(ws_with_pages >= 1, "≥1 websearch with real pages; kinds={kinds:?}");
        assert!(kinds.get("chat-delta").copied().unwrap_or(0) >= 1, "token deltas streamed");
        assert!(kinds.get("chat").copied().unwrap_or(0) >= 1, "≥1 finalized chat turn");
    }

    #[test]
    fn system_and_ratelimit_lines_are_ignored() {
        let mut n = ClaudeNormalizer::new(0);
        assert!(n.feed_line(r#"{"type":"system","subtype":"init","session_id":"x"}"#).is_empty());
        assert!(n.feed_line(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#).is_empty());
        assert!(n.feed_line("not json at all").is_empty());
    }

    // --- Part 1: tool-result echo ---

    #[test]
    fn non_websearch_tool_result_echoes_checkmark_milestone() {
        let mut n = ClaudeNormalizer::new(0);
        // tool_use for a non-websearch tool
        n.feed_line(r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"/x/README.md"}}]}}"#);
        // matching tool_result → ✅ Read → (no result)
        let line = r#"{"type":"user","message":{"content":[{"tool_use_id":"toolu_2","type":"tool_result","content":"File not found"}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["node"], "sage");
        assert!(v["text"].as_str().unwrap().starts_with("✅ Read →"));
        assert_eq!(v["text"], "✅ Read → File not found");
    }

    #[test]
    fn non_websearch_tool_result_with_is_error_emits_cross() {
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"false"}}]}}"#);
        let line = r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"exit 1","is_error":true}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["text"].as_str().unwrap(), "❌ Bash → exit 1");
    }

    #[test]
    fn non_websearch_tool_result_array_content_is_extracted() {
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"path":"/x/f.py"}}]}}"#);
        // content as array of text objects (same path used by websearch factoring)
        let line = r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":[{"type":"text","text":"File edited"}]}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["text"], "✅ Edit → File edited");
    }

    #[test]
    fn non_websearch_tool_result_empty_content_yields_no_result() {
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/x/f.txt"}}]}}"#);
        let line = r#"{"type":"user","message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":""}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["text"], "✅ Read → (no result)");
    }

    #[test]
    fn non_websearch_tool_summary_multibyte_truncated() {
        let mut n = ClaudeNormalizer::new(0);
        n.feed_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"echo hello"}}]}}"#);
        // 210 chars of content — char-capped at 200 + '…'
        let long_result: String = "🦀".repeat(210);
        let line = format!(r#"{{"type":"user","message":{{"content":[{{"tool_use_id":"t1","type":"tool_result","content":"{}"}}]}}}}"#, long_result);
        let out = n.feed_line(&line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        // char-capped at 200 + '…'
        let text = v["text"].as_str().unwrap();
        assert!(text.starts_with("✅ Bash → 🦀"));
        assert!(text.ends_with('…'));
        // '✅ Bash → ' (9 chars) + 200 truncated + '…' (1 char) = 210
        assert!(text.chars().count() == 210);
    }

    // --- Part 2: result summary banner ---

    #[test]
    fn result_success_emits_banner_with_turns_duration_cost() {
        let mut n = ClaudeNormalizer::new(0);
        // REAL captured line from claude_stream_text.jsonl
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":5860,"num_turns":1,"result":"Ciao, sto pianificando.","total_cost_usd":0.1512285}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "banner");
        assert_eq!(v["text"], "Turn done — 1 turns · 5.9s · $0.1512");
    }

    #[test]
    fn result_error_emits_banner_with_subtype_and_reason() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"result","subtype":"api_error","is_error":true,"result":"Rate limit exceeded"}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "banner");
        assert_eq!(v["text"], "Turn failed (api_error) — Rate limit exceeded");
    }

    #[test]
    fn result_error_missing_subtype_defaults_to_error() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"result","is_error":true,"result":"boom"}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["text"], "Turn failed (error) — boom");
    }

    #[test]
    fn result_subtype_error_max_turns_emits_failure_without_is_error() {
        let mut n = ClaudeNormalizer::new(0);
        // No is_error flag, but subtype="error_max_turns" → still a failure.
        let line = r#"{"type":"result","subtype":"error_max_turns","duration_ms":90000,"num_turns":10,"result":"Maximum turns reached"}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "banner");
        assert_eq!(v["text"], "Turn failed (error_max_turns) — Maximum turns reached");
    }

    #[test]
    fn result_error_reason_truncated_at_160_chars() {
        let mut n = ClaudeNormalizer::new(0);
        let long_reason: String = "x".repeat(200);
        let line = format!(r#"{{"type":"result","is_error":true,"result":"{}"}}"#, long_reason);
        let out = n.feed_line(&line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        let text = v["text"].as_str().unwrap();
        assert!(text.starts_with("Turn failed (error) — "));
        // the reason part is capped at 160 chars + '…'
        let reason_part = &text["Turn failed (error) — ".len()..];
        assert!(reason_part.ends_with('…'));
        assert!(reason_part.chars().count() <= 161);
    }

    #[test]
    fn result_success_without_cost_omits_cost_segment() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":3000,"num_turns":2}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["text"], "Turn done — 2 turns · 3.0s");
    }

    // --- Part 3: thinking rows ---

    #[test]
    fn assistant_with_thinking_block_emits_thinking_then_chat() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me think about this."},{"type":"text","text":"Here is the answer."}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 2, "got {out:?}");
        let thinking = serde_json::from_str::<Value>(&out[0]).unwrap();
        let chat = serde_json::from_str::<Value>(&out[1]).unwrap();
        assert_eq!(thinking["kind"], "thinking");
        assert_eq!(thinking["text"], "Let me think about this.");
        assert_eq!(chat["kind"], "chat");
        assert_eq!(chat["text"], "Here is the answer.");
    }

    #[test]
    fn thinking_trace_accumulation_still_works() {
        // Existing test must stay green — thinking_delta still accumulates into trace
        let mut n = ClaudeNormalizer::new(0);
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm, maybe "}}}"#)
            .is_empty());
        assert!(n
            .feed_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Postgres"}}}"#)
            .is_empty());
        assert_eq!(n.trace(), "hmm, maybe Postgres");
    }

    #[test]
    fn thinking_block_empty_string_is_skipped() {
        let mut n = ClaudeNormalizer::new(0);
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hello"}]}}"#;
        let out = n.feed_line(line);
        assert_eq!(out.len(), 1, "got {out:?}");
        let v = one(&out);
        assert_eq!(v["kind"], "chat");
    }

    #[test]
    fn thinking_block_capped_at_2000_chars_in_normalizer() {
        let mut n = ClaudeNormalizer::new(0);
        let long_thinking: String = "x".repeat(3000);
        let line = format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"thinking","thinking":"{}"}},{{"type":"text","text":"Hi"}}]}}}}"#, long_thinking);
        let out = n.feed_line(&line);
        assert_eq!(out.len(), 2, "got {out:?}");
        let v = serde_json::from_str::<Value>(&out[0]).unwrap();
        assert_eq!(v["kind"], "thinking");
        // Normalizer caps at 2000 chars + '…' so the tail's MAX_LINE_BYTES=8192 gate
        // doesn't drop the line.
        let text = v["text"].as_str().unwrap();
        assert!(text.ends_with('…'));
        assert_eq!(text.chars().count(), 2001); // 2000 + '…'
    }
}
