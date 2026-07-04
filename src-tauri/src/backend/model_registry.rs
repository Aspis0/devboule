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
use crate::backend::provider_detect::{probe_client, probe_get};
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
    /// Model context window in tokens (e.g. 262144 for Qwopus 35B, 160000 for Qwen 27B).
    /// Used to compute the 70% compaction threshold. Default 8192 (safe minimum).
    #[serde(default = "default_context_window", deserialize_with = "deserialize_context_window")]
    pub context_window: usize,
}

#[allow(dead_code)]
fn default_context_window() -> usize { 8192 }

fn deserialize_context_window<'de, D>(d: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<usize>::deserialize(d)?.unwrap_or_else(default_context_window))
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

        // context_window bounds: reject 0 (would make Phase B's 0.7*window threshold 0 →
        // divide-by-zero / always-compact) and absurd values. Floor 1024 (tiny but sane),
        // ceiling 2_097_152 (2M tokens — covers every 2026 model, blocks garbage).
        if entry.context_window < 1024 {
            return Err(format!(
                "context_window {} is below minimum (1024). Set a real context window (tokens).",
                entry.context_window
            ));
        }
        if entry.context_window > 2_097_152 {
            return Err(format!(
                "context_window {} exceeds maximum (2097152). Set a real context window (tokens).",
                entry.context_window
            ));
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
    // First run: a missing config.json is not an error here — start from an empty object
    // (get_model_registry already tolerates absence; the setter must too).
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_string(),
        Err(e) => return Err(format!("Could not read config.json: {e}")),
    };
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

/// A model actually INSTALLED on a local backend (for the Settings UI to offer for
/// curation into the registry). oMLX `/v1/models` has no size; Ollama `/api/tags` does.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub backend: String,
    pub size_bytes: u64,
    pub param_size: Option<String>,
    pub quant: Option<String>,
    /// Size-RECOMMENDED tier ("agentic" >= 20B / "emitEdits" < 20B). A UI hint ONLY —
    /// the user's curated tier always wins and nothing gates on this.
    pub recommended_tier: String,
    /// Model context window in tokens, if detected from the backend API.
    /// Best-effort: omlx `/v1/models` and ollama `/api/tags` do NOT always expose this,
    /// so it may be None. The curated `ModelRegistryEntry.context_window` (default 8192)
    /// is the source of truth; this is a detection hint only. (Serialize-only type — no
    /// serde default needed.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
}

/// Best-effort parameter count in billions: from the Ollama `param_size` ("30B") when
/// present, else parsed from the model name (the LARGEST `<n>B` token, so an MoE "35B-A3B"
/// reads 35 not 3, and version numbers like the "3.6" in "Qwen3.6" — not followed by B —
/// are ignored). Never panics.
fn model_param_billions(name: &str, param_size: Option<&str>) -> Option<f64> {
    // Largest `<n>B` token (n = digits with at most one '.') whose 'b'/'B' is followed by a
    // word boundary. So "35B-A3B" reads 35 (not the 3B active count), while "4bit" (b before a
    // letter) and "3.6" (no trailing b) are ignored. Manual scan — no regex dependency.
    fn largest_b(s: &str) -> Option<f64> {
        let b = s.as_bytes();
        let mut max: Option<f64> = None;
        let mut i = 0;
        while i < b.len() {
            if !b[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            let start = i;
            let mut seen_dot = false;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !seen_dot)) {
                seen_dot |= b[i] == b'.';
                i += 1;
            }
            if i < b.len() && (b[i] == b'b' || b[i] == b'B') {
                let boundary = i + 1 >= b.len() || !b[i + 1].is_ascii_alphanumeric();
                if boundary {
                    if let Ok(val) = s[start..i].parse::<f64>() {
                        max = Some(max.map_or(val, |c: f64| c.max(val)));
                    }
                }
            }
        }
        max
    }
    param_size.and_then(largest_b).or_else(|| largest_b(name))
}

