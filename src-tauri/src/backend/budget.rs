//! Resource broker — live backend memory sensing + global budget (Phase 1–2).
//!
//! Read-only probes of the local inference backends so the app can aggregate a
//! global memory picture across oMLX + Ollama (each backend only knows its own
//! pool). These reuse the loopback-only, redirect-free, body-capped probe client
//! from `provider_detect` (`probe_client`/`probe_get`) and parse tolerantly —
//! external JSON is never trusted, missing/wrong-type fields degrade to defaults.

use serde::{Deserialize, Serialize};

use crate::backend::hardware::HardwareInfo;
use crate::backend::provider_detect::probe_get;

/// 1 GiB in bytes (binary). sysinfo / oMLX / Ollama all denominate in binary units,
/// so all RAM math here is GiB-based — NOT SI 1e9.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Default RAM reserved for OS + app + Oracle (the non-model headroom): 8 GiB. Observe-only
/// default; becomes a Settings value in Phase 3.
const DEFAULT_RESERVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// oMLX engine-pool snapshot from `GET :8000/health` → `engine_pool`. oMLX keeps
/// multiple models resident under a self-imposed `final_ceiling`; these are the
/// MEASURED values the broker prefers over any estimate.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OmlxHealth {
    pub loaded_count: u32,
    pub model_count: u32,
    pub final_ceiling_bytes: u64,
    pub current_model_memory_bytes: u64,
}

#[derive(Deserialize)]
struct OmlxHealthResponse {
    engine_pool: Option<OmlxEnginePool>,
}

#[derive(Deserialize)]
struct OmlxEnginePool {
    // Wire-typed as u64 then narrowed with a saturating cast: a future/buggy oMLX
    // value outside u32 range degrades to u32::MAX rather than silently to 0.
    model_count: Option<u64>,
    loaded_count: Option<u64>,
    final_ceiling: Option<u64>,
    current_model_memory: Option<u64>,
}

/// Ollama currently-loaded models from `GET :11434/api/ps`. Empty at idle (Ollama
/// lazy-loads + TTL-unloads). `size`/`size_vram` are the measured per-model footprint.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OllamaPs {
    pub models: Vec<OllamaLoadedModel>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OllamaLoadedModel {
    pub name: String,
    pub size_bytes: u64,
    pub size_vram_bytes: u64,
}

#[derive(Deserialize)]
struct OllamaPsResponse {
    models: Option<Vec<OllamaModelRaw>>,
}

#[derive(Deserialize)]
struct OllamaModelRaw {
    name: Option<String>,
    size: Option<u64>,
    size_vram: Option<u64>,
}

/// Probe oMLX `/health`. Returns `None` if oMLX is down or the body is unparseable
/// (failure-isolated, like the provider-detect probes).
pub async fn probe_omlx_health(client: &reqwest::Client) -> Option<OmlxHealth> {
    let body = probe_get(client, "http://127.0.0.1:8000/health").await?;
    let resp: OmlxHealthResponse = serde_json::from_str(&body).ok()?;
    let pool = resp.engine_pool?;

    Some(OmlxHealth {
        loaded_count: pool.loaded_count.unwrap_or(0).min(u32::MAX as u64) as u32,
        model_count: pool.model_count.unwrap_or(0).min(u32::MAX as u64) as u32,
        final_ceiling_bytes: pool.final_ceiling.unwrap_or(0),
        current_model_memory_bytes: pool.current_model_memory.unwrap_or(0),
    })
}

/// Probe Ollama `/api/ps`. Returns `None` if Ollama is down or unparseable; an
/// empty `models` list is the normal idle state.
pub async fn probe_ollama_ps(client: &reqwest::Client) -> Option<OllamaPs> {
    let body = probe_get(client, "http://127.0.0.1:11434/api/ps").await?;
    let resp: OllamaPsResponse = serde_json::from_str(&body).ok()?;
    let models_raw = resp.models.unwrap_or_default();

    let models = models_raw
        .into_iter()
        .filter_map(|m| {
            // Skip entries without a usable name (mirrors provider_detect's name filter)
            // so the UI never renders a blank model row.
            let name = m.name.filter(|n| !n.is_empty())?;
            Some(OllamaLoadedModel {
                name,
                size_bytes: m.size.unwrap_or(0),
                size_vram_bytes: m.size_vram.unwrap_or(0),
            })
        })
        .collect();

    Some(OllamaPs { models })
}

/// Global budget accountant (Phase 2). The app owns this because each backend only
/// reports its own pool — `used` = oMLX resident bytes + Σ Ollama loaded bytes, and
/// `budget` = total RAM minus a reserve (OS + app + Oracle headroom). All saturating.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BudgetSummary {
    pub total_ram_bytes: u64,
    pub reserve_bytes: u64,
    pub budget_bytes: u64,
    pub omlx_used_bytes: u64,
    pub ollama_used_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// PURE: compute the global budget from the measured backend memory. No I/O.
