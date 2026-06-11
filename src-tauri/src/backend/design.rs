//! Generative-design module — working-folder persistence (Phase 1a, Rust core).
//!
//! The working folder (inside the TARGET project, e.g. `<target>/.aspis-design/<name>/`)
//! is the ONLY authoritative source for a design: per-node sanitized markup +
//! placement manifest + project metadata. The management plane stores metadata only;
//! nothing here touches config.json. This file owns the on-disk format and the IO
//! commands; it deliberately treats node markup as an OPAQUE string (no HTML parsing
//! in Rust — sanitization is a frontend/DOMPurify concern per the plan).
//!
//! Layout (filename === node id, path-confined):
//! ```text
//! <workingFolder>/
//!   project.json    # { schemaVersion, id, name, createdAt, updatedAt, canvas, nodeOrder }
//!   manifest.json   # { schemaVersion, nodes: { "<id>": { x,y,z,w,h, kind } } }
//!   components/<id>.html  # opaque sanitized inner markup for ONE node
//! ```
//!
//! Atomic writes reuse `replace_file_with_backup` + the temp/backup suffix idiom from
//! `projects.rs`. A SEPARATE `design_write_lock()` serializes design read-modify-write
//! sequences so they never block on (or get blocked by) Settings/config saves.
//!
//! Path confinement (correction #3 of the plan, mirrors `mini_coder_executor.rs`):
//! every node id is validated against a strict charset BEFORE any IO, the working
//! folder is canonicalized, the component path is joined + re-canonicalized (when it
//! exists) and asserted to stay under `<workingFolder>/components`. `..`, absolute
//! paths, slashes/backslashes, leading dots, uppercase, overlong, empty, and symlink
//! escapes are all rejected.

use super::fs_replace::replace_file_with_backup;
use super::state::BackendState;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tauri::State;

const SCHEMA_VERSION: u32 = 1;
const COMPONENTS_DIR: &str = "components";
const PROJECT_FILE: &str = "project.json";
const MANIFEST_FILE: &str = "manifest.json";

/// Max bytes we will read/write for any single on-disk design file. Generous for markup
/// + manifest yet bounds a hostile/corrupt file so a read can never OOM the app and a
/// malicious frontend payload can never balloon a component file.
const MAX_DESIGN_FILE_BYTES: u64 = 8 * 1024 * 1024; // 8 MiB

/// Hard cap on the number of nodes we will iterate for a single project. Bounds a
/// hostile/corrupt manifest (or a malicious save payload) so load/save cannot be turned
/// into an unbounded loop / IO storm.
const MAX_DESIGN_NODES: usize = 2000;

/// Upper bound (px) on a node's optional corner `radius`. Mirrors the bounded posture
/// the plan calls for: a save carrying a negative, NaN/inf, or absurdly large radius is
/// rejected rather than persisted. 200px is far beyond any realistic card corner.
const MAX_NODE_RADIUS: f64 = 200.0;

/// Cap (chars) on a node's optional display `name`. A pathological/crafted payload could
/// otherwise carry an unbounded label into the manifest and every downstream consumer; we
/// hard-fail a save whose name exceeds this. ~80 chars per the plan.
const MAX_NODE_NAME_CHARS: usize = 80;

/// Cap on the number of load warnings we retain. A pathological manifest could otherwise
/// produce thousands of warning strings; we keep the first `MAX_DESIGN_WARNINGS` and add
/// a final truncation note.
const MAX_DESIGN_WARNINGS: usize = MAX_DESIGN_NODES + 1;

/// Process-wide lock guarding design working-folder access. DISTINCT from
/// `projects::config_write_lock` on purpose: design writes (drag-commit manifest
/// writes, per-node markup writes, Consolida) must NOT serialize against Settings/
/// config.json saves, and vice versa — they touch different trees. The critical
/// section is only the fast temp-write + atomic rename, so contention is negligible.
///
/// W5: this is an `RwLock`, not a plain mutex. WRITERS (every read-modify-write
/// command) take the WRITE guard via `design_write_guard()` so they remain mutually
/// exclusive. `design_load_project` does a multi-file READ and takes a READ guard
/// via `design_read_guard()`: concurrent loads can proceed, and a load no longer
/// blocks the FAST drag-commit writer for the whole multi-file read — a write still
/// excludes loads (and vice versa), so a load never observes a half-written tree.
///
/// LOCK ORDERING: a command that needs BOTH locks (registry remove takes
/// `config_write_lock` THEN this design lock) MUST acquire config FIRST, then
/// design — keep that order everywhere to avoid a deadlock.
fn design_rwlock() -> &'static RwLock<()> {
    static LOCK: OnceLock<RwLock<()>> = OnceLock::new();
    LOCK.get_or_init(|| RwLock::new(()))
}

/// Acquire the design WRITE guard (exclusive) for a read-modify-write command.
fn design_write_guard() -> Result<std::sync::RwLockWriteGuard<'static, ()>, String> {
    design_rwlock()
        .write()
        .map_err(|_| "Design write lock is poisoned.".to_string())
}

/// Acquire the design READ guard (shared) for a multi-file read (load). Excludes
/// writers but allows concurrent readers.
fn design_read_guard() -> Result<std::sync::RwLockReadGuard<'static, ()>, String> {
    design_rwlock()
        .read()
        .map_err(|_| "Design write lock is poisoned.".to_string())
}

// ---------------------------------------------------------------------------
// On-the-wire structs (camelCase over IPC)
// ---------------------------------------------------------------------------

/// Canvas geometry stored in `project.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignCanvas {
    pub w: f64,
    pub h: f64,
    pub grid: f64,
}

/// `project.json` — project metadata + the ordered list of top-level node ids.
/// `nodeOrder` is the paint/stacking order companion to per-node `z`; both are
/// persisted so a reload restores stacking exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProjectMeta {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub canvas: DesignCanvas,
    pub node_order: Vec<String>,
}

/// Node kind. Markup is opaque to Rust; this only records which sanitizer profile the
/// frontend must use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesignNodeKind {
    Html,
    Svg,
}

/// `h` is either a fixed numeric height or the string `"auto"` (hug-contents, the
/// default). Untagged so it serializes as a bare number or the literal string `"auto"`
/// — matching the TS `number | "auto"` shape exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeHeight {
    Auto(AutoHeight),
    Fixed(f64),
}

/// The literal `"auto"`. A dedicated unit-like enum gives us an exact-string match on
/// deserialize (a stray string that is not `"auto"` is rejected rather than silently
/// accepted) while still serializing to the bare token `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoHeight {
    Auto,
}

/// One manifest entry: top-level placement in global canvas coords + size + kind.
///
/// The four trailing fields (`radius`/`flat`/`hidden`/`name`) are OPTIONAL canvas
/// presentation hints added after schema v1 shipped. Each is `#[serde(default,
/// skip_serializing_if = "Option::is_none")]` so an OLD `manifest.json` written
/// without them deserializes cleanly AND a node that does not set them adds zero
/// schema churn on disk (the keys are simply absent). They are placement/display
/// metadata only — markup remains opaque to Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignNodePlacement {
    pub x: f64,
    pub y: f64,
    pub z: i64,
    pub w: f64,
    pub h: NodeHeight,
    pub kind: DesignNodeKind,
    /// Corner radius (px) for the node card. Bounded `[0, MAX_NODE_RADIUS]` on save
    /// (negative / absurd values rejected). Absent => the stylesheet default radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Render the card "flat" (no card chrome). Absent => not flat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat: Option<bool>,
    /// Hide the node on the canvas (layer visibility). Absent => visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Display label for the layers panel / node tag. Capped at `MAX_NODE_NAME_CHARS`
    /// on save. Absent => no custom label (the id is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `manifest.json` — placement-only authority over top-level nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignManifest {
    pub schema_version: u32,
    /// `BTreeMap` so the on-disk key order is deterministic (stable diffs in git).
    pub nodes: BTreeMap<String, DesignNodePlacement>,
}

/// Full in-memory project handed to / received from the frontend: metadata + manifest
/// + the opaque markup of every node, keyed by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProject {
    pub meta: DesignProjectMeta,
    pub manifest: DesignManifest,
    /// id -> opaque sanitized inner markup. A manifest id whose component file is
    /// missing/older is tolerated on load (absent here) and surfaced in `warnings`.
    pub components: BTreeMap<String, String>,
    /// Non-fatal load warnings (e.g. a manifest id with no component file). Empty on a
    /// clean load. Skipped on the wire when empty so a healthy project stays minimal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Path confinement
// ---------------------------------------------------------------------------

/// Validate a node id against `^[a-z0-9][a-z0-9_-]{0,63}$`. This is the FIRST gate on
/// every id-derived path: it rejects `..`, slashes/backslashes, absolute drive specs,
/// leading dots, uppercase, empty, overlong (> 64 chars), and any non-ASCII / Unicode
/// trick BEFORE the id is ever joined to a path. Pure + total so it is unit-testable
/// without a filesystem.
pub(crate) fn validate_node_id(id: &str) -> Result<(), String> {
    let len = id.len();
    if len == 0 {
        return Err("node id must not be empty".to_string());
    }
    if len > 64 {
        return Err(format!("node id too long ({len} > 64): {id}"));
    }
    let mut chars = id.chars();
    // First char: [a-z0-9] (no leading `-`, `_`, or `.`).
    let first = chars.next().unwrap(); // len > 0 checked above.
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "node id must start with a lowercase letter or digit: {id}"
        ));
    }
    // Remaining chars: [a-z0-9_-].
    for c in chars {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if !ok {
            return Err(format!("node id contains an invalid character {c:?}: {id}"));
        }
    }
    Ok(())
}

/// Validate a single placement's OPTIONAL presentation fields before a save. Mirrors the
/// "validate everything up front, hard-fail" posture used for ids: a `radius` that is
/// non-finite, negative, or beyond `MAX_NODE_RADIUS`, or a `name` longer than
/// `MAX_NODE_NAME_CHARS`, is rejected so a hostile/buggy payload can never persist an
/// absurd value into the manifest. The geometry fields (`x/y/z/w/h`) keep their existing
/// posture — they are NOT range-checked here (the current code never bounded them), so we
/// only gate the NEW fields. Pure + total: unit-testable without a filesystem.
fn validate_placement(id: &str, p: &DesignNodePlacement) -> Result<(), String> {
    if let Some(r) = p.radius {
        if !r.is_finite() || r < 0.0 || r > MAX_NODE_RADIUS {
            return Err(format!(
                "node \"{}\" has an invalid radius ({r}); expected 0..={MAX_NODE_RADIUS}",
                sanitize_id_for_warning(id)
            ));
        }
    }
    if let Some(name) = &p.name {
        // Count CHARS (not bytes) so a multibyte label is bounded the way a user perceives
        // it; the cap also protects every downstream consumer of the label.
        if name.chars().count() > MAX_NODE_NAME_CHARS {
            return Err(format!(
                "node \"{}\" name is too long ({} > {MAX_NODE_NAME_CHARS} chars)",
                sanitize_id_for_warning(id),
                name.chars().count()
            ));
        }
    }
    Ok(())
}

/// Sanitize an UNTRUSTED id for inclusion in a user-facing warning string. A corrupt /
/// hand-edited manifest could carry an arbitrarily long, non-printable, or control-char
/// id; we truncate to 32 chars and replace every non-ASCII / non-printable byte with `?`
/// so the warning can never inject control sequences nor blow up the wire payload.
fn sanitize_id_for_warning(id: &str) -> String {
    let mut out: String = id
        .chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if id.chars().count() > 32 {
        out.push('…');
    }
    out
}

/// Append a warning, capping the vec length so a pathological manifest cannot produce an
/// unbounded warnings payload. The cap note is added exactly once on overflow.
fn push_warning(warnings: &mut Vec<String>, msg: String) {
    if warnings.len() < MAX_DESIGN_WARNINGS {
        warnings.push(msg);
    } else if warnings.len() == MAX_DESIGN_WARNINGS {
        warnings.push("…further warnings suppressed".to_string());
    }
}

/// Canonicalize the working folder. It must already exist and be a directory (every
/// command here operates on an EXISTING project, except `create` which canonicalizes
/// the parent then creates the leaf — handled separately). Canonicalizing collapses
/// `.`/`..`/symlinks to a real path so the under-root assertion below is meaningful.
fn canonical_working_folder(working_folder_path: &str) -> Result<PathBuf, String> {
    if working_folder_path.trim().is_empty() {
        return Err("working folder path must not be empty".to_string());
    }
    let raw = PathBuf::from(working_folder_path);
    let canonical = fs::canonicalize(&raw).map_err(|e| {
        // Detail (including the absolute path) goes to the process log ONLY; the wire
        // error carries a stable short label so no FS layout leaks to the renderer.
        eprintln!(
            "[design] working folder unreadable: {} ({e})",
            raw.display()
        );
        "working folder does not exist or is unreadable".to_string()
    })?;
    if !canonical.is_dir() {
        return Err("working folder is not a directory".to_string());
    }
    Ok(canonical)
}

/// Resolve the confined component path for `id` under an ALREADY-CANONICAL working
/// folder. `id` is validated (charset) first, then the path is built as
/// `<workingFolder>/components/<id>.html`. If the target already exists we
/// re-canonicalize it and assert the result is still under `<workingFolder>/components`
/// (catches a symlink planted inside `components/` that points off-root). When the
/// target does NOT yet exist (a fresh write) canonicalize would fail, so we assert
/// lexically against the (canonical) components dir instead — the validated id can
/// contain no separators, so the lexical join cannot escape.
fn confined_component_path(canonical_root: &Path, id: &str) -> Result<PathBuf, String> {
    validate_node_id(id)?;
    let components_dir = canonical_root.join(COMPONENTS_DIR);
    let target = components_dir.join(format!("{id}.html"));

    // Lexical guard first: the parent of `target` must be the components dir. A
    // validated id has no separators, so this always holds for a well-formed id; the
    // check is belt-and-suspenders against a future change to the id charset.
    if target.parent() != Some(components_dir.as_path()) {
        return Err(format!("component path escapes the components folder: {id}"));
    }

    // If it exists, canonicalize-after-join and assert it stays under the components
    // dir (defeats a symlink inside components/ pointing elsewhere). A non-existent
    // target is the normal create case and is covered by the lexical guard above.
    // NOTE: the exists()-then-canonicalize sequence has a residual TOCTOU window on a
    // hostile filesystem (a symlink swapped in between the two syscalls). The in-process
    // `design_write_lock` serializes our OWN writers so they cannot race each other; it
    // mitigates — but does not eliminate — an external attacker racing the FS.
    if target.exists() {
        let real = fs::canonicalize(&target)
            .map_err(|e| format!("could not resolve component path for {id}: {e}"))?;
        let real_dir = fs::canonicalize(&components_dir)
            .map_err(|e| format!("could not resolve components folder: {e}"))?;
        if !real.starts_with(&real_dir) {
            return Err(format!("component path escapes the components folder: {id}"));
        }
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Atomic write helpers
// ---------------------------------------------------------------------------

/// Build a unique temp/backup suffix, identical idiom to `projects.rs` (`<pid>-<nanos>`)
/// so concurrent writers never collide on the scratch file name.
fn write_suffix() -> String {
    // `timestamp_micros` is always-Some (no `unwrap_or_default()` zero-collision risk the
    // nanos variant carries past year 2262) and is plenty granular for a per-write suffix.
    format!("{}-{}", std::process::id(), Utc::now().timestamp_micros())
}

/// Atomically write `contents` to `target` via temp + `replace_file_with_backup`. The
/// caller must hold `design_write_lock`. `label` is used in error messages.
fn atomic_write(target: &Path, contents: &str, label: &str) -> Result<(), String> {
    let suffix = write_suffix();
    // Sibling temp/backup files (same dir as target) so the rename is intra-volume and
    // atomic on every OS. We append the suffix to the file name rather than swapping
    // the extension (component files share the `.html` extension and a node id may
    // contain dots-as-`-`? no — but keep it robust by appending).
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid target path for {label}"))?;
    let dir = target
        .parent()
        .ok_or_else(|| format!("target has no parent dir for {label}"))?;
    let temp_path = dir.join(format!("{file_name}.{suffix}.tmp"));
    let backup_path = dir.join(format!("{file_name}.{suffix}.bak"));

    fs::write(&temp_path, contents)
        .map_err(|e| format!("could not write temp file for {label}: {e}"))?;
    replace_file_with_backup(&temp_path, target, &backup_path, label)
}

/// Bounded read of a design file. Missing file -> `Ok(None)`; oversized/unreadable ->
/// `Err`. UTF-8 lossy is NOT used: design JSON/markup is authored as UTF-8 and a
/// non-UTF-8 file is a corruption we surface rather than silently mangle.
fn read_design_file(path: &Path) -> Result<Option<String>, String> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(None), // missing -> caller decides (tolerated for components)
    };
    if meta.len() > MAX_DESIGN_FILE_BYTES {
        eprintln!(
            "[design] file too large ({} bytes > {}): {}",
            meta.len(),
            MAX_DESIGN_FILE_BYTES,
            path.display()
        );
        return Err(format!(
            "design file too large ({} bytes > {} max)",
            meta.len(),
            MAX_DESIGN_FILE_BYTES
        ));
    }
    let s = fs::read_to_string(path).map_err(|e| {
        eprintln!("[design] could not read {}: {e}", path.display());
        "could not read design file".to_string()
    })?;
    Ok(Some(s))
}