/// P5 (Work Console skill injection): map a mini's MODEL name to its capability-tier PROFILE
/// (`"mini-big"` | `"mini-small"`) so a launched mini receives the correct tier's SKILL.md.
/// Reuses the SAME size threshold as [`recommended_tier`] (>= 20B params -> capable) so the
/// runtime tier matches the size shown in the Work Console tier switcher ("32B big / 8B small").
/// An UNKNOWN/absent size -> `"mini-big"` (the plan's default: the capable tier is the safer
/// fallback for a missing signal, and the skill reader still falls back to the legacy `mini`
/// skill when no `mini-big/SKILL.md` exists, so nothing regresses).
pub(crate) fn mini_tier_profile(model: Option<&str>) -> &'static str {
    match model.and_then(|m| model_param_billions(m, None)) {
        Some(billions) if billions >= 20.0 => "mini-big",
        Some(_) => "mini-small",
        None => "mini-big",
    }
}

/// Size-recommended tier (UI hint; the user still chooses). >= 20B -> "agentic",
/// else (or unknown size) -> the safer "emitEdits".
fn recommended_tier(name: &str, param_size: Option<&str>) -> String {
    match model_param_billions(name, param_size) {
        Some(billions) if billions >= 20.0 => "agentic".to_string(),
        _ => "emitEdits".to_string(),
    }
}

