//! Kairion (ORCHESTRATOR-ONLY): a TEXT-ONLY doubt sensor for CLOUD orchestrators.
//!
//! Cloud models (Claude/Codex CLIs) expose NO token logprobs, so unlike a local model we cannot
//! read distributional uncertainty. Instead we infer the orchestrator's "unrest" purely from
//! textual signals in its (summarized) reasoning trace:
//!   * hedge-marker density   — "maybe / perhaps / not sure / on the other hand …"
//!   * self-correction density — "wait / actually / reconsider / scratch that …"
//!   * oscillation            — flip-flopping between the discrete option terms in trace order
//!
//! Each signal is a per-100-token density squashed through `1 - exp(-k*hits)` so it saturates
//! smoothly in [0,1); `unrest` is their weighted mean. Because there are no logprobs,
//! `directionConfidence` is held LOW by construction (a hard cap). The output is the FROZEN
//! `DoubtSignal` shape; `reasons` lists exactly which signals fired, and is NEVER a percentage.
//!
//! This module is pure (no I/O) and is driven by the cloud normalizers, which accumulate the
//! reasoning trace and call [`parse_question_marker`] + [`build_question_line`] when the
//! orchestrator emits a parseable question marker.

use serde::Serialize;

/// One discrete option the orchestrator is weighing, with a 0..1 "pull" toward it (its share of
/// the option mentions in the trace). Mirrors the frozen contract `{ "label", "pull" }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub label: String,
    pub pull: f32,
}

/// The frozen `DoubtSignal` (serde camelCase):
/// `{ unrest, candidates:[{label,pull}], lean, directionConfidence, reasons }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubtSignal {
    /// Overall unrest in [0,1): 0 = confident, →1 = torn.
    pub unrest: f32,
    /// Per-option pull, sorted strongest-first.
    pub candidates: Vec<Candidate>,
    /// The option the trace leans toward, or `None` when it is genuinely torn / has no options.
    pub lean: Option<String>,
    /// How sure we are about the DIRECTION. Text-only → always LOW (hard-capped); never the API's
    /// logprob-grade confidence.
    pub direction_confidence: f32,
    /// Human-readable list of exactly which signals fired. NEVER a percentage.
    pub reasons: Vec<String>,
}

/// Hedge / uncertainty markers (lowercased, word-boundary matched).
const HEDGE_MARKERS: &[&str] = &[
    "maybe",
    "perhaps",
    "possibly",
    "might",
    "could be",
    "not sure",
    "unsure",
    "unclear",
    "hard to say",
    "it depends",
    "on the other hand",
    "however",
    "although",
    "i think",
    "probably",
    "seems",
    "tend to",
    "somewhat",
    "arguably",
    "tentatively",
    "either way",
    "either option",
    "not certain",
    "uncertain",
];

/// Self-correction / backtracking markers (lowercased, word-boundary matched).
const CORRECTION_MARKERS: &[&str] = &[
    "wait",
    "actually",
    "reconsider",
    "rethink",
    "on second thought",
    "scratch that",
    "let me reconsider",
    "hold on",
    "correction",
    "no,",
    "instead",
    "revise",
    "back up",
    "i was wrong",
];

/// Squash steepness for `1 - exp(-K_SQUASH * hitsPer100tok)`.
const K_SQUASH: f32 = 0.2;

/// Signal weights (sum to 1.0): hedging, self-correction, oscillation.
const W_HEDGE: f32 = 0.4;
const W_CORRECTION: f32 = 0.35;
const W_OSCILLATION: f32 = 0.25;

/// Hard ceiling on `directionConfidence`. Text-only inference can never be as sure about the
/// direction as logprobs, so we cap it well below a confident logprob signal.
const DIRECTION_CONFIDENCE_CAP: f32 = 0.4;

/// Lean requires the top option to clearly outweigh the runner-up AND the overall unrest to be
/// below this bar — a torn trace (high unrest) yields `None` even if one option is mentioned more.
const LEAN_MARGIN: f32 = 0.2;
const LEAN_MAX_UNREST: f32 = 0.6;

