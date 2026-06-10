//! Role grants — the admin-signed credential that tells a collaborator's app
//! which role it runs as.
//!
//! The grant mirrors the package-signing crypto (Ed25519 over a SHA-256 digest,
//! `verify_strict`) but is an INDEPENDENT artifact from the encrypted workspace
//! package, so a role change never forces re-packaging.
//!
//! ## ANTI-LOCKOUT INVARIANT (critical — read before changing `resolve_local_role`)
//!
//! `resolve_local_role` can NEVER deny access to the app. It always returns a
//! concrete `Role`:
//!   - trust anchor EMPTY  -> `Admin` (bootstrap / origin build): the admin is
//!     never locked out and can export the anchor. A correctly distributed
//!     collaborator build always ships a POPULATED anchor, so this branch can
//!     never make a collaborator an admin.
//!   - this device's signing key == anchor -> `Admin` (canonical recognition).
//!   - otherwise -> the role from a VERIFIED grant, else the least-privileged
//!     role (`Collaborator`). A missing/invalid grant narrows the UI; it is NEVER
//!     a hard lockout.
//!
//! The unlock flow (Windows Hello / Touch ID) is entirely independent of roles —
//! roles sit ON TOP of unlock and must never gate it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Manager;

use super::devices;
use super::model::{Role, RoleGrant, SignedRoleGrant};

/// Signature scheme tag for role grants. Distinct from the package scheme so a
/// package signature can never be replayed as a role grant or vice-versa.
const ROLE_GRANT_SCHEME: &str = "ed25519-role-grant-v1";
/// Domain-separation prefix mixed into the grant digest.
const ROLE_GRANT_DOMAIN: &[u8] = b"aspis-role-grant-v1:";

/// What the local role resolution decided, surfaced to the UI and onboarding.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRoleStatus {
    /// The role used for enforcement. Never absent — defaults to the least
    /// privileged role so enforcement always fails safe, never to a lockout.
    pub role: Role,
    /// True when this install is the admin OR holds a valid grant. False means a
    /// fresh collaborator that still needs onboarding.
    pub provisioned: bool,
    pub is_admin: bool,
    /// Whether the bundled trust anchor is set. When false this build must not be
    /// distributed (every collaborator would resolve to admin).
    pub anchor_configured: bool,
}

/// Privileged capabilities gated to specific roles. These are the few actions
/// with NO provider-side enforcement behind them, so app-level gating is the only
/// control: managing the collaborator roster, issuing grants, and creating the
/// master workspace package. Cloud actions are deliberately NOT here — a
/// collaborator's agents operate Cloudflare/Scaleway, bounded by their SCOPED
/// token (which the provider enforces), not by hidden UI or app gates. When a
/// future role needs PARTIAL cloud access, add a `WriteSecrets`/`DestructiveCloud`
/// variant here and gate the relevant commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    ManageDevices,
    CreateBootstrap,
    IssueRoleGrant,
}

/// The single source of truth for "what can this role do". Extending the role set
/// is one new arm here plus the nav list mirrored in the frontend.
pub fn role_has_capability(role: Role, _cap: Capability) -> bool {
    match role {
        // The admin can do everything.
        Role::Admin => true,
        // Collaborators hold no privileged capability. Their power comes only from
        // the (scoped) cloud token physically delivered to them — never from app
        // permissions, which a patched client could bypass anyway.
        Role::Collaborator => false,
    }
}

/// Enforce that the local role holds `cap`, after `ensure_unlocked`.
///
/// DEFENSE-IN-DEPTH ONLY — NOT a security boundary. A collaborator controls their
/// machine and could patch the client to skip this check, so it cannot stop a
/// determined malicious collaborator. The real boundaries are (1) the crypto
/// (they can't decrypt packages not wrapped for their fingerprint, nor forge an
/// admin grant) and (2) per-role scoped cloud tokens the provider enforces. This
/// check keeps the honest distributed binary correct and blocks accidental
/// privileged use. It resolves the role fresh (never caches), and resolution can
/// never lock the user out — see `resolve_local_role`.
pub fn require_capability(app: &tauri::AppHandle, cap: Capability) -> Result<(), String> {
    if role_has_capability(resolve_local_role(app), cap) {
        Ok(())
    } else {
        Err("Your role is not permitted to perform this action.".into())
    }
}

// --- trust anchor -----------------------------------------------------------

