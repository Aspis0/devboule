use std::path::{Path, PathBuf};
use super::{NetPolicy, SandboxPolicy};

/// Escape a string for an SBPL double-quoted literal: backslash FIRST, then double-quote,
/// so a path containing either cannot terminate the literal early and corrupt the profile.
///
/// Single source of truth: `mini_coder_executor::build_seatbelt_profile` imports this (the former
/// duplicate copy was removed, review F3). The one-shot profile FUNCTION is still separate.
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn sbpl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Canonicalize an absolute path for an SBPL `(subpath ...)` rule so the rule matches the REAL
/// inode the kernel checks (resolves `.`/`..`/symlinks). Falls back to the input when the path
/// does not exist yet (a not-yet-created scratch dir still needs its lexical subpath allowed).
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub(crate) fn canonical_sandbox_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Build the Seatbelt/SBPL profile string for `policy`. Generalizes the loopback-only mini
/// profile: reads are broad; writes are deny-default and limited to /dev/null + TMPDIR +
/// `policy.writable_paths` (each canonicalized + SBPL-escaped); the network section is chosen
/// by `policy.net`. `policy.readonly_root` is readable (via the broad read rule) and ABSENT from
/// file-write* => read-only. Kept uncfg'd so it is unit-testable on non-macOS dev hosts.
pub fn build_profile(policy: &SandboxPolicy) -> String {
    let mut p = String::new();
    p.push_str("(version 1)\n(deny default)\n\n");
    p.push_str("; reads broad — a tight file-read* breaks python3/dyld at load (dyld shared cache on a separate APFS volume).\n");
    p.push_str("; the security boundary lives on the WRITES (deny-by-default) and the NETWORK, NOT on reads.\n");
    p.push_str("(allow file-read*)\n(allow file-read-metadata)\n(allow sysctl-read)\n(allow mach-lookup)\n\n");

    p.push_str("; writes deny-by-default; ONLY /dev/null + TMPDIR + the policy's writable_paths\n");
    p.push_str("(allow file-write*\n");
    p.push_str("    (literal \"/dev/null\")\n");

    let tmpdir = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // review F2(race): a SILENT canonicalize fallback would emit a lexical TMPDIR rule that the
    // kernel (which resolves symlinks at open-time) won't match → the child's temp writes get
    // denied and it fails silently. Warn loudly so an operator can diagnose the broken sandbox.
    let tmpdir_canon = match std::fs::canonicalize(&tmpdir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[sandbox] WARNING: TMPDIR {tmpdir:?} canonicalize failed ({e}); sandboxed temp writes may be denied");
            tmpdir.clone()
        }
    };
    p.push_str(&format!("    (subpath \"{}\")\n", sbpl_escape(&tmpdir_canon.to_string_lossy())));

    for wp in &policy.writable_paths {
        // A relative `(subpath "..")` has dangerous CWD-relative SBPL semantics (the CWD is the
        // child's, attacker-influenceable). Skip non-absolute entries fail-safe (never widen).
        if !wp.is_absolute() {
            eprintln!("[sandbox] skipping non-absolute writable_path (dangerous CWD-relative SBPL): {wp:?}");
            continue;
        }
        let wp_canon = canonical_sandbox_path(wp);
        p.push_str(&format!("    (subpath \"{}\")\n", sbpl_escape(&wp_canon.to_string_lossy())));
    }

    p.push_str(")\n\n");

    // SECURITY (review F1): even when a writable path covers the project root, NEVER allow writes
    // to .git or .devboule. A build script run under the sandbox could otherwise plant a
    // .git/hooks/* script that the user's OWN git would later execute OUTSIDE the sandbox (RCE).
    // The deny comes AFTER the allow so it wins (SBPL is last-match-wins). Only meaningful when
    // readonly_root is a real absolute path.
    // SECURITY (review F1/F2): NEVER allow writes to a `.git` or `.devboule` directory — even when
    // a writable_path covers the project root. A build script could otherwise plant a `.git/hooks/*`
    // the user's own git later runs OUTSIDE the sandbox (RCE). A REGEX (not a top-level subpath)
    // covers .git ANYWHERE (nested repos / submodule module dirs), and is harmless globally since
    // the only writable paths are the explicit allowlist anyway. (deny wins — SBPL last-match.)
    p.push_str("; security: deny writes to any .git / .devboule (RCE via planted hooks), nested too\n");
    p.push_str("(deny file-write* (regex #\"/\\.git($|/)\"))\n");
    p.push_str("(deny file-write* (regex #\"/\\.devboule($|/)\"))\n");
    p.push('\n');

    // exec is NOT the security boundary (writes + network are). Allow process-exec BROADLY so the
    // agent runs toolchains living in $HOME (~/.cargo/bin, ~/.rustup, nvm/pyenv/…) AND the test
    // binaries it compiles into target/ (e.g. `cargo test` exec's target/debug/<test>). Spawned
    // children inherit THIS profile, so they remain write+network confined regardless. (review F1)
    p.push_str("; exec: broad — exec is not the boundary; children inherit the write+net confinement\n");
    p.push_str("(allow process-exec)\n");
    p.push_str("(allow process-fork)\n\n");

    match policy.net {
        NetPolicy::None => {
            p.push_str("; network: deny all\n");
            p.push_str("(deny network*)\n");
        }
        NetPolicy::Loopback => {
            p.push_str("; network: deny all, allow loopback only (covers 127.0.0.1 AND ::1; external IP stays denied)\n");
            p.push_str("(deny network*)\n");
            p.push_str("(allow network-outbound\n");
            p.push_str("    (remote tcp \"localhost:*\")\n");
            p.push_str("    (remote udp \"localhost:*\"))\n");
        }
        NetPolicy::Enabled => {
            // Full network for a trusted-egress role. `network*` covers outbound, inbound,
            // bind, AND AF_UNIX sockets (which network-outbound/inbound alone do NOT).
            p.push_str("; network: allow all (trusted-egress role)\n");
            p.push_str("(allow network*)\n");
        }
    }

    p
}

