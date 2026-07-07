use super::model::{
    DeviceInviteInput, DeviceInviteRecord, DeviceVaultStatus, DevicesInvitesSnapshot,
};
use super::state::BackendState;
use super::vault;
use chrono::Utc;
use ed25519_dalek::SigningKey;
use getrandom::fill as random_fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tauri::{Manager, State};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalDeviceMetadata {
    version: u32,
    device_id: String,
    device_name: String,
    platform: String,
    public_key: String,
    public_key_fingerprint: String,
    // Ed25519 signing identity (additive). Older records predate these fields, so
    // they default to empty and are backfilled on the next ensure_local_device.
    #[serde(default)]
    signing_public_key: String,
    #[serde(default)]
    signing_fingerprint: String,
    created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInvitesStore {
    version: u32,
    invites: Vec<DeviceInviteRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceJoinRequest {
    device_id: Option<String>,
    device_name: Option<String>,
    platform: Option<String>,
    public_key: String,
    #[serde(default)]
    signing_public_key: Option<String>,
}

#[tauri::command]
pub fn get_devices_invites_snapshot(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<DevicesInvitesSnapshot, String> {
    state.ensure_unlocked()?;
    devices_snapshot(&app)
}

#[tauri::command]
pub fn ensure_local_device_identity(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<DevicesInvitesSnapshot, String> {
    state.ensure_unlocked()?;
    ensure_local_device(&app)?;
    devices_snapshot(&app)
}

#[tauri::command]
pub fn reset_local_device_identity(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<DevicesInvitesSnapshot, String> {
    state.ensure_unlocked()?;
    let path = local_device_path(&app)?;
    let _ = fs::remove_file(path);
    vault::delete_device_private_key()?;
    vault::delete_device_signing_private_key()?;
    ensure_local_device(&app)?;
    devices_snapshot(&app)
}

#[tauri::command]
pub fn approve_device_invite(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    input: DeviceInviteInput,
) -> Result<DevicesInvitesSnapshot, String> {
    state.ensure_unlocked()?;
    super::roles::require_capability(&app, super::roles::Capability::ManageDevices)?;
    let mut store = read_invites_store(&app)?;
    let record = invite_record_from_input(input)?;
    store.invites.retain(|invite| {
        !invite
            .public_key_fingerprint
            .eq_ignore_ascii_case(&record.public_key_fingerprint)
    });
    store.invites.push(record);
    write_invites_store(&app, &store)?;
    devices_snapshot(&app)
}

#[tauri::command]
pub fn revoke_device_invite(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    invite_id: String,
) -> Result<DevicesInvitesSnapshot, String> {
    state.ensure_unlocked()?;
    super::roles::require_capability(&app, super::roles::Capability::ManageDevices)?;
    let id = invite_id.trim();
    if id.is_empty() {
        return Err("Invite id is required.".into());
    }
    let mut store = read_invites_store(&app)?;
    let now = now();
    let mut found = false;
    for invite in &mut store.invites {
        if invite.id == id {
            invite.status = "revoked".into();
            invite.revoked_at = Some(now.clone());
            found = true;
            break;
        }
    }
    if !found {
        return Err("Invite not found.".into());
    }
    write_invites_store(&app, &store)?;
    devices_snapshot(&app)
}

fn devices_snapshot(app: &tauri::AppHandle) -> Result<DevicesInvitesSnapshot, String> {
    let local_device = read_local_device_status(app)?;
    let mut invites = read_invites_store(app)?.invites;
    invites.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(DevicesInvitesSnapshot {
        local_device,
        invites,
    })
}

pub(crate) fn read_local_device_status(
    app: &tauri::AppHandle,
) -> Result<DeviceVaultStatus, String> {
    let last_checked_at = now();
    let private_key_configured = vault::read_device_private_key_hex()?.is_some();
    let signing_key_configured = vault::read_device_signing_private_key_hex()?.is_some();
    match read_local_device_metadata(app)? {
        Some(metadata) => {
            // Backward compatibility: a record written before the signing key
            // existed has empty signing fields. Surface them as absent rather
            // than as empty strings.
            let signing_public_key = (!metadata.signing_public_key.is_empty())
                .then(|| metadata.signing_public_key.clone());
            let signing_fingerprint = (!metadata.signing_fingerprint.is_empty())
                .then(|| metadata.signing_fingerprint.clone());
            Ok(DeviceVaultStatus {
                configured: private_key_configured,
                device_id: Some(metadata.device_id.clone()),
                device_name: Some(metadata.device_name.clone()),
                platform: metadata.platform.clone(),
                vault_backend: vault_backend_label().into(),
                biometric_label: biometric_label().into(),
                public_key: Some(metadata.public_key.clone()),
                public_key_fingerprint: Some(metadata.public_key_fingerprint.clone()),
                private_key_configured,
                signing_public_key: signing_public_key.clone(),
                signing_fingerprint,
                signing_key_configured: signing_key_configured && signing_public_key.is_some(),
                created_at: Some(metadata.created_at.clone()),
                last_checked_at,
                security_level: security_level().into(),
                join_request: Some(join_request_json(&metadata)?),
                message: Some(if private_key_configured {
                    "This device can receive encrypted workspace package keys.".into()
                } else {
                    "Public metadata exists, but the private key is missing from the OS vault."
                        .into()
                }),
                // The verified role is resolved separately (it needs the trust
                // anchor + grant store); this low-level status leaves it unset.
                role: None,
            })
        }
        None => Ok(DeviceVaultStatus {
            configured: false,
            platform: platform_label().into(),
            vault_backend: vault_backend_label().into(),
            biometric_label: biometric_label().into(),
            private_key_configured,
            signing_key_configured,
            last_checked_at,
            security_level: security_level().into(),
            message: Some(
                "Create this device identity before requesting a workspace invite.".into(),
            ),
            ..DeviceVaultStatus::default()
        }),
    }
}

fn ensure_local_device(app: &tauri::AppHandle) -> Result<LocalDeviceMetadata, String> {
    if let Some(mut metadata) = read_local_device_metadata(app)? {
        if vault::read_device_private_key_hex()?.is_some() {
            // The X25519 identity is intact. Additively backfill the Ed25519
            // signing key for devices created before signing existed, without
            // touching the existing key-exchange identity.
            if metadata.signing_public_key.is_empty()
                || vault::read_device_signing_private_key_hex()?.is_none()
            {
                let (signing_public_key, signing_fingerprint) = ensure_device_signing_key()?;
                metadata.signing_public_key = signing_public_key;
                metadata.signing_fingerprint = signing_fingerprint;
                write_local_device_metadata(app, &metadata)?;
            }
            return Ok(metadata);
        }
    }
    // A5: keep the raw private scalar and its hex encoding in zeroizing memory so
    // they are wiped on drop. What is stored in the OS vault is unchanged.
    let mut private_key = Zeroizing::new([0u8; 32]);
    random_fill(private_key.as_mut_slice())
        .map_err(|e| format!("Device key generation failed: {e}"))?;
    let secret = StaticSecret::from(*private_key);
    let public = PublicKey::from(&secret);
    let public_key = hex::encode(public.as_bytes());
    let fingerprint = public_key_fingerprint(&public_key)?;
    let (signing_public_key, signing_fingerprint) = ensure_device_signing_key()?;
    let metadata = LocalDeviceMetadata {
        version: STORE_VERSION,
        device_id: format!("dev_{}", fingerprint.to_ascii_lowercase().replace('-', "")),
        device_name: default_device_name(),
        platform: platform_label().into(),
        public_key,
        public_key_fingerprint: fingerprint,
        signing_public_key,
        signing_fingerprint,
        created_at: now(),
    };
    let private_key_hex = Zeroizing::new(hex::encode(*private_key));
    vault::save_device_private_key_hex(&private_key_hex)?;
    write_local_device_metadata(app, &metadata)?;
    Ok(metadata)
}

/// Generates (or reuses) the device Ed25519 signing keypair. The 32-byte seed is
/// stored hex-encoded in its own isolated OS vault account; only the public key
/// hex and signing fingerprint are returned for the device metadata / join
/// request. Returns `(signing_public_key_hex, signing_fingerprint)`.
fn ensure_device_signing_key() -> Result<(String, String), String> {
    if let Some(seed_hex) = vault::read_device_signing_private_key_hex()? {
        // A5: zeroize the decoded seed and the SigningKey wrapper.
        let seed = Zeroizing::new(
            hex::decode(seed_hex.trim())
                .map_err(|_| "Device signing key in vault is not valid hex.".to_string())?,
        );
        let seed_bytes: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| "Device signing key in vault has the wrong length.".to_string())?;
        let signing_key = SigningKey::from_bytes(&seed_bytes);
        let public_key = hex::encode(signing_key.verifying_key().to_bytes());
        let fingerprint = signing_key_fingerprint(&public_key)?;
        return Ok((public_key, fingerprint));
    }
    // A5: keep the raw Ed25519 seed and its hex encoding in zeroizing memory.
    let mut seed = Zeroizing::new([0u8; 32]);
    random_fill(seed.as_mut_slice())
        .map_err(|e| format!("Device signing key generation failed: {e}"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = hex::encode(signing_key.verifying_key().to_bytes());
    let fingerprint = signing_key_fingerprint(&public_key)?;
    let seed_hex = Zeroizing::new(hex::encode(*seed));
    vault::save_device_signing_private_key_hex(&seed_hex)?;
    Ok((public_key, fingerprint))
}

/// Normalizes an Ed25519 public key (hex, 32 bytes). Distinct from the X25519
/// `normalize_public_key` so the two key types are never confused.
pub(crate) fn normalize_signing_public_key(value: &str) -> Result<String, String> {
    let clean = value
        .trim()
        .trim_start_matches("ed25519:")
        .replace([' ', '-'], "");
    if clean.len() != 64 || !clean.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(
            "Device signing key must be a 32-byte Ed25519 public key encoded as hex.".into(),
        );
    }
    Ok(clean.to_ascii_lowercase())
}

/// Fingerprint of an Ed25519 signing public key. Uses a domain-separated SHA-256
/// (prefixed "ed25519-signing:") so it never collides with the X25519
/// key-exchange fingerprint of the same device.
pub(crate) fn signing_key_fingerprint(public_key: &str) -> Result<String, String> {
    let bytes = hex::decode(normalize_signing_public_key(public_key)?)
        .map_err(|_| "Device signing key is not valid hex.".to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(b"ed25519-signing:");
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let hex = hex::encode(digest);
    Ok(hex[..16]
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("-"))
}

fn invite_record_from_input(input: DeviceInviteInput) -> Result<DeviceInviteRecord, String> {
    let collaborator_name = clean_required(&input.collaborator_name, "Collaborator name")?;
    let raw = input.join_request.trim();
    if raw.is_empty() {
        return Err("Join request is required.".into());
    }
    let request = parse_join_request(raw)?;
    let public_key = normalize_public_key(&request.public_key)?;
    let fingerprint = public_key_fingerprint(&public_key)?;
    // Additive: capture the Ed25519 signing key if the request carried one. A
    // malformed signing key is rejected rather than silently dropped, but its
    // absence is fine (older requests).
    let signing_public_key = match request
        .signing_public_key
        .as_deref()
        .and_then(clean_optional)
    {
        Some(raw_key) => Some(normalize_signing_public_key(&raw_key)?),
        None => None,
    };
    let signing_fingerprint = match signing_public_key.as_deref() {
        Some(key) => Some(signing_key_fingerprint(key)?),
        None => None,
    };
    let _request_device_id = request.device_id.as_deref().and_then(clean_optional);
    let now = now();
    Ok(DeviceInviteRecord {
        id: format!(
            "invite_{}",
            fingerprint.to_ascii_lowercase().replace('-', "")
        ),
        collaborator_name,
        device_name: request
            .device_name
            .and_then(|value| clean_optional(&value))
            .unwrap_or_else(|| "Unknown device".into()),
        platform: request
            .platform
            .and_then(|value| clean_optional(&value))
            .unwrap_or_else(|| "unknown".into()),
        public_key,
        public_key_fingerprint: fingerprint,
        signing_public_key,
        signing_fingerprint,
        status: "approved".into(),
        created_at: now.clone(),
        approved_at: Some(now),
        revoked_at: None,
        notes: input.notes.as_deref().and_then(clean_optional),
        // Role the admin picked at approval; absent input defaults to the
        // least-privileged role.
        role: Some(input.role.unwrap_or_default()),
    })
}

fn parse_join_request(raw: &str) -> Result<DeviceJoinRequest, String> {
    if raw.starts_with('{') {
        serde_json::from_str(raw).map_err(|e| format!("Join request JSON is invalid: {e}"))
    } else {
        Ok(DeviceJoinRequest {
            device_id: None,
            device_name: None,
            platform: None,
            public_key: raw.into(),
            signing_public_key: None,
        })
    }
}

pub(crate) fn normalize_public_key(value: &str) -> Result<String, String> {
    let clean = value
        .trim()
        .trim_start_matches("x25519:")
        .replace([' ', '-'], "");
    if clean.len() != 64 || !clean.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("Device public key must be a 32-byte X25519 public key encoded as hex.".into());
    }
    Ok(clean.to_ascii_lowercase())
}

pub(crate) fn public_key_fingerprint(public_key: &str) -> Result<String, String> {
    let bytes = hex::decode(normalize_public_key(public_key)?)
        .map_err(|_| "Device public key is not valid hex.".to_string())?;
    let digest = Sha256::digest(bytes);
    let hex = hex::encode(digest);
    Ok(hex[..16]
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("-"))
}

fn join_request_json(metadata: &LocalDeviceMetadata) -> Result<String, String> {
    serde_json::to_string_pretty(&serde_json::json!({
        "kind": "aspis-device-join-request",
        "version": STORE_VERSION,
        "deviceId": metadata.device_id,
        "deviceName": metadata.device_name,
        "platform": metadata.platform,
        "publicKeyType": "x25519",
        "publicKey": metadata.public_key,
        "fingerprint": metadata.public_key_fingerprint,
        // Additive: Ed25519 signing identity so an approving device can pin the
        // collaborator's package-signing key. Older requests omit these.
        "signingPublicKeyType": "ed25519",
        "signingPublicKey": metadata.signing_public_key,
        "signingFingerprint": metadata.signing_fingerprint,
        "createdAt": metadata.created_at,
    }))
    .map_err(|e| format!("Join request could not be serialized: {e}"))
}

fn read_local_device_metadata(
    app: &tauri::AppHandle,
) -> Result<Option<LocalDeviceMetadata>, String> {
    let path = local_device_path(app)?;
    if !path.is_file() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("Device metadata could not be read: {e}"))?;
    serde_json::from_str(&raw).map(Some).map_err(|e| {
        format!(
            "Device metadata is invalid at {}: {e}",
            path.to_string_lossy()
        )
    })
}

fn write_local_device_metadata(
    app: &tauri::AppHandle,
    metadata: &LocalDeviceMetadata,
) -> Result<(), String> {
    let path = local_device_path(app)?;
    let raw = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("Device metadata could not be serialized: {e}"))?;
    fs::write(path, raw).map_err(|e| format!("Device metadata could not be saved: {e}"))
}

/// A device whose Ed25519 signing public key is trusted for signer-identity
/// pinning on decrypt: either the local device or an approved invite.
#[derive(Debug, Clone)]
pub(crate) struct KnownSigner {
    pub signing_public_key: String,
    pub name: String,
}

/// Returns the local device Ed25519 signing keypair for signing a package,
/// ensuring it exists first. The 32-byte seed stays in zeroizing memory; the
/// returned `SigningKey` itself zeroizes its secret on drop. Returns
/// `(SigningKey, signing_public_key_hex, signing_fingerprint)`.
pub(crate) fn load_local_signing_key(
    app: &tauri::AppHandle,
) -> Result<(SigningKey, String, String), String> {
    // Make sure the local identity (and therefore the signing key) exists.
    ensure_local_device(app)?;
    let seed_hex = vault::read_device_signing_private_key_hex()?
        .ok_or_else(|| "This device signing key is missing from the OS vault.".to_string())?;
    let seed = Zeroizing::new(
        hex::decode(seed_hex.trim())
            .map_err(|_| "Device signing key in vault is not valid hex.".to_string())?,
    );
    let seed_bytes: [u8; 32] = seed
        .as_slice()
        .try_into()
        .map_err(|_| "Device signing key in vault has the wrong length.".to_string())?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    let public_key = hex::encode(signing_key.verifying_key().to_bytes());
    let fingerprint = signing_key_fingerprint(&public_key)?;
    Ok((signing_key, public_key, fingerprint))
}

/// All Ed25519 signing keys this installation recognizes: the local device plus
/// every approved invite that carried a signing key. Used to mark a decrypted
/// package's signer as known/unknown.
pub(crate) fn known_signers(app: &tauri::AppHandle) -> Result<Vec<KnownSigner>, String> {
    let mut signers = Vec::new();
    if let Ok(local) = read_local_device_status(app) {
        if let Some(key) = local.signing_public_key {
            signers.push(KnownSigner {
                signing_public_key: key.to_ascii_lowercase(),
                name: local.device_name.unwrap_or_else(|| "This device".into()),
            });
        }
    }
    for invite in approved_device_invites(app)? {
        if let Some(key) = invite.signing_public_key {
            signers.push(KnownSigner {
                signing_public_key: key.to_ascii_lowercase(),
                name: format!("{} ({})", invite.collaborator_name, invite.device_name),
            });
        }
    }
    Ok(signers)
}

fn read_invites_store(app: &tauri::AppHandle) -> Result<DeviceInvitesStore, String> {
    let path = invites_path(app)?;
    if !path.is_file() {
        return Ok(DeviceInvitesStore {
            version: STORE_VERSION,
            invites: Vec::new(),
        });
    }
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("Invite store could not be read: {e}"))?;
    let mut store: DeviceInvitesStore = serde_json::from_str(&raw)
        .map_err(|e| format!("Invite store is invalid at {}: {e}", path.to_string_lossy()))?;
    store.version = STORE_VERSION;
    Ok(store)
}

pub(crate) fn approved_device_invites(
    app: &tauri::AppHandle,
) -> Result<Vec<DeviceInviteRecord>, String> {
    Ok(read_invites_store(app)?
        .invites
        .into_iter()
        .filter(|invite| invite.status == "approved")
        .collect())
}

fn write_invites_store(app: &tauri::AppHandle, store: &DeviceInvitesStore) -> Result<(), String> {
    let path = invites_path(app)?;
    let raw = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Invite store could not be serialized: {e}"))?;
    fs::write(path, raw).map_err(|e| format!("Invite store could not be saved: {e}"))
}

fn devices_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("App data dir could not be resolved: {e}"))?
        .join("devices");
    fs::create_dir_all(&dir).map_err(|e| format!("Devices dir could not be created: {e}"))?;
    Ok(dir)
}

