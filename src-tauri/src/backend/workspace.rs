use super::devices;
use super::model::{
    WorkspaceClassificationEntry, WorkspaceDecryptResult, WorkspaceGitRepoStatus,
    WorkspaceHygieneSnapshot, WorkspaceLargeFile, WorkspacePackageInfo, WorkspacePackageRecipient,
    WorkspacePackageResult, WorkspacePackageSnapshot, WorkspacePolicyFile, WorkspaceSizeEntry,
};
use super::state::BackendState;
use super::vault;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use getrandom::fill as random_fill;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tauri::{Manager, State};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const WORKSPACE_DIR: &str = "_workspace";
const INVENTORY_DIR: &str = "inventory";
const MANIFESTS_DIR: &str = "manifests";
const PACKAGES_DIR: &str = "packages";
const IMPORTS_DIR: &str = "imports";
/// Where a cloud-fetched `.aspiswspkg` lands before it is decrypted. Kept
/// separate from `imports` (which holds *extracted* folders) so a download can
/// never collide with a decrypt output.
const DOWNLOADS_DIR: &str = "downloads";
/// A 1 GiB package over a slow link must still complete; the body is hard-capped
/// at PACKAGE_MAX_BYTES regardless.
const PACKAGE_DOWNLOAD_TIMEOUT_SECS: u64 = 1800;
const TOP_LEVEL_CSV: &str = "top-level-size.csv";
const LARGE_FILES_CSV: &str = "large-files-over-50mb.csv";
const GIT_REPOS_CSV: &str = "git-repos.csv";
const CLASSIFICATION_CSV: &str = "classification.csv";
const LARGE_FILE_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;
const LARGE_FILE_LIMIT: usize = 500;
const PACKAGE_MAGIC: &[u8] = b"ASPISWSPKG3\n";
const PACKAGE_VERSION: u32 = 3;
const PACKAGE_CHUNK_SIZE: usize = 1024 * 1024;
const PACKAGE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const PACKAGE_MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
const PACKAGE_MAX_RECIPIENTS: usize = 256;
const PACKAGE_MAX_ENTRIES: u64 = 200_000;
// The manifest lives inside the JSON header, which is hard-capped at 1 MiB on
// read (see read_package_header). Each manifest entry is roughly 120 bytes of
// JSON (path + size + 64-hex sha256), so the header cap already bounds the
// manifest; this is an explicit, earlier ceiling on the entry count so CREATE
// fails with a clear message rather than overflowing the header cap.
const PACKAGE_MAX_MANIFEST_ENTRIES: usize = 50_000;

const TEXT_EXTENSIONS: &[&str] = &[
    "css",
    "gradle",
    "html",
    "java",
    "js",
    "jsx",
    "json",
    "jsonc",
    "kt",
    "kts",
    "md",
    "mjs",
    "cjs",
    "mts",
    "cts",
    "properties",
    "ps1",
    "py",
    "r",
    "rmd",
    "rs",
    "sh",
    "sql",
    "toml",
    "ts",
    "tsx",
    "xml",
    "txt",
    "yaml",
    "yml",
];

const ALWAYS_EXCLUDED_DIRS: &[&str] = &[
    ".cache",
    ".cxx",
    ".agents",
    ".claude",
    ".codex",
    ".deepseek",
    ".expo",
    ".git",
    ".gradle",
    ".gradle-home",
    ".gradle-home-release",
    ".idea",
    ".mypy_cache",
    ".pytest_cache",
    ".rnaseq-reference-cache",
    ".ruff_cache",
    ".secrets",
    ".venv",
    ".wrangler",
    "__pycache__",
    "build",
    "codex-runs",
    "codex-sessions",
    "coverage",
    "dist",
    "graphify-out",
    "node_modules",
    "oracle-data",
    "outputs",
    "target",
    "tmp",
    "venv",
];

#[tauri::command]
pub fn get_workspace_hygiene_snapshot(
    state: State<'_, BackendState>,
) -> Result<WorkspaceHygieneSnapshot, String> {
    state.ensure_unlocked()?;
    snapshot_from_reports(resolve_workspace_root()?, false)
}

#[tauri::command]
pub async fn scan_workspace_hygiene(
    state: State<'_, BackendState>,
) -> Result<WorkspaceHygieneSnapshot, String> {
    state.ensure_unlocked()?;
    let root = resolve_workspace_root()?;
    tauri::async_runtime::spawn_blocking(move || {
        write_workspace_reports(&root)?;
        snapshot_from_reports(root, true)
    })
    .await
    .map_err(|e| format!("Workspace scan task failed: {e}"))?
}

#[tauri::command]
pub fn get_workspace_package_snapshot(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<WorkspacePackageSnapshot, String> {
    state.ensure_unlocked()?;
    workspace_package_snapshot(&app)
}

#[tauri::command]
pub async fn create_workspace_bootstrap_package(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<WorkspacePackageResult, String> {
    state.ensure_unlocked()?;
    super::roles::require_capability(&app, super::roles::Capability::CreateBootstrap)?;
    tauri::async_runtime::spawn_blocking(move || create_bootstrap_package(&app))
        .await
        .map_err(|e| format!("Workspace package task failed: {e}"))?
}

#[tauri::command]
pub async fn decrypt_workspace_bootstrap_package(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    package_path: String,
    // C1: provenance gate. Defaults to false on the wire; the frontend passes it
    // explicitly. When false, a valid signature from an UNKNOWN (unapproved)
    // signer is refused before extraction. The user must opt in after verifying
    // the fingerprint out-of-band.
    #[allow(non_snake_case)] allowUnknownSigner: Option<bool>,
) -> Result<WorkspaceDecryptResult, String> {
    state.ensure_unlocked()?;
    let allow_unknown_signer = allowUnknownSigner.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        decrypt_bootstrap_package(&app, &package_path, allow_unknown_signer)
    })
    .await
    .map_err(|e| format!("Workspace decrypt task failed: {e}"))?
}

/// Fetch an encrypted bootstrap package from a cloud URL so a collaborator who
/// does NOT yet have the Aspis Bio folder can pull it, then run the normal
/// signature-verified decrypt on the downloaded file. The download is untrusted
/// transport only — it never decrypts; the existing decrypt path still enforces
/// the Ed25519 signature, recipient match and signed manifest.
#[tauri::command]
pub async fn download_workspace_bootstrap_package(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    url: String,
    // Optional out-of-band SHA-256 of the container; when present the download is
    // rejected unless it matches. Camel-cased on the wire to match the frontend.
    #[allow(non_snake_case)] expectedSha256: Option<String>,
) -> Result<WorkspacePackageInfo, String> {
    state.ensure_unlocked()?;
    tauri::async_runtime::spawn_blocking(move || {
        download_bootstrap_package(&app, &url, expectedSha256.as_deref())
    })
    .await
    .map_err(|e| format!("Workspace download task failed: {e}"))?
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageHeader {
    version: u32,
    package_id: String,
    created_at: String,
    algorithm: String,
    chunk_size: usize,
    payload_nonce_prefix: String,
    root_name: String,
    recipients: Vec<PackageHeaderRecipient>,
    /// Signed file inventory of the package payload. Lives inside the header, so
    /// it is covered both by the header digest (which the signature signs) and by
    /// the per-chunk AAD binding (which the payload AEAD enforces).
    manifest: Vec<PackageManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PackageManifestEntry {
    relative_path: String,
    size: u64,
    sha256_hex: String,
}

/// Ed25519 signature over `SHA-256(canonical_header_bytes)`. Stored OUTSIDE the
/// signed header (it signs the header) but inside the package container, framed
/// right after the header and before the encrypted payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageSignatureBlock {
    /// What the signature covers, for forward-compatibility. Always
    /// "sha256-header" in v3.
    scheme: String,
    signer_public_key: String,
    signer_fingerprint: String,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageHeaderRecipient {
    fingerprint: String,
    collaborator_name: String,
    device_name: String,
    platform: String,
    source: String,
    ephemeral_public_key: String,
    wrap_nonce: String,
    wrapped_key: String,
}

#[derive(Debug, Clone)]
struct PackageCandidate {
    path: PathBuf,
    relative_path: String,
    bytes: u64,
}

#[derive(Debug, Clone)]
struct PackageCandidateSet {
    files: Vec<PackageCandidate>,
    total_bytes: u64,
    skipped_files: u64,
    skipped_bytes: u64,
    warnings: Vec<String>,
}

fn workspace_package_snapshot(app: &tauri::AppHandle) -> Result<WorkspacePackageSnapshot, String> {
    let root = resolve_workspace_root()?;
    let package_dir = workspace_package_dir(&root)?;
    let import_dir = workspace_import_dir(&root)?;
    let approved_recipients = package_recipients(app)?;
    let latest_packages = latest_workspace_packages(&package_dir)?;
    let mut warnings = Vec::new();
    if approved_recipients.is_empty() {
        warnings.push("No approved devices are available for package encryption.".into());
    }
    Ok(WorkspacePackageSnapshot {
        root: root.to_string_lossy().into_owned(),
        package_dir: package_dir.to_string_lossy().into_owned(),
        import_dir: import_dir.to_string_lossy().into_owned(),
        approved_recipients,
        latest_packages,
        max_package_size_mb: PACKAGE_MAX_BYTES / 1024 / 1024,
        warnings,
    })
}

fn create_bootstrap_package(app: &tauri::AppHandle) -> Result<WorkspacePackageResult, String> {
    let root = resolve_workspace_root()?;
    let recipients = package_recipients(app)?;
    if recipients.is_empty() {
        return Err("Create at least one local or approved device before packaging.".into());
    }
    let candidates = collect_package_candidates(&root)?;
    if candidates.files.is_empty() {
        return Err("No packageable source/docs files found. Check .aspisignore.".into());
    }
    if candidates.total_bytes > PACKAGE_MAX_BYTES {
        return Err(format!(
            "Package would be {:.1} MB, above the {} MB limit. Tighten .aspisignore first.",
            candidates.total_bytes as f64 / 1024f64.powi(2),
            PACKAGE_MAX_BYTES / 1024 / 1024
        ));
    }

    let package_dir = workspace_package_dir(&root)?;
    let created_at = now();
    let stamp = created_at
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let package_id = format!("aspis-bootstrap-{stamp}");
    let file_name = format!("{package_id}.aspiswspkg");
    let output_path = package_dir.join(&file_name);

    // A5: keep the raw 32-byte data key in zeroizing memory.
    let mut data_key = Zeroizing::new([0u8; 32]);
    random_fill(data_key.as_mut_slice())
        .map_err(|e| format!("Package key generation failed: {e}"))?;
    let mut payload_prefix = [0u8; 4];
    random_fill(&mut payload_prefix)
        .map_err(|e| format!("Package nonce generation failed: {e}"))?;

    // S2: load the local device Ed25519 signing keypair (generating it if this is
    // a pre-signing installation) BEFORE building the header, so a missing key
    // fails fast rather than after writing a partial package.
    let (signing_key, signer_public_key, signer_fingerprint) =
        devices::load_local_signing_key(app)?;

    // S1: build the signed manifest over the exact tar payload (README first,
    // then every candidate file, matching the tar append order below).
    let readme = package_readme(&root, &candidates, &recipients, &created_at);
    let readme_path = "ASPIS_BOOTSTRAP_README.md";
    let manifest = build_package_manifest(readme_path, readme.as_bytes(), &candidates.files)?;

    let header = package_header(
        &package_id,
        &created_at,
        &root,
        &recipients,
        &data_key,
        &payload_prefix,
        manifest,
    )?;
    let mut out = fs::File::create(&output_path)
        .map_err(|e| format!("Could not create package {}: {e}", output_path.display()))?;
    // S3: write the header, then sign SHA-256(header) and write the signature
    // block right after the header (outside the signed bytes, inside the
    // container). The header digest is also bound into the payload AAD, so the
    // signature transitively authenticates the whole container.
    let header_digest = write_package_header(&mut out, &header)?;
    let signature_block = sign_package_header(
        &signing_key,
        &header_digest,
        &signer_public_key,
        &signer_fingerprint,
    )?;
    write_package_signature_block(&mut out, &signature_block)?;
    let mut writer = EncryptedPackageWriter::new(
        out,
        package_id.clone(),
        data_key,
        payload_prefix,
        header_digest,
    )?;

    {
        let mut builder = tar::Builder::new(&mut writer);
        append_bytes_to_tar(&mut builder, readme_path, readme.as_bytes())?;
        for candidate in &candidates.files {
            append_file_to_tar(&mut builder, candidate)?;
        }
        builder
            .finish()
            .map_err(|e| format!("Package tar finalization failed: {e}"))?;
    }
    let package_bytes = writer.finish()?;

    Ok(WorkspacePackageResult {
        package_id,
        path: output_path.to_string_lossy().into_owned(),
        file_name,
        file_count: candidates.files.len() as u64 + 1,
        total_bytes: candidates.total_bytes + readme.len() as u64,
        package_bytes,
        recipient_count: recipients.len() as u64,
        skipped_files: candidates.skipped_files,
        skipped_bytes: candidates.skipped_bytes,
        readme_path: readme_path.into(),
        created_at,
        warnings: candidates.warnings,
    })
}

fn decrypt_bootstrap_package(
    app: &tauri::AppHandle,
    package_path: &str,
    allow_unknown_signer: bool,
) -> Result<WorkspaceDecryptResult, String> {
    let path = PathBuf::from(package_path.trim());
    if !path.is_file() {
        return Err("Package file does not exist.".into());
    }
    let mut file =
        fs::File::open(&path).map_err(|e| format!("Could not open package file: {e}"))?;
    // A1/A3/A7: header_digest binds the header into payload AAD; read_package_header
    // also validates package_id charset, caps recipients, validates the manifest
    // (M3/M4/M5), and reads + scheme-checks the detached signature block (rejecting
    // v2/unsigned packages).
    let (header, header_digest, signature_block) = read_package_header(&mut file)?;

    // S4: verify the Ed25519 signature over the header digest BEFORE touching the
    // payload. Fails closed — a bad/forged signature aborts here, so we never
    // decrypt an unauthenticated container.
    verify_package_signature(&signature_block, &header_digest)?;

    // S4 (identity, best-effort): mark the signer known if their Ed25519 key
    // matches the local device or an approved device; otherwise valid-but-unknown.
    let signer_public_key = signature_block.signer_public_key.to_ascii_lowercase();
    // FOLLOW-UP: if the device store is unreadable we degrade to "unknown" rather
    // than failing — the signature itself is already verified.
    let known_signers = devices::known_signers(app).unwrap_or_default();
    // C1/M6: resolve provenance against the known signers and enforce the
    // fail-closed unknown-signer gate. The surfaced fingerprint is recomputed from
    // the verified public key (M6), never the attacker-controlled block field.
    let SignerDecision {
        signer_fingerprint,
        signer_known,
        signer_name,
    } = resolve_signer_decision(&signer_public_key, &known_signers, allow_unknown_signer)?;

    let local = devices::read_local_device_status(app)?;
    let fingerprint = local
        .public_key_fingerprint
        .ok_or_else(|| "This app installation has no device identity.".to_string())?;
    let recipient = header
        .recipients
        .iter()
        .find(|entry| entry.fingerprint.eq_ignore_ascii_case(&fingerprint))
        .ok_or_else(|| {
            "This package was not encrypted for this device fingerprint. Approve this device and recreate the package.".to_string()
        })?;
    let private_key_hex = vault::read_device_private_key_hex()?
        .ok_or_else(|| "This device private key is missing from the OS vault.".to_string())?;
    let data_key = unwrap_package_key(&header.package_id, recipient, &private_key_hex)?;
    let payload_prefix = decode_fixed_hex::<4>(&header.payload_nonce_prefix, "payload nonce")?;
    let reader = EncryptedPackageReader::new(
        file,
        header.package_id.clone(),
        data_key,
        payload_prefix,
        header_digest,
    )?;
    let root = resolve_workspace_root().unwrap_or_else(|_| {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("workspace")
    });
    let import_dir = workspace_import_dir(&root)?;
    // A3: defense-in-depth — after joining, the parent of the import target must
    // canonicalize to import_dir, otherwise the package_id escaped the folder.
    let output_dir = import_dir.join(&header.package_id);
    assert_import_target_within(&import_dir, &output_dir)?;
    if output_dir.exists() {
        return Err(format!(
            "Import folder already exists: {}",
            output_dir.to_string_lossy()
        ));
    }
    fs::create_dir_all(&output_dir).map_err(|e| format!("Could not create import folder: {e}"))?;
    let (files_restored, bytes_restored, warnings) = safe_unpack_tar(reader, &output_dir)?;

    // S5: after extraction, verify every restored file against the signed
    // manifest (size + SHA-256), and that no file is missing or extra. Fails
    // closed: any mismatch removes the output and aborts.
    if let Err(e) = verify_restored_against_manifest(&output_dir, &header.manifest) {
        let _ = fs::remove_dir_all(&output_dir);
        return Err(e);
    }

    Ok(WorkspaceDecryptResult {
        package_id: header.package_id,
        output_dir: output_dir.to_string_lossy().into_owned(),
        files_restored,
        bytes_restored,
        recipient_fingerprint: fingerprint,
        signature_valid: true,
        signer_public_key,
        signer_fingerprint,
        signer_known,
        signer_name,
        warnings,
    })
}

/// S5: verifies the restored tree against the signed manifest. Recomputes each
/// file's SHA-256 and size, requires an exact 1:1 match between the manifest and
/// the files on disk (no missing, no extra), and rejects any divergence. The
/// per-file read is capped by PACKAGE_MAX_FILE_BYTES (the same decrypt ceiling).
fn verify_restored_against_manifest(
    output_dir: &Path,
    manifest: &[PackageManifestEntry],
) -> Result<(), String> {
    // Index the on-disk files by their forward-slash relative path.
    let mut on_disk: HashMap<String, PathBuf> = HashMap::new();
    let mut stack = vec![output_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("Could not read restored folder: {e}"))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Could not read restored entry: {e}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| format!("Could not read restored metadata: {e}"))?;
            if is_reparse_or_symlink(&metadata) {
                return Err("Restored package contains a symlink or reparse point.".into());
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                let relative = relative_path(output_dir, &path).replace('\\', "/");
                on_disk.insert(relative, path);
            }
        }
    }

    // Every manifest entry must be present and match exactly.
    for entry in manifest {
        let path = on_disk.remove(&entry.relative_path).ok_or_else(|| {
            format!(
                "Manifest verification failed: file is missing from the package: {}",
                entry.relative_path
            )
        })?;
        let metadata = fs::metadata(&path)
            .map_err(|e| format!("Could not stat restored {}: {e}", entry.relative_path))?;
        if metadata.len() > PACKAGE_MAX_FILE_BYTES {
            return Err(format!(
                "Restored file {} exceeds the per-file decrypt ceiling.",
                entry.relative_path
            ));
        }
        if metadata.len() != entry.size {
            return Err(format!(
                "Manifest verification failed: size mismatch for {} (expected {}, got {}).",
                entry.relative_path,
                entry.size,
                metadata.len()
            ));
        }
        let actual_hash = sha256_file_hex(&path)?;
        if !actual_hash.eq_ignore_ascii_case(&entry.sha256_hex) {
            return Err(format!(
                "Manifest verification failed: SHA-256 mismatch for {}.",
                entry.relative_path
            ));
        }
    }

    // No file outside the manifest is allowed.
    if let Some((extra, _)) = on_disk.into_iter().next() {
        return Err(format!(
            "Manifest verification failed: package restored an unlisted file: {extra}"
        ));
    }
    Ok(())
}