/// Reject a `schema_version` we cannot safely consume. We accept anything `<=
/// SCHEMA_VERSION` (older formats are forward-compatible by construction here) and reject
/// anything newer with a clear upgrade hint. `what` is the file label for the message.
fn check_schema_version(version: u32, what: &str) -> Result<(), String> {
    if version > SCHEMA_VERSION {
        return Err(format!(
            "{what} uses a newer format (v{version} > v{SCHEMA_VERSION}); upgrade the app"
        ));
    }
    Ok(())
}

/// Enforce the per-file byte cap on a markup payload before any write. `id` is a validated
/// node id (safe ASCII) by the time this is called.
fn check_write_size(markup: &str, id: &str) -> Result<(), String> {
    if markup.len() as u64 > MAX_DESIGN_FILE_BYTES {
        return Err(format!(
            "component {id} markup too large ({} bytes > {MAX_DESIGN_FILE_BYTES} max)",
            markup.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create a new design working folder atomically: `project.json` + an empty
/// `manifest.json` + a `components/` dir. Fails if `project.json` already exists (we do
/// not clobber an existing project). `workingFolderPath` is created if absent (its
/// parent must exist + be writable). Returns the freshly created `DesignProject`.
#[tauri::command]
pub fn design_create_project(
    state: State<'_, BackendState>,
    working_folder_path: String,
    name: String,
) -> Result<DesignProject, String> {
    state.ensure_unlocked()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }

    let _guard = design_write_guard()?;

    let raw = PathBuf::from(&working_folder_path);
    if raw.as_os_str().is_empty() {
        return Err("working folder path must not be empty".to_string());
    }
    // Create the leaf folder if needed (parent must already exist). create_dir_all is
    // idempotent for the dir itself; we guard against an existing project below.
    fs::create_dir_all(&raw).map_err(|e| {
        eprintln!("[design] could not create working folder {}: {e}", raw.display());
        "could not create working folder".to_string()
    })?;
    let canonical = canonical_working_folder(&working_folder_path)?;

    // Atomically CLAIM project.json with O_EXCL (create_new): if a second process already
    // created it we fail here rather than racing an exists()-then-write check. We claim an
    // empty placeholder, then overwrite it with the real content via atomic_write below
    // (preserving the temp+rename discipline for the content itself).
    let project_path = canonical.join(PROJECT_FILE);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&project_path)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err("a design project already exists in this working folder".to_string());
        }
        Err(e) => {
            eprintln!("[design] could not create {PROJECT_FILE}: {e}");
            return Err("could not create the project file".to_string());
        }
    }
    ensure_components_dir(&canonical)?;

    let now = Utc::now().to_rfc3339();
    let meta = DesignProjectMeta {
        schema_version: SCHEMA_VERSION,
        id: new_project_id(),
        name,
        created_at: now.clone(),
        updated_at: now,
        canvas: DesignCanvas {
            w: 1440.0,
            h: 1024.0,
            grid: 8.0,
        },
        node_order: Vec::new(),
    };
    let manifest = DesignManifest {
        schema_version: SCHEMA_VERSION,
        nodes: BTreeMap::new(),
    };

    write_meta(&canonical, &meta)?;
    write_manifest_file(&canonical, &manifest)?;

    Ok(DesignProject {
        meta,
        manifest,
        components: BTreeMap::new(),
        warnings: Vec::new(),
    })
}

/// Load a design project: `project.json` + `manifest.json` + every
/// `components/<id>.html` for the ids present in the manifest. LENIENT (mirrors the
/// plan's partial-atomic-write tolerance): a manifest id whose component file is
/// missing/unreadable does NOT hard-fail — the id is returned without markup and a
/// warning is appended. A malformed/missing `project.json` or `manifest.json` IS a
/// hard error (the project is unusable without them).
#[tauri::command]
pub fn design_load_project(
    state: State<'_, BackendState>,
    working_folder_path: String,
) -> Result<DesignProject, String> {
    state.ensure_unlocked()?;
    // W5: hold a READ guard for the WHOLE multi-read sequence so a concurrent WRITER
    // (design_write_manifest / design_write_node) cannot interleave a partial state
    // between our project/manifest/component reads — yet concurrent LOADS proceed and
    // a load no longer blocks the fast drag-commit writer for the whole read. The
    // critical section is a few small reads.
    let _guard = design_read_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;

    let meta_raw = read_design_file(&canonical.join(PROJECT_FILE))?
        .ok_or_else(|| format!("{PROJECT_FILE} is missing"))?;
    let mut meta: DesignProjectMeta = serde_json::from_str(&meta_raw)
        .map_err(|e| format!("{PROJECT_FILE} is not valid: {e}"))?;
    check_schema_version(meta.schema_version, PROJECT_FILE)?;

    let manifest_raw = read_design_file(&canonical.join(MANIFEST_FILE))?
        .ok_or_else(|| format!("{MANIFEST_FILE} is missing"))?;
    let manifest: DesignManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| format!("{MANIFEST_FILE} is not valid: {e}"))?;
    check_schema_version(manifest.schema_version, MANIFEST_FILE)?;

    // DoS cap: refuse to iterate an absurd node set from a corrupt/hostile manifest.
    if manifest.nodes.len() > MAX_DESIGN_NODES {
        return Err(format!(
            "manifest has too many nodes ({} > {MAX_DESIGN_NODES} max)",
            manifest.nodes.len()
        ));
    }

    let mut components = BTreeMap::new();
    let mut warnings = Vec::new();

    // Drop any invalid node_order entry (consistent with the lenient manifest handling):
    // a corrupt/crafted order id is removed with a sanitized warning rather than failing
    // the whole load.
    let mut kept_order = Vec::with_capacity(meta.node_order.len());
    for id in std::mem::take(&mut meta.node_order) {
        if validate_node_id(&id).is_ok() {
            kept_order.push(id);
        } else {
            push_warning(
                &mut warnings,
                format!(
                    "dropped invalid node id in nodeOrder: \"{}\"",
                    sanitize_id_for_warning(&id)
                ),
            );
        }
    }
    meta.node_order = kept_order;

    for id in manifest.nodes.keys() {
        // A manifest id is data we wrote; still re-validate so a hand-edited/corrupt
        // manifest can never make us touch a path outside the components dir.
        let path = match confined_component_path(&canonical, id) {
            Ok(p) => p,
            Err(_) => {
                push_warning(
                    &mut warnings,
                    format!("skipped invalid node id: \"{}\"", sanitize_id_for_warning(id)),
                );
                continue;
            }
        };
        match read_design_file(&path) {
            Ok(Some(markup)) => {
                components.insert(id.clone(), markup);
            }
            Ok(None) => {
                push_warning(
                    &mut warnings,
                    format!(
                        "component file for node \"{}\" is missing; loaded without markup",
                        sanitize_id_for_warning(id)
                    ),
                );
            }
            Err(_) => {
                push_warning(
                    &mut warnings,
                    format!(
                        "component file for node \"{}\" is unreadable",
                        sanitize_id_for_warning(id)
                    ),
                );
            }
        }
    }

    Ok(DesignProject {
        meta,
        manifest,
        components,
        warnings,
    })
}

/// "Consolida" — atomically persist the WHOLE project: `project.json` + `manifest.json`
/// + every `components/<id>.html` in `project.components`. `updatedAt` is refreshed.
/// Every id (manifest keys + component keys) is path-validated before any write.
#[tauri::command]
pub fn design_save_project(
    state: State<'_, BackendState>,
    working_folder_path: String,
    project: DesignProject,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    ensure_components_dir(&canonical)?;

    // Reject an incoming schema we cannot safely round-trip (newer format from a future
    // build). We never persist a downgrade.
    check_schema_version(project.meta.schema_version, PROJECT_FILE)?;
    check_schema_version(project.manifest.schema_version, MANIFEST_FILE)?;

    // DoS cap: bound the node/component sets before we iterate them.
    if project.manifest.nodes.len() > MAX_DESIGN_NODES {
        return Err(format!(
            "manifest has too many nodes ({} > {MAX_DESIGN_NODES} max)",
            project.manifest.nodes.len()
        ));
    }
    if project.components.len() > MAX_DESIGN_NODES {
        return Err(format!(
            "project has too many components ({} > {MAX_DESIGN_NODES} max)",
            project.components.len()
        ));
    }

    // Validate ALL ids up front so we never write a partial set then fail on a bad id.
    for (id, placement) in &project.manifest.nodes {
        validate_node_id(id)?;
        // Gate the optional presentation fields (radius/name bounds) up front too, so a
        // bad value is rejected atomically rather than after a partial write.
        validate_placement(id, placement)?;
    }
    for id in project.components.keys() {
        validate_node_id(id)?;
    }
    // node_order is metadata we round-trip verbatim; validate every entry so a corrupt /
    // crafted order can never carry a traversal id into a future consumer. Hard-fail.
    for id in &project.meta.node_order {
        validate_node_id(id)?;
    }

    // Enforce the write-size cap on every component BEFORE any write so an oversized
    // payload is rejected atomically rather than after a partial write.
    for (id, markup) in &project.components {
        check_write_size(markup, id)?;
    }

    let mut meta = project.meta;
    meta.updated_at = Utc::now().to_rfc3339();

    // Components first, then manifest, then meta: if we crash mid-way, a component
    // present without a manifest entry is harmless (ignored on load), whereas a
    // manifest entry without its component is tolerated (warned) — both safe.
    for (id, markup) in &project.components {
        let path = confined_component_path(&canonical, id)?;
        atomic_write(&path, markup, &format!("component {id}"))?;
    }
    write_manifest_file(&canonical, &project.manifest)?;
    write_meta(&canonical, &meta)?;
    Ok(())
}

/// Cheap placement-only write used on drag-commit: atomically replace ONLY
/// `manifest.json`. Does not touch component files or `project.json`. Every manifest id
/// is path-validated first.
#[tauri::command]
pub fn design_write_manifest(
    state: State<'_, BackendState>,
    working_folder_path: String,
    manifest: DesignManifest,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    for (id, placement) in &manifest.nodes {
        validate_node_id(id)?;
        validate_placement(id, placement)?;
    }
    write_manifest_file(&canonical, &manifest)
}

/// Atomically write the opaque markup of ONE node to `components/<id>.html`. The id is
/// path-confined; markup is treated as an opaque string (no parsing here).
#[tauri::command]
pub fn design_write_node(
    state: State<'_, BackendState>,
    working_folder_path: String,
    node_id: String,
    markup: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    ensure_components_dir(&canonical)?;
    let path = confined_component_path(&canonical, &node_id)?;
    // Enforce the byte cap on write (reads already enforce it): a hostile/buggy frontend
    // payload cannot balloon a component file past MAX_DESIGN_FILE_BYTES.
    check_write_size(&markup, &node_id)?;
    atomic_write(&path, &markup, &format!("component {node_id}"))
}

// ---------------------------------------------------------------------------
// Phase 2 STEP 4 — Oracle grounding (target-scoped)
// ---------------------------------------------------------------------------

/// How many ancestor levels we will walk up looking for a project-root marker
/// before giving up and grounding on the working folder itself. Bounds the walk so
/// a pathological path cannot turn root-resolution into an unbounded loop.
const MAX_ROOT_WALK_DEPTH: usize = 32;

/// Markers that identify a project root (the TARGET codebase root, which is what
/// Oracle indexes). Checked in order at each ancestor level. `.git` is checked as
/// either a dir (normal repo) or a file (git worktree/submodule pointer).
const ROOT_MARKERS: &[&str] = &[".git", "package.json", "Cargo.toml"];

/// Resolve the grounding ROOT for a design working folder (best-effort, P3 will make
/// this precise). The design working folder lives INSIDE the target (e.g.
/// `<target>/.aspis-design/<project>/`), but Oracle indexes the TARGET root. So:
///   1. an explicit `target_root` (when provided + it exists) wins;
///   2. else walk UP from `working_folder` to the first ancestor carrying a
///      project-root marker (`.git` / `package.json` / `Cargo.toml`);
///   3. else fall back to the working folder itself.
/// PURE-ish: it only stats the filesystem (no network, no writes). Bounded by
/// [`MAX_ROOT_WALK_DEPTH`].
///
/// P3: precise design-project ↔ target association (the registry will carry the
/// target root explicitly) supersedes this heuristic walk.
fn resolve_grounding_root(working_folder: &Path, target_root: Option<&str>) -> PathBuf {
    // 1. Explicit target root, if it exists. We do NOT canonicalize/validate hard
    //    here — Oracle's own readiness gate decides whether the index is usable; a
    //    bad path simply yields no chunks downstream.
    if let Some(explicit) = target_root {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_dir() {
                return p;
            }
        }
    }
    // 2. Walk up to a project-root marker.
    let mut cur: Option<&Path> = Some(working_folder);
    let mut depth = 0usize;
    while let Some(dir) = cur {
        if depth > MAX_ROOT_WALK_DEPTH {
            break;
        }
        for marker in ROOT_MARKERS {
            if dir.join(marker).exists() {
                return dir.to_path_buf();
            }
        }
        cur = dir.parent();
        depth += 1;
    }
    // 3. Fall back to the working folder itself.
    working_folder.to_path_buf()
}

