//! Doubt sensor — a pure, deterministic read of a model's THINKING TRACE.
//!
//! The burst loop ([`crate::agent_loop`]) already streams the model's reasoning
//! text. This module derives, with NO LLM call, NO I/O and NO randomness, two
//! things from that text (plus optional decision-token logprobs):
//!   * how UNREST(ful) the model is — an uncertainty MAGNITUDE in `0..1`, and
//!   * which of the CALLER-SUPPLIED options it leans toward — a best-effort
//!     DIRECTION.
//!
//! Honest tiering (read this before trusting a field):
//!
//! * **CoT is a scratchpad, not the internal computation.** The chain-of-thought a
//!   model emits is rhetoric — post-hoc narration that need not reflect the actual
//!   forward pass (Turpin et al. 2023, "Language Models Don't Always Say What They
//!   Think"; Anthropic's faithfulness work). So EVERY signal here measures the
//!   text's rhetoric, not the model's hidden state. `unrest` is a robust PROXY for
//!   hesitation; the `lean` / `candidates` DIRECTION is strictly softer (text
//!   valence is easy to fake or misread). Treat magnitude as a sensor reading and
//!   direction as a hint.
//!
//! * **No percentages, anywhere.** These are unitless scores in `0..1`, not
//!   calibrated probabilities. Rendering them as "73% sure" would launder a
//!   rhetoric proxy into a false precision. Callers must present them as bars /
//!   tiers, never as a percent.
//!
//! * **The entropy tier is oMLX-only.** Per-token top-k logprobs are available in
//!   devboule TODAY only from the local oMLX / mlx-lm backend. The ollama
//!   think-path, Claude and Codex expose no usable token logprobs, so for those
//!   backends `logprobs` is `None` and the signal is TEXT-ONLY: the three text
//!   signals reweight to sum to 1 and `direction_confidence` stays low. When
//!   logprobs ARE present, the token-entropy signal carries the highest weight and
//!   a large top-2 margin is what licenses RAISING `direction_confidence`.
//!
//! * **Pure + deterministic ⇒ unit-testable with no GPU.** Same posture as the
//!   burst loop's OUTPUT_HASH watchdog ([`crate::agent_loop::OUTPUT_HASH_WINDOW`]):
//!   the oscillation signal below is that watchdog's cousin — it counts how often
//!   the trace flips its currently-favoured option, the rhetorical analogue of the
//!   loop-detector's repeated-output hash. Every signal is exercised on synthetic /
//!   recorded traces in the `tests` module with no model and no device.

// The burst-loop wiring that CONSUMES this sensor is out of scope for this module;
// the public surface is therefore only reached from `#[cfg(test)]` today. Mirror
// the crate's convention (see `model.rs`, `agent_loop.rs`) of allowing dead code
// for the non-test build rather than dropping the `pub` API.
#![cfg_attr(not(test), allow(dead_code))]

use serde::Serialize;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The sensor's verdict. `camelCase` on the wire so the frontend's `lean` field
/// (and `directionConfidence`) bind directly with no remapping.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubtSignal {
    /// Uncertainty MAGNITUDE, `0..1`. Robust tier: a weighted mean of the text
    /// (and, when present, logprob) signals that actually fired. Not a probability.
    pub unrest: f32,
    /// Per-option DIRECTION pulls, descending by `pull`. Softer tier than `unrest`.
    pub candidates: Vec<Candidate>,
    /// The single most-pulled option, or `None` when nothing pulls or the top two
    /// are tied within an epsilon (genuinely split — refuse to invent a winner).
    pub lean: Option<String>,
    /// Trust in `lean`, `0..1`. LOW by default because text valence is rhetoric;
    /// only a large decision-token top-2 margin (oMLX logprobs) raises it.
    pub direction_confidence: f32,
    /// Human-readable note of which signals fired and their sub-scores (`0..1`
    /// decimals — never a percentage). Empty when nothing fired.
    pub reasons: Vec<String>,
}

/// One option's directional pull, `0..1`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// The option label, echoed verbatim from the caller's `options`.
    pub label: String,
    /// Squashed positive-minus-hedged mention count, `0..1`.
    pub pull: f32,
}

/// A decision-span token with its top-k alternative probabilities. Only the local
/// oMLX / mlx-lm backend can supply these; every other backend passes `None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionToken {
    /// Probabilities of the top-k alternatives at this position, DESCENDING. They
    /// need not sum to 1 (top-k is a truncation of the full vocab distribution).
    pub top: Vec<f32>,
}

