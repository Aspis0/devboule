use super::auth;
use super::model::{
    ActivityEvent, AuthState, ProviderHealth, ProviderId, ProviderScopeSelection,
    ScalewayOfferSummary, ScalewayResourceSummary, ScalewayStorageSummary,
};
use super::providers::ProviderInventory;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration as StdDuration, Instant};

const UNLOCK_TTL_MINUTES: i64 = 15;
const ACTIVITY_HISTORY_LIMIT: usize = 80;

#[derive(Debug)]
struct AuthSession {
    locked: bool,
    /// User-facing unlock timestamp (wall clock). NOT used for expiry math.
    last_unlocked_at: Option<DateTime<Utc>>,
    /// D2: monotonic unlock instant used for the idle-TTL comparison so a
    /// backward wall-clock change cannot extend the unlocked window.
    unlocked_instant: Option<Instant>,
    lock_reason: Option<String>,
    session_id: u64,
}

pub struct BackendState {
    auth: RwLock<AuthSession>,
    auth_prompt: Mutex<()>,
    auth_retry_after: Mutex<Option<Instant>>,
    hello_available: OnceLock<bool>,
    scaleway_compute: RwLock<Vec<ScalewayResourceSummary>>,
    scaleway_compute_initialized: RwLock<bool>,
    /// Last synced Instance offer catalog. Read by the Instance create dry-run to
    /// price a chosen `commercial_type` WITHOUT a network call. Empty until the
    /// first successful sync; cleared on lock / inventory clear.
    scaleway_offers: RwLock<Vec<ScalewayOfferSummary>>,
    cached_cloudflare: RwLock<Option<ProviderInventory>>,
    cached_scaleway: RwLock<Option<ProviderInventory>>,
    activity_history: RwLock<Vec<ActivityEvent>>,
    // TODO/SECURITY: this shared client has NO redirect policy set, so it follows the
    // reqwest default (up to 10 redirects). Any callers that send secrets/tokens (Scaleway,
    // GitHub) could in principle have a 3xx redirect them off the intended host. This is
    // flagged for a later dedicated review — do NOT change it here, as those paths may rely
    // on the current behavior. The design-generation HTTP path deliberately does NOT use
    // this client; it uses its own redirect-disabled, loopback-only client (see
    // backend::design_generate::DesignGenState::http_client).
    pub http: reqwest::Client,
}

pub struct ScalewayComputeReplacement {
    pub previous: Vec<ScalewayResourceSummary>,
    pub had_previous_snapshot: bool,
}

impl BackendState {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("Aspis-Management/0.1")
            .timeout(StdDuration::from_secs(15))
            .build()
            .expect("failed to build reqwest client");