fn local_device_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(devices_dir(app)?.join("local-device.json"))
}

fn invites_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(devices_dir(app)?.join("invites.json"))
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .and_then(|value| clean_optional(&value))
        .unwrap_or_else(|| "Devboule device".into())
}

fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

fn vault_backend_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else if cfg!(target_os = "linux") {
        "System keyring"
    } else {
        "OS credential vault"
    }
}

fn biometric_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Touch ID or macOS password"
    } else if cfg!(target_os = "windows") {
        "Windows Hello"
    } else if cfg!(target_os = "linux") {
        "App password or system keyring"
    } else {
        "OS unlock"
    }
}

fn security_level() -> &'static str {
    if cfg!(target_os = "linux") {
        "dev"
    } else {
        "strong"
    }
}

fn clean_required(value: &str, label: &str) -> Result<String, String> {
    clean_optional(value).ok_or_else(|| format!("{label} is required."))
}

fn clean_optional(value: &str) -> Option<String> {
    let clean = value.trim();
    if clean.is_empty() {
        return None;
    }
    Some(clean.replace(['\r', '\n'], " ").chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_key_fingerprint_is_stable_and_grouped() {
        let key = "01".repeat(32);
        let fingerprint = public_key_fingerprint(&key).unwrap();
        assert_eq!(fingerprint.len(), 19);
        assert_eq!(fingerprint.matches('-').count(), 3);
    }

    #[test]
    fn invite_parser_accepts_raw_public_key() {
        let input = DeviceInviteInput {
            collaborator_name: "Ada".into(),
            join_request: "02".repeat(32),
            notes: Some("MacBook".into()),
            role: None,
        };
        let record = invite_record_from_input(input).unwrap();
        assert_eq!(record.collaborator_name, "Ada");
        assert_eq!(record.platform, "unknown");
        assert_eq!(record.status, "approved");
    }

    #[test]
    fn invite_parser_accepts_join_request_json() {
        let raw = serde_json::json!({
            "deviceId": "dev_test",
            "deviceName": "Ada MacBook",
            "platform": "macos",
            "publicKey": "03".repeat(32),
        })
        .to_string();
        let input = DeviceInviteInput {
            collaborator_name: "Ada".into(),
            join_request: raw,
            notes: None,
            role: None,
        };
        let record = invite_record_from_input(input).unwrap();
        assert_eq!(record.device_name, "Ada MacBook");
        assert_eq!(record.platform, "macos");
    }

    #[test]
    fn signing_fingerprint_is_stable_grouped_and_distinct_from_x25519() {
        // 32-byte all-0x01 Ed25519 public key, valid hex.
        let key = "01".repeat(32);
        let fingerprint = signing_key_fingerprint(&key).unwrap();
        // Same shape as the X25519 fingerprint (4 groups of 4, dash-joined).
        assert_eq!(fingerprint.len(), 19);
        assert_eq!(fingerprint.matches('-').count(), 3);
        // Domain separation: signing fingerprint must differ from the X25519
        // fingerprint of the same raw key bytes.
        assert_ne!(fingerprint, public_key_fingerprint(&key).unwrap());
    }

    #[test]
    fn signing_public_key_normalization_accepts_prefix_and_rejects_junk() {
        let key = "0a".repeat(32);
        assert_eq!(
            normalize_signing_public_key(&format!("ed25519:{key}")).unwrap(),
            key
        );
        assert!(normalize_signing_public_key("not-hex").is_err());
        assert!(normalize_signing_public_key(&"ab".repeat(31)).is_err());
    }

    #[test]
    fn generated_ed25519_key_has_expected_structure() {
        // Mirrors ensure_device_signing_key's keygen without touching the vault:
        // a fresh seed yields a 32-byte verifying key whose hex round-trips and
        // whose fingerprint has the canonical grouped shape.
        let mut seed = [0u8; 32];
        random_fill(&mut seed).expect("seed");
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = hex::encode(signing_key.verifying_key().to_bytes());
        assert_eq!(public_key.len(), 64);
        let normalized = normalize_signing_public_key(&public_key).expect("normalize");
        assert_eq!(normalized, public_key);
        let fingerprint = signing_key_fingerprint(&public_key).expect("fingerprint");
        assert_eq!(fingerprint.len(), 19);
    }

    #[test]
    fn join_request_carries_signing_public_key() {
        // A join request JSON with a signing key must round-trip the Ed25519 key
        // and derive its signing fingerprint onto the approved invite.
        let signing_key = "0b".repeat(32);
        let raw = serde_json::json!({
            "deviceId": "dev_test",
            "deviceName": "Bo Laptop",
            "platform": "windows",
            "publicKey": "03".repeat(32),
            "signingPublicKey": signing_key,
        })
        .to_string();
        let input = DeviceInviteInput {
            collaborator_name: "Bo".into(),
            join_request: raw,
            notes: None,
            role: None,
        };
        let record = invite_record_from_input(input).unwrap();
        assert_eq!(
            record.signing_public_key.as_deref(),
            Some(signing_key.as_str())
        );
        assert_eq!(
            record.signing_fingerprint,
            Some(signing_key_fingerprint(&signing_key).unwrap())
        );
    }

    #[test]
    fn invite_without_signing_key_is_backward_compatible() {
        // Older join requests (no signing key) must still parse, with the signing
        // fields left absent.
        let input = DeviceInviteInput {
            collaborator_name: "Ada".into(),
            join_request: "02".repeat(32),
            notes: None,
            role: None,
        };
        let record = invite_record_from_input(input).unwrap();
        assert!(record.signing_public_key.is_none());
        assert!(record.signing_fingerprint.is_none());
    }
}