/// Discover installed models across the local backends. UNGATED (like `detect_providers`
/// / `poll_backend_memory`): a non-secret installed-model list. A down backend contributes
/// nothing (not an error); parsing is tolerant (entries missing id/name are skipped).
#[tauri::command]
pub async fn discover_installed_models() -> Result<Vec<DiscoveredModel>, String> {
    let client = probe_client().ok_or_else(|| "probe client unavailable".to_string())?;
    let mut discovered = Vec::new();

    // oMLX /v1/models -> {"data":[{"id": "..."}]}
    if let Some(body) = probe_get(&client, "http://127.0.0.1:8000/v1/models").await {
        if let Ok(json) = body.parse::<serde_json::Value>() {
            if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                for item in data {
                    if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                        discovered.push(DiscoveredModel {
                            id: id.to_string(),
                            backend: "omlx".to_string(),
                            size_bytes: 0,
                            param_size: None,
                            quant: None,
                            // omlx /v1/models does not expose context_length; leave None.
                            context_window: None,
                            // oMLX gives no size → recommend from the model name.
                            recommended_tier: recommended_tier(id, None),
                        });
                    }
                }
            }
        }
    }

    // Ollama /api/tags -> {"models":[{"name","size","details":{parameter_size,quantization_level}}]}
    if let Some(body) = probe_get(&client, "http://127.0.0.1:11434/api/tags").await {
        if let Ok(json) = body.parse::<serde_json::Value>() {
            if let Some(models) = json.get("models").and_then(|m| m.as_array()) {
                for model in models {
                    if let Some(name) = model.get("name").and_then(|n| n.as_str()) {
                        let size = model.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                        let details = model.get("details");
                        let param_size = details
                            .and_then(|d| d.get("parameter_size"))
                            .and_then(|p| p.as_str())
                            .map(str::to_string);
                        let quant = details
                            .and_then(|d| d.get("quantization_level"))
                            .and_then(|q| q.as_str())
                            .map(str::to_string);
                        // Compute before `param_size` is moved into the struct.
                        let rec_tier = recommended_tier(name, param_size.as_deref());
                        discovered.push(DiscoveredModel {
                            id: name.to_string(),
                            backend: "ollama".to_string(),
                            size_bytes: size,
                            param_size,
                            quant,
                            // Best-effort: some ollama models expose context_length in details.
                            context_window: details
                                .and_then(|d| d.get("context_length"))
                                .and_then(|c| c.as_u64())
                                .map(|c| c as usize),
                            recommended_tier: rec_tier,
                        });
                    }
                }
            }
        }
    }

    Ok(discovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_tier_uses_real_param_size() {
        // oMLX has no param_size → parse from the name; the MoE TOTAL wins (35B, not the 3B
        // active count), and "4bit" / the version "3.6" are ignored.
        assert_eq!(recommended_tier("Qwen3.6-35B-A3B-4bit-DWQ", None), "agentic");
        assert_eq!(recommended_tier("qwen3:30b-a3b", None), "agentic");
        assert_eq!(recommended_tier("Qwen3.5-9B-MLX-4bit", None), "emitEdits");
        // Ollama's param_size wins over the name.
        assert_eq!(recommended_tier("anything", Some("30B")), "agentic");
        assert_eq!(recommended_tier("anything", Some("7B")), "emitEdits");
        // Unknown size → the safer default.
        assert_eq!(recommended_tier("mystery-model", None), "emitEdits");
    }

    #[test]
    fn mini_tier_profile_maps_by_size() {
        // Capable (>= 20B) -> mini-big; small (< 20B) -> mini-small.
        assert_eq!(mini_tier_profile(Some("Qwen3.6-35B-A3B-4bit-DWQ")), "mini-big");
        assert_eq!(mini_tier_profile(Some("qwen3:30b-a3b")), "mini-big");
        assert_eq!(mini_tier_profile(Some("Qwen3.5-9B-MLX-4bit")), "mini-small");
        assert_eq!(mini_tier_profile(Some("gemma-2-2b")), "mini-small");
        // Unknown size or no model -> the capable default (skill reader falls back to legacy mini).
        assert_eq!(mini_tier_profile(Some("mystery-model")), "mini-big");
        assert_eq!(mini_tier_profile(None), "mini-big");
    }

    #[test]
    fn param_billions_parsing() {
        assert_eq!(model_param_billions("Qwen3.6-35B-A3B-4bit-DWQ", None), Some(35.0));
        assert_eq!(model_param_billions("Qwen3.5-9B-MLX-4bit", None), Some(9.0));
        assert_eq!(model_param_billions("no-size", None), None);
        assert_eq!(model_param_billions("x", Some("405B")), Some(405.0));
    }

    #[test]
    fn model_registry_entry_has_context_window_field() {
        let entry = ModelRegistryEntry {
            id: "m".into(),
            backend: "omlx".into(),
            size_bytes: 0,
            tier: "agentic".into(),
            roles: vec![],
            enabled: true,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking_budget: None,
            context_window: 160000,
        };
        assert_eq!(entry.context_window, 160000);
    }

    #[test]
    fn discovered_model_has_optional_context_window() {
        let dm = DiscoveredModel {
            id: "x".into(),
            backend: "omlx".into(),
            size_bytes: 0,
            param_size: None,
            quant: None,
            recommended_tier: "emitEdits".into(),
            context_window: Some(32768),
        };
        assert_eq!(dm.context_window, Some(32768));

        // None must also be valid (model without detected window)
        let dm_none = DiscoveredModel {
            id: "y".into(),
            backend: "ollama".into(),
            size_bytes: 0,
            param_size: None,
            quant: None,
            recommended_tier: "emitEdits".into(),
            context_window: None,
        };
        assert_eq!(dm_none.context_window, None);
    }

    #[test]
    fn validate_model_registry_rejects_zero_context_window() {
        let bad = ModelRegistryEntry {
            id: "m".into(),
            backend: "omlx".into(),
            size_bytes: 0,
            tier: "agentic".into(),
            roles: vec![],
            enabled: true,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking_budget: None,
            context_window: 0,
        };
        let res = validate_model_registry(&[bad]);
        assert!(res.is_err(), "context_window 0 must be rejected");
        assert!(res.unwrap_err().to_lowercase().contains("context_window"));
    }

    #[test]
    fn deserialize_config_without_context_window_uses_default() {
        // Note: size_bytes is required (no default). The test ensures contextWindow
        // defaults to 8192 when omitted, but all required fields must be present.
        let json = r#"{"id":"m","backend":"omlx","sizeBytes":0,"tier":"agentic","roles":[],"enabled":true}"#;
        let entry: ModelRegistryEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.context_window, 8192, "backward compat: missing contextWindow backfills default 8192");
    }
}
