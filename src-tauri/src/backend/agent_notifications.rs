use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::backend::state::BackendState;

const STATE_FILE: &str = "agent-notifications.json";
const MAX_AGENT_RECORDS: usize = 500;
const MAX_SINCE_CHARS: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNotificationState {
    #[serde(default)]
    pub prev_since_by_agent: BTreeMap<String, String>,
}

#[tauri::command]
pub fn read_agent_notification_state(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
) -> Result<AgentNotificationState, String> {
    state.ensure_unlocked()?;
    Ok(read_state_file(&state_path(&app)?))
}

#[tauri::command]
pub fn write_agent_notification_state(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
    value: AgentNotificationState,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    write_state_file(&state_path(&app)?, &normalize_state(value))
}

fn state_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "App config directory is unavailable.".to_string())?;
    Ok(dir.join(STATE_FILE))
}

fn read_state_file(path: &Path) -> AgentNotificationState {
    let Ok(raw) = fs::read_to_string(path) else {
        return AgentNotificationState::default();
    };
    serde_json::from_str::<AgentNotificationState>(&raw)
        .map(normalize_state)
        .unwrap_or_default()
}

fn write_state_file(path: &Path, value: &AgentNotificationState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create notification state directory: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("Could not serialize notification state: {e}"))?;
    fs::write(&tmp, body).map_err(|e| format!("Could not write notification state: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not replace notification state: {e}")
    })
}

fn normalize_state(mut value: AgentNotificationState) -> AgentNotificationState {
    value.prev_since_by_agent = value
        .prev_since_by_agent
        .into_iter()
        .filter_map(|(agent, since)| {
            let agent = clean_agent_id(&agent)?;
            let since = clean_since(&since)?;
            Some((agent, since))
        })
        .take(MAX_AGENT_RECORDS)
        .collect();
    value
}

fn clean_agent_id(value: &str) -> Option<String> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn clean_since(value: &str) -> Option<String> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_SINCE_CHARS)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_path(label: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("aspis-agent-notify-{label}-{}-{n}", std::process::id()))
            .join(STATE_FILE)
    }

    #[test]
    fn read_state_file_is_corrupt_tolerant() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();

        assert_eq!(read_state_file(&path), AgentNotificationState::default());
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn write_state_file_round_trips_normalized_state() {
        let path = temp_path("roundtrip");
        let mut state = AgentNotificationState::default();
        state.prev_since_by_agent.insert(" agent-1 ".into(), " t1 ".into());
        state.prev_since_by_agent.insert("bad\nid".into(), " ".into());

        write_state_file(&path, &normalize_state(state)).unwrap();
        let read = read_state_file(&path);

        assert_eq!(read.prev_since_by_agent.get("agent-1").map(String::as_str), Some("t1"));
        assert_eq!(read.prev_since_by_agent.len(), 1);
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
