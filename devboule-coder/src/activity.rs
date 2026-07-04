//! Orchestrator → Console FILE BRIDGE (the writer half).
//!
//! The orchestrator runs as a SEPARATE process with a ratatui TUI on its PTY, so it
//! CANNOT print activity markers to stdout — that would corrupt the TUI. Instead it
//! APPENDS one tiny JSON event per line to the path in `DEVBOULE_ACTIVITY_FILE`. The
//! Tauri host (`src-tauri/src/backend/mini_activity.rs` + the launch tail task in
//! `projects.rs`) tails that file, parses each line, and turns it into a coder-tier
//! `CoderEntry` milestone in the live Activity Console for this launch's `agent_id`.
//!
//! BEST-EFFORT BY DESIGN: this is pure observability. If `DEVBOULE_ACTIVITY_FILE` is
//! unset, or the file is unwritable, or a write fails for ANY reason, we SILENTLY
//! no-op. Emitting a milestone must NEVER break, slow, or fail the orchestrator run.
//! There is no buffering and no background thread: each `milestone` opens the file in
//! append mode, writes one line, and closes it (append is atomic for the small,
//! single-line writes we do on a local fs — the host tail reads only whole lines).
//!
//! PRIVACY: a `milestone` event carries only a short, redacted LABEL (`text`) + a node
//! style — NEVER a raw transcript, file body, token, or secret. The planner / burst pick
//! the label; the host surfaces it verbatim. Keep every label a path basename + a verb.
//!
//! The `websearch` event is the ONE intentional exception: it carries the search query +
//! the PUBLIC web pages the orchestrator read (url + title + a capped ≤400-char summary),
//! because the whole point is to show the user the real sources feeding their plan. This
//! is public web content surfaced to the same user, in their own local app — not a secret.
//! (Edge: if the orchestrator ever fetched an AUTHENTICATED page, its summary would land
//! here too; the egress backend only does public Exa search/crawl today, so that path is
//! not reachable — revisit if authenticated fetch is ever added.)

use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

use crate::action::QOption;
use crate::doubt_sensor::{Candidate, DoubtSignal};
use crate::executor::ExaPage;

/// The env var the Tauri host sets at orchestrator launch to the per-agent activity
/// file path. Unset (the headless / standalone case) ⇒ every milestone no-ops.
const ENV_ACTIVITY_FILE: &str = "DEVBOULE_ACTIVITY_FILE";

/// Hard cap on a single milestone's `text` (chars, not bytes). The host also bounds
/// the line it reads back; capping here keeps the on-disk line small and the wire
/// payload tiny. A label longer than this is char-truncated (never split mid-codepoint).
const MAX_TEXT_CHARS: usize = 200;

/// Hard cap on a `chat` turn's `text` (chars). Much larger than [`MAX_TEXT_CHARS`]: a
/// milestone is a basename+verb LABEL, but a chat message is the orchestrator's prose
/// reply (a plan summary, an answer) — 200 would truncate it mid-sentence.
const MAX_CHAT_TEXT_CHARS: usize = 2000;

/// The timeline node style for a milestone — mirrors the host's `NodeStyle` /
/// the frontend `ConsoleEntry["node"]` union (`"" | "dot" | "sage" | "terra"`).
/// `Hollow` serializes to the empty string. There is NO "coral" node in the wire
/// contract; the terracotta (`Terra`) ring is the warm/warning color, so a
/// rejection milestone uses `Terra` and lets its TEXT carry the "rejected" meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// `""` — hollow teal (a neutral step, e.g. one EXPLORE).
    Hollow,
    /// `"dot"` — filled teal (a completed planner phase).
    Dot,
    /// `"sage"` — sage ring (a positive terminal, e.g. plan approved).
    Sage,
    /// `"terra"` — terracotta ring (a submit/awaiting or a rejection — warm/warning).
    Terra,
}

impl Node {
    /// The exact wire string the host parses into its `NodeStyle` (and the frontend
    /// renders). MUST stay in lockstep with `mini_activity::NodeStyle`'s serde.
    fn as_wire(self) -> &'static str {
        match self {
            Node::Hollow => "",
            Node::Dot => "dot",
            Node::Sage => "sage",
            Node::Terra => "terra",
        }
    }
}