/// Resolve `config.json` the same way `lib.rs::resolve_config_path` does, so the
/// backend reads the same bundled config the frontend sees.
fn config_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(dir) = app.path().resource_dir() {
        let path = dir.join("config.json");
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for candidate in [cwd.join("../config.json"), cwd.join("config.json")] {
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The admin signing public key (normalized hex) from `config.json`, or `None`
/// when unset/blank/malformed. A malformed anchor is treated as absent (→ admin
/// bootstrap) rather than erroring, to preserve the anti-lockout invariant.
fn read_trust_anchor(app: &tauri::AppHandle) -> Option<String> {
    let path = config_path(app)?;
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let key = value
        .get("trustAnchor")?
        .get("signingPublicKey")?
        .as_str()?
        .trim();
    if key.is_empty() {
        return None;
    }
    match devices::normalize_signing_public_key(key) {
        Ok(anchor) => Some(anchor),
        Err(_) => {
            // A NON-empty but malformed anchor is almost always a distribution
            // accident (a typo in config.json). Treating it as "absent" would
            // silently make EVERY collaborator an admin, so shout about it. The
            // build is still usable for the admin; this just makes the mistake
            // loud instead of silent.
            eprintln!(
                "WARNING: config.json trustAnchor.signingPublicKey is set but is not a valid \
                 Ed25519 key — treating the build as UNCONFIGURED (every install resolves to \
                 admin). Do NOT distribute this build until the anchor is fixed."
            );
            None
        }
    }
}

// --- digest / sign / verify -------------------------------------------------

/// Deterministic digest the admin signs and the collaborator verifies. serde
/// serializes struct fields in declaration order, so both sides hash identical
/// bytes for the same `RoleGrant`.
fn role_grant_digest(grant: &RoleGrant) -> Result<[u8; 32], String> {
    let bytes = serde_json::to_vec(grant)
        .map_err(|e| format!("Role grant could not be serialized: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(ROLE_GRANT_DOMAIN);
    hasher.update(&bytes);
    Ok(hasher.finalize().into())
}

fn decode_ed25519_hex(value: &str, label: &str) -> Result<[u8; 32], String> {
    let normalized = devices::normalize_signing_public_key(value)
        .map_err(|_| format!("{label} is not a valid Ed25519 key."))?;
    let bytes = hex::decode(&normalized).map_err(|_| format!("{label} is not valid hex."))?;
    bytes
        .try_into()
        .map_err(|_| format!("{label} has the wrong length."))
}

/// App-independent crypto verification: the scheme is understood, the issuer key
/// IS the trust anchor, and the Ed25519 signature over the recomputed digest is
/// valid (strict: rejects small-order keys and malleable signatures). Does NOT
/// check subject binding or expiry — see `verify_grant_against_anchor`.
fn verify_grant_crypto(signed: &SignedRoleGrant, anchor: &str) -> Result<(), String> {
    if signed.scheme != ROLE_GRANT_SCHEME {
        return Err(format!(
            "Role grant scheme {:?} is not supported; refusing.",
            signed.scheme
        ));
    }
    // The grant MUST be signed by the trust anchor's key — not merely any valid key.
    let issuer = devices::normalize_signing_public_key(&signed.issuer_signing_public_key)
        .map_err(|_| "Role grant issuer key is malformed.".to_string())?;
    let anchor = devices::normalize_signing_public_key(anchor)
        .map_err(|_| "Trust anchor key is malformed.".to_string())?;
    if !issuer.eq_ignore_ascii_case(&anchor) {
        return Err("Role grant was not issued by the trusted admin key; refusing.".into());
    }

    let digest = role_grant_digest(&signed.grant)?;
    let issuer_bytes = decode_ed25519_hex(&issuer, "Role grant issuer key")?;
    let verifying_key = VerifyingKey::from_bytes(&issuer_bytes)
        .map_err(|_| "Role grant issuer key is not a valid Ed25519 key.".to_string())?;
    let sig_bytes: [u8; 64] = hex::decode(signed.signature.trim())
        .map_err(|_| "Role grant signature is not valid hex.".to_string())?
        .try_into()
        .map_err(|_| "Role grant signature has the wrong length.".to_string())?;
    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify_strict(&digest, &signature)
        .map_err(|_| "Role grant signature is invalid; refusing.".to_string())
}

/// App-independent expiry check (fails closed on a malformed or past timestamp).
fn check_grant_not_expired(grant: &RoleGrant) -> Result<(), String> {
    if let Some(expires_at) = grant.expires_at.as_deref() {
        let expiry = DateTime::parse_from_rfc3339(expires_at.trim())
            .map_err(|_| "Role grant expiry timestamp is invalid.".to_string())?
            .with_timezone(&Utc);
        if Utc::now() >= expiry {
            return Err("Role grant has expired; ask the admin for a new one.".into());
        }
    }
    Ok(())
}

/// Verify a signed grant against the trust `anchor`. Fails closed: wrong scheme,
/// wrong issuer (not the anchor), bad signature, expired, or a subject that does
/// not match THIS device all return Err. On success returns the granted role.
fn verify_grant_against_anchor(
    app: &tauri::AppHandle,
    signed: &SignedRoleGrant,
    anchor: &str,
) -> Result<Role, String> {
    verify_grant_crypto(signed, anchor)?;
    check_grant_not_expired(&signed.grant)?;

    // Subject binding: the grant must be for THIS device's signing identity, so a
    // collaborator cannot adopt a grant minted for someone else with a higher role.
    let local = devices::read_local_device_status(app)?;
    let local_signing = local
        .signing_public_key
        .ok_or_else(|| "This device has no signing identity yet.".to_string())?;
    let grant_subject =
        devices::normalize_signing_public_key(&signed.grant.subject_signing_public_key)
            .map_err(|_| "Role grant subject key is malformed.".to_string())?;
    if !grant_subject.eq_ignore_ascii_case(&local_signing) {
        return Err("Role grant was issued for a different device; refusing.".into());
    }

    Ok(signed.grant.role)
}

// --- local grant store ------------------------------------------------------

fn role_grant_store_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "App data directory is unavailable.".to_string())?
        .join("devices");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the device data directory: {e}"))?;
    Ok(dir.join("role-grant.json"))
}

fn read_local_grant(app: &tauri::AppHandle) -> Option<SignedRoleGrant> {
    let path = role_grant_store_path(app).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_local_grant(app: &tauri::AppHandle, signed: &SignedRoleGrant) -> Result<(), String> {
    let path = role_grant_store_path(app)?;
    let raw = serde_json::to_string_pretty(signed)
        .map_err(|e| format!("Role grant could not be serialized: {e}"))?;
    std::fs::write(path, raw).map_err(|e| format!("Role grant could not be saved: {e}"))
}

// --- resolution (the anti-lockout core) -------------------------------------

// --- DEBUG-only role impersonation (Phase 7) --------------------------------
// Compiled out of release builds entirely: the override storage and setter exist
// only under `debug_assertions`, so production cannot impersonate a role.

#[cfg(debug_assertions)]
fn debug_role_override() -> &'static std::sync::RwLock<Option<Role>> {
    static OVERRIDE: std::sync::OnceLock<std::sync::RwLock<Option<Role>>> =
        std::sync::OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::RwLock::new(None))
}

#[cfg(debug_assertions)]
fn debug_impersonated_role() -> Option<Role> {
    debug_role_override().read().ok().and_then(|g| *g)
}

#[cfg(debug_assertions)]
fn set_debug_impersonated_role(role: Option<Role>) {
    if let Ok(mut guard) = debug_role_override().write() {
        *guard = role;
    }
}

/// The role this install runs as, for ENFORCEMENT. Never errors and never denies
/// access — see the module-level anti-lockout invariant.
pub fn resolve_local_role(app: &tauri::AppHandle) -> Role {
    // DEBUG builds only: an impersonation override wins, so the developer can test
    // each role. This branch does not exist in release.
    #[cfg(debug_assertions)]
    if let Some(role) = debug_impersonated_role() {
        return role;
    }
    let anchor = match read_trust_anchor(app) {
        // Empty/blank/malformed anchor => admin bootstrap. Never a lockout.
        None => return Role::Admin,
        Some(anchor) => anchor,
    };
    if let Ok(status) = devices::read_local_device_status(app) {
        if let Some(local_signing) = status.signing_public_key {
            if local_signing.eq_ignore_ascii_case(&anchor) {
                return Role::Admin;
            }
        }
    }
    match read_local_grant(app) {
        Some(signed) => verify_grant_against_anchor(app, &signed, &anchor).unwrap_or_default(),
        None => Role::default(),
    }
}

/// Richer status for the UI / onboarding: distinguishes a fresh collaborator
/// (no grant yet → wizard) from one running a verified grant.
pub fn local_role_status(app: &tauri::AppHandle) -> LocalRoleStatus {
    #[cfg(debug_assertions)]
    if let Some(role) = debug_impersonated_role() {
        return LocalRoleStatus {
            role,
            provisioned: true,
            is_admin: role == Role::Admin,
            anchor_configured: read_trust_anchor(app).is_some(),
        };
    }
    let anchor = read_trust_anchor(app);
    let anchor_configured = anchor.is_some();

    // Admin via empty anchor (bootstrap).
    let Some(anchor) = anchor else {
        return LocalRoleStatus {
            role: Role::Admin,
            provisioned: true,
            is_admin: true,
            anchor_configured: false,
        };
    };

    // Admin via key match.
    if let Ok(status) = devices::read_local_device_status(app) {
        if let Some(local_signing) = status.signing_public_key {
            if local_signing.eq_ignore_ascii_case(&anchor) {
                return LocalRoleStatus {
                    role: Role::Admin,
                    provisioned: true,
                    is_admin: true,
                    anchor_configured,
                };
            }
        }
    }

    // Collaborator: provisioned only if a valid grant verifies.
    match read_local_grant(app).and_then(|s| verify_grant_against_anchor(app, &s, &anchor).ok()) {
        Some(role) => LocalRoleStatus {
            role,
            provisioned: true,
            is_admin: false,
            anchor_configured,
        },
        None => LocalRoleStatus {
            role: Role::default(),
            provisioned: false,
            is_admin: false,
            anchor_configured,
        },
    }
}

// --- issuance (admin) -------------------------------------------------------

/// Build and sign a role grant for a subject device, using THIS device's Ed25519
/// signing key. Admin-only at the command layer.
pub fn issue_role_grant_inner(
    app: &tauri::AppHandle,
    role: Role,
    subject_public_key: &str,
    subject_signing_public_key: &str,
    subject_fingerprint: &str,
    expires_in_days: Option<u32>,
) -> Result<SignedRoleGrant, String> {
    let subject_public_key = devices::normalize_public_key(subject_public_key)
        .map_err(|_| "Subject key-exchange public key is malformed.".to_string())?;
    let subject_signing_public_key =
        devices::normalize_signing_public_key(subject_signing_public_key)
            .map_err(|_| "Subject signing public key is malformed.".to_string())?;
    let subject_fingerprint = subject_fingerprint.trim().to_string();
    if subject_fingerprint.is_empty() {
        return Err("Subject fingerprint is required.".into());
    }

    let issued_at = Utc::now();
    let expires_at =
        expires_in_days.map(|days| (issued_at + chrono::Duration::days(days as i64)).to_rfc3339());
    let grant = RoleGrant {
        role,
        subject_public_key,
        subject_signing_public_key,
        subject_fingerprint,
        issued_at: issued_at.to_rfc3339(),
        expires_at,
    };

    let (signing_key, issuer_public_key, issuer_fingerprint) =
        devices::load_local_signing_key(app)?;
    let digest = role_grant_digest(&grant)?;
    let signature: Signature = signing_key.sign(&digest);

    Ok(SignedRoleGrant {
        grant,
        scheme: ROLE_GRANT_SCHEME.into(),
        issuer_signing_public_key: issuer_public_key.to_ascii_lowercase(),
        issuer_fingerprint,
        signature: hex::encode(signature.to_bytes()),
    })
}

/// Verify a pasted grant and persist it locally so the app adopts the role.
pub fn adopt_role_grant_inner(
    app: &tauri::AppHandle,
    signed: &SignedRoleGrant,
) -> Result<LocalRoleStatus, String> {
    let anchor = read_trust_anchor(app).ok_or_else(|| {
        "This build has no trust anchor configured, so it cannot verify a role grant. \
         (An admin build runs as admin without one.)"
            .to_string()
    })?;
    // Verify before writing anything; a bad grant must not be stored.
    verify_grant_against_anchor(app, signed, &anchor)?;
    write_local_grant(app, signed)?;
    Ok(local_role_status(app))
}

// --- tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn get_local_role(
    app: tauri::AppHandle,
    state: tauri::State<'_, super::state::BackendState>,
) -> Result<LocalRoleStatus, String> {
    state.ensure_unlocked()?;
    Ok(local_role_status(&app))
}

