// TDD-strict mode — the orchestrator hands the mini a FAILING test and says "make it green".
// THE ANTI-CHEAT IS IN CODE, NOT THE PROMPT (per the owner's hard requirement):
//   (1) the test file is IMMUTABLE to the mini — the orchestrator EXCLUDES it from the directive
//       allowlist (so `apply_emitted_edits` already rejects edits to it), and `assert_test_untouched`
//       is a defense-in-depth guard that never relies on that single layer;
//   (2) `detect_test_gaming` flags emitted edits that try to NEUTER the test (skip/ignore/xfail,
//       trivial always-true assertions, or tampering with test-infrastructure files) instead of
//       satisfying it;
//   (3) `evaluate_gate` passes ONLY when the test went red→green AND no gaming was detected.
// Live test EXECUTION (running the command, capturing red/green) is wired by the executor and is
// GPU-deferred e2e; THIS module is the pure, fully unit-tested decision logic.

/// The subset of an emitted edit this module reasons about (path + the content being written).
pub struct EditView<'a> {
    pub path: &'a str,
    pub new_string: &'a str,
}

/// Defense-in-depth: an edit whose path resolves to the protected test file is a HARD violation
/// (the directive allowlist already excludes the test, but never trust a single layer). Paths are
/// compared after `\\`→`/` normalization. Returns Err naming the offending path; Ok(()) if clean.
/// Normalize a relative path for comparison: `\\`→`/`, then drop empty and `.` segments so
/// `./tests/x`, `tests//x`, `tests/./x` (and backslash variants) all compare equal to `tests/x`.
fn normalize_rel(p: &str) -> String {
    p.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

pub fn assert_test_untouched(edits: &[EditView], protected_test_path: &str) -> Result<(), String> {
    let protected = normalize_rel(protected_test_path);
    for edit in edits {
        if normalize_rel(edit.path) == protected {
            return Err(format!(
                "TDD-strict: the failing test '{protected_test_path}' is immutable — edit rejected"
            ));
        }
    }
    Ok(())
}

/// Scan emitted edits for attempts to GAME the failing test rather than make the code satisfy it.
/// Returns human-readable findings (empty = clean). Detects, in ANY edited file:
///   - skip/ignore/xfail markers (`#[ignore]`, `.skip(`, `it.skip`, `xfail`, `@pytest.mark.skip`, …);
///   - trivial always-true assertions (`assert!(true)`, `assert(true)`, `expect(true).toBe(true)`);
///   - edits to test-infrastructure files (conftest.py, build.rs, jest/vitest config, setup files)
///     that could neuter the handed-in test from outside it.
pub fn detect_test_gaming(edits: &[EditView]) -> Vec<String> {
    let mut findings = Vec::new();

    let skip_markers = [
        "#[ignore",                 // prefix: catches `#[ignore]` AND `#[ignore = "reason"]`
        "cfg_attr(test, ignore)",
        "it.skip",
        "describe.skip",
        "test.skip(",
        "xit(",
        "xdescribe(",
        "xtest(",
        "it.only",
        "describe.only",
        "test.only",
        "@pytest.mark.skip",
        "@pytest.mark.xfail",
        "pytest.mark.xfail",
        "@unittest.skip",
        "t.Skip(",                  // Go
        "@Disabled",                // JUnit5 / Kotlin
        "@Ignore",                  // JUnit4
    ];
    let trivial_assertions = [
        "assert!(true)",
        "assert(true)",
        "assert True",
        "expect(true).toBe(true)",
    ];
    let infra_names = ["conftest.py", "build.rs", "pytest.ini", "setup.cfg"];
    let infra_prefixes = ["jest.config", "vitest.config", "karma.conf", ".mocharc"];

    for edit in edits {
        let new_string = edit.new_string;
        let path = edit.path;

        // Check skip/ignore markers
        if skip_markers.iter().any(|marker| new_string.contains(marker)) {
            findings.push(format!("skip/ignore marker in {path}"));
        }

        // Check trivial always-true assertions
        if trivial_assertions.iter().any(|assertion| new_string.contains(assertion)) {
            findings.push(format!("trivial always-true assertion in {path}"));
        }

        // Check test-infrastructure files
        let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let is_infra = infra_names.contains(&file_name)
            || infra_prefixes.iter().any(|prefix| file_name.starts_with(prefix));

        if is_infra {
            findings.push(format!("test-infrastructure file tampered: {path}"));
        }
    }

    findings
}

/// The project's test command for a language (devboule is multi-language). None if unknown.
pub fn test_command_for(language: &str) -> Option<Vec<&'static str>> {
    match language.to_lowercase().as_str() {
        "rust" => Some(vec!["cargo", "test"]),
        "python" | "py" => Some(vec!["python", "-m", "pytest"]),
        "node" | "javascript" | "typescript" | "js" | "ts" => Some(vec!["npm", "test"]),
        "go" => Some(vec!["go", "test", "./..."]),
        "cpp" | "c++" => Some(vec!["ctest"]),
        "kotlin" => Some(vec!["gradle", "test"]),
        _ => None,
    }
}