/// The orchestrator-side activity emitter. Cheap to clone (just an `Option<PathBuf>`),
/// `Send + Sync` so it threads through the async planner / burst without ceremony.
/// A `None` path (env unset or blank) makes every `milestone` a no-op.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// The resolved activity-file path, or `None` to disable (the no-op case).
    path: Option<PathBuf>,
}

impl Activity {
    /// Build from the process env. Reads `DEVBOULE_ACTIVITY_FILE`; a missing or
    /// blank value yields a disabled (no-op) emitter. Does NOT touch the disk here
    /// (no create / probe) — the first `milestone` is what opens the file, so a host
    /// that sets the var but never created the file still works (the open creates it).
    pub fn from_env() -> Self {
        let path = std::env::var(ENV_ACTIVITY_FILE)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self { path }
    }

    /// An explicitly-disabled emitter (no path). Used by call sites / tests that
    /// never want to touch a file.
    pub fn disabled() -> Self {
        Self { path: None }
    }

    /// Build pointed at an explicit path (for tests + any non-env caller).
    #[cfg(test)]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    /// Append ONE `milestone` event line for `text` with node style `node`.
    /// BEST-EFFORT: a `None` path or ANY I/O error silently no-ops (never panics,
    /// never returns an error to the caller — observability must not break the run).
    /// The line is a compact JSON object: `{"kind":"milestone","text":"…","node":"…"}`
    /// plus a trailing `\n` so the host tail reads it as one whole line.
    pub fn milestone(&self, text: &str, node: Node) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_milestone(text, node);
        // Open in append mode (create if absent) and write the single line. We
        // deliberately DROP the handle each call: appends are independent and the
        // cost is negligible at planner-phase frequency (a handful per run). A
        // failure at any step is swallowed.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            // One write_all of the whole line (incl. the newline) so a partial write
            // cannot leave a half-line the host would skip; still best-effort.
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// Append ONE `websearch` event: the query + the REAL pages (url/title/summary)
    /// the orchestrator just read, so the host's Websearch view shows live sources +
    /// distilled findings instead of demo content. Same best-effort contract as
    /// [`milestone`] (None path / any I/O error silently no-ops). Pages are already
    /// capped (≤6, summaries ≤400 chars) by `parse_exa_pages`.
    pub fn websearch(&self, query: &str, pages: &[ExaPage]) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_websearch(query, pages);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// Append ONE `chat` turn (a conversational message in the planner chat): `role` is
    /// "assistant" (the orchestrator talking) or "user" (a steer echoed back). Same
    /// best-effort contract as [`milestone`]. This is what makes the panel chat a real
    /// two-way conversation instead of a one-way steer.
    pub fn chat(&self, role: &str, text: &str) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_chat(role, text);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
            // D3: fsync so frontend polling sees user message BEFORE
            // burst output that follows (response-before-question fix).
            let _ = file.sync_all();
        }
    }

    /// Append ONE `chat-delta` event: a CUMULATIVE snapshot of the assistant turn `seq`'s
    /// reply as it streams (B14b). The text is the full reply-so-far, not just the new
    /// chunk, so the host tail can take the latest state and never has to re-order or
    /// re-assemble fragments. Same best-effort contract as [`chat`] (None path / any I/O
    /// error silently no-ops). The host coalesces same-`seq` deltas into one live chat
    /// row and the final [`chat`] turn finalizes it.
    pub fn chat_delta(&self, seq: u64, text: &str) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_chat_delta(seq, text);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// `true` when this emitter is LIVE (a path is configured). The host sets
    /// `DEVBOULE_ACTIVITY_FILE` ONLY at ORCHESTRATOR launch, so a live bridge is the
    /// de-facto orchestrator-role signal the burst loop uses to gate the Kairion
    /// structured-question emission (and to skip its doubt-sensor work entirely for a
    /// plain coder / mini, keeping that path byte-identical). Mirrors the same implicit
    /// gate every other bridge emission already relies on.
    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    /// Append ONE `question` event (Kairion): a structured `ask_user` carrying the
    /// orchestrator's discrete `options` PLUS the doubt sensor's reading of `signal`
    /// (unrest magnitude, per-option candidate pulls, lean + direction confidence).
    /// `id` is a stable per-turn key, `status` is `"open"` (a fresh ask) or
    /// `"reopened"`, and `affects` lists the plan items the answer bears on (may be
    /// empty). Same best-effort contract as [`chat`] (None path / any I/O error
    /// silently no-ops). Mirrors [`chat_delta`]'s shape; the wire line is the FROZEN
    /// question contract built by [`encode_question`].
    pub fn question(
        &self,
        id: &str,
        text: &str,
        options: &[QOption],
        signal: &DoubtSignal,
        status: &str,
        affects: &[String],
    ) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let line = encode_question(id, text, options, signal, status, affects);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// Serialize ONE milestone event to its single-line JSON form, `text` char-capped to
