//! User-curated model registry (Phase 3), persisted in config.json under "modelRegistry".
//! The main coder reads this list to pick which local model to use per role + with which
//! tuned sampling params. Persistence mirrors `projects::set_custom_agent_clients`
//! (config_write_lock + read-modify-write + atomic temp+rename).

use std::fs;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::backend::fs_replace::replace_file_with_backup;
use crate::backend::projects::{config_write_lock, locate_config_path};
use crate::backend::state::BackendState;

/// One curated model the coders may choose from. `tier` selects execution mode
/// (`agentic` = >20B tool-loop, `emitEdits` = one-shot). Sampling params are the
/// per-model tuned values (bake-off: temp 0.6 / top_p 0.95 / top_k 20 / thinking_budget).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRegistryEntry {
    pub id: String,
    pub backend: String,
    pub size_bytes: u64,
    pub tier: String,
    pub roles: Vec<String>,
    pub enabled: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub thinking_budget: Option<u32>,
}

const MAX_REGISTRY_ENTRIES: usize = 64;

/// Validate + normalize: reject bad id/backend/tier, dedupe by (backend,id), drop unknown
/// roles, cap the list. Returns the normalized list.
pub fn validate_model_registry(
    entries: &[ModelRegistryEntry],
) -> Result<Vec<ModelRegistryEntry>, String> {
    let allowed_backends = ["omlx", "ollama"];
    let allowed_tiers = ["agentic", "emitEdits"];
    let allowed_roles = ["mainCoder", "miniCoder", "censor"];

    let mut seen = std::collections::HashSet::new();
    let mut validated: Vec<ModelRegistryEntry> = Vec::new();

    for entry in entries {
        if entry.id.trim().is_empty() {
            return Err("Model ID cannot be empty.".to_string());
        }
        if !allowed_backends.contains(&entry.backend.as_str()) {
            return Err(format!(
                "Invalid backend: {}. Must be 'omlx' or 'ollama'.",
                entry.backend
            ));
        }
        if !allowed_tiers.contains(&entry.tier.as_str()) {
            return Err(format!(
                "Invalid tier: {}. Must be 'agentic' or 'emitEdits'.",
                entry.tier
            ));
        }
        // Validate tuned sampling params at the CONFIG boundary (not the inference call):
        // an out-of-range value would otherwise be forwarded verbatim to oMLX/Ollama.
        if let Some(t) = entry.temperature {
            if !(0.0..=2.0).contains(&t) {
                return Err(format!("temperature {t} out of range [0, 2]."));
            }
        }
        if let Some(p) = entry.top_p {
            if !(0.0..=1.0).contains(&p) {
                return Err(format!("top_p {p} out of range [0, 1]."));
            }
        }
        if entry.top_k == Some(0) {
            return Err("top_k must be >= 1.".to_string());
        }
        if let Some(b) = entry.thinking_budget {
            if b > 32_768 {
                return Err(format!("thinking_budget {b} exceeds maximum (32768)."));
            }
        }

        let key = (entry.backend.clone(), entry.id.clone());
        if !seen.insert(key) {
            continue; // dedupe by (backend, id), keep first
        }

        let mut new_entry = entry.clone();
        new_entry.roles = entry
            .roles
            .iter()
            .filter(|r| allowed_roles.contains(&r.as_str()))
            .cloned()
            .collect();
        validated.push(new_entry);

        if validated.len() >= MAX_REGISTRY_ENTRIES {
            break;
        }
    }

    Ok(validated)
}

#[tauri::command]
pub fn get_model_registry(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<Vec<ModelRegistryEntry>, String> {
    state.ensure_unlocked()?;
    let path =
        locate_config_path(&app).ok_or_else(|| "config.json could not be located.".to_string())?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;

    // Per-entry tolerance (mirrors read_custom_agent_clients): a missing/invalid key OR
    // a single bad stored entry must NOT nuke the whole registry — otherwise GET returns
    // empty and a subsequent SET with that empty list would wipe the user's config.
    if let Some(arr) = value.get("modelRegistry").and_then(|v| v.as_array()) {
        return Ok(arr
            .iter()
            .filter_map(|e| serde_json::from_value::<ModelRegistryEntry>(e.clone()).ok())
            .collect());
    }
    Ok(Vec::new())
}

#[tauri::command]
pub fn set_model_registry(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    entries: Vec<ModelRegistryEntry>,
) -> Result<Vec<ModelRegistryEntry>, String> {
    state.ensure_unlocked()?;
    let normalized = validate_model_registry(&entries)?;
    let path =
        locate_config_path(&app).ok_or_else(|| "config.json could not be located.".to_string())?;
    // Serialize against the other config.json savers (last-writer-wins protection).
    let _config_guard = config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;
    let raw = fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("config.json is not a JSON object.".into());
    }
    value["modelRegistry"] = serde_json::to_value(&normalized)
        .map_err(|e| format!("Could not serialize model registry: {e}"))?;
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Could not serialize config.json: {e}"))?;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = path.with_extension(format!("json.{suffix}.tmp"));
    let backup_path = path.with_extension(format!("json.{suffix}.bak"));
    fs::write(&temp_path, format!("{pretty}\n")).map_err(|e| {
        format!("Could not write config.json: {e}. In a packaged build this file is read-only.")
    })?;
    replace_file_with_backup(&temp_path, &path, &backup_path, "config.json")
        .map_err(|e| format!("{e}. In a packaged build this file is read-only."))?;
    Ok(normalized)
}