pub fn compute_budget(
    hardware: &HardwareInfo,
    omlx: &Option<OmlxHealth>,
    ollama: &Option<OllamaPs>,
    reserve_bytes: u64,
) -> BudgetSummary {
    // ram_total_gb is GiB (sysinfo bytes / 1024^3) → multiply by GiB, NOT 1e9. Guard
    // against a non-finite/negative value (would otherwise cast to u64::MAX / 0).
    let total_ram_bytes = if hardware.ram_total_gb.is_finite() && hardware.ram_total_gb >= 0.0 {
        (hardware.ram_total_gb * GIB) as u64
    } else {
        0
    };
    let omlx_used_bytes = omlx
        .as_ref()
        .map(|o| o.current_model_memory_bytes)
        .unwrap_or(0);
    // saturating fold (not sum()) so a hostile/buggy Ollama size can't overflow-panic in debug.
    let ollama_used_bytes = ollama
        .as_ref()
        .map(|o| {
            o.models
                .iter()
                .fold(0u64, |acc, m| acc.saturating_add(m.size_bytes))
        })
        .unwrap_or(0);

    let used_bytes = omlx_used_bytes.saturating_add(ollama_used_bytes);
    let budget_bytes = total_ram_bytes.saturating_sub(reserve_bytes);
    let free_bytes = budget_bytes.saturating_sub(used_bytes);

    BudgetSummary {
        total_ram_bytes,
        reserve_bytes,
        budget_bytes,
        omlx_used_bytes,
        ollama_used_bytes,
        used_bytes,
        free_bytes,
    }
}

/// Aggregated, app-owned view across BOTH local backends + the machine + the budget.
/// The app is the global accountant because each backend only knows its own pool
/// (oMLX `/health` vs Ollama `/api/ps`); a down backend is `None`, not an error.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackendMemorySnapshot {
    pub hardware: HardwareInfo,
    pub omlx: Option<OmlxHealth>,
    pub ollama: Option<OllamaPs>,
    pub budget: BudgetSummary,
}

