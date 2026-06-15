//! Pure normalizers mapping each known tool's native severity/level vocabulary
//! onto Censor's `(Severity, Category)`. The A2 deterministic runners call these
//! after parsing each tool's structured output, so the mapping policy lives in
//! ONE tested place rather than scattered through the parsers.
//!
//! Every normalizer is pure and total: an unknown/empty input NEVER panics and
//! falls back to a conservative default. The default is `Medium` severity (don't
//! cry wolf, don't hide it) with a category appropriate to the tool's domain
//! (e.g. clippy/eslint → Correctness, ruff → Style). Matching is
//! case-insensitive on the level string because tools disagree on casing
//! (`HIGH` vs `high`, `Warn` vs `warning`).
//!
//! These normalizers are defined ahead of their first caller (A2 wires the
//! runners). The transient dead-code is annotated PER ITEM, not module-wide, so
//! that future genuinely-dead code in this file is still flagged.

use super::schema::{Category, Severity};

/// clippy / rustc diagnostic level. clippy emits `error` (often via `-D`
/// "deny"), `warning`, `note`, `help`. We treat deny/error as High, warnings as
/// Medium, anything quieter as Low. Category is Correctness (clippy lints are
/// correctness/idiom, not style for our taxonomy).
#[allow(dead_code)] // first caller is the A2 clippy runner.
pub fn severity_from_clippy(level: &str) -> (Severity, Category) {
    let sev = match level.trim().to_ascii_lowercase().as_str() {
        "deny" | "error" | "ice" => Severity::High,
        "warn" | "warning" => Severity::Medium,
        "note" | "help" | "info" => Severity::Low,
        _ => Severity::Medium,
    };
    (sev, Category::Correctness)
}

/// `cargo check` / rustc compiler diagnostics. `error` is a hard build failure
/// (High); `warning` Medium; notes Low. Always Correctness.
#[allow(dead_code)] // first caller is the A2 cargo-check runner.
pub fn severity_from_cargo_check(level: &str) -> (Severity, Category) {
    let sev = match level.trim().to_ascii_lowercase().as_str() {
        "error" | "error: internal compiler error" | "ice" => Severity::High,
        "warn" | "warning" => Severity::Medium,
        "note" | "help" | "info" => Severity::Low,
        _ => Severity::High, // an unrecognized cargo-check diagnostic still likely blocks the build
    };
    (sev, Category::Correctness)
}

/// ruff (Python linter). ruff has no severity field; it is style/lint by nature.
/// We map by rule-code prefix where useful: `S` (flake8-bandit security) → High
/// Security; `B` (bugbear) → Medium Correctness; everything else → Low Style.
#[allow(dead_code)] // first caller is the A2 ruff runner.
pub fn severity_from_ruff(rule_code: &str) -> (Severity, Category) {
    let code = rule_code.trim();
    let upper = code.to_ascii_uppercase();
    if upper.starts_with('S') {
        (Severity::High, Category::Security)
    } else if upper.starts_with('B') || upper.starts_with("E9") || upper.starts_with('F') {
        // E9 = syntax errors, F = pyflakes (undefined name etc.), B = bugbear.
        (Severity::Medium, Category::Correctness)
    } else {
        (Severity::Low, Category::Style)
    }
}

/// bandit (Python security scanner). Native severity `LOW|MEDIUM|HIGH`. Always
/// Security category.
#[allow(dead_code)] // first caller is the A2 bandit runner.
pub fn severity_from_bandit(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_uppercase().as_str() {
        "HIGH" => Severity::High,
        "MEDIUM" => Severity::Medium,
        "LOW" => Severity::Low,
        _ => Severity::Medium,
    };
    (s, Category::Security)
}

/// vulture (Python dead-code). No severity; it reports unused code with a
/// confidence percentage. Dead code is Low severity, DeadCode category.
#[allow(dead_code)] // first caller is the A2 vulture runner.
pub fn severity_from_vulture() -> (Severity, Category) {
    (Severity::Low, Category::DeadCode)
}

