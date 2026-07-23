use super::auth;
use super::model::AuthState;
use chrono::{DateTime, Utc};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration as StdDuration, Instant};

const UNLOCK_TTL_MINUTES: i64 = 15;

/// DEV ONLY: skip Touch ID / Hello and idle soft-lock so agents can drive the
/// app overnight without a human at the keyboard.
///
/// - **Release builds:** always off.
/// - **Unit tests (`cfg(test)`):** off by default (real lock semantics), unless
///   `DEVBOULE_DEV_UNLOCK=1` is set for a deliberate test.
/// - **Debug app runs:** on by default; set `DEVBOULE_DEV_UNLOCK=0` to exercise
///   the real lock screen.
pub fn dev_unlock_enabled() -> bool {
    #[cfg(not(debug_assertions))]
    {
        return false;
    }
    #[cfg(debug_assertions)]
    {
        match std::env::var("DEVBOULE_DEV_UNLOCK")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("0") | Some("false") | Some("no") | Some("off") => false,
            Some("1") | Some("true") | Some("yes") | Some("on") => true,
            // cfg(test): keep lock tests honest unless the env opts in.
            None | Some("") if cfg!(test) => false,
            // Debug cargo run / tauri dev / pilot: stay unlocked.
            None | Some("") => true,
            Some(_) => true,
        }
    }
}

#[derive(Debug)]
struct AuthSession {
    locked: bool,
    /// User-facing unlock timestamp (wall clock). NOT used for expiry math.
    last_unlocked_at: Option<DateTime<Utc>>,
    /// D2: monotonic instant of the last genuine user activity while unlocked
    /// (set on unlock; refreshed only via `touch_idle_activity`, never by
    /// background IPC / `ensure_unlocked` pollers). Used for the idle-TTL
    /// comparison so a backward wall-clock change cannot extend the unlocked
    /// window, and only real user interaction keeps the session open.
    unlocked_instant: Option<Instant>,
    lock_reason: Option<String>,
    session_id: u64,
}

pub struct BackendState {
    auth: RwLock<AuthSession>,
    auth_prompt: Mutex<()>,
    auth_retry_after: Mutex<Option<Instant>>,
    hello_available: OnceLock<bool>,
    /// Captured once at construction from `dev_unlock_enabled()` so AuthState
    /// reports a stable `devUnlock` without re-reading the env on every poll.
    dev_unlock: bool,
    /// Shared HTTP client for credentialed outbound calls (pigeon, censor/gemma).
    /// Redirects are DISABLED (`Policy::none`) so a cross-host 3xx cannot replay
    /// Authorization headers off the intended host. Callers that need redirects
    /// without auth must use a separate non-auth client.
    pub http: reqwest::Client,
}

/// Builds the shared authed HTTP client. Redirects are always off so credentialed
/// requests cannot be 3xx-replayed onto a different host.
fn build_shared_authed_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Devboule/0.1")
        .timeout(StdDuration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build reqwest client")
}