/// Count NON-OVERLAPPING, word-boundary occurrences of `needle` (already lowercased) inside
/// `haystack` (already lowercased). A boundary is a non-alphanumeric char (or string edge) on
/// each side, so "wait" does not match inside "await"/"waiting". Multi-word phrases match across
/// their internal spaces literally.
fn count_phrase(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut start = 0usize;
    while let Some(rel) = haystack[start..].find(needle) {
        let abs = start + rel;
        let before_ok = abs == 0
            || !haystack[..abs]
                .chars()
                .next_back()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false);
        let after_idx = abs + needle.len();
        let after_ok = after_idx >= haystack.len()
            || !haystack[after_idx..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false);
        if before_ok && after_ok {
            count += 1;
        }
        // Advance by at least one byte so we always make progress (needle.len() >= 1 here).
        start = abs + needle.len();
    }
    count
}

/// Squash a per-100-token density into [0,1).
fn squash(hits_per_100: f32) -> f32 {
    1.0 - (-K_SQUASH * hits_per_100).exp()
}

/// Pure: analyze a (summarized) reasoning `trace` against the discrete `options` and return a
/// [`DoubtSignal`]. Empty trace / no markers → unrest 0, `lean=None`, `directionConfidence=0`
/// and a single explanatory reason (the DEGRADED "plain question" case).
pub fn analyze_text(trace: &str, options: &[&str]) -> DoubtSignal {
    let lower = trace.to_lowercase();
    let token_count = trace.split_whitespace().count().max(1) as f32;
    let per_100 = 100.0 / token_count;

    // --- hedging + self-correction densities ---
    let hedge_hits: usize = HEDGE_MARKERS.iter().map(|m| count_phrase(&lower, m)).sum();
    let correction_hits: usize = CORRECTION_MARKERS
        .iter()
        .map(|m| count_phrase(&lower, m))
        .sum();

    // --- oscillation between the option terms (in trace order) ---
    // Collect (byte position, option index) for every option mention, sort by position, then
    // count adjacent switches between DISTINCT options.
    let mut mentions: Vec<(usize, usize)> = Vec::new();
    let mut counts: Vec<usize> = vec![0; options.len()];
    for (oi, opt) in options.iter().enumerate() {
        let needle = opt.to_lowercase();
        if needle.trim().is_empty() {
            continue;
        }
        // Oscillation must ignore very short option labels ("or"/"go"): even word-boundary-matched
        // they fire too easily across the trace and inflate unrest. They still contribute to the
        // candidate pulls below (a 2-char label is a legitimate option), just not to the switch
        // count that drives the oscillation signal.
        let osc_eligible = needle.trim().chars().count() >= 3;
        let mut start = 0usize;
        while let Some(rel) = lower[start..].find(&needle) {
            let abs = start + rel;
            // SAME word-boundary rule as `count_phrase`: a non-alphanumeric char (or a string
            // edge) on each side, so "or" does not match inside "for"/"corrupt".
            let before_ok = abs == 0
                || !lower[..abs]
                    .chars()
                    .next_back()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
            let after_idx = abs + needle.len();
            let after_ok = after_idx >= lower.len()
                || !lower[after_idx..]
                    .chars()
                    .next()
                    .map(|c| c.is_alphanumeric())
                    .unwrap_or(false);
            if before_ok && after_ok {
                counts[oi] += 1;
                if osc_eligible {
                    mentions.push((abs, oi));
                }
            }
            start = abs + needle.len();
        }
    }
    mentions.sort_by_key(|(pos, _)| *pos);
    let mut switches = 0usize;
    for w in mentions.windows(2) {
        if w[0].1 != w[1].1 {
            switches += 1;
        }
    }

    let sq_hedge = squash(hedge_hits as f32 * per_100);
    let sq_correction = squash(correction_hits as f32 * per_100);
    let sq_oscillation = squash(switches as f32 * per_100);
    let unrest =
        (W_HEDGE * sq_hedge + W_CORRECTION * sq_correction + W_OSCILLATION * sq_oscillation)
            .clamp(0.0, 1.0);

    // --- candidate pulls (share of option mentions), strongest-first ---
    let total_mentions: usize = counts.iter().sum();
    let mut candidates: Vec<Candidate> = options
        .iter()
        .enumerate()
        .map(|(oi, label)| Candidate {
            label: label.to_string(),
            pull: if total_mentions > 0 {
                counts[oi] as f32 / total_mentions as f32
            } else {
                0.0
            },
        })
        .collect();
    candidates.sort_by(|a, b| b.pull.partial_cmp(&a.pull).unwrap_or(std::cmp::Ordering::Equal));

    // --- lean + (LOW) direction confidence ---
    let top = candidates.first().map(|c| c.pull).unwrap_or(0.0);
    let second = candidates.get(1).map(|c| c.pull).unwrap_or(0.0);
    let margin = top - second;
    let lean = if total_mentions > 0 && margin >= LEAN_MARGIN && unrest < LEAN_MAX_UNREST {
        candidates.first().map(|c| c.label.clone())
    } else {
        None
    };
    // Direction confidence: scaled by the margin AND damped by unrest, then hard-capped LOW.
    let direction_confidence = (margin * (1.0 - unrest) * 0.5)
        .clamp(0.0, DIRECTION_CONFIDENCE_CAP);

    // --- reasons (what fired) ---
    let mut reasons: Vec<String> = Vec::new();
    if hedge_hits > 0 {
        reasons.push(format!(
            "{hedge_hits} hedging marker(s) in the reasoning (~{:.1} per 100 tokens)",
            hedge_hits as f32 * per_100
        ));
    }
    if correction_hits > 0 {
        reasons.push(format!(
            "{correction_hits} self-correction(s) (wait/actually/reconsider…)"
        ));
    }
    if switches > 0 {
        reasons.push(format!("oscillated {switches} time(s) between the options"));
    }
    if reasons.is_empty() {
        reasons.push("no hedging, self-correction, or oscillation in the reasoning".to_string());
    }

    DoubtSignal {
        unrest,
        candidates,
        lean,
        direction_confidence,
        reasons,
    }
}

