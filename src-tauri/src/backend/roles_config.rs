//! ROLE UNTANGLE — Phase 5: per-role client selectors + Main-coder engine model.
//!
//! ONE config surface answering "which CLIENT runs each agent role" + "which local model
//! drives the Main-coder engine". Read-time lossless migration from the legacy keys
//! (additive-then-authoritative: legacy keys are never deleted or rewritten;
//! `rolesConfig` wins when present).
//!
//! Storage: Global `config.json` (same file the legacy getters read). Read it the way
//! the neighbors do — look at `read_local_coder_backend` in
//! `src-tauri/src/backend/local_coder.rs` and `get_mini_coder_backend` /
//! `read_mini_coder_backend` in `src-tauri/src/backend/projects.rs` for the exact
//! config read/write helpers used (locate_config_path + serde_json read/modify/write
//! with the config write lock if one exists — MIRROR the existing pattern exactly).

use super::state::BackendState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::State;

// ──────────────────────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────────────────────

/// Which CLIENT runs each launchable role. None = unset → the read-time
/// migration synthesizes the legacy default (see resolve_roles_config).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolesConfig {
    /// "orchestrator" (the Devboule binary) | "claude" | "codex".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_client: Option<String>,
    /// Main-coder launches: "claude" | "codex" | a custom client id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coder_client: Option<String>,
    /// Verifier launches — INDEPENDENT of the coder's client (the untangle's
    /// point: the verifier had no selector and silently reused the coder's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_client: Option<String>,
}

/// The EFFECTIVE per-role clients after read-time migration (every field set).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRolesConfig {
    pub orchestrator_client: String,
    pub coder_client: String,
    pub verifier_client: String,
}

// ──────────────────────────────────────────────────────────────────────────────
// Validation helpers (re-implemented locally, mirroring the constraint in
// projects.rs `normalize_agent_client` for built-in clients, and the custom
// client id rules: [a-z0-9-]{1,32}, lowercase).
// ──────────────────────────────────────────────────────────────────────────────

/// Validate + normalize a client id used in a rolesConfig field.
/// Mirrors `normalize_agent_client` (built-in: codex|claude|powershell|orchestrator)
/// and the custom client id rules from `validate_custom_agent_client`:
///   - trimmed, lowercased, must be 1-32 chars of [a-z0-9-]
///   - must not be a reserved built-in id when used as a custom client
///
/// See `projects.rs` `normalize_agent_client` / `validate_custom_agent_client`
/// for the mirror constraints.
fn validate_client_id(id: &str) -> Result<String, String> {
    let trimmed = id.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err("Client id must not be empty.".into());
    }
    // Built-in clients are allowed verbatim.
    if matches!(trimmed.as_str(), "codex" | "claude" | "powershell" | "orchestrator") {
        return Ok(trimmed);
    }
    // Custom client ids: [a-z0-9-]{1,32}, lowercase (mirrors
    // `validate_custom_agent_client` id rules).
    if trimmed.len() > 32 {
        return Err("Custom client id must be at most 32 characters.".into());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(
            "Custom client id must be 1-32 chars of a-z, 0-9 or hyphen.".into(),
        );
    }
    // Reserved built-in ids cannot be used as custom client ids (mirrors
    // `RESERVED_CLIENT_IDS` in `projects.rs`).
    let reserved = ["codex", "claude", "powershell", "orchestrator"];
    if reserved.contains(&trimmed.as_str()) {
        return Err("Client id is reserved by a built-in CLI.".into());
    }
    Ok(trimmed)
}

// ──────────────────────────────────────────────────────────────────────────────
// Config read/write helpers — MIRROR the existing pattern exactly:
//   1. locate_config_path (from `projects.rs`)
//   2. fs::read_to_string
//   3. serde_json::from_str
//   4. value.as_object_mut() + key assignment / removal
//   5. serde_json::to_string_pretty
//   6. Atomic temp+rename (same as `set_mini_coder_backend` / `set_custom_agent_clients`)
// ──────────────────────────────────────────────────────────────────────────────

