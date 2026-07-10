//! Phase 2 — Pigeon heuristic routing (alpha).
//!
//! Routing intelligence lives in Rust (design doc §11 #3, decision #3): the pi
//! sidecar's `before_agent_start` hook asks Rust to classify a prompt and gets
//! back `{ tier, provider, model }`. This module owns the heuristic
//! classifier + the tier→provider/model table.
//!
//! Deliberately NOT in scope yet (explicit TODO markers):
//! - LLM-based classification (the heuristic is alpha; Phase 3 upgrades it).
//! - Self-learning bandit threshold adjustment (Phase 3): Pigeon runs cheap +
//!   Claude in parallel, compares via Censor/human review, and nudges the
//!   complexity threshold. Claude is ground truth, not a competitor.
//!
//! Design doc: `docs/devboule-on-pi-architecture.md` §9/#3, §11/#3.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Prompt complexity tier (Phase 2). Extends [`crate::backend::mini_coder::DirectiveTier`]
/// (plan-level Mini|Main) onto the prompt level: how much model to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptTier {
    /// Local small/free model (oMLX/Ollama loopback).
    Cheap,
    /// Cheap cloud / free tier.
    Moderate,
    /// Full cloud coder model.
    Expensive,
}

/// Classification result returned by the Tauri command + the JSONL `classified`
/// response. JSON-serializable (camelCase).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub tier: PromptTier,
    pub provider: String,
    pub model: String,
}

// ---- heuristic classifier (multi-factor weighted scorer) -----------------
//
// Adapted from Puppetmaster `router.py` (MIT license). A pure, deterministic
// capability score (0..100) is built from:
//   1. a role base score detected from prompt keywords,
//   2. additive "hard" complexity signals (audit/security/performance/...),
//   3. subtractive "easy" trivial-edit signals (typo/comment/rename/...),
//   4. additive frontend/UI signals (the coding analogue of vision detection),
//   5. an instruction-length adjustment,
//   6. clamping to 5..=100 (cheap floor; expensive ceiling = frontier flagship).
// Same input → same output. No LLM is consulted (Phase 3 may learn thresholds).

/// Map a capability score to a routing tier (design-doc tiers).
fn score_to_tier(score: u32) -> PromptTier {
    match score {
        0..=32 => PromptTier::Cheap,
        33..=65 => PromptTier::Moderate,
        _ => PromptTier::Expensive,
    }
}

/// True when `text` (already lowercased) contains any of the literal keywords
/// as whole words. Regex `\b` word boundaries prevent false positives such as
/// "read" matching inside "readability" or "fix" inside "fixes".
fn contains_any(text: &str, words: &[&str]) -> bool {
    let pattern = words
        .iter()
        .map(|w| format!(r"\b{}\b", regex::escape(w)))
        .collect::<Vec<_>>()
        .join("|");
    let re = Regex::new(&pattern).expect("role keyword pattern must be valid");
    re.is_match(text)
}

/// Role base score detected from prompt keywords. Returns `None` when no role
/// keyword matches (the caller uses the neutral default of 50 and may apply the
/// short-trivial floor). All matching role scores are collected and the MAX is
/// taken, so a prompt that is both "implement" (75) and "audit" (85) scores 85,
/// not 75 — the highest matching role must win (not first-match-wins).
fn role_base_score(text: &str) -> Option<u32> {
    let candidates: &[(&[&str], u32)] = &[
        (
            &["implement", "build a", "create a new", "from scratch"],
            75,
        ),
        (&["refactor", "restructure"], 75),
        (&["audit", "security review", "vulnerability"], 85),
        (&["design", "architecture", "system design"], 85),
        (&["fix", "debug", "bug", "crash"], 70),
        (&["test", "add test", "coverage"], 60),
        (&["add", "handle", "feature", "extend", "support"], 55),
        (&["explore", "find", "search", "read", "look at"], 50),
    ];
    candidates
        .iter()
        .filter(|(words, _)| contains_any(text, words))
        .map(|(_, score)| *score)
        .max()
}