/// Outcome of the TDD-strict gate.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TddGate {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// The gate. Passes ONLY if the test was RED before the edits (a genuine failing test), GREEN after,
/// and no gaming was detected. `was_red`/`is_green` come from the executor running the test.
pub fn evaluate_gate(was_red: bool, is_green: bool, gaming: &[String]) -> TddGate {
    let mut reasons = Vec::new();

    if !was_red {
        reasons.push("the test did not fail before the change (not a genuine RED test)".to_string());
    }
    if !is_green {
        reasons.push("the test does not pass (GREEN) after the change".to_string());
    }
    if !gaming.is_empty() {
        reasons.push("test-gaming detected".to_string());
        reasons.extend(gaming.iter().cloned());
    }

    let passed = reasons.is_empty();

    TddGate { passed, reasons }
}

#[cfg(test)]
mod tdd_strict_tests {
    use super::*;

    fn e<'a>(path: &'a str, new_string: &'a str) -> EditView<'a> {
        EditView { path, new_string }
    }

    // ---------- (1) test immutability — defense in depth ----------
    #[test]
    fn editing_the_protected_test_is_rejected() {
        let edits = [e("src/lib.rs", "fn f() {}"), e("tests/feature_test.rs", "// tampered")];
        assert!(assert_test_untouched(&edits, "tests/feature_test.rs").is_err());
    }

    #[test]
    fn editing_the_protected_test_via_backslash_path_is_rejected() {
        let edits = [e("tests\\feature_test.rs", "// tampered")];
        assert!(assert_test_untouched(&edits, "tests/feature_test.rs").is_err());
    }

    #[test]
    fn editing_the_protected_test_via_path_aliases_is_rejected() {
        // The guard must normalize away `./`, `//`, and `/./` so a cosmetic alias can't slip an
        // edit to the immutable test past it (consistent with the executor's path normalization).
        for variant in [
            "./tests/feature_test.rs",
            "tests//feature_test.rs",
            "tests/./feature_test.rs",
        ] {
            assert!(
                assert_test_untouched(&[e(variant, "x")], "tests/feature_test.rs").is_err(),
                "alias {variant:?} should be rejected"
            );
        }
    }

    #[test]
    fn edits_that_avoid_the_test_are_allowed() {
        let edits = [e("src/lib.rs", "fn f() -> i32 { 42 }")];
        assert!(assert_test_untouched(&edits, "tests/feature_test.rs").is_ok());
    }

    // ---------- (2) gaming detection ----------
    #[test]
    fn flags_rust_ignore_attribute() {
        let g = detect_test_gaming(&[e("src/lib.rs", "#[ignore]\nfn helper() {}")]);
        assert!(!g.is_empty(), "expected an #[ignore] finding");
    }

