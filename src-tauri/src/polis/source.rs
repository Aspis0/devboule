//! P7 — CityDataSource trait boundary (D2 forward-compat seam).
//!
//! Defines the ordered-source-fold pattern for CityState assembly.  The pure
//! scanner produces scanner truth; sources patch the city in a fixed order.
//! Each failure is captured (fail-open per source) — it never aborts the fold.
//!
//! ## D2 typed-surface evolution (intended, not yet built)
//!
//! The D2 design doc sketches a richer surface: `entities()`, `relationships()`,
//! `health_signals()`, `freshness()`.  Today's attach functions are imperative
//! mutations of `CityState`, so the trait provides a single `apply` method.
//! When a remote source (entire.io, an Oracle streaming endpoint) exists, the
//! typed-patch surface should be introduced — entity patches that *decorate*
//! scanner-discovered files (never invent buildings, per the data-purity
//! contract in `model.rs`), typed relation edges with weight+provenance,
//! health signals with deterministic attestation, and epoch-based staleness.
//!
//! ## Conflict rule (encoded in `fold_sources`)
//!
//! Scanner truth < source patches.  Sources are applied in order; a later
//! source never overrides an earlier one on the same field (the existing
//! attach functions already respect this by clearing stale markers before
//! re-attaching).  Each source failure is recorded in `city.scan_note` and
//! the fold continues — a single broken source never strips the city.

use crate::backend::model::AgentLiveState;
use crate::polis::model::{CityState, ExternalService};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// ScanContext — one-shot borrowed context for all sources in a fold
// ---------------------------------------------------------------------------

/// Immutable context bundle passed to every [`CityDataSource::apply`] call
/// during a single `fold_sources` invocation.  Built once in the command layer
/// before the fold begins.
pub struct ScanContext<'a> {
    /// The project root being scanned.
    pub project_root: &'a Path,
    /// Live agent state, if telemetry was readable.
    pub live: Option<&'a AgentLiveState>,
    /// Real project-id → root-path map for agent file resolution.
    pub project_roots: &'a BTreeMap<String, std::path::PathBuf>,
    /// Open bug-card suspects: (card_id, [suspect_file_rel_paths]).
    pub open_bug_suspects: &'a [(String, Vec<String>)],
    /// Era monuments carried over from the previous in-memory city.
    /// The pure scanner starts with an empty `external_services`; this slice is
    /// re-attached so scan/watch never wipe cumulative era monuments.
    pub preserved_monuments: &'a [ExternalService],
}

// ---------------------------------------------------------------------------
// Result alias
// ---------------------------------------------------------------------------

pub type SourceResult<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// CityDataSource trait
// ---------------------------------------------------------------------------

/// A source that contributes to CityState assembly.
///
/// Implementors apply their domain-specific augmentation to the city.  The
/// pure scanner output is the base truth; sources decorate it.  The trait is
/// `Send + Sync` so implementations can be stored in shared registries.
///
/// See the module-level docs for the D2 typed-surface evolution plan.
pub trait CityDataSource: Send + Sync {
    /// Stable identifier for this source (e.g. `"agents"`, `"suspect-cards"`,
    /// `"monuments"`).  Used in diagnostic notes.
    fn id(&self) -> &'static str;

    /// Apply this source's augmentation to `city`.  Called once per fold.
    fn apply(&self, ctx: &ScanContext, city: &mut CityState) -> SourceResult<()>;
}

// ---------------------------------------------------------------------------
// fold_sources — ordered, fail-open application
// ---------------------------------------------------------------------------