#[tauri::command]
pub fn issue_role_grant(
    app: tauri::AppHandle,
    state: tauri::State<'_, super::state::BackendState>,
    role: Role,
    #[allow(non_snake_case)] subjectPublicKey: String,
    #[allow(non_snake_case)] subjectSigningPublicKey: String,
    #[allow(non_snake_case)] subjectFingerprint: String,
    #[allow(non_snake_case)] expiresInDays: Option<u32>,
) -> Result<SignedRoleGrant, String> {
    state.ensure_unlocked()?;
    require_capability(&app, Capability::IssueRoleGrant)?;
    issue_role_grant_inner(
        &app,
        role,
        &subjectPublicKey,
        &subjectSigningPublicKey,
        &subjectFingerprint,
        expiresInDays,
    )
}

#[tauri::command]
pub fn verify_and_adopt_role_grant(
    app: tauri::AppHandle,
    state: tauri::State<'_, super::state::BackendState>,
    grant: SignedRoleGrant,
) -> Result<LocalRoleStatus, String> {
    state.ensure_unlocked()?;
    adopt_role_grant_inner(&app, &grant)
}

/// One-click admin setup: write THIS device's Ed25519 signing PUBLIC key into
/// `config.json` as the trust anchor, so the build you ship bundles it and every
/// collaborator verifies your grants against it. The PRIVATE key never leaves the
/// OS vault — only the public half is written. Run this in the dev build before
/// packaging; in a packaged (read-only) build the write fails with a clear error.
///
/// Returns the written public key. Admin-only (and only the admin can reach this
/// in the first place, since the device is admin while the anchor is unset).
#[tauri::command]
pub fn bake_trust_anchor(
    app: tauri::AppHandle,
    state: tauri::State<'_, super::state::BackendState>,
) -> Result<String, String> {
    state.ensure_unlocked()?;
    require_capability(&app, Capability::ManageDevices)?;

    let (_signing_key, signing_public_key, _fingerprint) = devices::load_local_signing_key(&app)?;
    let path = config_path(&app)
        .ok_or_else(|| "config.json could not be located to write the trust anchor.".to_string())?;

    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("config.json is not a JSON object.".into());
    }
    value["trustAnchor"] = serde_json::json!({
        "signingPublicKey": signing_public_key,
        "issuedAt": Utc::now().format("%Y-%m-%d").to_string(),
    });
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Could not serialize config.json: {e}"))?;
    std::fs::write(&path, format!("{pretty}\n")).map_err(|e| {
        format!(
            "Could not write config.json at {}: {e}. In a packaged build this file is read-only — \
             run this in the dev build before packaging.",
            path.to_string_lossy()
        )
    })?;

    Ok(signing_public_key)
}