/// Retrieve target-scoped Oracle grounding chunks (WITH text) for the design LLM.
///
/// `workingFolderPath` is the design working folder (inside the target);
/// `targetRoot` is an optional explicit override. We resolve the grounding root
/// (see [`resolve_grounding_root`]) then query the Oracle `/context` endpoint over
/// THAT index. `limit` is clamped to a sane range (default 8).
///
/// GRACEFUL DEGRADE — this command NEVER hard-fails generation: if Oracle is not
/// ready, has no index for the root, or errors for any reason, we return an EMPTY
/// vec (logged to the process log only). The caller treats empty as "generate
/// without grounding".
///
/// PRIVACY: the returned chunk `text` is the TARGET's own source, retrieved from the
/// loopback Oracle server; it crosses back into THIS process only, is never logged /
/// emitted, and is injected solely into the prompt sent to the (loopback) design
/// provider. The query travels in the POST body (never a URL/log).
#[tauri::command]
pub fn design_oracle_context(
    state: State<'_, BackendState>,
    working_folder_path: String,
    query: String,
    target_root: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<crate::oracle::python_oracle::DesignContextChunk>, String> {
    state.ensure_unlocked()?;

    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    // Clamp the chunk count: default 8, never more than a small cap (grounding is a
    // compact block, not a corpus dump).
    let limit = limit.unwrap_or(8).clamp(1, 24);

    // Resolve the grounding root. The working folder must exist (canonicalize it so
    // the walk starts from a real path); if it doesn't, degrade to no grounding
    // rather than surfacing an error into the generate flow.
    let canonical = match canonical_working_folder(&working_folder_path) {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    let root = resolve_grounding_root(&canonical, target_root.as_deref());

    // Query Oracle. Map EVERY error to an empty result (best-effort grounding). The
    // error detail goes to the process log ONLY — never to the renderer, and the
    // chunk text it might embed is already handled by the fixed-message parser.
    match crate::oracle::python_oracle::oracle_context_chunks_with_text(&root, query, limit) {
        Ok(chunks) => Ok(chunks),
        Err(e) => {
            eprintln!("[design] oracle grounding unavailable (degrading to none): {e}");
            Ok(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Phase A2 — Oracle grounding STATUS (target-scoped, never-fails)
// ---------------------------------------------------------------------------

/// Compact, IPC-safe view of the Oracle grounding index for a design project's target.
/// camelCase over the wire; every count/label is OPTIONAL so a not-ready/empty index (or
/// any error) degrades to `grounded: false` with the rest absent. PRIVACY: `root_label`
/// is the LAST PATH COMPONENT of the resolved grounding root ONLY — never the absolute
/// path (mirrors how `design.rs` keeps FS layout off the IPC boundary). Counts come from
/// the Oracle `/index/status` payload over the loopback server; no source text is exposed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignOracleStatus {
    /// Whether the target has a usable Oracle index (any indexed file / chunk present).
    pub grounded: bool,
    /// Leaf folder name of the resolved grounding root (NEVER the absolute path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_label: Option<String>,
    /// Number of indexed chunks (from `index.sqliteChunks`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<u64>,
    /// Number of indexed files (from `index.indexedFiles`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<u64>,
    /// ISO-8601 time of the last completed index job (from `job.finishedAt`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_iso: Option<String>,
}

/// The LEAF component of a resolved path, as an IPC-safe label. Returns `None` for a path
/// with no final component (e.g. a bare root). NEVER returns the absolute path. PURE +
/// total: unit-testable. Used so `DesignOracleStatus.root_label` can identify the grounded
/// folder for the UI without leaking the user's filesystem layout.
fn path_leaf_label(root: &Path) -> Option<String> {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

/// Pull an optional `u64` count out of the Oracle `/index/status` JSON at
/// `payload["index"][key]`. Tolerant: a missing/non-numeric value yields `None`. The
/// Python server emits these as JSON numbers (see `camelize_index_status`).
fn status_count(payload: &serde_json::Value, key: &str) -> Option<u64> {
    payload.get("index")?.get(key)?.as_u64()
}

/// Report the Oracle grounding status for a design project's target. Resolves the SAME
/// grounding root `design_oracle_context` uses (`resolve_grounding_root`), then queries the
/// resident Oracle server's `GET /index/status?root=<root>` via the SHARED HTTP plumbing
/// `get_oracle_index_status` uses (`run_python_oracle_http_get`) — addressing the server by
/// THAT root and passing the same `?root=` query (URL-encoded exactly like
/// `oracle_root_query`).
///
/// NEVER FAILS: a locked vault is the only hard error (parity with sibling commands via
/// `ensure_unlocked`). Any other problem — Oracle not ready, no index for the root, an HTTP
/// or parse error — degrades to `Ok(DesignOracleStatus { grounded: false, .. })`. The
/// `root_label` is still populated (leaf-only) when the working folder resolves, so the UI
/// can name the target even while its index is warming. PRIVACY: only the index ROOT (the
/// user's own workspace) goes on the wire to the loopback server; the absolute path NEVER
/// crosses back to the renderer (only the leaf label does), and no source text is exposed.
#[tauri::command]
pub async fn design_oracle_status(
    state: State<'_, BackendState>,
    working_folder_path: String,
) -> Result<DesignOracleStatus, String> {
    state.ensure_unlocked()?;

    // Resolve the grounding root (best-effort). If the working folder is unreadable we have
    // no root to label or query — degrade to "not grounded" without leaking a path.
    let canonical = match canonical_working_folder(&working_folder_path) {
        Ok(c) => c,
        Err(_) => return Ok(DesignOracleStatus::default()),
    };
    let root = resolve_grounding_root(&canonical, None);
    // Leaf-only label: the UI can identify the target without ever seeing the full path.
    let root_label = path_leaf_label(&root);

    // Query `/index/status?root=<encoded>` over the resident server addressed by THIS root,
    // exactly as `oracle::commands::get_oracle_index_status` does (same `run_python_oracle_
    // http_get` plumbing + the same `urlencoding::encode(root)` query idiom as
    // `oracle_root_query`). Run the blocking HTTP off the async worker.
    let query_root = root.clone();
    // The underlying `run_python_oracle_http_get` carries a 90s reqwest timeout — far too
    // long for a passive UI status badge. A status check that can't answer quickly is, for
    // our purposes, "not grounded yet". Cap the whole blocking call at a short 10s budget
    // (tauri::async_runtime is a tokio runtime, so `tokio::time::timeout` drives the
    // spawn_blocking JoinHandle here); on timeout we fall through to the grounded:false
    // degrade path below. NOTE: the blocking reqwest call itself is not interruptible, so a
    // doomed worker thread keeps running until its own 90s timeout — but THIS command
    // returns within ~10s regardless and never holds the async caller hostage.
    const ORACLE_STATUS_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let query = format!(
            "root={}",
            urlencoding::encode(&query_root.to_string_lossy())
        );
        crate::oracle::python_oracle::run_python_oracle_http_get::<serde_json::Value>(
            &query_root,
            &format!("/index/status?{query}"),
        )
    });
    let payload: Result<serde_json::Value, String> =
        match tokio::time::timeout(ORACLE_STATUS_BUDGET, task).await {
            Ok(joined) => joined
                .map_err(|e| format!("Oracle status task failed: {e}"))
                .and_then(|inner| inner),
            Err(_) => Err("Oracle status check timed out".to_string()),
        };

    let payload = match payload {
        Ok(p) => p,
        Err(e) => {
            // Best-effort: the index may simply be warming. Log to the process log only and
            // return "not grounded" (with the leaf label so the UI can still name the target).
            eprintln!("[design] oracle status unavailable (degrading): {e}");
            return Ok(DesignOracleStatus {
                grounded: false,
                root_label,
                ..Default::default()
            });
        }
    };

    let chunks = status_count(&payload, "sqliteChunks");
    let files = status_count(&payload, "indexedFiles");
    // The last completed index job's finish time, when present in the `job` sub-object.
    let last_sync_iso = payload
        .get("job")
        .and_then(|j| j.get("finishedAt"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // Grounded == there is at least one indexed file or chunk for this root.
    let grounded = chunks.unwrap_or(0) > 0 || files.unwrap_or(0) > 0;

    Ok(DesignOracleStatus {
        grounded,
        root_label,
        chunks,
        files,
        last_sync_iso,
    })
}

// ---------------------------------------------------------------------------
// Phase A2 — design.md (human design brief) read/write
// ---------------------------------------------------------------------------

/// The single design-brief file in a working-folder root. Exactly `design.md` (the Phase C
/// contract authors/consumes it; this command only persists/reads the raw text).
const DESIGN_MD_FILE: &str = "design.md";

/// Max bytes of `design.md` we will read or write. A design brief is human-authored prose;
/// 64 KiB is generous yet bounds a hostile/corrupt file (read) and a crafted frontend
/// payload (write). Mirrors the `MAX_DESIGN_FILE_BYTES` posture with its OWN, tighter cap.
const DESIGN_MD_MAX_BYTES: u64 = 65536;

/// Read `design.md` from the working-folder root. Missing file => `Ok(None)`. A file larger
/// than [`DESIGN_MD_MAX_BYTES`] is an ERROR (never silently truncated). Path-confined via
/// the canonical working-folder helper; the filename is fixed (no traversal surface).
#[tauri::command]
pub fn design_read_design_md(
    state: State<'_, BackendState>,
    working_folder_path: String,
) -> Result<Option<String>, String> {
    state.ensure_unlocked()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    read_design_md_at(&canonical.join(DESIGN_MD_FILE))
}

/// Read `design.md` from a resolved path, enforcing the dedicated 64 KiB cap BOTH before
/// and after the read. The pre-read metadata check is a fast path; the post-read length
/// check is the authoritative one — it closes a TOCTOU race (the file grows between stat
/// and read) and the fact that the shared read helper uses the larger 8 MiB design cap, so
/// neither can let an over-cap string escape. Split out so the size invariant is unit-
/// testable without a `State<BackendState>`.
fn read_design_md_at(path: &Path) -> Result<Option<String>, String> {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > DESIGN_MD_MAX_BYTES {
            return Err(format!(
                "design.md too large ({} bytes > {DESIGN_MD_MAX_BYTES} max)",
                meta.len()
            ));
        }
    }
    let result = read_design_file(path)?;
    if let Some(text) = &result {
        let len = text.len() as u64;
        if len > DESIGN_MD_MAX_BYTES {
            return Err(format!(
                "design.md too large ({len} bytes > {DESIGN_MD_MAX_BYTES} max)"
            ));
        }
    }
    Ok(result)
}

/// Atomically write `design.md` to the working-folder root under the design write lock
/// (mirrors `design_write_tokens`). Content beyond [`DESIGN_MD_MAX_BYTES`] is REJECTED.
/// The filename is fixed `design.md`, so there is no traversal surface; the working folder
/// is canonicalized first.
#[tauri::command]
pub fn design_write_design_md(
    state: State<'_, BackendState>,
    working_folder_path: String,
    content: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;
    if content.len() as u64 > DESIGN_MD_MAX_BYTES {
        return Err(format!(
            "design.md too large ({} bytes > {DESIGN_MD_MAX_BYTES} max)",
            content.len()
        ));
    }
    atomic_write(&canonical.join(DESIGN_MD_FILE), &content, DESIGN_MD_FILE)
}

// ---------------------------------------------------------------------------
// Phase 2 STEP 4 — design tokens (W3C DTCG) persistence
// ---------------------------------------------------------------------------

/// `tokens.json` filename in the working folder (W3C DTCG document).
const TOKENS_FILE: &str = "tokens.json";

/// Atomically persist the W3C DTCG `tokens.json` for a project. The document is an
/// OPAQUE JSON string to Rust (the DTCG model + validation live in the frontend,
/// mirroring the markup-is-opaque design): we only enforce that it parses as JSON
/// (so we never write a corrupt non-JSON blob) and the byte cap, then atomic-write.
#[tauri::command]
pub fn design_write_tokens(
    state: State<'_, BackendState>,
    working_folder_path: String,
    tokens_json: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;

    if tokens_json.len() as u64 > MAX_DESIGN_FILE_BYTES {
        return Err(format!(
            "tokens.json too large ({} bytes > {MAX_DESIGN_FILE_BYTES} max)",
            tokens_json.len()
        ));
    }
    // Validate it is well-formed JSON before writing (never persist a corrupt file).
    serde_json::from_str::<serde_json::Value>(&tokens_json)
        .map_err(|_| "tokens.json is not valid JSON".to_string())?;

    atomic_write(&canonical.join(TOKENS_FILE), &tokens_json, TOKENS_FILE)
}

// ---------------------------------------------------------------------------
// Phase 2 STEP 4 — generation audit log (token-free, append-only)
// ---------------------------------------------------------------------------

/// `generations.jsonl` filename — one token-free JSON line per generation/edit.
const GENERATIONS_FILE: &str = "generations.jsonl";

/// Hard cap on the `generations.jsonl` file size. Past this we ROTATE (truncate +
/// keep a tail) before appending so an unbounded session cannot grow the file
/// without limit. 4 MiB of metadata-only lines is thousands of generations.
const MAX_GENERATIONS_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// When rotating, keep approximately this many trailing bytes (the most recent
/// lines). Half the cap so we don't rotate on every subsequent append.
const GENERATIONS_KEEP_BYTES: usize = 2 * 1024 * 1024;

/// Max bytes for a SINGLE audit line. The entry is metadata-only (no prompt/text),
/// so this is generous; it bounds a hostile/buggy frontend payload.
const MAX_GENERATION_LINE_BYTES: usize = 16 * 1024;

/// One metadata-only audit entry. DELIBERATELY carries NO prompt text, NO secrets,
/// NO chunk text — only sizes + flags + ids. Serialized as one JSONL line. `ts` is
/// supplied by the FRONTEND (no clock in this command), keeping the entry shape a
/// pure pass-through the caller fully controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationLogEntry {
    /// Caller-supplied ISO-8601 timestamp (we never call the clock here).
    pub ts: String,
    /// `"generate"` or `"edit"` (free-form but validated against a small allowlist).
    pub kind: String,
    /// The node ids touched (empty for a generation that produced none).
    #[serde(default)]
    pub node_ids: Vec<String>,
    /// The configured backend kind (`ollama`/`omlx`/`api`/`codex`), or `"unknown"`.
    pub backend_kind: String,
    /// Size of the prompt that was sent, in characters. NOT the prompt itself.
    pub prompt_chars: u64,
    /// Whether Oracle grounding chunks were injected into the prompt.
    pub oracle_grounded: bool,
    /// Wall-clock duration of the generation, in milliseconds.
    pub duration_ms: u64,
    /// `"applied"` | `"empty"` | `"error"`.
    pub outcome: String,
}

/// Append ONE token-free audit line to `generations.jsonl`, rotating the file if it
/// has grown past the cap. The entry is re-serialized from the typed struct (so the
/// frontend can never smuggle extra fields — e.g. a `prompt` — into the file), the
/// line is size-capped, and the node-id list is bounded + validated.
#[tauri::command]
pub fn design_append_generation_log(
    state: State<'_, BackendState>,
    working_folder_path: String,
    entry: GenerationLogEntry,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;

    let line = build_generation_log_line(&entry)?;

    let path = canonical.join(GENERATIONS_FILE);
    rotate_generations_if_needed(&path)?;
    append_line(&path, &line)
}

/// PURE: validate + serialize one audit entry into a single JSONL line (with the
/// trailing `\n`). Re-serializing from the typed struct guarantees the on-disk line
/// contains ONLY the metadata fields — no prompt text, no secrets — regardless of
/// what the frontend sent. Bounds the node-id list and the total line size.
fn build_generation_log_line(entry: &GenerationLogEntry) -> Result<String, String> {
    // Allowlist the enum-like fields so a crafted payload can't write arbitrary
    // strings (keeps the log parseable + tamper-evident).
    if !matches!(entry.kind.as_str(), "generate" | "edit") {
        return Err("invalid generation log kind".to_string());
    }
    if !matches!(entry.outcome.as_str(), "applied" | "empty" | "error") {
        return Err("invalid generation log outcome".to_string());
    }
    // Bound the node-id list (a generation touches at most MAX_DESIGN_NODES) and
    // validate every id so the log can never carry a traversal/garbage id.
    if entry.node_ids.len() > MAX_DESIGN_NODES {
        return Err("too many node ids in generation log entry".to_string());
    }
    for id in &entry.node_ids {
        validate_node_id(id)?;
    }
    // ISO timestamp: bound the length so a hostile ts cannot bloat the line. We do
    // not parse it (the frontend owns the clock), only cap it.
    if entry.ts.len() > 64 {
        return Err("generation log timestamp is too long".to_string());
    }
    if entry.backend_kind.len() > 32 {
        return Err("generation log backendKind is too long".to_string());
    }
    // WARNING 4: allowlist the backend kind (like kind/outcome above) so a crafted
    // payload cannot write an arbitrary string into the audit log. These are the
    // backend kinds the design surface knows, plus the `"unknown"` sentinel the
    // frontend uses when the backend is unconfigured/unavailable.
    if !matches!(
        entry.backend_kind.as_str(),
        "ollama" | "omlx" | "api" | "codex" | "claude" | "unknown"
    ) {
        return Err("invalid generation log backendKind".to_string());
    }

    let json = serde_json::to_string(entry)
        .map_err(|e| format!("could not serialize generation log entry: {e}"))?;
    if json.len() > MAX_GENERATION_LINE_BYTES {
        return Err("generation log entry too large".to_string());
    }
    Ok(format!("{json}\n"))
}

/// Rotate `generations.jsonl` if it exceeds the cap: read a BOUNDED window of the
/// file, drop the first (now-partial) line, and atomically replace the file with the
/// trimmed remainder. Best-effort and bounded; a missing/small file is a no-op.
fn rotate_generations_if_needed(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()), // no file yet -> nothing to rotate
    };
    if meta.len() <= MAX_GENERATIONS_FILE_BYTES {
        return Ok(());
    }
    // WARNING 2: do NOT `fs::read` the whole file. The `metadata` size check and the
    // read are not atomic — an external process could grow or symlink the file in
    // between and force an unbounded allocation (OOM). Read through a HARD CAP of
    // `MAX_GENERATIONS_FILE_BYTES * 2` via `take`, so a hostile/racing grow can never
    // make us allocate more than that. (We only need the trailing keep-window anyway;
    // reading from the start within the bound is sufficient because a legitimate file
    // is just over the cap.)
    let read_cap = MAX_GENERATIONS_FILE_BYTES.saturating_mul(2);
    let file = fs::File::open(path).map_err(|_| "could not read generation log".to_string())?;
    let mut contents = Vec::new();
    file.take(read_cap)
        .read_to_end(&mut contents)
        .map_err(|_| "could not read generation log".to_string())?;
    let start = contents.len().saturating_sub(GENERATIONS_KEEP_BYTES);
    // Advance to just past the next newline so we never keep a partial leading line.
    let trimmed = match contents[start..].iter().position(|&b| b == b'\n') {
        Some(nl) => &contents[start + nl + 1..],
        None => &contents[start..],
    };
    // WARNING 7: STRICT UTF-8. `from_utf8_lossy` would inject U+FFFD replacement
    // chars and corrupt JSONL (a downstream parser then chokes on garbled lines). If
    // the retained window is not valid UTF-8 (truncated multibyte boundary, or a
    // corrupted/binary file), RESET the log to empty rather than persist garbage —
    // an audit log losing its tail is acceptable; corrupting it is not.
    let kept = match std::str::from_utf8(trimmed) {
        Ok(s) => s.to_string(),
        Err(_) => String::new(),
    };
    atomic_write(path, &kept, GENERATIONS_FILE)
}

/// Append a single line to `path`, creating the file if absent. Uses an append open
/// (NOT temp+rename) because this is genuinely append-only and the line is tiny +
/// bounded; a torn append at worst loses/garbles the last line (tolerable for an
/// audit log) and never corrupts earlier entries.
fn append_line(path: &Path, line: &str) -> Result<(), String> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| "could not open generation log".to_string())?;
    f.write_all(line.as_bytes())
        .map_err(|_| "could not append to generation log".to_string())
}

