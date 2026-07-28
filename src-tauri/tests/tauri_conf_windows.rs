//! Smoke test for `bundle.windows` block in tauri.conf.json (Milestone A).
//!
//! This test does NOT need a Windows host or any platform-specific tooling —
//! it just parses the JSON config and asserts the expected shape. Run with:
//!   cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows

use serde_json::Value;

#[test]
fn tauri_conf_json_has_windows_bundle_block() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read tauri.conf.json: {e}"));
    let v: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse tauri.conf.json: {e}"));

    assert!(v["bundle"]["active"].as_bool().unwrap_or(false),
            "bundle.active must be true");
    assert!(v["bundle"]["windows"].is_object(),
            "bundle.windows must be an object");

    if let Some(m) = v["bundle"]["windows"]["webviewInstallMode"].as_object() {
        let t = m.get("type").and_then(Value::as_str).unwrap_or("");
        assert!(
            matches!(t, "downloadBootstrapper" | "embedBootstrapper"
                          | "offlineInstaller" | "fixedRuntime" | "skip"),
            "bundle.windows.webviewInstallMode.type must be a valid Tauri value (got: {t})"
        );
        let silent = m.get("silent").and_then(Value::as_bool);
        assert_eq!(silent, Some(true),
                   "bundle.windows.webviewInstallMode.silent should be true for v1");
    } else {
        panic!("bundle.windows.webviewInstallMode must be present and an object");
    }

    let install_mode = v["bundle"]["windows"]["nsis"]["installMode"]
        .as_str().unwrap_or("");
    assert!(
        matches!(install_mode, "currentUser" | "perMachine" | "both"),
        "bundle.windows.nsis.installMode must be a valid Tauri NSISInstallerMode (got: {install_mode})"
    );

    assert_eq!(
        v["bundle"]["targets"].as_str().unwrap_or("all"), "all",
        "bundle.targets must remain 'all' to keep macOS + Windows cross-platform"
    );
}

#[test]
fn tauri_conf_json_no_unexpected_windows_keys() {
    // Gate: keeps the Windows block minimal and prevents accidental schema drift.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let windows = v["bundle"]["windows"].as_object()
        .expect("bundle.windows must be an object");
    let allowed: std::collections::HashSet<&str> =
        ["wix", "nsis", "webviewInstallMode"].iter().copied().collect();
    let extra: Vec<&str> = windows.keys()
        .filter(|k| !allowed.contains(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    assert!(extra.is_empty(),
            "bundle.windows has unexpected keys: {extra:?} (v1 must stay minimal)");
}
