// Backend for fetching + installing an external SKILL.md from a marketplace, with an SSRF guard and
// install provenance. NEVER auto-installs / auto-runs: the caller fetches → scans ([`skill_vet`]) →
// shows the owner a preview → on explicit confirm calls [`install_skill_package`].

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Reserved directory names a marketplace skill must NOT take — they are the role-skill dirs (and
/// internal agents) that the loader injects directly; an external skill named `coder` would clobber
/// the built-in role's SKILL.md (a supply-chain attack with zero privilege needed).
const RESERVED_SKILL_NAMES: &[&str] = &[
    "mini",
    "coder",
    "design",
    "orchestrator",
    "oracle",
    "censor",
    "reviewer",
    "architect",
    "mechanic",
    "core",
    "skill",
];

/// True if `v4` is one we must NOT fetch from (private / link-local / loopback / CGNAT / doc / bench).
fn is_disallowed_v4(v4: &Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()
        || v4.is_link_local()         // 169.254/16
        || v4.is_loopback()           // 127/8
        || v4.is_broadcast()          // 255.255.255.255
        || v4.is_documentation()      // 192.0.2/24, 198.51.100/24, 203.0.113/24
        || o[0] == 0                  // 0/8 "this network"
        || (o[0] == 100 && (64..=127).contains(&o[1]))   // 100.64/10 CGNAT
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))   // 198.18/15 benchmark
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)       // 192.0.0/24 IETF protocol assignments
}

/// True if `ip` is internal/disallowed. SSRF core: a marketplace URL that resolves to an internal
/// address (incl. a private v4 SMUGGLED inside a v6 mapped/compatible/NAT64/6to4 form) is refused.
fn is_disallowed_ip(ip: &IpAddr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => is_disallowed_v4(v4),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            if (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80 {
                return true; // ULA fc00::/7 + link-local fe80::/10
            }
            // Any v6 form that wraps a v4 → re-check the embedded v4 so a private v4 can't be smuggled.
            let embedded = |a: u16, b: u16| Ipv4Addr::new((a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8);
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_disallowed_v4(&v4); // ::ffff:0:0/96
            }
            if seg[..6].iter().all(|s| *s == 0) {
                return is_disallowed_v4(&embedded(seg[6], seg[7])); // IPv4-compatible ::a.b.c.d (deprecated)
            }
            if seg[0] == 0x0064 && seg[1] == 0xff9b {
                return is_disallowed_v4(&embedded(seg[6], seg[7])); // NAT64 64:ff9b::/96
            }
            if seg[0] == 0x2002 {
                return is_disallowed_v4(&embedded(seg[1], seg[2])); // 6to4 2002::/16
            }
            false
        }
    }
}

