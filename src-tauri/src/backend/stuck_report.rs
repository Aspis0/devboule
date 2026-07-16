//! Structured "stuck" report for a mini that exhausted its budget (timeout / failed /
//! loop). Gives the human an actionable record instead of a bare block string. Pure data
//! + formatting — no I/O, no deps beyond serde. See v6 Phase 5.

use serde::{Deserialize, Serialize};

/// Max chars of the mini's last output to keep in the report (keeps events/state small).
const MAX_OUTPUT_EXCERPT: usize = 800;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StuckReport {
    pub task_id: String,
    /// The agent/coder that spawned the stuck mini — needed so the frontend can route the
    /// report to the correct console panel.
    pub agent_id: String,
    /// Why it's stuck: "timeout" | "failed" | "loop" (free string, caller-provided).
    pub reason: String,
    pub attempts: u32,
    /// A bounded TAIL excerpt of the mini's last output (most-recent chars are the useful ones).
    pub last_output: String,
    pub files_touched: Vec<String>,
    /// The project this mini belongs to. The frontend uses this to show only the reports
    /// for the currently-open project, avoiding cross-project noise.
    pub project_id: Option<String>,
}

impl StuckReport {
    /// Build a report. `raw_output` is truncated to the LAST `MAX_OUTPUT_EXCERPT` chars
    /// on a valid UTF-8 char boundary (never panics on multi-byte input).
    pub fn new(
        task_id: impl Into<String>,
        agent_id: impl Into<String>,
        reason: impl Into<String>,
        attempts: u32,
        raw_output: &str,
        files_touched: Vec<String>,
        project_id: Option<String>,
    ) -> Self {
        let n = raw_output.chars().count();
        let last_output = if n <= MAX_OUTPUT_EXCERPT {
            raw_output.to_string()
        } else {
            raw_output.chars().skip(n - MAX_OUTPUT_EXCERPT).collect()
        };

        StuckReport {
            task_id: task_id.into(),
            agent_id: agent_id.into(),
            reason: reason.into(),
            attempts,
            last_output,
            files_touched,
            project_id,
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
        let report = StuckReport::new("T1", "agent-1", "timeout", 2, &long, vec![], Some("p1".into()));
        assert!(report.last_output.len() <= MAX_OUTPUT_EXCERPT);
        assert_eq!(report.last_output.len(), MAX_OUTPUT_EXCERPT);
        // The tail should be the last 800 chars, all 'x'.
        assert_eq!(report.last_output, "x".repeat(MAX_OUTPUT_EXCERPT));
    }

    #[test]
    fn new_short_output_kept_whole() {
        let short = "hello";
        let report = StuckReport::new("T2", "agent-1", "failed", 1, short, vec![], None);
        assert_eq!(report.last_output, short);
    }

    #[test]
    fn new_multibyte_utf8_no_panic() {
        let utf8: String = "日本語".repeat(500);
        let report = StuckReport::new("T3", "agent-1", "loop", 3, &utf8, vec![], Some("p1".into()));
        // Should not panic; char count must be on a valid boundary.
        assert_eq!(report.last_output.chars().count(), MAX_OUTPUT_EXCERPT);
    }

    #[test]
    fn human_summary_contains_task_id_and_reason() {
        let report = StuckReport::new("T42", "agent-1", "timeout", 5, "", vec!["a.rs".into(), "b.rs".into()], Some("p1".into()));
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
        let report = StuckReport::new("T99", "agent-1", "failed", 1, "", vec![], None);
        let summary = report.human_summary();
        assert!(summary.len() >= 12, "summary must be at least 12 chars, got: {}", summary);
        assert!(!summary.contains("touched:"));
    }

    #[test]
    fn json_round_trip_preserves_camel_case_keys() {
        // Build a report with every field set, serialize to camelCase JSON, then
        // deserialize back and assert equality (the PartialEq derive we added).
        let report = StuckReport::new(
            "task-7",
            "agent-x",
            "timeout",
            3,
            "some output excerpt",
            vec!["a.rs".into(), "b.rs".into()],
            Some("proj-1".into()),
        );
        let json = serde_json::to_string(&report).unwrap();
        // camelCase keys must appear (rename_all = "camelCase").
        assert!(json.contains("\"taskId\""), "json: {json}");
        assert!(json.contains("\"agentId\""), "json: {json}");
        assert!(json.contains("\"reason\""), "json: {json}");
        assert!(json.contains("\"attempts\""), "json: {json}");
        assert!(json.contains("\"lastOutput\""), "json: {json}");
        assert!(json.contains("\"filesTouched\""), "json: {json}");
        assert!(json.contains("\"projectId\""), "json: {json}");
        // snake_case must NOT leak.
        assert!(!json.contains("task_id"), "snake leaked: {json}");
        assert!(!json.contains("agent_id"), "snake leaked: {json}");
        // Round-trip must reconstruct the exact struct (PartialEq).
        let back: StuckReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back, "round-trip must be byte-identical via PartialEq");
        // And the values must match individually (sanity).
        assert_eq!(back.task_id, "task-7");
        assert_eq!(back.agent_id, "agent-x");
        assert_eq!(back.reason, "timeout");
        assert_eq!(back.attempts, 3);
        assert_eq!(back.last_output, "some output excerpt");
        assert_eq!(back.files_touched, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(back.project_id, Some("proj-1".into()));
    }

    #[test]
    fn json_omits_stuck_report_when_none() {
        // A directive whose stuck_report is None must NOT serialize the key
        // (skip_serializing_if = "Option::is_none"), preserving wire compat.
        use crate::backend::mini_coder::MiniCoderDirective;
        let d = MiniCoderDirective {
            id: "d-1".into(),
            parent_agent_id: "coder-1".into(),
            status: crate::backend::mini_coder::MiniCoderStatus::Pending,
            task: "t".into(),
            files: vec!["src/a.rs".into()],
            backend: None,
            write: false,
            write_mode: crate::backend::mini_coder::WriteMode::EmitEdits,
            tier: Default::default(),
            project_id: None,
            allow_oracle: false,
            kill_requested: false,
            steer_queue: Vec::new(),
            result_path: "mini/d-1.json".into(),
            agent_id: None,
            created_at: "2026-07-15T00:00:00Z".into(),
            claimed_at: None,
            scratch_path: None,
            started_at: None,
            result: None,
            stuck_report: None,
            censor_summary: None,
            attempt: 0,
            parent_directive_id: None,
            pigeon_ticket: None,
            collected: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains("stuckReport"),
            "None stuck_report must be omitted from JSON: {json}"
        );
    }
}