// ---------------------------------------------------------------------------
// Tunables (all derived from the spec; no magic spread through the body)
// ---------------------------------------------------------------------------

/// Markers whose density signals soft uncertainty / weighing of options.
const HEDGE_MARKERS: &[&str] = &[
    "maybe",
    "i think",
    "might",
    "not sure",
    "unsure",
    "unclear",
    "hmm",
    "possibly",
    "perhaps",
    "i guess",
    "on the other hand",
    "it depends",
];

/// Markers whose density signals the model REVERSING itself mid-trace.
const CORRECTION_MARKERS: &[&str] = &[
    "wait",
    "actually",
    "let me reconsider",
    "scratch that",
    "on second thought",
    "hold on",
    "rethink",
];

/// Cues that, in the window immediately BEFORE an option mention, flip that
/// mention from a positive proposal to a negated / hedged one.
const NEGATION_CUES: &[&str] = &[
    "not ",
    "n't ",
    "never ",
    "no ",
    "rule out",
    "ruled out",
    "unlikely",
    "instead of",
    "rather than",
    "doubt",
];

/// Enumeration cues ("either X or Y", "X vs Y", "could be A, B, or C"). When any
/// is present, every NAMED option gets a small upward nudge — enumeration is the
/// model laying out live alternatives, which keeps them split rather than crowning
/// one.
const ENUMERATION_CUES: &[&str] = &["either", " vs ", " versus", "could be", "one of"];

/// Squash steepness for the text-density signals (hedge / correction / oscillation).
/// `s = 1 - exp(-K * density)`, density = hits per 100 whitespace-tokens.
const TEXT_SQUASH_K: f32 = 0.12;
/// Squash steepness for a single option's directional pull.
const DIRECTION_SQUASH_K: f32 = 0.7;

/// Relative weights in the active-signal weighted mean. Renormalised over whichever
/// signals actually fired, so a lone signal yields its own sub-score and the absent
/// entropy tier simply drops out (the text signals then reweight to sum to 1).
const W_ENTROPY: f32 = 0.50; // highest — logprobs beat rhetoric
const W_HEDGE: f32 = 0.40;
const W_CORRECTION: f32 = 0.35;
const W_OSCILLATION: f32 = 0.30;

/// Chars of left-context inspected for a negation cue before an option mention.
const NEGATION_WINDOW: usize = 20;
/// Added to an option's raw (positive - hedged) count when enumeration is present.
const ENUM_NUDGE: f32 = 0.4;

/// A candidate below this pull is dropped (not a real contender).
const PULL_EPS: f32 = 0.02;
/// `lean` is `None` if the top pull is below this.
const LEAN_EPS: f32 = 0.05;
/// `lean` is `None` if the top two pulls are within this of each other (a tie).
const TIE_EPS: f32 = 0.08;