/// cargo fmt (Rust formatting). Style-only check.
/// Formatting issues are Low severity, Style category.
#[allow(dead_code)] // first caller is the A3 cargo_fmt runner.
pub fn severity_from_cargo_fmt() -> (Severity, Category) {
    (Severity::Low, Category::Style)
}

/// gofmt (Go formatting). Style-only check, exactly like cargo fmt / prettier:
/// a file that is not gofmt-formatted is Low severity, Style category.
#[allow(dead_code)] // first caller is the gofmt runner.
pub fn severity_from_gofmt() -> (Severity, Category) {
    (Severity::Low, Category::Style)
}

/// go vet (Go correctness analyzer). vet flags suspicious constructs (printf
/// mismatches, lost struct tags, unreachable code). P2 ROLLOUT DISCIPLINE:
/// advisory-first — vet diagnostics are CAPPED at Medium (NOT High) until the
/// FP-rate on this repo is measured and the owner promotes it. Always Correctness
/// (vet is a correctness/idiom analyzer, not a style or security tool).
#[allow(dead_code)] // first caller is the go_vet runner.
pub fn severity_from_go_vet() -> (Severity, Category) {
    (Severity::Medium, Category::Correctness)
}

/// cppcheck (C/C++ static analyzer). cppcheck emits a per-finding severity token
/// (`error`/`warning`/`portability`/`performance`/`style`/`information`); we run the
/// curated `--enable=warning` set, so only `error`/`warning` (plus the occasional
/// `portability`/`performance` that ride along) reach us. P2 ROLLOUT DISCIPLINE:
/// advisory-first — even an `error` token is CAPPED at Medium (NOT High) until the
/// FP-rate on this repo is measured and the owner promotes it; `warning` → Medium too,
/// `portability`/`performance` → Low, anything else → Low. Always Correctness (cppcheck
/// is a correctness/bug analyzer, not a style or security tool — and the noisy `style`/
/// `information` classes are disabled at the flag level so they never poison labels).
#[allow(dead_code)] // first caller is the cppcheck runner.
pub fn severity_from_cppcheck(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_lowercase().as_str() {
        // Advisory cap: `error`/`warning` → Medium (NOT High) until FP-rate measured.
        "error" | "warning" => Severity::Medium,
        "portability" | "performance" => Severity::Low,
        _ => Severity::Low,
    };
    (s, Category::Correctness)
}

/// HTML Tidy (HTML validity checker). Tidy emits `Warning:`/`Error:` diagnostics
/// (markup validity, deprecated attributes, missing required structure). P2 ROLLOUT
/// DISCIPLINE: advisory-first — a tidy `Error` is CAPPED at Medium (NOT High) until the
/// FP-rate on this repo is measured; a `Warning` → Low. Always Correctness (tidy is a
/// markup-correctness checker, not a style or security tool).
#[allow(dead_code)] // first caller is the tidy runner.
pub fn severity_from_tidy(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_lowercase().as_str() {
        // Advisory cap: `error` → Medium (NOT High) until FP-rate measured.
        "error" => Severity::Medium,
        "warning" => Severity::Low,
        _ => Severity::Low,
    };
    (s, Category::Correctness)
}

/// ktlint (Kotlin formatter/linter). ktlint is a STYLE/formatting tool (it enforces a
/// consistent Kotlin code style — indentation, import ordering, spacing), exactly like
/// gofmt / cargo fmt / prettier: every ktlint finding is Low severity, Style category.
#[allow(dead_code)] // first caller is the ktlint runner.
pub fn severity_from_ktlint() -> (Severity, Category) {
    (Severity::Low, Category::Style)
}