// =============================================================================
// QUESTION assembly — the orchestrator-prompt convention
// =============================================================================
//
// The cloud orchestrator marks a clarification turn by emitting, on its own line in the
// assistant message, the literal sentinel `KAIRION_QUESTION` followed by a JSON object:
//
//   KAIRION_QUESTION {"id":"q1","text":"Which DB?","options":[{"id":"pg","label":"Postgres"},
//                     {"id":"my","label":"MySQL"}],"affects":["schema.rs"]}
//
// The cloud normalizers detect this on assistant finalization, run [`analyze_text`] over the
// accumulated reasoning trace, and emit the FROZEN question wire line (see [`build_question_line`])
// into the same activity `.jsonl` the duplex already appends to. The bridge (`mini_activity`)
// then parses that line into a `ConsoleEntry::Question`.

/// The orchestrator-prompt sentinel that introduces a question marker.
pub const QUESTION_MARKER: &str = "KAIRION_QUESTION";

/// A parsed orchestrator question marker (before doubt analysis is layered on).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuestion {
    pub id: String,
    pub text: String,
    /// `(id, label)` pairs for the discrete options (may be empty → a free-form question).
    pub options: Vec<(String, String)>,
    pub affects: Vec<String>,
    /// "open" | "reopened".
    pub status: String,
}

impl ParsedQuestion {
    /// The option LABELS, for feeding [`analyze_text`].
    pub fn option_labels(&self) -> Vec<&str> {
        self.options.iter().map(|(_, l)| l.as_str()).collect()
    }
}

/// Scan an assistant turn's `text` for a `KAIRION_QUESTION {json}` marker line and parse it.
/// Returns `None` when no well-formed marker is present (so the normalizer falls back to a plain
/// chat turn). Tolerant: a malformed JSON marker is skipped, not fatal.
pub fn parse_question_marker(text: &str) -> Option<ParsedQuestion> {
    parse_question_marker_with_preamble(text).map(|(_, q)| q)
}