/// Ceiling on text-only `direction_confidence` (rhetoric must not look certain).
const TEXT_CONF_CAP: f32 = 0.4;
/// A candidate at/above this pull counts as a "strong" contender.
const STRONG_PULL: f32 = 0.4;
/// Below this mean top-2 margin the logits are themselves torn.
const SMALL_MARGIN: f32 = 0.2;
/// Hard cap on `direction_confidence` when logprobs say the decision is torn.
const TORN_CONF_CAP: f32 = 0.25;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Analyse a thinking `trace` (plus optional decision-token `logprobs`) against a
/// set of `options`, returning the [`DoubtSignal`]. Pure and deterministic.
pub fn analyze(trace: &str, logprobs: Option<&[DecisionToken]>, options: &[&str]) -> DoubtSignal {
    // One whitespace-normalised, lowercased view drives all text matching so that
    // multi-word markers survive newlines and byte offsets stay self-consistent
    // for the negation windows.
    let norm = normalize(trace);
    let token_count = norm.split(' ').filter(|s| !s.is_empty()).count();

    // ---- UNREST: collect each signal as (weight, sub-score, reason) when it fired.
    let mut active: Vec<(f32, f32)> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();

    let hedge = density_signal(&norm, token_count, HEDGE_MARKERS, TEXT_SQUASH_K);
    if hedge > f32::EPSILON {
        active.push((W_HEDGE, hedge));
        reasons.push(format!("hedge density {hedge:.2}"));
    }

    let correction = density_signal(&norm, token_count, CORRECTION_MARKERS, TEXT_SQUASH_K);
    if correction > f32::EPSILON {
        active.push((W_CORRECTION, correction));
        reasons.push(format!("self-correction {correction:.2}"));
    }

    let oscillation = oscillation_signal(&norm, token_count, options, TEXT_SQUASH_K);
    if oscillation > f32::EPSILON {
        active.push((W_OSCILLATION, oscillation));
        reasons.push(format!("option oscillation {oscillation:.2}"));
    }

    // Token-entropy is the highest-weighted tier but only when oMLX supplied
    // logprobs AND at least one token contributed a value. A peaked distribution
    // legitimately yields a LOW score (confidence), so it is "active" whenever it
    // produced a reading, not only when high.
    if let Some(entropy) = logprobs.and_then(token_entropy) {
        active.push((W_ENTROPY, entropy));
        reasons.push(format!("token-entropy {entropy:.2} (oMLX logprobs)"));
    }

    let unrest = weighted_mean(&active);

    // ---- DIRECTION: per-option pulls (the softer tier).
    let enum_present = ENUMERATION_CUES.iter().any(|c| norm.contains(c));
    let mut candidates: Vec<Candidate> = Vec::new();
    for &opt in options {
        let label_lc = opt.to_lowercase();
        if label_lc.is_empty() {
            continue;
        }
        let positions = find_mentions(&norm, &label_lc);
        if positions.is_empty() {
            continue;
        }
        let (positive, hedged) = classify_mentions(&norm, &positions);
        let nudge = if enum_present { ENUM_NUDGE } else { 0.0 };
        let raw = (positive as f32) - (hedged as f32) + nudge;
        let pull = squash(raw.max(0.0), DIRECTION_SQUASH_K);
        if pull > PULL_EPS {
            candidates.push(Candidate {
                label: opt.to_string(),
                pull,
            });
        }
    }
    // Descending by pull; stable tie-break on label keeps output deterministic.
    candidates.sort_by(|a, b| {
        b.pull
            .partial_cmp(&a.pull)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.label.cmp(&b.label))
    });

    let (lean, separation) = pick_lean(&candidates);
    let direction_confidence = direction_confidence(separation, &candidates, lean.is_some(), logprobs);

    DoubtSignal {
        unrest,
        candidates,
        lean,
        direction_confidence,
        reasons,
    }
}

// ---------------------------------------------------------------------------
// UNREST helpers
// ---------------------------------------------------------------------------

/// `1 - exp(-k * x)`, clamped to `0..1`. Monotone, saturating: turns an unbounded
/// non-negative density into a comparable score that stops rewarding ever-higher
/// counts.
fn squash(x: f32, k: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    (1.0 - (-k * x).exp()).clamp(0.0, 1.0)
}

/// Marker-density signal: count marker hits, convert to hits-per-100-tokens, squash.
fn density_signal(norm: &str, token_count: usize, markers: &[&str], k: f32) -> f32 {
    if token_count == 0 {
        return 0.0;
    }
    let hits: usize = markers.iter().map(|m| count_occurrences(norm, m)).sum();
    let density = (hits as f32) * 100.0 / token_count as f32;
    squash(density, k)
}

/// Oscillation signal — the rhetorical cousin of the burst loop's repeated-output
/// watchdog. Walk every option mention in textual order and count how often the
/// currently-favoured option FLIPS to a DISTINCT one; that flip count, per 100
/// tokens, is squashed like any other density.
fn oscillation_signal(norm: &str, token_count: usize, options: &[&str], k: f32) -> f32 {
    if token_count == 0 || options.len() < 2 {
        return 0.0; // a single (or no) option cannot oscillate
    }
    // (start, end, option-index) for every mention, then ordered left-to-right.
    // A shorter option can match INSIDE a longer one ("server" inside "web server"),
    // so after ordering we drop any mention whose byte range overlaps the one we
    // just kept — that inner match is not a distinct mention and must not count as
    // a flip (else a single "web server" would inflate oscillation, and `unrest`).
    let mut mentions: Vec<(usize, usize, usize)> = Vec::new();
    for (idx, &opt) in options.iter().enumerate() {
        let label_lc = opt.to_lowercase();
        if label_lc.is_empty() {
            continue;
        }
        let len = label_lc.len();
        for pos in find_mentions(norm, &label_lc) {
            mentions.push((pos, pos + len, idx));
        }
    }
    // By start ascending, then the LONGER match first at the same start.
    mentions.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));

    let mut flips = 0usize;
    let mut prev: Option<usize> = None;
    let mut covered_end = 0usize;
    for &(start, end, idx) in &mentions {
        if start < covered_end {
            continue; // overlapping (substring) match — not a distinct mention
        }
        covered_end = end;
        if let Some(p) = prev {
            if p != idx {
                flips += 1;
            }
        }
        prev = Some(idx);
    }
    let density = (flips as f32) * 100.0 / token_count as f32;
    squash(density, k)
}