        Self {
            auth: RwLock::new(AuthSession {
                locked: true,
                last_unlocked_at: None,
                unlocked_instant: None,
                lock_reason: Some("startup".into()),
                session_id: 0,
            }),
            auth_prompt: Mutex::new(()),
            auth_retry_after: Mutex::new(None),
            hello_available: OnceLock::new(),
            scaleway_compute: RwLock::new(Vec::new()),
            scaleway_compute_initialized: RwLock::new(false),
            scaleway_offers: RwLock::new(Vec::new()),
            cached_cloudflare: RwLock::new(None),
            cached_scaleway: RwLock::new(None),
            activity_history: RwLock::new(Vec::new()),
            http,
        }
    }

    pub fn auth_state(&self) -> Result<AuthState, String> {
        let (locked, last_unlocked_at, lock_reason, expired) = {
            let mut auth = self.auth.write().map_err(|e| e.to_string())?;
            let expired = Self::expire_if_needed(&mut auth);
            (
                auth.locked,
                auth.last_unlocked_at.as_ref().map(|ts| ts.to_rfc3339()),
                auth.lock_reason.clone(),
                expired,
            )
        };
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        Ok(AuthState {
            locked,
            hello_available: *self.hello_available.get_or_init(auth::hello_available),
            last_unlocked_at,
            lock_reason,
        })
    }

    pub fn lock(&self, reason: &str) -> Result<AuthState, String> {
        {
            let mut auth = self.auth.write().map_err(|e| e.to_string())?;
            auth.locked = true;
            auth.lock_reason = Some(reason.to_string());
            auth.session_id = auth.session_id.wrapping_add(1);
        }
        self.clear_sensitive_runtime_data()?;
        self.auth_state()
    }

    pub fn unlock_after_verification(&self) -> Result<AuthState, String> {
        {
            let mut auth = self.auth.write().map_err(|e| e.to_string())?;
            auth.locked = false;
            auth.last_unlocked_at = Some(Utc::now());
            // D2: capture the monotonic unlock instant for the TTL comparison.
            auth.unlocked_instant = Some(Instant::now());
            auth.lock_reason = None;
            auth.session_id = auth.session_id.wrapping_add(1);
        }
        // Bring up the resident, app-supervised Oracle server and publish the
        // discovery file for MCP thin-clients. Best-effort: failures never block
        // the unlock (the operator `/ask` path lazily starts the server anyway).
        oracle_service_on_unlock();
        self.auth_state()
    }

    pub fn verify_unlock(&self, message: &str) -> Result<AuthState, String> {
        self.ensure_auth_retry_allowed()?;

        let _prompt_guard = self
            .auth_prompt
            .try_lock()
            .map_err(|_| "Windows Hello verification is already in progress.".to_string())?;

        match auth::verify_user(message) {
            Ok(true) => {
                self.clear_auth_retry_cooldown()?;
                self.unlock_after_verification()
            }
            Ok(false) => {
                self.start_auth_retry_cooldown()?;
                Err("Windows Hello was cancelled or not approved. Use PIN or try again in a few seconds.".into())
            }
            Err(e) => {
                self.start_auth_retry_cooldown()?;
                Err(e)
            }
        }
    }

    fn ensure_auth_retry_allowed(&self) -> Result<(), String> {
        let now = Instant::now();
        let mut retry_after = self.auth_retry_after.lock().map_err(|e| e.to_string())?;
        if let Some(until) = *retry_after {
            if until > now {
                let seconds = until.saturating_duration_since(now).as_secs().max(1);
                return Err(format!(
                    "Windows Hello retry is cooling down. Try again in {seconds} seconds."
                ));
            }
            *retry_after = None;
        }
        Ok(())
    }

    fn start_auth_retry_cooldown(&self) -> Result<(), String> {
        *self.auth_retry_after.lock().map_err(|e| e.to_string())? =
            Some(Instant::now() + StdDuration::from_secs(6));
        Ok(())
    }

    fn clear_auth_retry_cooldown(&self) -> Result<(), String> {
        *self.auth_retry_after.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }

    pub fn ensure_unlocked(&self) -> Result<(), String> {
        let mut auth = self.auth.write().map_err(|e| e.to_string())?;
        let expired = Self::expire_if_needed(&mut auth);
        let locked = auth.locked;
        drop(auth);
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        if locked {
            return Err("App is locked. Unlock with Windows Hello first.".into());
        }
        Ok(())
    }

    pub fn sensitive_session_id(&self) -> Result<u64, String> {
        let mut auth = self.auth.write().map_err(|e| e.to_string())?;
        let expired = Self::expire_if_needed(&mut auth);
        let locked = auth.locked;
        let session_id = auth.session_id;
        drop(auth);
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        if locked {
            return Err("App is locked. Unlock with Windows Hello first.".into());
        }
        Ok(session_id)
    }

    pub fn ensure_same_sensitive_session(&self, session_id: u64) -> Result<(), String> {
        let mut auth = self.auth.write().map_err(|e| e.to_string())?;
        let expired = Self::expire_if_needed(&mut auth);
        let locked = auth.locked;
        let changed = auth.session_id != session_id;
        drop(auth);
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        if locked {
            return Err("App is locked. Unlock with Windows Hello first.".into());
        }
        if changed {
            return Err("App lock state changed. Retry after unlocking.".into());
        }
        Ok(())
    }

    pub fn replace_scaleway_compute(
        &self,
        current: Vec<ScalewayResourceSummary>,
    ) -> Result<ScalewayComputeReplacement, String> {
        let mut previous = self.scaleway_compute.write().map_err(|e| e.to_string())?;
        let mut initialized = self
            .scaleway_compute_initialized
            .write()
            .map_err(|e| e.to_string())?;
        let old = previous.clone();
        *previous = current;
        let had_previous_snapshot = *initialized;
        *initialized = true;
        Ok(ScalewayComputeReplacement {
            previous: old,
            had_previous_snapshot,
        })
    }

    /// Replace the cached Instance offer catalog after a successful sync.
    pub fn replace_scaleway_offers(&self, offers: Vec<ScalewayOfferSummary>) -> Result<(), String> {
        *self.scaleway_offers.write().map_err(|e| e.to_string())? = offers;
        Ok(())
    }

    /// Snapshot of the cached Instance offer catalog (empty until first sync). Read
    /// by the create dry-run to price a chosen offer without a network call.
    pub fn scaleway_offers(&self) -> Result<Vec<ScalewayOfferSummary>, String> {
        self.scaleway_offers
            .read()
            .map(|offers| offers.clone())
            .map_err(|e| e.to_string())
    }

    pub fn record_activity_events(&self, events: &[ActivityEvent]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }

        let mut history = self.activity_history.write().map_err(|e| e.to_string())?;
        let mut seen = history
            .iter()
            .map(|event| event.id.clone())
            .collect::<HashSet<_>>();
        for event in events {
            if seen.insert(event.id.clone()) {
                history.push(event.clone());
            }
        }
        history.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| a.id.cmp(&b.id)));
        history.truncate(ACTIVITY_HISTORY_LIMIT);
        Ok(())
    }

    pub fn recent_activity(&self) -> Result<Vec<ActivityEvent>, String> {
        self.activity_history
            .read()
            .map(|history| history.clone())
            .map_err(|e| e.to_string())
    }

    pub fn replace_provider_inventory(&self, inventory: ProviderInventory) -> Result<(), String> {
        let target = match inventory.health.id {
            ProviderId::Cloudflare => &self.cached_cloudflare,
            ProviderId::Scaleway => &self.cached_scaleway,
        };
        *target.write().map_err(|e| e.to_string())? = Some(inventory);
        Ok(())
    }

    pub fn clear_provider_inventory(&self, provider: ProviderId) -> Result<(), String> {
        let target = match provider {
            ProviderId::Cloudflare => &self.cached_cloudflare,
            ProviderId::Scaleway => &self.cached_scaleway,
        };
        *target.write().map_err(|e| e.to_string())? = None;

        if provider == ProviderId::Scaleway {
            self.scaleway_compute
                .write()
                .map_err(|e| e.to_string())?
                .clear();
            *self
                .scaleway_compute_initialized
                .write()
                .map_err(|e| e.to_string())? = false;
            self.scaleway_offers
                .write()
                .map_err(|e| e.to_string())?
                .clear();
        }

        Ok(())
    }

    pub fn clear_sensitive_runtime_data(&self) -> Result<(), String> {
        // LIFECYCLE: the resident Oracle server, its supervisor, and the discovery
        // file are tied to the APP PROCESS — NOT the vault lock — so agents keep
        // querying (and any in-flight index keeps running) across a lock. A vault
        // lock / idle-expiry / startup-clear therefore MUST NOT kill the server or
        // delete the discovery file; that teardown happens only on app exit
        // (`oracle_service::on_app_exit`, wired into the Tauri exit handler + Drop).
        // `on_lock` is now a no-op hook kept only to mark the lock event. We still
        // clear the app's OWN in-memory provider caches/secrets below (re-auth).
        super::oracle_service::on_lock();
        *self.cached_cloudflare.write().map_err(|e| e.to_string())? = None;
        *self.cached_scaleway.write().map_err(|e| e.to_string())? = None;
        self.scaleway_compute
            .write()
            .map_err(|e| e.to_string())?
            .clear();
        *self
            .scaleway_compute_initialized
            .write()
            .map_err(|e| e.to_string())? = false;
        self.scaleway_offers
            .write()
            .map_err(|e| e.to_string())?
            .clear();
        self.activity_history
            .write()
            .map_err(|e| e.to_string())?
            .clear();
        Ok(())
    }

    pub fn cached_provider_inventories(&self) -> Result<Vec<ProviderInventory>, String> {
        let cloudflare = self
            .cached_cloudflare
            .read()
            .map_err(|e| e.to_string())?
            .clone();
        let scaleway = self
            .cached_scaleway
            .read()
            .map_err(|e| e.to_string())?
            .clone();
        Ok([cloudflare, scaleway].into_iter().flatten().collect())
    }

    pub fn has_cloudflare_worker(
        &self,
        account_id: &str,
        worker_name: &str,
    ) -> Result<bool, String> {
        Ok(self
            .cached_cloudflare
            .read()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|inventory| {
                inventory.workers.iter().any(|worker| {
                    worker.account_id == account_id.trim() && worker.name == worker_name.trim()
                })
            })
            .unwrap_or(false))
    }

    pub fn cloudflare_selected_scope(&self) -> Result<Option<ProviderScopeSelection>, String> {
        Ok(self
            .cached_cloudflare
            .read()
            .map_err(|e| e.to_string())?
            .as_ref()
            .and_then(|inventory| inventory.selected_scope.clone()))
    }

    pub fn scaleway_health(&self) -> Result<Option<ProviderHealth>, String> {
        Ok(self
            .cached_scaleway
            .read()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|inventory| inventory.health.clone()))
    }

    pub fn scaleway_resource(
        &self,
        resource_id: &str,
    ) -> Result<Option<ScalewayResourceSummary>, String> {
        let resource_id = resource_id.trim();
        Ok(self
            .cached_scaleway
            .read()
            .map_err(|e| e.to_string())?
            .as_ref()
            .and_then(|inventory| {
                inventory
                    .compute
                    .iter()
                    .find(|resource| resource.id == resource_id)
                    .cloned()
            }))
    }

    pub fn scaleway_storage_resource(
        &self,
        resource_id: &str,
    ) -> Result<Option<ScalewayStorageSummary>, String> {
        let resource_id = resource_id.trim();
        Ok(self
            .cached_scaleway
            .read()
            .map_err(|e| e.to_string())?
            .as_ref()
            .and_then(|inventory| {
                inventory
                    .storage
                    .iter()
                    .find(|resource| resource.id == resource_id)
                    .cloned()
            }))
    }

    fn expire_if_needed(auth: &mut AuthSession) -> bool {
        if auth.locked {
            return false;
        }
        // D2: the idle TTL is measured against the MONOTONIC unlock instant, so a
        // backward wall-clock change cannot extend the unlocked window. A missing
        // monotonic instant is treated as unavailable and locks immediately.
        let Some(unlocked_instant) = auth.unlocked_instant else {
            auth.locked = true;
            auth.lock_reason = Some("unavailable".into());
            auth.session_id = auth.session_id.wrapping_add(1);
            return true;
        };
        if unlocked_instant.elapsed() > StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64) * 60) {
            auth.locked = true;
            auth.lock_reason = Some("idle".into());
            auth.session_id = auth.session_id.wrapping_add(1);
            return true;
        }
        false
    }
}

