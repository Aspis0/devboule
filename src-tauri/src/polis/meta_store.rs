//! Polis Map — `.aspis-meta.json` stable-ID store.
//!
//! Lives in the scanned project root, treated as untracked (the doc says it
//! belongs in `.gitignore`). It maps `file_path -> file_id (UUID v4)` and also
//! persists each building's `coords` and any learned `purpose` overrides, so a
//! re-scan keeps stable IDs, stable layout, and learned classifications.
//!
//! Design guarantees:
//! - On first scan, generate UUID v4 for each file and persist.
//! - On later scans, reuse existing IDs — renames/re-scans don't lose history.
//! - A missing or corrupt meta file never panics: we start fresh.
//!
//! All keys are stored using forward-slash, project-relative paths so the store
//! is stable across OSes and absolute-path changes.

use crate::polis::model::{Coords, Feature};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

/// Process-level serialization lock for ALL `.aspis-meta.json` writes.
///
/// FIX 1 (meta-write clobber): multiple commands (scanner terminal save,
/// `polis_generate_dossier`, `polis_set_scan_extensions`, `reset_city_to_new_era`,
/// `polis_reclassify_features` + its rollback) each do a load → mutate → save of the
/// WHOLE `MetaStore`. Without serialization, a writer carrying only the fields it
/// loaded silently reverts another writer's fields that landed between its load and
/// its save. A single global mutex fully serializes the load-modify-save section so
/// no writer ever clobbers another's fields. Meta writes are infrequent (explicit
/// user actions + one terminal save per scan), so a single global lock has no
/// meaningful contention cost.
///
/// CRITICAL: the lock is acquired INSIDE `with_write_lock` and held ONLY across the
/// (fast, synchronous) reload→mutate→atomic-save. It is NEVER held across an Oracle
/// `.await`: callers do the Oracle call FIRST without the lock, then call
/// `with_write_lock` to apply+persist their own fields onto the freshest on-disk
/// state. This is a single-process lock; it does not coordinate across separate OS
/// processes, but Polis runs the meta writers in one Tauri process so that is moot.
static META_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Polis F2 — Oracle's human label + 1-line description override for one feature.
/// Persisted in `.aspis-meta.json` (`feature_label_overrides`) so the Oracle's
/// naming is REUSED on every scan WITHOUT re-contacting the Oracle. Applied
/// deterministically by the scanner when building the feature registry (see
/// `scanner::apply_feature_overrides`); absent fields fall back to the F1
/// deterministic label / empty description.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureLabelOverride {
    /// Human product-area name (e.g. `"RNA-seq"`). Empty -> keep F1 label.
    pub label: String,
    /// One-line description of the feature. Empty -> no description.
    pub description: String,
}

/// File name written in the project root.
pub const META_FILE_NAME: &str = ".aspis-meta.json";

/// Polis 4b — a PERSISTED narrative "More details" dossier for one file: a deeper,
/// product-level, plain-language Oracle explanation of what the file is
/// RESPONSIBLE for / what DECISIONS it makes / how it ORCHESTRATES the flow.
///
/// `fingerprint` is the file's content hash (`content_fingerprint`) AT THE TIME
/// the dossier was generated. The dossier is considered STALE — and regenerated
/// on the next explicit "More details" — when the file's CURRENT content hash no
/// longer equals this. Persisting the fingerprint (not an mtime) means "changed"
/// only fires when the bytes actually changed. Generated ONLY on an explicit user
/// action (lazy), never on a scan, and only re-generated after a content change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDossier {
    /// The narrative product-level prose returned by the Oracle.
    pub text: String,
    /// The file's content hash when this dossier text was generated.
    pub fingerprint: String,
}

/// Stable, deterministic FNV-1a 64-bit content fingerprint, hex-encoded. Used as
/// the dossier staleness witness: it changes iff the file's bytes change. Pure, no
/// RNG, no per-process seed — identical input always yields identical output, so a
/// fingerprint persisted on one run compares correctly on the next. Computed from
/// the content the scanner ALREADY read (no extra IO).
pub fn content_fingerprint(content: &str) -> String {
    // FNV-1a constants (same family as the scanner's color hash; kept here so the
    // dossier persistence layer owns its own fingerprint and both the scanner and
    // the commands hash content identically).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Per-file persisted record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub file_id: String,
    /// Last persisted layout position, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coords: Option<Coords>,
    /// Learned purpose override (e.g. from the Oracle later). Takes precedence
    /// over the heuristic during classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Persisted Polis F1 feature assignment for this file (the `Feature::id`).
    /// Reused on a re-scan when the file's directory-spine inputs are unchanged
    /// (stability), mirroring the coord-persistence reuse logic. `None` until the
    /// file has been assigned. Set alongside `feature_source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_id: Option<String>,
    /// Provenance of the persisted `feature_id` ("directory"|"commons"|"default").
    /// Persisted so the reused assignment carries its source across scans without
    /// recomputation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_source: Option<String>,
    /// Polis F3: the DISTRICT id this file was last laid out into (the resolved
    /// feature district, which may be `commons` for a sub-`MIN_DISTRICT_BUILDINGS`
    /// feature folded into commons). Persisted so the coord-reuse fast path can
    /// tell when a building's DISTRICT ASSIGNMENT changed (feature move / fold
    /// flip) and must therefore repack instead of reusing a coord that lands it in
    /// the old district. `None` until the file has been laid out once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub district_id: Option<String>,
    /// The directory-spine INPUT KEY the persisted `feature_id` was computed from
    /// (the raw dir-spine slug BEFORE commons routing, or `""` for a root file).
    /// The stability check compares the file's CURRENT spine key against this:
    /// equal -> reuse the persisted assignment; changed (file moved to a new
    /// dir) -> recompute. Keeping the input alongside the output is what makes
    /// "reuse only when inputs unchanged" decidable without re-running the whole
    /// assignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature_spine: Option<String>,
    /// Polis 4b: the persisted narrative "More details" dossier for this file, if
    /// one has been generated. Written ONLY by `polis_generate_dossier` on a
    /// successful Oracle answer; carries its own content fingerprint for staleness.
    /// `#[serde(default, skip_serializing_if)]` so an older meta file (pre-4b) loads
    /// and a file with no dossier writes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dossier: Option<FileDossier>,
}

