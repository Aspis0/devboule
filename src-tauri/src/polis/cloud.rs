//! Polis Map — external cloud services (Section 5: "the city meets the cloud").
//!
//! DATA-PURITY CONTRACT (see `model::ExternalService`): the scaleway/cloudflare
//! services placed on the map mirror the REAL cached provider inventory only
//! (`backend::providers::ProviderInventory`, synced from the live
//! Scaleway/Cloudflare APIs). This module is PURE and OFFLINE: it reads an
//! already-fetched `&[ProviderInventory]` snapshot the app holds in
//! `BackendState` and maps each relevant resource to an `ExternalService`. It
//! NEVER performs a network call, NEVER blocks, and NEVER fabricates a resource —
//! an empty / unavailable inventory yields an empty service list.
//!
//! The command layer (`commands::scan_and_store`) reads the in-memory snapshot
//! via `BackendState::cached_provider_inventories()` and folds the services into
//! the city AFTER the pure scan, mirroring how `scanner::attach_agents` folds in
//! the real agent live-state. The pure scanner core still emits an empty service
//! list (an empty city is honest).
//!
//! PLACEMENT: services live OUTSIDE the building grid, in a deterministic column
//! along the seaward (east) margin — a "harbour where the city meets the cloud".
//! Coords are computed from the laid-out building extent so they never overlap a
//! building or each other, and the order is fully deterministic (sorted by
//! provider, then name, then id — no `HashMap`/RNG order leaks).
//!
//! NO-SECRET: only safe display fields are copied (id, name, kind, normalized
//! state). Endpoints, IPs, tokens, DSNs and any credential-bearing field are
//! NEVER read into an `ExternalService` (they are not even referenced here).

use crate::backend::providers::ProviderInventory;
use crate::polis::model::{Coords, ExternalService};
use crate::polis::scanner::{map_extent, GAP};

/// Vocabulary of the `ExternalService::service_type` slugs this module emits.
/// Matches the doc set on `model::ExternalService` ("container" | "gpu_vm" |
/// "cpu_vm" | "object_store" | "llm_api" | "worker"). Kept as a module of
/// `&str` consts (like the other Polis vocabularies) so callers/tests share one
/// source of truth.
pub mod service_type {
    pub const CONTAINER: &str = "container";
    pub const GPU_VM: &str = "gpu_vm";
    pub const CPU_VM: &str = "cpu_vm";
    pub const OBJECT_STORE: &str = "object_store";
    pub const LLM_API: &str = "llm_api";
    pub const WORKER: &str = "worker";
}

/// Provider slugs (mirror `model::provider`).
pub mod provider {
    pub const SCALEWAY: &str = "scaleway";
    pub const CLOUDFLARE: &str = "cloudflare";
}

/// Vertical spacing (tiles) between adjacent harbour nodes in the seaward column.
/// `> 1` so the cloud outposts read as separated structures, never overlapping.
/// PUBLIC so the terrain frame (`terrain::build_terrain`) can extend the sea band
/// to cover the full harbour column for `n` services (the harbours must sit ON the
/// sea, not below it on grass) — both modules MUST agree on this pitch.
pub const ROW_PITCH: f64 = 2.0;

/// The y of the LAST (lowest) harbour node when `n` services are placed in a
/// column anchored at `top_y` (the land's `min_y`). With `n == 0` there is no
/// harbour, so this returns `top_y` (a degenerate single row). Pure; mirrors the
/// `top_y + i*ROW_PITCH` stepping in `place_external_services`.
pub fn harbour_bottom_y(n_services: usize, top_y: f64) -> f64 {
    let last = n_services.saturating_sub(1) as f64;
    top_y + last * ROW_PITCH
}

/// Normalized `ExternalService::status` vocabulary ("running" | "stopped" |
/// "spawning" | "error"). The renderer keys its status indicator on these.
pub mod status {
    pub const RUNNING: &str = "running";
    pub const STOPPED: &str = "stopped";
    pub const SPAWNING: &str = "spawning";
    pub const ERROR: &str = "error";
}