/// Apply `sources` in order, accumulating each failure into `city.scan_note`
/// and NEVER aborting the fold.  The scanner produces `city` before this call;
/// sources decorate it.
pub fn fold_sources(sources: &[Box<dyn CityDataSource>], ctx: &ScanContext, city: &mut CityState) {
    for source in sources {
        match source.apply(ctx, city) {
            Ok(()) => {}
            Err(e) => {
                let note = format!("source {}: {e}", source.id());
                match &mut city.scan_note {
                    Some(existing) => {
                        existing.push_str("; ");
                        existing.push_str(&note);
                    }
                    None => {
                        city.scan_note = Some(note);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete source implementors (bodies delegate to existing attach fns)
// ---------------------------------------------------------------------------

/// Folds real MCP agents into the city as players.
///
/// Sourced ONLY from the real agent live state.  The pure scanner core is
/// agent-free; this source adds agents that exist in `.aspis-agents.json`.
/// A missing/unreadable live state leaves the city honestly agent-less.
pub struct AgentsSource;

impl CityDataSource for AgentsSource {
    fn id(&self) -> &'static str {
        "agents"
    }

    fn apply(&self, ctx: &ScanContext, city: &mut CityState) -> SourceResult<()> {
        let live = ctx
            .live
            .ok_or_else(|| "agent telemetry read failed".to_string())?;
        crate::polis::scanner::attach_agents(city, live, ctx.project_root, ctx.project_roots);
        Ok(())
    }
}

/// Marks buildings that are suspected by open bug cards.
///
/// The investigative-smoke overlay (`Building::suspect_of_card_id`) is sourced
/// from live project files.  Fail-open: an empty suspect list simply clears
/// stale markers, which is safe.
pub struct SuspectCardsSource;

impl CityDataSource for SuspectCardsSource {
    fn id(&self) -> &'static str {
        "suspect-cards"
    }

    fn apply(&self, ctx: &ScanContext, city: &mut CityState) -> SourceResult<()> {
        crate::polis::scanner::attach_suspect_cards(city, ctx.open_bug_suspects);
        Ok(())
    }
}

/// Re-attaches era monuments (`provider == "monument"`) preserved from the
/// previous in-memory city.  After cloud-provider inventory removal,
/// `external_services` holds **only** these monument entries.
pub struct MonumentsSource;

impl CityDataSource for MonumentsSource {
    fn id(&self) -> &'static str {
        "monuments"
    }

    fn apply(&self, ctx: &ScanContext, city: &mut CityState) -> SourceResult<()> {
        // Drop any non-monument residue (legacy cloud outposts from old saves),
        // then restore cumulative era monuments from the previous city.
        city.external_services
            .retain(|s| s.provider == "monument");
        for m in ctx.preserved_monuments {
            if m.provider != "monument" {
                continue;
            }
            if city
                .external_services
                .iter()
                .any(|s| s.service_id == m.service_id)
            {
                continue;
            }
            city.external_services.push(m.clone());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------

/// The ordered, canonical source vector shared by both the command-layer scan
/// (`commands::scan_and_store`) and the file-watcher re-scan
/// (`watcher::rescan_and_emit`).  **Every new data source MUST be added here**
/// so the two call sites can never diverge.
pub fn default_sources() -> Vec<Box<dyn CityDataSource>> {
    vec![
        Box::new(AgentsSource),
        Box::new(SuspectCardsSource),
        Box::new(MonumentsSource),
    ]
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polis::model::*;

    /// A test source that stamps a marker into the city's notes.
    struct MarkerSource {
        name: &'static str,
        marker: &'static str,
        fail: bool,
    }

    impl CityDataSource for MarkerSource {
        fn id(&self) -> &'static str {
            self.name
        }

        fn apply(&self, _ctx: &ScanContext, city: &mut CityState) -> SourceResult<()> {
            if self.fail {
                return Err("injected failure".to_string());
            }
            city.notes.push(self.marker.to_string());
            Ok(())
        }
    }

    fn empty_city() -> CityState {
        CityState::empty("test", "Alpha")
    }

    fn empty_ctx() -> ScanContext<'static> {
        // Safe: we only use the ctx fields that test sources actually read,
        // and our MarkerSource reads none of them.
        let dummy_path: &'static Path = Path::new("/dummy");
        let empty_roots: &'static BTreeMap<String, std::path::PathBuf> =
            Box::leak(Box::new(BTreeMap::new()));
        let empty_suspects: &'static [(String, Vec<String>)] = &[];
        let empty_monuments: &'static [ExternalService] = &[];
        ScanContext {
            project_root: dummy_path,
            live: None,
            project_roots: empty_roots,
            open_bug_suspects: empty_suspects,
            preserved_monuments: empty_monuments,
        }
    }

    #[test]
    fn fold_applies_sources_in_order() {
        let mut city = empty_city();
        let ctx = empty_ctx();
        let sources: Vec<Box<dyn CityDataSource>> = vec![
            Box::new(MarkerSource {
                name: "a",
                marker: "A",
                fail: false,
            }),
            Box::new(MarkerSource {
                name: "b",
                marker: "B",
                fail: false,
            }),
            Box::new(MarkerSource {
                name: "c",
                marker: "C",
                fail: false,
            }),
        ];
        fold_sources(&sources, &ctx, &mut city);
        assert_eq!(city.notes, vec!["A", "B", "C"]);
    }

    #[test]
    fn failing_source_does_not_abort_fold() {
        let mut city = empty_city();
        let ctx = empty_ctx();
        let sources: Vec<Box<dyn CityDataSource>> = vec![
            Box::new(MarkerSource {
                name: "ok",
                marker: "before",
                fail: false,
            }),
            Box::new(MarkerSource {
                name: "bad",
                marker: "X",
                fail: true,
            }),
            Box::new(MarkerSource {
                name: "ok2",
                marker: "after",
                fail: false,
            }),
        ];
        fold_sources(&sources, &ctx, &mut city);
        // "before" from first source, "after" from third (second failed).
        assert_eq!(city.notes, vec!["before", "after"]);
        // Failure recorded in scan_note.
        assert!(city
            .scan_note
            .as_deref()
            .unwrap()
            .contains("source bad: injected failure"));
    }

    #[test]
    fn multiple_failures_all_recorded_in_scan_note() {
        let mut city = empty_city();
        let ctx = empty_ctx();
        let sources: Vec<Box<dyn CityDataSource>> = vec![
            Box::new(MarkerSource {
                name: "f1",
                marker: "-",
                fail: true,
            }),
            Box::new(MarkerSource {
                name: "f2",
                marker: "-",
                fail: true,
            }),
            Box::new(MarkerSource {
                name: "ok",
                marker: "survivor",
                fail: false,
            }),
        ];
        fold_sources(&sources, &ctx, &mut city);
        assert_eq!(city.notes, vec!["survivor"]);
        let note = city.scan_note.as_deref().unwrap();
        assert!(note.contains("source f1: injected failure"));
        assert!(note.contains("source f2: injected failure"));
    }

    #[test]
    fn empty_sources_leaves_city_unchanged() {
        let mut city = empty_city();
        city.notes.push("original".to_string());
        let ctx = empty_ctx();
        let sources: Vec<Box<dyn CityDataSource>> = vec![];
        fold_sources(&sources, &ctx, &mut city);
        assert_eq!(city.notes, vec!["original"]);
        assert!(city.scan_note.is_none());
    }

    #[test]
    fn source_ids_are_stable_and_distinct() {
        assert_eq!(AgentsSource.id(), "agents");
        assert_eq!(SuspectCardsSource.id(), "suspect-cards");
        assert_eq!(MonumentsSource.id(), "monuments");
    }

    #[test]
    fn monuments_source_restores_era_monuments_only() {
        let monument = ExternalService {
            service_id: "monument-alpha".into(),
            provider: "monument".into(),
            service_type: "parthenon".into(),
            name: "Era Alpha".into(),
            status: "active".into(),
            coords: Coords { x: -8.0, y: 0.0 },
            spawnable: false,
        };
        let legacy_cloud = ExternalService {
            service_id: "cf-worker-1".into(),
            provider: "cloudflare".into(),
            service_type: "worker".into(),
            name: "legacy".into(),
            status: "running".into(),
            coords: Coords { x: 10.0, y: 0.0 },
            spawnable: false,
        };
        let preserved = vec![monument.clone(), legacy_cloud];
        let mut city = empty_city();
        let dummy_path = Path::new("/dummy");
        let roots = BTreeMap::new();
        let suspects: [(String, Vec<String>); 0] = [];
        let ctx = ScanContext {
            project_root: dummy_path,
            live: None,
            project_roots: &roots,
            open_bug_suspects: &suspects,
            preserved_monuments: &preserved,
        };
        MonumentsSource.apply(&ctx, &mut city).unwrap();
        assert_eq!(city.external_services.len(), 1);
        assert_eq!(city.external_services[0].service_id, "monument-alpha");
        assert_eq!(city.external_services[0].provider, "monument");
    }

    /// Identity test: running default_sources() through fold_sources produces
    /// the SAME result as calling the individual attach functions directly,
    /// and scan_note stays None on success.
    #[test]
    fn default_sources_is_equivalent_to_manual_attach() {
        use crate::backend::model::{AgentLiveState, AgentSession};
        use crate::polis::model::Coords;

        // Build a minimal city with two buildings.
        let mut city_a = CityState::empty("test-proj", "Alpha");
        city_a.buildings = vec![
            Building {
                file_id: "f1".into(),
                file_path: "src/lib.rs".into(),
                district_id: "commons".into(),
                purpose: "library".into(),
                purpose_source: "extension".into(),
                feature_id: String::new(),
                feature_source: String::new(),
                provider: None,
                lines_of_code: 42,
                visual_tier: "kalybe".into(),
                coords: Coords { x: 0.0, y: 0.0 },
                status: "normal".into(),
                label: "lib".into(),
                description: String::new(),
                notes: vec![],
                sins: vec![],
                agent_present: None,
                suspect_of_card_id: None,
                kanban_card_id: None,
                untracked_change: None,
                last_modified: String::new(),
            },
            Building {
                file_id: "f2".into(),
                file_path: "src/main.rs".into(),
                district_id: "commons".into(),
                purpose: "workshop".into(),
                purpose_source: "extension".into(),
                feature_id: String::new(),
                feature_source: String::new(),
                provider: None,
                lines_of_code: 99,
                visual_tier: "oikia".into(),
                coords: Coords { x: 1.0, y: 0.0 },
                status: "normal".into(),
                label: "main".into(),
                description: String::new(),
                notes: vec![],
                sins: vec![],
                agent_present: None,
                suspect_of_card_id: None,
                kanban_card_id: None,
                untracked_change: None,
                last_modified: String::new(),
            },
        ];

        // Synthetic agent live state: one agent working on f1.
        let now = chrono::Utc::now().to_rfc3339();
        let live = AgentLiveState {
            version: 1,
            updated_at: now.clone(),
            sessions: vec![AgentSession {
                agent_id: "test-agent".into(),
                role: "coder".into(),
                model: None,
                status: "working".into(),
                message: None,
                client: None,
                current_project_id: None,
                current_task_id: None,
                current_file_path: Some("src/lib.rs".into()),
                first_seen_at: Some(now.clone()),
                last_seen_at: Some(now.clone()),
                launch_token_hash: None,
                launch_token_issued_at: None,
                session_token_hash: None,
                session_token_issued_at: None,
                launch_consumed_at: None,
                subagents: vec![],
                needs_user: None,
                host: None,
                parent_agent_id: None,
                pending_question: None,
                user_reply: None,
            }],
            claims: vec![],
            events: vec![],
            rules: vec![],
            state_path: "/fake/.aspis-agents.json".into(),
            mcp_command: String::new(),
            mcp_client_config: String::new(),
            mini_coder_directives: vec![],
            visual_check_directives: vec![],
            design_request_directives: vec![],
            git_push_requests: vec![],
            plan_approval_requests: vec![],
            consent_requests: vec![],
        };

        // One suspect pair.
        let suspects: Vec<(String, Vec<String>)> =
            vec![("CARD-1".into(), vec!["src/main.rs".into()])];

        let project_root = std::path::Path::new("/tmp/test-proj");
        let project_roots = std::collections::BTreeMap::new();
        let preserved_monuments: Vec<ExternalService> = vec![];

        let ctx = ScanContext {
            project_root,
            live: Some(&live),
            project_roots: &project_roots,
            open_bug_suspects: &suspects,
            preserved_monuments: &preserved_monuments,
        };

        // Variant A: fold_sources with default_sources().
        let sources = default_sources();
        fold_sources(&sources, &ctx, &mut city_a);

        // Variant B: manual attach, identical inputs.
        let mut city_b = city_a.clone();
        // Reset city_b to pre-attach state (the same base city_a was before fold).
        city_b.agents.clear();
        city_b.external_services.clear();
        for b in city_b.buildings.iter_mut() {
            b.agent_present = None;
            b.suspect_of_card_id = None;
        }
        crate::polis::scanner::attach_agents(&mut city_b, &live, project_root, &project_roots);
        crate::polis::scanner::attach_suspect_cards(&mut city_b, &suspects);
        // Monuments: none preserved → empty external_services.
        city_b.external_services.retain(|s| s.provider == "monument");

        // Compare agent lists: same agents.
        assert_eq!(city_a.agents.len(), city_b.agents.len(), "agent count");
        for (a, b) in city_a.agents.iter().zip(city_b.agents.iter()) {
            assert_eq!(a.agent_id, b.agent_id);
            assert_eq!(a.current_file_id, b.current_file_id);
        }

        // Compare suspect_of_card_id on each building.
        for (ba, bb) in city_a.buildings.iter().zip(city_b.buildings.iter()) {
            assert_eq!(
                ba.suspect_of_card_id, bb.suspect_of_card_id,
                "suspect for {}",
                ba.file_path
            );
        }

        // Compare external_services (monuments only).
        assert_eq!(city_a.external_services.len(), city_b.external_services.len());

        // scan_note stays None on success (no source failure).
        assert!(
            city_a.scan_note.is_none(),
            "scan_note should be None, got {:?}",
            city_a.scan_note
        );
    }
}