/// Like [`parse_question_marker`], but also returns the PREAMBLE — any non-empty prose the
/// orchestrator wrote on the lines BEFORE the `KAIRION_QUESTION` marker line. The cloud
/// normalizers emit that preamble as its own `chat` bubble before the question line, so a turn
/// like "Here's my reasoning.\nKAIRION_QUESTION {…}" does not silently DROP the reasoning.
/// The preamble is trimmed; an empty preamble yields `None` (no spurious empty chat bubble).
pub fn parse_question_marker_with_preamble(
    text: &str,
) -> Option<(Option<String>, ParsedQuestion)> {
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix(QUESTION_MARKER) else {
            continue;
        };
        let json = rest.trim();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            continue;
        };
        let text_field = v
            .get("text")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text_field.is_empty() {
            continue;
        }
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("q")
            .to_string();
        let options = v
            .get("options")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| {
                        let label = o.get("label").and_then(|l| l.as_str())?.trim();
                        if label.is_empty() {
                            return None;
                        }
                        let oid = o
                            .get("id")
                            .and_then(|i| i.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .unwrap_or(label);
                        Some((oid.to_string(), label.to_string()))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let affects = v
            .get("affects")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let status = match v.get("status").and_then(|x| x.as_str()) {
            Some("reopened") => "reopened",
            _ => "open",
        }
        .to_string();
        // The preamble is every line BEFORE the marker line (joined back with '\n'), trimmed.
        let preamble: String = text
            .lines()
            .take(idx)
            .collect::<Vec<_>>()
            .join("\n");
        let preamble = preamble.trim();
        let preamble = if preamble.is_empty() {
            None
        } else {
            Some(preamble.to_string())
        };
        return Some((
            preamble,
            ParsedQuestion {
                id,
                text: text_field,
                options,
                affects,
                status,
            },
        ));
    }
    None
}

/// Assemble the FROZEN question wire line from a parsed question + the accumulated reasoning
/// `trace`. Runs [`analyze_text`] over the option labels and serializes:
/// `{"kind":"question","id","text","options":[{"id","label"}],"unrest","candidates":[{"label",
/// "pull"}],"lean","directionConfidence","status","affects"}`. No trailing newline (the writer
/// appends it), matching the other bridge lines.
pub fn build_question_line(trace: &str, q: &ParsedQuestion) -> String {
    let labels = q.option_labels();
    let signal = analyze_text(trace, &labels);
    let options_json: Vec<serde_json::Value> = q
        .options
        .iter()
        .map(|(id, label)| serde_json::json!({ "id": id, "label": label }))
        .collect();
    let candidates_json: Vec<serde_json::Value> = signal
        .candidates
        .iter()
        .map(|c| serde_json::json!({ "label": c.label, "pull": c.pull }))
        .collect();

    // Only `text` and `affects` are unbounded, untrusted fields. Assemble with whichever
    // (possibly truncated) variant of them is passed.
    let assemble = |text: &str, affects: &[String]| -> String {
        serde_json::json!({
            "kind": "question",
            "id": q.id,
            "text": text,
            "options": options_json,
            "unrest": signal.unrest,
            "candidates": candidates_json,
            "lean": signal.lean,
            "directionConfidence": signal.direction_confidence,
            "status": q.status,
            "affects": affects,
        })
        .to_string()
    };

    // Fast path: real questions are tiny and fit comfortably.
    let line = assemble(&q.text, &q.affects);
    if line.len() <= QUESTION_LINE_BUDGET {
        return line;
    }

    // Oversized: a pathological `text` and/or `affects` would push the line past the host's
    // MAX_LINE_BYTES guard in `mini_activity::parse_question_line`, which SILENTLY DROPS any
    // longer line — losing the whole question. Shrink to fit: first try truncating `text` while
    // keeping `affects`; if `affects` alone is too big, drop it and truncate `text`.
    if let Some(fitted) = fit_question_line(&assemble, &q.text, &q.affects) {
        return fitted;
    }
    if let Some(fitted) = fit_question_line(&assemble, &q.text, &[]) {
        return fitted;
    }
    // Degenerate fallback (e.g. an enormous options/candidates list, beyond text+affects): emit
    // the smallest line we can. Still far below MAX_LINE_BYTES for any realistic option set.
    assemble("", &[])
}

/// Target byte budget for an assembled question line. Kept well under
/// [`mini_activity`]'s `MAX_LINE_BYTES` (8192) so that even after the writer appends a newline and
/// the reader re-caps fields, the line is never dropped for being oversized.
const QUESTION_LINE_BUDGET: usize = 6000;