/// On-disk shape of `.aspis-meta.json`.
///
/// `version` lets us migrate later. Unknown fields are ignored on load so a
/// newer file written by a future build still parses (best-effort).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaStore {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Current prestige era ("Alpha", "Beta", ...). Persisted so a re-scan
    /// reflects the *real* current era instead of a hardcoded constant. Defaults
    /// to "Alpha" on first run (no era reset has happened yet).
    #[serde(default = "default_era")]
    pub era: String,
    /// project-relative (forward-slash) path -> per-file metadata.
    #[serde(default)]
    pub files: BTreeMap<String, FileMeta>,
    /// Per-workspace scan extension override (lowercase, no leading dot). `None`
    /// = never configured → the scanner uses its built-in default set. `Some`
    /// (even empty) = an explicit user choice from the in-game File-Types menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_extensions: Option<Vec<String>>,
    /// Persisted Polis F1 feature registry (id/label/color/kind), keyed
    /// implicitly by `Feature::id`. Persisted at the top level — separate from
    /// the per-file `feature_id` — so the registry is STABLE across scans and
    /// survives the Oracle being unavailable (F2 will refine labels/descriptions
    /// in place without losing F1's structural keys). `#[serde(default)]` so an
    /// older meta file with no `features` still loads.
    #[serde(default)]
    pub features: Vec<Feature>,
    /// Polis F2 — Oracle's human label + 1-line description per feature id. Keyed
    /// by `Feature::id`. Filled ONLY by the explicit `polis_reclassify_features`
    /// command (never per-scan); the scanner READS it deterministically to build
    /// the registry, so once written it is reused offline every scan. An override
    /// for a feature id that no longer exists is simply ignored. `#[serde(default)]`
    /// so an older meta file (pre-F2) loads with an empty map.
    #[serde(default)]
    pub feature_label_overrides: BTreeMap<String, FeatureLabelOverride>,
    /// Polis F2 — Oracle's cross-tree feature UNIFICATION: `source_feature_id ->
    /// canonical_feature_id` (e.g. `web_rnaseq -> rnaseq`, `workers_rnaseq ->
    /// rnaseq`). Persisted so the merge is reused every scan WITHOUT re-asking the
    /// Oracle. The scanner remaps each building's `feature_id` through this map to
    /// its canonical id (transitively, to a fixed point, with cycles broken
    /// deterministically — see `scanner::resolve_canonical_feature`) so merged
    /// features collapse into ONE district in F3. Filled ONLY by
    /// `polis_reclassify_features`. `#[serde(default)]` so a pre-F2 meta loads empty.
    #[serde(default)]
    pub feature_merges: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    1
}

/// Default era on first run, before any `reset_city_to_new_era` call.
pub fn default_era() -> String {
    "Alpha".to_string()
}

impl Default for MetaStore {
    fn default() -> Self {
        Self {
            version: default_version(),
            era: default_era(),
            files: BTreeMap::new(),
            enabled_extensions: None,
            features: Vec::new(),
            feature_label_overrides: BTreeMap::new(),
            feature_merges: BTreeMap::new(),
        }
    }
}

impl MetaStore {
    /// Full path of the meta file inside `project_root`.
    pub fn path_in(project_root: &Path) -> PathBuf {
        project_root.join(META_FILE_NAME)
    }

    /// Load the store from `project_root`. A missing or corrupt file yields a
    /// fresh, empty store — this never errors and never panics. Corruption is
    /// silently recovered (we'd rather regenerate IDs than crash the scan).
    pub fn load(project_root: &Path) -> Self {
        let path = Self::path_in(project_root);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        match serde_json::from_slice::<MetaStore>(&bytes) {
            Ok(store) => store,
            // Corrupt / unparseable: start fresh rather than panicking.
            Err(_) => Self::default(),
        }
    }

    /// Persist the store to `project_root/.aspis-meta.json` (pretty JSON),
    /// ATOMICALLY: serialize to a sibling temp file in the SAME directory, then
    /// rename it over the target. `std::fs::rename` is an atomic replace on the
    /// same volume on BOTH Unix and Windows (Windows `MoveFileEx` with
    /// `MOVEFILE_REPLACE_EXISTING`, which Rust's libstd uses), so a crash or
    /// interruption mid-write can never leave a half-written `.aspis-meta.json`
    /// that `load` would discard as corrupt — the old file stays intact until
    /// the rename completes, and the rename either fully succeeds or doesn't
    /// happen. This protects file ids, coords, and the feature registry together.
    ///
    /// NOTE (Windows): a same-directory rename onto an existing destination
    /// replaces it atomically; this requires the temp file to live on the SAME
    /// volume as the target — guaranteed here since it's a sibling in the same
    /// directory. On any failure we remove the temp file (best-effort) so we
    /// don't leak a `.tmp` and return the error (the call site keeps save
    /// best-effort).
    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = Self::path_in(project_root);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize meta store: {e}"))?;

