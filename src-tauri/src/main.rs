#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Headless STRUCTURE bridge: `devboule structure --root <path>` prints the
    // deterministic project structure graph as JSON and exits, with NO GUI. Detected
    // FIRST (before the auth helper + the Tauri builder) so the shared, read-only
    // `project_structure` MCP tool can reuse the Rust builder by shelling out to this
    // binary. A normal launch (no `structure` argv) returns None and falls through.
    if let Some(code) = devboule_lib::run_structure_cli_from_args() {
        std::process::exit(code);
    }
    // Headless CKG bridge: `devboule ckg --root <path>` (parallel to `structure`).
    if let Some(code) = devboule_lib::run_ckg_cli_from_args() {
        std::process::exit(code);
    }
    if let Some(code) = devboule_lib::run_auth_helper_from_args() {
        std::process::exit(code);
    }
    devboule_lib::run()
}