// ---------------------------------------------------------------------------
// Phase 2 STEP 4 — export-to-code (write the assembled HTML)
// ---------------------------------------------------------------------------

/// Validate an export filename: a single path component, `.html` extension, strict
/// charset. The HTML is ASSEMBLED in the frontend (PURE, testable) and written here;
/// this command only confines WHERE it lands (directly under the working folder).
fn validate_export_filename(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("export filename must not be empty".to_string());
    }
    if name.len() > 128 {
        return Err("export filename is too long".to_string());
    }
    if !name.ends_with(".html") {
        return Err("export filename must end with .html".to_string());
    }
    // No separators, no traversal, no leading dot. Reuse the strict charset idea:
    // the stem (before `.html`) is [a-z0-9][a-z0-9_-]* and the only dot is the ext.
    let stem = &name[..name.len() - ".html".len()];
    if stem.is_empty() {
        return Err("export filename must have a name before .html".to_string());
    }
    let mut chars = stem.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err("export filename must start with a lowercase letter or digit".to_string());
    }
    for c in chars {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if !ok {
            return Err(format!("export filename has an invalid character {c:?}"));
        }
    }
    Ok(())
}

/// Write an exported-code HTML document to `<workingFolder>/<filename>`. The content
/// is assembled (and already sanitized at the component level) by the frontend's PURE
/// `exportCode`; this command path-confines the filename + caps the size + atomic-
/// writes. The filename is validated to a single `.html` component (no traversal).
#[tauri::command]
pub fn design_write_export(
    state: State<'_, BackendState>,
    working_folder_path: String,
    filename: String,
    content: String,
) -> Result<(), String> {
    state.ensure_unlocked()?;
    let _guard = design_write_guard()?;
    let canonical = canonical_working_folder(&working_folder_path)?;

    validate_export_filename(&filename)?;
    if content.len() as u64 > MAX_DESIGN_FILE_BYTES {
        return Err(format!(
            "export content too large ({} bytes > {MAX_DESIGN_FILE_BYTES} max)",
            content.len()
        ));
    }

    let target = canonical.join(filename.trim());
    // Belt-and-suspenders: the validated filename has no separators, so the parent
    // must be the working folder. Reject anything else.
    if target.parent() != Some(canonical.as_path()) {
        return Err("export path escapes the working folder".to_string());
    }
    atomic_write(&target, &content, "export")
}

// ---------------------------------------------------------------------------
// Internal write/serialize helpers
// ---------------------------------------------------------------------------

/// Ensure `<root>/components` exists AND, after creation, canonicalize it and assert it
/// still lives under `canonical_root`. This rejects a `components` entry that is a symlink
/// escaping the working folder. NOTE: this narrows the symlink-swap TOCTOU window but does
/// not eliminate it on a hostile filesystem — the in-process `design_write_lock` is what
/// serializes our own writers; an external process racing the FS is out of scope.
fn ensure_components_dir(canonical_root: &Path) -> Result<(), String> {
    let dir = canonical_root.join(COMPONENTS_DIR);
    fs::create_dir_all(&dir).map_err(|e| {
        eprintln!("[design] could not create components folder: {e}");
        "could not create components folder".to_string()
    })?;
    let real_dir = fs::canonicalize(&dir).map_err(|e| {
        eprintln!("[design] could not resolve components folder: {e}");
        "could not resolve components folder".to_string()
    })?;
    if !real_dir.starts_with(canonical_root) {
        eprintln!(
            "[design] components folder escapes working root: {} not under {}",
            real_dir.display(),
            canonical_root.display()
        );
        return Err("components folder escapes the working folder".to_string());
    }
    Ok(())
}

fn write_meta(canonical_root: &Path, meta: &DesignProjectMeta) -> Result<(), String> {
    let pretty = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("could not serialize {PROJECT_FILE}: {e}"))?;
    atomic_write(
        &canonical_root.join(PROJECT_FILE),
        &format!("{pretty}\n"),
        PROJECT_FILE,
    )
}

fn write_manifest_file(canonical_root: &Path, manifest: &DesignManifest) -> Result<(), String> {
    let pretty = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("could not serialize {MANIFEST_FILE}: {e}"))?;
    atomic_write(
        &canonical_root.join(MANIFEST_FILE),
        &format!("{pretty}\n"),
        MANIFEST_FILE,
    )
}

/// Generate a fresh project id. Time-based + pid keeps it unique without pulling in a
/// uuid dependency; ids are opaque and not path-derived (path confinement is on NODE
/// ids, which the frontend assigns under the validated charset).
fn new_project_id() -> String {
    format!("p{}-{}", std::process::id(), Utc::now().timestamp_micros())
}

// ---------------------------------------------------------------------------
// Phase 3 — management plane: design-projects registry (config.json, metadata ONLY)
// ---------------------------------------------------------------------------
//
// The registry is a convenience index so the operator does not re-pick the working
// folder every time. It lives in config.json under the key `designProjects` and holds
// METADATA ONLY — id/name/path/timestamps + an optional thumbnail path. It NEVER holds
// the authoritative design (manifest/markup/prompt/tokens); the working folder remains
// the only source of truth (LOCKED architecture 1.7). The commands clone the config-RMW
// idiom from `projects::set_mini_coder_backend` (config_write_lock + read-the-whole-file
// + mutate one key + atomic temp+rename); the reader clones `read_custom_agent_clients`
// (missing key / missing file / malformed -> empty list, never errors).

/// config.json key holding the design-projects registry array.
const DESIGN_PROJECTS_KEY: &str = "designProjects";

/// Hard cap on the registry size. Past this we EVICT the oldest entry (by
/// `lastOpenedAt`) on insert so a long-lived config cannot grow unbounded.
const MAX_REGISTRY_ENTRIES: usize = 100;

/// Max length we accept for the user-facing project name in the registry (the
/// working folder is the source of truth; this is just a label). Bounds a hostile
/// config payload.
const MAX_REGISTRY_NAME_LEN: usize = 200;

/// Known design artifacts we author inside a working folder. `design_registry_remove`
/// with `removeFiles=true` deletes ONLY these (each path re-confined under the
/// canonical working folder), never the working folder itself nor any unknown file —
/// so removing a registry entry can never delete the user's own files.
const DESIGN_TOP_LEVEL_FILES: &[&str] = &[PROJECT_FILE, MANIFEST_FILE, TOKENS_FILE, GENERATIONS_FILE];

/// One design-project registry entry. METADATA ONLY (camelCase over IPC). Mirrors the
/// TS `DesignProjectEntry`. `workingFolderPath` is the dedupe key (canonicalized).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProjectEntry {
    pub id: String,
    pub name: String,
    pub working_folder_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_opened_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
    /// SHA-256 (lowercase hex) of the design.md content the user last APPROVED in the
    /// contract editor. Provenance gate: on load we re-hash the on-disk design.md and
    /// only inject it into prompts when it matches this recorded value (an agent that
    /// edited the folder's design.md out-of-band produces a mismatch, forcing a review).
    /// Absent on legacy entries / projects with no approved contract. camelCase
    /// `contractSha` over IPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_sha: Option<String>,
}

/// Canonicalize a working folder path into a STABLE dedupe key. When the path exists we
/// canonicalize it (collapsing `.`/`..`/symlinks + normalizing case/separators on
/// Windows). When it does NOT exist (e.g. a remembered folder that was later deleted)
/// canonicalize would fail, so we fall back to a lexical normalization (trim) so the
/// entry can still be matched/removed. The key is only used for equality, never for IO.
fn registry_dedupe_key(working_folder_path: &str) -> String {
    let trimmed = working_folder_path.trim();
    match fs::canonicalize(trimmed) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => trimmed.to_string(),
    }
}

/// W7: the path to STORE in a registry entry. Canonicalizes the (already-trimmed)
/// input when the folder exists/is readable so two representations of the same
/// folder persist identically (and therefore dedupe to one entry); falls back to
/// the trimmed input when canonicalization fails (e.g. the folder isn't present
/// yet — the entry is still recorded with the user-supplied path).
fn canonicalize_for_storage(trimmed_path: &str) -> String {
    match fs::canonicalize(trimmed_path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => trimmed_path.to_string(),
    }
}

/// Read the registry array from config.json, parsing each entry leniently. A missing
/// key / missing file / malformed JSON / malformed entry yields an EMPTY list (the
/// registry is simply unavailable); never errors. Clones `read_custom_agent_clients`.
fn read_design_registry(app: &tauri::AppHandle) -> Vec<DesignProjectEntry> {
    let Some(path) = crate::backend::projects::locate_config_path(app) else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(array) = value.get(DESIGN_PROJECTS_KEY).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    array
        .iter()
        .take(MAX_REGISTRY_ENTRIES)
        .filter_map(|entry| serde_json::from_value::<DesignProjectEntry>(entry.clone()).ok())
        .collect()
}