fn package_recipients(app: &tauri::AppHandle) -> Result<Vec<WorkspacePackageRecipient>, String> {
    let mut recipients = Vec::new();
    if let Ok(local) = devices::read_local_device_status(app) {
        if local.configured {
            if let (Some(public_key), Some(fingerprint)) =
                (local.public_key, local.public_key_fingerprint)
            {
                recipients.push(WorkspacePackageRecipient {
                    fingerprint,
                    collaborator_name: "This device".into(),
                    device_name: local.device_name.unwrap_or_else(|| "Local device".into()),
                    platform: local.platform,
                    source: "local_device".into(),
                    public_key: devices::normalize_public_key(&public_key)?,
                    signing_public_key: local.signing_public_key,
                    signing_fingerprint: local.signing_fingerprint,
                });
            }
        }
    }
    for invite in devices::approved_device_invites(app)? {
        recipients.push(WorkspacePackageRecipient {
            fingerprint: invite.public_key_fingerprint,
            collaborator_name: invite.collaborator_name,
            device_name: invite.device_name,
            platform: invite.platform,
            source: "approved_invite".into(),
            public_key: devices::normalize_public_key(&invite.public_key)?,
            signing_public_key: invite.signing_public_key,
            signing_fingerprint: invite.signing_fingerprint,
        });
    }
    let mut seen = HashSet::new();
    recipients.retain(|recipient| seen.insert(recipient.fingerprint.to_ascii_lowercase()));
    Ok(recipients)
}

#[allow(clippy::too_many_arguments)]
fn package_header(
    package_id: &str,
    created_at: &str,
    root: &Path,
    recipients: &[WorkspacePackageRecipient],
    data_key: &Zeroizing<[u8; 32]>,
    payload_prefix: &[u8; 4],
    manifest: Vec<PackageManifestEntry>,
) -> Result<PackageHeader, String> {
    let mut header_recipients = Vec::new();
    for recipient in recipients {
        header_recipients.push(wrap_key_for_recipient(
            package_id,
            recipient,
            &recipient.public_key,
            data_key,
        )?);
    }
    Ok(PackageHeader {
        version: PACKAGE_VERSION,
        package_id: package_id.into(),
        created_at: created_at.into(),
        algorithm: "AES-256-GCM payload, X25519-HKDF-SHA256 key wrap, Ed25519 header signature"
            .into(),
        chunk_size: PACKAGE_CHUNK_SIZE,
        payload_nonce_prefix: hex::encode(payload_prefix),
        root_name: root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("aspis bio")
            .into(),
        recipients: header_recipients,
        manifest,
    })
}

/// S1: builds the file inventory for the package payload. Hashes the README
/// (synthesized in-memory) and every candidate file with SHA-256, recording
/// `{ relative_path, size, sha256_hex }`. Entry order matches the tar append
/// order. Bounded by PACKAGE_MAX_MANIFEST_ENTRIES so the header stays well under
/// its 1 MiB cap.
fn build_package_manifest(
    readme_path: &str,
    readme_bytes: &[u8],
    files: &[PackageCandidate],
) -> Result<Vec<PackageManifestEntry>, String> {
    if files.len() + 1 > PACKAGE_MAX_MANIFEST_ENTRIES {
        return Err(format!(
            "Package would list {} files, above the {PACKAGE_MAX_MANIFEST_ENTRIES} manifest entry cap.",
            files.len() + 1
        ));
    }
    let mut manifest = Vec::with_capacity(files.len() + 1);
    manifest.push(PackageManifestEntry {
        relative_path: readme_path.to_string(),
        size: readme_bytes.len() as u64,
        sha256_hex: hex::encode(Sha256::digest(readme_bytes)),
    });
    for candidate in files {
        let sha256_hex = sha256_file_hex(&candidate.path)?;
        manifest.push(PackageManifestEntry {
            relative_path: candidate.relative_path.clone(),
            size: candidate.bytes,
            sha256_hex,
        });
    }
    // M4 (create side): refuse to emit a manifest with duplicate relative paths.
    // M3 (create side): each path must be a Component::Normal-only relative path.
    validate_manifest_paths(&manifest)?;
    Ok(manifest)
}

/// M3/M4/M5: validates a package manifest's `relative_path` entries.
///   - M5: at least one entry (the package always injects a README).
///   - M3: every path is a `Component::Normal`-only relative path (no `..`, root,
///     prefix, or absolute), reusing `safe_tar_relative_path`'s rule so a future
///     refactor that trusts the manifest cannot become a traversal primitive.
///   - M4: no duplicate `relative_path` (case-sensitive, after `\`→`/` normalize).
fn validate_manifest_paths(manifest: &[PackageManifestEntry]) -> Result<(), String> {
    // M5: empty-manifest policy.
    if manifest.is_empty() {
        return Err("Package manifest is empty; refusing (a valid package always lists at least its README).".into());
    }
    let mut seen: HashSet<String> = HashSet::with_capacity(manifest.len());
    for entry in manifest {
        // M3: reject traversal / absolute / prefix components at verify time.
        safe_tar_relative_path(Path::new(&entry.relative_path)).map_err(|_| {
            format!(
                "Package manifest contains an unsafe path: {}",
                entry.relative_path
            )
        })?;
        // M4: reject duplicate paths (normalize separators so `a/b` and `a\b` clash).
        let key = entry.relative_path.replace('\\', "/");
        if !seen.insert(key) {
            return Err(format!(
                "Package manifest lists a duplicate path: {}",
                entry.relative_path
            ));
        }
    }
    Ok(())
}

/// Streams a file through SHA-256 without loading it into memory.
fn sha256_file_hex(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Could not open {} for hashing: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("Could not read {} for hashing: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// S3: signs `package_digest = SHA-256(canonical_header_bytes)` with the
/// creator's Ed25519 key and returns the detached signature block.
fn sign_package_header(
    signing_key: &SigningKey,
    header_digest: &[u8; 32],
    signer_public_key: &str,
    signer_fingerprint: &str,
) -> Result<PackageSignatureBlock, String> {
    let signature: Signature = signing_key.sign(header_digest);
    Ok(PackageSignatureBlock {
        scheme: "sha256-header".into(),
        signer_public_key: signer_public_key.to_ascii_lowercase(),
        signer_fingerprint: signer_fingerprint.to_string(),
        signature: hex::encode(signature.to_bytes()),
    })
}

/// S4: verifies the signature block against the recomputed header digest. Fails
/// closed: a wrong scheme, a malformed key/signature, or a bad signature all
/// return Err before any decryption happens.
fn verify_package_signature(
    signature_block: &PackageSignatureBlock,
    header_digest: &[u8; 32],
) -> Result<(), String> {
    // H2: only the v3 scheme is understood. Reject any other (or empty) scheme so
    // a future/forged block cannot claim a signature covers something it does not.
    if signature_block.scheme != "sha256-header" {
        return Err(format!(
            "Package signature scheme {:?} is not supported; refusing to decrypt.",
            signature_block.scheme
        ));
    }
    let public_bytes = decode_fixed_hex::<32>(
        &signature_block.signer_public_key,
        "package signer public key",
    )?;
    let verifying_key = VerifyingKey::from_bytes(&public_bytes)
        .map_err(|_| "Package signer public key is not a valid Ed25519 key.".to_string())?;
    let signature_bytes = decode_fixed_hex::<64>(&signature_block.signature, "package signature")?;
    let signature = Signature::from_bytes(&signature_bytes);
    // H1: verify_strict rejects small-order public keys A and non-canonical /
    // malleable S, closing the malleability gap that plain `verify` leaves open.
    verifying_key
        .verify_strict(header_digest, &signature)
        .map_err(|_| "Package signature is invalid; refusing to decrypt.".to_string())
}

/// Resolved provenance of a verified package signer, used to populate the decrypt
/// result and surfaced to the UI.
#[derive(Debug)]
struct SignerDecision {
    /// Fingerprint RECOMPUTED from the verified signer public key (M6).
    signer_fingerprint: String,
    /// True when the signer's Ed25519 key matches the local device or an approved
    /// invite; false means the signature is valid but the signer is unrecognized.
    signer_known: bool,
    signer_name: Option<String>,
}

/// C1/M6: resolves the signer identity from the *verified* signer public key and
/// the set of known signers, then enforces the fail-closed provenance gate.
///
/// The Ed25519 signature itself is verified by the caller BEFORE this runs, which
/// only proves integrity ("the header was signed by the holder of this key"), not
/// provenance ("the key is one we approved"). A signature-swap attacker re-signs
/// the header with their own key, so `signer_known` is false; with
/// `allow_unknown_signer == false` this returns Err and the package is refused
/// before any extraction. The surfaced fingerprint is always recomputed from the
/// verified key (M6), never the self-reported block field.
fn resolve_signer_decision(
    signer_public_key: &str,
    known_signers: &[devices::KnownSigner],
    allow_unknown_signer: bool,
) -> Result<SignerDecision, String> {
    let signer_fingerprint = devices::signing_key_fingerprint(signer_public_key)?;
    let (signer_known, signer_name) = known_signers
        .iter()
        .find(|signer| {
            signer
                .signing_public_key
                .eq_ignore_ascii_case(signer_public_key)
        })
        .map(|signer| (true, Some(signer.name.clone())))
        .unwrap_or((false, None));

    // C1: provenance enforcement, fail-closed. A valid-but-unknown signer is
    // refused unless the user has explicitly opted in after verifying the
    // fingerprint out-of-band.
    if !signer_known && !allow_unknown_signer {
        return Err(format!(
            "Package is signed by an UNKNOWN device (not an approved signer); refused. \
             Verify this fingerprint out-of-band, then re-run with allow-unknown if you \
             trust it: {signer_fingerprint}"
        ));
    }

    Ok(SignerDecision {
        signer_fingerprint,
        signer_known,
        signer_name,
    })
}

fn wrap_key_for_recipient(
    package_id: &str,
    recipient: &WorkspacePackageRecipient,
    public_key_hex: &str,
    data_key: &Zeroizing<[u8; 32]>,
) -> Result<PackageHeaderRecipient, String> {
    let public_bytes = decode_fixed_hex::<32>(public_key_hex, "recipient public key")?;
    let recipient_public = PublicKey::from(public_bytes);
    // A5: zeroize the ephemeral private scalar.
    let mut ephemeral_private = Zeroizing::new([0u8; 32]);
    random_fill(ephemeral_private.as_mut_slice())
        .map_err(|e| format!("Recipient ephemeral key generation failed: {e}"))?;
    let ephemeral_secret = StaticSecret::from(*ephemeral_private);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&recipient_public);
    // A6: reject an all-zero shared secret (low-order point) in constant time.
    reject_all_zero_shared_secret(shared.as_bytes())?;
    let wrap_key = derive_wrap_key(package_id, &recipient.fingerprint, shared.as_bytes())?;
    let cipher = Aes256Gcm::new_from_slice(wrap_key.as_slice())
        .map_err(|_| "AES-256-GCM wrap key setup failed.".to_string())?;
    let mut wrap_nonce = [0u8; 12];
    random_fill(&mut wrap_nonce).map_err(|e| format!("Wrap nonce generation failed: {e}"))?;
    let aad = key_wrap_aad(package_id, &recipient.fingerprint);
    let wrapped_key = cipher
        .encrypt(
            Nonce::from_slice(&wrap_nonce),
            Payload {
                msg: data_key.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "Package key wrap failed.".to_string())?;
    Ok(PackageHeaderRecipient {
        fingerprint: recipient.fingerprint.clone(),
        collaborator_name: recipient.collaborator_name.clone(),
        device_name: recipient.device_name.clone(),
        platform: recipient.platform.clone(),
        source: recipient.source.clone(),
        ephemeral_public_key: hex::encode(ephemeral_public.as_bytes()),
        wrap_nonce: hex::encode(wrap_nonce),
        wrapped_key: hex::encode(wrapped_key),
    })
}

fn unwrap_package_key(
    package_id: &str,
    recipient: &PackageHeaderRecipient,
    private_key_hex: &str,
) -> Result<Zeroizing<[u8; 32]>, String> {
    // A5: keep the device private scalar in zeroizing memory.
    let private_bytes = Zeroizing::new(decode_fixed_hex::<32>(
        private_key_hex,
        "device private key",
    )?);
    let ephemeral_public_bytes = decode_fixed_hex::<32>(
        &recipient.ephemeral_public_key,
        "recipient ephemeral public key",
    )?;
    let wrap_nonce = decode_fixed_hex::<12>(&recipient.wrap_nonce, "wrap nonce")?;
    let wrapped_key = hex::decode(&recipient.wrapped_key)
        .map_err(|_| "Wrapped package key is not valid hex.".to_string())?;
    let device_secret = StaticSecret::from(*private_bytes);
    let ephemeral_public = PublicKey::from(ephemeral_public_bytes);
    let shared = device_secret.diffie_hellman(&ephemeral_public);
    // A6: reject an all-zero shared secret (low-order point) in constant time.
    reject_all_zero_shared_secret(shared.as_bytes())?;
    let wrap_key = derive_wrap_key(package_id, &recipient.fingerprint, shared.as_bytes())?;
    let cipher = Aes256Gcm::new_from_slice(wrap_key.as_slice())
        .map_err(|_| "AES-256-GCM unwrap key setup failed.".to_string())?;
    let aad = key_wrap_aad(package_id, &recipient.fingerprint);
    let data_key = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&wrap_nonce),
                Payload {
                    msg: &wrapped_key,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "Package key unwrap failed for this device.".to_string())?,
    );
    if data_key.len() != 32 {
        return Err("Unwrapped package key has the wrong length.".into());
    }
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&data_key);
    Ok(out)
}