/// Parse + SSRF-validate a marketplace URL. https-only; no userinfo (it would leak credentials to an
/// untrusted server); resolves the host and refuses if ANY resolved IP is internal. Returns the URL
/// AND the validated public addresses so the fetch can PIN them (closing the DNS-rebind window).
pub fn validate_public_url(raw: &str) -> Result<(reqwest::Url, Vec<SocketAddr>), String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid URL".to_string())?;
    if url.scheme() != "https" {
        return Err("only https is allowed".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not contain credentials".to_string());
    }
    let host = url.host_str().ok_or_else(|| "missing host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| "cannot resolve host".to_string())?
        .collect();
    if addrs.is_empty() {
        return Err("host did not resolve".to_string());
    }
    for addr in &addrs {
        if is_disallowed_ip(&addr.ip()) {
            return Err("refusing to fetch from a private/loopback address".to_string());
        }
    }
    Ok((url, addrs))
}

/// Blocking GET of a pre-validated URL, PINNED to the validated `addrs` (so a DNS rebind after
/// validation can't redirect the connection to an internal IP). Redirects are DISABLED (a 3xx could
/// bounce to internal; the caller must re-validate any Location). Body is STREAM-capped at `max_bytes`
/// (we never buffer an unbounded body — a hostile multi-GB response can't OOM us).
pub fn fetch_text_capped(
    url: &reqwest::Url,
    addrs: &[SocketAddr],
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<String, String> {
    let host = url.host_str().ok_or_else(|| "missing host".to_string())?;
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::none());
    if !addrs.is_empty() {
        builder = builder.resolve_to_addrs(host, addrs);
    }
    let client = builder
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let resp = client
        .get(url.clone())
        .send()
        .map_err(|e| format!("fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("fetch failed: HTTP {}", resp.status().as_u16()));
    }
    // Early reject on an honest Content-Length, then STREAM with a hard cap (the header can lie).
    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err("response too large".to_string());
        }
    }
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    let read = resp
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if read > max_bytes {
        return Err("response too large".to_string());
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Install provenance, written to `.provenance.json` in the skill dir so an update can detect drift.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SkillProvenance {
    pub source_url: String,
    pub fetched_at: String,
    pub sha256: String,
}

/// Lowercase-hex SHA-256 of `content` (for provenance + drift detection on update).
pub fn sha256_hex(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    h.finalize().iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !RESERVED_SKILL_NAMES.contains(&name)
        && name.bytes().enumerate().all(|(i, b)| {
            if i == 0 {
                b.is_ascii_lowercase() || b.is_ascii_digit()
            } else {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            }
        })
}

/// Install a vetted SKILL.md package into `lib_root/<skill_name>/`: SKILL.md + each bundled file
/// (traversal-guarded) + `.provenance.json`. Returns the install dir. Caller must have VETTED first.
pub fn install_skill_package(
    lib_root: &Path,
    skill_name: &str,
    skill_md: &str,
    bundled: &[(String, Vec<u8>)],
    provenance: &SkillProvenance,
) -> Result<PathBuf, String> {
    if !valid_skill_name(skill_name) {
        return Err(
            "invalid or reserved skill name (need ^[a-z0-9][a-z0-9._-]{0,63}$, not a role name)"
                .to_string(),
        );
    }
    let dest = lib_root.join(skill_name);
    fs::create_dir_all(&dest).map_err(|e| format!("create skill dir failed: {e}"))?;
    let canon_dest = fs::canonicalize(&dest).map_err(|e| format!("canonicalize dest failed: {e}"))?;

    // DEFERRED TOCTOU (single-user desktop posture, same as FsBackend): we canonicalize-and-check, then
    // write by path; a racing process could swap a component in between. An O_NOFOLLOW open would close
    // it — tracked, not done (needs a platform flag / cap-std).
    fs::write(canon_dest.join("SKILL.md"), skill_md)
        .map_err(|e| format!("write SKILL.md failed: {e}"))?;

    for (rel, bytes) in bundled {
        let relp = Path::new(rel);
        // Parse-time guard: no absolute / `..` / drive-prefix.
        if relp.is_absolute()
            || relp
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(format!("refusing bundled path '{rel}' (absolute or '..')"));
        }
        let candidate = canon_dest.join(relp);
        // Canonical containment on the parent (catches a symlink-escaping component too).
        if let Some(parent) = candidate.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create dir for '{rel}' failed: {e}"))?;
            let canon_parent = fs::canonicalize(parent)
                .map_err(|e| format!("canonicalize '{rel}' parent failed: {e}"))?;
            if !canon_parent.starts_with(&canon_dest) {
                return Err(format!("refusing bundled path '{rel}' (escapes the skill dir)"));
            }
        }
        fs::write(&candidate, bytes).map_err(|e| format!("write '{rel}' failed: {e}"))?;
    }

    let prov = serde_json::to_string_pretty(provenance)
        .map_err(|e| format!("provenance serialize failed: {e}"))?;
    fs::write(canon_dest.join(".provenance.json"), prov)
        .map_err(|e| format!("write provenance failed: {e}"))?;

    Ok(canon_dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("market_test_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_non_https_creds_and_bad_url() {
        assert!(validate_public_url("http://example.com").is_err());
        assert!(validate_public_url("not a url").is_err());
        assert!(validate_public_url("ftp://example.com").is_err());
        assert!(validate_public_url("https://user:pass@example.com").is_err()); // userinfo leak
    }

    #[test]
    fn rejects_internal_literal_ips() {
        for u in [
            "https://127.0.0.1",
            "https://10.0.0.1",
            "https://169.254.1.1",
            "https://192.168.1.1",
            "https://100.64.0.1",     // CGNAT
            "https://198.18.0.1",     // benchmark
            "https://[::1]",
            "https://[::ffff:192.168.1.1]", // IPv4-mapped private
            "https://[::192.168.1.1]",      // IPv4-compatible private
            "https://[64:ff9b::a00:1]",     // NAT64 wrapping 10.0.0.1
            "https://localhost",
        ] {
            assert!(validate_public_url(u).is_err(), "{u} must be rejected");
        }
    }

    #[test]
    fn public_host_never_passes_as_private() {
        match validate_public_url("https://example.com") {
            Ok(_) => {}
            Err(e) => assert!(
                e.contains("resolve"),
                "a public host must only fail with a resolve error, got: {e}"
            ),
        }
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_writes_package_and_provenance() {
        let lib = fresh_dir("install_ok");
        let prov = SkillProvenance {
            source_url: "https://m/skill".into(),
            fetched_at: "2026-06-22T00:00:00Z".into(),
            sha256: sha256_hex("body"),
        };
        let dest = install_skill_package(
            &lib,
            "my-skill",
            "---\nname: my-skill\n---\nbody",
            &[("scripts/run.sh".into(), b"#!/bin/sh\necho hi".to_vec())],
            &prov,
        )
        .expect("install ok");
        assert_eq!(
            fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "---\nname: my-skill\n---\nbody"
        );
        assert_eq!(fs::read(dest.join("scripts/run.sh")).unwrap(), b"#!/bin/sh\necho hi");
        let read_back: SkillProvenance =
            serde_json::from_str(&fs::read_to_string(dest.join(".provenance.json")).unwrap()).unwrap();
        assert_eq!(read_back, prov);
    }

    #[test]
    fn install_rejects_traversal_and_absolute_bundled_paths() {
        let lib = fresh_dir("install_trav");
        let prov = SkillProvenance {
            source_url: "x".into(),
            fetched_at: "x".into(),
            sha256: "x".into(),
        };
        for bad in ["../escape.txt", "/etc/evil", "a/../../escape"] {
            assert!(
                install_skill_package(&lib, "skill", "body", &[(bad.to_string(), b"x".to_vec())], &prov)
                    .is_err(),
                "bundled path {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn install_name_gate_stays_tolerant_but_blocks_reserved() {
        // D2 policy: spec-conformance is surfaced as a WARNING by `skill_format::validate_skill`
        // (e.g. `_` in a name is non-conformant), NOT enforced at install. The install gate must
        // stay TOLERANT so a slightly-off-spec community skill still installs — we "warn, don't
        // break". So `my_skill` (underscore) is still accepted here on purpose…
        assert!(
            valid_skill_name("my_skill"),
            "install gate must stay tolerant of underscores (spec-conformance is a warning, not a block)"
        );
        // …while a RESERVED role name like `coder` is still hard-rejected (supply-chain clobber).
        assert!(
            !valid_skill_name("coder"),
            "reserved role names must remain blocked at install"
        );
    }

    #[test]
    fn install_rejects_bad_and_reserved_skill_names() {
        let lib = fresh_dir("install_name");
        let prov = SkillProvenance {
            source_url: "x".into(),
            fetched_at: "x".into(),
            sha256: "x".into(),
        };
        for bad in ["Bad Name", "../x", "UPPER", ".hidden", "", "x/y", "coder", "mini", "oracle"] {
            assert!(
                install_skill_package(&lib, bad, "b", &[], &prov).is_err(),
                "skill name {bad:?} must be rejected"
            );
        }
    }
}