/// Map a Scaleway `ScalewayResourceSummary::resource_type` (compute) to a Polis
/// `service_type` slug. The provider-layer vocabulary is "GPU" / "CPU VM" /
/// "Serverless" / "Serverless SQL" / "Generative API Model" (see
/// `backend::providers`). We pick the closest Polis kind; an unrecognized
/// compute kind falls back to `container` (the generic serverless/compute motif)
/// rather than being dropped, so a new Scaleway resource type still appears.
fn scaleway_compute_type(resource_type: &str) -> &'static str {
    let lowered = resource_type.trim().to_ascii_lowercase();
    if lowered.contains("gpu") {
        service_type::GPU_VM
    } else if lowered.contains("cpu") || lowered.contains("vm") || lowered.contains("instance") {
        service_type::CPU_VM
    } else if lowered.contains("generative")
        || lowered.contains("api model")
        || lowered.contains("llm")
    {
        service_type::LLM_API
    } else {
        // "Serverless", "Serverless SQL", or any future kind: the container motif.
        service_type::CONTAINER
    }
}

/// Normalize a provider-layer resource `state` (already lowercased by
/// `backend::providers::normalize_scaleway_state`: "running" | "available" |
/// "stopped" | "provisioning" | "error" | "unknown") and the Cloudflare worker
/// status ("healthy" | "degraded" | "unknown") to the Polis service `status`
/// vocabulary. Unknown/odd values map to `stopped` (a dim, non-alarming dot) so a
/// surprising state never renders as a false "running" (lit) or false "error"
/// (red alarm). Pure, never panics.
fn normalize_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        // Live, serving traffic.
        "running" | "ready" | "healthy" | "available" | "deployed" => status::RUNNING,
        // Mid-transition.
        "provisioning" | "spawning" | "starting" | "booting" | "creating" | "pending" => {
            status::SPAWNING
        }
        // Hard failure / degraded.
        "error" | "failed" | "locked" | "degraded" => status::ERROR,
        // Idle/off/unknown -> a dim, honest "stopped" dot (never a false lit/alarm).
        _ => status::STOPPED,
    }
}

/// PURE mapping: turn the cached provider inventories into Polis `ExternalService`
/// records (UNPLACED — every `coords` is the origin; `place_external_services`
/// assigns real margin coords). Deterministically ordered by (provider, name,
/// service_id). `spawnable = false` for every service in this phase (inspect-only;
/// no spawn/stop action wired yet). NEVER fabricates: an empty/missing inventory
/// yields an empty list.
///
/// Only safe display fields are read (id, name, kind, state). No endpoint / IP /
/// DSN / token is ever copied — those fields are not referenced.
pub fn map_inventory_to_services(inventories: &[ProviderInventory]) -> Vec<ExternalService> {
    use crate::backend::model::ProviderId;

    let mut services: Vec<ExternalService> = Vec::new();

    for inv in inventories {
        match inv.health.id {
            ProviderId::Cloudflare => {
                for w in &inv.workers {
                    services.push(ExternalService {
                        // Provider-prefixed so a Scaleway and a Cloudflare id can
                        // never collide, and the id is stable across scans.
                        service_id: format!("cf-worker-{}", w.id),
                        provider: provider::CLOUDFLARE.to_string(),
                        service_type: service_type::WORKER.to_string(),
                        name: w.name.clone(),
                        status: normalize_status(&w.status).to_string(),
                        coords: Coords::new(0.0, 0.0), // placed later
                        spawnable: false,
                    });
                }
            }
            ProviderId::Scaleway => {
                for r in &inv.compute {
                    services.push(ExternalService {
                        // Distinct prefix per namespace (compute vs storage) so two
                        // resources can never collide even if Scaleway ever reused
                        // a UUID across APIs; also makes the id self-describing.
                        service_id: format!("scw-compute-{}", r.id),
                        provider: provider::SCALEWAY.to_string(),
                        service_type: scaleway_compute_type(&r.resource_type).to_string(),
                        name: r.name.clone(),
                        status: normalize_status(&r.state).to_string(),
                        coords: Coords::new(0.0, 0.0),
                        spawnable: false,
                    });
                }
                for s in &inv.storage {
                    services.push(ExternalService {
                        service_id: format!("scw-storage-{}", s.id),
                        provider: provider::SCALEWAY.to_string(),
                        service_type: service_type::OBJECT_STORE.to_string(),
                        name: s.name.clone(),
                        status: normalize_status(&s.state).to_string(),
                        coords: Coords::new(0.0, 0.0),
                        spawnable: false,
                    });
                }
            }
        }
    }

    // DETERMINISTIC order (provider, name, service_id) — no HashMap/RNG leak.
    services.sort_by(|a, b| {
        a.provider
            .cmp(&b.provider)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.service_id.cmp(&b.service_id))
    });
    services
}