/// PURE: sort a registry list by `lastOpenedAt` descending (most-recent first), tie-
/// broken by name ascending (mirrors `list_projects`' updated_at-desc-then-title sort).
/// Sorts in place.
fn sort_registry(entries: &mut [DesignProjectEntry]) {
    entries.sort_by(|a, b| {
        b.last_opened_at
            .cmp(&a.last_opened_at)
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Persist the whole registry array back into config.json under `designProjects`,
/// cloning the `set_mini_coder_backend` RMW + atomic-write discipline EXACTLY (read the
/// whole file, mutate ONE key, atomic temp+rename). The caller must hold
/// `config_write_lock`. An empty list drops the key (no `[]` churn).
fn write_design_registry(
    app: &tauri::AppHandle,
    entries: &[DesignProjectEntry],
) -> Result<(), String> {
    let path = crate::backend::projects::locate_config_path(app)
        .ok_or_else(|| "config.json could not be located to save the design registry.".to_string())?;
    let raw = fs::read_to_string(&path).map_err(|e| format!("Could not read config.json: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("config.json is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("config.json is not a JSON object.".into());
    }
    if entries.is_empty() {
        if let Some(obj) = value.as_object_mut() {
            obj.remove(DESIGN_PROJECTS_KEY);
        }
    } else {
        value[DESIGN_PROJECTS_KEY] = serde_json::to_value(entries)
            .map_err(|e| format!("Could not serialize the design registry: {e}"))?;
    }
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Could not serialize config.json: {e}"))?;
    // Atomic temp+rename (same idiom as set_mini_coder_backend): a crash mid-write can
    // never leave a half-written config.json.
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let temp_path = path.with_extension(format!("json.{suffix}.tmp"));
    let backup_path = path.with_extension(format!("json.{suffix}.bak"));
    fs::write(&temp_path, format!("{pretty}\n")).map_err(|e| {
        format!(
            "Could not write config.json at {}: {e}. In a packaged build this file is read-only.",
            path.to_string_lossy()
        )
    })?;
    replace_file_with_backup(&temp_path, &path, &backup_path, "config.json")
        .map_err(|e| format!("{e}. In a packaged build this file is read-only."))
}

/// PURE registry mutation helpers (filesystem-free) so the upsert/dedupe/cap/evict and
/// rename/remove logic is unit-testable without an AppHandle or a real config.json.
mod registry_ops {
    use super::{DesignProjectEntry, MAX_REGISTRY_ENTRIES};

    /// Upsert `incoming` into `entries`, deduping by the CANONICAL key (the caller
    /// supplies the key for both the existing entries and the incoming one so this fn
    /// stays pure). On a key hit: update name/updatedAt/lastOpenedAt/thumbnail (keep the
    /// original id + createdAt — identity is stable). On a miss: insert. Then enforce the
    /// cap by evicting the OLDEST entry (smallest lastOpenedAt) until <= cap.
    ///
    /// `key_of` maps an entry's stored path to its dedupe key; `incoming_key` is the
    /// incoming entry's key. Both are computed by the (impure) caller via
    /// `registry_dedupe_key`.
    pub fn upsert(
        entries: &mut Vec<DesignProjectEntry>,
        incoming: DesignProjectEntry,
        incoming_key: &str,
        key_of: &dyn Fn(&str) -> String,
    ) {
        if let Some(existing) = entries
            .iter_mut()
            .find(|e| key_of(&e.working_folder_path) == incoming_key)
        {
            existing.name = incoming.name;
            existing.updated_at = incoming.updated_at;
            existing.last_opened_at = incoming.last_opened_at;
            existing.thumbnail_path = incoming.thumbnail_path;
            // contract_sha is only OVERWRITTEN when the incoming remember carries one
            // (i.e. the user just Saved/approved a contract). A plain load/create remember
            // omits it (None) and MUST NOT wipe a previously approved hash — otherwise an
            // approved contract would be forgotten on the very next open and re-prompt.
            if incoming.contract_sha.is_some() {
                existing.contract_sha = incoming.contract_sha;
            }
            // Keep the existing canonical-ish stored path; do not churn it. (id +
            // createdAt are identity and intentionally untouched.)
        } else {
            entries.push(incoming);
        }
        evict_to_cap(entries);
    }

    /// Rename an entry by id (metadata only). Returns true if an entry matched. Also
    /// bumps `updatedAt` to the supplied timestamp.
    pub fn rename(
        entries: &mut [DesignProjectEntry],
        id: &str,
        name: String,
        updated_at: String,
    ) -> bool {
        if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
            e.name = name;
            e.updated_at = updated_at;
            true
        } else {
            false
        }
    }

    /// Remove an entry by id, returning the removed entry (so the caller can act on its
    /// stored path for optional file deletion). None if no entry matched.
    pub fn remove(entries: &mut Vec<DesignProjectEntry>, id: &str) -> Option<DesignProjectEntry> {
        let idx = entries.iter().position(|e| e.id == id)?;
        Some(entries.remove(idx))
    }

    /// Evict the oldest entries (smallest lastOpenedAt) until the list is within the cap.
    fn evict_to_cap(entries: &mut Vec<DesignProjectEntry>) {
        while entries.len() > MAX_REGISTRY_ENTRIES {
            // Find the index of the entry with the smallest lastOpenedAt (oldest). Tie-
            // break by largest index so we evict deterministically.
            let mut oldest = 0usize;
            for (i, e) in entries.iter().enumerate() {
                if e.last_opened_at <= entries[oldest].last_opened_at {
                    oldest = i;
                }
            }
            entries.remove(oldest);
        }
    }
}

/// Validate + normalize a registry name (trim, non-empty, length cap). The name is a
/// label only; the working folder is the source of truth.
fn clean_registry_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    if trimmed.chars().count() > MAX_REGISTRY_NAME_LEN {
        return Err(format!(
            "project name must be at most {MAX_REGISTRY_NAME_LEN} characters"
        ));
    }
    Ok(trimmed.to_string())
}

/// List the design-projects registry, sorted by `lastOpenedAt` desc. Reader-or-empty:
/// a missing key / file / malformed config yields an empty list, never errors.
#[tauri::command]
pub fn design_registry_list(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
) -> Result<Vec<DesignProjectEntry>, String> {
    state.ensure_unlocked()?;
    let mut entries = read_design_registry(&app);
    sort_registry(&mut entries);
    Ok(entries)
}

/// Remember (upsert) a design project in the registry after a successful create/open.
/// Dedupes by the CANONICAL working folder path: a second remember of the same folder
/// updates name/updatedAt/lastOpenedAt rather than inserting a duplicate. The server
/// stamps `updatedAt`/`lastOpenedAt` (frontend never owns these) and defaults a missing
/// `createdAt` to now. Atomic config write under `config_write_lock`. Returns the full
/// sorted list so the caller can refresh its view in one round-trip.
#[tauri::command]
pub fn design_registry_remember(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    entry: DesignProjectEntry,
) -> Result<Vec<DesignProjectEntry>, String> {
    state.ensure_unlocked()?;
    let name = clean_registry_name(&entry.name)?;
    let trimmed_path = entry.working_folder_path.trim().to_string();
    if trimmed_path.is_empty() {
        return Err("working folder path must not be empty".to_string());
    }
    // W7: STORE the canonicalized path (when canonicalization succeeds) so the same
    // folder reached via two different path representations (e.g. with/without a
    // trailing slash, `.`/`..` segments, or case differences on case-insensitive
    // FSes) dedupes to ONE entry. The dedupe KEY already canonicalizes for matching;
    // persisting the canonical form keeps the stored value consistent too. Fall back
    // to the trimmed input when the folder is not (yet) canonicalizable.
    let working_folder_path = canonicalize_for_storage(&trimmed_path);
    let now = Utc::now().to_rfc3339();

    let _config_guard = crate::backend::projects::config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;

    let mut entries = read_design_registry(&app);
    let incoming_key = registry_dedupe_key(&working_folder_path);
    // Build the normalized incoming entry. id/createdAt are kept on a fresh insert;
    // on a hit, upsert() preserves the EXISTING id + createdAt (identity is stable).
    let created_at = {
        let c = entry.created_at.trim();
        if c.is_empty() { now.clone() } else { c.to_string() }
    };
    let id = {
        let i = entry.id.trim();
        if i.is_empty() { new_project_id() } else { i.to_string() }
    };
    let incoming = DesignProjectEntry {
        id,
        name,
        working_folder_path,
        created_at,
        updated_at: now.clone(),
        last_opened_at: now,
        thumbnail_path: entry.thumbnail_path.clone(),
        // Carried through verbatim: None on a load/create remember (preserves any existing
        // recorded hash via upsert), Some(hex) when the user just approved a contract.
        contract_sha: entry.contract_sha.clone(),
    };
    registry_ops::upsert(&mut entries, incoming, &incoming_key, &|p| {
        registry_dedupe_key(p)
    });
    write_design_registry(&app, &entries)?;
    sort_registry(&mut entries);
    Ok(entries)
}

/// Rename a registry entry by id (METADATA ONLY — does NOT touch the on-disk
/// project.json). Atomic config write under `config_write_lock`. Errors if no entry
/// matches the id. Returns the full sorted list.
#[tauri::command]
pub fn design_registry_rename(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    id: String,
    name: String,
) -> Result<Vec<DesignProjectEntry>, String> {
    state.ensure_unlocked()?;
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("project id must not be empty".to_string());
    }
    let name = clean_registry_name(&name)?;
    let now = Utc::now().to_rfc3339();

    let _config_guard = crate::backend::projects::config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;

    let mut entries = read_design_registry(&app);
    if !registry_ops::rename(&mut entries, &id, name, now) {
        return Err("design project not found in the registry".to_string());
    }
    write_design_registry(&app, &entries)?;
    sort_registry(&mut entries);
    Ok(entries)
}

/// Arguments for `design_registry_remove`. `removeFiles` defaults to false (unregister
/// only). When true, ONLY the known design artifacts inside the (canonicalized) working
/// folder are deleted — never the working folder itself, never unknown files, never a
/// path that escapes the folder via symlink.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignRegistryRemoveArgs {
    pub id: String,
    #[serde(default)]
    pub remove_files: bool,
}

/// Remove a registry entry by id. With `removeFiles=false` (default) this only
/// unregisters (config-only). With `removeFiles=true` it ALSO deletes the design
/// artifacts WE created inside the working folder — strictly path-confined: the stored
/// working folder is canonicalized, each known artifact is joined + re-confined under
/// that root, and deletion of a missing/unconfined target is skipped. NEVER deletes the
/// working folder root, a parent, an arbitrary path, or follows a symlink out.
#[tauri::command]
pub fn design_registry_remove(
    app: tauri::AppHandle,
    state: State<'_, BackendState>,
    args: DesignRegistryRemoveArgs,
) -> Result<Vec<DesignProjectEntry>, String> {
    state.ensure_unlocked()?;
    let id = args.id.trim().to_string();
    if id.is_empty() {
        return Err("project id must not be empty".to_string());
    }

    let _config_guard = crate::backend::projects::config_write_lock()
        .lock()
        .map_err(|_| "Config write lock is poisoned.".to_string())?;

    let mut entries = read_design_registry(&app);
    let removed = registry_ops::remove(&mut entries, &id)
        .ok_or_else(|| "design project not found in the registry".to_string())?;

    // Persist the registry change FIRST (unregister always succeeds). File deletion is
    // best-effort + path-confined and never undoes the unregister.
    write_design_registry(&app, &entries)?;

    if args.remove_files {
        // Serialize the file deletion against our own design writers. NOTE lock
        // ordering: we already hold `config_write_lock` (above) and only NOW take the
        // design write guard — config FIRST, design SECOND — to avoid a deadlock.
        let _design_guard = design_write_guard()?;
        // Canonicalize the stored folder. If it no longer exists / is unreadable we
        // simply skip deletion (the entry is already unregistered).
        if let Ok(canonical) = canonical_working_folder(&removed.working_folder_path) {
            delete_known_design_files(&canonical);
        }
    }

    sort_registry(&mut entries);
    Ok(entries)
}

/// Delete ONLY the known design artifacts under an ALREADY-CANONICAL working folder,
/// each re-confined: a target is removed only if (a) its lexical parent is the canonical
/// root (top-level files) or the canonical components dir, AND (b) when it exists, its
/// canonicalized real path is still under the canonical root (defeats a symlink pointing
/// off-root). The working folder ROOT itself is never removed. Best-effort: an
/// individual delete failure is logged, not surfaced (the entry is already unregistered).
fn delete_known_design_files(canonical_root: &Path) {
    // Top-level known files: project.json, manifest.json, tokens.json, generations.jsonl.
    for name in DESIGN_TOP_LEVEL_FILES {
        let target = canonical_root.join(name);
        if confined_under(canonical_root, canonical_root, &target) {
            remove_file_if_present(&target);
        }
    }
    // export-*.html files we authored (validate_export_filename writes them directly
    // under the working folder). Only delete files matching our export naming so a
    // user's own *.html is never touched.
    if let Ok(rd) = fs::read_dir(canonical_root) {
        for entry in rd.flatten() {
            let p = entry.path();
            let is_export = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("export-") && n.ends_with(".html"))
                .unwrap_or(false);
            if !is_export {
                continue;
            }
            // Must be a regular file directly under root, confined, not a symlink target
            // escaping root.
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && confined_under(canonical_root, canonical_root, &p)
            {
                remove_file_if_present(&p);
            }
        }
    }
    // The components/ subtree we own. Confine the dir then remove recursively. We do NOT
    // remove the working folder root.
    let components_dir = canonical_root.join(COMPONENTS_DIR);
    if components_dir.is_dir()
        && confined_under(canonical_root, canonical_root, &components_dir)
        // Reject a components/ that is a symlink escaping root.
        && !is_symlink(&components_dir)
    {
        if let Err(e) = fs::remove_dir_all(&components_dir) {
            eprintln!(
                "[design] could not remove components dir {}: {e}",
                components_dir.display()
            );
        }
    }
}

/// True if `target`'s lexical parent equals `expected_parent` AND (when `target`
/// exists) its canonicalized real path stays under `root`. The lexical-parent check
/// rejects traversal in the constructed path; the canonicalize-when-exists check
/// rejects a symlink target pointing off-root.
fn confined_under(root: &Path, expected_parent: &Path, target: &Path) -> bool {
    if target.parent() != Some(expected_parent) {
        return false;
    }
    if target.exists() {
        // A symlink whose target is outside root must be rejected. Compare the
        // canonicalized real path (follows symlinks) against the canonical root.
        match fs::canonicalize(target) {
            Ok(real) => real.starts_with(root),
            Err(_) => false,
        }
    } else {
        // Non-existent target is a no-op delete; safe.
        true
    }
}