/// A6: constant-time rejection of an all-zero X25519 shared secret, which would
/// indicate a contributory (low-order point) attack.
fn reject_all_zero_shared_secret(shared: &[u8]) -> Result<(), String> {
    let zero = [0u8; 32];
    if bool::from(shared.ct_eq(&zero)) {
        return Err("X25519 shared secret is all-zero (low-order point); refusing.".into());
    }
    Ok(())
}

fn derive_wrap_key(
    package_id: &str,
    fingerprint: &str,
    shared_secret: &[u8],
) -> Result<Zeroizing<[u8; 32]>, String> {
    let salt = format!("aspis-workspace-package-v1:{package_id}");
    let info = format!("x25519-aes256gcm-wrap:{fingerprint}");
    let hk = Hkdf::<Sha256>::new(Some(salt.as_bytes()), shared_secret);
    // A5: derived wrap key stays in zeroizing memory.
    let mut key = Zeroizing::new([0u8; 32]);
    hk.expand(info.as_bytes(), key.as_mut_slice())
        .map_err(|_| "HKDF wrap key derivation failed.".to_string())?;
    Ok(key)
}

fn key_wrap_aad(package_id: &str, fingerprint: &str) -> String {
    format!("aspis-workspace-key-wrap-v1:{package_id}:{fingerprint}")
}

/// SHA-256 over the canonical serialized header bytes (A1). Binding this into
/// every payload chunk's AAD means any header edit (recipients, package_id,
/// payload_nonce_prefix, chunk_size, ...) invalidates payload decryption.
fn header_digest(header_bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(header_bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Per-chunk AEAD associated data. Binds package_id, chunk_index, the
/// authenticated header digest (A1) and, for the final chunk, a ":final"
/// marker (A2) so truncation/extension is detectable.
fn payload_aad(
    package_id: &str,
    chunk_index: u64,
    header_digest: &[u8; 32],
    is_final: bool,
) -> String {
    let marker = if is_final { ":final" } else { "" };
    format!(
        "aspis-workspace-payload-v2:{package_id}:{chunk_index}:{}{marker}",
        hex::encode(header_digest)
    )
}

fn decode_fixed_hex<const N: usize>(value: &str, label: &str) -> Result<[u8; N], String> {
    let bytes = hex::decode(value.trim()).map_err(|_| format!("{label} is not valid hex."))?;
    bytes
        .try_into()
        .map_err(|_| format!("{label} must be {N} bytes."))
}

/// Strict validation for a package_id used as a directory name (A3). Untrusted
/// JSON must never be used as a path component without this check.
fn validate_package_id(package_id: &str) -> Result<(), String> {
    if package_id.is_empty() {
        return Err("Package id is empty.".into());
    }
    if package_id.len() > 200 {
        return Err("Package id is too long.".into());
    }
    if package_id == "." || package_id == ".." {
        return Err("Package id is a reserved path name.".into());
    }
    if package_id.contains("..") {
        return Err("Package id may not contain '..'.".into());
    }
    if package_id.chars().all(|ch| ch == '.') {
        return Err("Package id may not be dots only.".into());
    }
    if package_id
        .chars()
        .any(|ch| ch == '/' || ch == '\\' || ch == ':')
    {
        return Err("Package id may not contain path separators or drive letters.".into());
    }
    if !package_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return Err("Package id contains characters outside [A-Za-z0-9._-].".into());
    }
    Ok(())
}

/// A3: defense-in-depth assertion that `output_dir = import_dir.join(package_id)`
/// did not escape `import_dir`. `import_dir` already exists (canonicalizable);
/// `output_dir` may not exist yet, so compare against its parent.
fn assert_import_target_within(import_dir: &Path, output_dir: &Path) -> Result<(), String> {
    let canonical_import = import_dir
        .canonicalize()
        .map_err(|_| "Import folder could not be resolved.".to_string())?;
    let parent = output_dir
        .parent()
        .ok_or_else(|| "Import target has no parent directory.".to_string())?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| "Import target parent could not be resolved.".to_string())?;
    if canonical_parent != canonical_import {
        return Err("Package id resolves outside the import directory.".into());
    }
    Ok(())
}

/// Returns the SHA-256 over the canonical serialized header bytes (A1), which
/// callers bind into the payload AAD.
fn write_package_header(file: &mut fs::File, header: &PackageHeader) -> Result<[u8; 32], String> {
    let raw = serde_json::to_vec(header)
        .map_err(|e| format!("Package header could not be serialized: {e}"))?;
    if raw.len() > u32::MAX as usize {
        return Err("Package header is too large.".into());
    }
    file.write_all(PACKAGE_MAGIC)
        .map_err(|e| format!("Package magic could not be written: {e}"))?;
    file.write_all(&(raw.len() as u32).to_le_bytes())
        .map_err(|e| format!("Package header length could not be written: {e}"))?;
    file.write_all(&raw)
        .map_err(|e| format!("Package header could not be written: {e}"))?;
    Ok(header_digest(&raw))
}

/// S3: writes the detached signature block framed as `u32` length prefix + JSON,
/// directly after the header and before the encrypted payload.
fn write_package_signature_block(
    file: &mut fs::File,
    block: &PackageSignatureBlock,
) -> Result<(), String> {
    let raw = serde_json::to_vec(block)
        .map_err(|e| format!("Package signature block could not be serialized: {e}"))?;
    if raw.len() > u32::MAX as usize {
        return Err("Package signature block is too large.".into());
    }
    file.write_all(&(raw.len() as u32).to_le_bytes())
        .map_err(|e| format!("Package signature length could not be written: {e}"))?;
    file.write_all(&raw)
        .map_err(|e| format!("Package signature block could not be written: {e}"))?;
    Ok(())
}

/// Returns the parsed header, the SHA-256 over the exact header bytes read from
/// disk (A1, also bound into the payload AAD), and the detached signature block
/// (S3). A v2 or unsigned package is rejected here.
fn read_package_header(
    file: &mut fs::File,
) -> Result<(PackageHeader, [u8; 32], PackageSignatureBlock), String> {
    let mut magic = vec![0u8; PACKAGE_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|e| format!("Package header could not be read: {e}"))?;
    if magic != PACKAGE_MAGIC {
        return Err(
            "Not an Aspis workspace package (or an older unsigned package version).".into(),
        );
    }
    let mut len = [0u8; 4];
    file.read_exact(&mut len)
        .map_err(|e| format!("Package header length could not be read: {e}"))?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > 1024 * 1024 {
        return Err("Package header length is invalid.".into());
    }
    let mut raw = vec![0u8; len];
    file.read_exact(&mut raw)
        .map_err(|e| format!("Package header body could not be read: {e}"))?;
    let header: PackageHeader =
        serde_json::from_slice(&raw).map_err(|e| format!("Package header JSON is invalid: {e}"))?;
    if header.version != PACKAGE_VERSION {
        return Err(format!("Unsupported package version {}.", header.version));
    }
    // A3: validate package_id before it is ever used as a path component.
    validate_package_id(&header.package_id)?;
    // A7: cap recipients to a sane bound.
    if header.recipients.len() > PACKAGE_MAX_RECIPIENTS {
        return Err(format!(
            "Package lists {} recipients, above the {PACKAGE_MAX_RECIPIENTS} cap.",
            header.recipients.len()
        ));
    }
    // S1: cap manifest entries (defense-in-depth; the header cap already bounds it).
    if header.manifest.len() > PACKAGE_MAX_MANIFEST_ENTRIES {
        return Err(format!(
            "Package manifest lists {} entries, above the {PACKAGE_MAX_MANIFEST_ENTRIES} cap.",
            header.manifest.len()
        ));
    }
    // M3/M4/M5: reject an empty manifest, traversal/absolute paths, and duplicate
    // paths at read/verify time so the manifest can never be a traversal primitive.
    validate_manifest_paths(&header.manifest)?;
    let digest = header_digest(&raw);

    // S3: read the detached signature block that immediately follows the header.
    let mut sig_len = [0u8; 4];
    file.read_exact(&mut sig_len).map_err(|_| {
        "Package is missing its signature block (unsigned or truncated).".to_string()
    })?;
    let sig_len = u32::from_le_bytes(sig_len) as usize;
    if sig_len == 0 || sig_len > 64 * 1024 {
        return Err("Package signature block length is invalid.".into());
    }
    let mut sig_raw = vec![0u8; sig_len];
    file.read_exact(&mut sig_raw)
        .map_err(|e| format!("Package signature block could not be read: {e}"))?;
    let signature_block: PackageSignatureBlock = serde_json::from_slice(&sig_raw)
        .map_err(|e| format!("Package signature block JSON is invalid: {e}"))?;

    Ok((header, digest, signature_block))
}

struct EncryptedPackageWriter {
    inner: fs::File,
    cipher: Aes256Gcm,
    package_id: String,
    nonce_prefix: [u8; 4],
    header_digest: [u8; 32],
    chunk_index: u64,
    buffer: Vec<u8>,
}

impl EncryptedPackageWriter {
    fn new(
        inner: fs::File,
        package_id: String,
        data_key: Zeroizing<[u8; 32]>,
        nonce_prefix: [u8; 4],
        header_digest: [u8; 32],
    ) -> Result<Self, String> {
        let cipher = Aes256Gcm::new_from_slice(data_key.as_slice())
            .map_err(|_| "AES-256-GCM payload setup failed.".to_string())?;
        Ok(Self {
            inner,
            cipher,
            package_id,
            nonce_prefix,
            header_digest,
            chunk_index: 0,
            buffer: Vec::with_capacity(PACKAGE_CHUNK_SIZE),
        })
    }

    fn finish(mut self) -> Result<u64, String> {
        // A2: always emit an authenticated final chunk (possibly empty) so the
        // reader requires an authenticated end-of-stream marker. The bare 0u32
        // length prefix is no longer the sole end signal.
        let chunk = std::mem::take(&mut self.buffer);
        self.write_encrypted_chunk(&chunk, true)
            .map_err(|e| format!("Final package chunk could not be written: {e}"))?;
        self.inner
            .write_all(&0u32.to_le_bytes())
            .map_err(|e| format!("Package EOF marker could not be written: {e}"))?;
        self.inner
            .flush()
            .map_err(|e| format!("Package file could not be flushed: {e}"))?;
        self.inner
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|e| format!("Package metadata could not be read: {e}"))
    }

    fn write_encrypted_chunk(&mut self, plaintext: &[u8], is_final: bool) -> io::Result<()> {
        let nonce = payload_nonce(self.nonce_prefix, self.chunk_index);
        let aad = payload_aad(
            &self.package_id,
            self.chunk_index,
            &self.header_digest,
            is_final,
        );
        let ciphertext = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload encrypt failed"))?;
        if ciphertext.len() > u32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "encrypted chunk too large",
            ));
        }
        self.inner
            .write_all(&(ciphertext.len() as u32).to_le_bytes())?;
        self.inner.write_all(&ciphertext)?;
        self.chunk_index += 1;
        Ok(())
    }
}

impl Write for EncryptedPackageWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        while self.buffer.len() >= PACKAGE_CHUNK_SIZE {
            let chunk = self.buffer.drain(..PACKAGE_CHUNK_SIZE).collect::<Vec<_>>();
            self.write_encrypted_chunk(&chunk, false)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct EncryptedPackageReader<R: Read> {
    inner: R,
    cipher: Aes256Gcm,
    package_id: String,
    nonce_prefix: [u8; 4],
    header_digest: [u8; 32],
    chunk_index: u64,
    plain: Vec<u8>,
    plain_offset: usize,
    /// Look-ahead length prefix for the next chunk, if already consumed.
    pending_len: Option<u32>,
    /// True once an authenticated final chunk (AAD ":final") has been decrypted.
    saw_final: bool,
    eof: bool,
}

impl<R: Read> EncryptedPackageReader<R> {
    fn new(
        inner: R,
        package_id: String,
        data_key: Zeroizing<[u8; 32]>,
        nonce_prefix: [u8; 4],
        header_digest: [u8; 32],
    ) -> Result<Self, String> {
        let cipher = Aes256Gcm::new_from_slice(data_key.as_slice())
            .map_err(|_| "AES-256-GCM payload reader setup failed.".to_string())?;
        Ok(Self {
            inner,
            cipher,
            package_id,
            nonce_prefix,
            header_digest,
            chunk_index: 0,
            plain: Vec::new(),
            plain_offset: 0,
            pending_len: None,
            saw_final: false,
            eof: false,
        })
    }

    fn read_len_prefix(&mut self) -> io::Result<u32> {
        if let Some(len) = self.pending_len.take() {
            return Ok(len);
        }
        let mut len = [0u8; 4];
        self.inner.read_exact(&mut len)?;
        Ok(u32::from_le_bytes(len))
    }

    fn load_next_chunk(&mut self) -> io::Result<()> {
        if self.eof {
            return Ok(());
        }
        let len = self.read_len_prefix()?;
        if len == 0 {
            // A2: a bare 0u32 / clean EOF is only acceptable AFTER an
            // authenticated final-chunk marker has been seen. Reject otherwise
            // (truncation before the final chunk).
            if !self.saw_final {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "package truncated: end-of-stream before authenticated final chunk",
                ));
            }
            self.eof = true;
            self.plain.clear();
            self.plain_offset = 0;
            return Ok(());
        }
        if self.saw_final {
            // Trailing data after the authenticated final chunk is tampering.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "package has extra data after the authenticated final chunk",
            ));
        }
        let mut ciphertext = vec![0u8; len as usize];
        self.inner.read_exact(&mut ciphertext)?;

        // Peek the next length prefix to decide whether THIS chunk is the
        // authenticated final one (followed by the 0u32 EOF marker).
        let mut next_len = [0u8; 4];
        self.inner.read_exact(&mut next_len)?;
        let next_len = u32::from_le_bytes(next_len);
        let is_final = next_len == 0;
        if !is_final {
            self.pending_len = Some(next_len);
        }

        let nonce = payload_nonce(self.nonce_prefix, self.chunk_index);
        let aad = payload_aad(
            &self.package_id,
            self.chunk_index,
            &self.header_digest,
            is_final,
        );
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload decrypt failed"))?;
        self.chunk_index += 1;
        if is_final {
            self.saw_final = true;
            self.pending_len = Some(0);
        }
        self.plain = plaintext;
        self.plain_offset = 0;
        Ok(())
    }
}

impl<R: Read> Read for EncryptedPackageReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.plain_offset >= self.plain.len() {
            if self.eof {
                return Ok(0);
            }
            self.load_next_chunk()?;
        }
        let available = self.plain.len() - self.plain_offset;
        let to_copy = available.min(out.len());
        out[..to_copy].copy_from_slice(&self.plain[self.plain_offset..self.plain_offset + to_copy]);
        self.plain_offset += to_copy;
        Ok(to_copy)
    }
}