/// Truncate `text` (keeping `affects`) so `assemble(text, affects)` fits in
/// [`QUESTION_LINE_BUDGET`]. Returns `None` when even an EMPTY text cannot fit with this `affects`
/// (the caller then retries with `affects` dropped). Char-boundary safe.
fn fit_question_line<F: Fn(&str, &[String]) -> String>(
    assemble: &F,
    text: &str,
    affects: &[String],
) -> Option<String> {
    let full = assemble(text, affects);
    if full.len() <= QUESTION_LINE_BUDGET {
        return Some(full);
    }
    let excess = full.len() - QUESTION_LINE_BUDGET;
    // Removing N source bytes from `text` shrinks the JSON by AT LEAST N (a JSON-escaped char is
    // never shorter than its source bytes), so dropping `excess + MARGIN` source bytes guarantees
    // we land under budget. MARGIN absorbs the char-boundary rounding below.
    const MARGIN: usize = 16;
    let removable = excess + MARGIN;
    if text.len() <= removable {
        return None; // text can't absorb the excess → this affects-set won't fit
    }
    let keep_bytes = text.len() - removable;
    let truncated = truncate_to_bytes(text, keep_bytes);
    let line = assemble(&truncated, affects);
    if line.len() <= QUESTION_LINE_BUDGET {
        Some(line)
    } else {
        None
    }
}