/// npm audit. npm uses low|moderate|high|critical; a dependency vulnerability
/// is always Security. critical/high → High, moderate → Medium, low → Low,
/// unknown → Medium.
#[allow(dead_code)] // first caller is the A3 npm_audit runner.
pub fn severity_from_npm_audit(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_lowercase().as_str() {
        "critical" | "high" => Severity::High,
        "moderate" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Medium,
    };
    (s, Category::Security)
}

/// cargo-deny diagnostics. P2 ROLLOUT DISCIPLINE: advisory-first — `error`
/// maps to Medium (NOT High) until the FP-rate on this repo is measured and
/// the owner promotes it; everything else is Low. Always Security: the tool
/// only speaks about the dependency graph (advisories, bans, sources).
#[allow(dead_code)] // first caller is the cargo-deny runner.
pub fn severity_from_cargo_deny(sev: &str) -> (Severity, Category) {
    match sev.to_ascii_lowercase().as_str() {
        "error" => (Severity::Medium, Category::Security),
        _ => (Severity::Low, Category::Security),
    }
}

/// zizmor. zizmor uses high|medium|low|informational; CI hardening findings
/// are always Security. high → High, medium → Medium, low/informational → Low,
/// unknown → Medium.
#[allow(dead_code)] // first caller is the A3 zizmor runner.
pub fn severity_from_zizmor(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_lowercase().as_str() {
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" | "informational" => Severity::Low,
        _ => Severity::Medium,
    };
    (s, Category::Security)
}

/// Shared helper for style-only format checkers (ruff format, prettier).
/// Formatting issues are Low severity, Style category.
#[allow(dead_code)]
pub fn severity_from_format_checker() -> (Severity, Category) {
    (Severity::Low, Category::Style)
}

/// gitleaks (secret scanner). A leaked secret is always the most serious finding
/// we surface: High Security.
#[allow(dead_code)] // first caller is the A2 gitleaks runner.
pub fn gitleaks_category() -> (Severity, Category) {
    (Severity::High, Category::Security)
}

/// jscpd (copy/paste detector). Duplication is Medium Duplication.
#[allow(dead_code)] // first caller is the A2 jscpd runner.
pub fn jscpd_category() -> (Severity, Category) {
    (Severity::Medium, Category::Duplication)
}

/// lizard (cyclomatic complexity). Returns `Some((Medium, Complexity))` only
/// when the function's CCN exceeds the configured threshold; below threshold
/// there is no finding (`None`). A `threshold` of 0 is treated as "report any"
/// to avoid a divide-by-nothing surprise, but still requires ccn > 0.
#[allow(dead_code)] // first caller is the A2 lizard runner.
pub fn lizard_complexity(ccn: u32, threshold: u32) -> Option<(Severity, Category)> {
    if ccn > threshold {
        Some((Severity::Medium, Category::Complexity))
    } else {
        None
    }
}

/// eslint. Native numeric severity: 2 = error, 1 = warning, 0 = off. The JSON
/// reporter emits the integer; runners stringify it. Correctness category.
#[allow(dead_code)] // first caller is the A2 eslint runner.
pub fn severity_from_eslint(level: &str) -> (Severity, Category) {
    let sev = match level.trim().to_ascii_lowercase().as_str() {
        "2" | "error" => Severity::High,
        "1" | "warn" | "warning" => Severity::Medium,
        "0" | "off" => Severity::Low,
        _ => Severity::Medium,
    };
    (sev, Category::Correctness)
}

/// tsc (TypeScript compiler). A type error blocks the build: High Correctness.
/// tsc has no warning tier in `--noEmit` diagnostics, but `suggestion`/`message`
/// categories from the JSON-ish output map to Low.
#[allow(dead_code)] // first caller is the A2 tsc runner.
pub fn severity_from_tsc(category: &str) -> (Severity, Category) {
    let sev = match category.trim().to_ascii_lowercase().as_str() {
        "error" => Severity::High,
        "warning" => Severity::Medium,
        "suggestion" | "message" | "info" => Severity::Low,
        _ => Severity::High,
    };
    (sev, Category::Correctness)
}