        // Sibling temp file in the SAME directory (same volume -> atomic rename).
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, json) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Failed to write {}: {e}", tmp.display()));
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Failed to replace {}: {e}", path.display()));
        }
        Ok(())
    }

    /// FIX 1 — the ONLY sanctioned way to PERSIST a change to `.aspis-meta.json`.
    ///
    /// Serializes every meta write through `META_WRITE_LOCK`, then RELOADS the
    /// freshest store from disk INSIDE the lock, runs `f` to mutate ONLY the
    /// caller's own fields on that fresh load, and atomically saves. Because the
    /// load and the save happen under the same lock, a concurrent writer can never
    /// interleave between this caller's load and save, so the caller never reverts
    /// fields another writer persisted: each writer touches only its own fields on
    /// top of everyone else's already-on-disk state.
    ///
    /// USAGE RULE (enforced by convention): do any Oracle `.await` BEFORE calling
    /// this — never inside `f`. `f` must be a short, synchronous closure that only
    /// applies the caller's fields. Holding the lock across an `.await` would
    /// serialize unrelated multi-second Oracle calls and risk deadlock; the lock is
    /// for the disk reload→save only.
    ///
    /// Returns whatever `f` returns on a successful save, or the save error.
    /// `f` runs BEFORE the save; if the save fails the in-memory mutation is
    /// discarded (nothing was persisted) and the error is returned.
    ///
    /// RESIDUAL RACE: none within this process — load+save are fully serialized
    /// under the lock. The lock does not span separate OS processes, but Polis runs
    /// all meta writers in the single Tauri process, so meta writes are totally
    /// ordered.
    pub fn with_write_lock<R>(
        project_root: &Path,
        f: impl FnOnce(&mut MetaStore) -> R,
    ) -> Result<R, String> {
        // A poisoned lock means a previous holder panicked mid-section. The meta
        // store is plain data with no cross-call invariant broken by a panic, so we
        // recover the guard and proceed rather than propagating the poison.
        let _guard = META_WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fresh = Self::load(project_root);
        let out = f(&mut fresh);
        fresh.save(project_root)?;
        Ok(out)
    }

    /// Current persisted era (defaults to "Alpha" on a fresh store).
    pub fn era(&self) -> &str {
        &self.era
    }

    /// Persist the current era (called by `reset_city_to_new_era`).
    pub fn set_era(&mut self, era: impl Into<String>) {
        self.era = era.into();
    }

    /// The per-workspace scan extension override, if the user has set one
    /// (`None` means "use the scanner's built-in default set").
    pub fn enabled_extensions(&self) -> Option<&Vec<String>> {
        self.enabled_extensions.as_ref()
    }

    /// Persist a per-workspace scan extension override (from the in-game
    /// File-Types menu). Stored verbatim; the caller is expected to have
    /// sanitized to lowercase, dot-stripped, de-duplicated extensions.
    pub fn set_enabled_extensions(&mut self, exts: Vec<String>) {
        self.enabled_extensions = Some(exts);
    }

    /// Return the stable `file_id` for `rel_path`, generating and storing a new
    /// UUID v4 on first sight. `rel_path` MUST be the normalized,
    /// project-relative, forward-slash key.
    pub fn ensure_file_id(&mut self, rel_path: &str) -> String {
        if let Some(meta) = self.files.get(rel_path) {
            if !meta.file_id.is_empty() {
                return meta.file_id.clone();
            }
        }
        let id = Uuid::new_v4().to_string();
        let entry = self.files.entry(rel_path.to_string()).or_default();
        entry.file_id = id.clone();
        id
    }

    /// Look up an existing `file_id` without creating one.
    pub fn file_id(&self, rel_path: &str) -> Option<String> {
        self.files
            .get(rel_path)
            .map(|m| m.file_id.clone())
            .filter(|id| !id.is_empty())
    }

    /// Persisted coords for `rel_path`, if any.
    pub fn coords(&self, rel_path: &str) -> Option<Coords> {
        self.files.get(rel_path).and_then(|m| m.coords)
    }

    /// Persist coords for `rel_path`. Creates the entry if missing (with an
    /// empty id — callers normally call `ensure_file_id` first).
    pub fn set_coords(&mut self, rel_path: &str, coords: Coords) {
        self.files.entry(rel_path.to_string()).or_default().coords = Some(coords);
    }

    /// The district id `rel_path` was last laid out into (Polis F3), if any.
    pub fn district(&self, rel_path: &str) -> Option<String> {
        self.files.get(rel_path).and_then(|m| m.district_id.clone())
    }

    /// Persist the district id `rel_path` was laid out into (Polis F3).
    pub fn set_district(&mut self, rel_path: &str, district_id: impl Into<String>) {
        self.files
            .entry(rel_path.to_string())
            .or_default()
            .district_id = Some(district_id.into());
    }

    /// Learned purpose override for `rel_path`, if any.
    pub fn purpose(&self, rel_path: &str) -> Option<String> {
        self.files.get(rel_path).and_then(|m| m.purpose.clone())
    }

    /// Persist a learned purpose override for `rel_path`.
    // POLIS FOLLOW-UP: the Oracle classifier will write learned purposes here
    // so they survive across scans and can be refined manually by the user.
    pub fn set_purpose(&mut self, rel_path: &str, purpose: impl Into<String>) {
        self.files.entry(rel_path.to_string()).or_default().purpose = Some(purpose.into());
    }

    /// The persisted Polis F1 feature assignment for `rel_path`, if any:
    /// `(feature_id, feature_source, feature_spine)`. The scanner reuses it only
    /// when the file's CURRENT dir-spine equals the persisted `feature_spine`
    /// (inputs unchanged), mirroring the coord-reuse stability rule.
    pub fn feature(&self, rel_path: &str) -> Option<(String, String, String)> {
        let m = self.files.get(rel_path)?;
        Some((
            m.feature_id.clone()?,
            m.feature_source.clone().unwrap_or_default(),
            m.feature_spine.clone().unwrap_or_default(),
        ))
    }

    /// Persist a Polis F1 feature assignment for `rel_path`. `spine` is the raw
    /// directory-spine input key the assignment was computed from (the stability
    /// witness — see `FileMeta::feature_spine`). Creates the entry if missing.
    pub fn set_feature(
        &mut self,
        rel_path: &str,
        feature_id: impl Into<String>,
        feature_source: impl Into<String>,
        spine: impl Into<String>,
    ) {
        let e = self.files.entry(rel_path.to_string()).or_default();
        e.feature_id = Some(feature_id.into());
        e.feature_source = Some(feature_source.into());
        e.feature_spine = Some(spine.into());
    }

    /// The persisted narrative dossier for `rel_path`, if one has been generated
    /// (Polis 4b). `None` when no dossier exists yet.
    pub fn dossier(&self, rel_path: &str) -> Option<&FileDossier> {
        self.files.get(rel_path).and_then(|m| m.dossier.as_ref())
    }

    /// Persist a narrative dossier for `rel_path` with its content fingerprint
    /// (Polis 4b). Called ONLY by `polis_generate_dossier` on a successful Oracle
    /// answer. Creates the entry if missing.
    pub fn set_dossier(
        &mut self,
        rel_path: &str,
        text: impl Into<String>,
        fingerprint: impl Into<String>,
    ) {
        self.files.entry(rel_path.to_string()).or_default().dossier = Some(FileDossier {
            text: text.into(),
            fingerprint: fingerprint.into(),
        });
    }

    /// FIX 1 — apply the SCANNER-OWNED fields from the scanner's in-memory store
    /// (`self`, built during `generate_city_state`) onto a FRESH on-disk load
    /// (`disk`), preserving every field the scanner does NOT own. Called as the
    /// `f` closure inside `with_write_lock` for the scanner's terminal save, so the
    /// scanner persists its per-file layout/feature data WITHOUT clobbering fields
    /// other writers persisted while the scan was walking the tree (which can take
    /// seconds): `dossier` (written by `polis_generate_dossier`), `feature_merges`
    /// + `feature_label_overrides` (written by `polis_reclassify_features`),
    /// `enabled_extensions` (written by `polis_set_scan_extensions`), and `era`
    /// (written by `reset_city_to_new_era`).
    ///
    /// Scanner-owned (overwritten on `disk`): the per-file entries the scanner
    /// rebuilds each scan — `file_id`, `coords`, `district_id`,
    /// `feature_id`/`feature_source`/`feature_spine` — plus the top-level `features`
    /// registry. The file SET is also scanner-owned: a path the scanner pruned this
    /// scan (deleted file) is removed from `disk` so the store stays bounded.
    ///
    /// NOT scanner-owned (kept from `disk`): per-file `dossier` and `purpose` (lazy
    /// Oracle-write fields the scanner only ever reads), and the top-level
    /// `feature_label_overrides`, `feature_merges`, `enabled_extensions`, `era`.
    /// This REPLACES the old `merge_generate_only_from_disk` reload-narrowing: under
    /// the write lock the reload is the SAME fresh load `f` mutates, so there is no
    /// residual window.
    pub fn apply_scanner_owned_onto(&self, disk: &mut MetaStore) {
        // The file SET the scanner saw is authoritative (it already ran
        // `retain_paths` to prune deleted files). Drop any disk entry the scanner
        // no longer has, so deletions are honored.
        disk.files.retain(|path, _| self.files.contains_key(path));

        for (path, scan_meta) in &self.files {
            let entry = disk.files.entry(path.clone()).or_default();
            // Scanner-owned per-file fields: overwrite from the scan.
            entry.file_id = scan_meta.file_id.clone();
            entry.coords = scan_meta.coords;
            entry.district_id = scan_meta.district_id.clone();
            entry.feature_id = scan_meta.feature_id.clone();
            entry.feature_source = scan_meta.feature_source.clone();
            entry.feature_spine = scan_meta.feature_spine.clone();
            // `dossier` and `purpose` are NOT scanner-owned: leave the disk value
            // intact (a mid-scan `polis_generate_dossier` / Oracle purpose write
            // must survive the scanner's save).
        }

        // Top-level registry is scanner-owned (recomputed each scan).
        disk.features = self.features.clone();
        // `era`, `enabled_extensions`, `feature_label_overrides`, `feature_merges`
        // are left as loaded from disk — owned by their respective commands.
    }

    /// The persisted Polis F1 feature registry (id/label/color/kind).
    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// Replace the persisted Polis F1 feature registry.
    pub fn set_features(&mut self, features: Vec<Feature>) {
        self.features = features;
    }

    /// The persisted Polis F2 Oracle label/description overrides (id -> override).
    /// Read by the scanner to build the registry; never auto-written per scan.
    pub fn feature_label_overrides(&self) -> &BTreeMap<String, FeatureLabelOverride> {
        &self.feature_label_overrides
    }

    /// Replace the persisted Polis F2 Oracle label/description overrides. Called
    /// ONLY by the explicit `polis_reclassify_features` command on success.
    pub fn set_feature_label_overrides(
        &mut self,
        overrides: BTreeMap<String, FeatureLabelOverride>,
    ) {
        self.feature_label_overrides = overrides;
    }

    /// The persisted Polis F2 cross-tree merge map (source id -> canonical id).
    /// Read by the scanner to remap each building's feature; never per-scan write.
    pub fn feature_merges(&self) -> &BTreeMap<String, String> {
        &self.feature_merges
    }

    /// Replace the persisted Polis F2 cross-tree merge map. Called ONLY by the
    /// explicit `polis_reclassify_features` command on success.
    pub fn set_feature_merges(&mut self, merges: BTreeMap<String, String>) {
        self.feature_merges = merges;
    }

    /// Drop entries whose path is no longer present in `keep`. Keeps the file
    /// from growing unbounded after deletions, while still surviving renames
    /// within a single scan (callers pass the full current path set).
    pub fn retain_paths(&mut self, keep: &std::collections::HashSet<String>) {
        self.files.retain(|k, _| keep.contains(k));
    }
}