/// [`MAX_TEXT_CHARS`]. Factored out so it is directly unit-testable and so the wire
/// shape lives in one place. Always ends with a single `\n`.
fn encode_milestone(text: &str, node: Node) -> String {
    let capped: String = if text.chars().count() > MAX_TEXT_CHARS {
        text.chars().take(MAX_TEXT_CHARS).collect()
    } else {
        text.to_string()
    };
    // serde_json escapes control chars / quotes / newlines, guaranteeing the value
    // stays on ONE physical line even if a label contained a stray newline — the
    // host tail splits on '\n', so the payload must never embed a raw one.
    let value = serde_json::json!({
        "kind": "milestone",
        "text": capped,
        "node": node.as_wire(),
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Serialize ONE `websearch` event to single-line JSON:
/// `{"kind":"websearch","query":"…","pages":[{"url","title","summary"}]}` + `\n`.
/// The query is char-capped to [`MAX_TEXT_CHARS`]; pages are serialized as-is
/// (already capped upstream). serde_json guarantees the value stays on one line.
fn encode_websearch(query: &str, pages: &[ExaPage]) -> String {
    let capped: String = if query.chars().count() > MAX_TEXT_CHARS {
        query.chars().take(MAX_TEXT_CHARS).collect()
    } else {
        query.to_string()
    };
    let value = serde_json::json!({
        "kind": "websearch",
        "query": capped,
        "pages": pages,
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Serialize ONE `chat` turn to single-line JSON:
/// `{"kind":"chat","role":"assistant"|"user","text":"…"}` + `\n`. `text` is char-capped
/// to [`MAX_TEXT_CHARS`] (codepoint-safe); `role` is not capped.
fn encode_chat(role: &str, text: &str) -> String {
    let capped: String = if text.chars().count() > MAX_CHAT_TEXT_CHARS {
        text.chars().take(MAX_CHAT_TEXT_CHARS).collect()
    } else {
        text.to_string()
    };
    let value = serde_json::json!({
        "kind": "chat",
        "role": role,
        "text": capped,
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Serialize ONE `chat-delta` event to single-line JSON:
/// `{"kind":"chat-delta","seq":N,"text":"…"}` + `\n`. `text` is the cumulative reply-so-far,
/// char-capped to [`MAX_CHAT_TEXT_CHARS`] (codepoint-safe). serde_json escapes control chars
/// so the value stays on one physical line even mid-token.
fn encode_chat_delta(seq: u64, text: &str) -> String {
    let capped: String = if text.chars().count() > MAX_CHAT_TEXT_CHARS {
        text.chars().take(MAX_CHAT_TEXT_CHARS).collect()
    } else {
        text.to_string()
    };
    let value = serde_json::json!({
        "kind": "chat-delta",
        "seq": seq,
        "text": capped,
    });
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Serialize ONE `question` event (Kairion) to its single-line JSON form — the FROZEN
/// wire contract:
/// `{"kind":"question","id":…,"text":…,"options":[{"id","label"}],"unrest":f,"candidates":[{"label","pull"}],"lean":…|null,"directionConfidence":f,"status":…,"affects":[…]}`
/// + a trailing `\n`.
///
/// Built from a [`DoubtSignal`] (unrest / candidates / lean / direction confidence) +
/// the question prose + the discrete options + a stable id + a status. The `text` is
/// char-capped to [`MAX_CHAT_TEXT_CHARS`] (codepoint-safe), exactly like a chat turn.
///
/// KEY-ORDER NOTE: a DERIVED `Serialize` struct emits its fields in DECLARATION order
/// (unlike `serde_json::Value`, whose `Map` is a `BTreeMap` that would re-sort the keys
/// alphabetically), so the on-wire key order matches the frozen contract byte-for-byte.
fn encode_question(
    id: &str,
    text: &str,
    options: &[QOption],
    signal: &DoubtSignal,
    status: &str,
    affects: &[String],
) -> String {
    #[derive(Serialize)]
    struct WireQOption<'a> {
        id: &'a str,
        label: &'a str,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct QuestionEvent<'a> {
        kind: &'static str,
        id: &'a str,
        text: String,
        options: Vec<WireQOption<'a>>,
        unrest: f32,
        // `Candidate`'s own `#[serde(rename_all = "camelCase")]` renders each as
        // `{"label":…,"pull":…}`, matching the contract.
        candidates: &'a [Candidate],
        lean: Option<&'a str>,
        // The ONLY multi-word key — `rename_all` turns it into `directionConfidence`.
        direction_confidence: f32,
        status: &'a str,
        affects: &'a [String],
    }

    let capped: String = if text.chars().count() > MAX_CHAT_TEXT_CHARS {
        text.chars().take(MAX_CHAT_TEXT_CHARS).collect()
    } else {
        text.to_string()
    };
    let event = QuestionEvent {
        kind: "question",
        id,
        text: capped,
        options: options
            .iter()
            .map(|o| WireQOption {
                id: &o.id,
                label: &o.label,
            })
            .collect(),
        unrest: signal.unrest,
        candidates: &signal.candidates,
        lean: signal.lean.as_deref(),
        direction_confidence: signal.direction_confidence,
        status,
        affects,
    };
    // Serialization of this plain struct cannot realistically fail; on the impossible
    // error we still emit a (newline-terminated) blank rather than panic — best-effort.
    let mut line = serde_json::to_string(&event).unwrap_or_default();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn encode_chat_delta_is_one_line_with_seq_and_capped_text() {
        let line = encode_chat_delta(7, "Hel\nlo");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "exactly one physical line");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "chat-delta");
        assert_eq!(v["seq"], 7);
        assert_eq!(v["text"], "Hel\nlo");
    }

    #[test]
    fn encode_chat_delta_caps_overlong_text_codepoint_safe() {
        let long = "é".repeat(MAX_CHAT_TEXT_CHARS + 50);
        let line = encode_chat_delta(1, &long);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(
            v["text"].as_str().unwrap().chars().count(),
            MAX_CHAT_TEXT_CHARS
        );
    }

    #[test]
    fn encode_question_emits_the_exact_frozen_wire_line() {
        let signal = DoubtSignal {
            unrest: 0.5,
            candidates: vec![
                Candidate {
                    label: "SQLite".to_string(),
                    pull: 0.5,
                },
                Candidate {
                    label: "Postgres".to_string(),
                    pull: 0.25,
                },
            ],
            lean: Some("SQLite".to_string()),
            direction_confidence: 0.25,
            // `reasons` is intentionally NOT on the wire — present here to prove the
            // encoder drops it.
            reasons: vec!["hedge density 0.40".to_string()],
        };
        let options = vec![
            QOption {
                id: "sqlite".to_string(),
                label: "SQLite".to_string(),
            },
            QOption {
                id: "pg".to_string(),
                label: "Postgres".to_string(),
            },
        ];
        let affects = vec!["src/db.rs".to_string()];
        let line = encode_question("q1", "Which store?", &options, &signal, "open", &affects);

        // EXACT serialized line: every key, in the frozen order, camelCase, all fields.
        let expected = concat!(
            r#"{"kind":"question","id":"q1","text":"Which store?","#,
            r#""options":[{"id":"sqlite","label":"SQLite"},{"id":"pg","label":"Postgres"}],"#,
            r#""unrest":0.5,"candidates":[{"label":"SQLite","pull":0.5},{"label":"Postgres","pull":0.25}],"#,
            r#""lean":"SQLite","directionConfidence":0.25,"status":"open","affects":["src/db.rs"]}"#,
            "\n"
        );
        assert_eq!(line, expected);
    }

    #[test]
    fn encode_question_degrades_with_no_options_and_null_lean() {
        // The DEGRADE case: no thinking trace / no options ⇒ unrest 0, empty candidates,
        // lean null. The line is still a well-formed question event.
        let signal = DoubtSignal {
            unrest: 0.0,
            candidates: vec![],
            lean: None,
            direction_confidence: 0.0,
            reasons: vec![],
        };
        let line = encode_question("q2", "Go on?", &[], &signal, "reopened", &[]);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "exactly one physical line");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "question");
        assert_eq!(v["id"], "q2");
        assert_eq!(v["text"], "Go on?");
        assert_eq!(v["options"].as_array().unwrap().len(), 0);
        assert_eq!(v["unrest"], 0.0);
        assert_eq!(v["candidates"].as_array().unwrap().len(), 0);
        assert!(
            v["lean"].is_null(),
            "lean serializes as JSON null when None"
        );
        assert_eq!(v["directionConfidence"], 0.0);
        assert_eq!(v["status"], "reopened");
        assert_eq!(v["affects"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn encode_question_caps_overlong_text_codepoint_safe_and_stays_one_line() {
        let long = "é".repeat(MAX_CHAT_TEXT_CHARS + 30);
        let signal = DoubtSignal {
            unrest: 0.0,
            candidates: vec![],
            lean: None,
            direction_confidence: 0.0,
            reasons: vec![],
        };
        let line = encode_question("q3", &long, &[], &signal, "open", &[]);
        assert_eq!(line.matches('\n').count(), 1);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(
            v["text"].as_str().unwrap().chars().count(),
            MAX_CHAT_TEXT_CHARS
        );
    }

    #[test]
    fn question_appends_well_formed_jsonl_and_disabled_no_ops() {
        let signal = DoubtSignal {
            unrest: 0.3,
            candidates: vec![],
            lean: None,
            direction_confidence: 0.0,
            reasons: vec![],
        };
        // Disabled emitter never touches disk.
        let d = Activity::disabled();
        assert!(!d.is_enabled());
        d.question("x", "q", &[], &signal, "open", &[]); // no panic, no file

        // Live emitter appends one parseable line and reports enabled.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("activity.jsonl");
        let a = Activity::with_path(&file);
        assert!(a.is_enabled());
        a.question("x", "q", &[], &signal, "open", &[]);
        let body = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["kind"], "question");
    }

    #[test]
    fn encode_milestone_is_one_well_formed_json_line() {
        let line = encode_milestone("Planning: 3 spine files", Node::Dot);
        // Exactly one trailing newline; no embedded newline.
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);

        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "milestone");
        assert_eq!(v["text"], "Planning: 3 spine files");
        assert_eq!(v["node"], "dot");
    }

    #[test]
    fn encode_websearch_is_one_line_with_pages_and_capped_query() {
        let pages = vec![
            ExaPage {
                url: "https://stripe.com/docs".to_string(),
                title: "Usage billing".to_string(),
                summary: "meter via UsageRecord".to_string(),
            },
            ExaPage {
                url: "https://x.test".to_string(),
                title: "X".to_string(),
                summary: "".to_string(),
            },
        ];
        // A query with an embedded newline must NOT break the single-line contract.
        let line = encode_websearch("stripe\nbilling", &pages);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "exactly one physical line");

        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "websearch");
        assert_eq!(v["query"], "stripe\nbilling");
        let arr = v["pages"].as_array().expect("pages array");
        assert_eq!(arr.len(), 2);
        // Pages serialize with the exact wire keys (not camelCase — they have no _).
        assert_eq!(arr[0]["url"], "https://stripe.com/docs");
        assert_eq!(arr[0]["title"], "Usage billing");
        assert_eq!(arr[0]["summary"], "meter via UsageRecord");
    }

    #[test]
    fn encode_websearch_caps_an_overlong_query() {
        let long = "é".repeat(MAX_TEXT_CHARS + 40);
        let line = encode_websearch(&long, &[]);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(
            v["query"].as_str().unwrap().chars().count(),
            MAX_TEXT_CHARS,
            "query char-capped, codepoint-safe"
        );
    }

    #[test]
    fn encode_chat_is_one_line_with_role_and_capped_text() {
        // A reply with an embedded newline must stay on ONE physical line.
        let line = encode_chat("assistant", "I drafted a plan.\nReading the docs now.");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1, "exactly one physical line");
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["kind"], "chat");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["text"], "I drafted a plan.\nReading the docs now.");

        // Text is char-capped at the larger CHAT cap (codepoint-safe).
        let long = "é".repeat(MAX_CHAT_TEXT_CHARS + 40);
        let capped: Value = serde_json::from_str(encode_chat("user", &long).trim_end()).unwrap();
        assert_eq!(
            capped["text"].as_str().unwrap().chars().count(),
            MAX_CHAT_TEXT_CHARS,
        );
        assert_eq!(capped["role"], "user");
    }

    #[test]
    fn node_wire_strings_match_the_contract() {
        assert_eq!(Node::Hollow.as_wire(), "");
        assert_eq!(Node::Dot.as_wire(), "dot");
        assert_eq!(Node::Sage.as_wire(), "sage");
        assert_eq!(Node::Terra.as_wire(), "terra");
    }

    #[test]
    fn text_is_char_capped_without_splitting_a_codepoint() {
        // A multi-byte label longer than the cap is truncated to MAX_TEXT_CHARS chars.
        let long = "é".repeat(MAX_TEXT_CHARS + 50);
        let line = encode_milestone(&long, Node::Hollow);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        let got = v["text"].as_str().unwrap();
        assert_eq!(
            got.chars().count(),
            MAX_TEXT_CHARS,
            "capped to MAX_TEXT_CHARS chars"
        );
        // Every char is intact 'é' (no replacement char from a split codepoint).
        assert!(got.chars().all(|c| c == 'é'));
    }

    #[test]
    fn embedded_newline_in_label_stays_on_one_line() {
        // A label with a stray newline must not break the one-event-per-line contract.
        let line = encode_milestone("explor\ning src/a.rs", Node::Hollow);
        assert_eq!(
            line.matches('\n').count(),
            1,
            "the only newline is the terminator"
        );
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["text"], "explor\ning src/a.rs");
    }

    #[test]
    fn disabled_emitter_never_touches_disk() {
        // A disabled (no-path) emitter is a silent no-op — nothing to assert beyond
        // "it does not panic and writes nothing" (there is no path to write to).
        let a = Activity::disabled();
        a.milestone("anything", Node::Dot);
        // from_env with the var unset is also disabled.
        std::env::remove_var(ENV_ACTIVITY_FILE);
        let b = Activity::from_env();
        b.milestone("anything", Node::Dot);
        // No file path exists on either; both are no-ops by construction.
        assert!(a.path.is_none());
        assert!(b.path.is_none());
    }

    #[test]
    fn milestone_appends_well_formed_jsonl_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("activity.jsonl");
        let a = Activity::with_path(&file);

        a.milestone("Planning: 2 spine files", Node::Dot);
        a.milestone("exploring src/a.rs", Node::Hollow);
        a.milestone("plan approved", Node::Sage);

        let body = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "one line per milestone, appended in order");

        let l0: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(l0["kind"], "milestone");
        assert_eq!(l0["text"], "Planning: 2 spine files");
        assert_eq!(l0["node"], "dot");

        let l1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(l1["text"], "exploring src/a.rs");
        assert_eq!(l1["node"], "");

        let l2: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(l2["text"], "plan approved");
        assert_eq!(l2["node"], "sage");
    }

    #[test]
    fn milestone_to_unwritable_path_silently_no_ops() {
        // Point at a path whose PARENT does not exist → open fails → swallowed.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("does-not-exist").join("activity.jsonl");
        let a = Activity::with_path(&bad);
        // Must not panic; nothing is created.
        a.milestone("should be dropped", Node::Dot);
        assert!(!bad.exists());
    }
}
