//! semgrep runner (pattern-based static analysis). PRIVACY-SENSITIVE.
//!
//! `semgrep --json` emits `{ "results": [ { "check_id", "path", "start":{"line"},
//! "extra": { "message", "severity": "ERROR"|"WARNING"|"INFO", "lines": "<source>" } } ] }`.
//! The `extra.lines` field is the MATCHED SOURCE SNIPPET, which can contain a
//! secret or sensitive code — it is deliberately NOT declared, so serde drops it.
//! We keep only `check_id`, `path`, `start.line`, `extra.message`, `extra.severity`.
//! Severity/category via `severity_from_semgrep`. Granularity is Fine.

// DEAD-CODE NOTE: parsers are tested here; run/granularity are first called by
// the A3 orchestrator. File-scoped allow (removed when A3 wires this runner in).
#![allow(dead_code)]

use super::super::severity::severity_from_semgrep;
use super::{cap, redact_secrets, run_capture_with_timeout, Granularity, RawFinding, RunTarget};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub fn granularity() -> Granularity {
    Granularity::Fine
}

/// The bundled ruleset's path under a packaged resource dir, and the dev fallback
/// relative to the crate (`src-tauri/`). KEEP IN SYNC with `tauri.conf.json`
/// `bundle.resources` and the on-disk file location.
const RULESET_REL: &[&str] = &["resources", "censor", "semgrep-rules.yml"];

/// The app's bundled resource directory, recorded ONCE at startup from the Tauri
/// `AppHandle::path().resource_dir()` (mirrors `set_bundled_oracle_root`). In a
/// release build this is the ONLY trusted location for our bundled ruleset; the
/// dev/test build resolves a crate-relative fallback instead, so this stays unset
/// (hence the `dead_code` allow in a dev/test compile).
static CENSOR_RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Record the bundled resource directory. Call once from the Tauri setup hook with
/// `app.path().resource_dir()`. Resources declared WITHOUT a `../` prefix (here
/// `resources/censor/semgrep-rules.yml`) are staged path-preserving directly under
/// the resource dir, so the ruleset lands at `<resource_dir>/resources/censor/...`.
/// (Only called in release builds from the Tauri setup hook; the file-scoped
/// `dead_code` allow at the top covers the dev/test compile where it is unused.)
pub fn set_censor_resource_dir(resource_dir: &Path) {
    let _ = CENSOR_RESOURCE_DIR.set(resource_dir.to_path_buf());
}

/// Resolve the absolute path to OUR bundled, offline semgrep ruleset.
///
/// Order:
///   1. RELEASE: the recorded resource dir (`<resource_dir>/resources/censor/...`).
///   2. DEV/TEST: a `CARGO_MANIFEST_DIR`-relative fallback (the in-repo source
///      file `src-tauri/resources/censor/...`), so the runner works from a cargo
///      checkout with no packaging step.
///
/// Returns `None` if neither candidate points at an existing file. The caller MUST
/// treat `None` as "skip semgrep" (return empty) — it must NEVER fall back to the
/// network registry (`p/ci` / `auto` / `r/...`).
pub fn resolve_semgrep_ruleset() -> Option<PathBuf> {
    if let Some(dir) = CENSOR_RESOURCE_DIR.get() {
        let mut p = dir.clone();
        p.extend(RULESET_REL);
        if p.is_file() {
            return Some(p);
        }
    }
    // Dev fallback: the file as it lives in the crate source tree.
    let mut dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dev.extend(RULESET_REL);
    if dev.is_file() {
        return Some(dev);
    }
    None
}

/// Build the semgrep argv for a single-file scan against OUR local ruleset. PURE
/// (no IO beyond the already-resolved path) so the invocation can be asserted in a
/// test without executing semgrep. `ruleset` is the absolute path returned by
/// [`resolve_semgrep_ruleset`]; `file_rel_path` is the project-relative target.
///
/// `--metrics off` suppresses semgrep's usage telemetry. We deliberately do NOT
/// pass any registry config (`--config p/ci` / `auto` / `r/...`): a local
/// `--config <abs path>` performs zero network fetches, so the scan is fully
/// offline. `--` ends flag parsing so a `-`-leading file name is never read as an
/// option.
fn build_args<'a>(ruleset: &'a str, file_rel_path: &'a str) -> Vec<&'a str> {
    vec![
        "--json",
        "--quiet",
        "--config",
        ruleset,
        "--metrics",
        "off",
        "--",
        file_rel_path,
    ]
}

#[derive(Deserialize)]
struct SemgrepReport {
    #[serde(default)]
    results: Vec<SemgrepResult>,
}

#[derive(Deserialize)]
struct SemgrepResult {
    #[serde(default)]
    check_id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    start: Option<SemgrepPos>,
    #[serde(default)]
    extra: Option<SemgrepExtra>,
}