/// Mean normalised token entropy over the decision tokens, `0..1`, or `None` when
/// no token contributed. Per token: normalise `top` to a distribution `p`, compute
/// `H = -Σ p ln p`, normalise by `ln(k)`. Guards: empty `top` ⇒ skip the token;
/// `k <= 1` ⇒ entropy 0 (still counts); a zero / negative probability term ⇒ skip
/// that term (avoid `ln(0)`); a non-positive sum ⇒ skip the token (cannot normalise).
fn token_entropy(tokens: &[DecisionToken]) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut counted = 0usize;
    for tok in tokens {
        let top = &tok.top;
        if top.is_empty() {
            continue; // no information at this position
        }
        let k = top.len();
        if k <= 1 {
            counted += 1; // a single alternative ⇒ zero entropy, but it IS a reading
            continue;
        }
        let total: f32 = top.iter().filter(|&&v| v > 0.0).sum();
        if total <= 0.0 {
            continue; // degenerate, cannot normalise
        }
        let mut h = 0.0f32;
        for &v in top {
            if v <= 0.0 {
                continue; // skip the term, never ln(0)
            }
            let p = v / total;
            h -= p * p.ln();
        }
        let norm = (k as f32).ln();
        let normalised = if norm > 0.0 {
            (h / norm).clamp(0.0, 1.0)
        } else {
            0.0
        };
        sum += normalised;
        counted += 1;
    }
    if counted == 0 {
        None
    } else {
        Some(sum / counted as f32)
    }
}