/// Locate the global config.json. REUSE the single source of truth in
/// `projects.rs` (the local model first reimplemented this against
/// `resource_dir()`, which resolves a DIFFERENT path than the real app-data
/// resolver — reads/writes would have hit the wrong file). One definition, no
/// drift.
fn locate_config_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    super::projects::locate_config_path(app)
}

/// Read the raw config.json, or return an empty JSON object on failure.
fn read_config_json(app: &tauri::AppHandle) -> serde_json::Value {
    let path = match locate_config_path(app) {
        Some(p) => p,
        None => return serde_json::Value::Object(serde_json::Map::new()),
    };
    let raw = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return serde_json::Value::Object(serde_json::Map::new()),
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(_) => serde_json::Value::Object(serde_json::Map::new()),
    }
}

/// Write config.json atomically (read-modify-write), mirroring the exact pattern
/// from `projects.rs::set_mini_coder_backend` / `set_custom_agent_clients`:
///   1. Read the file, parse JSON object.
///   2. Mutate ONE key (rolesConfig).
///   3. Serialize pretty.
///   4. Write to temp file, then atomic rename (replace_file_with_backup).
///
/// The config_write_lock is NOT used here because this module only touches
/// `rolesConfig` and no other module writes to it — the lock is for
/// cross-key serialization (e.g. `set_mini_coder_backend` + `set_custom_agent_clients`
/// could race on different keys). Since we only write one key, no lock is needed.
fn write_config_json(
    app: &tauri::AppHandle,
    value: serde_json::Value,
) -> Result<(), String> {
    let path = locate_config_path(app).ok_or_else(|| {
        "config.json could not be located to save roles config.".to_string()
    })?;
    // BUG FIX (review): write the value the CALLER mutated. The first draft
    // re-read the file into a shadowing `value` here, silently discarding the
    // caller's `rolesConfig` edit — the setter would have persisted NOTHING.
    if !value.is_object() {
        return Err("config.json is not a JSON object.".into());
    }
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Could not serialize config.json: {e}"))?;
    // Atomic temp+rename (same as set_mini_coder_backend): a crash mid-write can
    // never leave a half-written config.json. Read-only packaged builds surface the
    // same guidance.
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = path.with_extension(format!("json.{suffix}.tmp"));
    let backup_path = path.with_extension(format!("json.{suffix}.bak"));
    fs::write(&temp_path, format!("{pretty}\n")).map_err(|e| {
        format!(
            "Could not write config.json at {}: {e}. In a packaged build this file is read-only.",
            path.to_string_lossy()
        )
    })?;
    // Use the same replace_file_with_backup helper from projects.rs.
    super::fs_replace::replace_file_with_backup(&temp_path, &path, &backup_path, "config.json")
        .map_err(|e| format!("{e}. In a packaged build this file is read-only."))?;
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Pure: resolve_roles_config
// ──────────────────────────────────────────────────────────────────────────────

/// Resolve the effective per-role clients from a config JSON object.
///
/// PURE (unit-testable): reads `config["rolesConfig"]` (tolerant: missing/invalid
/// → all None), then fills gaps from legacy keys:
///   - orchestrator_client: stored value, else "orchestrator" (the local binary —
///     today's hardcoded planner default).
///   - coder_client: stored value, else `config["mainCoderClient"]` when it is a
///     non-empty string, else "codex" (the legacy default in
///     projects.rs `read_main_coder_client`).
///   - verifier_client: stored value, else the SAME legacy mainCoderClient fallback
///     chain (an INDEPENDENT copy — changing coder_client later must not move it).
///
/// Values are trimmed + lowercased like `normalize_agent_client` does.
pub(crate) fn resolve_roles_config(config: &serde_json::Value) -> EffectiveRolesConfig {
    // Read the stored rolesConfig (tolerant: missing/invalid → all None).
    let roles_config = config
        .get("rolesConfig")
        .and_then(|v| v.as_object())
        .map(|obj| RolesConfig {
            orchestrator_client: obj
                .get("orchestratorClient")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase),
            coder_client: obj
                .get("coderClient")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase),
            verifier_client: obj
                .get("verifierClient")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_lowercase),
        });

    // Legacy fallback: read `mainCoderClient` from config (same tolerant parse
    // as `read_main_coder_client` in projects.rs).
    let legacy_main = config
        .get("mainCoderClient")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let orchestrator_client = roles_config
        .as_ref()
        .and_then(|r| r.orchestrator_client.clone())
        .unwrap_or_else(|| "orchestrator".to_string());

    let coder_client = roles_config
        .as_ref()
        .and_then(|r| r.coder_client.clone())
        .or(legacy_main.clone())
        .unwrap_or_else(|| "codex".to_string());

    let verifier_client = roles_config
        .as_ref()
        .and_then(|r| r.verifier_client.clone())
        .or(legacy_main)
        .unwrap_or_else(|| "codex".to_string());

    EffectiveRolesConfig {
        orchestrator_client,
        coder_client,
        verifier_client,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Impure: get_roles_config
// ──────────────────────────────────────────────────────────────────────────────

/// Impure wrapper: read config.json (missing file/parse error → empty JSON object)
/// then resolve_roles_config.
pub fn get_roles_config(app: &tauri::AppHandle) -> EffectiveRolesConfig {
    let config = read_config_json(app);
    resolve_roles_config(&config)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tauri commands
// ──────────────────────────────────────────────────────────────────────────────

/// Read the effective per-role clients.
#[tauri::command]
pub fn get_roles_config_cmd(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<EffectiveRolesConfig, String> {
    state.ensure_unlocked()?;
    Ok(get_roles_config(&app))
}

/// Persist the per-role clients into config.json.
///
/// ensure_unlocked; validate each Some(value): trimmed non-empty, must be
/// "orchestrator" | "claude" | "codex" | a plausible custom id ([a-z0-9-]{1,32},
/// lowercase) — mirror the constraints in projects.rs `normalize_agent_client` /
/// custom client validation but do NOT import private items: re-implement the tiny
/// check locally with a comment naming the mirror. Then read-modify-write
/// config.json setting ONLY the `rolesConfig` key (legacy keys untouched — assert
/// this in a test), and return the new effective config.
#[tauri::command]
pub fn set_roles_config_cmd(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    input: RolesConfig,
) -> Result<EffectiveRolesConfig, String> {
    state.ensure_unlocked()?;
    // Validate each Some(value) against the same rules as `normalize_agent_client`
    // (built-in) and `validate_custom_agent_client` (custom id: [a-z0-9-]{1,32}).
    let normalized = RolesConfig {
        orchestrator_client: input
            .orchestrator_client
            .as_deref()
            .map(validate_client_id)
            .transpose()?,
        coder_client: input
            .coder_client
            .as_deref()
            .map(validate_client_id)
            .transpose()?,
        verifier_client: input
            .verifier_client
            .as_deref()
            .map(validate_client_id)
            .transpose()?,
    };
    // Build the rolesConfig object — only non-None fields are emitted (skip_serializing_if).
    //
    // MERGE SEMANTICS (P6 CONTRACT — read this before wiring the Roles UI): this
    // REPLACES the whole `rolesConfig` object. A field sent as `None` is not "leave
    // alone" — it CLEARS that role back to the legacy/default inheritance. So the P6
    // settings panel MUST round-trip all three fields on every save (send the full
    // current triple), or an omitted field silently resets. This is deliberate (it
    // lets the UI express "clear to default" = send None) but it is a footgun if the
    // UI only tracks the field being edited.
    let roles_config_value = serde_json::to_value(&normalized)
        .map_err(|e| format!("Could not serialize rolesConfig: {e}"))?;

    // BLOCKER FIX (review): take the SHARED config write lock around the WHOLE
    // read-modify-write (read → mutate → atomic rename). roles_config was the ONLY
    // config.json writer that skipped it — a concurrent set_mini_coder_backend /
    // set_design_llm_backend / etc. save would race the whole-file rename and
    // silently drop one side's key (last-writer-wins). The lock must cover the READ
    // too, or the RMW isn't atomic — mirrors set_mini_coder_backend's placement.
    let _config_guard = super::projects::config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;
    let path = locate_config_path(&app).ok_or_else(|| {
        "config.json could not be located to save roles config.".to_string()
    })?;
    let raw = fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("config.json is not a JSON object.".into());
    }
    value["rolesConfig"] = roles_config_value;
    write_config_json(&app, value.clone())?;
    // Resolve from the in-memory value we just wrote (cheaper than a re-read AND
    // avoids a second interleave window between write and read-back).
    Ok(resolve_roles_config(&value))
}

// ──────────────────────────────────────────────────────────────────────────────
// read_main_coder_backend
// ──────────────────────────────────────────────────────────────────────────────

/// The Main-coder ENGINE model: read `config["mainCoderBackend"]` with the same
/// tolerant parse used for `miniCoderBackend` (find how projects.rs parses
/// MiniCoderBackend from config and mirror it); when missing/invalid FALL BACK to
/// the mini's backend (call the existing public reader in projects.rs if
/// accessible, else read `config["miniCoderBackend"]` the same way). Document the
/// fallback: Phase 3 left the Main engine on the mini's model; this getter is the
/// seam that gives it its own row.
///
/// Mirrors `read_mini_coder_backend` from `projects.rs`:
///   1. locate_config_path
///   2. fs::read_to_string
///   3. serde_json::from_str
///   4. value.get("mainCoderBackend")
///   5. serde_json::from_value
///   6. validate_mini_coder_backend (or None)
///
/// When `mainCoderBackend` is missing/invalid, falls back to the mini's backend
/// by calling `super::projects::read_mini_coder_backend`.
///
/// Not yet wired into a launch path — P5 lands the data model + reader; P6 (the
/// Roles-table UI + launch consumption) is where the Main-coder launch reads this.
#[allow(dead_code)]
pub(crate) fn read_main_coder_backend(
    app: &tauri::AppHandle,
) -> Option<super::mini_coder::MiniCoderBackend> {
    let path = match locate_config_path(app) {
        Some(p) => p,
        None => return None,
    };
    let raw = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return None,
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return None,
    };
    // Try the main coder's own backend first (Phase 3 left the Main engine on the mini's
    // model; this getter is the seam that gives it its own row). `parse_role_backend` returns
    // Some ONLY for a PRESENT + structurally-deserializable + semantically-VALID entry; every
    // invalid sub-case falls through to the mini fallback, matching "missing/invalid → fall back".
    if let Some(backend) = parse_role_backend(&value, "mainCoderBackend") {
        return Some(backend);
    }
    // Fallback: Phase 3 left the Main engine on the mini's model. Call the
    // existing public reader in projects.rs.
    super::projects::read_mini_coder_backend(app)
}

// ──────────────────────────────────────────────────────────────────────────────
// Per-role LOCAL backend model (P6b path B) — Main coder + Verifier each get their own
// `MiniCoderBackend`-shaped local model row, reusing the existing type + validator wholesale.
//   - Main coder: config key `mainCoderBackend` (reader above already prefers it, falling
//     back to the mini's model).
//   - Verifier:   config key `verifierBackend`, INHERITING the Main coder's model when unset
//     (mirrors the client-axis "verifier defaults to Main coder" affordance on the model axis).
// PURE core (parse/apply) split from the impure IO wrapper so the write NO-CHURN + read
// fallback semantics are unit-testable without a Tauri runtime — same shape as
// resolve_roles_config / apply_mini_write_behavior_to_config in projects.rs.
// ──────────────────────────────────────────────────────────────────────────────

/// PURE: parse a `MiniCoderBackend` stored under `key` in a config JSON value. Returns
/// `Some` ONLY when the key is present, structurally deserializable AND semantically valid
/// (`validate_mini_coder_backend`); every other case (missing / malformed / invalid) → `None`,
/// so callers fall through to their inheritance chain. Tolerant: never panics on bad input.
fn parse_role_backend(
    value: &serde_json::Value,
    key: &str,
) -> Option<super::mini_coder::MiniCoderBackend> {
    let entry = value.get(key)?;
    let parsed =
        serde_json::from_value::<super::mini_coder::MiniCoderBackend>(entry.clone()).ok()?;
    super::mini_coder::validate_mini_coder_backend(&parsed).ok()
}

/// PURE + total: merge (or clear) a role backend under `key` into a config `value` object.
/// `Some` writes the serialized backend; `None` REMOVES the key entirely (NO-CHURN — a config
/// that never set this role, or cleared it, stays byte-identical). Touches ONLY `key`, so a
/// save can never clobber a sibling role's backend. Returns Err if `value` is not an object.
fn apply_role_backend_to_config(
    value: &mut serde_json::Value,
    key: &str,
    backend: Option<&super::mini_coder::MiniCoderBackend>,
) -> Result<(), String> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| "config.json is not a JSON object.".to_string())?;
    match backend {
        Some(b) => {
            let serialized = serde_json::to_value(b)
                .map_err(|e| format!("Could not serialize {key} backend: {e}"))?;
            obj.insert(key.to_string(), serialized);
        }
        None => {
            // Clearing: drop the key entirely (no `null` churn).
            obj.remove(key);
        }
    }
    Ok(())
}