/// pyright (Python type checker). Maps JSON severity strings to our severity.
#[allow(dead_code)] // first caller is the A3 pyright runner.
pub fn severity_from_pyright(severity: &str) -> (Severity, Category) {
    let sev = match severity.trim().to_ascii_lowercase().as_str() {
        "error" => Severity::High,
        "warning" => Severity::Medium,
        "information" | "info" => Severity::Low,
        _ => Severity::High,
    };
    (sev, Category::Correctness)
}

/// oxlint (Node linter). Maps message content to severity.
#[allow(dead_code)] // first caller is the A3 oxlint runner.
pub fn severity_from_oxlint(message: &str) -> (Severity, Category) {
    let sev = if message.contains("[Error") {
        Severity::High
    } else {
        Severity::Medium
    };
    (sev, Category::Correctness)
}

/// semgrep. Native severity `ERROR|WARNING|INFO`. semgrep rules are mostly
/// security/correctness; without a rule taxonomy we default the category to
/// Security (semgrep's headline use is security patterns) but let the runner
/// override per-rule later. Unknown level → Medium.
#[allow(dead_code)] // first caller is the A2 semgrep runner.
pub fn severity_from_semgrep(sev: &str) -> (Severity, Category) {
    let s = match sev.trim().to_ascii_uppercase().as_str() {
        "ERROR" => Severity::High,
        "WARNING" => Severity::Medium,
        "INFO" => Severity::Low,
        _ => Severity::Medium,
    };
    (s, Category::Security)
}

