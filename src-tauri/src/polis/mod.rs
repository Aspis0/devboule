//! Polis Map — backend foundation.
//!
//! Self-contained module implementing the pure-Rust, fully-testable foundation
//! of the "Polis Map" feature (design doc: `aspis-bio-polis-map.md`):
//!   - `model`      — `CityState` data structures (serde camelCase contract).
//!   - `meta_store` — `.aspis-meta.json` stable UUIDs + persisted coords/purpose.
//!   - `scanner`    — `generate_city_state` core (file scan, heuristic classify,
//!                    import roads, deterministic layout, road graph + BFS).
//!   - `sins`       — the Augure's pure-Rust urban-sin detectors (with secret
//!                    redaction guarantee).
//!   - `commands`   — Tauri commands over a shared `Arc<Mutex<CityState>>`.
//!
//! DEFERRED for the user-present session (clearly-commented `// POLIS FOLLOW-UP:`
//! seams in the code): PixiJS frontend render, live Oracle classification, live
//! Scaleway integration, agent movement, visual effects, file watcher, MCP sync,
//! and the `semantic`/`infrastructure` road types.

// This is the BACKEND FOUNDATION: several public items (vocabulary constants
// matching the design doc, the `find_path`/`adjacency` road-graph API, and
// meta-store helpers) are part of the stable seam for the deferred frontend /
// Oracle / agent-movement work and are exercised by tests but not yet by app
// code. Allow dead_code at the module root rather than peppering attributes.
#![allow(dead_code)]

pub mod cloud;
pub mod commands;
pub mod footprint;
pub mod grid;
pub mod meta_store;
pub mod model;
pub mod nav;
pub mod scanner;
pub mod sins;
pub mod terrain;
pub mod watcher;

// `PolisState` is registered with `.manage(...)` in `lib.rs`; the command fns
// are referenced via their full `polis::commands::*` path by the
// `tauri::generate_handler!` macro (which needs the hidden `__cmd__*` items in
// the defining module, so they cannot be re-exported here).
pub use commands::PolisState;