/// Normalize a project-relative path into the canonical store key: strip a
/// leading `./`, convert backslashes to forward slashes, trim slashes.
pub fn normalize_rel_path(rel: &str) -> String {
    let replaced = rel.replace('\\', "/");
    let trimmed = replaced.trim_start_matches("./").trim_matches('/');
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique temp dir under the OS temp folder; cleaned up by `Drop`.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("polis_meta_{tag}_{pid}_{nanos}_{n}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn normalize_rel_path_canonicalizes_separators() {
        assert_eq!(normalize_rel_path("./src\\main.tsx"), "src/main.tsx");
        assert_eq!(normalize_rel_path("/src/main.tsx/"), "src/main.tsx");
        assert_eq!(normalize_rel_path("src/main.tsx"), "src/main.tsx");
    }

    #[test]
    fn missing_meta_file_starts_fresh_without_panicking() {
        let dir = TempDir::new("missing");
        let store = MetaStore::load(&dir.path);
        assert!(store.files.is_empty());
        assert_eq!(store.version, 1);
    }

    #[test]
    fn corrupt_meta_file_recovers_to_empty_store() {
        let dir = TempDir::new("corrupt");
        std::fs::write(
            MetaStore::path_in(&dir.path),
            b"{ this is not valid json :::",
        )
        .unwrap();
        let store = MetaStore::load(&dir.path);
        assert!(
            store.files.is_empty(),
            "corrupt file must yield empty store"
        );
    }

    #[test]
    fn empty_file_recovers_to_empty_store() {
        let dir = TempDir::new("emptyfile");
        std::fs::write(MetaStore::path_in(&dir.path), b"").unwrap();
        let store = MetaStore::load(&dir.path);
        assert!(store.files.is_empty());
    }

    #[test]
    fn uuid_is_stable_across_save_load_and_second_scan() {
        let dir = TempDir::new("stable");

        // First "scan": generate ids and persist.
        let mut first = MetaStore::load(&dir.path);
        let id_main = first.ensure_file_id("src/main.tsx");
        let id_client = first.ensure_file_id("src/oracle/client.ts");
        first.set_coords("src/main.tsx", Coords::new(3.0, 4.0));
        first.save(&dir.path).unwrap();

        // Calling ensure again in the same store is idempotent.
        assert_eq!(first.ensure_file_id("src/main.tsx"), id_main);

        // Second "scan": fresh load, ids must be identical.
        let mut second = MetaStore::load(&dir.path);
        assert_eq!(second.ensure_file_id("src/main.tsx"), id_main);
        assert_eq!(second.ensure_file_id("src/oracle/client.ts"), id_client);
        // Coords survived the round-trip.
        assert_eq!(second.coords("src/main.tsx"), Some(Coords::new(3.0, 4.0)));
    }

    #[test]
    fn rename_keeps_old_id_when_caller_remaps_key() {
        // A rename is modeled by the scanner moving the entry to the new key.
        // The store itself keys by path; this test documents that a fresh path
        // gets a *new* id unless the caller deliberately reuses the record.
        let dir = TempDir::new("rename");
        let mut store = MetaStore::load(&dir.path);
        let old_id = store.ensure_file_id("src/old_name.ts");

        // Simulate a rename: caller transplants the record under the new key
        // (this is the "renaming a file doesn't destroy history" behavior).
        let record = store.files.remove("src/old_name.ts").unwrap();
        store.files.insert("src/new_name.ts".to_string(), record);

        assert_eq!(store.file_id("src/new_name.ts"), Some(old_id));
        assert!(store.file_id("src/old_name.ts").is_none());
    }

    #[test]
    fn distinct_paths_get_distinct_ids() {
        let dir = TempDir::new("distinct");
        let mut store = MetaStore::load(&dir.path);
        let a = store.ensure_file_id("src/a.ts");
        let b = store.ensure_file_id("src/b.ts");
        assert_ne!(a, b);
    }

    #[test]
    fn purpose_override_persists() {
        let dir = TempDir::new("purpose");
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("src/x.ts");
        store.set_purpose("src/x.ts", "laboratorio");
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(reloaded.purpose("src/x.ts").as_deref(), Some("laboratorio"));
    }

    #[test]
    fn era_defaults_to_alpha_and_persists_across_save_load() {
        let dir = TempDir::new("era");
        // Fresh store defaults to Alpha (no era reset has happened yet).
        let mut store = MetaStore::load(&dir.path);
        assert_eq!(store.era(), "Alpha");

        store.set_era("Beta");
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(reloaded.era(), "Beta", "era must survive the round-trip");
    }

    #[test]
    fn era_missing_in_old_meta_file_defaults_to_alpha() {
        // An older meta file written before the `era` field existed must still
        // load, defaulting era to "Alpha" rather than failing to parse.
        let dir = TempDir::new("era_legacy");
        std::fs::write(
            MetaStore::path_in(&dir.path),
            br#"{"version":1,"files":{}}"#,
        )
        .unwrap();
        let store = MetaStore::load(&dir.path);
        assert_eq!(store.era(), "Alpha");
    }

    #[test]
    fn retain_paths_drops_deleted_entries() {
        let dir = TempDir::new("retain");
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("src/a.ts");
        store.ensure_file_id("src/b.ts");

        let mut keep = HashSet::new();
        keep.insert("src/a.ts".to_string());
        store.retain_paths(&keep);

        assert!(store.file_id("src/a.ts").is_some());
        assert!(store.file_id("src/b.ts").is_none());
    }

    #[test]
    fn feature_assignment_and_registry_survive_save_load() {
        use crate::polis::model::{Feature, FeatureKind};
        let dir = TempDir::new("feature");
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("apps/web/rnaseq/quant.ts");
        store.set_feature("apps/web/rnaseq/quant.ts", "rnaseq", "directory", "rnaseq");
        store.set_features(vec![Feature {
            id: "rnaseq".into(),
            label: "Rnaseq".into(),
            description: String::new(),
            color_accent: "#C17A5A".into(),
            kind: FeatureKind::Domain,
        }]);
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(
            reloaded.feature("apps/web/rnaseq/quant.ts"),
            Some(("rnaseq".into(), "directory".into(), "rnaseq".into())),
            "per-file feature assignment must survive the round-trip"
        );
        assert_eq!(reloaded.features().len(), 1);
        assert_eq!(reloaded.features()[0].id, "rnaseq");
        assert_eq!(reloaded.features()[0].kind, FeatureKind::Domain);
    }

    // FIX 5: atomic save. The save must (a) round-trip ids/coords/features
    // together, (b) leave NO `.tmp` sibling behind, and (c) overwrite an
    // existing target in place (Windows rename-replace semantics: a
    // same-directory rename onto an existing file replaces it atomically).
    #[test]
    fn save_is_atomic_round_trips_and_leaves_no_temp_file() {
        use crate::polis::model::{Feature, FeatureKind};
        let dir = TempDir::new("atomic");

        // First save establishes a target file.
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("src/a.ts");
        store.set_coords("src/a.ts", Coords::new(1.0, 2.0));
        store.set_features(vec![Feature {
            id: "auth".into(),
            label: "Auth".into(),
            description: String::new(),
            color_accent: "#C17A5A".into(),
            kind: FeatureKind::Domain,
        }]);
        store.save(&dir.path).unwrap();
        assert!(MetaStore::path_in(&dir.path).exists(), "target written");

        // Second save must REPLACE the existing target atomically (Windows: the
        // rename onto an existing destination succeeds rather than erroring).
        let id_a = store.file_id("src/a.ts").unwrap();
        store.ensure_file_id("src/b.ts");
        store.save(&dir.path).unwrap();

        // No leftover temp sibling after either save.
        let tmp = MetaStore::path_in(&dir.path).with_extension("json.tmp");
        assert!(!tmp.exists(), "atomic save must not leak a .tmp sibling");

        // Everything round-trips together after reload.
        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(reloaded.file_id("src/a.ts").as_deref(), Some(id_a.as_str()));
        assert!(reloaded.file_id("src/b.ts").is_some());
        assert_eq!(reloaded.coords("src/a.ts"), Some(Coords::new(1.0, 2.0)));
        assert_eq!(reloaded.features().len(), 1);
        assert_eq!(reloaded.features()[0].id, "auth");
    }

    #[test]
    fn legacy_meta_without_features_loads_with_empty_registry() {
        // A meta file written before F1 (no `features`, no per-file feature_*)
        // must still load — the new fields default to empty/None.
        let dir = TempDir::new("feature_legacy");
        std::fs::write(
            MetaStore::path_in(&dir.path),
            br#"{"version":1,"era":"Alpha","files":{"a.ts":{"fileId":"x"}}}"#,
        )
        .unwrap();
        let store = MetaStore::load(&dir.path);
        assert!(store.features().is_empty());
        assert_eq!(store.feature("a.ts"), None, "no persisted feature -> None");
        assert_eq!(store.file_id("a.ts").as_deref(), Some("x"));
    }

    #[test]
    fn feature_overrides_and_merges_round_trip_and_legacy_loads() {
        // F2: the Oracle label/description overrides + the cross-tree merge map
        // persist to `.aspis-meta.json` and reload byte-faithfully.
        let dir = TempDir::new("f2_overrides");
        let mut store = MetaStore::load(&dir.path);
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "rnaseq".to_string(),
            FeatureLabelOverride {
                label: "RNA-seq".into(),
                description: "RNA sequencing pipeline (frontend + workers).".into(),
            },
        );
        store.set_feature_label_overrides(overrides);
        let mut merges = BTreeMap::new();
        merges.insert("web_rnaseq".to_string(), "rnaseq".to_string());
        merges.insert("workers_rnaseq".to_string(), "rnaseq".to_string());
        store.set_feature_merges(merges);
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(
            reloaded.feature_label_overrides().get("rnaseq"),
            Some(&FeatureLabelOverride {
                label: "RNA-seq".into(),
                description: "RNA sequencing pipeline (frontend + workers).".into(),
            })
        );
        assert_eq!(
            reloaded
                .feature_merges()
                .get("web_rnaseq")
                .map(String::as_str),
            Some("rnaseq")
        );
        assert_eq!(
            reloaded
                .feature_merges()
                .get("workers_rnaseq")
                .map(String::as_str),
            Some("rnaseq")
        );
    }

    #[test]
    fn legacy_meta_without_f2_fields_loads_empty() {
        // A pre-F2 meta file (no `featureLabelOverrides` / `featureMerges`) must
        // still load, defaulting both to empty maps.
        let dir = TempDir::new("f2_legacy");
        std::fs::write(
            MetaStore::path_in(&dir.path),
            br#"{"version":1,"era":"Alpha","files":{"a.ts":{"fileId":"x"}}}"#,
        )
        .unwrap();
        let store = MetaStore::load(&dir.path);
        assert!(store.feature_label_overrides().is_empty());
        assert!(store.feature_merges().is_empty());
    }

    // -----------------------------------------------------------------------
    // Polis 4b — dossier persistence + content-fingerprint staleness.
    // -----------------------------------------------------------------------

    #[test]
    fn content_fingerprint_is_stable_and_changes_with_content() {
        // Deterministic: identical content -> identical fingerprint (no per-run seed).
        assert_eq!(
            content_fingerprint("hello world"),
            content_fingerprint("hello world")
        );
        // Different content -> different fingerprint.
        assert_ne!(
            content_fingerprint("hello world"),
            content_fingerprint("hello worlD")
        );
        // Empty content is hashable (non-UTF-8 files scan to "" — must not panic).
        assert_eq!(content_fingerprint(""), content_fingerprint(""));
        // 16 hex chars (u64).
        assert_eq!(content_fingerprint("x").len(), 16);
    }

    #[test]
    fn dossier_round_trips_through_save_load() {
        let dir = TempDir::new("dossier_rt");
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("src/worker.ts");
        let fp = content_fingerprint("fn main() {}");
        store.set_dossier(
            "src/worker.ts",
            "This worker orchestrates RNA-seq.",
            fp.clone(),
        );
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        let d = reloaded
            .dossier("src/worker.ts")
            .expect("dossier persisted");
        assert_eq!(d.text, "This worker orchestrates RNA-seq.");
        assert_eq!(d.fingerprint, fp);
    }

    #[test]
    fn dossier_stale_flips_when_content_hash_changes_and_is_false_when_unchanged() {
        // Model the get-dossier staleness rule directly: stale = no dossier OR
        // dossier.fingerprint != current content hash.
        let dir = TempDir::new("dossier_stale");
        let mut store = MetaStore::load(&dir.path);
        store.ensure_file_id("src/a.ts");

        let original = "export const x = 1;\n";
        let fp_original = content_fingerprint(original);
        store.set_dossier(
            "src/a.ts",
            "Dossier for original content.",
            fp_original.clone(),
        );

        // Unchanged content -> NOT stale.
        let current_same = content_fingerprint(original);
        let d = store.dossier("src/a.ts").unwrap();
        assert_eq!(
            d.fingerprint, current_same,
            "fingerprint matches unchanged content"
        );
        assert!(
            d.fingerprint == current_same,
            "dossier is fresh when the content hash is unchanged"
        );

        // Content changed -> the new hash differs -> the persisted dossier is stale.
        let changed = "export const x = 2;\n";
        let fp_changed = content_fingerprint(changed);
        let d2 = store.dossier("src/a.ts").unwrap();
        assert_ne!(
            d2.fingerprint, fp_changed,
            "stale: persisted fingerprint != changed content hash"
        );
    }

    #[test]
    fn legacy_meta_without_dossier_fields_loads_empty() {
        // A pre-4b meta file (no contentHash / dossier) must still load, defaulting
        // dossier to None — old meta loads, new behavior degrades to "needs generate".
        // (`contentHash` is an unknown field on load now that we no longer persist
        // it; serde ignores it, so an OLDER meta that still carries it loads fine.)
        let dir = TempDir::new("dossier_legacy");
        std::fs::write(
            MetaStore::path_in(&dir.path),
            br#"{"version":1,"era":"Alpha","files":{"a.ts":{"fileId":"x","contentHash":"deadbeefdeadbeef"}}}"#,
        )
        .unwrap();
        let store = MetaStore::load(&dir.path);
        assert!(store.dossier("a.ts").is_none());
        assert_eq!(store.file_id("a.ts").as_deref(), Some("x"));
    }

    // -----------------------------------------------------------------------
    // FIX 1 — serialized reload-before-save (with_write_lock) no-clobber.
    // -----------------------------------------------------------------------

    // (a) Two sequential `with_write_lock` calls, each setting a DIFFERENT field,
    // both persist. The second writer reloads the freshest disk inside the lock, so
    // it does NOT revert the first writer's field.
    #[test]
    fn with_write_lock_two_writers_different_fields_both_persist() {
        let dir = TempDir::new("wwl_two");

        // Writer A persists ONLY the era.
        MetaStore::with_write_lock(&dir.path, |m| m.set_era("Beta")).unwrap();
        // Writer B persists ONLY an extensions override (reloads A's era first).
        MetaStore::with_write_lock(&dir.path, |m| {
            m.set_enabled_extensions(vec!["rs".to_string()])
        })
        .unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(
            reloaded.era(),
            "Beta",
            "writer A's era must survive writer B"
        );
        assert_eq!(
            reloaded.enabled_extensions().cloned(),
            Some(vec!["rs".to_string()]),
            "writer B's extensions must persist"
        );
    }

    // (b) A scanner terminal save (via with_write_lock + apply_scanner_owned_onto)
    // that only sets scanner-owned fields PRESERVES an on-disk dossier AND
    // feature_merges AND enabled_extensions AND era written by prior calls.
    #[test]
    fn scanner_save_preserves_dossier_merges_extensions_and_era() {
        let dir = TempDir::new("wwl_scanner");

        // Prior writers populated the non-scanner fields on disk.
        let mut seeded = MetaStore::default();
        seeded.set_era("Gamma");
        seeded.set_enabled_extensions(vec!["ts".to_string(), "rs".to_string()]);
        seeded.ensure_file_id("src/worker.ts");
        let fp = content_fingerprint("fn main() {}");
        seeded.set_dossier("src/worker.ts", "Orchestrates RNA-seq.", fp.clone());
        let mut merges = BTreeMap::new();
        merges.insert("web_rnaseq".to_string(), "rnaseq".to_string());
        seeded.set_feature_merges(merges);
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "rnaseq".to_string(),
            FeatureLabelOverride {
                label: "RNA-seq".into(),
                description: "pipeline".into(),
            },
        );
        seeded.set_feature_label_overrides(overrides);
        seeded.save(&dir.path).unwrap();

        // Scanner's in-memory store: only scanner-owned fields, NO dossier/era/exts.
        let mut scanner_mem = MetaStore::default();
        scanner_mem.ensure_file_id("src/worker.ts");
        scanner_mem.set_coords("src/worker.ts", Coords::new(7.0, 8.0));
        scanner_mem.set_feature("src/worker.ts", "rnaseq", "directory", "rnaseq");

        // Scanner terminal save — fresh reload + apply scanner-owned fields only.
        MetaStore::with_write_lock(&dir.path, |disk| scanner_mem.apply_scanner_owned_onto(disk))
            .unwrap();

        let reloaded = MetaStore::load(&dir.path);
        // Scanner-owned fields applied.
        assert_eq!(
            reloaded.coords("src/worker.ts"),
            Some(Coords::new(7.0, 8.0))
        );
        assert_eq!(
            reloaded.feature("src/worker.ts"),
            Some(("rnaseq".into(), "directory".into(), "rnaseq".into()))
        );
        // Non-scanner fields PRESERVED.
        assert_eq!(reloaded.era(), "Gamma", "era preserved across scanner save");
        assert_eq!(
            reloaded.enabled_extensions().cloned(),
            Some(vec!["ts".to_string(), "rs".to_string()]),
            "extensions preserved across scanner save"
        );
        let d = reloaded
            .dossier("src/worker.ts")
            .expect("dossier preserved");
        assert_eq!(d.text, "Orchestrates RNA-seq.");
        assert_eq!(d.fingerprint, fp);
        assert_eq!(
            reloaded
                .feature_merges()
                .get("web_rnaseq")
                .map(String::as_str),
            Some("rnaseq"),
            "feature_merges preserved across scanner save"
        );
        assert_eq!(
            reloaded
                .feature_label_overrides()
                .get("rnaseq")
                .map(|o| o.label.as_str()),
            Some("RNA-seq"),
            "feature_label_overrides preserved across scanner save"
        );
    }

    // The scanner save honors deletions: a file the scanner no longer carries is
    // pruned from disk (not resurrected), and its disk dossier goes with it.
    #[test]
    fn scanner_save_prunes_deleted_files() {
        let dir = TempDir::new("wwl_prune");

        let mut seeded = MetaStore::default();
        seeded.ensure_file_id("src/deleted.ts");
        seeded.set_dossier("src/deleted.ts", "old", content_fingerprint("x"));
        seeded.ensure_file_id("src/kept.ts");
        seeded.save(&dir.path).unwrap();

        // Scanner kept only `src/kept.ts`.
        let mut scanner_mem = MetaStore::default();
        scanner_mem.ensure_file_id("src/kept.ts");
        scanner_mem.set_coords("src/kept.ts", Coords::new(1.0, 1.0));

        MetaStore::with_write_lock(&dir.path, |disk| scanner_mem.apply_scanner_owned_onto(disk))
            .unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert!(
            reloaded.file_id("src/deleted.ts").is_none(),
            "deleted file pruned"
        );
        assert!(reloaded.dossier("src/deleted.ts").is_none());
        assert!(reloaded.file_id("src/kept.ts").is_some());
    }

    // (c) The reclassify rollback (restore overrides+merges via with_write_lock)
    // preserves a dossier written concurrently between the original save and the
    // rollback — the rollback reloads fresh disk and touches ONLY overrides+merges.
    #[test]
    fn reclassify_rollback_preserves_concurrently_written_dossier() {
        let dir = TempDir::new("wwl_rollback");

        // Pre-call state: empty overrides/merges captured by the command.
        let old_overrides: BTreeMap<String, FeatureLabelOverride> = BTreeMap::new();
        let old_merges: BTreeMap<String, String> = BTreeMap::new();

        // Reclassify persisted new overrides/merges.
        MetaStore::with_write_lock(&dir.path, |m| {
            let mut ov = BTreeMap::new();
            ov.insert(
                "rnaseq".to_string(),
                FeatureLabelOverride {
                    label: "RNA-seq".into(),
                    description: String::new(),
                },
            );
            m.set_feature_label_overrides(ov);
            let mut mg = BTreeMap::new();
            mg.insert("web_rnaseq".to_string(), "rnaseq".to_string());
            m.set_feature_merges(mg);
        })
        .unwrap();

        // CONCURRENT writer (e.g. polis_generate_dossier) lands a dossier on disk
        // between the reclassify save and the rollback.
        let fp = content_fingerprint("fn main() {}");
        MetaStore::with_write_lock(&dir.path, |m| {
            m.ensure_file_id("src/a.ts");
            m.set_dossier("src/a.ts", "Mid-flight dossier.", fp.clone());
        })
        .unwrap();

        // Scan failed -> ROLL BACK overrides+merges to the captured pre-call state.
        MetaStore::with_write_lock(&dir.path, |m| {
            m.set_feature_label_overrides(old_overrides.clone());
            m.set_feature_merges(old_merges.clone());
        })
        .unwrap();

        let reloaded = MetaStore::load(&dir.path);
        // Rollback restored the empty overrides/merges.
        assert!(
            reloaded.feature_label_overrides().is_empty(),
            "overrides rolled back"
        );
        assert!(reloaded.feature_merges().is_empty(), "merges rolled back");
        // The concurrently-written dossier SURVIVED the rollback.
        let d = reloaded
            .dossier("src/a.ts")
            .expect("dossier written between save and rollback must survive");
        assert_eq!(d.text, "Mid-flight dossier.");
        assert_eq!(d.fingerprint, fp);
    }

    #[test]
    fn enabled_extensions_default_to_none_and_round_trip() {
        let dir = TempDir::new("exts");
        let store = MetaStore::load(&dir.path);
        assert!(
            store.enabled_extensions().is_none(),
            "a fresh store has no override (scanner uses its default set)"
        );

        let mut store = store;
        store.set_enabled_extensions(vec!["rs".to_string(), "ts".to_string()]);
        store.save(&dir.path).unwrap();

        let reloaded = MetaStore::load(&dir.path);
        assert_eq!(
            reloaded.enabled_extensions().cloned(),
            Some(vec!["rs".to_string(), "ts".to_string()]),
            "the per-workspace override must survive the round-trip"
        );
    }
}