impl BackendState {
    pub fn new() -> Self {
        let http = build_shared_authed_http_client();

        let dev = dev_unlock_enabled();
        if dev {
            eprintln!(
                "[devboule] DEV unlock active — no Touch ID / Hello, no idle soft-lock \
                 (set DEVBOULE_DEV_UNLOCK=0 to restore real lock)"
            );
        }
        Self {
            auth: RwLock::new(AuthSession {
                locked: !dev,
                last_unlocked_at: if dev { Some(Utc::now()) } else { None },
                unlocked_instant: if dev { Some(Instant::now()) } else { None },
                lock_reason: if dev { None } else { Some("startup".into()) },
                session_id: 0,
            }),
            auth_prompt: Mutex::new(()),
            auth_retry_after: Mutex::new(None),
            hello_available: OnceLock::new(),
            dev_unlock: dev,
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
            dev_unlock: self.dev_unlock,
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
        // Debug / pilot overnight: no biometric prompt.
        if dev_unlock_enabled() {
            return self.unlock_after_verification();
        }

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
        // Idle TTL tracks genuine user activity only (`touch_idle_activity`).
        // Background pollers that gate via ensure_unlocked must NOT refresh
        // the clock — that would defeat soft-lock while the window is visible.
        drop(auth);
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        if locked {
            return Err("App is locked. Unlock to continue.".into());
        }
        Ok(())
    }

    /// Refresh the idle-TTL clock after genuine user activity (pointer/key).
    /// Does not extend the window when already locked or when expire fires.
    pub fn touch_idle_activity(&self) -> Result<(), String> {
        let mut auth = self.auth.write().map_err(|e| e.to_string())?;
        let expired = Self::expire_if_needed(&mut auth);
        let locked = auth.locked;
        if !locked {
            auth.unlocked_instant = Some(Instant::now());
        }
        drop(auth);
        if expired {
            self.clear_sensitive_runtime_data()?;
        }
        if locked {
            return Err("App is locked. Unlock to continue.".into());
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
            return Err("App is locked. Unlock to continue.".into());
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
            return Err("App is locked. Unlock to continue.".into());
        }
        if changed {
            return Err("App lock state changed. Retry after unlocking.".into());
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
        // `on_lock` is now a no-op hook kept only to mark the lock event.
        super::oracle_service::on_lock();
        Ok(())
    }

    fn expire_if_needed(auth: &mut AuthSession) -> bool {
        if auth.locked {
            return false;
        }
        // Debug overnight: never soft-lock on idle (Touch ID would block agents).
        if dev_unlock_enabled() {
            return false;
        }
        // D2: the idle TTL is measured against the MONOTONIC last-activity instant
        // (refreshed only by genuine user activity via touch_idle_activity), so a
        // backward wall-clock change cannot extend the unlocked window and real
        // interactive use stays open without background IPC defeating the lock.
        // A missing monotonic instant is treated as unavailable and locks immediately.
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

/// F40: crate-visible lock for process-wide `DEVBOULE_DEV_UNLOCK` mutations.
/// Any test (any module) that sets/clears this env OR asserts locked/unlocked
/// baseline after `BackendState::new()` must hold this mutex for the duration.
#[cfg(test)]
pub(crate) static DEV_UNLOCK_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::sync::MutexGuard;

    fn lock_dev_unlock_env() -> MutexGuard<'static, ()> {
        DEV_UNLOCK_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Run `f` with DEV unlock env forced OFF (locked baseline). Restores prior value.
    fn with_dev_unlock_env_off<R>(f: impl FnOnce() -> R) -> R {
        let _g = lock_dev_unlock_env();
        let prev = std::env::var_os("DEVBOULE_DEV_UNLOCK");
        std::env::remove_var("DEVBOULE_DEV_UNLOCK");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var("DEVBOULE_DEV_UNLOCK", v),
            None => std::env::remove_var("DEVBOULE_DEV_UNLOCK"),
        }
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    /// Run `f` with DEV unlock env forced ON. Restores prior value.
    fn with_dev_unlock_env_on<R>(f: impl FnOnce() -> R) -> R {
        let _g = lock_dev_unlock_env();
        let prev = std::env::var_os("DEVBOULE_DEV_UNLOCK");
        std::env::set_var("DEVBOULE_DEV_UNLOCK", "1");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var("DEVBOULE_DEV_UNLOCK", v),
            None => std::env::remove_var("DEVBOULE_DEV_UNLOCK"),
        }
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn sensitive_gate_blocks_when_locked() {
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            assert!(state.ensure_unlocked().is_err());
        });
    }

    #[test]
    fn dev_unlock_skips_biometric_and_idle_ttl() {
        with_dev_unlock_env_on(|| {
            let state = BackendState::new();
            assert!(
                !state.auth_state().unwrap().locked,
                "debug + DEVBOULE_DEV_UNLOCK=1 starts unlocked"
            );
            assert!(state.ensure_unlocked().is_ok());
            // Far past idle TTL — still open.
            {
                let mut auth = state.auth.write().unwrap();
                auth.unlocked_instant = Some(
                    Instant::now()
                        - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 10) * 60),
                );
            }
            assert!(
                state.ensure_unlocked().is_ok(),
                "dev unlock must not idle-expire"
            );
            // No biometric path required.
            assert!(state.verify_unlock("dev").is_ok());
        });
    }

    #[test]
    fn sensitive_gate_expires_after_ttl() {
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            {
                let mut auth = state.auth.write().unwrap();
                auth.locked = false;
                auth.last_unlocked_at =
                    Some(Utc::now() - Duration::minutes(UNLOCK_TTL_MINUTES + 1));
                // D2: expiry is now measured against the monotonic instant.
                auth.unlocked_instant = Some(
                    Instant::now()
                        - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 1) * 60),
                );
                auth.lock_reason = None;
            }

            let err = state.ensure_unlocked().unwrap_err();
            assert!(err.contains("App is locked"), "{err}");
            assert_eq!(
                state.auth_state().unwrap().lock_reason.as_deref(),
                Some("idle")
            );
        });
    }

    #[test]
    fn ensure_unlocked_does_not_refresh_idle_ttl() {
        // ensure_unlocked is a pure gate: background pollers must not reset idle.
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            let almost_expired = Instant::now()
                - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64) * 60 - 30);
            {
                let mut auth = state.auth.write().unwrap();
                auth.locked = false;
                auth.last_unlocked_at = Some(Utc::now());
                auth.unlocked_instant = Some(almost_expired);
                auth.lock_reason = None;
            }

            assert!(state.ensure_unlocked().is_ok());
            let elapsed = {
                let auth = state.auth.read().unwrap();
                auth.unlocked_instant.expect("unchanged").elapsed()
            };
            // Still near TTL (≈30s remaining), not refreshed to "now".
            assert!(
                elapsed > StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64) * 60 - 60),
                "ensure_unlocked must not refresh idle clock, elapsed={elapsed:?}"
            );
            assert!(!state.auth_state().unwrap().locked);
        });
    }

    #[test]
    fn touch_idle_activity_refreshes_idle_ttl_when_unlocked() {
        // Genuine user activity extends the expire window.
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            {
                let mut auth = state.auth.write().unwrap();
                auth.locked = false;
                auth.last_unlocked_at = Some(Utc::now());
                // Almost expired (TTL - 30s). Without touch, waiting would expire.
                auth.unlocked_instant = Some(
                    Instant::now()
                        - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64) * 60 - 30),
                );
                auth.lock_reason = None;
            }

            assert!(state.touch_idle_activity().is_ok());
            let elapsed = {
                let auth = state.auth.read().unwrap();
                auth.unlocked_instant.expect("refreshed").elapsed()
            };
            assert!(
                elapsed < StdDuration::from_secs(5),
                "expected touch to Instant::now(), elapsed={elapsed:?}"
            );
            // Expire window extended: ensure_unlocked still succeeds after the touch.
            assert!(state.ensure_unlocked().is_ok());
            assert!(!state.auth_state().unwrap().locked);
        });
    }

    #[test]
    fn touch_idle_activity_errors_when_locked() {
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            let err = state.touch_idle_activity().unwrap_err();
            assert!(err.contains("App is locked"), "{err}");
        });
    }

    #[test]
    fn idle_ttl_uses_monotonic_clock_not_wall_clock() {
        // D2: pushing the wall-clock timestamp far into the FUTURE (simulating a
        // backward system-clock change relative to unlock) must NOT keep the
        // session unlocked once the monotonic TTL elapses.
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            {
                let mut auth = state.auth.write().unwrap();
                auth.locked = false;
                // Wall clock claims we just unlocked (or even in the future), but the
                // monotonic instant is already past the TTL.
                auth.last_unlocked_at = Some(Utc::now() + Duration::minutes(60));
                auth.unlocked_instant = Some(
                    Instant::now()
                        - StdDuration::from_secs((UNLOCK_TTL_MINUTES as u64 + 1) * 60),
                );
                auth.lock_reason = None;
            }

            assert!(state.ensure_unlocked().is_err());
            assert_eq!(
                state.auth_state().unwrap().lock_reason.as_deref(),
                Some("idle")
            );
        });
    }

    #[test]
    fn sensitive_session_rejects_operations_after_relock() {
        with_dev_unlock_env_off(|| {
            let state = BackendState::new();
            state.unlock_after_verification().unwrap();
            let session_id = state.sensitive_session_id().unwrap();

            state.lock("manual").unwrap();
            state.unlock_after_verification().unwrap();

            assert!(state.ensure_same_sensitive_session(session_id).is_err());
        });
    }

    #[test]
    fn shared_authed_http_client_does_not_follow_redirects() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = "HTTP/1.1 302 Found\r\n\
                        Location: http://127.0.0.1:9/exfil\r\n\
                        Content-Length: 0\r\n\
                        Connection: close\r\n\r\n";
            let _ = stream.write_all(body.as_bytes());
        });

        let client = build_shared_authed_http_client();
        let url = format!("http://{addr}/probe");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let status = rt.block_on(async {
            client
                .get(&url)
                .send()
                .await
                .expect("request completes without following off-host")
                .status()
        });
        assert_eq!(
            status.as_u16(),
            302,
            "authed client must surface the 3xx instead of following Location"
        );
        server.join().expect("server thread");
    }
}