fn payload_nonce(prefix: [u8; 4], chunk_index: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(&prefix);
    nonce[4..].copy_from_slice(&chunk_index.to_le_bytes());
    nonce
}

fn workspace_package_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(WORKSPACE_DIR).join(PACKAGES_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create package dir: {e}"))?;
    Ok(dir)
}

fn workspace_import_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(WORKSPACE_DIR).join(IMPORTS_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create import dir: {e}"))?;
    Ok(dir)
}

fn workspace_download_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(WORKSPACE_DIR).join(DOWNLOADS_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create download dir: {e}"))?;
    Ok(dir)
}

/// Workspace root if configured, otherwise the app data dir — mirrors the
/// fallback the decrypt path uses so a collaborator with NO folder yet still has
/// a writable place to receive the package.
fn resolve_workspace_root_or_app_data(app: &tauri::AppHandle) -> PathBuf {
    resolve_workspace_root().unwrap_or_else(|_| {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("workspace")
    })
}

/// Derive a safe local filename from the URL's last path segment: keep only
/// `[A-Za-z0-9._-]`, drop leading dots (no hidden/`..` names), force a non-empty
/// stem, and ensure the `.aspiswspkg` extension. Path separators map to `_`, so
/// the result can never traverse out of the downloads dir.
fn sanitize_package_filename(url: &str) -> String {
    let raw = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let mut cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    while cleaned.starts_with('.') {
        cleaned.remove(0);
    }
    if cleaned.is_empty() {
        cleaned = "cloud-workspace".to_string();
    }
    if !cleaned.to_ascii_lowercase().ends_with(".aspiswspkg") {
        cleaned.push_str(".aspiswspkg");
    }
    cleaned
}

fn download_bootstrap_package(
    app: &tauri::AppHandle,
    url: &str,
    expected_sha256: Option<&str>,
) -> Result<WorkspacePackageInfo, String> {
    let url = url.trim();
    // HTTPS only: forbid http/file/ftp and any scheme-downgrade.
    if !url.starts_with("https://") {
        return Err("The package URL must use https://.".into());
    }
    // A caller-provided digest must be 64 hex chars if present.
    let expected = match expected_sha256.map(str::trim).filter(|s| !s.is_empty()) {
        Some(hex) => {
            let lower = hex.to_ascii_lowercase();
            if lower.len() != 64 || !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Expected SHA-256 must be 64 hex characters.".into());
            }
            Some(lower)
        }
        None => None,
    };

    let root = resolve_workspace_root_or_app_data(app);
    let download_dir = workspace_download_dir(&root)?;
    let file_name = sanitize_package_filename(url);
    let target = download_dir.join(&file_name);
    // Defense in depth: the sanitized name must resolve inside the downloads dir.
    assert_import_target_within(&download_dir, &target)?;

    // No redirects: a 30x to http:// (or an internal host) must not be followed.
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(PACKAGE_DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Could not build the download client: {e}"))?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("Download request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Download failed with HTTP {}.",
            response.status().as_u16()
        ));
    }
    // Reject an oversized body up front when the server advertises a length.
    if let Some(len) = response.content_length() {
        if len > PACKAGE_MAX_BYTES {
            return Err(format!(
                "Remote package is {} MB, over the {} MB ceiling.",
                len / 1024 / 1024,
                PACKAGE_MAX_BYTES / 1024 / 1024
            ));
        }
    }

    // Stream to a `.part` temp with a running size cap + hash, then rename.
    let tmp = download_dir.join(format!("{file_name}.part"));
    let mut out =
        fs::File::create(&tmp).map_err(|e| format!("Could not create download file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; PACKAGE_CHUNK_SIZE];
    let mut total: u64 = 0;
    loop {
        let read = response.read(&mut buf);
        let n = match read {
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(format!("Download stream error: {e}"));
            }
        };
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > PACKAGE_MAX_BYTES {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Remote package exceeds the {} MB ceiling; aborting.",
                PACKAGE_MAX_BYTES / 1024 / 1024
            ));
        }
        hasher.update(&buf[..n]);
        if let Err(e) = out.write_all(&buf[..n]) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("Could not write download: {e}"));
        }
    }
    out.flush().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not flush download: {e}")
    })?;
    drop(out);

    if total == 0 {
        let _ = fs::remove_file(&tmp);
        return Err("The downloaded package is empty.".into());
    }
    if let Some(want) = expected {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(&want) {
            let _ = fs::remove_file(&tmp);
            return Err("Downloaded package SHA-256 does not match the expected digest.".into());
        }
    }
    if target.exists() {
        let _ = fs::remove_file(&target);
    }
    fs::rename(&tmp, &target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Could not finalize the download: {e}")
    })?;

    let metadata =
        fs::metadata(&target).map_err(|e| format!("Could not stat the download: {e}"))?;
    Ok(WorkspacePackageInfo {
        path: target.to_string_lossy().into_owned(),
        file_name,
        size_mb: metadata.len() as f64 / 1024.0 / 1024.0,
        created_at: None,
    })
}

fn latest_workspace_packages(package_dir: &Path) -> Result<Vec<WorkspacePackageInfo>, String> {
    let mut packages = Vec::new();
    for entry in fs::read_dir(package_dir).map_err(|e| format!("Package dir read failed: {e}"))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("aspiswspkg") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        packages.push(WorkspacePackageInfo {
            file_name: entry.file_name().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            size_mb: round2(metadata.len() as f64 / 1024f64.powi(2)),
            created_at: metadata.modified().ok().map(system_time_iso),
        });
    }
    packages.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    packages.truncate(6);
    Ok(packages)
}

fn collect_package_candidates(root: &Path) -> Result<PackageCandidateSet, String> {
    let policy = IgnorePolicy::load(root);
    let always_excluded = ALWAYS_EXCLUDED_DIRS
        .iter()
        .map(|value| (*value).to_string())
        .collect::<HashSet<_>>();
    let mut set = PackageCandidateSet {
        files: Vec::new(),
        total_bytes: 0,
        skipped_files: 0,
        skipped_bytes: 0,
        warnings: Vec::new(),
    };
    let mut stack = vec![root.to_path_buf()];
    let mut visited = visited_with(root);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            let relative = relative_path(root, &path)
                .replace('\\', "/")
                .to_ascii_lowercase();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_or_symlink(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                if should_skip_package_dir(&relative, &name, &policy, &always_excluded) {
                    continue;
                }
                push_unseen_dir(&mut stack, &mut visited, path);
            } else if metadata.is_file() {
                if should_skip_package_file(&relative, &name, &path, &policy) {
                    set.skipped_files += 1;
                    set.skipped_bytes = set.skipped_bytes.saturating_add(metadata.len());
                    continue;
                }
                if metadata.len() > PACKAGE_MAX_FILE_BYTES {
                    set.skipped_files += 1;
                    set.skipped_bytes = set.skipped_bytes.saturating_add(metadata.len());
                    set.warnings.push(format!(
                        "Skipped large file over {} MB: {}",
                        PACKAGE_MAX_FILE_BYTES / 1024 / 1024,
                        relative_path(root, &path)
                    ));
                    continue;
                }
                set.total_bytes = set.total_bytes.saturating_add(metadata.len());
                set.files.push(PackageCandidate {
                    path,
                    relative_path: relative_path(root, &entry.path()).replace('\\', "/"),
                    bytes: metadata.len(),
                });
            }
        }
    }
    set.files
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(set)
}

fn should_skip_package_dir(
    relative: &str,
    name: &str,
    policy: &IgnorePolicy,
    always_excluded: &HashSet<String>,
) -> bool {
    matches!(
        relative,
        "_workspace/packages"
            | "_workspace/imports"
            | "_workspace/inventory"
            | "_workspace/quarantine"
    ) || relative.starts_with("_workspace/packages/")
        || relative.starts_with("_workspace/imports/")
        || relative.starts_with("_workspace/inventory/")
        || relative.starts_with("_workspace/quarantine/")
        || always_excluded.contains(name)
        || matches!(
            name,
            ".secrets" | "secrets" | "aspis-secrets" | ".wrangler" | ".git"
        )
        || policy.matches_dir(relative, name)
}

fn should_skip_package_file(
    relative: &str,
    name: &str,
    path: &Path,
    policy: &IgnorePolicy,
) -> bool {
    if policy.matches_file(relative) || is_sensitive_file_name(name) {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }
    !matches!(
        name,
        ".gitignore"
            | ".aspisignore"
            | ".oracleignore"
            | "dockerfile"
            | "makefile"
            | "license"
            | "cargo.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
    )
}

fn is_sensitive_file_name(name: &str) -> bool {
    name == ".env"
        || name.starts_with(".env.")
        || name == ".dev.vars"
        || name == "credentials.json"
        || name == "token.txt"
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
        || name.ends_with(".jks")
        || name.ends_with(".keystore")
}

fn append_bytes_to_tar<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header
        .set_path(path)
        .map_err(|e| format!("Tar path failed: {e}"))?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append(&header, bytes)
        .map_err(|e| format!("Tar README append failed: {e}"))
}

fn append_file_to_tar<W: Write>(
    builder: &mut tar::Builder<W>,
    candidate: &PackageCandidate,
) -> Result<(), String> {
    let mut file = fs::File::open(&candidate.path).map_err(|e| {
        format!(
            "Could not open package candidate {}: {e}",
            candidate.path.display()
        )
    })?;
    let mut header = tar::Header::new_gnu();
    header
        .set_path(&candidate.relative_path)
        .map_err(|e| format!("Tar path failed for {}: {e}", candidate.relative_path))?;
    header.set_size(candidate.bytes);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append(&header, &mut file)
        .map_err(|e| format!("Tar append failed for {}: {e}", candidate.relative_path))
}

fn safe_unpack_tar<R: Read>(
    reader: R,
    output_dir: &Path,
) -> Result<(u64, u64, Vec<String>), String> {
    match safe_unpack_tar_inner(reader, output_dir) {
        Ok(result) => Ok(result),
        Err(e) => {
            // A4: clean up any partially restored output on a breach.
            let _ = fs::remove_dir_all(output_dir);
            Err(e)
        }
    }
}

fn safe_unpack_tar_inner<R: Read>(
    reader: R,
    output_dir: &Path,
) -> Result<(u64, u64, Vec<String>), String> {
    let mut archive = tar::Archive::new(reader);
    let mut files_restored = 0u64;
    let mut bytes_restored = 0u64;
    let mut entry_count = 0u64;
    let mut warnings = Vec::new();
    let entries = archive
        .entries()
        .map_err(|e| format!("Package tar entries could not be read: {e}"))?;
    for entry in entries {
        // A4: cap the total number of entries.
        entry_count += 1;
        if entry_count > PACKAGE_MAX_ENTRIES {
            return Err(format!(
                "Package exceeds the {PACKAGE_MAX_ENTRIES} entry cap; aborting decrypt."
            ));
        }
        let mut entry = entry.map_err(|e| format!("Package tar entry failed: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Package tar path failed: {e}"))?;
        let relative = safe_tar_relative_path(&path)?;
        let target = output_dir.join(&relative);
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|e| format!("Could not create package dir {}: {e}", target.display()))?;
            continue;
        }
        if !entry_type.is_file() {
            warnings.push(format!(
                "Skipped non-file tar entry: {}",
                relative.display()
            ));
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("Could not create package parent {}: {e}", parent.display())
            })?;
        }
        let mut out = fs::File::create(&target)
            .map_err(|e| format!("Could not restore {}: {e}", target.display()))?;
        // A4: cap per-file bytes with a capped reader, and check the cumulative
        // ceiling before/after each entry so a lying tar header cannot blow past
        // the limit on decrypt.
        let remaining_budget = PACKAGE_MAX_BYTES.saturating_sub(bytes_restored);
        if remaining_budget == 0 {
            return Err(format!(
                "Package exceeds the {} MB cumulative decrypt ceiling; aborting.",
                PACKAGE_MAX_BYTES / 1024 / 1024
            ));
        }
        let per_file_cap = PACKAGE_MAX_FILE_BYTES.min(remaining_budget);
        // Read one extra byte over the cap to detect overflow without writing it.
        let mut capped = entry.by_ref().take(per_file_cap.saturating_add(1));
        let copied = io::copy(&mut capped, &mut out)
            .map_err(|e| format!("Could not write {}: {e}", target.display()))?;
        if copied > per_file_cap {
            if copied > remaining_budget {
                return Err(format!(
                    "Package exceeds the {} MB cumulative decrypt ceiling; aborting.",
                    PACKAGE_MAX_BYTES / 1024 / 1024
                ));
            }
            return Err(format!(
                "Package file {} exceeds the {} MB per-file decrypt ceiling; aborting.",
                relative.display(),
                PACKAGE_MAX_FILE_BYTES / 1024 / 1024
            ));
        }
        files_restored += 1;
        bytes_restored = bytes_restored.saturating_add(copied);
    }
    Ok((files_restored, bytes_restored, warnings))
}

fn safe_tar_relative_path(path: &Path) -> Result<PathBuf, String> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Package contains an unsafe path.".into());
            }
        }
    }
    if clean.as_os_str().is_empty() {
        return Err("Package contains an empty path.".into());
    }
    Ok(clean)
}

fn package_readme(
    root: &Path,
    candidates: &PackageCandidateSet,
    recipients: &[WorkspacePackageRecipient],
    created_at: &str,
) -> String {
    let mut out = String::new();
    out.push_str("# Aspis Bio Workspace Bootstrap\n\n");
    out.push_str("This package contains source files, docs, tests and small configuration files selected by Aspis Management.\n\n");
    out.push_str("## Security\n\n");
    out.push_str("- Bulk data is encrypted with AES-256-GCM.\n");
    out.push_str(
        "- The package key is wrapped for each approved device with X25519 + HKDF-SHA256.\n",
    );
    out.push_str("- Secrets, raw data, dependency caches, model binaries, agent logs and build outputs are excluded by policy.\n");
    out.push_str("- A cloud drive may store this file, but only approved devices should be able to decrypt it.\n\n");
    out.push_str("## How to Use\n\n");
    out.push_str("1. Download the `.aspiswspkg` file locally.\n");
    out.push_str("2. Open Aspis Management on the approved device.\n");
    out.push_str(
        "3. Use Workspace Bootstrap / Decrypt package and paste or select the package path.\n",
    );
    out.push_str("4. Work from the restored folder and clone GitHub repos normally for active development.\n\n");
    out.push_str("## Package Summary\n\n");
    out.push_str(&format!("- Created: {created_at}\n"));
    out.push_str(&format!("- Source root: {}\n", root.display()));
    out.push_str(&format!("- Files: {}\n", candidates.files.len()));
    out.push_str(&format!(
        "- Plaintext selected size: {:.2} MB\n",
        candidates.total_bytes as f64 / 1024f64.powi(2)
    ));
    out.push_str(&format!("- Recipients: {}\n", recipients.len()));
    out.push_str(&format!(
        "- Skipped files: {}\n\n",
        candidates.skipped_files
    ));
    out.push_str("## Approved Device Fingerprints\n\n");
    for recipient in recipients {
        out.push_str(&format!(
            "- {} / {} / {} / {}\n",
            recipient.fingerprint,
            recipient.collaborator_name,
            recipient.device_name,
            recipient.platform
        ));
    }
    out.push_str("\n## Notes\n\n");
    out.push_str("This bootstrap is not a Git replacement. Use it only for first setup and context transfer. Future code work should happen through GitHub branches and pull requests.\n");
    out
}