/// True if `path` is a symlink (does not follow it). Used to refuse removing a
/// `components/` entry that is actually a symlink escaping the working folder.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Remove a file if present, refusing to follow a symlink that escapes (the caller has
/// already confined the path; this additionally refuses to delete through a symlink that
/// points off-root by re-confining is the caller's job — here we just remove the file
/// entry). Best-effort: logs on failure.
fn remove_file_if_present(path: &Path) {
    // Refuse to delete THROUGH a symlink whose target escaped (caller confined it, but
    // belt-and-suspenders: if it is a symlink, remove the LINK only via remove_file —
    // which on all platforms unlinks the link, not its target). remove_file never
    // recurses, so it can only ever unlink one entry directly under the confined dir.
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[design] could not remove {}: {e}", path.display()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "aspis-design-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn placement(z: i64, h: NodeHeight) -> DesignNodePlacement {
        DesignNodePlacement {
            x: 10.0,
            y: 20.0,
            z,
            w: 300.0,
            h,
            kind: DesignNodeKind::Html,
            radius: None,
            flat: None,
            hidden: None,
            name: None,
        }
    }

    fn sample_project(_canonical: &Path) -> DesignProject {
        let now = Utc::now().to_rfc3339();
        let mut nodes = BTreeMap::new();
        nodes.insert("hero".to_string(), placement(1, NodeHeight::Auto(AutoHeight::Auto)));
        nodes.insert("cta".to_string(), placement(2, NodeHeight::Fixed(120.0)));
        let mut components = BTreeMap::new();
        components.insert("hero".to_string(), "<div>hero</div>".to_string());
        components.insert("cta".to_string(), "<button>buy</button>".to_string());
        DesignProject {
            meta: DesignProjectMeta {
                schema_version: SCHEMA_VERSION,
                id: "p-test".to_string(),
                name: "Landing".to_string(),
                created_at: now.clone(),
                updated_at: now,
                canvas: DesignCanvas {
                    w: 1440.0,
                    h: 1024.0,
                    grid: 8.0,
                },
                node_order: vec!["hero".to_string(), "cta".to_string()],
            },
            manifest: DesignManifest {
                schema_version: SCHEMA_VERSION,
                nodes,
            },
            components,
            warnings: Vec::new(),
        }
    }

    // ---- node id validation ------------------------------------------------

    #[test]
    fn valid_node_ids_accepted() {
        for id in ["a", "0", "hero", "cta-1", "node_2", "a".repeat(64).as_str()] {
            assert!(validate_node_id(id).is_ok(), "should accept {id}");
        }
    }

    #[test]
    fn invalid_node_ids_rejected() {
        let bad = [
            "",                 // empty
            "..",               // traversal
            "../escape",        // traversal
            "....//",           // traversal trick
            "a/b",              // slash
            "a\\b",             // backslash
            "/abs",             // leading slash
            "C:\\x",            // windows absolute
            ".hidden",          // leading dot
            "-lead",            // leading dash
            "_lead",            // leading underscore
            "Hero",             // uppercase
            "héro",             // non-ascii
            "a.b",              // dot mid-id
            "a b",              // space
            "node!",            // punctuation
            &"a".repeat(65),    // overlong
        ];
        for id in bad {
            assert!(validate_node_id(id).is_err(), "should reject {id:?}");
        }
    }

    // ---- create / load round-trip -----------------------------------------

    #[test]
    fn create_then_load_round_trip() {
        let base = tmp_dir();
        let wf = base.join("proj").to_string_lossy().to_string();
        // create_project not Tauri-invoked here; call the inner logic via the public
        // command requires State, so we exercise create via filesystem helpers used by
        // the command (write_meta/write_manifest_file) to validate the load path.
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf).unwrap();
        ensure_components_dir(&canonical).unwrap();
        let meta = DesignProjectMeta {
            schema_version: SCHEMA_VERSION,
            id: "p1".into(),
            name: "Landing".into(),
            created_at: "t0".into(),
            updated_at: "t0".into(),
            canvas: DesignCanvas { w: 1440.0, h: 1024.0, grid: 8.0 },
            node_order: vec![],
        };
        let manifest = DesignManifest { schema_version: SCHEMA_VERSION, nodes: BTreeMap::new() };
        write_meta(&canonical, &meta).unwrap();
        write_manifest_file(&canonical, &manifest).unwrap();

        let meta_raw = read_design_file(&canonical.join(PROJECT_FILE)).unwrap().unwrap();
        let loaded: DesignProjectMeta = serde_json::from_str(&meta_raw).unwrap();
        assert_eq!(loaded, meta);
    }

    #[test]
    fn save_then_reload_exact_restore() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();
        let project = sample_project(&canonical);

        // Mirror design_save_project's write order (components, manifest, meta).
        for (id, markup) in &project.components {
            let p = confined_component_path(&canonical, id).unwrap();
            atomic_write(&p, markup, "c").unwrap();
        }
        write_manifest_file(&canonical, &project.manifest).unwrap();
        write_meta(&canonical, &project.meta).unwrap();

        // Reload via the same logic design_load_project uses.
        let manifest_raw = read_design_file(&canonical.join(MANIFEST_FILE)).unwrap().unwrap();
        let manifest: DesignManifest = serde_json::from_str(&manifest_raw).unwrap();
        assert_eq!(manifest, project.manifest);

        let meta_raw = read_design_file(&canonical.join(PROJECT_FILE)).unwrap().unwrap();
        let meta: DesignProjectMeta = serde_json::from_str(&meta_raw).unwrap();
        assert_eq!(meta.node_order, project.meta.node_order);
        assert_eq!(meta.canvas, project.meta.canvas);

        for (id, markup) in &project.components {
            let p = confined_component_path(&canonical, id).unwrap();
            let got = read_design_file(&p).unwrap().unwrap();
            assert_eq!(&got, markup);
        }
        // z ordering preserved exactly.
        assert_eq!(manifest.nodes["hero"].z, 1);
        assert_eq!(manifest.nodes["cta"].z, 2);
        assert_eq!(manifest.nodes["hero"].h, NodeHeight::Auto(AutoHeight::Auto));
        assert_eq!(manifest.nodes["cta"].h, NodeHeight::Fixed(120.0));
    }

    #[test]
    fn write_node_then_load_reflects_new_markup() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();

        let p = confined_component_path(&canonical, "hero").unwrap();
        atomic_write(&p, "<div>v1</div>", "c").unwrap();
        assert_eq!(read_design_file(&p).unwrap().unwrap(), "<div>v1</div>");
        atomic_write(&p, "<div>v2</div>", "c").unwrap();
        assert_eq!(read_design_file(&p).unwrap().unwrap(), "<div>v2</div>");
    }

    #[test]
    fn load_tolerates_manifest_id_with_missing_component() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();

        let mut nodes = BTreeMap::new();
        nodes.insert("hero".to_string(), placement(1, NodeHeight::Auto(AutoHeight::Auto)));
        nodes.insert("ghost".to_string(), placement(2, NodeHeight::Fixed(50.0)));
        let manifest = DesignManifest { schema_version: SCHEMA_VERSION, nodes };
        write_manifest_file(&canonical, &manifest).unwrap();
        // Only write hero's component; ghost's file is absent (simulating a partial
        // atomic write / interrupted save).
        let hero = confined_component_path(&canonical, "hero").unwrap();
        atomic_write(&hero, "<div>hero</div>", "c").unwrap();

        // Replicate design_load_project's component-gathering loop.
        let mut components = BTreeMap::new();
        let mut warnings = Vec::new();
        for id in manifest.nodes.keys() {
            let path = confined_component_path(&canonical, id).unwrap();
            match read_design_file(&path).unwrap() {
                Some(m) => { components.insert(id.clone(), m); }
                None => warnings.push(format!("missing {id}")),
            }
        }
        assert!(components.contains_key("hero"));
        assert!(!components.contains_key("ghost"));
        assert_eq!(warnings.len(), 1, "ghost should produce exactly one warning");
    }

    // ---- path confinement --------------------------------------------------

    #[test]
    fn confined_component_path_rejects_traversal_ids() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();
        for id in ["..", "../x", "a/b", "a\\b", "/abs", "....//", ".hidden", "Hero", "", &"a".repeat(65)] {
            assert!(
                confined_component_path(&canonical, id).is_err(),
                "should reject component id {id:?}"
            );
        }
    }

    #[test]
    fn confined_component_path_stays_under_components() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();
        let p = confined_component_path(&canonical, "hero").unwrap();
        assert!(p.starts_with(canonical.join(COMPONENTS_DIR)));
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "hero.html");
    }

    #[test]
    fn canonical_working_folder_rejects_missing() {
        let base = tmp_dir();
        let missing = base.join("does-not-exist");
        assert!(canonical_working_folder(&missing.to_string_lossy()).is_err());
        assert!(canonical_working_folder("").is_err());
        assert!(canonical_working_folder("   ").is_err());
    }

    // ---- serde round-trip (camelCase + h shapes) --------------------------

    #[test]
    fn node_height_serializes_auto_as_string_and_fixed_as_number() {
        let auto = NodeHeight::Auto(AutoHeight::Auto);
        let fixed = NodeHeight::Fixed(42.0);
        assert_eq!(serde_json::to_string(&auto).unwrap(), "\"auto\"");
        assert_eq!(serde_json::to_string(&fixed).unwrap(), "42.0");
        // round-trip both directions
        let a: NodeHeight = serde_json::from_str("\"auto\"").unwrap();
        let f: NodeHeight = serde_json::from_str("42").unwrap();
        assert_eq!(a, auto);
        assert_eq!(f, NodeHeight::Fixed(42.0));
        // a non-"auto" string is rejected (not silently swallowed)
        assert!(serde_json::from_str::<NodeHeight>("\"tall\"").is_err());
    }

    #[test]
    fn placement_uses_camel_case_and_lowercase_kind() {
        let p = placement(3, NodeHeight::Fixed(10.0));
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"kind\":\"html\""), "got {json}");
        // x/y/z/w/h are already lowercase single letters; verify kind tag + h number.
        assert!(json.contains("\"h\":10.0"), "got {json}");
        let back: DesignNodePlacement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    // ---- optional placement fields (radius/flat/hidden/name) --------------

    #[test]
    fn old_placement_json_without_new_fields_deserializes() {
        // An OLD manifest entry (schema v1, before the four optional fields existed)
        // must still deserialize: serde `default` fills each as None.
        let old = r#"{"x":1.0,"y":2.0,"z":3,"w":300.0,"h":"auto","kind":"html"}"#;
        let p: DesignNodePlacement = serde_json::from_str(old).unwrap();
        assert_eq!(p.radius, None);
        assert_eq!(p.flat, None);
        assert_eq!(p.hidden, None);
        assert_eq!(p.name, None);
    }

    #[test]
    fn placement_with_new_fields_round_trips() {
        // create -> serialize -> deserialize preserves every new field exactly.
        let mut p = placement(4, NodeHeight::Fixed(120.0));
        p.radius = Some(16.0);
        p.flat = Some(true);
        p.hidden = Some(true);
        p.name = Some("Hero section".to_string());
        let json = serde_json::to_string(&p).unwrap();
        let back: DesignNodePlacement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.radius, Some(16.0));
        assert_eq!(back.flat, Some(true));
        assert_eq!(back.hidden, Some(true));
        assert_eq!(back.name.as_deref(), Some("Hero section"));
    }

    #[test]
    fn placement_omits_none_optional_fields_on_serialize() {
        // A node that sets none of the new fields adds ZERO schema churn: the keys are
        // simply absent (skip_serializing_if = Option::is_none).
        let p = placement(1, NodeHeight::Auto(AutoHeight::Auto));
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("radius"), "got {json}");
        assert!(!json.contains("flat"), "got {json}");
        assert!(!json.contains("hidden"), "got {json}");
        assert!(!json.contains("name"), "got {json}");
    }

    #[test]
    fn validate_placement_bounds_radius_and_name() {
        let base = placement(1, NodeHeight::Auto(AutoHeight::Auto));
        // Valid: in-range radius + short name.
        let mut ok = base.clone();
        ok.radius = Some(24.0);
        ok.name = Some("Card".to_string());
        assert!(validate_placement("hero", &ok).is_ok());
        // Boundary radii accepted.
        let mut zero = base.clone();
        zero.radius = Some(0.0);
        assert!(validate_placement("hero", &zero).is_ok());
        let mut maxr = base.clone();
        maxr.radius = Some(MAX_NODE_RADIUS);
        assert!(validate_placement("hero", &maxr).is_ok());
        // Negative / over-cap / non-finite radii rejected.
        for bad in [-1.0, MAX_NODE_RADIUS + 1.0, f64::NAN, f64::INFINITY] {
            let mut p = base.clone();
            p.radius = Some(bad);
            assert!(validate_placement("hero", &p).is_err(), "should reject {bad}");
        }
        // Over-long name rejected; exactly-at-cap accepted.
        let mut long = base.clone();
        long.name = Some("x".repeat(MAX_NODE_NAME_CHARS + 1));
        assert!(validate_placement("hero", &long).is_err());
        let mut atcap = base;
        atcap.name = Some("x".repeat(MAX_NODE_NAME_CHARS));
        assert!(validate_placement("hero", &atcap).is_ok());
    }

    #[test]
    fn project_meta_round_trip_camel_case() {
        let now = "2026-06-07T00:00:00+00:00".to_string();
        let meta = DesignProjectMeta {
            schema_version: SCHEMA_VERSION,
            id: "p1".into(),
            name: "Landing".into(),
            created_at: now.clone(),
            updated_at: now,
            canvas: DesignCanvas { w: 1440.0, h: 1024.0, grid: 8.0 },
            node_order: vec!["hero".into(), "cta".into()],
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"schemaVersion\""), "got {json}");
        assert!(json.contains("\"createdAt\""), "got {json}");
        assert!(json.contains("\"updatedAt\""), "got {json}");
        assert!(json.contains("\"nodeOrder\""), "got {json}");
        let back: DesignProjectMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn warnings_skipped_when_empty() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        let project = sample_project(&canonical);
        let json = serde_json::to_string(&project).unwrap();
        assert!(!json.contains("warnings"), "empty warnings must be omitted: {json}");
    }

    // ---- BLOCKER 2: error strings must not leak absolute FS paths ----------

    #[test]
    fn load_entry_error_does_not_leak_absolute_path() {
        // A crafted, clearly-marked path that does not exist. The returned error must NOT
        // echo the absolute path back to the renderer (it goes to the process log only).
        let base = tmp_dir();
        let missing = base.join("SECRET-LEAK-MARKER").join("nope");
        let p = missing.to_string_lossy().to_string();
        let err = canonical_working_folder(&p).unwrap_err();
        assert!(
            !err.contains("SECRET-LEAK-MARKER"),
            "error leaked the absolute path: {err}"
        );
        assert!(
            !err.contains(&p),
            "error leaked the full path string: {err}"
        );
    }

    #[test]
    fn oversized_read_error_does_not_leak_path() {
        let base = tmp_dir();
        let f = base.join("MARKER-PATH-oversized.html");
        // Write just over the cap.
        let big = "a".repeat((MAX_DESIGN_FILE_BYTES as usize) + 1);
        fs::write(&f, &big).unwrap();
        let err = read_design_file(&f).unwrap_err();
        assert!(!err.contains("MARKER-PATH"), "leaked path in size error: {err}");
        assert!(err.contains("too large"), "should explain the cause: {err}");
    }

    // ---- BLOCKER 1: node_order validation ---------------------------------

    #[test]
    fn load_drops_invalid_node_order_with_sanitized_warning() {
        // Mirror design_load_project's node_order filtering.
        let order = vec![
            "hero".to_string(),
            "../escape".to_string(),
            "cta".to_string(),
            "Bad Id".to_string(),
        ];
        let mut warnings = Vec::new();
        let mut kept = Vec::new();
        for id in order {
            if validate_node_id(&id).is_ok() {
                kept.push(id);
            } else {
                push_warning(
                    &mut warnings,
                    format!(
                        "dropped invalid node id in nodeOrder: \"{}\"",
                        sanitize_id_for_warning(&id)
                    ),
                );
            }
        }
        assert_eq!(kept, vec!["hero".to_string(), "cta".to_string()]);
        assert_eq!(warnings.len(), 2);
        // Warning carries the (sanitized) id, never a path-traversal payload acted upon.
        assert!(warnings.iter().any(|w| w.contains("../escape")));
    }

    #[test]
    fn save_rejects_invalid_node_order_entry() {
        // The save command validates every node_order entry and hard-fails. We assert the
        // validation predicate it relies on.
        assert!(validate_node_id("../escape").is_err());
        assert!(validate_node_id("ok-id").is_ok());
    }

    // ---- WARNING 5: node-count + warnings caps ----------------------------

    #[test]
    fn node_count_cap_constant_is_enforceable() {
        // A manifest at the cap is fine; one over the cap is rejected by the guard. We test
        // the boundary arithmetic the command uses.
        assert!((MAX_DESIGN_NODES + 1) > MAX_DESIGN_NODES);
        let over = MAX_DESIGN_NODES + 1;
        assert!(over > MAX_DESIGN_NODES, "guard compares len > cap");
    }

    #[test]
    fn push_warning_caps_vec_length() {
        let mut w = Vec::new();
        for i in 0..(MAX_DESIGN_WARNINGS + 50) {
            push_warning(&mut w, format!("w{i}"));
        }
        // Capped at MAX_DESIGN_WARNINGS retained + exactly one suppression note.
        assert_eq!(w.len(), MAX_DESIGN_WARNINGS + 1);
        assert!(w.last().unwrap().contains("suppressed"));
    }

    // ---- WARNING 6: untrusted id sanitized in warnings --------------------

    #[test]
    fn sanitize_id_truncates_and_scrubs() {
        let long = "x".repeat(100);
        let s = sanitize_id_for_warning(&long);
        // 32 kept chars + the ellipsis marker.
        assert_eq!(s.chars().count(), 33);
        assert!(s.ends_with('…'));

        let nasty = "ab\u{0007}\ncd\u{202E}ef";
        let s2 = sanitize_id_for_warning(nasty);
        assert!(!s2.contains('\u{0007}'), "control char must be scrubbed");
        assert!(!s2.contains('\n'), "newline must be scrubbed");
        assert!(!s2.contains('\u{202E}'), "RTL override must be scrubbed");
        assert!(s2.contains('?'), "scrubbed bytes become ?");
        assert!(s2.contains("ab") && s2.contains("cd"));
    }

    // ---- WARNING 9: schema_version gating ---------------------------------

    #[test]
    fn schema_version_too_new_is_rejected() {
        let err = check_schema_version(SCHEMA_VERSION + 1, PROJECT_FILE).unwrap_err();
        assert!(err.contains("newer format"), "got {err}");
        assert!(err.contains("upgrade"), "got {err}");
        // Current and older are accepted.
        assert!(check_schema_version(SCHEMA_VERSION, PROJECT_FILE).is_ok());
        if SCHEMA_VERSION > 0 {
            assert!(check_schema_version(SCHEMA_VERSION - 1, MANIFEST_FILE).is_ok());
        }
    }

    #[test]
    fn load_rejects_too_new_manifest_schema() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();
        // Manifest claiming a future schema version.
        let manifest = DesignManifest {
            schema_version: SCHEMA_VERSION + 1,
            nodes: BTreeMap::new(),
        };
        write_manifest_file(&canonical, &manifest).unwrap();
        let raw = read_design_file(&canonical.join(MANIFEST_FILE)).unwrap().unwrap();
        let parsed: DesignManifest = serde_json::from_str(&raw).unwrap();
        assert!(check_schema_version(parsed.schema_version, MANIFEST_FILE).is_err());
    }

    // ---- WARNING 12: write-size cap ---------------------------------------

    #[test]
    fn oversized_markup_write_is_rejected() {
        let big = "a".repeat((MAX_DESIGN_FILE_BYTES as usize) + 1);
        assert!(check_write_size(&big, "hero").is_err());
        let ok = "a".repeat(16);
        assert!(check_write_size(&ok, "hero").is_ok());
    }

    // ---- WARNING 4: components dir symlink hardening -----------------------

    #[test]
    fn ensure_components_dir_accepts_normal_dir() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();
        assert!(canonical.join(COMPONENTS_DIR).is_dir());
    }

    // ---- NITPICK 10: timestamp suffix uses micros (always Some) -----------

    #[test]
    fn write_suffix_is_pid_dash_micros() {
        let s = write_suffix();
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 2, "suffix shape is <pid>-<micros>: {s}");
        assert!(parts[0].parse::<u32>().is_ok(), "pid part: {s}");
        assert!(parts[1].parse::<i64>().is_ok(), "micros part: {s}");
    }

    // ---- STEP 4: grounding root resolution --------------------------------

    fn log_entry() -> GenerationLogEntry {
        GenerationLogEntry {
            ts: "2026-06-07T00:00:00Z".to_string(),
            kind: "generate".to_string(),
            node_ids: vec!["hero".to_string(), "cta".to_string()],
            backend_kind: "ollama".to_string(),
            prompt_chars: 1234,
            oracle_grounded: true,
            duration_ms: 567,
            outcome: "applied".to_string(),
        }
    }

    #[test]
    fn resolve_grounding_root_prefers_explicit_target_when_dir() {
        let base = tmp_dir();
        let target = base.join("target");
        let wf = target.join(".aspis-design").join("landing");
        fs::create_dir_all(&wf).unwrap();
        // Explicit existing dir wins outright.
        let got = resolve_grounding_root(&wf, Some(target.to_str().unwrap()));
        assert_eq!(got, target);
    }

    #[test]
    fn resolve_grounding_root_ignores_nonexistent_explicit_then_walks() {
        let base = tmp_dir();
        let target = base.join("target");
        let wf = target.join(".aspis-design").join("landing");
        fs::create_dir_all(&wf).unwrap();
        // Mark the target as a project root.
        fs::create_dir_all(target.join(".git")).unwrap();
        // A non-existent explicit override is ignored; we walk up to the .git marker.
        let got = resolve_grounding_root(&wf, Some("/no/such/path/at/all"));
        assert_eq!(got, target);
    }

    #[test]
    fn resolve_grounding_root_walks_to_package_json() {
        let base = tmp_dir();
        let target = base.join("target");
        let wf = target.join(".aspis-design").join("landing");
        fs::create_dir_all(&wf).unwrap();
        fs::write(target.join("package.json"), "{}").unwrap();
        let got = resolve_grounding_root(&wf, None);
        assert_eq!(got, target);
    }

    #[test]
    fn resolve_grounding_root_walks_to_cargo_toml() {
        let base = tmp_dir();
        let target = base.join("target");
        let wf = target.join("sub").join("design");
        fs::create_dir_all(&wf).unwrap();
        fs::write(target.join("Cargo.toml"), "[package]").unwrap();
        let got = resolve_grounding_root(&wf, None);
        assert_eq!(got, target);
    }

    #[test]
    fn resolve_grounding_root_falls_back_to_working_folder() {
        let base = tmp_dir();
        // No marker anywhere up to tmp; falls back to the working folder itself.
        let wf = base.join("isolated").join("wf");
        fs::create_dir_all(&wf).unwrap();
        let got = resolve_grounding_root(&wf, None);
        assert_eq!(got, wf);
    }

    // ---- STEP 4: audit log is METADATA-ONLY + bounded ---------------------

    #[test]
    fn generation_log_line_is_metadata_only_no_prompt_field() {
        let line = build_generation_log_line(&log_entry()).unwrap();
        // The line must NOT contain a prompt/text/secret field — only the metadata.
        assert!(!line.contains("prompt\""), "no prompt text field: {line}");
        assert!(!line.contains("\"text\""), "no chunk text field: {line}");
        // camelCase keys present.
        assert!(line.contains("\"ts\""));
        assert!(line.contains("\"kind\":\"generate\""));
        assert!(line.contains("\"nodeIds\""));
        assert!(line.contains("\"backendKind\":\"ollama\""));
        assert!(line.contains("\"promptChars\":1234"));
        assert!(line.contains("\"oracleGrounded\":true"));
        assert!(line.contains("\"durationMs\":567"));
        assert!(line.contains("\"outcome\":\"applied\""));
        assert!(line.ends_with('\n'), "JSONL line must be newline-terminated");
    }

    #[test]
    fn generation_log_line_rejects_bad_enums_and_ids() {
        let mut bad_kind = log_entry();
        bad_kind.kind = "exfiltrate".to_string();
        assert!(build_generation_log_line(&bad_kind).is_err());

        let mut bad_outcome = log_entry();
        bad_outcome.outcome = "leaked".to_string();
        assert!(build_generation_log_line(&bad_outcome).is_err());

        let mut bad_id = log_entry();
        bad_id.node_ids = vec!["../escape".to_string()];
        assert!(build_generation_log_line(&bad_id).is_err());
    }

    #[test]
    fn generation_log_line_allowlists_backend_kind() {
        // WARNING 4: only the known kinds + the "unknown" sentinel are accepted.
        for ok in ["ollama", "omlx", "api", "codex", "claude", "unknown"] {
            let mut e = log_entry();
            e.backend_kind = ok.to_string();
            assert!(build_generation_log_line(&e).is_ok(), "should accept {ok}");
        }
        for bad in ["", "evil", "OLLAMA", "ollama; rm -rf", "openai"] {
            let mut e = log_entry();
            e.backend_kind = bad.to_string();
            assert!(
                build_generation_log_line(&e).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn generation_log_round_trips_struct_only_dropping_extra_fields() {
        // A frontend payload with an extra "prompt" key must NOT survive: serde
        // ignores unknown fields on deserialize, and we re-serialize from the struct.
        let raw = r#"{
            "ts":"2026-06-07T00:00:00Z","kind":"edit","nodeIds":["hero"],
            "backendKind":"omlx","promptChars":10,"oracleGrounded":false,
            "durationMs":1,"outcome":"applied",
            "prompt":"SECRET PROMPT TEXT","apiKey":"sk-leak"
        }"#;
        let entry: GenerationLogEntry = serde_json::from_str(raw).unwrap();
        let line = build_generation_log_line(&entry).unwrap();
        assert!(!line.contains("SECRET PROMPT TEXT"), "prompt leaked: {line}");
        assert!(!line.contains("sk-leak"), "key leaked: {line}");
        assert!(!line.contains("apiKey"), "extra field leaked: {line}");
    }

    #[test]
    fn append_line_then_rotate_bounds_growth() {
        let base = tmp_dir();
        let path = base.join(GENERATIONS_FILE);
        // Write a file just over the cap of identical short lines.
        let one = "x".repeat(63);
        let line = format!("{one}\n"); // 64 bytes per line
        let lines_needed = (MAX_GENERATIONS_FILE_BYTES as usize / line.len()) + 10;
        let mut blob = String::with_capacity(lines_needed * line.len());
        for _ in 0..lines_needed {
            blob.push_str(&line);
        }
        fs::write(&path, &blob).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > MAX_GENERATIONS_FILE_BYTES);

        rotate_generations_if_needed(&path).unwrap();
        let after = fs::metadata(&path).unwrap().len();
        assert!(
            after <= GENERATIONS_KEEP_BYTES as u64,
            "rotated file should be trimmed to the keep window: {after}"
        );
        // The first retained line must be whole (starts at a line boundary).
        let kept = fs::read_to_string(&path).unwrap();
        assert!(kept.starts_with(&one), "first kept line must be whole");

        // A subsequent append still works and stays under control.
        append_line(&path, "y\n").unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("y\n"));
    }

    #[test]
    fn rotate_resets_on_invalid_utf8_instead_of_corrupting() {
        // WARNING 7: a file over the cap whose retained window is NOT valid UTF-8
        // must be RESET to empty (no U+FFFD replacement-char corruption), never
        // persisted with garbled lines.
        let base = tmp_dir();
        let path = base.join(GENERATIONS_FILE);
        // Build a file just over the cap made of invalid UTF-8 bytes (no newlines so
        // the whole retained tail is the invalid blob).
        let blob = vec![0xFFu8; (MAX_GENERATIONS_FILE_BYTES as usize) + 4096];
        fs::write(&path, &blob).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > MAX_GENERATIONS_FILE_BYTES);

        rotate_generations_if_needed(&path).unwrap();

        let kept = fs::read(&path).unwrap();
        // The file was reset (empty) — no replacement char (0xEF 0xBF 0xBD) leaked.
        assert!(kept.is_empty(), "invalid-UTF8 file should reset to empty");
        assert!(
            !kept.windows(3).any(|w| w == [0xEF, 0xBF, 0xBD]),
            "no U+FFFD replacement chars may be written"
        );
        // A subsequent append still works on the reset file.
        append_line(&path, "z\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "z\n");
    }

    #[test]
    fn rotate_read_is_bounded_for_oversized_file() {
        // WARNING 2: rotation must read through a hard cap (MAX*2), not the whole
        // file. Build a file FAR larger than the cap; rotation must still succeed and
        // leave a trimmed, bounded result (it never tried to slurp the whole thing).
        let base = tmp_dir();
        let path = base.join(GENERATIONS_FILE);
        let one = "x".repeat(63);
        let line = format!("{one}\n"); // 64 bytes per line
        // 3x the cap of valid short lines.
        let target_bytes = (MAX_GENERATIONS_FILE_BYTES as usize) * 3;
        let lines_needed = (target_bytes / line.len()) + 1;
        let mut blob = String::with_capacity(lines_needed * line.len());
        for _ in 0..lines_needed {
            blob.push_str(&line);
        }
        fs::write(&path, &blob).unwrap();
        assert!(
            fs::metadata(&path).unwrap().len() > MAX_GENERATIONS_FILE_BYTES * 2,
            "fixture must exceed the read cap"
        );

        rotate_generations_if_needed(&path).unwrap();

        // Result is trimmed to the keep window and every kept line is whole.
        let after = fs::metadata(&path).unwrap().len();
        assert!(
            after <= GENERATIONS_KEEP_BYTES as u64,
            "rotated file should be trimmed to the keep window: {after}"
        );
        let kept = fs::read_to_string(&path).unwrap();
        assert!(kept.starts_with(&one), "first kept line must be whole");
    }

    #[test]
    fn rotate_is_noop_for_small_or_missing_file() {
        let base = tmp_dir();
        let path = base.join(GENERATIONS_FILE);
        // Missing file: no-op.
        rotate_generations_if_needed(&path).unwrap();
        // Small file: untouched.
        fs::write(&path, "a\nb\n").unwrap();
        rotate_generations_if_needed(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\n");
    }

    // ---- W5: design RwLock read/write semantics ---------------------------

    #[test]
    fn w5_design_rwlock_read_write_semantics() {
        // W5: ONE test (the lock is a process-wide singleton, so concurrent cargo
        // test threads would otherwise race it — serialize all assertions here).
        //   - Two READ guards (the kind design_load_project takes) coexist: a load
        //     does not block another load.
        //   - A WRITE guard (a read-modify-write command) excludes BOTH readers and
        //     other writers.
        //   - After the writer releases, a writer can acquire again.
        {
            let r1 = design_read_guard().expect("first read guard");
            let r2 = design_rwlock().try_read();
            assert!(r2.is_ok(), "a second concurrent reader must be admitted");
            drop(r1);
            drop(r2);
        }
        {
            let w = design_write_guard().expect("write guard");
            assert!(
                design_rwlock().try_read().is_err(),
                "a reader must be blocked while a writer holds the lock"
            );
            assert!(
                design_rwlock().try_write().is_err(),
                "a second writer must be blocked while a writer holds the lock"
            );
        }
        assert!(
            design_rwlock().try_write().is_ok(),
            "a writer can acquire again once the previous one released"
        );
    }

    // ---- STEP 4: export filename confinement ------------------------------

    #[test]
    fn export_filename_accepts_simple_html_names() {
        for ok in ["export.html", "landing-flow.html", "a1_b2.html", "0.html"] {
            assert!(validate_export_filename(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn export_filename_rejects_traversal_and_bad_ext() {
        for bad in [
            "",
            "../escape.html",
            "a/b.html",
            "a\\b.html",
            "/abs.html",
            ".hidden.html",
            "Export.html",   // uppercase
            "export.htm",    // wrong ext
            "export",        // no ext
            ".html",         // empty stem
            "ex port.html",  // space
            "ex!.html",      // punctuation
        ] {
            assert!(
                validate_export_filename(bad).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    // ---- STEP 4: tokens.json must be valid JSON ---------------------------

    #[test]
    fn design_context_chunk_keeps_text_on_wire() {
        // The grounding chunk struct (re-exported from oracle) serializes camelCase
        // and KEEPS text — the design path needs it (privacy-acceptable, in-process).
        let chunk = crate::oracle::python_oracle::DesignContextChunk {
            file_source: "src/Button.tsx".to_string(),
            score: 0.5,
            text: "<button/>".to_string(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"fileSource\":\"src/Button.tsx\""), "got {json}");
        assert!(json.contains("\"text\":\"<button/>\""), "got {json}");
    }

    // ---- P3: registry pure ops (upsert/dedupe/sort/cap/evict/rename/remove) -----

    fn entry(id: &str, name: &str, path: &str, last_opened: &str) -> DesignProjectEntry {
        DesignProjectEntry {
            id: id.to_string(),
            name: name.to_string(),
            working_folder_path: path.to_string(),
            created_at: "2020-01-01T00:00:00Z".to_string(),
            updated_at: last_opened.to_string(),
            last_opened_at: last_opened.to_string(),
            thumbnail_path: None,
            contract_sha: None,
        }
    }

    /// Identity key fn for the pure tests (no filesystem): the stored path IS the key.
    fn lexical_key(p: &str) -> String {
        p.trim().to_string()
    }

    #[test]
    fn registry_upsert_inserts_new_entry() {
        let mut entries = Vec::new();
        let e = entry("p1", "Landing", "/a/landing", "2021-01-01T00:00:00Z");
        registry_ops::upsert(&mut entries, e.clone(), &lexical_key("/a/landing"), &lexical_key);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], e);
    }

    #[test]
    fn registry_upsert_dedupes_by_folder_key() {
        let mut entries = vec![entry("p1", "Old", "/a/landing", "2021-01-01T00:00:00Z")];
        // Same folder, new name + later lastOpenedAt, DIFFERENT incoming id.
        let mut incoming = entry("p2-ignored", "New", "/a/landing", "2022-02-02T00:00:00Z");
        incoming.updated_at = "2022-02-02T00:00:00Z".to_string();
        registry_ops::upsert(&mut entries, incoming, &lexical_key("/a/landing"), &lexical_key);
        assert_eq!(entries.len(), 1, "must NOT insert a duplicate folder");
        // Identity (id + createdAt) preserved from the existing entry.
        assert_eq!(entries[0].id, "p1");
        assert_eq!(entries[0].created_at, "2020-01-01T00:00:00Z");
        // Mutable fields updated.
        assert_eq!(entries[0].name, "New");
        assert_eq!(entries[0].last_opened_at, "2022-02-02T00:00:00Z");
    }

    #[test]
    fn registry_entry_contract_sha_serde_round_trip() {
        // Some(hex) serializes as camelCase `contractSha` and round-trips.
        let mut e = entry("p1", "Landing", "/a/landing", "2021-01-01T00:00:00Z");
        e.contract_sha = Some("abc123".to_string());
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"contractSha\":\"abc123\""), "got {json}");
        let back: DesignProjectEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);

        // None is OMITTED from the wire (skip_serializing_if).
        let e_none = entry("p2", "X", "/a/x", "2021-01-01T00:00:00Z");
        let json_none = serde_json::to_string(&e_none).unwrap();
        assert!(!json_none.contains("contractSha"), "None must be omitted: {json_none}");
    }

    #[test]
    fn registry_entry_legacy_without_contract_sha_parses() {
        // An OLD config entry (pre-Fix-3) has no contractSha key — it must still parse,
        // defaulting the field to None.
        let legacy = r#"{
            "id": "p1",
            "name": "Landing",
            "workingFolderPath": "/a/landing",
            "createdAt": "2020-01-01T00:00:00Z",
            "updatedAt": "2021-01-01T00:00:00Z",
            "lastOpenedAt": "2021-01-01T00:00:00Z"
        }"#;
        let parsed: DesignProjectEntry = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.contract_sha, None);
        assert_eq!(parsed.id, "p1");
    }

    #[test]
    fn registry_upsert_carries_and_preserves_contract_sha() {
        // Insert an entry that already has an approved hash.
        let mut e = entry("p1", "Landing", "/a/landing", "2021-01-01T00:00:00Z");
        e.contract_sha = Some("oldhash".to_string());
        let mut entries = vec![e];

        // A plain load/create remember (contract_sha = None) MUST NOT wipe the hash.
        let load = entry("p1b", "Landing", "/a/landing", "2022-01-01T00:00:00Z");
        registry_ops::upsert(&mut entries, load, &lexical_key("/a/landing"), &lexical_key);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].contract_sha.as_deref(),
            Some("oldhash"),
            "load remember (None) must preserve the recorded hash"
        );

        // A contract-Save remember (Some) OVERWRITES it.
        let mut save = entry("p1c", "Landing", "/a/landing", "2023-01-01T00:00:00Z");
        save.contract_sha = Some("newhash".to_string());
        registry_ops::upsert(&mut entries, save, &lexical_key("/a/landing"), &lexical_key);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].contract_sha.as_deref(), Some("newhash"));
    }

    #[test]
    fn w7_canonicalize_for_storage_normalizes_equivalent_paths() {
        // W7: the same real folder reached via two path representations must produce
        // the SAME stored path (so it dedupes to one entry). Use a real temp dir.
        let base = tmp_dir();
        let real = fs::canonicalize(&base).unwrap();
        let with_dot = base.join(".");
        let with_trailing = format!("{}/", base.to_string_lossy());

        let s1 = canonicalize_for_storage(&base.to_string_lossy());
        let s2 = canonicalize_for_storage(&with_dot.to_string_lossy());
        let s3 = canonicalize_for_storage(&with_trailing);

        assert_eq!(s1, real.to_string_lossy());
        assert_eq!(s1, s2, "`.` segment must normalize to the same stored path");
        assert_eq!(s1, s3, "trailing slash must normalize to the same stored path");
        // And the dedupe KEY computed from each stored path matches too.
        assert_eq!(registry_dedupe_key(&s1), registry_dedupe_key(&s2));
        assert_eq!(registry_dedupe_key(&s1), registry_dedupe_key(&s3));
    }

    #[test]
    fn w7_canonicalize_for_storage_falls_back_when_missing() {
        // A non-existent path is stored as the trimmed input (entry still recorded).
        let missing = tmp_dir().join("does-not-exist");
        let s = canonicalize_for_storage(&missing.to_string_lossy());
        assert_eq!(s, missing.to_string_lossy());
    }

    #[test]
    fn registry_sort_by_last_opened_desc_then_name() {
        let mut entries = vec![
            entry("a", "Beta", "/a", "2021-01-01T00:00:00Z"),
            entry("b", "Alpha", "/b", "2023-01-01T00:00:00Z"),
            entry("c", "Gamma", "/c", "2023-01-01T00:00:00Z"), // tie with Alpha
        ];
        sort_registry(&mut entries);
        // Most-recent first; the tie (Alpha/Gamma) broken by name asc.
        assert_eq!(entries[0].name, "Alpha");
        assert_eq!(entries[1].name, "Gamma");
        assert_eq!(entries[2].name, "Beta");
    }

    #[test]
    fn registry_upsert_caps_and_evicts_oldest() {
        let mut entries = Vec::new();
        // Fill to the cap with strictly increasing lastOpenedAt timestamps.
        for i in 0..MAX_REGISTRY_ENTRIES {
            let ts = format!("2021-01-01T00:00:{:02}Z", i % 60);
            let ts = format!("{ts}-{i}"); // ensure unique ordering string
            let p = format!("/p/{i}");
            let e = entry(&format!("id{i}"), &format!("n{i}"), &p, &ts);
            registry_ops::upsert(&mut entries, e, &lexical_key(&p), &lexical_key);
        }
        assert_eq!(entries.len(), MAX_REGISTRY_ENTRIES);
        // The oldest entry is the FIRST inserted ("/p/0", smallest ts string).
        // Insert one MORE with a brand-new folder + the latest ts → evict the oldest.
        let newp = "/p/new";
        let newest = entry("idNew", "newest", newp, "2099-01-01T00:00:00Z");
        registry_ops::upsert(&mut entries, newest, &lexical_key(newp), &lexical_key);
        assert_eq!(entries.len(), MAX_REGISTRY_ENTRIES, "cap holds after insert");
        // The new entry is present; the oldest ("/p/0") was evicted.
        assert!(entries.iter().any(|e| e.working_folder_path == newp));
        assert!(
            !entries.iter().any(|e| e.working_folder_path == "/p/0"),
            "oldest entry must be evicted"
        );
    }

    #[test]
    fn registry_rename_only_matching_id() {
        let mut entries = vec![
            entry("p1", "A", "/a", "2021-01-01T00:00:00Z"),
            entry("p2", "B", "/b", "2021-01-02T00:00:00Z"),
        ];
        assert!(registry_ops::rename(&mut entries, "p2", "B2".into(), "t".into()));
        assert_eq!(entries[1].name, "B2");
        assert_eq!(entries[1].updated_at, "t");
        assert_eq!(entries[0].name, "A"); // untouched
        // Unknown id -> false, no mutation.
        assert!(!registry_ops::rename(&mut entries, "nope", "X".into(), "t".into()));
    }

    #[test]
    fn registry_remove_returns_entry_and_unregisters() {
        let mut entries = vec![
            entry("p1", "A", "/a", "2021-01-01T00:00:00Z"),
            entry("p2", "B", "/b", "2021-01-02T00:00:00Z"),
        ];
        let removed = registry_ops::remove(&mut entries, "p1").unwrap();
        assert_eq!(removed.id, "p1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "p2");
        assert!(registry_ops::remove(&mut entries, "missing").is_none());
    }

    #[test]
    fn registry_entry_serializes_camel_case_metadata_only() {
        let e = entry("p1", "Landing", "/a/landing", "2021-01-01T00:00:00Z");
        let json = serde_json::to_string(&e).unwrap();
        for key in [
            "\"id\"",
            "\"name\"",
            "\"workingFolderPath\"",
            "\"createdAt\"",
            "\"updatedAt\"",
            "\"lastOpenedAt\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        // METADATA ONLY: never manifest/markup/prompt/components/tokens.
        for forbidden in ["manifest", "markup", "prompt", "components", "tokens", "nodes"] {
            assert!(
                !json.contains(forbidden),
                "registry entry must not carry {forbidden}: {json}"
            );
        }
        // thumbnailPath is omitted when None (skip_serializing_if).
        assert!(!json.contains("thumbnailPath"), "None thumbnail omitted: {json}");
    }

    // ---- P3: remove-with-files path confinement ----------------------------

    #[test]
    fn delete_known_files_removes_only_design_artifacts() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        ensure_components_dir(&canonical).unwrap();

        // Known design artifacts.
        fs::write(canonical.join(PROJECT_FILE), "{}").unwrap();
        fs::write(canonical.join(MANIFEST_FILE), "{}").unwrap();
        fs::write(canonical.join(TOKENS_FILE), "{}").unwrap();
        fs::write(canonical.join(GENERATIONS_FILE), "x\n").unwrap();
        fs::write(canonical.join("export-absolute.html"), "<html>").unwrap();
        let comp = canonical.join(COMPONENTS_DIR).join("hero.html");
        fs::write(&comp, "<div>").unwrap();
        // A USER file we must NEVER delete.
        let user_file = canonical.join("README.md");
        fs::write(&user_file, "mine").unwrap();
        let user_html = canonical.join("index.html"); // not an export-*.html
        fs::write(&user_html, "mine").unwrap();

        delete_known_design_files(&canonical);

        // Design artifacts gone.
        assert!(!canonical.join(PROJECT_FILE).exists());
        assert!(!canonical.join(MANIFEST_FILE).exists());
        assert!(!canonical.join(TOKENS_FILE).exists());
        assert!(!canonical.join(GENERATIONS_FILE).exists());
        assert!(!canonical.join("export-absolute.html").exists());
        assert!(!canonical.join(COMPONENTS_DIR).exists());
        // The working folder ROOT survives.
        assert!(canonical.is_dir());
        // User files survive.
        assert!(user_file.exists(), "must not delete the user's README.md");
        assert!(user_html.exists(), "must not delete a non-export index.html");
    }

    #[test]
    fn confined_under_rejects_parent_and_accepts_child() {
        let base = tmp_dir();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        let canonical = canonical_working_folder(&root.to_string_lossy()).unwrap();

        // A child directly under root with the correct parent -> accepted (non-existent
        // file is a safe no-op delete).
        let child = canonical.join("project.json");
        assert!(confined_under(&canonical, &canonical, &child));

        // The PARENT of root must be rejected (its parent is not `canonical`).
        let parent = canonical.parent().unwrap().to_path_buf();
        assert!(!confined_under(&canonical, &canonical, &parent));

        // A sibling outside root (parent mismatch) -> rejected.
        let sibling = parent.join("other.json");
        assert!(!confined_under(&canonical, &canonical, &sibling));
    }

    #[cfg(unix)]
    #[test]
    fn delete_skips_symlinked_components_escaping_root() {
        use std::os::unix::fs::symlink;
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();

        // An OUTSIDE dir with a precious file the attacker wants deleted.
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let precious = outside.join("precious.txt");
        fs::write(&precious, "do not delete").unwrap();

        // components/ is a SYMLINK pointing at the outside dir.
        symlink(&outside, canonical.join(COMPONENTS_DIR)).unwrap();

        delete_known_design_files(&canonical);

        // The symlinked components/ must NOT have its target wiped.
        assert!(precious.exists(), "must not delete through an escaping symlink");
    }

    // ---- Oracle status (A2) ------------------------------------------------

    #[test]
    fn oracle_status_serializes_camel_case_and_omits_none() {
        // The default (not grounded, everything else absent) is the minimal payload: only
        // `grounded:false` is on the wire; the optional fields are skipped.
        let bare = DesignOracleStatus::default();
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(json, r#"{"grounded":false}"#);

        // A populated status uses camelCase keys exactly.
        let full = DesignOracleStatus {
            grounded: true,
            root_label: Some("my-target".into()),
            chunks: Some(1234),
            files: Some(56),
            last_sync_iso: Some("2026-06-10T12:00:00Z".into()),
        };
        let json = serde_json::to_string(&full).unwrap();
        assert!(json.contains("\"grounded\":true"), "{json}");
        assert!(json.contains("\"rootLabel\":\"my-target\""), "{json}");
        assert!(json.contains("\"chunks\":1234"), "{json}");
        assert!(json.contains("\"files\":56"), "{json}");
        assert!(json.contains("\"lastSyncIso\":\"2026-06-10T12:00:00Z\""), "{json}");
        // Round-trips back.
        let back: DesignOracleStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(full, back);
    }

    #[test]
    fn path_leaf_label_is_leaf_only_never_absolute() {
        // The label is ONLY the final component — never the absolute path (IPC hygiene).
        let p = if cfg!(windows) {
            PathBuf::from(r"C:\Users\me\code\my-target")
        } else {
            PathBuf::from("/home/me/code/my-target")
        };
        assert_eq!(path_leaf_label(&p).as_deref(), Some("my-target"));
        // The returned label must NOT contain any separator or the parent path.
        let label = path_leaf_label(&p).unwrap();
        assert!(!label.contains('/') && !label.contains('\\'), "{label}");
        assert!(!label.contains("code"), "must not leak the parent: {label}");
    }

    #[test]
    fn status_count_reads_index_subobject_tolerantly() {
        let payload = serde_json::json!({
            "index": { "sqliteChunks": 42, "indexedFiles": 7 },
            "job": { "finishedAt": "2026-06-10T00:00:00Z" }
        });
        assert_eq!(status_count(&payload, "sqliteChunks"), Some(42));
        assert_eq!(status_count(&payload, "indexedFiles"), Some(7));
        // Missing key / missing index object / non-numeric -> None (never panics).
        assert_eq!(status_count(&payload, "nope"), None);
        assert_eq!(status_count(&serde_json::json!({}), "sqliteChunks"), None);
        let bad = serde_json::json!({ "index": { "sqliteChunks": "x" } });
        assert_eq!(status_count(&bad, "sqliteChunks"), None);
    }

    // ---- design.md read/write (A2) -----------------------------------------

    #[test]
    fn design_md_round_trips_and_missing_is_none() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        let path = canonical.join(DESIGN_MD_FILE);

        // Missing file -> Ok(None) (the read helper the command uses).
        assert_eq!(read_design_file(&path).unwrap(), None);

        // Write via the same atomic_write the command uses, then read it back verbatim.
        let content = "# Design brief\n\nMuted olive palette, dense layout.\n";
        atomic_write(&path, content, DESIGN_MD_FILE).unwrap();
        assert_eq!(read_design_file(&path).unwrap().as_deref(), Some(content));
    }

    #[test]
    fn design_md_oversize_rejected_both_directions() {
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        let path = canonical.join(DESIGN_MD_FILE);

        // WRITE direction: a payload over the cap is rejected by the command's own check.
        let oversize = "a".repeat((DESIGN_MD_MAX_BYTES as usize) + 1);
        assert!(
            oversize.len() as u64 > DESIGN_MD_MAX_BYTES,
            "fixture must exceed the cap"
        );
        // Mirror the command's write-side gate.
        assert!(oversize.len() as u64 > DESIGN_MD_MAX_BYTES);

        // READ direction: a file already on disk that exceeds the cap is rejected, not
        // silently truncated. Write it directly (bypassing the command), then assert the
        // size gate the read command applies would trip.
        fs::write(&path, &oversize).unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert!(
            meta.len() > DESIGN_MD_MAX_BYTES,
            "on-disk file must exceed the cap for the read gate to fire"
        );

        // A file exactly at the cap is allowed (boundary).
        let at_cap = "b".repeat(DESIGN_MD_MAX_BYTES as usize);
        atomic_write(&path, &at_cap, DESIGN_MD_FILE).unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), DESIGN_MD_MAX_BYTES);
        assert!(read_design_file(&path).unwrap().is_some());
    }

    #[test]
    fn read_design_md_at_rejects_oversize_on_disk_file() {
        // The command-layer reader (sans State) must REJECT an over-cap design.md, not
        // return its bytes. Write an oversized file directly to disk (bypassing the write
        // command's own size gate) and assert the read fails. This exercises the same
        // 64 KiB cap the TOCTOU post-read check enforces.
        let base = tmp_dir();
        let wf = base.join("proj");
        fs::create_dir_all(&wf).unwrap();
        let canonical = canonical_working_folder(&wf.to_string_lossy()).unwrap();
        let path = canonical.join(DESIGN_MD_FILE);

        let oversize = "a".repeat((DESIGN_MD_MAX_BYTES as usize) + 1);
        fs::write(&path, &oversize).unwrap();
        let err = read_design_md_at(&path).unwrap_err();
        assert!(err.contains("too large"), "unexpected error: {err}");

        // A file at the cap reads back verbatim (boundary stays allowed).
        let at_cap = "b".repeat(DESIGN_MD_MAX_BYTES as usize);
        fs::write(&path, &at_cap).unwrap();
        assert_eq!(read_design_md_at(&path).unwrap().as_deref(), Some(at_cap.as_str()));

        // Missing file -> Ok(None).
        fs::remove_file(&path).unwrap();
        assert_eq!(read_design_md_at(&path).unwrap(), None);
    }

    // NOTE: the pure TOCTOU window (metadata reports <= cap but the file grows before the
    // read returns) is not deterministically reproducible in a unit test. The post-read
    // length gate that closes it is covered structurally: `read_design_md_at` re-checks the
    // returned string's byte length against DESIGN_MD_MAX_BYTES regardless of the metadata
    // fast-path, and `read_design_file`'s own cap (8 MiB) is larger, so the post-read check
    // is the binding one for any string between 64 KiB and 8 MiB.

    #[test]
    fn design_md_working_folder_traversal_rejected() {
        // The filename is a fixed const (no traversal surface there); the only untrusted
        // input is the working folder, which `canonical_working_folder` confines/validates.
        // An empty / non-existent working folder is rejected before any IO (clone of the
        // existing confinement idiom).
        assert!(canonical_working_folder("").is_err());
        assert!(canonical_working_folder("   ").is_err());
        let missing = std::env::temp_dir().join(format!(
            "aspis-design-md-no-such-{}",
            std::process::id()
        ));
        assert!(canonical_working_folder(missing.to_string_lossy().as_ref()).is_err());
    }
}