/// Live resource snapshot for the UI: hardware + per-backend status + the global budget.
/// Only a missing probe client is an error; a backend being down just yields `None`.
///
/// UNGATED (like `detect_hardware`/`detect_providers`): returns non-secret machine +
/// local-backend status (loaded model names, pool memory) — no vault secrets or user
/// data. If model names are later deemed sensitive, gate via `BackendState::ensure_unlocked`.
#[tauri::command]
pub async fn poll_backend_memory() -> Result<BackendMemorySnapshot, String> {
    let client = crate::backend::provider_detect::probe_client()
        .ok_or_else(|| "probe client unavailable".to_string())?;

    // The two probes are independent (different backends/ports) — run them concurrently.
    let (omlx, ollama) = tokio::join!(probe_omlx_health(&client), probe_ollama_ps(&client));
    // collect_hardware() shells out to system_profiler (macOS) / DXGI (Windows) — a
    // blocking call; keep it OFF the async worker thread.
    let hardware = tauri::async_runtime::spawn_blocking(crate::backend::hardware::collect_hardware)
        .await
        .map_err(|e| format!("hardware probe failed: {e}"))?;

    let budget = compute_budget(&hardware, &omlx, &ollama, DEFAULT_RESERVE_BYTES);

    Ok(BackendMemorySnapshot {
        hardware,
        omlx,
        ollama,
        budget,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(ram_total_gb: f64) -> HardwareInfo {
        HardwareInfo {
            cpu_cores: 10,
            ram_total_gb,
            ram_available_gb: ram_total_gb,
            gpu_name: "test".to_string(),
            vram_gb: None,
            gpu_kind: "integrated".to_string(),
        }
    }

    const GIB_U64: u64 = 1024 * 1024 * 1024;

    #[test]
    fn compute_budget_uses_binary_gib_and_saturates() {
        let omlx = Some(OmlxHealth {
            loaded_count: 0,
            model_count: 0,
            final_ceiling_bytes: 0,
            current_model_memory_bytes: 40_000_000_000,
        });
        let ollama = Some(OllamaPs {
            models: vec![
                OllamaLoadedModel { name: "m1".into(), size_bytes: 8_000_000_000, size_vram_bytes: 0 },
                OllamaLoadedModel { name: "m2".into(), size_bytes: 2_000_000_000, size_vram_bytes: 0 },
            ],
        });
        let reserve = 8 * GIB_U64;
        let s = compute_budget(&hw(64.0), &omlx, &ollama, reserve);
        // 64 GiB in BYTES — not 64e9. This assertion is the GiB-vs-GB regression guard.
        assert_eq!(s.total_ram_bytes, 64 * GIB_U64);
        assert_eq!(s.used_bytes, 50_000_000_000);
        assert_eq!(s.budget_bytes, 64 * GIB_U64 - reserve);
        assert_eq!(s.free_bytes, 64 * GIB_U64 - reserve - 50_000_000_000);

        // used > budget → free saturates to 0
        let heavy = Some(OmlxHealth {
            loaded_count: 0,
            model_count: 0,
            final_ceiling_bytes: 0,
            current_model_memory_bytes: 60_000_000_000,
        });
        let s2 = compute_budget(&hw(64.0), &heavy, &ollama, reserve);
        assert_eq!(s2.free_bytes, 0);
    }

    #[test]
    fn compute_budget_handles_down_backends() {
        let reserve = 8 * GIB_U64;
        let s = compute_budget(&hw(16.0), &None, &None, reserve);
        assert_eq!(s.used_bytes, 0);
        assert_eq!(s.total_ram_bytes, 16 * GIB_U64);
        assert_eq!(s.budget_bytes, 16 * GIB_U64 - reserve);
        assert_eq!(s.free_bytes, 16 * GIB_U64 - reserve);
    }
}


/// Phase 5 — recommended role→placement by hardware tier (PURE). The broker proposes a
/// sensible default the user can apply/override: discrete-GPU machines are bounded by VRAM,
/// unified/integrated by the RAM budget. Oracle (lightest) stays local in every tier.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecommendedConfig {
    pub tier: String,
    pub main_coder: String,
    pub mini_coder: String,
    pub censor: String,
    pub oracle: String,
    pub rationale: String,
}

pub fn recommend_config(hardware: &HardwareInfo, budget: &BudgetSummary) -> RecommendedConfig {
    // discrete GPU → bounded by VRAM; unified/integrated/unknown → bounded by the RAM budget.
    let avail_gib = if hardware.gpu_kind == "discrete" {
        hardware.vram_gb.unwrap_or(0.0)
    } else {
        budget.budget_bytes as f64 / 1_073_741_824.0
    };

    let (tier, main_coder, mini_coder, censor, oracle) = if avail_gib < 6.0 {
        ("minimal", "cloud", "cloud", "cloud", "local")
    } else if avail_gib < 14.0 {
        ("low", "cloud", "cloud", "local", "local")
    } else if avail_gib < 40.0 {
        ("mid", "cloud", "local", "local", "local")
    } else {
        ("high", "local", "local", "local", "local")
    };

    let rationale = format!(
        "{tier} tier ({}, {avail_gib:.1} GiB usable): main={main_coder}, mini={mini_coder}, censor={censor}, oracle={oracle}",
        hardware.gpu_kind
    );

    RecommendedConfig {
        tier: tier.to_string(),
        main_coder: main_coder.to_string(),
        mini_coder: mini_coder.to_string(),
        censor: censor.to_string(),
        oracle: oracle.to_string(),
        rationale,
    }
}

/// Phase 5 command: the recommended config for THIS machine (polls the live budget).
#[tauri::command]
pub async fn recommend_resource_config() -> Result<RecommendedConfig, String> {
    let snapshot = poll_backend_memory().await?;
    Ok(recommend_config(&snapshot.hardware, &snapshot.budget))
}

#[cfg(test)]
mod recommend_tests {
    use super::*;

    const GIB_U64: u64 = 1_073_741_824;

    fn bud(total_gib: u64, budget_gib: u64) -> BudgetSummary {
        BudgetSummary {
            total_ram_bytes: total_gib * GIB_U64,
            reserve_bytes: (total_gib.saturating_sub(budget_gib)) * GIB_U64,
            budget_bytes: budget_gib * GIB_U64,
            omlx_used_bytes: 0,
            ollama_used_bytes: 0,
            used_bytes: 0,
            free_bytes: budget_gib * GIB_U64,
        }
    }

    fn hw(gpu_kind: &str, vram_gb: Option<f64>, ram_total_gb: f64) -> HardwareInfo {
        HardwareInfo {
            cpu_cores: 8,
            ram_total_gb,
            ram_available_gb: ram_total_gb,
            gpu_name: "test".to_string(),
            vram_gb,
            gpu_kind: gpu_kind.to_string(),
        }
    }

    #[test]
    fn discrete_6gb_vram_is_low() {
        let c = recommend_config(&hw("discrete", Some(6.0), 16.0), &bud(16, 8));
        assert_eq!(c.tier, "low");
        assert_eq!(c.censor, "local");
        assert_eq!(c.main_coder, "cloud");
    }

    #[test]
    fn unified_64gib_is_high_all_local() {
        let c = recommend_config(&hw("integrated", None, 64.0), &bud(64, 56));
        assert_eq!(c.tier, "high");
        assert_eq!(c.main_coder, "local");
        assert_eq!(c.mini_coder, "local");
        assert_eq!(c.censor, "local");
        assert_eq!(c.oracle, "local");
    }

    #[test]
    fn tiny_is_minimal_censor_cloud() {
        let c = recommend_config(&hw("integrated", None, 8.0), &bud(8, 5));
        assert_eq!(c.tier, "minimal");
        assert_eq!(c.censor, "cloud");
        assert_eq!(c.main_coder, "cloud");
        assert_eq!(c.oracle, "local");
    }
}