    #[test]
    fn flags_ignore_with_reason() {
        // `#[ignore = "..."]` (Rust 1.74+) must be caught by a prefix match, not exact `#[ignore]`.
        assert!(!detect_test_gaming(&[e("src/lib.rs", "#[ignore = \"make it green\"]")]).is_empty());
        assert!(!detect_test_gaming(&[e("src/lib.rs", "#[cfg_attr(test, ignore)]")]).is_empty());
    }

    #[test]
    fn flags_skip_markers_across_languages() {
        for marker in [
            "it.skip(\"x\"",
            "describe.skip(",
            "@pytest.mark.skip",
            "@pytest.mark.xfail",
            "test.skip(",
            "xit(\"x\"",
            "xdescribe(",
            "it.only(\"x\"",
            "describe.only(",
            "t.Skip()",   // Go
            "@Disabled",  // JUnit5 / Kotlin
            "@Ignore",    // JUnit4
        ] {
            let g = detect_test_gaming(&[e("src/x.ts", marker)]);
            assert!(!g.is_empty(), "expected a finding for marker {marker:?}");
        }
    }

    #[test]
    fn does_not_flag_legit_iterator_skip_or_xfail_identifier() {
        // FALSE-POSITIVE GUARD: `.skip(` is a common Rust iterator method; `xfail` appears in
        // legitimate identifiers. Clean implementation code must NOT be flagged as gaming.
        assert!(detect_test_gaming(&[e("src/lib.rs", "let rest: Vec<_> = items.iter().skip(2).collect();")]).is_empty());
        assert!(detect_test_gaming(&[e("src/lib.rs", "let xfailures = 0; // count")]).is_empty());
        assert!(detect_test_gaming(&[e("src/lib.rs", "fn only(&self) -> bool { true }")]).is_empty());
    }

    #[test]
    fn flags_trivial_always_true_assertions() {
        assert!(!detect_test_gaming(&[e("src/lib.rs", "assert!(true);")]).is_empty());
        assert!(!detect_test_gaming(&[e("a.py", "assert True")]).is_empty());
        assert!(!detect_test_gaming(&[e("a.ts", "expect(true).toBe(true)")]).is_empty());
    }

    #[test]
    fn flags_test_infrastructure_tampering() {
        for f in ["conftest.py", "build.rs", "vitest.config.ts", "jest.config.js"] {
            let g = detect_test_gaming(&[e(f, "// anything")]);
            assert!(!g.is_empty(), "expected a finding for infra file {f}");
        }
    }

    #[test]
    fn clean_implementation_edit_is_not_flagged() {
        let g = detect_test_gaming(&[e("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }")]);
        assert!(g.is_empty(), "clean edit should not be flagged: {g:?}");
    }

    // ---------- (3) multi-language test command ----------
    #[test]
    fn resolves_test_commands_per_language() {
        assert!(test_command_for("rust").is_some());
        assert!(test_command_for("python").is_some());
        assert!(test_command_for("node").is_some());
        assert!(test_command_for("go").is_some());
        assert!(test_command_for("brainfuck").is_none());
    }

    // ---------- (4) the gate ----------
    #[test]
    fn gate_passes_only_on_red_then_green_and_no_gaming() {
        assert!(evaluate_gate(true, true, &[]).passed);
    }

    #[test]
    fn gate_fails_when_test_was_never_red() {
        let r = evaluate_gate(false, true, &[]);
        assert!(!r.passed);
        assert!(r.reasons.iter().any(|s| s.to_lowercase().contains("red")));
    }

    #[test]
    fn gate_fails_when_test_is_not_green_after() {
        let r = evaluate_gate(true, false, &[]);
        assert!(!r.passed);
        assert!(r.reasons.iter().any(|s| s.to_lowercase().contains("green") || s.to_lowercase().contains("pass")));
    }

    #[test]
    fn gate_fails_when_gaming_detected_even_if_green() {
        let r = evaluate_gate(true, true, &["added #[ignore]".to_string()]);
        assert!(!r.passed);
        assert!(!r.reasons.is_empty());
    }
}
