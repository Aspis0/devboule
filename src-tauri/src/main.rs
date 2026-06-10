#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = aspis_management_lib::run_auth_helper_from_args() {
        std::process::exit(code);
    }
    aspis_management_lib::run()
}
