//! Phase D: normalize Codex CLI `app-server` JSON-RPC notifications (NDJSON over stdout) into our
//! activity bridge JSONL lines, so a CLOUD Codex orchestrator drives the SAME planner Stage as the
//! local/Claude paths. Mirrors `cloud_claude::ClaudeNormalizer`.
//!
//! ⚠️ Event field names are from the documented app-server protocol (the owner has no Codex CLI to
//! capture real output) — they are SYNTHETIC and must be validated against real `codex app-server`
//! output in e2e. The parser is deliberately tolerant (tries candidate field names) so small
//! schema differences degrade gracefully instead of dropping events.
//!
//! Notifications handled (`{"method":..,"params":..}`):
//!   turn/started                         → a new assistant turn (bumps the chat seq)
//!   item/agentMessage/delta {delta}      → a token  → chat-delta (cumulative)
//!   item/completed {item:{type,..}}      → agentMessage→chat ; webSearch→websearch ; command/file→milestone
//!   *approval* (serverRequest/approval…) → milestone (the duplex client answers the request)
//!
//! Output bridge lines match `mini_activity` (no trailing newline):
//!   {"kind":"chat-delta","seq":N,"text":..} / {"kind":"chat","role":"assistant","text":..}
//!   {"kind":"websearch","query":..,"pages":[{"url":..,"title":..,"summary":""}]}
//!   {"kind":"milestone","text":..,"node":"dot"}

/// Stateful normalizer for ONE Codex app-server session.
pub struct CodexNormalizer {
    seq: u64,
    cur_text: String,
}

impl CodexNormalizer {
    pub fn new(base_seq: u64) -> Self {
        Self {
            seq: base_seq,
            cur_text: String::new(),
        }
    }

    /// Feed ONE NDJSON notification line; return 0+ bridge JSONL lines (no trailing newline).
    /// Total + panic-free: malformed/irrelevant lines yield an empty Vec.
    pub fn feed_line(&mut self, line: &str) -> Vec<String> {
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let method = value.get("method").and_then(|v| v.as_str()).unwrap_or("");
        match method {
            "turn/started" => {
                self.seq += 1;
                self.cur_text.clear();
                Vec::new()
            }
            "item/agentMessage/delta" => {
                let delta = value
                    .get("params")
                    .and_then(|p| p.get("delta"))
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                if delta.is_empty() {
                    return Vec::new();
                }
                self.cur_text.push_str(delta);
                vec![serde_json::json!({"kind":"chat-delta","seq":self.seq,"text":self.cur_text}).to_string()]
            }
            "item/completed" => {
                let item = match value.get("params").and_then(|p| p.get("item")) {
                    Some(i) => i,
                    None => return Vec::new(),
                };
                let itype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match itype {
                    "agentMessage" => {
                        let text = str_field(item, &["text", "message"]);
                        if text.is_empty() {
                            return Vec::new();
                        }
                        vec![serde_json::json!({"kind":"chat","role":"assistant","text":text}).to_string()]
                    }
                    "webSearch" | "web_search" => {
                        let query = str_field(item, &["query"]);
                        let pages_val = item
                            .get("results")
                            .or_else(|| item.get("pages"))
                            .unwrap_or(&serde_json::Value::Null);
                        let mut pages = Vec::new();
                        if let Some(arr) = pages_val.as_array() {
                            for entry in arr.iter().take(6) {
                                let url = str_field(entry, &["url", "link"]);
                                if url.is_empty() {
                                    continue;
                                }
                                let title = str_field(entry, &["title"]);
                                pages.push(serde_json::json!({"url": url, "title": title, "summary": ""}));
                            }
                        }
                        vec![serde_json::json!({"kind":"websearch","query":query,"pages":pages}).to_string()]
                    }
                    "commandExecution" | "command" => {
                        let cmd = str_field(item, &["command", "cmd"]);
                        let label = format!("$ {}", truncate_chars(cmd, 70));
                        vec![serde_json::json!({"kind":"milestone","text":label,"node":"dot"}).to_string()]
                    }
                    "fileChange" | "fileModification" | "patch" => {
                        let path = str_field(item, &["path", "file"]);
                        let label = truncate_chars(path, 70);
                        vec![serde_json::json!({"kind":"milestone","text":label,"node":"dot"}).to_string()]
                    }
                    _ => Vec::new(),
                }
            }
            m if m.to_lowercase().contains("approval") => {
                let params = value.get("params").unwrap_or(&serde_json::Value::Null);
                let detail = str_field(params, &["command", "cmd", "call"]);
                let label = if detail.is_empty() {
                    "⚠ approval requested".to_string()
                } else {
                    format!("⚠ approval requested: {}", truncate_chars(detail, 70))
                };
                vec![serde_json::json!({"kind":"milestone","text":label,"node":"terra"}).to_string()]
            }
            _ => Vec::new(),
        }
    }
}

