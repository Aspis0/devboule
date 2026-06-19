//! Resource broker — live backend memory sensing (Phase 1).
//!
//! Read-only probes of the local inference backends so the app can aggregate a
//! global memory picture across oMLX + Ollama (each backend only knows its own
//! pool). These reuse the loopback-only, redirect-free, body-capped probe client
//! from `provider_detect` (`probe_client`/`probe_get`) and parse tolerantly —
//! external JSON is never trusted, missing/wrong-type fields degrade to defaults.

use serde::{Deserialize, Serialize};

use crate::backend::hardware::HardwareInfo;
use crate::backend::provider_detect::probe_get;

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

/// Aggregated, app-owned view across BOTH local backends + the machine. The app is
/// the global accountant because each backend only knows its own pool (oMLX `/health`
/// vs Ollama `/api/ps`); a down backend is `None`, not an error.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackendMemorySnapshot {
    pub hardware: HardwareInfo,
    pub omlx: Option<OmlxHealth>,
    pub ollama: Option<OllamaPs>,
}

/// Live resource snapshot for the UI: hardware + whatever each local backend reports.
/// Only a missing probe client is an error; a backend being down just yields `None`.
///
/// UNGATED (like `detect_hardware`/`detect_providers`): returns non-secret machine +
/// local-backend status (loaded model names, pool memory) — no vault secrets or user
/// data. If model names are later deemed sensitive, gate via `BackendState::ensure_unlocked`.
#[tauri::command]
pub async fn poll_backend_memory() -> Result<BackendMemorySnapshot, String> {
    let client = crate::backend::provider_detect::probe_client()
        .ok_or_else(|| "probe client unavailable".to_string())?;

    let omlx = probe_omlx_health(&client).await;
    let ollama = probe_ollama_ps(&client).await;
    // collect_hardware() shells out to system_profiler (macOS) / DXGI (Windows) — a
    // blocking call; keep it OFF the async worker thread.
    let hardware = tauri::async_runtime::spawn_blocking(crate::backend::hardware::collect_hardware)
        .await
        .map_err(|e| format!("hardware probe failed: {e}"))?;

    Ok(BackendMemorySnapshot {
        hardware,
        omlx,
        ollama,
    })
}