/// knip (unused files/exports/dependencies in JS/TS projects). Dead code: Low
/// DeadCode.
#[allow(dead_code)] // first caller is the A2 knip runner.
pub fn knip_category() -> (Severity, Category) {
    (Severity::Low, Category::DeadCode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clippy_levels() {
        assert_eq!(
            severity_from_clippy("deny"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_clippy("ERROR"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_clippy("warning"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_clippy("help"),
            (Severity::Low, Category::Correctness)
        );
        // unknown → Medium/Correctness, no panic.
        assert_eq!(
            severity_from_clippy("banana"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_clippy(""),
            (Severity::Medium, Category::Correctness)
        );
    }

    #[test]
    fn cargo_check_levels() {
        assert_eq!(
            severity_from_cargo_check("error"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_cargo_check("warning"),
            (Severity::Medium, Category::Correctness)
        );
        // unknown defaults High (likely blocks build).
        assert_eq!(
            severity_from_cargo_check("???"),
            (Severity::High, Category::Correctness)
        );
    }

    #[test]
    fn ruff_codes() {
        assert_eq!(
            severity_from_ruff("S105"),
            (Severity::High, Category::Security)
        );
        assert_eq!(
            severity_from_ruff("B008"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_ruff("F401"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(severity_from_ruff("E501"), (Severity::Low, Category::Style));
        assert_eq!(severity_from_ruff(""), (Severity::Low, Category::Style));
    }

    #[test]
    fn bandit_levels() {
        assert_eq!(
            severity_from_bandit("HIGH"),
            (Severity::High, Category::Security)
        );
        assert_eq!(
            severity_from_bandit("medium"),
            (Severity::Medium, Category::Security)
        );
        assert_eq!(
            severity_from_bandit("Low"),
            (Severity::Low, Category::Security)
        );
        assert_eq!(
            severity_from_bandit("???"),
            (Severity::Medium, Category::Security)
        );
    }

    #[test]
    fn vulture_is_dead_code() {
        assert_eq!(severity_from_vulture(), (Severity::Low, Category::DeadCode));
    }

    #[test]
    fn gitleaks_is_high_security() {
        assert_eq!(gitleaks_category(), (Severity::High, Category::Security));
    }

    #[test]
    fn jscpd_is_duplication() {
        assert_eq!(jscpd_category(), (Severity::Medium, Category::Duplication));
    }

    #[test]
    fn lizard_threshold() {
        assert_eq!(
            lizard_complexity(20, 15),
            Some((Severity::Medium, Category::Complexity))
        );
        assert_eq!(lizard_complexity(15, 15), None);
        assert_eq!(lizard_complexity(10, 15), None);
        // threshold 0 reports anything above 0.
        assert_eq!(
            lizard_complexity(1, 0),
            Some((Severity::Medium, Category::Complexity))
        );
        assert_eq!(lizard_complexity(0, 0), None);
    }

    #[test]
    fn eslint_levels() {
        assert_eq!(
            severity_from_eslint("2"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_eslint("error"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_eslint("1"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_eslint("0"),
            (Severity::Low, Category::Correctness)
        );
        assert_eq!(
            severity_from_eslint("9"),
            (Severity::Medium, Category::Correctness)
        );
    }

    #[test]
    fn tsc_levels() {
        assert_eq!(
            severity_from_tsc("error"),
            (Severity::High, Category::Correctness)
        );
        assert_eq!(
            severity_from_tsc("suggestion"),
            (Severity::Low, Category::Correctness)
        );
        // unknown → High (type errors block).
        assert_eq!(
            severity_from_tsc("???"),
            (Severity::High, Category::Correctness)
        );
    }

    #[test]
    fn semgrep_levels() {
        assert_eq!(
            severity_from_semgrep("ERROR"),
            (Severity::High, Category::Security)
        );
        assert_eq!(
            severity_from_semgrep("warning"),
            (Severity::Medium, Category::Security)
        );
        assert_eq!(
            severity_from_semgrep("INFO"),
            (Severity::Low, Category::Security)
        );
        assert_eq!(
            severity_from_semgrep("???"),
            (Severity::Medium, Category::Security)
        );
    }

    #[test]
    fn knip_is_dead_code() {
        assert_eq!(knip_category(), (Severity::Low, Category::DeadCode));
    }

    #[test]
    fn gofmt_is_style_low() {
        assert_eq!(severity_from_gofmt(), (Severity::Low, Category::Style));
    }

    #[test]
    fn go_vet_is_correctness_capped_at_medium() {
        // Advisory-first: never High until FP-rate is measured.
        assert_eq!(
            severity_from_go_vet(),
            (Severity::Medium, Category::Correctness)
        );
    }

    #[test]
    fn cppcheck_is_correctness_advisory_capped_at_medium() {
        // Advisory-first: even an `error` token is capped at Medium (never High) until
        // the FP-rate is measured. Always Correctness.
        assert_eq!(
            severity_from_cppcheck("error"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_cppcheck("warning"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_cppcheck("portability"),
            (Severity::Low, Category::Correctness)
        );
        assert_eq!(
            severity_from_cppcheck("performance"),
            (Severity::Low, Category::Correctness)
        );
        // Case-insensitive; unknown/empty → Low (never crash, never escalate).
        assert_eq!(
            severity_from_cppcheck("ERROR"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_cppcheck("???"),
            (Severity::Low, Category::Correctness)
        );
    }

    #[test]
    fn tidy_is_correctness_advisory_capped_at_medium() {
        // Advisory-first: an `Error` token is capped at Medium (never High); a `Warning`
        // → Low. Always Correctness. Case-insensitive; unknown/empty → Low.
        assert_eq!(
            severity_from_tidy("Error"),
            (Severity::Medium, Category::Correctness)
        );
        assert_eq!(
            severity_from_tidy("Warning"),
            (Severity::Low, Category::Correctness)
        );
        assert_eq!(
            severity_from_tidy("WARNING"),
            (Severity::Low, Category::Correctness)
        );
        assert_eq!(
            severity_from_tidy("???"),
            (Severity::Low, Category::Correctness)
        );
    }

    #[test]
    fn ktlint_is_style_low() {
        // ktlint is a formatting/style tool, like gofmt/cargo-fmt/prettier.
        assert_eq!(severity_from_ktlint(), (Severity::Low, Category::Style));
    }
}