/// PURE placement: assign each service a deterministic margin coord OUTSIDE the
/// building grid — a column along the seaward (east, +x) edge, stepping down in
/// `y`. Coords are derived from the laid-out building extent (`map_extent`) so
/// they never overlap a building (the column sits `GAP` tiles beyond the
/// buildings' max-x) nor each other (each service gets its own `y` row spaced by
/// `ROW_PITCH`). Order follows the already-sorted `services` vector, so placement
/// is fully deterministic. With no buildings the harbour anchors at a fixed
/// offset from the origin.
///
/// Mutates `services` in place (sets `coords`).
pub fn place_external_services(
    services: &mut [ExternalService],
    buildings: &[crate::polis::model::Building],
) {
    if services.is_empty() {
        return;
    }

    // Horizontal gap (tiles) between the city's east edge and the harbour column.
    const SEA_GAP: f64 = GAP as f64;

    // Anchor the harbour off the real building extent so it always sits OUTSIDE
    // the grid. With no buildings, anchor at a small fixed offset from origin.
    let (col_x, top_y) = match map_extent(buildings) {
        Some((_min_x, min_y, max_x, _max_y)) => (max_x + SEA_GAP, min_y),
        None => (SEA_GAP, 0.0),
    };

    for (i, svc) in services.iter_mut().enumerate() {
        svc.coords = Coords::new(col_x, top_y + (i as f64) * ROW_PITCH);
    }
}

/// Populate `city.external_services` from the cached provider inventory snapshot.
/// PURE + OFFLINE: maps the already-fetched `inventories` to services, places them
/// at the seaward margin (using the city's own laid-out building extent), and
/// REPLACES the city's service list — EXCEPT it preserves any non-inventory
/// "monument" services (era prestige arches, derived from real archived stats; see
/// `commands::reset_city_in_place`), which are legitimately not in the inventory.
///
/// Empty / missing inventory -> the inventory-backed services are simply empty
/// (monuments, if any, are kept). Never fabricates a cloud resource. Never blocks.
///
/// REBUILDS the terrain frame with the real harbour count: the seaward harbour
/// column steps DOWN from the land's `min_y`, so with enough services the lowest
/// harbour lands below the land's `max_y`. The pure scanner built the terrain with
/// `0` harbours (it has no inventory); here we re-derive it so the sea band covers
/// the FULL harbour column and every harbour sits on water (FIX 3). Deterministic +
/// pure (same buildings/roads + count -> same terrain). Monuments sit on the WEST
/// edge and never enter the seaward column, so they don't count toward the harbours.
/// The stable prefix of the walkability-violation note (the part that doesn't
/// vary with the offending tile), used to detect a note already present.
const NAV_NOTE_PREFIX: &str = "Polis terrain: a routed road is not fully walkable";

/// Compose the city's `scan_note` when a walkability violation is surfaced.
///
/// FIX 5: the walkability note is its OWN, clearly-delimited entry — never
/// concatenated into the unrelated file-scan-cap note as a run-on message.
/// FIX 2: `attach_external_services` runs on EVERY attach, so this MUST be
/// idempotent — a re-attach with the same (still-failing) terrain must not stack
/// duplicate copies of the note. It preserves any non-walkability prefix (e.g. the
/// file-scan-cap note) and replaces only the trailing walkability segment.
fn compose_scan_note(prev: Option<String>, note: String) -> String {
    match prev {
        // A walkability note is already present (from this or a prior attach):
        // keep any leading non-walkability prefix and refresh the nav segment, so
        // re-attaching never duplicates it.
        Some(prev) if prev.contains(NAV_NOTE_PREFIX) => match prev.split_once(NAV_NOTE_PREFIX) {
            Some((head, _)) if !head.is_empty() => format!("{head}{note}"),
            _ => note,
        },
        // A different (e.g. file-scan-cap) note exists: keep BOTH, delimited.
        Some(prev) => format!("{prev} | {note}"),
        None => note,
    }
}