fn resolve_workspace_root() -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var("ASPIS_WORKSPACE_ROOT") {
        let path = PathBuf::from(root);
        if path.is_dir() {
            return path
                .canonicalize()
                .map_err(|_| "Workspace root could not be resolved.".to_string());
        }
    }
    if let Ok(preferences) = vault::read_oracle_index_preferences() {
        if let Some(root) = preferences.index_root {
            let path = PathBuf::from(root);
            if path.is_dir() {
                return path
                    .canonicalize()
                    .map_err(|_| "Workspace root could not be resolved.".to_string());
            }
        }
    }
    // `USERPROFILE` is Windows-only; macOS/Linux use `HOME`. Try both, and the
    // common case variants of the folder name.
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        for name in ["aspis bio", "Aspis Bio", "aspis-bio"] {
            let path = home.join("Desktop").join(name);
            if path.is_dir() {
                return path
                    .canonicalize()
                    .map_err(|_| "Workspace root could not be resolved.".to_string());
            }
        }
    }
    Err("Aspis Bio workspace root is not configured. Set Oracle index root first.".into())
}

fn snapshot_from_reports(
    root: PathBuf,
    scan_ran: bool,
) -> Result<WorkspaceHygieneSnapshot, String> {
    let workspace_dir = root.join(WORKSPACE_DIR);
    let inventory_dir = workspace_dir.join(INVENTORY_DIR);
    let manifests_dir = workspace_dir.join(MANIFESTS_DIR);
    let top_level_path = inventory_dir.join(TOP_LEVEL_CSV);
    let large_files_path = inventory_dir.join(LARGE_FILES_CSV);
    let git_repos_path = inventory_dir.join(GIT_REPOS_CSV);
    let classification_path = manifests_dir.join(CLASSIFICATION_CSV);
    let needs_scan =
        !top_level_path.is_file() || !large_files_path.is_file() || !git_repos_path.is_file();
    let mut warnings = Vec::new();
    let top_level = read_top_level_csv(&top_level_path).unwrap_or_else(|e| {
        warnings.push(format!("Top-level inventory could not be read: {e}"));
        Vec::new()
    });
    let large_files = read_large_files_csv(&large_files_path, &root).unwrap_or_else(|e| {
        warnings.push(format!("Large-file inventory could not be read: {e}"));
        Vec::new()
    });
    let git_repos = read_git_repos_csv(&git_repos_path, &root).unwrap_or_else(|e| {
        warnings.push(format!("Git repo inventory could not be read: {e}"));
        Vec::new()
    });
    let classifications = read_classification_csv(&classification_path).unwrap_or_else(|e| {
        warnings.push(format!("Workspace classification could not be read: {e}"));
        Vec::new()
    });
    let report_read_failed = !warnings.is_empty();
    let policy_files = workspace_policy_files(&root);
    let total_size_gb = round2(top_level.iter().map(|entry| entry.size_gb).sum());
    let total_files = top_level.iter().map(|entry| entry.file_count).sum();
    let oracle_candidate_files = count_oracle_candidate_files(&root).unwrap_or(0);
    if needs_scan && !scan_ran {
        warnings.push("Workspace inventory is missing or incomplete. Run Scan workspace.".into());
    }
    if git_repos.iter().any(|repo| repo.dirty_count > 0) {
        warnings.push("One or more code repositories have uncommitted local changes.".into());
    }
    if policy_files.iter().any(|policy| !policy.exists) {
        warnings.push("One or more workspace policy files are missing.".into());
    }
    if oracle_candidate_files == 0 {
        warnings.push("Oracle candidate count is zero. Check .oracleignore and index root.".into());
    }

    Ok(WorkspaceHygieneSnapshot {
        root: root.to_string_lossy().into_owned(),
        workspace_dir: workspace_dir.to_string_lossy().into_owned(),
        scanned_at: now(),
        needs_scan: needs_scan || report_read_failed,
        total_size_gb,
        total_files,
        oracle_candidate_files,
        top_level,
        large_files,
        git_repos,
        classifications,
        policy_files,
        warnings,
    })
}

fn write_workspace_reports(root: &Path) -> Result<(), String> {
    let workspace_dir = root.join(WORKSPACE_DIR);
    let inventory_dir = workspace_dir.join(INVENTORY_DIR);
    let manifests_dir = workspace_dir.join(MANIFESTS_DIR);
    fs::create_dir_all(&inventory_dir)
        .map_err(|e| format!("Could not create inventory folder: {e}"))?;
    fs::create_dir_all(&manifests_dir)
        .map_err(|e| format!("Could not create manifest folder: {e}"))?;

    let top_level = scan_top_level(root)?;
    write_csv(
        &inventory_dir.join(TOP_LEVEL_CSV),
        &["Name", "Type", "SizeGB", "FileCount", "LastWrite", "Path"],
        top_level.iter().map(|entry| {
            vec![
                entry.name.clone(),
                entry.entry_type.clone(),
                format!("{:.4}", entry.size_gb),
                entry.file_count.to_string(),
                entry.last_write.clone().unwrap_or_default(),
                entry.path.clone(),
            ]
        }),
    )?;

    let large_files = scan_large_files(root)?;
    write_csv(
        &inventory_dir.join(LARGE_FILES_CSV),
        &["SizeGB", "SizeMB", "Path", "LastWriteTime"],
        large_files.iter().map(|entry| {
            vec![
                format!("{:.4}", entry.size_gb),
                format!("{:.2}", entry.size_mb),
                entry.path.clone(),
                entry.last_write.clone().unwrap_or_default(),
            ]
        }),
    )?;

    let git_repos = scan_git_repos(root)?;
    write_csv(
        &inventory_dir.join(GIT_REPOS_CSV),
        &["Path", "Name", "Branch", "Origin", "DirtyCount", "GitSize"],
        git_repos.iter().map(|repo| {
            vec![
                repo.path.clone(),
                repo.name.clone(),
                repo.branch.clone(),
                repo.origin.clone().unwrap_or_default(),
                repo.dirty_count.to_string(),
                repo.git_size.clone().unwrap_or_default(),
            ]
        }),
    )?;

    if !manifests_dir.join(CLASSIFICATION_CSV).is_file() {
        write_default_classification(&manifests_dir.join(CLASSIFICATION_CSV))?;
    }
    Ok(())
}

fn scan_top_level(root: &Path) -> Result<Vec<WorkspaceSizeEntry>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| format!("Could not read workspace root: {e}"))? {
        let entry = entry.map_err(|e| format!("Could not read workspace entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == WORKSPACE_DIR {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("Could not read metadata for {}: {e}", path.display()))?;
        if is_reparse_or_symlink(&metadata) {
            continue;
        }
        let (bytes, file_count) = if metadata.is_dir() {
            directory_size(&path)
        } else if metadata.is_file() {
            (metadata.len(), 1)
        } else {
            (0, 0)
        };
        entries.push(WorkspaceSizeEntry {
            name,
            entry_type: if metadata.is_dir() { "dir" } else { "file" }.into(),
            path: path.to_string_lossy().into_owned(),
            size_gb: round4(bytes as f64 / 1024f64.powi(3)),
            file_count,
            last_write: metadata.modified().ok().map(system_time_iso),
        });
    }
    entries.sort_by(|a, b| {
        b.size_gb
            .partial_cmp(&a.size_gb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

fn scan_large_files(root: &Path) -> Result<Vec<WorkspaceLargeFile>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = visited_with(root);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if is_workspace_control_path(root, &path) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_or_symlink(&metadata) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if metadata.is_dir() {
                if should_skip_common_scan_dir(&name) {
                    continue;
                }
                push_unseen_dir(&mut stack, &mut visited, path);
            } else if metadata.is_file() && metadata.len() >= LARGE_FILE_THRESHOLD_BYTES {
                let relative = relative_path(root, &path);
                files.push(WorkspaceLargeFile {
                    relative_path: relative.clone(),
                    class_label: classify_path(root, &path),
                    path: relative,
                    size_gb: round4(metadata.len() as f64 / 1024f64.powi(3)),
                    size_mb: round2(metadata.len() as f64 / 1024f64.powi(2)),
                    last_write: metadata.modified().ok().map(system_time_iso),
                });
            }
        }
    }
    files.sort_by(|a, b| {
        b.size_gb
            .partial_cmp(&a.size_gb)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    files.truncate(LARGE_FILE_LIMIT);
    Ok(files)
}

fn scan_git_repos(root: &Path) -> Result<Vec<WorkspaceGitRepoStatus>, String> {
    let mut repos = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = visited_with(root);
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        if should_skip_git_scan_dir(root, &dir) {
            continue;
        }
        if dir.join(".git").exists() && dir != root {
            repos.push(git_repo_status(root, &dir));
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_or_symlink(&metadata) {
                continue;
            }
            if metadata.is_dir() && mark_dir_seen(&mut visited, &path) {
                stack.push((path, depth + 1));
            }
        }
    }
    repos.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(repos)
}

fn git_repo_status(root: &Path, path: &Path) -> WorkspaceGitRepoStatus {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("repo")
        .to_string();
    let branch =
        git_output(path, &["branch", "--show-current"]).unwrap_or_else(|| "unknown".into());
    let origin =
        git_output(path, &["remote", "get-url", "origin"]).map(|value| sanitize_remote_url(&value));
    let dirty_count = git_output(path, &["status", "--porcelain=v1"])
        .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count() as u32)
        .unwrap_or(0);
    let git_size = git_output(path, &["count-objects", "-vH"]).and_then(|raw| {
        raw.lines().find_map(|line| {
            line.strip_prefix("size-pack:")
                .map(|value| value.trim().to_string())
        })
    });
    let mut warnings = Vec::new();
    if dirty_count > 0 {
        warnings.push(format!("{dirty_count} local change(s) not committed."));
    }
    if origin.is_none() {
        warnings.push("No origin remote configured.".into());
    }
    if branch != "main" && !branch.starts_with("feature/") && !branch.starts_with("wb-") {
        warnings.push("Branch name is not a normal main/feature/work branch.".into());
    }
    let clone_command = origin
        .as_ref()
        .map(|remote| format!("git clone {}", remote.trim_end_matches(".git")));
    WorkspaceGitRepoStatus {
        name,
        path: path.to_string_lossy().into_owned(),
        relative_path: relative_path(root, path),
        branch,
        origin,
        dirty_count,
        git_size,
        clone_command,
        warnings,
    }
}

fn read_top_level_csv(path: &Path) -> Result<Vec<WorkspaceSizeEntry>, String> {
    Ok(parse_csv(path)?
        .into_iter()
        .filter_map(|record| {
            Some(WorkspaceSizeEntry {
                name: field(&record, "Name")?.to_string(),
                entry_type: field(&record, "Type").unwrap_or("").to_string(),
                size_gb: field(&record, "SizeGB")
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0.0),
                file_count: field(&record, "FileCount")
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0),
                last_write: clean_field(field(&record, "LastWrite")),
                path: field(&record, "Path").unwrap_or("").to_string(),
            })
        })
        .collect())
}

fn read_large_files_csv(path: &Path, root: &Path) -> Result<Vec<WorkspaceLargeFile>, String> {
    Ok(parse_csv(path)?
        .into_iter()
        .filter_map(|record| {
            let stored_path = field(&record, "Path")
                .or_else(|| field(&record, "FullName"))?
                .to_string();
            let path = PathBuf::from(&stored_path);
            let relative = if path.is_absolute() {
                relative_path(root, &path)
            } else {
                stored_path.clone()
            };
            Some(WorkspaceLargeFile {
                relative_path: relative.clone(),
                class_label: classify_path(root, &path),
                path: relative,
                size_gb: field(&record, "SizeGB")
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0.0),
                size_mb: field(&record, "SizeMB")
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0.0),
                last_write: clean_field(field(&record, "LastWriteTime")),
            })
        })
        .collect())
}

fn read_git_repos_csv(path: &Path, root: &Path) -> Result<Vec<WorkspaceGitRepoStatus>, String> {
    Ok(parse_csv(path)?
        .into_iter()
        .filter_map(|record| {
            let full = field(&record, "Path")?.to_string();
            let repo_path = PathBuf::from(&full);
            let origin =
                clean_field(field(&record, "Origin")).map(|value| sanitize_remote_url(&value));
            let dirty_count = field(&record, "DirtyCount")
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            let mut warnings = Vec::new();
            if dirty_count > 0 {
                warnings.push(format!("{dirty_count} local change(s) not committed."));
            }
            if origin.is_none() {
                warnings.push("No origin remote configured.".into());
            }
            Some(WorkspaceGitRepoStatus {
                name: field(&record, "Name").unwrap_or("").to_string(),
                path: full,
                relative_path: relative_path(root, &repo_path),
                branch: field(&record, "Branch").unwrap_or("unknown").to_string(),
                clone_command: origin
                    .as_ref()
                    .map(|remote| format!("git clone {}", remote.trim_end_matches(".git"))),
                origin,
                dirty_count,
                git_size: clean_field(field(&record, "GitSize")),
                warnings,
            })
        })
        .collect())
}

fn read_classification_csv(path: &Path) -> Result<Vec<WorkspaceClassificationEntry>, String> {
    Ok(parse_csv(path)?
        .into_iter()
        .filter_map(|record| {
            Some(WorkspaceClassificationEntry {
                path: field(&record, "Path")?.to_string(),
                class_label: field(&record, "Class").unwrap_or("UNKNOWN").to_string(),
                git: field(&record, "Git").unwrap_or("").to_string(),
                oracle: field(&record, "Oracle").unwrap_or("").to_string(),
                storage: field(&record, "Storage").unwrap_or("").to_string(),
                notes: field(&record, "Notes").unwrap_or("").to_string(),
            })
        })
        .collect())
}

fn workspace_policy_files(root: &Path) -> Vec<WorkspacePolicyFile> {
    [".aspisignore", ".oracleignore", "ASPIS_WORKSPACE.md"]
        .iter()
        .map(|name| {
            let path = root.join(name);
            let content = fs::read_to_string(&path).unwrap_or_default();
            let line_count = content.lines().count() as u32;
            let active_rules = content
                .lines()
                .filter(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with('#')
                })
                .count() as u32;
            WorkspacePolicyFile {
                name: (*name).into(),
                path: path.to_string_lossy().into_owned(),
                exists: path.is_file(),
                line_count,
                active_rules,
            }
        })
        .collect()
}