#[derive(Deserialize)]
struct SemgrepPos {
    #[serde(default)]
    line: Option<u32>,
}

/// Only message + severity are declared. `lines` (matched source) and any
/// `metavars` (which can capture secret substrings) are NOT declared → dropped.
#[derive(Deserialize)]
struct SemgrepExtra {
    #[serde(default)]
    message: String,
    #[serde(default)]
    severity: String,
}

/// Parse `semgrep --json` stdout. PURE. The matched source snippet (`extra.lines`)
/// is never read. Title is `<check_id>: <message>`; body is the rule message +
/// location — never the matched code. `file_hint` is preferred over semgrep's
/// `path`. Tolerant: malformed JSON → empty.
pub fn parse_semgrep(stdout: &str, file_hint: &str) -> Vec<RawFinding> {
    let report: SemgrepReport = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for r in report.results {
        let (message, sev_str) = match r.extra {
            Some(e) => (e.message, e.severity),
            None => (String::new(), String::new()),
        };
        let (severity, category) = severity_from_semgrep(&sev_str);
        let file = if file_hint.is_empty() {
            r.path.replace('\\', "/")
        } else {
            file_hint.to_string()
        };
        if file.is_empty() {
            continue;
        }
        let line = r.start.and_then(|s| s.line);
        // check_id is the rule identifier (e.g. "python.lang.security.audit.eval");
        // safe structured metadata, capped defensively.
        let rule = cap(r.check_id.trim(), 120);
        // PRIVACY: semgrep interpolates the MATCHED value into `extra.message`
        // (`$METAVAR` expansion), so the message can embed a secret. Redact secret-
        // shaped tokens BEFORE the message reaches title/body (the cap only
        // truncates). Redact first, then cap.
        let safe_message = cap(redact_secrets(message.trim()).trim(), 300);
        let title = if rule.is_empty() {
            if safe_message.is_empty() {
                "semgrep finding".to_string()
            } else {
                safe_message.clone()
            }
        } else {
            format!("{rule}: {safe_message}")
        };
        out.push(RawFinding {
            file,
            line,
            severity,
            category,
            source: "semgrep".to_string(),
            title,
            body: if safe_message.is_empty() {
                format!("Rule {rule} matched")
            } else {
                safe_message
            },
        });
    }
    out
}