impl Drop for BackendState {
    fn drop(&mut self) {
        // BackendState is dropped on app process teardown, so this is an app-EXIT
        // path: run the full Oracle teardown (stop supervisor, kill the server
        // child, delete the discovery file). `on_app_exit` does NO network I/O (it
        // deliberately skips the courtesy watcher-stop HTTP and only kills + bounded-
        // reaps the child), so it is safe to call while the tokio runtime is tearing
        // down — a reqwest::blocking client/response constructed or dropped here
        // would otherwise panic ("Cannot drop a runtime in a context where blocking
        // is not allowed"). Idempotent with the `RunEvent::Exit` handler that also
        // calls it. On Windows this is what reaps the otherwise-orphaned server.
        #[cfg(test)]
        {
            // Unit tests construct/drop BackendState without a real Oracle runtime;
            // only the integration path opts into the real teardown.
            if std::env::var_os("ASPIS_TEST_STOP_ORACLE_ON_STATE_CLEAR").is_none() {
                return;
            }
        }
        super::oracle_service::on_app_exit();
    }
}

fn oracle_service_on_unlock() {
    #[cfg(test)]
    {
        // Unit tests exercise unlock/lock transitions in a tight loop without a
        // real Oracle runtime; spawning the supervisor (or touching the network)
        // there is undesirable. The integration test opts in via this env var.
        if std::env::var_os("ASPIS_TEST_STOP_ORACLE_ON_STATE_CLEAR").is_none() {
            return;
        }
    }
    super::oracle_service::on_unlock();
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn sensitive_gate_blocks_when_locked() {
        let state = BackendState::new();
        assert!(state.ensure_unlocked().is_err());
    }

    #[test]
    fn sensitive_gate_expires_after_ttl() {
        let state = BackendState::new();
        {
            let mut auth = state.auth.write().unwrap();
            auth.locked = false;
            auth.last_unlocked_at = Some(Utc::now() - Duration::minutes(UNLOCK_TTL_MINUTES + 1));
            // D2: expiry is now measured against the monotonic instant.
            auth.unlocked_instant =
                Some(Instant::now() - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 1) * 60));
            auth.lock_reason = None;
        }

        assert!(state.ensure_unlocked().is_err());
        assert_eq!(
            state.auth_state().unwrap().lock_reason.as_deref(),
            Some("idle")
        );
    }

    #[test]
    fn idle_ttl_uses_monotonic_clock_not_wall_clock() {
        // D2: pushing the wall-clock timestamp far into the FUTURE (simulating a
        // backward system-clock change relative to unlock) must NOT keep the
        // session unlocked once the monotonic TTL elapses.
        let state = BackendState::new();
        {
            let mut auth = state.auth.write().unwrap();
            auth.locked = false;
            // Wall clock claims we just unlocked (or even in the future), but the
            // monotonic instant is already past the TTL.
            auth.last_unlocked_at = Some(Utc::now() + Duration::minutes(60));
            auth.unlocked_instant =
                Some(Instant::now() - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 1) * 60));
            auth.lock_reason = None;
        }

        assert!(state.ensure_unlocked().is_err());
        assert_eq!(
            state.auth_state().unwrap().lock_reason.as_deref(),
            Some("idle")
        );
    }

    #[test]
    fn sensitive_gate_expiry_clears_runtime_provider_cache() {
        let state = BackendState::new();
        state.unlock_after_verification().unwrap();
        state
            .replace_provider_inventory(ProviderInventory::missing(ProviderId::Cloudflare))
            .unwrap();
        {
            let mut auth = state.auth.write().unwrap();
            auth.last_unlocked_at = Some(Utc::now() - Duration::minutes(UNLOCK_TTL_MINUTES + 1));
            // D2: drive expiry via the monotonic instant.
            auth.unlocked_instant =
                Some(Instant::now() - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 1) * 60));
        }

        assert!(state.ensure_unlocked().is_err());

        assert!(state.cached_provider_inventories().unwrap().is_empty());
    }

    #[test]
    fn sensitive_session_rejects_operations_after_relock() {
        let state = BackendState::new();
        state.unlock_after_verification().unwrap();
        let session_id = state.sensitive_session_id().unwrap();

        state.lock("manual").unwrap();
        state.unlock_after_verification().unwrap();

        assert!(state.ensure_same_sensitive_session(session_id).is_err());
    }

    #[test]
    fn activity_history_keeps_recent_events_newest_first_without_duplicates() {
        let state = BackendState::new();
        let first = ActivityEvent {
            id: "scw_spawn_gpu-1_running".into(),
            message: "gpu-1 appeared as running GPU in fr-par-1.".into(),
            timestamp: "2026-05-27T10:00:00Z".into(),
            event_type: "spawn".into(),
            source: "Scaleway".into(),
        };
        let second = ActivityEvent {
            id: "scw_state_vm-1_stopped_running".into(),
            message: "vm-1 changed state stopped -> running.".into(),
            timestamp: "2026-05-27T10:01:00Z".into(),
            event_type: "scale".into(),
            source: "Scaleway".into(),
        };

        state.record_activity_events(&[first.clone()]).unwrap();
        state
            .record_activity_events(&[second.clone(), first.clone()])
            .unwrap();

        let history = state.recent_activity().unwrap();

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, second.id);
        assert_eq!(history[1].id, first.id);
    }

    #[test]
    fn first_scaleway_inventory_replacement_is_baseline_not_live_change() {
        let state = BackendState::new();
        let resource = ScalewayResourceSummary {
            id: "gpu-1".into(),
            name: "gpu-1".into(),
            resource_type: "GPU".into(),
            region: "fr-par-1".into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: "running".into(),
            commercial_type: Some("GPU-3070-S".into()),
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose: "model-training".into(),
            purpose_source: "tag".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query: "gpu-1 model-training".into(),
            available_actions: Vec::new(),
            idle_cost_risk: false,
        };

        let first = state
            .replace_scaleway_compute(vec![resource.clone()])
            .unwrap();
        let second = state.replace_scaleway_compute(vec![resource]).unwrap();

        assert!(!first.had_previous_snapshot);
        assert!(first.previous.is_empty());
        assert!(second.had_previous_snapshot);
        assert_eq!(second.previous.len(), 1);
    }

    #[test]
    fn provider_inventory_cache_preserves_other_provider_during_partial_sync() {
        let state = BackendState::new();
        let cloudflare = ProviderInventory::missing(ProviderId::Cloudflare);
        let mut scaleway = ProviderInventory::missing(ProviderId::Scaleway);
        scaleway.health.status = "healthy".into();

        state
            .replace_provider_inventory(cloudflare.clone())
            .unwrap();
        state.replace_provider_inventory(scaleway.clone()).unwrap();

        let cached = state.cached_provider_inventories().unwrap();

        assert_eq!(cached.len(), 2);
        assert_eq!(cached[0].health.id, ProviderId::Cloudflare);
        assert_eq!(cached[1].health.id, ProviderId::Scaleway);
        assert_eq!(cached[1].health.status, "healthy");
    }

    #[test]
    fn clearing_provider_inventory_removes_stale_cache_and_resets_scaleway_baseline() {
        let state = BackendState::new();
        let cloudflare = ProviderInventory::missing(ProviderId::Cloudflare);
        let mut scaleway = ProviderInventory::missing(ProviderId::Scaleway);
        scaleway.health.status = "healthy".into();
        scaleway.compute.push(ScalewayResourceSummary {
            id: "gpu-1".into(),
            name: "gpu-1".into(),
            resource_type: "GPU".into(),
            region: "fr-par-1".into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: "running".into(),
            commercial_type: Some("GPU-3070-S".into()),
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose: "model-training".into(),
            purpose_source: "tag".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query: "gpu-1 model-training".into(),
            available_actions: Vec::new(),
            idle_cost_risk: false,
        });

        state.replace_provider_inventory(cloudflare).unwrap();
        state.replace_provider_inventory(scaleway.clone()).unwrap();
        state
            .replace_scaleway_compute(scaleway.compute.clone())
            .unwrap();

        state
            .clear_provider_inventory(ProviderId::Scaleway)
            .unwrap();

        let cached = state.cached_provider_inventories().unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].health.id, ProviderId::Cloudflare);
        let replacement = state
            .replace_scaleway_compute(scaleway.compute.clone())
            .unwrap();
        assert!(!replacement.had_previous_snapshot);
        assert!(replacement.previous.is_empty());
    }

    #[test]
    fn locking_clears_runtime_provider_cache() {
        let state = BackendState::new();
        state.unlock_after_verification().unwrap();
        state
            .replace_provider_inventory(ProviderInventory::missing(ProviderId::Cloudflare))
            .unwrap();
        assert_eq!(state.cached_provider_inventories().unwrap().len(), 1);

        state.lock("manual").unwrap();

        assert!(state.cached_provider_inventories().unwrap().is_empty());
        assert!(state.ensure_unlocked().is_err());
    }

    #[test]
    fn cloudflare_worker_lookup_is_bound_to_cached_inventory() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Cloudflare);
        inventory
            .workers
            .push(super::super::model::CloudflareWorkerSummary {
                id: "worker-1".into(),
                account_id: "023e105f4ecef8ad9ca31a8372d0c353".into(),
                account_name: None,
                name: "worker-1".into(),
                status: "healthy".into(),
                purpose: "test".into(),
                purpose_source: "test".into(),
                routes: Vec::new(),
                last_deploy: None,
                usage_model: None,
                compatibility_date: None,
                compatibility_flags: Vec::new(),
                handlers: Vec::new(),
                tags: Vec::new(),
                oracle_query: "worker-1".into(),
            });
        state.replace_provider_inventory(inventory).unwrap();

        assert!(state
            .has_cloudflare_worker("023e105f4ecef8ad9ca31a8372d0c353", "worker-1")
            .unwrap());
        assert!(!state
            .has_cloudflare_worker("023e105f4ecef8ad9ca31a8372d0c353", "other-worker")
            .unwrap());
    }

    #[test]
    fn scaleway_resource_lookup_is_bound_to_cached_inventory() {
        let state = BackendState::new();
        let mut inventory = ProviderInventory::missing(ProviderId::Scaleway);
        inventory.compute.push(ScalewayResourceSummary {
            id: "srv-1".into(),
            name: "cpu-a".into(),
            resource_type: "CPU VM".into(),
            region: "fr-par-1".into(),
            project_id: Some("bio-project".into()),
            project_name: Some("Aspis Bio".into()),
            state: "running".into(),
            commercial_type: Some("DEV1-S".into()),
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            endpoint: None,
            privacy: None,
            purpose: "test".into(),
            purpose_source: "test".into(),
            tags: Vec::new(),
            image: None,
            public_ip: None,
            created_at: None,
            updated_at: None,
            oracle_query: "cpu-a".into(),
            available_actions: vec!["poweroff".into(), "reboot".into(), "terminate".into()],
            idle_cost_risk: false,
        });
        state.replace_provider_inventory(inventory).unwrap();

        assert_eq!(
            state.scaleway_resource("srv-1").unwrap().unwrap().name,
            "cpu-a"
        );
        assert!(state.scaleway_resource("missing").unwrap().is_none());
    }
}