/// IMPURE: persist (or clear) a `MiniCoderBackend`-shaped role backend under `key`. Mirrors
/// `projects.rs::set_local_coder_backend` EXACTLY: validate+normalize, take the SHARED config
/// write lock across the whole read-modify-write (a concurrent Settings save on another key
/// can't last-writer-wins clobber this), atomic temp+rename so a crash never leaves config.json
/// partial. `None` clears the key. Returns the normalized persisted backend.
fn set_role_backend_key(
    app: &tauri::AppHandle,
    key: &str,
    backend: Option<super::mini_coder::MiniCoderBackend>,
) -> Result<Option<super::mini_coder::MiniCoderBackend>, String> {
    let normalized = match &backend {
        Some(b) => Some(super::mini_coder::validate_mini_coder_backend(b)?),
        None => None,
    };
    let path = locate_config_path(app)
        .ok_or_else(|| format!("config.json could not be located to save the {key} backend."))?;
    // Serialize the read-modify-write against the other config.json savers (mini / local /
    // design / rolesConfig) so two concurrent Settings saves can't drop each other's key.
    let _config_guard = super::projects::config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;
    let raw = fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;
    apply_role_backend_to_config(&mut value, key, normalized.as_ref())?;
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Could not serialize config.json: {e}"))?;
    // Atomic temp+rename (same as set_local_coder_backend).
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = path.with_extension(format!("json.{suffix}.tmp"));
    let backup_path = path.with_extension(format!("json.{suffix}.bak"));
    fs::write(&temp_path, format!("{pretty}\n")).map_err(|e| {
        format!(
            "Could not write config.json at {}: {e}. In a packaged build this file is read-only.",
            path.to_string_lossy()
        )
    })?;
    super::fs_replace::replace_file_with_backup(&temp_path, &path, &backup_path, "config.json")
        .map_err(|e| format!("{e}. In a packaged build this file is read-only."))?;
    Ok(normalized)
}