/// Compile `(regex, weight)` pairs once.
fn compile_weighted(specs: &[(&str, i32)]) -> Vec<(Regex, i32)> {
    specs
        .iter()
        .map(|(p, w)| {
            (
                Regex::new(p).expect("pigeon routing regex must be valid"),
                *w,
            )
        })
        .collect()
}

/// Sum the weights of every matched "hard" (complexity-raising) pattern.
fn hard_signal_weight(text: &str) -> i32 {
    static RE: OnceLock<Vec<(Regex, i32)>> = OnceLock::new();
    let patterns = RE.get_or_init(|| {
        compile_weighted(&[
            (r"(?i)\baudit\b|\bsecurity\b|\bvulnerability\b|\bexploit\b|\binjection\b", 15),
            (
                r"(?i)\bperformance\b|\boptimize\b|\bslow\b|\blatency\b|\bthroughput\b|\bbottleneck\b",
                12,
            ),
            (r"(?i)\bcross.?repo\b|\bcross.?module\b|\bmonorepo\b", 10),
            (r"(?i)\bdesign\b|\barchitecture\b", 10),
            (r"(?i)\brefactor\b|\brewrite\b|\brestructure\b", 8),
            (
                r"(?i)\bdistributed\b|\bconcurrent\b|\basync\b|\bparallel\b|\brace condition\b|\bdeadlock\b",
                8,
            ),
            (r"(?i)\bcomplex\b|\bnon.?trivial\b|\bchallenging\b", 5),
            (r"(?i)\bimplement from scratch\b|\bbuild from ground up\b", 10),
        ])
    });
    patterns
        .iter()
        .filter(|(re, _)| re.is_match(text))
        .map(|(_, w)| *w)
        .sum()
}

/// Sum the (negative) weights of every matched "easy" (trivial-edit) pattern.
fn easy_signal_weight(text: &str) -> i32 {
    static RE: OnceLock<Vec<(Regex, i32)>> = OnceLock::new();
    let patterns = RE.get_or_init(|| {
        compile_weighted(&[
            (r"(?i)\btypo\b|\bspelling\b", -15),
            (r"(?i)\bcomment\b|\bdoc comment\b|\badd doc\b", -8),
            (r"(?i)\brename\b|\bmove file\b", -8),
            (r"(?i)\bformat\b|\bprettier\b|\brustfmt\b|\bclippy\b", -8),
            (r"(?i)\bsimple\b|\btrivial\b|\bstraightforward\b", -5),
        ])
    });
    patterns
        .iter()
        .filter(|(re, _)| re.is_match(text))
        .map(|(_, w)| *w)
        .sum()
}

/// Sum the weights of every matched frontend/UI pattern (vision analogue).
fn ui_signal_weight(text: &str) -> i32 {
    static RE: OnceLock<Vec<(Regex, i32)>> = OnceLock::new();
    let patterns = RE.get_or_init(|| {
        compile_weighted(&[
            (
                r"(?i)\bcss\b|\bhtml\b|\bjsx\b|\bcomponent\b|\bstyling\b|\blayout\b|\bresponsive\b|\btailwind\b",
                3,
            ),
            (r"(?i)\bfrontend\b|\bui\b|\bux\b|\buser interface\b", 3),
        ])
    });
    patterns
        .iter()
        .filter(|(re, _)| re.is_match(text))
        .map(|(_, w)| *w)
        .sum()
}

