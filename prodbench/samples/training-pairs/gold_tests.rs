// GOLD fail-to-pass tests for the `training-pairs-loader` prodbench sample (a real Devboule
// feature: the backend that reads the local .aspis-training rail so the app can show the
// self-improvement pairs). Independent ground truth; the harness strips the candidate's own
// tests and appends this module. RED at base (module absent); GREEN only when implemented.
#[cfg(test)]
mod gold_tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prodbench_tp_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create temp dir");
        d
    }

    #[test]
    fn loads_valid_pairs_and_skips_malformed_and_blank_lines() {
        let d = tmpdir("valid");
        let jsonl = concat!(
            r#"{"origin":"a","gate":"clippy","rejected":"x","chosen":"y","judge_free":true,"scorer":"clippy"}"#,
            "\n",
            "this is not json\n",
            r#"{"origin":"b","judge_free":false}"#,
            "\n",
            "\n",
        );
        std::fs::write(d.join("r.jsonl"), jsonl).expect("write");
        let pairs = load_training_pairs(&d);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(pairs.len(), 2, "malformed + blank lines must be skipped");
        let a = pairs.iter().find(|p| p.origin == "a").expect("pair a");
        assert!(a.judge_free && a.gate == "clippy" && a.chosen == "y");
        let b = pairs.iter().find(|p| p.origin == "b").expect("pair b");
        assert!(!b.judge_free, "missing judge_free must default to false");
    }

    #[test]
    fn ignores_non_jsonl_files() {
        let d = tmpdir("nonjsonl");
        std::fs::write(d.join("notes.txt"), r#"{"origin":"ignore"}"#).expect("write txt");
        std::fs::write(d.join("p.jsonl"), "{\"origin\":\"keep\"}\n").expect("write jsonl");
        let pairs = load_training_pairs(&d);
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].origin, "keep");
    }

    #[test]
    fn missing_dir_yields_empty_without_panic() {
        let d = std::env::temp_dir().join(format!("prodbench_tp_{}_absent", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert!(load_training_pairs(&d).is_empty());
    }
}
