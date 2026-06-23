use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DesignRequestStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
    Timeout,
}

impl DesignRequestStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Timeout)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignRequestOutcome {
    pub status: DesignRequestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_project_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DesignRequestOutcome {
    pub fn done(design_project_path: impl Into<String>, registry_id: impl Into<String>) -> Self {
        Self {
            status: DesignRequestStatus::Done,
            design_project_path: Some(design_project_path.into()),
            registry_id: Some(registry_id.into()),
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: DesignRequestStatus::Failed,
            design_project_path: None,
            registry_id: None,
            error: Some(error.into()),
        }
    }
    // (No Rust `timeout()` constructor: the Python MCP dispatch writes the "timeout"
    // status into the directive JSON directly; Rust only deserializes that variant.)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesignRequestDirective {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_agent_id: String,
    #[serde(default)]
    pub status: DesignRequestStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_context: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<DesignRequestOutcome>,
}

pub fn apply_claim(
    d: &DesignRequestDirective,
    claimed_at: impl Into<String>,
) -> Result<DesignRequestDirective, String> {
    if d.status != DesignRequestStatus::Pending {
        return Err("Directive is not Pending".to_string());
    }
    let mut new_d = d.clone();
    new_d.status = DesignRequestStatus::Running;
    new_d.claimed_at = Some(claimed_at.into());
    Ok(new_d)
}

pub fn apply_result(
    d: &DesignRequestDirective,
    outcome: DesignRequestOutcome,
) -> Result<DesignRequestDirective, String> {
    if d.status != DesignRequestStatus::Running {
        return Err("Directive is not Running".to_string());
    }
    let mut new_d = d.clone();
    new_d.status = outcome.status.clone();
    new_d.result = Some(outcome);
    Ok(new_d)
}

// NOTE: abandoned-directive timeout/eviction is handled by the Python MCP dispatch's
// synthesized write-back (the orchestrator's blocking poll times out and stamps the
// directive failed), so no Rust scan-pass / pass-plan is needed here.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_then_result_transitions() {
        let d = DesignRequestDirective {
            id: "d1".into(),
            parent_agent_id: "orch-1".into(),
            status: DesignRequestStatus::Pending,
            prompt: "a billing dashboard".into(),
            ..Default::default()
        };
        // Only Pending can be claimed.
        let running = apply_claim(&d, "2026-06-23T00:00:01Z").expect("claim a pending directive");
        assert_eq!(running.status, DesignRequestStatus::Running);
        assert_eq!(running.claimed_at.as_deref(), Some("2026-06-23T00:00:01Z"));
        assert!(apply_claim(&running, "t").is_err(), "cannot re-claim a running directive");

        // Only Running can receive a result.
        let done = apply_result(&running, DesignRequestOutcome::done("/proj/.design/d1", "reg-9"))
            .expect("result on a running directive");
        assert_eq!(done.status, DesignRequestStatus::Done);
        assert_eq!(
            done.result.as_ref().unwrap().design_project_path.as_deref(),
            Some("/proj/.design/d1")
        );
        assert!(
            apply_result(&done, DesignRequestOutcome::failed("x")).is_err(),
            "cannot result a non-running directive"
        );
    }

    #[test]
    fn directive_round_trips_camel_case() {
        let d = DesignRequestDirective {
            id: "d1".into(),
            parent_agent_id: "orch-1".into(),
            status: DesignRequestStatus::Pending,
            prompt: "home page".into(),
            plan_context: Some("task 5: dashboard".into()),
            result_path: "d1.json".into(),
            created_at: "2026-06-23T00:00:00Z".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"parentAgentId\""), "camelCase field: {json}");
        assert!(json.contains("\"planContext\""), "camelCase field: {json}");
        let back: DesignRequestDirective = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
}
