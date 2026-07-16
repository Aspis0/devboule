//! The app-side Oracle/GPU coordinator (core).
//!
//! Owns the process-global GPU/compute arbitration state and the PURE decision policies. This first
//! slice is self-contained — the §4c compute-concurrency counter (RAII permit) plus the pure
//! embed-device + index-burst-deferral policies. Wiring these into the live spawn path
//! (`mini_coder_executor`) and the Oracle (re)spawn (`oracle_service`, embed-device env) is a
//! deliberate follow-on. `devboule-coder` is a separate binary and reaches this state only over
//! MCP, so the coordinator lives app-side.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

/// §4c default: at most this many LOCAL model decodes run concurrently on the single GPU, so
/// resident-but-serialized models don't all crawl. Configurable via `set_max_concurrent_decodes`.
pub const DEFAULT_MAX_CONCURRENT_DECODES: u32 = 2;

pub struct OracleCoordinator {
    /// How many local decodes are active right now.
    active_local_decodes: AtomicU32,
    /// The cap (0 = disabled: the counter still tracks for observability but never rejects).
    max_concurrent_decodes: AtomicU32,
}

static COORDINATOR: OnceLock<OracleCoordinator> = OnceLock::new();

/// The process-global coordinator (lazy). Every caller shares ONE.
pub fn coordinator() -> &'static OracleCoordinator {
    COORDINATOR.get_or_init(|| OracleCoordinator {
        active_local_decodes: AtomicU32::new(0),
        max_concurrent_decodes: AtomicU32::new(DEFAULT_MAX_CONCURRENT_DECODES),
    })
}

impl OracleCoordinator {
    /// Set the concurrent-local-decode cap (0 disables the cap).
    pub fn set_max_concurrent_decodes(&self, cap: u32) {
        self.max_concurrent_decodes.store(cap, Ordering::Release);
    }

    /// The number of local decodes currently in flight.
    pub fn active_local_decodes(&self) -> u32 {
        self.active_local_decodes.load(Ordering::Acquire)
    }

    /// Try to acquire a compute slot for a local decode. Increment
    /// optimistically, validate against the live cap, roll back + return `None` on overflow so a
    /// rejected acquire never permanently consumes a slot. `cap == 0` never rejects.
    pub fn try_acquire_decode(&self) -> Option<ComputePermit> {
        let prior = self.active_local_decodes.fetch_add(1, Ordering::AcqRel);
        let cap = self.max_concurrent_decodes.load(Ordering::Acquire);
        if cap != 0 && prior >= cap {
            self.active_local_decodes.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(ComputePermit { _private: () })
    }
}

/// Held for the duration of one local decode; decrements the active count on EVERY drop path
/// (normal return, early exit, panic unwind).
pub struct ComputePermit {
    _private: (),
}

impl Drop for ComputePermit {
    fn drop(&mut self) {
        coordinator()
            .active_local_decodes
            .fetch_sub(1, Ordering::AcqRel);
    }
}

/// D3 — query-embed device policy (PURE). The featherweight per-query embed routes to CPU when a
/// coder is resident OR free memory is below the GPU floor (avoid the OOM / don't fight the coder);
/// otherwise MPS. The HEAVY post-commit index burst is governed separately (see
/// [`should_defer_index_burst`]).
pub fn embed_device_policy(free_gb: f64, min_gpu_free_gb: f64, coder_resident: bool) -> &'static str {
    if coder_resident || free_gb < min_gpu_free_gb {
        "cpu"
    } else {
        "mps"
    }
}

/// Defer the heavy GPU index re-embed burst (PURE) when there is no Metal headroom for it right now —
/// i.e. a coder/other model is resident and `current + est_burst > ceiling`. Saturating to avoid
/// overflow. The caller polls + retries; this is the "two heavy GPU bursts never overlap" guard from
/// the side the app controls.
pub fn should_defer_index_burst(
    current_model_memory_bytes: u64,
    est_burst_bytes: u64,
    final_ceiling_bytes: u64,
) -> bool {
    current_model_memory_bytes.saturating_add(est_burst_bytes) > final_ceiling_bytes
}

/// A6-core — machine-tiered output-token budget for ONE local decode (PURE). Replaces the blind
/// hardcoded 6144/8192 caps with a value that scales to the host's available unified memory. The
/// tiers mirror `budget::recommend_config`'s boundaries (<6 / <14 / <40 / >=40 GiB). A safe coarse
/// FLOOR; the exact per-model oMLX prefill-guard inversion (free_bytes -> max safe tokens) is a
/// follow-on refinement, and the wiring to the spawn sites (replacing the 6144/8192 constants in
/// mini_coder_executor / agentic_transport) is part of A4's live wiring.
pub fn recommended_max_tokens(free_gb: f64) -> u32 {
    if free_gb < 6.0 {
        2048
    } else if free_gb < 14.0 {
        4096
    } else if free_gb < 40.0 {
        8192
    } else {
        16384
    }
}

/// A6 (live wiring) — the machine-tiered output-token budget for a local decode on THIS host. Tiers
/// off TOTAL unified memory (the stable machine-CAPABILITY class, mirroring `budget::recommend_config`'s
/// `budget_bytes` basis — NOT the volatile `available_memory`, which under normal app+oMLX load would
/// mis-tier a 64GB host down). Read once + cached (the machine's capability tier doesn't change within
/// a run). Used by the AGENTIC path (where `max_rounds` is the runaway guard); the one-shot mini path
/// keeps its tighter `OMLX_MAX_TOKENS_DEFAULT` bound (there `max_tokens` IS the sole runaway wall).
pub fn detected_max_tokens() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let total_gb = crate::backend::hardware::read_cpu_ram().ram_total_gb;
        recommended_max_tokens(total_gb)
    })
}