/// DEBUG-only role impersonation for testing each role before distribution. The
/// override path is compiled out of release, so in a release build this returns
/// an error and changes nothing — impersonation is physically impossible there.
#[tauri::command]
pub fn set_debug_role(
    app: tauri::AppHandle,
    state: tauri::State<'_, super::state::BackendState>,
    role: Option<Role>,
) -> Result<LocalRoleStatus, String> {
    state.ensure_unlocked()?;
    #[cfg(debug_assertions)]
    {
        set_debug_impersonated_role(role);
        Ok(local_role_status(&app))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (app, role);
        Err("Role impersonation is not available in release builds.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> (SigningKey, String) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let public = hex::encode(signing_key.verifying_key().to_bytes());
        (signing_key, public)
    }

    fn sample_grant(role: Role) -> RoleGrant {
        RoleGrant {
            role,
            subject_public_key: "aa".repeat(32),
            subject_signing_public_key: "bb".repeat(32),
            subject_fingerprint: "ABCD-1234-EF56-7890".into(),
            issued_at: "2026-05-30T00:00:00+00:00".into(),
            expires_at: None,
        }
    }

    /// Sign `grant` with `signer`, but stamp `issuer_field` as the claimed issuer
    /// key (so we can model an attacker who lies about who signed).
    fn signed_with(grant: RoleGrant, signer: &SigningKey, issuer_field: &str) -> SignedRoleGrant {
        let digest = role_grant_digest(&grant).unwrap();
        let signature: Signature = signer.sign(&digest);
        SignedRoleGrant {
            grant,
            scheme: ROLE_GRANT_SCHEME.into(),
            issuer_signing_public_key: issuer_field.to_string(),
            issuer_fingerprint: "ISSU-ER00".into(),
            signature: hex::encode(signature.to_bytes()),
        }
    }

    #[test]
    fn digest_is_deterministic_and_role_sensitive() {
        let g = sample_grant(Role::Collaborator);
        assert_eq!(
            role_grant_digest(&g).unwrap(),
            role_grant_digest(&g).unwrap()
        );
        let admin = sample_grant(Role::Admin);
        assert_ne!(
            role_grant_digest(&g).unwrap(),
            role_grant_digest(&admin).unwrap(),
            "changing the role must change the signed digest"
        );
    }

    #[test]
    fn honest_grant_verifies_against_its_anchor() {
        let (admin, admin_pub) = key(1);
        let signed = signed_with(sample_grant(Role::Collaborator), &admin, &admin_pub);
        verify_grant_crypto(&signed, &admin_pub).expect("honest grant must verify");
    }

    #[test]
    fn tampered_role_fails() {
        // Sign a Collaborator grant, then flip the role to Admin without re-signing.
        let (admin, admin_pub) = key(1);
        let mut signed = signed_with(sample_grant(Role::Collaborator), &admin, &admin_pub);
        signed.grant.role = Role::Admin;
        assert!(
            verify_grant_crypto(&signed, &admin_pub).is_err(),
            "a privilege-escalated grant must fail signature verification"
        );
    }

    #[test]
    fn wrong_issuer_is_refused() {
        // Attacker signs with their OWN key and honestly stamps their own pubkey,
        // but the anchor is the admin key -> issuer != anchor.
        let (attacker, attacker_pub) = key(9);
        let (_admin, admin_pub) = key(1);
        let signed = signed_with(sample_grant(Role::Admin), &attacker, &attacker_pub);
        assert!(
            verify_grant_crypto(&signed, &admin_pub).is_err(),
            "a grant signed by a non-anchor key must be refused"
        );
    }

    #[test]
    fn lied_issuer_with_wrong_signature_is_refused() {
        // Attacker claims the admin issued it (issuer field = anchor) but actually
        // signed with their own key -> signature check against the anchor fails.
        let (attacker, _attacker_pub) = key(9);
        let (_admin, admin_pub) = key(1);
        let signed = signed_with(sample_grant(Role::Admin), &attacker, &admin_pub);
        assert!(
            verify_grant_crypto(&signed, &admin_pub).is_err(),
            "claiming the anchor as issuer without its signature must fail"
        );
    }

    #[test]
    fn flipped_signature_fails() {
        let (admin, admin_pub) = key(1);
        let mut signed = signed_with(sample_grant(Role::Collaborator), &admin, &admin_pub);
        let mut bytes = hex::decode(&signed.signature).unwrap();
        bytes[0] ^= 0x01;
        signed.signature = hex::encode(bytes);
        assert!(verify_grant_crypto(&signed, &admin_pub).is_err());
    }

    #[test]
    fn wrong_scheme_fails() {
        let (admin, admin_pub) = key(1);
        let mut signed = signed_with(sample_grant(Role::Collaborator), &admin, &admin_pub);
        signed.scheme = "sha256-header".into(); // the package scheme, not ours
        assert!(
            verify_grant_crypto(&signed, &admin_pub).is_err(),
            "a package-signature scheme must not be accepted as a role grant"
        );
    }

    #[test]
    fn expiry_is_enforced() {
        let mut past = sample_grant(Role::Collaborator);
        past.expires_at = Some((Utc::now() - chrono::Duration::days(1)).to_rfc3339());
        assert!(
            check_grant_not_expired(&past).is_err(),
            "expired grant must fail"
        );

        let mut future = sample_grant(Role::Collaborator);
        future.expires_at = Some((Utc::now() + chrono::Duration::days(1)).to_rfc3339());
        assert!(
            check_grant_not_expired(&future).is_ok(),
            "future grant must pass"
        );

        let mut bad = sample_grant(Role::Collaborator);
        bad.expires_at = Some("not-a-date".into());
        assert!(
            check_grant_not_expired(&bad).is_err(),
            "malformed expiry must fail closed"
        );

        assert!(
            check_grant_not_expired(&sample_grant(Role::Collaborator)).is_ok(),
            "a grant with no expiry must pass the expiry check"
        );
    }

    #[test]
    fn capability_matrix_admin_all_collaborator_none() {
        for cap in [
            Capability::ManageDevices,
            Capability::CreateBootstrap,
            Capability::IssueRoleGrant,
        ] {
            assert!(
                role_has_capability(Role::Admin, cap),
                "admin must hold every capability"
            );
            assert!(
                !role_has_capability(Role::Collaborator, cap),
                "collaborator must hold no privileged capability"
            );
        }
    }
}
