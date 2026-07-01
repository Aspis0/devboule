//! Structured "stuck" report for a mini that exhausted its budget (timeout / failed /
//! loop). Gives the human an actionable record instead of a bare block string. Pure data
//! + formatting — no I/O, no deps beyond serde. See v6 Phase 5.

use serde::Serialize;

/// Max chars of the mini's last output to keep in the report (keeps events/state small).
const MAX_OUTPUT_EXCERPT: usize = 800;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckReport {
    pub task_id: String,
    /// Why it's stuck: "timeout" | "failed" | "loop" (free string, caller-provided).
    pub reason: String,
    pub attempts: u32,
    /// A bounded TAIL excerpt of the mini's last output (most-recent chars are the useful ones).
    pub last_output: String,
    pub files_touched: Vec<String>,
}

impl StuckReport {
    /// Build a report. `raw_output` is truncated to the LAST `MAX_OUTPUT_EXCERPT` chars
    /// on a valid UTF-8 char boundary (never panics on multi-byte input).
    pub fn new(
        task_id: impl Into<String>,
        reason: impl Into<String>,
        attempts: u32,
        raw_output: &str,
        files_touched: Vec<String>,
    ) -> Self {
        let n = raw_output.chars().count();
        let last_output = if n <= MAX_OUTPUT_EXCERPT {
            raw_output.to_string()
        } else {
            raw_output.chars().skip(n - MAX_OUTPUT_EXCERPT).collect()
        };

        StuckReport {
            task_id: task_id.into(),
            reason: reason.into(),
            attempts,
            last_output,
            files_touched,
        }
    }

    /// A concise one-line human summary, ALWAYS at least 12 chars (a caller uses it as
    /// evidence text). Example: "mini T3 stuck after 2 attempt(s) (timeout); touched: a.rs, b.rs".
    pub fn human_summary(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("mini {} stuck after {} attempt(s) ({})", self.task_id, self.attempts, self.reason));
        if !self.files_touched.is_empty() {
            parts.push(format!("touched: {}", self.files_touched.join(", ")));
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_truncates_long_output_to_tail() {
        let long = "x".repeat(1500);
        let report = StuckReport::new("T1", "timeout", 2, &long, vec![]);
        assert!(report.last_output.len() <= MAX_OUTPUT_EXCERPT);
        assert_eq!(report.last_output.len(), MAX_OUTPUT_EXCERPT);
        // The tail should be the last 800 chars, all 'x'.
        assert_eq!(report.last_output, "x".repeat(MAX_OUTPUT_EXCERPT));
    }

    #[test]
    fn new_short_output_kept_whole() {
        let short = "hello";
        let report = StuckReport::new("T2", "failed", 1, short, vec![]);
        assert_eq!(report.last_output, short);
    }

    #[test]
    fn new_multibyte_utf8_no_panic() {
        let utf8: String = "日本語".repeat(500);
        let report = StuckReport::new("T3", "loop", 3, &utf8, vec![]);
        // Should not panic; char count must be on a valid boundary.
        assert_eq!(report.last_output.chars().count(), MAX_OUTPUT_EXCERPT);
    }

    #[test]
    fn human_summary_contains_task_id_and_reason() {
        let report = StuckReport::new("T42", "timeout", 5, "", vec!["a.rs".into(), "b.rs".into()]);
        let summary = report.human_summary();
        assert!(summary.len() >= 12, "summary must be at least 12 chars, got: {}", summary);
        assert!(summary.contains("T42"));
        assert!(summary.contains("timeout"));
        assert!(summary.contains("touched:"));
        assert!(summary.contains("a.rs"));
        assert!(summary.contains("b.rs"));
    }

    #[test]
    fn human_summary_empty_files_touched_omits_touched() {
        let report = StuckReport::new("T99", "failed", 1, "", vec![]);
        let summary = report.human_summary();
        assert!(summary.len() >= 12, "summary must be at least 12 chars, got: {}", summary);
        assert!(!summary.contains("touched:"));
    }
}