/// Capability score 0..100 for a prompt. Pure heuristic, no LLM, deterministic.
/// Adapted from Puppetmaster `router.py` (MIT license).
pub fn classify_capability_needed(text: &str) -> u32 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 5; // floor for empty/trivial prompts
    }
    let lower = trimmed.to_ascii_lowercase();
    let chars = trimmed.len();

    // 1. Role base score (neutral 50 when no role keyword matches).
    let role = role_base_score(&lower);
    let base = role.unwrap_or(50);

    // 2. Easy signals. A *truly trivial* edit (cumulative penalty <= -10, e.g.
    //    typo -15, or typo+comment) collapses the base to a cheap floor. A
    //    *minor* easy signal such as "simple" (-5) must NOT collapse the whole
    //    role base — it is handled below as a downgrade, not a collapse.
    let easy = easy_signal_weight(&lower);
    let effective_base = if easy <= -10 { base.min(25) } else { base };

    let hard = hard_signal_weight(&lower);
    let ui = ui_signal_weight(&lower);

    // Short, no-role, signal-free prompts are trivial → cheap floor.
    if role.is_none() && hard == 0 && ui == 0 && chars < 40 {
        return 5;
    }

    let mut score: i32 = effective_base as i32 + hard + easy + ui;

    // 5. Length adjustment.
    if chars > 2000 {
        score += 10;
    } else if chars > 800 {
        score += 5;
    }

    // Minor easy signal, no hard signals: keep the score in the Moderate band so
    // a small "simple"/"trivial" qualifier can't keep a high role base
    // (e.g. "implement" = 75) Expensive, nor collapse it to Cheap.
    if easy < 0 && easy > -10 && hard == 0 {
        score = score.min(65);
    }

    // 6. Clamp 5..=100.
    score.clamp(5, 100) as u32
}

/// Heuristic classifier: prompt → routing tier. Pure, deterministic, no LLM.
/// Wraps [`classify_capability_needed`] and maps the score to a [`PromptTier`].
pub fn classify_prompt(text: &str) -> PromptTier {
    score_to_tier(classify_capability_needed(text))
}

// ---- tier table -----------------------------------------------------------

/// Pigeon-ON fallback table: pure tier→(provider, model) defaults.
/// Used by unit tests and as the spike-time fallback when Pigeon routing is enabled.
pub fn resolve_tier_defaults(tier: PromptTier) -> (String, String) {
    match tier {
        PromptTier::Cheap => ("openai".to_string(), "qwen2.5-coder:7b".to_string()),
        // TODO(Phase 3): bandit-tuned tier→provider/model; these cloud defaults
        // only apply when Pigeon is ENABLED.
        PromptTier::Moderate => ("openrouter".to_string(), "tencent/hy3:free".to_string()),
        PromptTier::Expensive => ("openrouter".to_string(), "openai/gpt-4o".to_string()),
    }
}