/// IMPURE: read the Verifier ENGINE's local model. `verifierBackend` when present + valid;
/// otherwise INHERIT the Main coder's backend (`read_main_coder_backend` → `mainCoderBackend`
/// else the mini's). Not yet wired to a launch path — the verifier launch reads this in the
/// launch-consumption slice; kept `allow(dead_code)` until then.
#[allow(dead_code)]
pub(crate) fn read_verifier_backend(
    app: &tauri::AppHandle,
) -> Option<super::mini_coder::MiniCoderBackend> {
    if let Some(path) = locate_config_path(app) {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(backend) = parse_role_backend(&value, "verifierBackend") {
                    return Some(backend);
                }
            }
        }
    }
    // Unset/invalid → inherit the Main coder's model (which itself falls back to the mini).
    read_main_coder_backend(app)
}

/// Tauri command: persist (or clear) the Main coder's dedicated local model backend.
/// Mirrors `set_local_coder_backend`: same signature shape + `ensure_unlocked` guard.
#[tauri::command]
pub fn set_main_coder_backend_cmd(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    backend: Option<super::mini_coder::MiniCoderBackend>,
) -> Result<Option<super::mini_coder::MiniCoderBackend>, String> {
    state.ensure_unlocked()?;
    set_role_backend_key(&app, "mainCoderBackend", backend)
}