#[cfg(test)]
mod tests {
    use super::*;
    // ResourceLimits lives in the parent `sandbox` module (mod.rs), not in `seatbelt`.
    use super::super::ResourceLimits;
    use std::path::PathBuf;

    #[test]
    fn net_none_denies_all() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![],
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("network-outbound"));
    }

    #[test]
    fn net_loopback_allows_only_localhost() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![],
            net: NetPolicy::Loopback,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        assert!(profile.contains("(remote tcp \"localhost:*\")"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(allow network-outbound\n    (remote"));
        assert!(!profile.contains("(allow network-outbound)\n"));
        assert!(!profile.contains("(allow network-inbound)"));
    }

    #[test]
    fn net_enabled_allows_all_network() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![],
            net: NetPolicy::Enabled,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn writable_paths_appear_under_file_write() {
        // The writable path must be absolute on every host platform: on Windows
        // a POSIX-style "/tmp/..." is relative and would be skipped by the
        // builder, breaking this test on Windows CI hosts.
        #[cfg(target_os = "windows")]
        let writable = PathBuf::from(r"C:\tmp\scratch-xyz");
        #[cfg(not(target_os = "windows"))]
        let writable = PathBuf::from("/tmp/scratch-xyz");
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![writable],
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        let write_idx = profile.find("(allow file-write*").expect("should contain file-write*");
        let subpath_idx = profile.find("scratch-xyz").expect("should contain scratch-xyz");
        assert!(subpath_idx > write_idx, "subpath should appear after file-write*");
    }

    #[test]
    fn non_absolute_writable_path_is_skipped() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![PathBuf::from("../../etc")],
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        assert!(!profile.contains("etc"), "a non-absolute writable path must NOT be emitted");
    }

    #[test]
    fn reads_are_broad_and_default_deny() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/readonly"),
            readonly_paths: vec![],
            writable_paths: vec![],
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        assert!(profile.starts_with("(version 1)\n(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
    }

    /// The load-bearing regression a string test cannot give: feed the generated profile to the
    /// REAL kernel parser and assert it is ACCEPTED, the writable path is WRITABLE, and writes to
    /// both the readonly root AND an unlisted external path are DENIED (proving writable_paths is
    /// an allowlist). readonly/writable/outside live under `/private/tmp`, OUTSIDE the `$TMPDIR`
    /// (`/var/folders/...`) that build_profile always grants — so deny checks are meaningful.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_real_parser_regression() {
        let pid = std::process::id();
        let readonly = PathBuf::from(format!("/private/tmp/sb_ro_test_{pid}"));
        let writable = PathBuf::from(format!("/private/tmp/sb_rw_test_{pid}"));
        let _ = std::fs::create_dir_all(&readonly);
        let _ = std::fs::create_dir_all(&writable);

        let policy = SandboxPolicy {
            readonly_root: readonly.clone(),
            readonly_paths: vec![],
            writable_paths: vec![writable.clone()],
            net: NetPolicy::None,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);

        let res = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg("echo ok")
            .output()
            .expect("failed to execute sandbox-exec");
        assert!(
            res.status.success(),
            "sandbox-exec should accept the generated profile: {}",
            String::from_utf8_lossy(&res.stderr)
        );

        // write to readonly_root must be DENIED
        let should_fail = readonly.join("should_fail");
        let _ = std::fs::remove_file(&should_fail);
        let res2 = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(format!("echo x > {}/should_fail", readonly.display()))
            .output()
            .expect("failed to execute sandbox-exec");
        assert!(
            !res2.status.success(),
            "sandbox-exec should DENY a write to the readonly root"
        );
        assert!(
            !should_fail.exists(),
            "the denied write must not have created the file"
        );

        // (a) write INTO a writable_path must SUCCEED
        let allowed = writable.join("ok.txt");
        let _ = std::fs::remove_file(&allowed);
        let res3 = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(format!("echo y > {}/ok.txt", writable.display()))
            .output()
            .expect("failed to execute sandbox-exec");
        assert!(
            res3.status.success(),
            "write into a writable_path must be ALLOWED: {}",
            String::from_utf8_lossy(&res3.stderr)
        );
        assert!(allowed.exists(), "the allowed write must have created the file");

        // (b) write to an UNLISTED external path must be DENIED (allowlist, not deny-all-except-readonly)
        let outside = PathBuf::from(format!("/private/tmp/sb_outside_{pid}"));
        let _ = std::fs::create_dir_all(&outside);
        let outside_file = outside.join("should_fail");
        let _ = std::fs::remove_file(&outside_file);
        let res4 = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(format!("echo x > {}/should_fail", outside.display()))
            .output()
            .expect("failed to execute sandbox-exec");
        assert!(
            !res4.status.success(),
            "write to an UNLISTED external path must be DENIED"
        );
        assert!(
            !outside_file.exists(),
            "the denied external write must not have created the file"
        );

        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&readonly);
        let _ = std::fs::remove_dir_all(&writable);
    }

    /// The Enabled (`(allow network*)`) profile must be accepted by the kernel parser.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_enabled_profile_accepted_by_kernel() {
        let policy = SandboxPolicy {
            readonly_root: PathBuf::from("/private/tmp"),
            readonly_paths: vec![],
            writable_paths: vec![],
            net: NetPolicy::Enabled,
            rlimits: ResourceLimits::default(),
        };
        let profile = build_profile(&policy);
        let res = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg("echo ok")
            .output()
            .expect("failed to execute sandbox-exec");
        assert!(
            res.status.success(),
            "Enabled profile must be accepted by the kernel: {}",
            String::from_utf8_lossy(&res.stderr)
        );
    }

    /// SECURITY (review F1): `.git` must be DENIED for writes even when the project root is writable
    /// (otherwise a build script could plant a hook the user's git later runs unsandboxed = RCE).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_git_dir_denied_even_when_root_writable() {
        let pid = std::process::id();
        let root = PathBuf::from(format!("/private/tmp/sb_git_test_{pid}"));
        let _ = std::fs::create_dir_all(root.join(".git/hooks"));
        let policy = SandboxPolicy::deny(root.clone()).writable(root.clone());
        let profile = build_profile(&policy);

        // a normal write under the writable root SUCCEEDS
        let ok = root.join("src.txt");
        let _ = std::fs::remove_file(&ok);
        let r_ok = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p").arg(&profile).arg("--")
            .arg("/bin/sh").arg("-c").arg(format!("echo y > {}/src.txt", root.display()))
            .output().expect("sandbox-exec");
        assert!(
            r_ok.status.success(),
            "write under the writable root must be allowed: {}",
            String::from_utf8_lossy(&r_ok.stderr)
        );

        // a write into .git/hooks is DENIED despite the root being writable
        let hook = root.join(".git/hooks/post-checkout");
        let _ = std::fs::remove_file(&hook);
        let r_git = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p").arg(&profile).arg("--")
            .arg("/bin/sh").arg("-c").arg(format!("echo evil > {}/.git/hooks/post-checkout", root.display()))
            .output().expect("sandbox-exec");
        assert!(!r_git.status.success(), "write into .git must be DENIED");
        assert!(!hook.exists(), "the .git hook must not have been created");

        let _ = std::fs::remove_dir_all(&root);
    }
}