fn str_field<'a>(v: &'a serde_json::Value, keys: &[&str]) -> &'a str {
    for &k in keys {
        if let Some(s) = v.get(k).and_then(|val| val.as_str()) {
            if !s.is_empty() {
                return s;
            }
        }
    }
    ""
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn one(v: &[String]) -> Value {
        assert_eq!(v.len(), 1, "expected exactly one bridge line, got {v:?}");
        serde_json::from_str(&v[0]).expect("valid JSON")
    }

    #[test]
    fn agent_message_deltas_accumulate_into_cumulative_chat_delta() {
        let mut n = CodexNormalizer::new(0);
        assert!(n.feed_line(r#"{"method":"turn/started","params":{}}"#).is_empty());
        let a = one(&n.feed_line(r#"{"method":"item/agentMessage/delta","params":{"delta":"Ciao"}}"#));
        assert_eq!(a["kind"], "chat-delta");
        assert_eq!(a["seq"], 1);
        assert_eq!(a["text"], "Ciao");
        let b = one(&n.feed_line(r#"{"method":"item/agentMessage/delta","params":{"delta":", come va"}}"#));
        assert_eq!(b["text"], "Ciao, come va");
        assert_eq!(b["seq"], 1);
    }

    #[test]
    fn completed_agent_message_finalizes_chat() {
        let mut n = CodexNormalizer::new(0);
        let v = one(&n.feed_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","text":"Tutto bene."}}}"#,
        ));
        assert_eq!(v["kind"], "chat");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["text"], "Tutto bene.");
    }

    #[test]
    fn completed_websearch_item_emits_websearch_with_pages() {
        let mut n = CodexNormalizer::new(0);
        let line = r#"{"method":"item/completed","params":{"item":{"type":"webSearch","query":"rust version","results":[{"url":"https://releases.rs/","title":"Rust Versions"},{"url":"https://blog.rust-lang.org/","title":"Blog"}]}}}"#;
        let v = one(&n.feed_line(line));
        assert_eq!(v["kind"], "websearch");
        assert_eq!(v["query"], "rust version");
        let pages = v["pages"].as_array().expect("pages");
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["url"], "https://releases.rs/");
        assert_eq!(pages[0]["title"], "Rust Versions");
    }

    #[test]
    fn completed_command_execution_emits_milestone() {
        let mut n = CodexNormalizer::new(0);
        let v = one(&n.feed_line(
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","command":"cargo build"}}}"#,
        ));
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["node"], "dot");
        assert!(v["text"].as_str().unwrap().contains("cargo build"));
    }

    #[test]
    fn approval_request_surfaces_a_milestone() {
        let mut n = CodexNormalizer::new(0);
        let v = one(&n.feed_line(
            r#"{"method":"serverRequest/approval","id":7,"params":{"command":"rm -rf x"}}"#,
        ));
        assert_eq!(v["kind"], "milestone");
        assert!(
            v["text"].as_str().unwrap().to_lowercase().contains("approv"),
            "names it an approval: {}",
            v["text"]
        );
    }

    #[test]
    fn irrelevant_notifications_and_non_json_are_ignored() {
        let mut n = CodexNormalizer::new(0);
        assert!(n.feed_line(r#"{"method":"thread/status/changed","params":{}}"#).is_empty());
        assert!(n.feed_line(r#"{"method":"turn/completed","params":{}}"#).is_empty());
        assert!(n.feed_line("not json").is_empty());
    }
}