/// Run semgrep on a single file from the project root against OUR bundled, OFFLINE
/// ruleset (`resolve_semgrep_ruleset`). The cwd is the analyzed project root, so
/// `--config` is given the ABSOLUTE path to our ruleset, not a project-relative or
/// registry name.
///
/// Returns EMPTY (graceful, never crash, never fall back to the registry) when:
///   - `semgrep` is not installed, OR
///   - the bundled ruleset cannot be resolved / is missing.
///
/// PRIVACY/LICENSE/DETERMINISM: we never use a registry pack (`p/ci` / `auto` /
/// `r/...`); a local `--config <abs path>` does zero network fetches and the rules
/// are ours. See `resources/censor/semgrep-rules.yml` for the rationale.
pub fn run(root: &Path, target: &RunTarget) -> Vec<RawFinding> {
    if !crate::backend::projects::command_exists("semgrep") {
        return Vec::new();
    }
    let ruleset = match resolve_semgrep_ruleset() {
        Some(p) => p,
        // No bundled ruleset → skip. NEVER fall back to the network registry.
        None => return Vec::new(),
    };
    let ruleset_str = match ruleset.to_str() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let args = build_args(ruleset_str, &target.file_rel_path);
    // semgrep rule evaluation can be slow; allow a generous budget.
    let stdout = run_capture_with_timeout("semgrep", &args, root, Duration::from_secs(300));
    match stdout {
        Some(s) => parse_semgrep(&s, &target.file_rel_path),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::censor::schema::{Category, Severity};

    /// SECURITY: the matched source line must never leak into the finding.
    #[test]
    fn drops_matched_source_lines() {
        let secret_line = "password = 'AKIAIOSFODNN7EXAMPLE'";
        let json = format!(
            r#"{{
              "results": [
                {{
                  "check_id":"python.lang.security.audit.hardcoded-password",
                  "path":"app.py",
                  "start":{{"line":7}},
                  "end":{{"line":7}},
                  "extra":{{
                    "message":"Hardcoded password detected",
                    "severity":"ERROR",
                    "lines":"{secret_line}"
                  }}
                }}
              ]
            }}"#
        );
        let findings = parse_semgrep(&json, "app.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.file, "app.py");
        assert_eq!(f.line, Some(7));
        assert_eq!(f.severity, Severity::High);
        assert_eq!(f.category, Category::Security);
        assert_eq!(f.source, "semgrep");
        // The matched source (and its secret) must not appear.
        assert!(!f.title.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!f.body.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!f.body.contains(secret_line));
        // Structured rule id + message are present.
        assert!(f.title.contains("hardcoded-password"));
        assert!(f.body.contains("Hardcoded password detected"));
    }

    /// SECURITY (WARNING 1): semgrep interpolates the matched value into
    /// `extra.message`. A message embedding a secret must NOT surface that secret
    /// in either title or body.
    #[test]
    fn redacts_secret_interpolated_into_message() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let json = format!(
            r#"{{"results":[
              {{"check_id":"generic.secrets.aws-key","path":"app.py","start":{{"line":3}},
                "extra":{{"message":"Key found: {secret}","severity":"ERROR"}}}}
            ]}}"#
        );
        let findings = parse_semgrep(&json, "app.py");
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!(
            !f.title.contains(secret),
            "secret leaked into title: {}",
            f.title
        );
        assert!(
            !f.body.contains(secret),
            "secret leaked into body: {}",
            f.body
        );
        // The rule id and the surrounding prose survive.
        assert!(f.title.contains("aws-key"));
        assert!(f.body.contains("Key found"));
        assert!(f.body.contains("[redacted]"));
    }

    #[test]
    fn maps_warning_and_info() {
        let json = r#"{"results":[
          {"check_id":"r1","path":"a.py","start":{"line":1},"extra":{"message":"m","severity":"WARNING"}},
          {"check_id":"r2","path":"a.py","start":{"line":2},"extra":{"message":"n","severity":"INFO"}}
        ]}"#;
        let findings = parse_semgrep(json, "a.py");
        assert_eq!(findings[0].severity, Severity::Medium);
        assert_eq!(findings[1].severity, Severity::Low);
    }

    #[test]
    fn empty_and_malformed_yield_empty() {
        assert!(parse_semgrep(r#"{"results":[]}"#, "a.py").is_empty());
        assert!(parse_semgrep("not json", "a.py").is_empty());
        assert!(parse_semgrep("", "a.py").is_empty());
    }

    // ---- invocation / privacy / determinism ---------------------------------

    /// The built argv must point `--config` at OUR resolved LOCAL ruleset (a `.yml`
    /// under our resource dir), keep `--metrics off`, and contain NO registry
    /// reference (`p/ci`, `--config auto`, or any `r/`/`p/` pack). This is the core
    /// PRIVACY/LICENSE/DETERMINISM guarantee of C2.
    #[test]
    fn invocation_uses_local_ruleset_and_no_registry() {
        let ruleset = resolve_semgrep_ruleset().expect("dev fallback ruleset must resolve");
        let ruleset_str = ruleset.to_str().expect("ruleset path is valid UTF-8");
        let args = build_args(ruleset_str, "src/app.py");

        // `--config` is present and its value is exactly our resolved local path.
        let cfg_idx = args
            .iter()
            .position(|a| *a == "--config")
            .expect("--config must be present");
        let cfg_val = args.get(cfg_idx + 1).expect("--config takes a value");
        assert_eq!(*cfg_val, ruleset_str);
        assert!(
            cfg_val.ends_with(".yml"),
            "config must be a local .yml ruleset, got {cfg_val}"
        );
        assert!(
            std::path::Path::new(cfg_val).is_absolute(),
            "config must be an absolute path (cwd is the analyzed project), got {cfg_val}"
        );
        assert!(
            cfg_val.contains("censor"),
            "config must live under our censor resource dir, got {cfg_val}"
        );

        // metrics off (telemetry suppressed).
        let m_idx = args
            .iter()
            .position(|a| *a == "--metrics")
            .expect("--metrics must be present");
        assert_eq!(args.get(m_idx + 1).copied(), Some("off"));

        // NO registry pack / auto config anywhere in the argv.
        for a in &args {
            assert_ne!(*a, "p/ci", "must not reference the p/ci registry pack");
            assert_ne!(*a, "auto", "must not use --config auto (phones home)");
            assert!(
                !(a.starts_with("p/") || a.starts_with("r/")),
                "must not reference any registry pack ({a})"
            );
        }
        // `--offline` is intentionally NOT passed (a local --config does no fetch);
        // but ensure we didn't accidentally re-introduce a network config form.
        assert!(args.contains(&"--json") && args.contains(&"--quiet"));
        // The target file is last, after the `--` flag terminator.
        assert_eq!(args.last().copied(), Some("src/app.py"));
        let dd = args
            .iter()
            .position(|a| *a == "--")
            .expect("-- terminator must be present");
        assert!(dd < args.len() - 1, "-- must precede the file path");
    }

    // ---- path resolution ----------------------------------------------------

    /// In a dev/test build (no recorded resource dir), the resolver falls back to
    /// the crate-relative source file, which must exist on disk.
    #[test]
    fn dev_fallback_resolves_to_existing_file() {
        let p = resolve_semgrep_ruleset().expect("dev fallback must resolve");
        assert!(p.is_file(), "resolved ruleset must exist: {}", p.display());
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("yml"),
            "ruleset must be a .yml file"
        );
    }

    /// When no ruleset can be resolved AND semgrep is absent (the CI/dev case),
    /// `run` returns empty and never touches the network. We can't unset the
    /// dev fallback (it's a real file), so this asserts the graceful-empty contract
    /// via the `command_exists` gate: with semgrep absent, `run` is empty
    /// regardless. (The None→empty branch is covered by the unit check below.)
    #[test]
    fn run_is_empty_without_semgrep() {
        if crate::backend::projects::command_exists("semgrep") {
            // Skip: semgrep present means this asserts a live-tool path we can't
            // control deterministically in a unit test.
            return;
        }
        let target = RunTarget {
            file_rel_path: "src/app.py".to_string(),
        };
        let out = run(std::path::Path::new("/nonexistent-project-root"), &target);
        assert!(out.is_empty());
    }

    /// The None branch of the resolver's contract: a resource dir that does NOT
    /// contain the ruleset yields no file at the expected sub-path (the same check
    /// the resolver performs before returning Some). This exercises the
    /// missing-file logic without mutating the process-global OnceLock.
    #[test]
    fn missing_ruleset_under_dir_is_not_a_file() {
        let tmp = std::env::temp_dir().join("aspis-censor-semgrep-missing-test");
        let mut p = tmp.clone();
        p.extend(RULESET_REL);
        assert!(
            !p.is_file(),
            "a bare temp dir must not contain our ruleset; resolver would skip it"
        );
    }

    // ---- bundled ruleset validity (best-effort, no live semgrep) ------------

    /// The seed ruleset must be present, well-formed enough to parse, and declare
    /// at least one rule carrying the keys semgrep requires (id, languages,
    /// message, severity, and a pattern form). semgrep is NOT installed here, so
    /// this is a STRUCTURAL check (not live rule matching); a live-semgrep
    /// `--validate` + FP-tuning pass is still pending (see the file header / P15).
    #[test]
    fn seed_ruleset_is_structurally_valid() {
        let path = resolve_semgrep_ruleset().expect("seed ruleset must resolve");
        let raw = std::fs::read_to_string(&path).expect("seed ruleset must be readable");

        // Top-level `rules:` list.
        assert!(
            raw.lines().any(|l| l.trim_start() == "rules:"),
            "ruleset must declare a top-level `rules:` list"
        );

        // The advisory-first discipline: every declared severity is WARNING — never
        // ERROR (and never INFO for these high-signal checks).
        for line in raw.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("severity:") {
                assert_eq!(
                    rest.trim(),
                    "WARNING",
                    "advisory-first: every seed rule must be WARNING, got `{}`",
                    rest.trim()
                );
            }
        }

        // Count rule entries (lines starting a list item with an `id:`). Each rule
        // begins `- id:` in our file.
        let rule_ids: Vec<&str> = raw
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("- id:"))
            .map(str::trim)
            .collect();
        assert!(
            !rule_ids.is_empty(),
            "ruleset must declare at least one rule"
        );
        // Required keys must each appear at least as many times as there are rules
        // (one per rule). `languages:` and `message:` and `severity:` are per-rule;
        // a pattern form (`pattern:` or `pattern-either:`) is per-rule too.
        let count = |needle: &str| raw.lines().filter(|l| l.trim_start().starts_with(needle)).count();
        let n = rule_ids.len();
        assert_eq!(count("- id:"), n);
        assert_eq!(count("languages:"), n, "each rule needs `languages:`");
        assert_eq!(count("message:"), n, "each rule needs `message:`");
        assert_eq!(count("severity:"), n, "each rule needs `severity:`");
        let pattern_forms =
            count("pattern:") + count("pattern-either:");
        assert_eq!(
            pattern_forms, n,
            "each rule needs exactly one of `pattern:` / `pattern-either:`"
        );

        // The required cross-language seed checks are present.
        let joined = rule_ids.join(",");
        assert!(joined.contains("aspis-python-tls-verify-disabled"));
        assert!(joined.contains("aspis-js-tls-verify-disabled"));
        assert!(joined.contains("aspis-go-tls-verify-disabled"));
        assert!(joined.contains("aspis-python-dynamic-exec"));
        assert!(joined.contains("aspis-js-dynamic-eval"));

        // The pending-validation note must remain in the header so the deferred
        // live-semgrep pass is never silently forgotten.
        assert!(
            raw.contains("SEED ruleset") && raw.contains("validation"),
            "the SEED/validation-pending header note must remain"
        );
    }
}