const OMLX_BASE_URL: &str = "http://127.0.0.1:8000";

/// A4 — drive oMLX model residency: POST `/v1/models/{id}/load` or `/unload`. Used to make room for an
/// index burst DELIBERATELY (never to evict the coder). Best-effort, short timeout; returns Err on a
/// transport error or non-2xx so the caller can fall back to deferral. Blocking reqwest (callers run on
/// blocking worker threads).
pub fn omlx_set_model(model_id: &str, load: bool) -> Result<(), String> {
    let action = if load { "load" } else { "unload" };
    let url = format!("{OMLX_BASE_URL}/v1/models/{model_id}/{action}");
    let resp = reqwest::blocking::Client::new()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .map_err(|e| format!("omlx {action} request failed: {}", e.without_url()))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("omlx {action} returned HTTP {}", resp.status()))
    }
}

static BURST_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// RAII guard for the single GPU heavy-burst slot; releases on drop on EVERY path.
pub struct BurstGuard {
    _private: (),
}

impl Drop for BurstGuard {
    fn drop(&mut self) {
        BURST_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// A4 — try to claim the single heavy-GPU-burst slot (e.g. the post-commit index re-embed) so two heavy
/// GPU bursts never overlap. Returns `None` if a burst is already in progress (the caller defers/retries).
pub fn try_acquire_burst() -> Option<BurstGuard> {
    match BURST_IN_PROGRESS.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(BurstGuard { _private: () }),
        Err(_) => None,
    }
}

/// A4 — the device the Oracle embedder should use RIGHT NOW: routes the featherweight query embed to CPU
/// when a local decode is active OR available memory is below the GPU floor (3 GiB). Uses AVAILABLE memory
/// (current pressure — unlike the capability-tiered `detected_max_tokens` which uses total). Read fresh
/// (cheap) so the env set at Oracle (re)spawn reflects the moment.
pub fn current_embed_device() -> &'static str {
    let free_gb = crate::backend::hardware::read_cpu_ram().ram_available_gb;
    let coder_active = coordinator().active_local_decodes() > 0;
    embed_device_policy(free_gb, 3.0, coder_active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_permit_caps_and_releases() {
        let coord = coordinator();
        coord.set_max_concurrent_decodes(2);
        // This is the ONLY test that touches the shared global counter, so it starts at 0.
        assert_eq!(coord.active_local_decodes(), 0);

        let permit1 = coord.try_acquire_decode();
        let permit2 = coord.try_acquire_decode();
        assert!(permit1.is_some());
        assert!(permit2.is_some());
        assert_eq!(coord.active_local_decodes(), 2);

        // The 3rd is rejected and does NOT leak a slot.
        let permit3 = coord.try_acquire_decode();
        assert!(permit3.is_none());
        assert_eq!(coord.active_local_decodes(), 2);

        drop(permit1);
        assert_eq!(coord.active_local_decodes(), 1);

        let permit4 = coord.try_acquire_decode();
        assert!(permit4.is_some());
        assert_eq!(coord.active_local_decodes(), 2);

        drop(permit2);
        drop(permit4);
        assert_eq!(coord.active_local_decodes(), 0);
    }

    #[test]
    fn embed_policy_routes_to_cpu_under_pressure() {
        // Coder resident → CPU even with plenty free.
        assert_eq!(embed_device_policy(10.0, 2.0, true), "cpu");
        // Free below the floor → CPU.
        assert_eq!(embed_device_policy(1.5, 2.0, false), "cpu");
        // Free above the floor and no coder → MPS.
        assert_eq!(embed_device_policy(5.0, 2.0, false), "mps");
    }

    #[test]
    fn defer_burst_when_no_headroom() {
        assert!(should_defer_index_burst(8, 3, 10)); // 11 > 10 → defer
        assert!(!should_defer_index_burst(7, 3, 10)); // 10 == 10 → no defer
        assert!(!should_defer_index_burst(5, 3, 10)); // 8 < 10 → no defer
        assert!(!should_defer_index_burst(u64::MAX, 1, u64::MAX)); // saturates, no false positive
    }

    #[test]
    fn recommended_max_tokens_scales_with_memory() {
        assert_eq!(recommended_max_tokens(4.0), 2048); // tiny / cloud-first
        assert_eq!(recommended_max_tokens(10.0), 4096); // low
        assert_eq!(recommended_max_tokens(32.0), 8192); // mid
        assert_eq!(recommended_max_tokens(64.0), 16384); // high (M1 Max 64GB)
        // tier edges take the HIGHER tier (strict `<`).
        assert_eq!(recommended_max_tokens(40.0), 16384);
        assert_eq!(recommended_max_tokens(6.0), 4096);
    }

    #[test]
    fn burst_slot_is_mutually_exclusive() {
        let g1 = try_acquire_burst();
        assert!(g1.is_some());
        assert!(try_acquire_burst().is_none()); // a second concurrent burst is rejected
        drop(g1);
        let g2 = try_acquire_burst(); // freed → acquirable again
        assert!(g2.is_some());
        drop(g2);
    }
}