fn count_oracle_candidate_files(root: &Path) -> Result<u64, String> {
    let policy = IgnorePolicy::load(root);
    let always_excluded = ALWAYS_EXCLUDED_DIRS
        .iter()
        .map(|value| (*value).to_string())
        .collect::<HashSet<_>>();
    let text_extensions = TEXT_EXTENSIONS
        .iter()
        .map(|value| (*value).to_string())
        .collect::<HashSet<_>>();
    let mut count = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut visited = visited_with(root);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            let relative = relative_path(root, &path)
                .replace('\\', "/")
                .to_ascii_lowercase();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if is_reparse_or_symlink(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                if is_oracle_control_noise_path(&relative)
                    || always_excluded.contains(&name)
                    || policy.matches_dir(&relative, &name)
                {
                    continue;
                }
                push_unseen_dir(&mut stack, &mut visited, path);
            } else if metadata.is_file() {
                if policy.matches_file(&relative) {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if text_extensions.contains(&ext) {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

#[derive(Default)]
struct IgnorePolicy {
    directory_names: HashSet<String>,
    directory_name_prefixes: Vec<String>,
    file_names: HashSet<String>,
    file_name_prefixes: Vec<String>,
    exact_paths: HashSet<String>,
    prefixes: Vec<String>,
    suffixes: Vec<String>,
}

impl IgnorePolicy {
    fn load(root: &Path) -> Self {
        let mut policy = Self::default();
        for file_name in [".oracleignore", ".aspisignore"] {
            let path = root.join(file_name);
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            for raw_line in content.lines() {
                let line = raw_line.trim().replace('\\', "/");
                if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                    continue;
                }
                policy.add_pattern(&line);
            }
        }
        policy.directory_names.retain(|value| !value.is_empty());
        policy.file_names.retain(|value| !value.is_empty());
        policy
    }

    fn add_pattern(&mut self, raw: &str) {
        let is_dir_pattern = raw.ends_with('/');
        let lower = raw
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if lower.is_empty() {
            return;
        }

        let portable = lower.strip_prefix("**/").unwrap_or(&lower);
        if let Some(suffix) = portable.strip_prefix("*.") {
            if !suffix.contains('/') {
                self.suffixes.push(format!(".{suffix}"));
                return;
            }
        }
        if let Some(prefix) = portable.strip_suffix(".*") {
            if !prefix.contains('/') {
                self.file_name_prefixes.push(format!("{prefix}."));
                return;
            }
        }
        if let Some(prefix) = portable.strip_suffix("-*") {
            if !prefix.contains('/') {
                if is_dir_pattern {
                    self.directory_name_prefixes.push(format!("{prefix}-"));
                } else {
                    self.file_name_prefixes.push(format!("{prefix}-"));
                }
                return;
            }
        }

        if is_dir_pattern {
            if !portable.contains('*') && !portable.contains('/') {
                self.directory_names.insert(portable.to_string());
            } else if !lower.contains('*') {
                self.prefixes.push(format!("{lower}/"));
            } else if let Some(name) = portable.rsplit('/').next() {
                let name = name.replace('*', "");
                if !name.is_empty() {
                    self.directory_names.insert(name);
                }
            }
            return;
        }

        if !portable.contains('*') && !portable.contains('/') {
            self.file_names.insert(portable.to_string());
            self.directory_names.insert(portable.to_string());
        } else if !lower.contains('*') {
            self.exact_paths.insert(lower);
        } else if let Some(name) = portable.rsplit('/').next() {
            let name = name.replace('*', "");
            if !name.is_empty() {
                self.file_name_prefixes.push(name);
            }
        }
    }

    fn matches_dir(&self, relative: &str, name: &str) -> bool {
        self.directory_names.contains(name)
            || self.exact_paths.contains(relative)
            || self
                .directory_name_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
            || self
                .prefixes
                .iter()
                .any(|prefix| format!("{relative}/").starts_with(prefix))
    }

    fn matches_file(&self, relative: &str) -> bool {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        self.suffixes
            .iter()
            .any(|suffix| relative.ends_with(suffix))
            || self.file_names.contains(name)
            || self.exact_paths.contains(relative)
            || self
                .file_name_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
            || self
                .prefixes
                .iter()
                .any(|prefix| relative.starts_with(prefix))
    }
}

fn parse_csv(path: &Path) -> Result<Vec<Vec<(String, String)>>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut rows = content.lines().map(parse_csv_line).collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let headers = rows.remove(0);
    Ok(rows
        .into_iter()
        .map(|row| {
            headers
                .iter()
                .cloned()
                .zip(row.into_iter().chain(std::iter::repeat(String::new())))
                .take(headers.len())
                .collect::<Vec<_>>()
        })
        .collect())
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn field<'a>(record: &'a [(String, String)], name: &str) -> Option<&'a str> {
    record
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn clean_field(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn write_default_classification(path: &Path) -> Result<(), String> {
    let rows = [
        [
            "aspis-lab",
            "CODE_REPO",
            "yes",
            "include_code_docs_tests",
            "github",
            "Main app/cloudflare/mobile repo.",
        ],
        [
            "aspis-biovision",
            "CODE_REPO_WITH_LOCAL_DATA",
            "yes",
            "include_code_docs_tests",
            "github_for_code_external_for_data",
            "Biovision/Python repo with heavy local data.",
        ],
        [
            "aspis-lab/cloudflare/Aspis-bio-website",
            "CODE_REPO",
            "yes",
            "include_code_docs_tests",
            "github",
            "Website repo.",
        ],
        [
            "aspis-biovision/data",
            "DATA_RAW_MODELS",
            "no",
            "manifest_summary_only",
            "object_storage_or_sync_drive",
            "Raw WB/Zenodo/model artifacts.",
        ],
        [
            "aspis-lab/.rnaseq-reference-cache",
            "DATA_REFERENCE_CACHE",
            "no",
            "manifest_summary_only",
            "object_storage_or_regenerate",
            "RNA-seq references and indexes.",
        ],
        [
            ".secrets",
            "SECRET",
            "never",
            "never",
            "vault_only",
            "Do not sync or index.",
        ],
    ];
    write_csv(
        path,
        &["Path", "Class", "Git", "Oracle", "Storage", "Notes"],
        rows.into_iter()
            .map(|row| row.into_iter().map(String::from).collect()),
    )
}

fn write_csv<I>(path: &Path, headers: &[&str], rows: I) -> Result<(), String>
where
    I: IntoIterator<Item = Vec<String>>,
{
    let mut out = String::new();
    out.push_str(
        &headers
            .iter()
            .map(|value| csv_escape(value))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for row in rows {
        out.push_str(
            &row.iter()
                .map(|value| csv_escape(value))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    fs::write(path, out).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn csv_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn directory_size(path: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![path.to_path_buf()];
    let mut visited = visited_with(path);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_reparse_or_symlink(&metadata) {
                continue;
            }
            if metadata.is_dir() {
                push_unseen_dir(&mut stack, &mut visited, path);
            } else if metadata.is_file() {
                bytes = bytes.saturating_add(metadata.len());
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn visited_with(path: &Path) -> HashSet<PathBuf> {
    let mut visited = HashSet::new();
    if let Ok(canonical) = path.canonicalize() {
        visited.insert(canonical);
    }
    visited
}

fn push_unseen_dir(stack: &mut Vec<PathBuf>, visited: &mut HashSet<PathBuf>, path: PathBuf) {
    if mark_dir_seen(visited, &path) {
        stack.push(path);
    }
}

fn mark_dir_seen(visited: &mut HashSet<PathBuf>, path: &Path) -> bool {
    match path.canonicalize() {
        Ok(canonical) => visited.insert(canonical),
        Err(_) => true,
    }
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn should_skip_common_scan_dir(name: &str) -> bool {
    ALWAYS_EXCLUDED_DIRS.contains(&name)
}

fn should_skip_git_scan_dir(root: &Path, path: &Path) -> bool {
    let relative = relative_path(root, path)
        .replace('\\', "/")
        .to_ascii_lowercase();
    relative.split('/').any(|segment| {
        matches!(
            segment,
            WORKSPACE_DIR
                | ".claude"
                | ".codex"
                | ".deepseek"
                | ".agents"
                | "node_modules"
                | ".venv"
        ) || segment.starts_with(".gradle-home")
    })
}

fn is_oracle_control_noise_path(relative: &str) -> bool {
    relative == "_workspace/inventory"
        || relative.starts_with("_workspace/inventory/")
        || relative == "_workspace/quarantine"
        || relative.starts_with("_workspace/quarantine/")
}

fn is_workspace_control_path(root: &Path, path: &Path) -> bool {
    let relative = relative_path(root, path)
        .replace('\\', "/")
        .to_ascii_lowercase();
    relative == WORKSPACE_DIR || relative.starts_with("_workspace/")
}

fn git_output(path: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW so this git probe never flashes a console window in the
        // release GUI exe (it runs on workspace scans).
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().ok()?;
    let started_at = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            let output = child.wait_with_output().ok()?;
            if !output.status.success() {
                return None;
            }
            return Some(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
        if started_at.elapsed() >= Duration::from_secs(3) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn sanitize_remote_url(value: &str) -> String {
    if let Some((scheme, rest)) = value.split_once("://") {
        if let Some(at) = rest.find('@') {
            return format!("{scheme}://{}", &rest[at + 1..]);
        }
    }
    value.to_string()
}

fn classify_path(root: &Path, path: &Path) -> String {
    let rel = relative_path(root, path)
        .replace('\\', "/")
        .to_ascii_lowercase();
    if rel
        .split('/')
        .any(|segment| segment == ".venv" || segment == "node_modules")
    {
        "DEPENDENCY_CACHE".into()
    } else if rel.split('/').any(|segment| {
        segment.starts_with(".gradle-home") || segment == "android" || segment == "build"
    }) {
        "BUILD_CACHE".into()
    } else if rel.split('/').any(|segment| {
        segment == "data" || segment == "reference-imports" || segment == ".rnaseq-reference-cache"
    }) {
        "HEAVY_DATA".into()
    } else if rel.ends_with(".onnx") || rel.ends_with(".pt") || rel.ends_with(".safetensors") {
        "MODEL_ARTIFACT".into()
    } else if rel.ends_with(".zip")
        || rel.ends_with(".gz")
        || rel.ends_with(".tif")
        || rel.ends_with(".ome.tif")
    {
        "BINARY_DATA".into()
    } else {
        "LARGE_FILE".into()
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .trim_start_matches(['\\', '/'])
        .to_string()
}

fn system_time_iso(value: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(value).to_rfc3339()
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "aspis-workspace-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temp workspace root");
        root
    }

    fn write_text(path: &Path, value: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("test parent directory");
        }
        fs::write(path, value).expect("test file");
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn init_git_repo(path: &Path, origin: &str) {
        fs::create_dir_all(path).expect("git repo directory");
        assert!(
            Command::new("git")
                .arg("init")
                .current_dir(path)
                .status()
                .expect("git init")
                .success(),
            "git init failed in {}",
            path.display()
        );
        let _ = Command::new("git")
            .args(["remote", "add", "origin", origin])
            .current_dir(path)
            .status();
    }

    #[test]
    fn git_scan_skips_agent_roots() {
        if !git_available() {
            return;
        }
        let root = temp_root("git-scan");
        init_git_repo(
            &root.join("aspis-lab"),
            "https://github.com/Saurias92/aspis-lab.git",
        );
        init_git_repo(
            &root.join(".claude").join("worktrees").join("shadow"),
            "https://github.com/Saurias92/shadow.git",
        );
        init_git_repo(
            &root.join("nested").join(".codex").join("scratch"),
            "https://github.com/Saurias92/scratch.git",
        );

        let repos = scan_git_repos(&root).expect("git repo scan");
        let relative_paths = repos
            .iter()
            .map(|repo| repo.relative_path.replace('\\', "/"))
            .collect::<Vec<_>>();

        let _ = fs::remove_dir_all(&root);

        assert!(relative_paths.iter().any(|path| path == "aspis-lab"));
        assert!(
            !relative_paths
                .iter()
                .any(|path| path.contains(".claude") || path.contains(".codex")),
            "agent scratch repos leaked into workspace inventory: {relative_paths:?}"
        );
    }

    #[test]
    fn oracle_candidate_count_respects_policy_and_control_dir() {
        let root = temp_root("oracle-count");
        write_text(&root.join(".oracleignore"), "data/\n*.secret.txt\n");
        write_text(&root.join(".aspisignore"), "notes/archive/\n");
        write_text(&root.join("README.md"), "# Aspis\n");
        write_text(&root.join("src").join("app.py"), "print('ok')\n");
        write_text(&root.join("data").join("raw.py"), "print('ignored')\n");
        write_text(
            &root
                .join("app")
                .join("node_modules")
                .join("pkg")
                .join("index.ts"),
            "ignored\n",
        );
        write_text(
            &root.join("notes").join("archive").join("old.md"),
            "ignored\n",
        );
        write_text(
            &root.join("_workspace").join("inventory").join("report.md"),
            "ignored\n",
        );
        write_text(&root.join("notes").join("key.secret.txt"), "ignored\n");

        let count = count_oracle_candidate_files(&root).expect("oracle candidate count");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(count, 2);
    }

    #[test]
    fn workspace_report_smoke_writes_readable_inventory() {
        let root = temp_root("report-smoke");
        write_text(&root.join(".oracleignore"), "data/\n");
        write_text(&root.join(".aspisignore"), "node_modules/\n");
        write_text(&root.join("README.md"), "# Workspace\n");
        write_text(
            &root.join("src").join("main.ts"),
            "export const ok = true;\n",
        );

        write_workspace_reports(&root).expect("workspace report write");
        let snapshot = snapshot_from_reports(root.clone(), true).expect("workspace snapshot");
        let inventory_dir = root.join(WORKSPACE_DIR).join(INVENTORY_DIR);
        let manifests_dir = root.join(WORKSPACE_DIR).join(MANIFESTS_DIR);

        assert!(inventory_dir.join(TOP_LEVEL_CSV).is_file());
        assert!(inventory_dir.join(LARGE_FILES_CSV).is_file());
        assert!(inventory_dir.join(GIT_REPOS_CSV).is_file());
        assert!(manifests_dir.join(CLASSIFICATION_CSV).is_file());
        assert!(!snapshot.needs_scan);
        assert_eq!(snapshot.oracle_candidate_files, 2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn package_filter_keeps_design_tokens_but_skips_credentials() {
        let root = temp_root("package-filter");
        write_text(
            &root.join("src").join("theme").join("tokens.ts"),
            "export const x = 1;\n",
        );
        write_text(&root.join("token.txt"), "secret\n");
        write_text(
            &root.join("aspis-secrets").join("aspis").join("token.txt"),
            "secret\n",
        );
        write_text(&root.join("image.png"), "not source\n");

        let candidates = collect_package_candidates(&root).expect("package candidates");
        let paths = candidates
            .files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect::<Vec<_>>();
        let _ = fs::remove_dir_all(&root);

        assert!(paths.iter().any(|path| path == "src/theme/tokens.ts"));
        assert!(!paths.iter().any(|path| path.contains("aspis-secrets")));
        assert!(!paths.iter().any(|path| path == "token.txt"));
        assert!(!paths.iter().any(|path| path == "image.png"));
    }

    #[test]
    fn encrypted_package_stream_roundtrip_uses_aes_gcm_chunks() {
        let root = temp_root("package-stream");
        let package_path = root.join("roundtrip.bin");
        let plaintext = b"hello encrypted workspace package".repeat(90_000);
        let key = Zeroizing::new([7u8; 32]);
        let prefix = [1u8, 2, 3, 4];
        let digest = [9u8; 32];
        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-test".into(), key.clone(), prefix, digest)
                    .unwrap();
            writer.write_all(&plaintext).expect("encrypt write");
            writer.finish().expect("encrypt finish");
        }

        let file = fs::File::open(&package_path).expect("package read");
        let mut reader =
            EncryptedPackageReader::new(file, "pkg-test".into(), key, prefix, digest).unwrap();
        let mut restored = Vec::new();
        reader.read_to_end(&mut restored).expect("decrypt read");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(restored, plaintext);
    }

    #[test]
    fn encrypted_package_rejects_wrong_header_digest() {
        // A1: a different header digest in the AAD must fail payload decryption.
        let root = temp_root("package-header-bind");
        let package_path = root.join("roundtrip.bin");
        let plaintext = b"bind the header into payload aad".to_vec();
        let key = Zeroizing::new([3u8; 32]);
        let prefix = [10u8, 11, 12, 13];
        let digest = [1u8; 32];
        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-bind".into(), key.clone(), prefix, digest)
                    .unwrap();
            writer.write_all(&plaintext).expect("encrypt write");
            writer.finish().expect("encrypt finish");
        }

        let file = fs::File::open(&package_path).expect("package read");
        let tampered_digest = [2u8; 32];
        let mut reader =
            EncryptedPackageReader::new(file, "pkg-bind".into(), key, prefix, tampered_digest)
                .unwrap();
        let mut restored = Vec::new();
        let result = reader.read_to_end(&mut restored);
        let _ = fs::remove_dir_all(&root);

        assert!(result.is_err(), "wrong header digest must fail decryption");
    }

    #[test]
    fn encrypted_package_rejects_truncated_final_chunk() {
        // A2: dropping the final authenticated chunk (and its EOF marker) must be
        // detected rather than treated as a clean end-of-stream.
        let root = temp_root("package-truncate");
        let package_path = root.join("roundtrip.bin");
        // Two full chunks plus a final chunk.
        let plaintext = vec![42u8; PACKAGE_CHUNK_SIZE * 2 + 10];
        let key = Zeroizing::new([5u8; 32]);
        let prefix = [20u8, 21, 22, 23];
        let digest = [7u8; 32];
        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-trunc".into(), key.clone(), prefix, digest)
                    .unwrap();
            writer.write_all(&plaintext).expect("encrypt write");
            writer.finish().expect("encrypt finish");
        }

        // Read the first non-final chunk only (length prefix + ciphertext), then
        // append a bare 0u32 EOF marker — i.e. a truncated stream missing the
        // authenticated final chunk.
        let raw = fs::read(&package_path).expect("read package");
        let first_len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        let mut truncated = raw[..4 + first_len].to_vec();
        truncated.extend_from_slice(&0u32.to_le_bytes());

        let mut reader = EncryptedPackageReader::new(
            std::io::Cursor::new(truncated),
            "pkg-trunc".into(),
            key,
            prefix,
            digest,
        )
        .unwrap();
        let mut restored = Vec::new();
        let result = reader.read_to_end(&mut restored);
        let _ = fs::remove_dir_all(&root);

        assert!(
            result.is_err(),
            "truncation before the authenticated final chunk must be rejected"
        );
    }

    #[test]
    fn encrypted_package_rejects_tampered_payload_chunk() {
        // The signature/manifest layer is additive; the underlying AEAD must
        // still catch a flipped ciphertext byte. Flip one byte inside the first
        // chunk's ciphertext and confirm decryption fails.
        let root = temp_root("package-payload-tamper");
        let package_path = root.join("roundtrip.bin");
        let plaintext = b"the payload AEAD must still catch tampering".to_vec();
        let key = Zeroizing::new([6u8; 32]);
        let prefix = [30u8, 31, 32, 33];
        let digest = [8u8; 32];
        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-pay".into(), key.clone(), prefix, digest)
                    .unwrap();
            writer.write_all(&plaintext).expect("encrypt write");
            writer.finish().expect("encrypt finish");
        }

        let mut raw = fs::read(&package_path).expect("read package");
        // Byte 4 is the first ciphertext byte (after the 4-byte length prefix).
        raw[4] ^= 0x01;

        let mut reader = EncryptedPackageReader::new(
            std::io::Cursor::new(raw),
            "pkg-pay".into(),
            key,
            prefix,
            digest,
        )
        .unwrap();
        let mut restored = Vec::new();
        let result = reader.read_to_end(&mut restored);
        let _ = fs::remove_dir_all(&root);

        assert!(
            result.is_err(),
            "a flipped payload ciphertext byte must fail AEAD decryption"
        );
    }

    #[test]
    fn encrypted_tar_package_restores_files_safely() {
        let root = temp_root("package-tar");
        let package_path = root.join("roundtrip.aspiswspkg");
        let output_dir = root.join("out");
        let key = Zeroizing::new([8u8; 32]);
        let prefix = [5u8, 6, 7, 8];
        let digest = [4u8; 32];
        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-tar".into(), key.clone(), prefix, digest)
                    .unwrap();
            {
                let mut builder = tar::Builder::new(&mut writer);
                append_bytes_to_tar(&mut builder, "src/app.ts", b"export const ok = true;\n")
                    .expect("append file");
                builder.finish().expect("tar finish");
            }
            writer.finish().expect("encrypt finish");
        }

        let file = fs::File::open(&package_path).expect("package read");
        let reader =
            EncryptedPackageReader::new(file, "pkg-tar".into(), key, prefix, digest).unwrap();
        let (files, bytes, warnings) = safe_unpack_tar(reader, &output_dir).expect("unpack");
        let restored = fs::read_to_string(output_dir.join("src").join("app.ts")).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(files, 1);
        assert!(bytes > 0);
        assert!(warnings.is_empty());
        assert_eq!(restored, "export const ok = true;\n");
    }

    #[test]
    fn validate_package_id_rejects_traversal_and_separators() {
        // A3
        assert!(validate_package_id("aspis-bootstrap-2026-05-29").is_ok());
        assert!(validate_package_id("").is_err());
        assert!(validate_package_id("..").is_err());
        assert!(validate_package_id("a/b").is_err());
        assert!(validate_package_id("a\\b").is_err());
        assert!(validate_package_id("C:foo").is_err());
        assert!(validate_package_id("../escape").is_err());
        assert!(validate_package_id("foo..bar").is_err());
        assert!(validate_package_id("...").is_err());
        assert!(validate_package_id("space name").is_err());
    }

    #[test]
    fn package_key_wrap_roundtrip_uses_x25519_and_hkdf() {
        let private_key = [11u8; 32];
        let public_key = PublicKey::from(&StaticSecret::from(private_key));
        let recipient = WorkspacePackageRecipient {
            fingerprint: devices::public_key_fingerprint(&hex::encode(public_key.as_bytes()))
                .unwrap(),
            collaborator_name: "Ada".into(),
            device_name: "Ada Mac".into(),
            platform: "macos".into(),
            source: "approved_invite".into(),
            public_key: hex::encode(public_key.as_bytes()),
            signing_public_key: None,
            signing_fingerprint: None,
        };
        let data_key = Zeroizing::new([44u8; 32]);
        let wrapped =
            wrap_key_for_recipient("pkg-test", &recipient, &recipient.public_key, &data_key)
                .expect("wrap key");
        let unwrapped = unwrap_package_key("pkg-test", &wrapped, &hex::encode(private_key))
            .expect("unwrap key");

        assert_eq!(*unwrapped, *data_key);
    }

    #[test]
    #[ignore]
    fn current_workspace_package_candidate_smoke() {
        let root = std::env::var("ASPIS_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
                    .join("Desktop")
                    .join("aspis bio")
            });
        if !root.is_dir() {
            eprintln!("workspace root missing: {}", root.display());
            return;
        }
        let candidates = collect_package_candidates(&root).expect("package candidates");
        println!(
            "package candidates: {} files, {:.2} MB, skipped {} files / {:.2} MB",
            candidates.files.len(),
            candidates.total_bytes as f64 / 1024f64.powi(2),
            candidates.skipped_files,
            candidates.skipped_bytes as f64 / 1024f64.powi(2)
        );
        for warning in candidates.warnings.iter().take(20) {
            println!("warning: {warning}");
        }
        assert!(
            candidates.total_bytes < PACKAGE_MAX_BYTES,
            "candidate package exceeds 1GB"
        );
        assert!(
            !candidates.files.iter().any(|file| file
                .relative_path
                .to_ascii_lowercase()
                .contains("aspis-secrets")),
            "secret folder leaked into package candidates"
        );
    }

    #[test]
    fn csv_parser_handles_commas_and_quotes() {
        let parsed = parse_csv_line("\"Path\",\"Notes\"");
        assert_eq!(parsed, vec!["Path".to_string(), "Notes".to_string()]);

        let parsed = parse_csv_line("\"src/app, one\",\"quote \"\"inside\"\"\"");
        assert_eq!(
            parsed,
            vec!["src/app, one".to_string(), "quote \"inside\"".to_string()]
        );
    }

    // ---- S1..S5: package signing + manifest tests -------------------------

    /// A fresh Ed25519 keypair, mirroring how the device signing key is created.
    fn test_signing_key() -> (SigningKey, String, String) {
        let mut seed = [0u8; 32];
        random_fill(&mut seed).expect("seed");
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = hex::encode(signing_key.verifying_key().to_bytes());
        let fingerprint = devices::signing_key_fingerprint(&public_key).expect("fingerprint");
        (signing_key, public_key, fingerprint)
    }

    fn test_header(package_id: &str, manifest: Vec<PackageManifestEntry>) -> PackageHeader {
        PackageHeader {
            version: PACKAGE_VERSION,
            package_id: package_id.into(),
            created_at: now(),
            algorithm: "test".into(),
            chunk_size: PACKAGE_CHUNK_SIZE,
            payload_nonce_prefix: hex::encode([1u8, 2, 3, 4]),
            root_name: "aspis bio".into(),
            recipients: Vec::new(),
            manifest,
        }
    }

    /// S3/S4: signing the header digest produces a block that verifies, and any
    /// tamper to the digest or signer key makes verification fail.
    #[test]
    fn package_signature_signs_and_verifies_header_digest() {
        let (signing_key, public_key, fingerprint) = test_signing_key();
        let digest = [42u8; 32];
        let block = sign_package_header(&signing_key, &digest, &public_key, &fingerprint)
            .expect("sign header");

        // Happy path verifies.
        assert!(verify_package_signature(&block, &digest).is_ok());

        // Tampered header digest -> fails.
        let other_digest = [7u8; 32];
        assert!(
            verify_package_signature(&block, &other_digest).is_err(),
            "a different header digest must fail signature verification"
        );

        // Tampered signature bytes -> fails.
        let mut bad_block = block.clone();
        let mut sig_bytes = hex::decode(&block.signature).unwrap();
        sig_bytes[0] ^= 0xff;
        bad_block.signature = hex::encode(sig_bytes);
        assert!(
            verify_package_signature(&bad_block, &digest).is_err(),
            "a flipped signature byte must fail"
        );

        // Wrong signer public key (different keypair) -> fails.
        let (_other_key, wrong_public, wrong_fp) = test_signing_key();
        let mut wrong_block = block.clone();
        wrong_block.signer_public_key = wrong_public;
        wrong_block.signer_fingerprint = wrong_fp;
        assert!(
            verify_package_signature(&wrong_block, &digest).is_err(),
            "a signature checked against the wrong public key must fail"
        );
    }

    /// S3/S4: full container round trip — write header + signature block to disk,
    /// read them back, and confirm the recomputed digest verifies. This is the
    /// "sign -> verify happy path" at the framing level.
    #[test]
    fn package_header_and_signature_block_roundtrip() {
        let root = temp_root("package-sig-roundtrip");
        let package_path = root.join("pkg.bin");
        let (signing_key, public_key, fingerprint) = test_signing_key();

        let manifest = vec![PackageManifestEntry {
            relative_path: "src/app.ts".into(),
            size: 3,
            sha256_hex: hex::encode(Sha256::digest(b"abc")),
        }];
        let header = test_header("pkg-sig", manifest.clone());

        let digest_written = {
            let mut out = fs::File::create(&package_path).expect("create");
            let digest = write_package_header(&mut out, &header).expect("write header");
            let block = sign_package_header(&signing_key, &digest, &public_key, &fingerprint)
                .expect("sign");
            write_package_signature_block(&mut out, &block).expect("write sig");
            digest
        };

        let mut file = fs::File::open(&package_path).expect("open");
        let (read_header, read_digest, read_block) =
            read_package_header(&mut file).expect("read header+sig");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(read_header.package_id, "pkg-sig");
        assert_eq!(read_header.manifest, manifest);
        assert_eq!(read_digest, digest_written);
        assert_eq!(read_block.signer_public_key, public_key);
        verify_package_signature(&read_block, &read_digest)
            .expect("signature must verify after round trip");
    }

    /// S4: a tampered header on disk changes the recomputed digest, so the stored
    /// signature no longer verifies — caught before any decrypt.
    #[test]
    fn tampered_header_fails_signature_verification() {
        let root = temp_root("package-sig-tamper-header");
        let package_path = root.join("pkg.bin");
        let (signing_key, public_key, fingerprint) = test_signing_key();
        let header = test_header("pkg-tamper", Vec::new());

        {
            let mut out = fs::File::create(&package_path).expect("create");
            let digest = write_package_header(&mut out, &header).expect("write header");
            let block = sign_package_header(&signing_key, &digest, &public_key, &fingerprint)
                .expect("sign");
            write_package_signature_block(&mut out, &block).expect("write sig");
        }

        // Flip one byte inside the JSON header body (after magic + 4-byte length).
        let mut raw = fs::read(&package_path).expect("read");
        let tamper_at = PACKAGE_MAGIC.len() + 4 + 5;
        raw[tamper_at] ^= 0x20; // perturb a character without breaking JSON shape badly
        fs::write(&package_path, &raw).expect("rewrite");

        let mut file = fs::File::open(&package_path).expect("open");
        let result = read_package_header(&mut file)
            .and_then(|(_, digest, block)| verify_package_signature(&block, &digest));
        let _ = fs::remove_dir_all(&root);

        assert!(
            result.is_err(),
            "a tampered header must fail signature verification (or fail to parse)"
        );
    }

    /// Missing signature block (e.g. a v2-style file with only a header) is
    /// rejected by the v3 reader before any decryption.
    #[test]
    fn missing_signature_block_is_rejected() {
        let root = temp_root("package-no-sig");
        let package_path = root.join("pkg.bin");
        let header = test_header("pkg-nosig", Vec::new());

        {
            // Write only the header, no signature block (simulating an unsigned
            // / v2-style package under the v3 magic).
            let mut out = fs::File::create(&package_path).expect("create");
            write_package_header(&mut out, &header).expect("write header");
        }

        let mut file = fs::File::open(&package_path).expect("open");
        let result = read_package_header(&mut file);
        let _ = fs::remove_dir_all(&root);

        assert!(
            result.is_err(),
            "a package with no signature block must be rejected by the v3 reader"
        );
    }

    /// A v2 magic is not accepted by the v3 reader.
    #[test]
    fn v2_magic_is_rejected_by_v3_reader() {
        let root = temp_root("package-v2-magic");
        let package_path = root.join("pkg.bin");
        {
            let mut out = fs::File::create(&package_path).expect("create");
            out.write_all(b"ASPISWSPKG2\n").expect("write old magic");
            out.write_all(&8u32.to_le_bytes()).expect("len");
            out.write_all(b"{\"v\":2}\n").expect("body");
        }
        let mut file = fs::File::open(&package_path).expect("open");
        let result = read_package_header(&mut file);
        let _ = fs::remove_dir_all(&root);
        assert!(result.is_err(), "v2 magic must be rejected");
    }

    /// S1: the manifest covers the README plus every candidate file with the
    /// correct size and SHA-256.
    #[test]
    fn build_manifest_hashes_readme_and_files() {
        let root = temp_root("package-manifest-build");
        let file_a = root.join("a.txt");
        write_text(&file_a, "alpha");
        let file_b = root.join("dir").join("b.txt");
        write_text(&file_b, "bravo!!");

        let candidates = vec![
            PackageCandidate {
                path: file_a.clone(),
                relative_path: "a.txt".into(),
                bytes: 5,
            },
            PackageCandidate {
                path: file_b.clone(),
                relative_path: "dir/b.txt".into(),
                bytes: 7,
            },
        ];
        let readme = b"# readme body";
        let manifest = build_package_manifest("README.md", readme, &candidates).expect("manifest");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(manifest.len(), 3);
        assert_eq!(manifest[0].relative_path, "README.md");
        assert_eq!(manifest[0].size, readme.len() as u64);
        assert_eq!(manifest[0].sha256_hex, hex::encode(Sha256::digest(readme)));
        assert_eq!(manifest[1].relative_path, "a.txt");
        assert_eq!(
            manifest[1].sha256_hex,
            hex::encode(Sha256::digest(b"alpha"))
        );
        assert_eq!(manifest[2].relative_path, "dir/b.txt");
        assert_eq!(
            manifest[2].sha256_hex,
            hex::encode(Sha256::digest(b"bravo!!"))
        );
    }

    /// S5: a restored tree that exactly matches the manifest verifies; a content
    /// change, a size change, a missing file, and an extra file each fail closed.
    #[test]
    fn manifest_verification_detects_mismatch_missing_and_extra() {
        let root = temp_root("package-manifest-verify");
        let out_dir = root.join("out");
        write_text(&out_dir.join("README.md"), "readme");
        write_text(&out_dir.join("src").join("app.ts"), "export const ok = 1;");

        let manifest = vec![
            PackageManifestEntry {
                relative_path: "README.md".into(),
                size: 6,
                sha256_hex: hex::encode(Sha256::digest(b"readme")),
            },
            PackageManifestEntry {
                relative_path: "src/app.ts".into(),
                size: "export const ok = 1;".len() as u64,
                sha256_hex: hex::encode(Sha256::digest(b"export const ok = 1;")),
            },
        ];

        // Happy path.
        verify_restored_against_manifest(&out_dir, &manifest).expect("manifest matches");

        // Content mismatch (same length, different bytes) -> SHA-256 fails.
        write_text(&out_dir.join("README.md"), "READMX");
        assert!(
            verify_restored_against_manifest(&out_dir, &manifest).is_err(),
            "content change with same size must fail on hash"
        );

        // Restore correct content, then change a size -> size mismatch fails.
        write_text(&out_dir.join("README.md"), "readme");
        write_text(&out_dir.join("README.md"), "readme-longer");
        assert!(
            verify_restored_against_manifest(&out_dir, &manifest).is_err(),
            "size change must fail"
        );

        // Missing file -> fails.
        write_text(&out_dir.join("README.md"), "readme");
        fs::remove_file(out_dir.join("src").join("app.ts")).expect("rm");
        assert!(
            verify_restored_against_manifest(&out_dir, &manifest).is_err(),
            "a missing manifest file must fail"
        );

        // Extra file not in the manifest -> fails.
        write_text(&out_dir.join("src").join("app.ts"), "export const ok = 1;");
        write_text(&out_dir.join("extra.txt"), "surprise");
        assert!(
            verify_restored_against_manifest(&out_dir, &manifest).is_err(),
            "an unlisted extra file must fail"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// S5 end-to-end through the encrypted payload: write a tar, encrypt it,
    /// decrypt+unpack it, and verify the restored files against a manifest built
    /// from the same bytes. Also confirms a manifest crafted with a wrong hash is
    /// rejected (simulating a payload that does not match its inventory).
    #[test]
    fn signed_manifest_roundtrip_through_encrypted_payload() {
        let root = temp_root("package-manifest-e2e");
        let package_path = root.join("pkg.bin");
        let output_dir = root.join("out");
        let key = Zeroizing::new([8u8; 32]);
        let prefix = [5u8, 6, 7, 8];
        let digest = [4u8; 32];

        let readme = b"# readme";
        let app_ts = b"export const ok = true;\n";
        let candidates = vec![PackageCandidate {
            path: {
                let p = root.join("app.ts");
                write_text(&p, std::str::from_utf8(app_ts).unwrap());
                p
            },
            relative_path: "src/app.ts".into(),
            bytes: app_ts.len() as u64,
        }];
        let manifest = build_package_manifest("README.md", readme, &candidates).expect("manifest");

        {
            let file = fs::File::create(&package_path).expect("package file");
            let mut writer =
                EncryptedPackageWriter::new(file, "pkg-e2e".into(), key.clone(), prefix, digest)
                    .unwrap();
            {
                let mut builder = tar::Builder::new(&mut writer);
                append_bytes_to_tar(&mut builder, "README.md", readme).expect("readme");
                for candidate in &candidates {
                    append_file_to_tar(&mut builder, candidate).expect("file");
                }
                builder.finish().expect("tar finish");
            }
            writer.finish().expect("encrypt finish");
        }

        let file = fs::File::open(&package_path).expect("package read");
        let reader =
            EncryptedPackageReader::new(file, "pkg-e2e".into(), key, prefix, digest).unwrap();
        safe_unpack_tar(reader, &output_dir).expect("unpack");

        // Good manifest verifies.
        verify_restored_against_manifest(&output_dir, &manifest)
            .expect("restored tree must match manifest");

        // A manifest whose hash is wrong (payload does not match inventory) fails.
        let mut bad_manifest = manifest.clone();
        bad_manifest[1].sha256_hex = hex::encode(Sha256::digest(b"different bytes"));
        assert!(
            verify_restored_against_manifest(&output_dir, &bad_manifest).is_err(),
            "a restored file that differs from its manifest hash must fail"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// Confirms the v3 magic/version are what we expect after the bump.
    #[test]
    fn package_format_is_version_three() {
        assert_eq!(PACKAGE_VERSION, 3);
        assert_eq!(PACKAGE_MAGIC, b"ASPISWSPKG3\n");
    }

    // ---- C1/H1/H2/M3/M4/M5/M6: provenance + hardening tests ----------------

    /// Helper: a `KnownSigner` for a given Ed25519 public key hex (mirrors how
    /// `devices::known_signers` builds entries).
    fn known_signer(public_key: &str, name: &str) -> devices::KnownSigner {
        devices::KnownSigner {
            signing_public_key: public_key.to_ascii_lowercase(),
            name: name.to_string(),
        }
    }

    /// C1 end-to-end signature-swap attack at the framing level (no AppHandle): an
    /// attacker re-signs the package header with their OWN Ed25519 key and swaps the
    /// signature block. The signature still verifies (integrity holds), but the
    /// signer is unknown, so the provenance gate must REFUSE with
    /// allow_unknown_signer=false and ALLOW (signer_known=false) with =true.
    #[test]
    fn swap_attack_unknown_signer_is_refused_by_default() {
        let root = temp_root("package-swap-attack");
        let package_path = root.join("pkg.bin");

        // Honest signer (the approved device) and the attacker (a different key).
        let (honest_key, honest_public, honest_fp) = test_signing_key();
        let (attacker_key, attacker_public, _attacker_fp) = test_signing_key();
        assert_ne!(honest_public, attacker_public, "keys must differ");

        let manifest = vec![PackageManifestEntry {
            relative_path: "ASPIS_BOOTSTRAP_README.md".into(),
            size: 3,
            sha256_hex: hex::encode(Sha256::digest(b"abc")),
        }];
        let header = test_header("pkg-swap", manifest);

        // Write the header, then the HONEST signature block.
        let digest = {
            let mut out = fs::File::create(&package_path).expect("create");
            let digest = write_package_header(&mut out, &header).expect("write header");
            let block = sign_package_header(&honest_key, &digest, &honest_public, &honest_fp)
                .expect("sign honest");
            write_package_signature_block(&mut out, &block).expect("write sig");
            digest
        };

        // Attacker re-signs the SAME header digest with their own key and rewrites
        // the signature block in place (header bytes untouched, so the digest is
        // identical — this is the swap, not a header tamper).
        {
            let attacker_block = sign_package_header(
                &attacker_key,
                &digest,
                &attacker_public,
                // M6 proves this self-reported fingerprint is ignored for display:
                // feed a lie here and confirm the recomputed one wins below.
                "DEAD-BEEF-DEAD-BEEF",
            )
            .expect("sign attacker");
            let mut out = fs::OpenOptions::new()
                .write(true)
                .open(&package_path)
                .expect("reopen");
            // Re-emit header (same bytes) then the attacker block, truncating any
            // trailing bytes from the longer/shorter honest block.
            out.set_len(0).expect("truncate");
            let rewritten = write_package_header(&mut out, &header).expect("rewrite header");
            assert_eq!(rewritten, digest, "header digest must be unchanged");
            write_package_signature_block(&mut out, &attacker_block).expect("write attacker sig");
        }

        // Read it back: the attacker's signature still VERIFIES (integrity holds).
        let mut file = fs::File::open(&package_path).expect("open");
        let (_header, read_digest, block) = read_package_header(&mut file).expect("read");
        verify_package_signature(&block, &read_digest)
            .expect("attacker's own signature is cryptographically valid");

        let signer_public_key = block.signer_public_key.to_ascii_lowercase();
        assert_eq!(signer_public_key, attacker_public);

        // Only the HONEST device is a known signer.
        let known = vec![known_signer(&honest_public, "This device")];

        // allow_unknown_signer = false -> REFUSED before extraction.
        let refused = resolve_signer_decision(&signer_public_key, &known, false);
        assert!(
            refused.is_err(),
            "a valid signature from an UNKNOWN signer must be refused by default"
        );
        assert!(
            refused
                .unwrap_err()
                .to_ascii_lowercase()
                .contains("unknown"),
            "the refusal must name the unknown-signer reason"
        );

        // allow_unknown_signer = true -> imports, but signer_known = false.
        let allowed =
            resolve_signer_decision(&signer_public_key, &known, true).expect("opt-in import");
        assert!(
            !allowed.signer_known,
            "an opt-in import of an unknown signer must still report signer_known = false"
        );
        assert!(allowed.signer_name.is_none());
        // M6: the surfaced fingerprint is RECOMPUTED from the attacker's verified
        // key, not the "DEAD-BEEF..." lie in the block.
        let expected_fp = devices::signing_key_fingerprint(&attacker_public).unwrap();
        assert_eq!(allowed.signer_fingerprint, expected_fp);
        assert_ne!(allowed.signer_fingerprint, "DEAD-BEEF-DEAD-BEEF");

        let _ = fs::remove_dir_all(&root);
    }

    /// C1: a package signed by a KNOWN (approved) signer resolves to known and is
    /// allowed regardless of the opt-in flag.
    #[test]
    fn known_signer_is_allowed_and_named() {
        let (_key, public, _fp) = test_signing_key();
        let known = vec![known_signer(&public, "Ada (Ada Mac)")];

        let decision = resolve_signer_decision(&public, &known, false).expect("known signer ok");
        assert!(decision.signer_known);
        assert_eq!(decision.signer_name.as_deref(), Some("Ada (Ada Mac)"));
        // M6: recomputed fingerprint matches the device fingerprint of this key.
        assert_eq!(
            decision.signer_fingerprint,
            devices::signing_key_fingerprint(&public).unwrap()
        );
    }

    /// H1: verify_strict accepts an honest signature and rejects a flipped-bit one.
    #[test]
    fn verify_strict_accepts_honest_and_rejects_forged() {
        let (signing_key, public_key, fingerprint) = test_signing_key();
        let digest = [55u8; 32];
        let block =
            sign_package_header(&signing_key, &digest, &public_key, &fingerprint).expect("sign");

        // Honest signature verifies under verify_strict.
        verify_package_signature(&block, &digest).expect("honest signature must verify");

        // A flipped signature bit fails under verify_strict.
        let mut forged = block.clone();
        let mut sig_bytes = hex::decode(&block.signature).unwrap();
        sig_bytes[10] ^= 0x40;
        forged.signature = hex::encode(sig_bytes);
        assert!(
            verify_package_signature(&forged, &digest).is_err(),
            "a flipped-bit signature must fail verify_strict"
        );
    }

    /// H2: the signature scheme is validated; an unexpected scheme is rejected even
    /// when the signature itself would verify.
    #[test]
    fn unsupported_signature_scheme_is_rejected() {
        let (signing_key, public_key, fingerprint) = test_signing_key();
        let digest = [77u8; 32];
        let mut block =
            sign_package_header(&signing_key, &digest, &public_key, &fingerprint).expect("sign");

        // Sanity: with the real scheme it verifies.
        verify_package_signature(&block, &digest).expect("baseline verify");

        // A different / empty scheme is refused before the cryptographic check.
        block.scheme = "sha512-header".into();
        assert!(
            verify_package_signature(&block, &digest).is_err(),
            "an unexpected scheme must be rejected"
        );
        block.scheme = String::new();
        assert!(
            verify_package_signature(&block, &digest).is_err(),
            "an empty scheme must be rejected"
        );
    }

    /// M5: a header whose manifest has 0 entries is rejected at read/verify time.
    #[test]
    fn empty_manifest_is_rejected() {
        let root = temp_root("package-empty-manifest");
        let package_path = root.join("pkg.bin");
        let (signing_key, public_key, fingerprint) = test_signing_key();
        // Empty manifest.
        let header = test_header("pkg-empty", Vec::new());
        {
            let mut out = fs::File::create(&package_path).expect("create");
            let digest = write_package_header(&mut out, &header).expect("write header");
            let block = sign_package_header(&signing_key, &digest, &public_key, &fingerprint)
                .expect("sign");
            write_package_signature_block(&mut out, &block).expect("write sig");
        }
        let mut file = fs::File::open(&package_path).expect("open");
        let result = read_package_header(&mut file);
        let _ = fs::remove_dir_all(&root);
        assert!(
            result.is_err(),
            "a v3 header with an empty manifest must be rejected"
        );
    }

    /// M4: duplicate relative_path entries are rejected, both on the create side
    /// (build_package_manifest, via shared validator) and on read/verify.
    #[test]
    fn duplicate_manifest_path_is_rejected() {
        // Direct validator: duplicate paths fail.
        let dup = vec![
            PackageManifestEntry {
                relative_path: "src/app.ts".into(),
                size: 1,
                sha256_hex: hex::encode(Sha256::digest(b"a")),
            },
            PackageManifestEntry {
                relative_path: "src/app.ts".into(),
                size: 1,
                sha256_hex: hex::encode(Sha256::digest(b"b")),
            },
        ];
        assert!(
            validate_manifest_paths(&dup).is_err(),
            "duplicate manifest paths must be rejected"
        );

        // Read/verify side: a header carrying duplicate manifest paths is rejected.
        let root = temp_root("package-dup-manifest");
        let package_path = root.join("pkg.bin");
        let (signing_key, public_key, fingerprint) = test_signing_key();
        let header = test_header("pkg-dup", dup);
        {
            let mut out = fs::File::create(&package_path).expect("create");
            let digest = write_package_header(&mut out, &header).expect("write header");
            let block = sign_package_header(&signing_key, &digest, &public_key, &fingerprint)
                .expect("sign");
            write_package_signature_block(&mut out, &block).expect("write sig");
        }
        let mut file = fs::File::open(&package_path).expect("open");
        let result = read_package_header(&mut file);
        let _ = fs::remove_dir_all(&root);
        assert!(
            result.is_err(),
            "a header with duplicate manifest paths must be rejected on read"
        );
    }

    /// M3: a manifest entry whose relative_path escapes via `..` (or is absolute)
    /// is rejected by the Component::Normal-only validator.
    #[test]
    fn traversal_manifest_path_is_rejected() {
        let traversal = vec![PackageManifestEntry {
            relative_path: "../escape.txt".into(),
            size: 1,
            sha256_hex: hex::encode(Sha256::digest(b"x")),
        }];
        assert!(
            validate_manifest_paths(&traversal).is_err(),
            "a manifest path containing '..' must be rejected"
        );

        // A nested traversal is also rejected.
        let nested = vec![PackageManifestEntry {
            relative_path: "src/../../secret".into(),
            size: 1,
            sha256_hex: hex::encode(Sha256::digest(b"x")),
        }];
        assert!(
            validate_manifest_paths(&nested).is_err(),
            "a nested '..' manifest path must be rejected"
        );

        // A clean relative path passes.
        let ok = vec![PackageManifestEntry {
            relative_path: "src/app.ts".into(),
            size: 1,
            sha256_hex: hex::encode(Sha256::digest(b"x")),
        }];
        assert!(
            validate_manifest_paths(&ok).is_ok(),
            "a clean relative manifest path must pass"
        );
    }

    /// The cloud-download filename sanitizer must never let a remote URL choose a
    /// traversal/absolute/hidden path, and must always land on `.aspiswspkg`.
    #[test]
    fn sanitize_package_filename_is_safe_and_suffixed() {
        // Normal case keeps the name, already has the extension.
        assert_eq!(
            sanitize_package_filename("https://cdn.example.com/aspis-bio.aspiswspkg"),
            "aspis-bio.aspiswspkg"
        );
        // Query string and fragment are dropped before the last segment.
        assert_eq!(
            sanitize_package_filename("https://x/y/pkg.aspiswspkg?sig=abc&t=1#frag"),
            "pkg.aspiswspkg"
        );
        // Missing extension gets one appended.
        assert_eq!(
            sanitize_package_filename("https://x/bootstrap"),
            "bootstrap.aspiswspkg"
        );
        // Separators and traversal collapse to underscores (no escape possible).
        let cleaned = sanitize_package_filename("https://x/..%2F..%2Fetc%2Fpasswd");
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('\\'));
        assert!(!cleaned.starts_with('.'));
        assert!(cleaned.ends_with(".aspiswspkg"));
        // A path ending in a slash yields the non-empty fallback stem.
        assert_eq!(
            sanitize_package_filename("https://x/folder/"),
            "cloud-workspace.aspiswspkg"
        );
        // A dotfile-only segment cannot produce a hidden file.
        let hidden = sanitize_package_filename("https://x/.hidden");
        assert!(!hidden.starts_with('.'));
        assert!(hidden.ends_with(".aspiswspkg"));
    }
}