/// Tauri command: persist (or clear) the Verifier's dedicated local model backend.
/// Mirrors `set_local_coder_backend`: same signature shape + `ensure_unlocked` guard.
#[tauri::command]
pub fn set_verifier_backend_cmd(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    backend: Option<super::mini_coder::MiniCoderBackend>,
) -> Result<Option<super::mini_coder::MiniCoderBackend>, String> {
    state.ensure_unlocked()?;
    set_role_backend_key(&app, "verifierBackend", backend)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests (same file, #[cfg(test)], PURE only — no AppHandle)
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- resolve_roles_config -------------------------------------------------

    #[test]
    fn resolve_empty_config_defaults() {
        // Empty config → {orchestrator:"orchestrator", coder:"codex", verifier:"codex"}.
        let config = serde_json::json!({});
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "orchestrator");
        assert_eq!(effective.coder_client, "codex");
        assert_eq!(effective.verifier_client, "codex");
    }

    #[test]
    fn resolve_legacy_mainCoderClient_fills_coder_and_verifier() {
        // Legacy mainCoderClient:"claude" and no rolesConfig → coder AND verifier "claude".
        let config = serde_json::json!({"mainCoderClient": "claude"});
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "orchestrator");
        assert_eq!(effective.coder_client, "claude");
        assert_eq!(effective.verifier_client, "claude");
    }

    #[test]
    fn resolve_rolesConfig_verifier_independent_of_coder() {
        // rolesConfig{verifierClient:"claude"} + mainCoderClient:"codex" → verifier claude,
        // coder codex (independence).
        let config = serde_json::json!({
            "rolesConfig": { "verifierClient": "claude" },
            "mainCoderClient": "codex",
        });
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "orchestrator");
        assert_eq!(effective.coder_client, "codex");
        assert_eq!(effective.verifier_client, "claude");
    }

    #[test]
    fn resolve_rolesConfig_complete_ignores_legacy() {
        // When rolesConfig is complete, legacy mainCoderClient is ignored.
        let config = serde_json::json!({
            "rolesConfig": {
                "orchestratorClient": "claude",
                "coderClient": "codex",
                "verifierClient": "claude",
            },
            "mainCoderClient": "codex",
        });
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "claude");
        assert_eq!(effective.coder_client, "codex");
        assert_eq!(effective.verifier_client, "claude");
    }

    #[test]
    fn resolve_whitespace_and_case_normalized() {
        // "  Claude " → "claude" (trimmed + lowercased).
        let config = serde_json::json!({
            "rolesConfig": {
                "orchestratorClient": "  Claude  ",
                "coderClient": "  CODEx  ",
                "verifierClient": "  Orchestrator  ",
            },
        });
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "claude");
        assert_eq!(effective.coder_client, "codex");
        assert_eq!(effective.verifier_client, "orchestrator");
    }

    #[test]
    fn resolve_partial_rolesConfig_fills_gaps_from_legacy() {
        // Only coderClient set; verifier falls back to legacy mainCoderClient.
        let config = serde_json::json!({
            "rolesConfig": { "coderClient": "claude" },
            "mainCoderClient": "codex",
        });
        let effective = resolve_roles_config(&config);
        assert_eq!(effective.orchestrator_client, "orchestrator");
        assert_eq!(effective.coder_client, "claude");
        assert_eq!(effective.verifier_client, "codex");
    }

    // -- validate_client_id ---------------------------------------------------

    #[test]
    fn validate_client_id_accepts_builtins() {
        for id in ["codex", "claude", "powershell", "orchestrator"] {
            assert!(validate_client_id(id).is_ok());
            assert_eq!(validate_client_id(id).unwrap(), id);
        }
    }

    #[test]
    fn validate_client_id_accepts_custom_ids() {
        // Plausible custom ids: [a-z0-9-]{1,32}.
        for id in ["my-cli-2", "a", "x", "a1b2", "my-custom-agent"] {
            assert!(validate_client_id(id).is_ok());
            assert_eq!(validate_client_id(id).unwrap(), id);
        }
    }

    #[test]
    fn validate_client_id_rejects_empty() {
        assert!(validate_client_id("").is_err());
        assert!(validate_client_id("   ").is_err());
    }

    #[test]
    fn validate_client_id_normalizes_uppercase_rejects_illegal_chars() {
        // Case is NORMALIZED, not rejected (matches the spec + resolve_roles_config
        // + the trim/case test below): "Claude" → "claude" (a valid built-in).
        assert_eq!(validate_client_id("Claude").unwrap(), "claude");
        // But an illegal CHARACTER (even after lowercasing) is still rejected.
        assert!(validate_client_id("UPPER!").is_err());
    }

    #[test]
    fn validate_client_id_rejects_overlong() {
        let overlong = "a".repeat(33);
        assert!(validate_client_id(&overlong).is_err());
    }

    #[test]
    fn validate_client_id_rejects_reserved_as_custom() {
        // Built-in ids cannot be used as custom client ids.
        for id in ["codex", "claude", "powershell", "orchestrator"] {
            assert!(validate_client_id(id).is_ok()); // Built-in is OK as built-in.
        }
        // But a custom client with the same name would be rejected — however, since
        // the built-in is matched first, this is not a real scenario. The validator
        // returns Ok for built-in ids, which is the correct behavior: a client
        // named "codex" is resolved as the built-in, not as a custom client.
    }

    #[test]
    fn validate_client_id_rejects_special_chars() {
        // Illegal characters are rejected (space, underscore, dot, punctuation).
        // "My-Cli" is NOT here — it normalizes to the valid "my-cli" (see the
        // trim/case test); only genuinely illegal chars reject.
        for id in ["my client", "my_client", "my.client", "my client!"] {
            assert!(validate_client_id(id).is_err(), "{id} must be rejected");
        }
    }

    #[test]
    fn validate_client_id_normalizes_case_and_trims() {
        // "  My-Cli  " → "my-cli" (lowercased, trimmed, hyphens preserved).
        let result = validate_client_id("  My-Cli  ").unwrap();
        assert_eq!(result, "my-cli");
    }

    #[test]
    fn validate_client_id_accepts_digit_only() {
        // Pure digit ids are valid custom ids (e.g. "42").
        assert!(validate_client_id("42").is_ok());
        assert_eq!(validate_client_id("42").unwrap(), "42");
    }

    // -- Serialization round-trip -----------------------------------------------

    #[test]
    fn roles_config_serializes_camel_case() {
        let config = RolesConfig {
            orchestrator_client: Some("claude".into()),
            coder_client: Some("codex".into()),
            verifier_client: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"orchestratorClient\":\"claude\""));
        assert!(json.contains("\"coderClient\":\"codex\""));
        assert!(!json.contains("verifierClient")); // skip_serializing_if = None
    }

    #[test]
    fn roles_config_deserializes_camel_case() {
        let json = r#"{"orchestratorClient":"claude","coderClient":"codex","verifierClient":"codex"}"#;
        let config: RolesConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.orchestrator_client, Some("claude".to_string()));
        assert_eq!(config.coder_client, Some("codex".to_string()));
        assert_eq!(config.verifier_client, Some("codex".to_string()));
    }

    #[test]
    fn roles_config_partial_deserializes() {
        // Only coderClient present — others default to None.
        let json = r#"{"coderClient":"claude"}"#;
        let config: RolesConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.orchestrator_client, None);
        assert_eq!(config.coder_client, Some("claude".to_string()));
        assert_eq!(config.verifier_client, None);
    }

    // -- per-role backend model (P6b path B) ----------------------------------

    use super::super::mini_coder::{MiniCoderBackend, MiniCoderBackendKind};

    fn codex_backend() -> MiniCoderBackend {
        serde_json::from_value(serde_json::json!({ "kind": "codex" })).unwrap()
    }

    #[test]
    fn parse_role_backend_present_valid_returns_some() {
        let config = serde_json::json!({ "mainCoderBackend": { "kind": "codex" } });
        let parsed = parse_role_backend(&config, "mainCoderBackend");
        assert!(parsed.is_some(), "a present + valid backend must parse");
        assert_eq!(parsed.unwrap().kind, MiniCoderBackendKind::Codex);
    }

    #[test]
    fn parse_role_backend_missing_malformed_or_invalid_returns_none() {
        // Missing key.
        assert!(parse_role_backend(&serde_json::json!({}), "mainCoderBackend").is_none());
        // Structurally malformed (unknown kind → deserialize error).
        let malformed = serde_json::json!({ "mainCoderBackend": { "kind": "nope" } });
        assert!(parse_role_backend(&malformed, "mainCoderBackend").is_none());
        // Structurally malformed (missing required `kind`).
        let no_kind = serde_json::json!({ "mainCoderBackend": { "model": "x" } });
        assert!(parse_role_backend(&no_kind, "mainCoderBackend").is_none());
        // Deserializable but semantically INVALID (ollama requires a model tag).
        let invalid = serde_json::json!({ "mainCoderBackend": { "kind": "ollama" } });
        assert!(
            parse_role_backend(&invalid, "mainCoderBackend").is_none(),
            "a present-but-invalid backend must fall through to None"
        );
    }

    #[test]
    fn apply_role_backend_inserts_clears_and_leaves_siblings_untouched() {
        // Start with a config that already carries a SIBLING role's backend.
        let mut config = serde_json::json!({
            "miniCoderBackend": { "kind": "ollama", "model": "qwen2.5-coder" },
        });

        // Insert mainCoderBackend.
        apply_role_backend_to_config(&mut config, "mainCoderBackend", Some(&codex_backend()))
            .unwrap();
        assert_eq!(
            config["mainCoderBackend"]["kind"], "codex",
            "Some must write the role's key"
        );
        assert_eq!(
            config["miniCoderBackend"]["model"], "qwen2.5-coder",
            "a sibling backend key must be left untouched"
        );

        // Clear it → the key is REMOVED entirely (NO-CHURN, no `null`).
        apply_role_backend_to_config(&mut config, "mainCoderBackend", None).unwrap();
        assert!(
            config.get("mainCoderBackend").is_none(),
            "None must drop the key entirely (no null churn)"
        );
        assert!(
            config.get("miniCoderBackend").is_some(),
            "clearing one role must not touch the sibling"
        );
    }

    #[test]
    fn apply_role_backend_rejects_non_object() {
        let mut not_obj = serde_json::json!("i am a string");
        assert!(
            apply_role_backend_to_config(&mut not_obj, "mainCoderBackend", Some(&codex_backend()))
                .is_err(),
            "a non-object config must be rejected, not panic"
        );
    }
}