/// Truncate `s` to at most `max_bytes` bytes WITHOUT splitting a UTF-8 char.
fn truncate_to_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn confident_trace_has_low_unrest_and_a_clear_lean() {
        let trace = "We should use Postgres. Postgres is the right choice here. \
                     Postgres handles transactional workloads well, so Postgres it is.";
        let s = analyze_text(trace, &["Postgres", "MySQL"]);
        assert!(s.unrest < 0.25, "confident trace stays calm: {}", s.unrest);
        assert_eq!(s.lean.as_deref(), Some("Postgres"));
        assert!(s.direction_confidence <= DIRECTION_CONFIDENCE_CAP);
        assert_eq!(s.candidates[0].label, "Postgres");
        assert!(s.candidates[0].pull > s.candidates[1].pull);
    }

    #[test]
    fn torn_trace_has_high_unrest_and_no_lean() {
        let trace = "Maybe Postgres, but perhaps MySQL would be simpler. On the other hand \
                     Postgres scales better. However MySQL might be easier to host. I'm not \
                     sure; it depends. Either way could work — hard to say.";
        let s = analyze_text(trace, &["Postgres", "MySQL"]);
        assert!(s.unrest > 0.5, "torn trace is restless: {}", s.unrest);
        assert!(s.lean.is_none(), "torn trace must not lean: {:?}", s.lean);
        // oscillation fired
        assert!(
            s.reasons.iter().any(|r| r.contains("oscillated")),
            "reasons: {:?}",
            s.reasons
        );
        // never a percentage
        assert!(s.reasons.iter().all(|r| !r.contains('%')));
    }

    #[test]
    fn backtracking_trace_flags_self_corrections() {
        let trace = "Let's go with MySQL. Wait, actually Postgres is better for this. \
                     Hold on, let me reconsider. No, MySQL. Actually Postgres after all.";
        let s = analyze_text(trace, &["Postgres", "MySQL"]);
        assert!(
            s.reasons.iter().any(|r| r.contains("self-correction")),
            "reasons: {:?}",
            s.reasons
        );
        let calm = analyze_text("Postgres. Postgres. Postgres.", &["Postgres", "MySQL"]);
        assert!(
            s.unrest > calm.unrest,
            "backtracking ({}) more unrest than calm ({})",
            s.unrest,
            calm.unrest
        );
    }

    #[test]
    fn empty_trace_degrades_to_a_calm_plain_signal() {
        let s = analyze_text("", &["A", "B"]);
        assert_eq!(s.unrest, 0.0);
        assert!(s.lean.is_none());
        assert_eq!(s.direction_confidence, 0.0);
        assert_eq!(s.candidates.len(), 2);
        assert!(s.candidates.iter().all(|c| c.pull == 0.0));
        assert_eq!(s.reasons.len(), 1);
        assert!(s.reasons[0].contains("no hedging"));
    }

    #[test]
    fn word_boundary_avoids_false_matches() {
        // "await"/"waiting" must NOT count as the "wait" correction marker; "Postgresql" must not
        // be matched when the option is "Postgres" with a trailing alnum... actually it is a
        // prefix match by design — we only assert the correction boundary here.
        assert_eq!(count_phrase("awaiting the awaited result", "wait"), 0);
        assert_eq!(count_phrase("wait, hold on", "wait"), 1);
    }

    #[test]
    fn doubt_signal_serializes_to_the_frozen_camelcase_shape() {
        let s = analyze_text("maybe A, maybe B", &["A", "B"]);
        let v: Value = serde_json::to_value(&s).unwrap();
        assert!(v.get("unrest").is_some());
        assert!(v.get("candidates").is_some());
        assert!(v.get("directionConfidence").is_some(), "camelCase key");
        assert!(v.get("reasons").is_some());
        // candidate camelCase
        assert!(v["candidates"][0].get("label").is_some());
        assert!(v["candidates"][0].get("pull").is_some());
    }

    #[test]
    fn parses_a_question_marker_line() {
        let text = "Here are the options.\n\
                    KAIRION_QUESTION {\"id\":\"q1\",\"text\":\"Which DB?\",\"options\":[{\"id\":\"pg\",\"label\":\"Postgres\"},{\"id\":\"my\",\"label\":\"MySQL\"}],\"affects\":[\"schema.rs\"]}";
        let q = parse_question_marker(text).expect("parses the marker");
        assert_eq!(q.id, "q1");
        assert_eq!(q.text, "Which DB?");
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0], ("pg".to_string(), "Postgres".to_string()));
        assert_eq!(q.affects, vec!["schema.rs".to_string()]);
        assert_eq!(q.status, "open");
    }

    #[test]
    fn missing_or_malformed_marker_is_none() {
        assert!(parse_question_marker("just a normal chat turn").is_none());
        assert!(parse_question_marker("KAIRION_QUESTION not json").is_none());
        // a marker with no text field is rejected
        assert!(parse_question_marker("KAIRION_QUESTION {\"id\":\"q\"}").is_none());
    }

    #[test]
    fn reopened_status_is_carried() {
        let q = parse_question_marker(
            "KAIRION_QUESTION {\"text\":\"again?\",\"status\":\"reopened\",\"options\":[]}",
        )
        .unwrap();
        assert_eq!(q.status, "reopened");
        assert_eq!(q.id, "q", "missing id defaults to q");
        assert!(q.options.is_empty());
    }

    #[test]
    fn build_question_line_matches_the_frozen_wire_shape() {
        let q = parse_question_marker(
            "KAIRION_QUESTION {\"id\":\"q1\",\"text\":\"Which DB?\",\"options\":[{\"id\":\"pg\",\"label\":\"Postgres\"},{\"id\":\"my\",\"label\":\"MySQL\"}],\"affects\":[\"a.rs\"]}",
        )
        .unwrap();
        let trace = "Maybe Postgres, but perhaps MySQL. However Postgres. Not sure.";
        let line = build_question_line(trace, &q);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "question");
        assert_eq!(v["id"], "q1");
        assert_eq!(v["text"], "Which DB?");
        assert_eq!(v["status"], "open");
        assert_eq!(v["options"][0]["id"], "pg");
        assert_eq!(v["options"][0]["label"], "Postgres");
        assert_eq!(v["affects"][0], "a.rs");
        assert!(v.get("unrest").is_some());
        assert!(v.get("directionConfidence").is_some());
        let cands = v["candidates"].as_array().expect("candidates array");
        assert_eq!(cands.len(), 2);
        assert!(cands[0].get("label").is_some());
        assert!(cands[0].get("pull").is_some());
        // lean is present (possibly null) — it is a required wire field
        assert!(v.as_object().unwrap().contains_key("lean"));
    }

    #[test]
    fn degraded_question_with_no_thinking_still_assembles() {
        // No trace, no options → a plain question with zero doubt (degrade gracefully).
        let q = parse_question_marker("KAIRION_QUESTION {\"text\":\"go ahead?\"}").unwrap();
        let line = build_question_line("", &q);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["unrest"], 0.0);
        assert_eq!(v["lean"], Value::Null);
        assert_eq!(v["directionConfidence"], 0.0);
        assert_eq!(v["options"].as_array().unwrap().len(), 0);
    }
}