/// Full classification used by the JSONL protocol (`classified` response).
pub fn classify_prompt_full(text: &str) -> Classification {
    let tier = classify_prompt(text);
    let (provider, model) = resolve_tier_defaults(tier);
    Classification {
        tier,
        provider,
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_prompt_cheap() {
        // Short, trivial, no complexity keyword → Cheap.
        assert_eq!(classify_prompt("fix typo in main.rs"), PromptTier::Cheap);
    }

    #[test]
    fn classify_prompt_moderate() {
        // Mid-length (~90 words), no complexity keywords → Moderate by length.
        let prompt = "\
Add a small settings panel to the preferences window. It should have a checkbox \
for enabling notifications and a text field for the user display name. Wire the \
checkbox to the existing settings store and persist on change. Add a unit test \
that toggles the flag and verifies the stored value. Keep the layout consistent \
with the other panels and use the shared button component. Update the localization \
strings for the new labels. Make sure focus order is correct for keyboard users and \
that the panel scrolls if the window is short.";
        assert!(
            prompt.split_whitespace().count() > 50,
            "fixture must exceed 50 words"
        );
        assert_eq!(classify_prompt(&prompt), PromptTier::Moderate);
    }

    #[test]
    fn classify_prompt_expensive() {
        // Long prompt (>200 words) → Expensive by length.
        let prompt = "We need to build a comprehensive system for this. ".repeat(40);
        assert!(
            prompt.split_whitespace().count() > 200,
            "fixture must exceed 200 words"
        );
        assert_eq!(classify_prompt(&prompt), PromptTier::Expensive);
    }

    #[test]
    fn classify_prompt_expensive_complex_keywords() {
        // Short but architecture-level → Expensive via patterns (not length).
        assert_eq!(
            classify_prompt("implement a distributed cache with TTL"),
            PromptTier::Expensive
        );
    }

    #[test]
    fn classify_prompt_moderate_pattern() {
        // Short but a real bounded change → Moderate via pattern.
        assert_eq!(classify_prompt("add error handling"), PromptTier::Moderate);
    }

    #[test]
    fn classify_prompt_command_returns_cheap_local_provider() {
        // Cheap ⇒ local loopback provider (openai) + qwen model (Pigeon-ON fallback).
        let tier = classify_prompt("fix typo in main.rs");
        assert_eq!(tier, PromptTier::Cheap);
        let (provider, model) = resolve_tier_defaults(tier);
        assert_eq!(
            provider, "openai",
            "Cheap must route to the local loopback provider"
        );
        assert_eq!(model, "qwen2.5-coder:7b");
    }

    #[test]
    fn classify_empty_string_is_cheap() {
        // Empty / whitespace-only prompt ⇒ zero words ⇒ Cheap (safe default).
        assert_eq!(classify_prompt(""), PromptTier::Cheap);
        assert_eq!(classify_prompt("   \n\t  "), PromptTier::Cheap);
    }

    #[test]
    fn classify_unicode_prompt_is_cheap() {
        // Non-ASCII (CJK) short prompt with no complexity keyword ⇒ Cheap.
        assert_eq!(classify_prompt("修正错字"), PromptTier::Cheap);
    }

    #[test]
    fn classify_respects_char_length_boundaries() {
        // Short, no-role, signal-free prompt → cheap floor (trivial).
        let short = "do the thing";
        assert!(short.len() < 40, "fixture must be <40 chars");
        assert_eq!(classify_prompt(short), PromptTier::Cheap);

        // Same words, lengthened past 800 chars with no new signals → the length
        // bump pushes the neutral base into Moderate.
        let long = "do the thing ".repeat(70);
        assert!(long.len() > 800, "fixture must exceed 800 chars");
        assert_eq!(classify_prompt(&long), PromptTier::Moderate);

        // Past 2000 chars → still Moderate (cheap floor + length cap).
        let longer = "do the thing ".repeat(180);
        assert!(longer.len() > 2000, "fixture must exceed 2000 chars");
        assert_eq!(classify_prompt(&longer), PromptTier::Moderate);
    }

    #[test]
    fn classify_mixed_keyword_and_length_takes_higher() {
        // Short (Cheap by length) but a Moderate keyword present ⇒ Moderate (OR-ed).
        assert_eq!(classify_prompt("add error handling"), PromptTier::Moderate);
    }

    // ---- multi-factor scorer (Puppetmaster router.py port) ----------------

    #[test]
    fn score_trivial_typo_is_cheap() {
        // Short trivial edit → Cheap (<33). Easy signal collapses the base.
        let score = classify_capability_needed("fix typo in main.rs");
        assert!(score < 33, "expected cheap score, got {score}");
        assert_eq!(classify_prompt("fix typo in main.rs"), PromptTier::Cheap);
    }

    #[test]
    fn score_simple_explore_is_cheap_or_moderate() {
        // Read-only explore → not Expensive.
        let tier = classify_prompt("read the file and tell me");
        assert!(
            matches!(tier, PromptTier::Cheap | PromptTier::Moderate),
            "expected cheap/moderate, got {tier:?}"
        );
    }

    #[test]
    fn score_add_error_handling_is_moderate() {
        // Bounded edit → Moderate (33..=65).
        let score = classify_capability_needed("add error handling to the login flow");
        assert!(
            (33..=65).contains(&score),
            "expected moderate score, got {score}"
        );
        assert_eq!(
            classify_prompt("add error handling to the login flow"),
            PromptTier::Moderate
        );
    }

    #[test]
    fn score_refactor_near_expensive() {
        // Refactor → Expensive (at/above the 66 boundary).
        let score = classify_capability_needed("refactor the auth module to use Arc");
        assert!(score >= 66, "expected expensive score, got {score}");
        assert_eq!(
            classify_prompt("refactor the auth module to use Arc"),
            PromptTier::Expensive
        );
    }

    #[test]
    fn score_distributed_design_is_expensive() {
        // Design + distributed systems → Expensive.
        let score = classify_capability_needed(
            "design a distributed cache with TTL and consistency guarantees",
        );
        assert!(score >= 66, "expected expensive score, got {score}");
        assert_eq!(
            classify_prompt("design a distributed cache with TTL and consistency guarantees"),
            PromptTier::Expensive
        );
    }

    #[test]
    fn score_audit_security_is_expensive() {
        // Audit + security vulnerability → Expensive.
        let score = classify_capability_needed(
            "audit the codebase for security vulnerabilities in the auth module",
        );
        assert!(score >= 66, "expected expensive score, got {score}");
        assert_eq!(
            classify_prompt("audit the codebase for security vulnerabilities in the auth module"),
            PromptTier::Expensive
        );
    }

    #[test]
    fn score_easy_penalty_offsets_fix_keyword() {
        // Even with the "fix" keyword, typo+comment penalties force Cheap.
        let score = classify_capability_needed("fix typo in comment");
        assert!(score < 33, "expected cheap score, got {score}");
        assert_eq!(classify_prompt("fix typo in comment"), PromptTier::Cheap);
    }

    #[test]
    fn score_length_matters_without_keywords() {
        // 2000+ chars of repetitive text, no hard keywords → Moderate via length.
        let long = "lorem ipsum dolor ".repeat(140);
        assert!(long.len() > 2000, "fixture must exceed 2000 chars");
        let score = classify_capability_needed(&long);
        assert!(
            (33..=65).contains(&score),
            "expected moderate score, got {score}"
        );
        assert_eq!(classify_prompt(&long), PromptTier::Moderate);
    }

    #[test]
    fn score_ui_task_is_moderate() {
        // Frontend/UI coding task → Moderate (UI signals, no Expensive role).
        let score =
            classify_capability_needed("create a responsive login component with Tailwind CSS");
        assert!(
            (33..=65).contains(&score),
            "expected moderate score, got {score}"
        );
        assert_eq!(
            classify_prompt("create a responsive login component with Tailwind CSS"),
            PromptTier::Moderate
        );
    }

    #[test]
    fn score_empty_and_hi_hit_floor() {
        // Empty / trivial short prompt → Cheap floor (5).
        assert_eq!(classify_capability_needed(""), 5);
        assert_eq!(classify_prompt(""), PromptTier::Cheap);
        assert_eq!(classify_capability_needed("hi"), 5);
        assert_eq!(classify_prompt("hi"), PromptTier::Cheap);
    }

    #[test]
    fn score_is_deterministic() {
        // Same input → same output (pure heuristic, no LLM).
        let a = classify_capability_needed("refactor the auth module to use Arc");
        let b = classify_capability_needed("refactor the auth module to use Arc");
        assert_eq!(a, b);
    }

    // ---- regression tests for the phase1/infra MAJOR findings ----------------

    #[test]
    fn regression_find1_highest_role_wins() {
        // Finding #1: highest matching role must win. "audit" (85) must beat
        // "implement" (75) even though implement appears first in the list.
        assert_eq!(
            classify_prompt("audit and implement the auth module"),
            PromptTier::Expensive
        );
    }

    #[test]
    fn regression_find3_simple_counter_is_moderate() {
        // Finding #3: a minor "simple" easy signal must not collapse an
        // "implement" (75) role to Cheap, nor leave it Expensive.
        let tier = classify_prompt("implement a simple counter");
        assert_eq!(tier, PromptTier::Moderate);
        let score = classify_capability_needed("implement a simple counter");
        assert!(
            (33..=65).contains(&score),
            "expected moderate score, got {score}"
        );
    }
}