/// Weighted mean over the signals that fired, renormalising the weights to sum to
/// 1 across the active set. Empty ⇒ `0.0`.
fn weighted_mean(active: &[(f32, f32)]) -> f32 {
    let wsum: f32 = active.iter().map(|&(w, _)| w).sum();
    if wsum <= 0.0 {
        return 0.0;
    }
    let dot: f32 = active.iter().map(|&(w, s)| w * s).sum();
    (dot / wsum).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// DIRECTION helpers
// ---------------------------------------------------------------------------

/// Split mentions into (positive, negated/hedged) by inspecting the left window of
/// each occurrence for a negation cue.
fn classify_mentions(norm: &str, positions: &[usize]) -> (u32, u32) {
    let mut positive = 0u32;
    let mut hedged = 0u32;
    for &start in positions {
        let window_start = floor_char_boundary(norm, start.saturating_sub(NEGATION_WINDOW));
        let window = &norm[window_start..start];
        if NEGATION_CUES.iter().any(|c| window.contains(c)) {
            hedged += 1;
        } else {
            positive += 1;
        }
    }
    (positive, hedged)
}

/// The most-pulled option and its separation from the runner-up. `None` lean when
/// nothing meaningfully pulls or the top two are tied within [`TIE_EPS`].
fn pick_lean(candidates: &[Candidate]) -> (Option<String>, f32) {
    match candidates.first() {
        None => (None, 0.0),
        Some(top) => {
            if top.pull < LEAN_EPS {
                return (None, 0.0);
            }
            let separation = match candidates.get(1) {
                Some(second) => (top.pull - second.pull).max(0.0),
                None => top.pull, // a lone candidate is fully separated
            };
            if candidates.len() >= 2 && separation < TIE_EPS {
                (None, separation) // genuinely split — do not crown one
            } else {
                (Some(top.label.clone()), separation)
            }
        }
    }
}

/// `direction_confidence`, `0..1`.
///
/// Text valence is rhetoric, so the text-only ceiling is [`TEXT_CONF_CAP`] and the
/// score there is just `separation * cap` — a dominant single lean earns a modest
/// number, a split earns ~0. It is RAISED above that ceiling ONLY when oMLX
/// logprobs corroborate a direction: a large mean top-2 margin at the decision
/// tokens means the model was actually decisive, so we trust the lean and blend
/// `margin` (primary) with text `separation`. Conversely a SMALL margin with two+
/// strong text candidates means the logits are themselves torn — confidence is then
/// hard-capped at [`TORN_CONF_CAP`] no matter how loud the rhetoric.
fn direction_confidence(
    separation: f32,
    candidates: &[Candidate],
    has_lean: bool,
    logprobs: Option<&[DecisionToken]>,
) -> f32 {
    let text_conf = (separation * TEXT_CONF_CAP).clamp(0.0, TEXT_CONF_CAP);
    match logprobs.and_then(mean_top2_margin) {
        Some(margin) if has_lean => {
            let strong = candidates.iter().filter(|c| c.pull >= STRONG_PULL).count();
            if margin < SMALL_MARGIN && strong >= 2 {
                // Logits are torn too: keep it low regardless of rhetoric.
                (margin * 0.3 + text_conf * 0.5).min(TORN_CONF_CAP)
            } else {
                // Logprobs corroborate a decisive direction: raise it.
                (margin * 0.7 + separation * 0.3).clamp(0.0, 1.0)
            }
        }
        // No logprobs, or no lean to trust: stay in the rhetoric-only floor.
        _ => text_conf,
    }
}

/// Mean of `(p1 - p2)` (top-1 minus top-2 probability) across the decision tokens,
/// or `None` when no token has at least two alternatives. `top` is contractually
/// descending; `max(0.0)` guards a malformed pair.
fn mean_top2_margin(tokens: &[DecisionToken]) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut counted = 0usize;
    for tok in tokens {
        if tok.top.len() < 2 {
            continue;
        }
        // Guard malformed input (raw logits / negatives mistakenly passed): a
        // non-positive top-1 or negative top-2 would inflate the margin and let the
        // torn-confidence cap be bypassed. Mirror the entropy path's defensiveness.
        if tok.top[0] <= 0.0 || tok.top[1] < 0.0 {
            continue;
        }
        let total: f32 = tok.top.iter().filter(|&&v| v > 0.0).sum();
        if total <= 0.0 {
            continue;
        }
        let p1 = tok.top[0] / total;
        let p2 = tok.top[1].max(0.0) / total;
        sum += (p1 - p2).max(0.0);
        counted += 1;
    }
    if counted == 0 {
        None
    } else {
        Some(sum / counted as f32)
    }
}

// ---------------------------------------------------------------------------
// Text utilities
// ---------------------------------------------------------------------------

