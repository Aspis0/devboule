// === src-tauri/src/backend/cost.rs ===

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

const MAX_PRICING_BYTES: usize = 1024 * 1024; // 1 MiB
const MAX_LEDGER_BYTES: usize = 1024 * 1024; // 1 MiB

// ---------------------------------------------------------------------------
// 1. Pricing types + fallback
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Case-insensitive substring match against a small hardcoded table.
/// Returns `None` if no known model pattern is recognised.
fn fallback_price(model_id: &str) -> Option<ModelPrice> {
    let lower = model_id.to_lowercase();
    let table: &[(&str, f64, f64)] = &[
        ("claude-opus", 15.0, 75.0),
        ("claude-sonnet", 3.0, 15.0),
        ("claude-haiku", 0.80, 4.0),
        ("glm-5.2", 1.40, 4.40),
        ("deepseek-v4-pro", 0.435, 0.87),
    ];
    for (needle, inp, out) in table {
        if lower.contains(needle) {
            return Some(ModelPrice {
                input_per_mtok: *inp,
                output_per_mtok: *out,
            });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 2. OpenRouter pricing fetch + cache
// ---------------------------------------------------------------------------

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn pricing_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create app data dir: {e}"))?;
    Ok(dir.join("openrouter-pricing.json"))
}

/// Parse the JSON body of `GET /api/v1/models` into a per-Mtok price map.
/// Malformed or non-finite pricing entries are silently skipped.
fn parse_openrouter_models(body: &serde_json::Value) -> BTreeMap<String, ModelPrice> {
    let mut map = BTreeMap::new();
    let data = match body.get("data").and_then(|d| d.as_array()) {
        Some(arr) => arr,
        None => return map,
    };
    for item in data {
        let id = match item.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let pricing = match item.get("pricing") {
            Some(p) => p,
            None => continue,
        };
        let Some(prompt) = pricing
            .get("prompt")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
        else {
            continue
        };
        let Some(completion) = pricing
            .get("completion")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
        else {
            continue
        };
        map.insert(
            id,
            ModelPrice {
                input_per_mtok: prompt * 1_000_000.0,
                output_per_mtok: completion * 1_000_000.0,
            },
        );
    }
    map
}

fn write_pricing_to_path(
    path: &Path,
    map: &BTreeMap<String, ModelPrice>,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    super::design::atomic_write(path, &json, "openrouter-pricing")
}

/// Fetch the full model catalogue from OpenRouter, extract per-Mtok pricing,
/// and atomically cache it to `<app-data>/openrouter-pricing.json`.
/// Returns the number of entries cached.
/// Errors **only** on HTTP/network failure (no key in the message).
#[tauri::command]
pub async fn refresh_openrouter_pricing(
    app: tauri::AppHandle,
) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {}", e))?;

    let mut req = client.get("https://openrouter.ai/api/v1/models");
    if let Ok(Some(key)) = super::vault::read_cloud_llm_key() {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("OpenRouter pricing fetch failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "OpenRouter pricing fetch returned HTTP {}",
            response.status()
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("OpenRouter pricing parse failed: {}", e))?;

    let map = parse_openrouter_models(&body);

    let path = pricing_path(&app)?;
    write_pricing_to_path(&path, &map)?;

    Ok(map.len())
}

fn load_cached_pricing_from_path(path: &Path) -> BTreeMap<String, ModelPrice> {
    let data = match std::fs::read(path) {
        Ok(d) if d.len() <= MAX_PRICING_BYTES => d,
        _ => return BTreeMap::new(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

fn load_cached_pricing(app: &tauri::AppHandle) -> BTreeMap<String, ModelPrice> {
    match pricing_path(app) {
        Ok(p) => load_cached_pricing_from_path(&p),
        Err(_) => BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------------
// 3. price_for + estimator
// ---------------------------------------------------------------------------

/// Try cached OpenRouter map first (exact id only), then fall back to the
/// hardcoded table, then `None`.
fn price_for_from_map(
    cached: &BTreeMap<String, ModelPrice>,
    model_id: &str,
) -> Option<ModelPrice> {
    if let Some(p) = cached.get(model_id) {
        return Some(p.clone());
    }
    fallback_price(model_id)
}

fn price_for(app: &tauri::AppHandle, model_id: &str) -> Option<ModelPrice> {
    let cached = load_cached_pricing(app);
    price_for_from_map(&cached, model_id)
}

fn estimate_cost(price: &ModelPrice, input_tokens: u64, output_tokens: u64) -> f64 {
    (input_tokens as f64) / 1_000_000.0 * price.input_per_mtok
        + (output_tokens as f64) / 1_000_000.0 * price.output_per_mtok
}

/// Rough heuristic for a typical task.  This is a COARSE estimate, not precise.
/// Reasoning models get a larger output budget because thinking tokens eat it.
fn default_task_budget(model_id: &str) -> (u64, u64) {
    let lower = model_id.to_lowercase();
    let long_markers = [
        "glm-5",
        "deepseek-r",
        "-thinking",
        "reasoning",
        "qwq",
    ];
    let short_markers = ["o1", "o3", "o4"];
    let is_reasoning = long_markers.iter().any(|m| lower.contains(m))
        || short_markers.iter().any(|m| {
            lower.contains(&format!("/{m}"))
                || lower.contains(&format!("-{m}"))
                || lower.starts_with(m)
        });
    if is_reasoning {
        (8000, 14000)
    } else {
        (8000, 2500)
    }
}

#[tauri::command]
pub fn estimate_task_cost(
    app: tauri::AppHandle,
    model_id: String,
) -> Result<Option<f64>, String> {
    Ok(price_for(&app, &model_id).and_then(|p| {
        let (input, output) = default_task_budget(&model_id);
        let cost = estimate_cost(&p, input, output);
        if cost.is_finite() {
            Some(cost)
        } else {
            None
        }
    }))
}

// ---------------------------------------------------------------------------
// 4. Ledger (persistent running total)
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct CostLedger {
    #[serde(default)]
    pub total_usd: f64,
    #[serde(default)]
    pub by_model: BTreeMap<String, f64>,
}

fn ledger_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create app data dir: {e}"))?;
    Ok(dir.join("cost-ledger.json"))
}

fn load_ledger_from_path(path: &Path) -> CostLedger {
    let data = match std::fs::read(path) {
        Ok(d) if d.len() <= MAX_LEDGER_BYTES => d,
        _ => return CostLedger::default(),
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

fn load_ledger(app: &tauri::AppHandle) -> CostLedger {
    match ledger_path(app) {
        Ok(p) => load_ledger_from_path(&p),
        Err(_) => CostLedger::default(),
    }
}

fn record_cost_to_path(path: &Path, model_id: &str, usd: f64) -> Result<(), String> {
    if usd < 0.0 || usd.is_nan() || usd.is_infinite() {
        return Err("cost must be a non-negative finite number".into());
    }
    let mut ledger = load_ledger_from_path(path);
    ledger.total_usd += usd;
    *ledger.by_model.entry(model_id.to_string()).or_insert(0.0) += usd;
    let json = serde_json::to_string_pretty(&ledger).map_err(|e| e.to_string())?;
    super::design::atomic_write(path, &json, "cost-ledger")
}

#[tauri::command]
pub fn record_cost(
    app: tauri::AppHandle,
    model_id: String,
    usd: f64,
) -> Result<(), String> {
    let path = ledger_path(&app)?;
    record_cost_to_path(&path, &model_id, usd)
}

#[tauri::command]
pub fn get_cost_summary(app: tauri::AppHandle) -> Result<CostLedger, String> {
    Ok(load_ledger(&app))
}

// ---------------------------------------------------------------------------
// 5. Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_openrouter_models --

    #[test]
    fn test_parse_openrouter_models() {
        let json = serde_json::json!({
            "data": [
                {
                    "id": "z-ai/glm-5.2",
                    "pricing": {
                        "prompt": "0.0000014",
                        "completion": "0.0000044",
                        "request": "0"
                    }
                },
                {
                    "id": "deepseek/deepseek-v4-pro",
                    "pricing": {
                        "prompt": "0.000000435",
                        "completion": "0.00000087"
                    }
                }
            ]
        });
        let map = parse_openrouter_models(&json);
        assert_eq!(map.len(), 2);

        let glm = map.get("z-ai/glm-5.2").unwrap();
        assert!((glm.input_per_mtok - 1.4).abs() < 0.001);
        assert!((glm.output_per_mtok - 4.4).abs() < 0.001);

        let ds = map.get("deepseek/deepseek-v4-pro").unwrap();
        assert!((ds.input_per_mtok - 0.435).abs() < 0.001);
        assert!((ds.output_per_mtok - 0.87).abs() < 0.001);
    }

    #[test]
    fn test_parse_openrouter_models_skips_malformed() {
        let json = serde_json::json!({
            "data": [
                { "id": "good/model", "pricing": { "prompt": "0.001", "completion": "0.002" } },
                { "id": "bad/model", "pricing": { "prompt": "not-a-number", "completion": "0.002" } },
                { "no_id": true },
                { "id": "no_pricing" }
            ]
        });
        let map = parse_openrouter_models(&json);
        // malformed/missing-price entries are silently skipped
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("good/model"));
        assert!(!map.contains_key("bad/model"));
    }

    // -- fallback_price --

    #[test]
    fn test_fallback_price_substring() {
        let p = fallback_price("anthropic/claude-sonnet-4").unwrap();
        assert!((p.input_per_mtok - 3.0).abs() < 0.001);
        assert!((p.output_per_mtok - 15.0).abs() < 0.001);

        let p = fallback_price("anthropic/claude-opus-4-20250514").unwrap();
        assert!((p.input_per_mtok - 15.0).abs() < 0.001);

        let p = fallback_price("z-ai/glm-5.2").unwrap();
        assert!((p.input_per_mtok - 1.40).abs() < 0.001);
        assert!((p.output_per_mtok - 4.40).abs() < 0.001);

        let p = fallback_price("deepseek/deepseek-v4-pro").unwrap();
        assert!((p.input_per_mtok - 0.435).abs() < 0.001);

        assert!(fallback_price("unknown/model").is_none());
    }

    #[test]
    fn test_fallback_price_case_insensitive() {
        let p = fallback_price("ANTHROPIC/CLAUDE-HAIKU").unwrap();
        assert!((p.input_per_mtok - 0.80).abs() < 0.001);
    }

    // -- estimate_cost --

    #[test]
    fn test_estimate_cost() {
        let price = ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        // 1000 input tokens @ $3/Mtok = $0.003
        //  500 output tokens @ $15/Mtok = $0.0075
        // total = $0.0105
        let cost = estimate_cost(&price, 1000, 500);
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn test_estimate_cost_zero() {
        let price = ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        };
        assert_eq!(estimate_cost(&price, 0, 0), 0.0);
    }

    // -- default_task_budget --

    #[test]
    fn test_default_task_budget_normal() {
        let (i, o) = default_task_budget("anthropic/claude-sonnet-4");
        assert_eq!(i, 8000);
        assert_eq!(o, 2500);
    }

    #[test]
    fn test_default_task_budget_reasoning() {
        let (i, o) = default_task_budget("z-ai/glm-5.2");
        assert_eq!(i, 8000);
        assert_eq!(o, 14000);

        let (_, o) = default_task_budget("openai/o1-preview");
        assert_eq!(o, 14000);

        let (_, o) = default_task_budget("openai/o3-mini");
        assert_eq!(o, 14000);

        let (_, o) = default_task_budget("deepseek/deepseek-r1");
        assert_eq!(o, 14000);

        let (_, o) = default_task_budget("qwen/qwq-32b");
        assert_eq!(o, 14000);

        let (_, o) = default_task_budget("model-thinking");
        assert_eq!(o, 14000);
    }

    // -- price_for_from_map --

    #[test]
    fn test_price_for_from_map_exact() {
        let mut cached = BTreeMap::new();
        cached.insert(
            "z-ai/glm-5.2".to_string(),
            ModelPrice {
                input_per_mtok: 1.4,
                output_per_mtok: 4.4,
            },
        );
        let p = price_for_from_map(&cached, "z-ai/glm-5.2").unwrap();
        assert!((p.input_per_mtok - 1.4).abs() < 0.001);
    }

    #[test]
    fn test_price_for_from_map_substring() {
        // With exact-match-only cache lookup, a versioned id like "z-ai/glm-5.2-2025"
        // no longer matches the cached "z-ai/glm-5.2" key directly, but resolves
        // via fallback_price which still returns the glm price.
        let mut cached = BTreeMap::new();
        cached.insert(
            "z-ai/glm-5.2".to_string(),
            ModelPrice {
                input_per_mtok: 1.4,
                output_per_mtok: 4.4,
            },
        );
        let p = price_for_from_map(&cached, "z-ai/glm-5.2-2025").unwrap();
        assert!((p.input_per_mtok - 1.4).abs() < 0.001);
    }

    #[test]
    fn test_price_for_from_map_fallback() {
        let cached = BTreeMap::new();
        let p = price_for_from_map(&cached, "anthropic/claude-opus-4").unwrap();
        assert!((p.input_per_mtok - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_price_for_from_map_none() {
        let cached = BTreeMap::new();
        assert!(price_for_from_map(&cached, "totally/unknown").is_none());
    }

    // -- Ledger round-trip --

    fn tmp_ledger_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cost-ledger-test-{}-{}.json",
            std::process::id(),
            tag
        ))
    }

    #[test]
    fn test_ledger_round_trip() {
        let path = tmp_ledger_path("round_trip");
        let _ = std::fs::remove_file(&path);

        record_cost_to_path(&path, "claude-sonnet", 0.05).unwrap();
        record_cost_to_path(&path, "claude-sonnet", 0.03).unwrap();
        record_cost_to_path(&path, "glm-5.2", 0.01).unwrap();

        let ledger = load_ledger_from_path(&path);
        assert!((ledger.total_usd - 0.09).abs() < 0.0001);
        assert!((ledger.by_model.get("claude-sonnet").unwrap() - 0.08).abs() < 0.0001);
        assert!((ledger.by_model.get("glm-5.2").unwrap() - 0.01).abs() < 0.0001);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_record_cost_rejects_invalid() {
        let path = tmp_ledger_path("invalid");
        let _ = std::fs::remove_file(&path);

        assert!(record_cost_to_path(&path, "model", -1.0).is_err());
        assert!(record_cost_to_path(&path, "model", f64::NAN).is_err());
        assert!(record_cost_to_path(&path, "model", f64::INFINITY).is_err());

        // nothing should have been written
        assert!(!path.exists());

        // valid cost works
        record_cost_to_path(&path, "model", 0.01).unwrap();
        let ledger = load_ledger_from_path(&path);
        assert!((ledger.total_usd - 0.01).abs() < 0.0001);

        let _ = std::fs::remove_file(&path);
    }

    // -- Fail-open --

    #[test]
    fn test_load_cached_pricing_missing_file() {
        let path = std::env::temp_dir().join("nonexistent-pricing-12345678.json");
        let _ = std::fs::remove_file(&path);
        let map = load_cached_pricing_from_path(&path);
        assert!(map.is_empty());
    }

    #[test]
    fn test_load_cached_pricing_malformed() {
        let path = std::env::temp_dir().join(format!(
            "malformed-pricing-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "not json at all").unwrap();
        let map = load_cached_pricing_from_path(&path);
        assert!(map.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_load_ledger_missing_file() {
        let path = std::env::temp_dir().join("nonexistent-ledger-12345678.json");
        let _ = std::fs::remove_file(&path);
        let ledger = load_ledger_from_path(&path);
        assert_eq!(ledger.total_usd, 0.0);
        assert!(ledger.by_model.is_empty());
    }

    #[test]
    fn test_load_ledger_malformed() {
        let path = std::env::temp_dir().join(format!(
            "malformed-ledger-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "{{{{not json").unwrap();
        let ledger = load_ledger_from_path(&path);
        assert_eq!(ledger.total_usd, 0.0);
        assert!(ledger.by_model.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