pub fn attach_external_services(
    city: &mut crate::polis::model::CityState,
    inventories: &[ProviderInventory],
) {
    // Keep era monuments (the one legitimate non-inventory entry); drop any prior
    // inventory-backed services so a re-attach reflects the CURRENT snapshot
    // (resources that vanished from the inventory disappear from the map).
    city.external_services.retain(|s| s.provider == "monument");

    let mut services = map_inventory_to_services(inventories);
    place_external_services(&mut services, &city.buildings);

    // Extend the sea band to cover the harbour column (only the seaward
    // inventory-backed nodes anchor in it; monuments are on the west edge).
    city.terrain =
        crate::polis::terrain::build_terrain(&city.buildings, &city.roads, services.len());

    city.external_services.extend(services);

    // FIX 2 — LOAD-BEARING GUARANTEE on the FINAL terrain ("citizens walk only on
    // roads/bridges, never on water or a footprint"). The pure scanner built its
    // terrain with 0 harbours; the REBUILD just above extends the sea band to the
    // real harbour count, and THIS `city.terrain` is the one the CityState carries
    // and the frontend renders/guards (`makeWaterBlocker`). So the routed
    // `Road.path` polylines must be entirely walkable against THIS terrain — the
    // same bridges/sea the frontend sees — not against the scan-time 0-harbour map.
    // Roads don't enter the harbour extension band, so this holds by construction,
    // but the check must validate the real map. Cheap: O(total road tile length).
    //   - dev/test: `debug_assert!` so the suite + dev builds fail loudly;
    //   - release: surface an honest, DISTINCT `scan_note` (FIX 5 — its own entry,
    //     never concatenated onto the unrelated file-scan-cap note) and NEVER panic
    //     (a cosmetic terrain edge must not crash the user's app).
    if let Err(why) =
        crate::polis::nav::road_paths_all_walkable(&city.buildings, &city.roads, &city.terrain)
    {
        let note = format!("Polis terrain: a routed road is not fully walkable ({why})");
        debug_assert!(false, "{note}");
        eprintln!("polis cloud: {note}");
        // Surface as its OWN distinct note, never buried inside the unrelated
        // file-scan-cap note (FIX 5), and idempotent on re-attach (FIX 2 runs every
        // attach). See `compose_scan_note`.
        city.scan_note = Some(compose_scan_note(city.scan_note.take(), note));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::{
        CloudflareWorkerSummary, ProviderId, ScalewayResourceSummary, ScalewayStorageSummary,
    };
    use crate::polis::model::{
        building_status, purpose, purpose_source, road_style, road_type, visual_tier, Building,
        CityState, Coords, Road,
    };

    fn cf_inventory_with_worker(id: &str, name: &str, status: &str) -> ProviderInventory {
        let mut inv = ProviderInventory::missing(ProviderId::Cloudflare);
        inv.workers.push(CloudflareWorkerSummary {
            id: id.into(),
            account_id: "acct".into(),
            account_name: None,
            name: name.into(),
            status: status.into(),
            purpose: "test".into(),
            purpose_source: "test".into(),
            routes: Vec::new(),
            last_deploy: None,
            usage_model: None,
            compatibility_date: None,
            compatibility_flags: Vec::new(),
            handlers: Vec::new(),
            tags: Vec::new(),
            oracle_query: "q".into(),
        });
        inv
    }

    fn scw_compute(
        id: &str,
        name: &str,
        resource_type: &str,
        state: &str,
    ) -> ScalewayResourceSummary {
        ScalewayResourceSummary {
            id: id.into(),
            name: name.into(),
            resource_type: resource_type.into(),
            region: "fr-par-1".into(),
            project_id: Some("p".into()),
            project_name: Some("Aspis Bio".into()),
            state: state.into(),
            commercial_type: None,
            runtime: None,
            min_scale: None,
            max_scale: None,
            domain_name: None,
            // A credential-bearing endpoint MUST never leak onto an ExternalService.
            endpoint: Some("postgres://user:secret@db.example/db".into()),
            privacy: None,
            purpose: "test".into(),
            purpose_source: "test".into(),
            tags: Vec::new(),
            image: None,
            public_ip: Some("1.2.3.4".into()),
            created_at: None,
            updated_at: None,
            oracle_query: "q".into(),
            available_actions: Vec::new(),
            idle_cost_risk: false,
        }
    }

    fn scw_inventory(
        compute: Vec<ScalewayResourceSummary>,
        storage: Vec<ScalewayStorageSummary>,
    ) -> ProviderInventory {
        let mut inv = ProviderInventory::missing(ProviderId::Scaleway);
        inv.compute = compute;
        inv.storage = storage;
        inv
    }

    fn scw_object_store(id: &str, name: &str, state: &str) -> ScalewayStorageSummary {
        ScalewayStorageSummary {
            id: id.into(),
            name: name.into(),
            storage_type: "Object Bucket".into(),
            region: "fr-par".into(),
            project_id: Some("p".into()),
            project_name: Some("Aspis Bio".into()),
            state: state.into(),
            size_gb: 10.0,
            price_eur_per_gb_hour: None,
            estimated_eur_month: None,
            pricing_label: "std".into(),
            pricing_note: String::new(),
            created_at: None,
            updated_at: None,
            tags: Vec::new(),
            billable: true,
        }
    }

    fn building_at(file_id: &str, x: f64, y: f64) -> Building {
        Building {
            file_id: file_id.into(),
            file_path: format!("src/{file_id}.rs"),
            district_id: "core".into(),
            purpose: purpose::HOUSE.into(),
            purpose_source: purpose_source::DEFAULT.into(),
            feature_id: "core".into(),
            feature_source: "directory".into(),
            provider: None,
            lines_of_code: 50,
            visual_tier: visual_tier::KALYBE.into(),
            coords: Coords::new(x, y),
            status: building_status::NORMAL.into(),
            label: file_id.into(),
            description: String::new(),
            last_modified: String::new(),
            agent_present: None,
            suspect_of_card_id: None,
            kanban_card_id: None,
            untracked_change: None,
            sins: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn maps_scaleway_container_and_cloudflare_worker_with_correct_fields() {
        // Scaleway "Serverless" container (running) + Cloudflare worker (healthy).
        let invs = vec![
            scw_inventory(
                vec![scw_compute("c1", "rnaseq-job", "Serverless", "running")],
                vec![],
            ),
            cf_inventory_with_worker("w1", "aspis-bio-api", "healthy"),
        ];

        let services = map_inventory_to_services(&invs);
        assert_eq!(services.len(), 2);

        // Deterministic order: provider "cloudflare" < "scaleway".
        assert_eq!(services[0].provider, "cloudflare");
        assert_eq!(services[0].service_type, "worker");
        assert_eq!(services[0].name, "aspis-bio-api");
        assert_eq!(services[0].status, "running");
        assert_eq!(services[0].service_id, "cf-worker-w1");
        assert!(!services[0].spawnable);

        assert_eq!(services[1].provider, "scaleway");
        assert_eq!(services[1].service_type, "container");
        assert_eq!(services[1].name, "rnaseq-job");
        assert_eq!(services[1].status, "running");
        assert_eq!(services[1].service_id, "scw-compute-c1");
        assert!(!services[1].spawnable);
    }

    #[test]
    fn empty_inventory_yields_no_services() {
        assert!(map_inventory_to_services(&[]).is_empty());
        // A "missing token" inventory has no workers/compute/storage -> empty.
        let invs = vec![
            ProviderInventory::missing(ProviderId::Cloudflare),
            ProviderInventory::missing(ProviderId::Scaleway),
        ];
        assert!(map_inventory_to_services(&invs).is_empty());
    }

    #[test]
    fn maps_compute_kinds_and_storage_to_closest_type() {
        let invs = vec![scw_inventory(
            vec![
                scw_compute("g1", "trainer", "GPU", "running"),
                scw_compute("v1", "box", "CPU VM", "stopped"),
                scw_compute("m1", "llm", "Generative API Model", "available"),
                scw_compute("s1", "fn", "Serverless SQL", "running"),
            ],
            vec![scw_object_store("o1", "bucket", "available")],
        )];
        let services = map_inventory_to_services(&invs);
        let by_name = |n: &str| services.iter().find(|s| s.name == n).unwrap().clone();

        assert_eq!(by_name("trainer").service_type, "gpu_vm");
        assert_eq!(by_name("box").service_type, "cpu_vm");
        assert_eq!(by_name("box").status, "stopped");
        assert_eq!(by_name("llm").service_type, "llm_api");
        assert_eq!(by_name("llm").status, "running"); // "available" -> running (lit)
        assert_eq!(by_name("fn").service_type, "container");
        assert_eq!(by_name("bucket").service_type, "object_store");
    }

    #[test]
    fn status_normalization_covers_transitions_and_errors() {
        assert_eq!(normalize_status("provisioning"), "spawning");
        assert_eq!(normalize_status("error"), "error");
        assert_eq!(normalize_status("degraded"), "error");
        assert_eq!(normalize_status("stopped"), "stopped");
        // Unknown -> dim "stopped", never a false lit/alarm.
        assert_eq!(normalize_status("banana"), "stopped");
        assert_eq!(normalize_status("unknown"), "stopped");
    }

    #[test]
    fn placement_is_outside_grid_no_overlap_and_deterministic() {
        // Buildings occupy roughly x in [0,5], y in [0,4].
        let buildings = vec![
            building_at("a", 0.0, 0.0),
            building_at("b", 4.0, 3.0),
            building_at("c", 2.0, 1.0),
        ];
        let (_min_x, _min_y, max_x, _max_y) = map_extent(&buildings).unwrap();

        let invs = vec![
            scw_inventory(
                vec![scw_compute("c1", "zeta", "Serverless", "running")],
                vec![],
            ),
            cf_inventory_with_worker("w1", "alpha", "healthy"),
        ];
        let mut services = map_inventory_to_services(&invs);
        place_external_services(&mut services, &buildings);

        // Every service sits beyond the building extent (seaward east margin).
        for s in &services {
            assert!(
                s.coords.x > max_x,
                "service {} at x={} must be east of building max_x={}",
                s.name,
                s.coords.x,
                max_x
            );
        }
        // No two services share a coord (own row each).
        for i in 0..services.len() {
            for j in (i + 1)..services.len() {
                assert!(
                    services[i].coords != services[j].coords,
                    "services {} and {} overlap",
                    i,
                    j
                );
            }
        }
        // No service lands on a building tile.
        for s in &services {
            for b in &buildings {
                assert!(
                    !(s.coords.x == b.coords.x && s.coords.y == b.coords.y),
                    "service {} overlaps building {}",
                    s.name,
                    b.file_id
                );
            }
        }

        // Determinism: re-run yields identical coords.
        let mut again = map_inventory_to_services(&invs);
        place_external_services(&mut again, &buildings);
        assert_eq!(services, again);
    }

    #[test]
    fn placement_anchors_at_offset_when_no_buildings() {
        let invs = vec![cf_inventory_with_worker("w1", "solo", "healthy")];
        let mut services = map_inventory_to_services(&invs);
        place_external_services(&mut services, &[]);
        assert_eq!(services.len(), 1);
        assert!(
            services[0].coords.x > 0.0,
            "harbour anchors off-origin with no buildings"
        );
    }

    #[test]
    fn attach_replaces_inventory_services_but_keeps_monuments() {
        let mut city = CityState::empty("Test", "Alpha");
        city.buildings.push(building_at("a", 0.0, 0.0));
        // A pre-existing era monument (non-inventory, legitimate) + a stale service.
        city.external_services.push(ExternalService {
            service_id: "monument-alpha".into(),
            provider: "monument".into(),
            service_type: "arco_di_trionfo".into(),
            name: "Era Alpha".into(),
            status: "running".into(),
            coords: Coords::new(0.0, 0.0),
            spawnable: false,
        });
        city.external_services.push(ExternalService {
            service_id: "scw-old".into(),
            provider: "scaleway".into(),
            service_type: "container".into(),
            name: "old".into(),
            status: "running".into(),
            coords: Coords::new(0.0, 0.0),
            spawnable: false,
        });

        let invs = vec![cf_inventory_with_worker("w1", "fresh", "healthy")];
        attach_external_services(&mut city, &invs);

        // Monument kept, stale scaleway service dropped, fresh worker added.
        let ids: Vec<&str> = city
            .external_services
            .iter()
            .map(|s| s.service_id.as_str())
            .collect();
        assert!(
            ids.contains(&"monument-alpha"),
            "monument must be preserved"
        );
        assert!(
            !ids.contains(&"scw-old"),
            "stale inventory service must be dropped"
        );
        assert!(ids.contains(&"cf-worker-w1"), "fresh worker must be added");

        // Empty inventory -> only the monument remains.
        attach_external_services(&mut city, &[]);
        assert_eq!(city.external_services.len(), 1);
        assert_eq!(city.external_services[0].provider, "monument");
    }

    fn road_with_path(id: &str, path: Vec<(i32, i32)>) -> Road {
        Road {
            road_id: id.into(),
            from: "a".into(),
            to: "b".into(),
            road_type: road_type::IMPORT.into(),
            style: road_style::TERRA_BATTUTA.into(),
            weight: 1,
            path: Some(
                path.into_iter()
                    .map(|(x, y)| Coords::new(x as f64, y as f64))
                    .collect(),
            ),
            provenance: None,
        }
    }

    // A spread of houses wide enough that `build_terrain` frames a sea + a river to
    // route around (mirrors the terrain/nav tests' `wide_city`).
    fn wide_buildings() -> Vec<Building> {
        let mut v = Vec::new();
        for (i, x) in (0..=24).step_by(4).enumerate() {
            for (j, y) in (0..=8).step_by(4).enumerate() {
                v.push(building_at(&format!("b{i}-{j}"), x as f64, y as f64));
            }
        }
        v
    }

    // FIX 2 — the load-bearing walkability guarantee runs against the FINAL terrain
    // `attach_external_services` rebuilds (real harbour count), not the scanner's
    // 0-harbour terrain. A normal city's routed roads (incl. a river crossing via a
    // bridge) stay walkable, so no nav-walkability note is surfaced — even though
    // the attach EXTENDED the sea band for the harbour column.
    #[test]
    fn attach_validates_walkability_on_the_final_rebuilt_terrain() {
        use crate::polis::terrain::build_terrain;

        let buildings = wide_buildings();
        // Find the river the terrain places for these buildings, then route a road
        // that crosses it (so the bridge path is exercised) plus a land road.
        let t0 = build_terrain(&buildings, &[], 0);
        let river = t0.rivers[0];
        let cross_y = t0.min_y + 1;
        let roads = vec![
            road_with_path(
                "cross",
                vec![(river.gx_min - 3, cross_y), (river.gx_max + 3, cross_y)],
            ),
            road_with_path("land", vec![(0, 2), (24, 2)]),
        ];

        let mut city = CityState::empty("Test", "Alpha");
        city.buildings = buildings;
        city.roads = roads;

        // Many cloud services so the harbour column (anchored at min_y, stepping
        // down by ROW_PITCH=2) reaches BELOW the land's max_y, forcing the attach to
        // EXTEND the sea band — the exact divergence FIX 2 is about (the final
        // terrain differs from the scan-time 0-harbour one). 8 services →
        // harbour_bottom ≈ min_y + 14, well past land_max_y.
        let invs = vec![scw_inventory(
            (1..=8)
                .map(|i| {
                    scw_compute(
                        &format!("c{i}"),
                        &format!("svc{i}"),
                        "Serverless",
                        "running",
                    )
                })
                .collect(),
            vec![],
        )];
        let n_services = 8usize;

        let scan_max_y = t0.max_y;
        // Cross-check the precondition with the SAME math the terrain rebuild uses:
        // the harbour column must reach below the land bottom for the band to grow.
        let harbour_bottom = harbour_bottom_y(n_services, t0.min_y as f64).ceil() as i32;
        assert!(
            harbour_bottom + 1 > scan_max_y,
            "test setup: harbour column (bottom {harbour_bottom}) must extend past land max_y {scan_max_y}"
        );

        attach_external_services(&mut city, &invs);

        // The rebuilt terrain extended the sea band (final terrain != scan terrain).
        assert!(
            city.terrain.max_y > scan_max_y,
            "attach must extend the sea band for the harbour column: scan max_y={} final max_y={}",
            scan_max_y,
            city.terrain.max_y
        );
        // The crossing road actually produced a bridge on the FINAL terrain (so the
        // guarantee really exercised the road-over-river case, not a vacuous set).
        assert!(
            !city.terrain.bridges.is_empty(),
            "the crossing road marks a bridge on the final terrain"
        );

        // Under `cargo test` debug_assertions are ON, so a violation here would have
        // already panicked. Pin the pass-path: no walkability note was surfaced.
        if let Some(note) = &city.scan_note {
            assert!(
                !note.contains("not fully walkable"),
                "a normal city must not surface a walkability violation; got: {note}"
            );
        }

        // And the guarantee, evaluated directly against the FINAL terrain, holds.
        crate::polis::nav::road_paths_all_walkable(&city.buildings, &city.roads, &city.terrain)
            .expect("every routed road tile is walkable on the final terrain");
    }

    // FIX 5 — when a walkability violation IS surfaced (release path), it is its OWN
    // distinct scan_note entry, never buried inside an unrelated file-scan-cap note;
    // and FIX 2 runs every attach, so the composition is IDEMPOTENT on re-attach.
    // We can't trip the real check in a debug build (it debug_asserts), so we verify
    // the `compose_scan_note` contract directly.
    #[test]
    fn walkability_note_is_distinct_and_idempotent() {
        let nav = "Polis terrain: a routed road is not fully walkable (tile (5, 3) is Sea)";

        // (1) No prior note → the walkability note stands alone.
        let only = compose_scan_note(None, nav.to_string());
        assert_eq!(only, nav, "alone when no prior note");

        // (2) A pre-existing file-scan-cap note → BOTH kept, clearly delimited.
        let cap = "Polis scan: stopped at 5000 files (cap)".to_string();
        let combined = compose_scan_note(Some(cap.clone()), nav.to_string());
        let parts: Vec<&str> = combined.split(" | ").collect();
        assert_eq!(parts.len(), 2, "two distinct notes: {combined}");
        assert!(
            parts[0].contains("(cap)"),
            "truncation note preserved as its own entry"
        );
        assert!(
            parts[1].contains("walkable"),
            "walkability note is its own entry"
        );

        // (3) IDEMPOTENT: re-composing onto a note that ALREADY carries the
        // walkability segment must NOT stack a duplicate copy.
        let again = compose_scan_note(Some(combined.clone()), nav.to_string());
        assert_eq!(
            again.matches(NAV_NOTE_PREFIX).count(),
            1,
            "re-attach must not duplicate the walkability note: {again}"
        );
        assert!(
            again.contains("(cap)"),
            "the truncation prefix survives re-attach"
        );

        // (4) Idempotent even with no prefix (walkability-only note re-composed).
        let nav_only_again = compose_scan_note(Some(nav.to_string()), nav.to_string());
        assert_eq!(nav_only_again, nav, "walkability-only note is idempotent");
    }

    #[test]
    fn no_secret_or_endpoint_leaks_into_services() {
        // The compute summary carries an endpoint (DSN with a password) + a public
        // IP. NEITHER may appear in ANY ExternalService field.
        let invs = vec![scw_inventory(
            vec![scw_compute("c1", "db", "Serverless SQL", "running")],
            vec![],
        )];
        let services = map_inventory_to_services(&invs);
        let json = serde_json::to_string(&services).unwrap();
        assert!(!json.contains("secret"), "no secret token may leak: {json}");
        assert!(
            !json.contains("postgres://"),
            "no DSN/endpoint may leak: {json}"
        );
        assert!(!json.contains("1.2.3.4"), "no public IP may leak: {json}");
    }
}