/// Lowercase + collapse every run of whitespace to a single space. Multi-word
/// markers ("on the other hand") then match across original newlines, and all
/// byte offsets used downstream live in this single normalised string.
fn normalize(trace: &str) -> String {
    trace
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Count non-overlapping, word-boundary-respecting occurrences of `needle` in
/// `hay`. A boundary is required only on a side whose marker edge is alphanumeric
/// (so "no -" still matches at "no - that", and "maybe" does not match inside
/// "maybes").
fn count_occurrences(hay: &str, needle: &str) -> usize {
    find_occurrences(hay, needle).len()
}

/// Word-boundary mentions of a (possibly multi-word) `label`.
fn find_mentions(hay: &str, label: &str) -> Vec<usize> {
    find_occurrences(hay, label)
}

/// Shared scanner returning the start byte offset of every boundary-respecting,
/// non-overlapping match of `needle` in `hay`.
fn find_occurrences(hay: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let first_alnum = needle.chars().next().is_some_and(|c| c.is_alphanumeric());
    let last_alnum = needle.chars().last().is_some_and(|c| c.is_alphanumeric());

    let mut start = 0usize;
    while let Some(rel) = hay[start..].find(needle) {
        let abs = start + rel;
        let end = abs + needle.len();

        let left_ok = !first_alnum
            || hay[..abs]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        let right_ok = !last_alnum
            || hay[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());

        if left_ok && right_ok {
            out.push(abs);
        }
        // Non-overlapping advance; `end` is a valid char boundary (end of a match).
        start = end;
        if start >= hay.len() {
            break;
        }
    }
    out
}

/// Largest char boundary `<= idx` (std's `floor_char_boundary` is still unstable).
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Tests — synthetic / recorded traces, no model and no device.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // --- helpers -----------------------------------------------------------
    fn tok(top: &[f32]) -> DecisionToken {
        DecisionToken { top: top.to_vec() }
    }

    fn reason_with(sig: &DoubtSignal, needle: &str) -> bool {
        sig.reasons.iter().any(|r| r.contains(needle))
    }

    // --- 1. confident trace -> low unrest + a clear lean -------------------
    #[test]
    fn confident_trace_low_unrest_clear_lean() {
        let trace = "The bug is clearly in the server. The server rejects valid \
                     sessions because its clock-skew check is too strict. I am \
                     confident the fix lives in the server token verifier. It is the \
                     server.";
        let sig = analyze(trace, None, &["server", "jwt"]);

        // No hedging / backtracking / oscillation ⇒ essentially calm.
        assert!(sig.unrest < 0.05, "unrest should be ~0, got {}", sig.unrest);
        assert!(sig.reasons.is_empty(), "no signal should fire: {:?}", sig.reasons);
        // A single, clear lean.
        assert_eq!(sig.lean.as_deref(), Some("server"));
        assert_eq!(sig.candidates.len(), 1, "only `server` is mentioned");
        assert!(sig.candidates[0].pull > 0.8, "server pull {}", sig.candidates[0].pull);
        // Text-only ⇒ confidence is present but capped (rhetoric, not logits).
        assert!(
            sig.direction_confidence > 0.2 && sig.direction_confidence <= TEXT_CONF_CAP,
            "text-only confidence {}",
            sig.direction_confidence
        );
    }

    // --- 2. torn / genuinely split -> high unrest, split, LOW dir-conf -----
    #[test]
    fn torn_trace_high_unrest_split_low_confidence() {
        let trace = "Hmm, this is hard. Maybe it is the server, or it could be the \
                     jwt. The server fits one clue, but the jwt fits another. On the \
                     other hand, perhaps the server. Then again, possibly the jwt. I \
                     keep going back and forth: server, jwt, server, jwt. Not sure, it \
                     depends, unclear.";
        let sig = analyze(trace, None, &["server", "jwt"]);

        // Hedge density + oscillation drive a high magnitude.
        assert!(sig.unrest > 0.5, "torn unrest should be high, got {}", sig.unrest);
        assert!(reason_with(&sig, "hedge"), "hedge must fire: {:?}", sig.reasons);
        assert!(reason_with(&sig, "oscillation"), "oscillation must fire: {:?}", sig.reasons);

        // Two real contenders, both with substantial pull, near-tied.
        assert_eq!(sig.candidates.len(), 2, "both options are live");
        assert!(sig.candidates[0].pull > 0.3 && sig.candidates[1].pull > 0.3);
        let gap = sig.candidates[0].pull - sig.candidates[1].pull;
        assert!(gap < TIE_EPS, "should be ~tied, gap {gap}");
        // Genuinely split ⇒ refuse a winner, and direction trust is LOW.
        assert!(sig.lean.is_none(), "a tie must not crown a lean: {:?}", sig.lean);
        assert!(
            sig.direction_confidence < 0.2,
            "split direction confidence must be low, got {}",
            sig.direction_confidence
        );
    }

    // --- 3. heavy backtracking -> high unrest via self-correction ----------
    #[test]
    fn backtracking_trace_high_unrest_via_correction() {
        let trace = "Let me reconsider. Wait, actually that approach is wrong. Hold \
                     on. Scratch that. On second thought, no - rethink this from the \
                     start. Actually, wait, let me reconsider again.";
        // Empty options: isolate the correction signal (no direction, no panic).
        let sig = analyze(trace, None, &[]);

        assert!(
            sig.unrest > 0.6,
            "backtracking unrest should be high, got {}",
            sig.unrest
        );
        assert!(reason_with(&sig, "self-correction"), "correction must fire: {:?}", sig.reasons);
        // Correction is the ONLY active signal here, so it must dominate.
        assert!(!reason_with(&sig, "hedge"), "no hedge expected: {:?}", sig.reasons);
        assert!(!reason_with(&sig, "oscillation"), "no oscillation expected: {:?}", sig.reasons);
        assert!(sig.candidates.is_empty() && sig.lean.is_none());
    }

    // --- 4. oscillating server<->jwt -> high unrest via oscillation --------
    #[test]
    fn oscillating_trace_high_unrest_via_oscillation() {
        // No hedge / correction markers: oscillation is the SOLE driver.
        let trace = "The server handles it. The jwt handles it. The server handles \
                     it. The jwt handles it. The server. The jwt.";
        let sig = analyze(trace, None, &["server", "jwt"]);

        assert!(
            sig.unrest > 0.5,
            "oscillation unrest should be high, got {}",
            sig.unrest
        );
        assert!(reason_with(&sig, "oscillation"), "oscillation must fire: {:?}", sig.reasons);
        assert!(!reason_with(&sig, "hedge"), "no hedge markers present: {:?}", sig.reasons);
        assert!(
            !reason_with(&sig, "self-correction"),
            "no correction markers present: {:?}",
            sig.reasons
        );
    }

    // --- 5. empty trace -> unrest 0.0, no panic, no spurious direction -----
    #[test]
    fn empty_trace_is_inert() {
        let sig = analyze("", None, &["server", "jwt"]);
        assert_eq!(sig.unrest, 0.0);
        assert!(sig.candidates.is_empty());
        assert!(sig.lean.is_none());
        assert_eq!(sig.direction_confidence, 0.0);
        assert!(sig.reasons.is_empty());

        // Whitespace-only must behave identically.
        let ws = analyze("   \n\t  ", None, &["server"]);
        assert_eq!(ws.unrest, 0.0);
        assert!(ws.lean.is_none());
    }

    // --- 6. logprobs path: flat -> high entropy, peaked -> low -------------
    #[test]
    fn logprobs_entropy_flat_vs_peaked() {
        let flat = vec![tok(&[0.25, 0.25, 0.25, 0.25]), tok(&[0.25, 0.25, 0.25, 0.25])];
        let peaked = vec![tok(&[0.97, 0.01, 0.01, 0.01]), tok(&[0.95, 0.03, 0.01, 0.01])];

        // Empty trace + empty options ⇒ ONLY the entropy tier is active, so
        // `unrest` is exactly the (highest-weighted) entropy sub-score.
        let flat_sig = analyze("", Some(&flat), &[]);
        let peaked_sig = analyze("", Some(&peaked), &[]);

        assert!(reason_with(&flat_sig, "token-entropy"), "{:?}", flat_sig.reasons);
        assert!(reason_with(&peaked_sig, "token-entropy"), "{:?}", peaked_sig.reasons);

        assert!(flat_sig.unrest > 0.8, "flat entropy should be high, got {}", flat_sig.unrest);
        assert!(peaked_sig.unrest < 0.3, "peaked entropy should be low, got {}", peaked_sig.unrest);
        assert!(
            flat_sig.unrest > peaked_sig.unrest + 0.5,
            "flat {} must clearly exceed peaked {}",
            flat_sig.unrest,
            peaked_sig.unrest
        );
    }

    // --- 6b. entropy guards: empty top skipped, k<=1 ⇒ 0, p=0 skipped ------
    #[test]
    fn entropy_guards_no_panic() {
        // [empty -> skipped] + [single alt, k=1 -> entropy 0, counted] +
        // [flat 4-way -> entropy 1]. Mean over the 2 counted = 0.5.
        let toks = vec![tok(&[]), tok(&[1.0]), tok(&[0.25, 0.25, 0.25, 0.25])];
        let sig = analyze("", Some(&toks), &[]);
        assert!(
            (sig.unrest - 0.5).abs() < 0.02,
            "guarded entropy mean should be ~0.5, got {}",
            sig.unrest
        );

        // A zero-probability term must be skipped (never ln(0)) and not panic.
        let with_zero = vec![tok(&[0.5, 0.5, 0.0])];
        let sig2 = analyze("", Some(&with_zero), &[]);
        // k=3, total=1.0, H=ln(2), norm=ln(3) ⇒ ~0.630. Pin it so any arithmetic
        // regression in the entropy/margin path is caught, not just "no panic".
        assert!(
            (sig2.unrest - 0.630).abs() < 0.01,
            "expected ~0.630, got {}",
            sig2.unrest
        );

        // No usable tokens ⇒ entropy contributes nothing (no signal at all).
        let none_usable = vec![tok(&[]), tok(&[])];
        let sig3 = analyze("", Some(&none_usable), &[]);
        assert_eq!(sig3.unrest, 0.0);
        assert!(sig3.reasons.is_empty());
    }

    // --- 7. single option -> no spurious split -----------------------------
    #[test]
    fn single_option_no_spurious_split() {
        let trace = "Maybe it's the server, possibly the server, I think the server.";
        let sig = analyze(trace, None, &["server"]);
        // At most one candidate can ever exist for one option.
        assert_eq!(sig.candidates.len(), 1, "exactly one contender");
        assert_eq!(sig.lean.as_deref(), Some("server"), "no tie is possible");
        // Hedging is present, so there IS unrest, but no oscillation (one option).
        assert!(sig.unrest > 0.0, "hedges should register some unrest");
        assert!(!reason_with(&sig, "oscillation"), "one option cannot oscillate: {:?}", sig.reasons);
    }

    // --- direction: negation suppresses an option's pull -------------------
    #[test]
    fn negation_suppresses_option() {
        let trace = "It is definitely the server. It is not the jwt. Rule out the jwt.";
        let sig = analyze(trace, None, &["server", "jwt"]);
        assert_eq!(sig.lean.as_deref(), Some("server"));
        // jwt is mentioned but only negatively ⇒ dropped or far below server.
        let jwt_pull = sig
            .candidates
            .iter()
            .find(|c| c.label == "jwt")
            .map_or(0.0, |c| c.pull);
        let server_pull = sig
            .candidates
            .iter()
            .find(|c| c.label == "server")
            .map_or(0.0, |c| c.pull);
        assert!(server_pull > jwt_pull, "server {server_pull} must beat jwt {jwt_pull}");
        assert!(jwt_pull < 0.2, "negated jwt pull should be low, got {jwt_pull}");
    }

    // --- direction: logprobs RAISE confidence on a corroborated lean -------
    #[test]
    fn logprobs_raise_confidence_on_clear_lean() {
        let trace = "It is the server. The server. The server.";
        let opts = ["server", "jwt"];

        let text_only = analyze(trace, None, &opts);
        // A decisive top-2 margin at every decision token.
        let confident_logits = vec![tok(&[0.9, 0.1]), tok(&[0.88, 0.12]), tok(&[0.92, 0.08])];
        let corroborated = analyze(trace, Some(&confident_logits), &opts);

        assert_eq!(text_only.lean.as_deref(), Some("server"));
        assert_eq!(corroborated.lean.as_deref(), Some("server"));
        // Logprobs corroboration must push direction_confidence ABOVE the
        // rhetoric-only ceiling.
        assert!(
            corroborated.direction_confidence > text_only.direction_confidence,
            "logprobs should raise confidence: {} !> {}",
            corroborated.direction_confidence,
            text_only.direction_confidence
        );
        assert!(
            corroborated.direction_confidence > TEXT_CONF_CAP,
            "corroborated confidence {} should exceed the text cap {}",
            corroborated.direction_confidence,
            TEXT_CONF_CAP
        );
    }

    // --- direction: torn logits hold confidence LOW despite loud rhetoric --
    #[test]
    fn torn_logits_keep_confidence_capped() {
        // Rhetoric earns a REAL lean (3 server vs 1 jwt, both strong) but the logits
        // are flat ⇒ genuinely torn. This drives the torn-cap branch (has_lean=true,
        // strong>=2, margin<SMALL_MARGIN). Assert capped AND non-zero, so the cap
        // path actually ran (not the trivial split-returns-0 path).
        let trace = "The server. The server. The server. The jwt.";
        let opts = ["server", "jwt"];
        let flat_logits = vec![tok(&[0.5, 0.5]), tok(&[0.52, 0.48]), tok(&[0.51, 0.49])];
        let sig = analyze(trace, Some(&flat_logits), &opts);
        assert_eq!(sig.lean.as_deref(), Some("server"), "server should earn the lean");
        assert!(
            sig.direction_confidence > 0.0 && sig.direction_confidence <= TORN_CONF_CAP,
            "torn cap branch must run: expected 0 < c <= {}, got {}",
            TORN_CONF_CAP,
            sig.direction_confidence
        );
    }

    // --- serialization: camelCase keys, no percentages ---------------------
    #[test]
    fn serializes_camel_case() {
        let sig = analyze("Maybe the server.", None, &["server", "jwt"]);
        let json = serde_json::to_string(&sig).expect("serialize");
        assert!(json.contains("\"directionConfidence\""), "camelCase key: {json}");
        assert!(json.contains("\"unrest\""));
        assert!(!json.contains('%'), "no percentages anywhere: {json}");
    }
}
