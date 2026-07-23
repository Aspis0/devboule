//! Lazy, sandboxed page-preview fetch for the orchestrator Websearch console.
//!
//! The websearch pipeline still only ships `{url, title, summary}` on the wire.
//! This command is invoked **lazily** from the frontend for each visited URL so
//! the console can render a real sanitized HTML thumbnail + a readable text
//! excerpt. Security is first-class: SSRF host guards (hostname + resolved IP),
//! redirect re-validation, body cap, strict CSP meta in the sanitized document
//! (blocks all network subresources in the sandboxed iframe), and best-effort
//! script/iframe/on*/javascript: stripping (no ammonia dep — regex is
//! defense-in-depth only; the CSP + FE `sandbox=""` are the real boundary).
//!
//! NEVER logs the page body.

use crate::backend::skill_marketplace::is_disallowed_ip;
use regex::Regex;
use serde::Serialize;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Hard body cap: never buffer more than this many bytes of a hostile response.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Visible-text excerpt for the FINDINGS column / AI-reader surface.
const TEXT_EXCERPT_MAX_CHARS: usize = 4000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_REDIRECTS: u32 = 3;
const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; DevboulePagePreview/1.0; +https://aspis-bio.com)";
/// Max concurrent `fetch_page_preview` blocking workers (pool starvation guard).
const PREVIEW_CONCURRENCY: usize = 4;
/// Strict CSP for sandboxed `srcDoc` previews: no network subresources at all.
/// Inline styles allowed so the thumbnail still shows structure; scripts never
/// run (sandbox + no script-src). Injected as the first element of `<head>`.
const PREVIEW_CSP_META: &str = concat!(
    r#"<meta http-equiv="Content-Security-Policy" content=""#,
    "default-src 'none'; style-src 'unsafe-inline'; img-src data:; ",
    "font-src data:; base-uri 'none'; form-action 'none'",
    r#"">"#,
);

/// Wire payload for a single page preview. camelCase for the TS side.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PagePreview {
    /// Original request URL (as the console knew it).
    pub url: String,
    /// Final URL after (re-validated) redirects.
    pub final_url: String,
    /// Best-effort `<title>` text (empty when missing).
    pub title: String,
    /// HTML with scripts/iframes/on*-attrs stripped + strict CSP meta + `<base href>`.
    pub sanitized_html: String,
    /// Visible text content, collapsed whitespace, capped.
    pub text_excerpt: String,
    /// Bytes actually read from the body (before UTF-8 lossy conversion).
    pub byte_len: usize,
    /// True when the body hit the hard cap (or text was truncated to the excerpt limit).
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested; no network).
// ---------------------------------------------------------------------------

/// Parse + SSRF-validate a page-preview URL without DNS.
///
/// Accepts `http`/`https` only. Rejects credentials, missing host, `localhost`,
/// `*.local`, loopback/private/link-local IP literals (v4 + v6). Hostname
/// forms that only resolve to private IPs are caught later at fetch time via
/// [`resolve_public_addrs`].
pub fn validate_page_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "invalid URL".to_string())?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("only http/https is allowed".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not contain credentials".to_string());
    }
    let host = url.host_str().ok_or_else(|| "missing host".to_string())?;
    if is_blocked_host(host) {
        return Err("refusing to fetch from a private/loopback/local host".to_string());
    }
    Ok(url)
}

/// Hostname / IP-literal SSRF blocklist (no DNS). Pure + total.
pub fn is_blocked_host(host: &str) -> bool {
    let host = host.trim().trim_matches(|c| c == '[' || c == ']');
    if host.is_empty() {
        return true;
    }
    let lower = host.to_ascii_lowercase();
    // Trailing-dot FQDNs (`localhost.`) and bare localhost.
    let bare = lower.trim_end_matches('.');
    if bare == "localhost" || bare.ends_with(".localhost") {
        return true;
    }
    if bare.ends_with(".local") || bare == "local" {
        return true;
    }
    // Metadata / intranet TLDs commonly used in cloud SSRF.
    if bare.ends_with(".internal") || bare == "metadata.google.internal" {
        return true;
    }
    // IPv4 literal (incl. shorthand that looks like dotted decimal).
    if let Ok(v4) = bare.parse::<Ipv4Addr>() {
        return is_disallowed_v4_local(&v4);
    }
    // IPv6 literal.
    if let Ok(v6) = bare.parse::<Ipv6Addr>() {
        return is_disallowed_ip(&IpAddr::V6(v6));
    }
    // All-numeric dotted shorthand (e.g. `127.1`) that some resolvers expand to loopback.
    let labels: Vec<&str> = bare.split('.').collect();
    let is_numeric_host = !labels.is_empty()
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_numeric_host {
        return true;
    }
    false
}

fn is_disallowed_v4_local(v4: &Ipv4Addr) -> bool {
    // Reuse the marketplace SSRF core (private/loopback/link-local/CGNAT/…).
    is_disallowed_ip(&IpAddr::V4(*v4))
}

/// Cap a byte buffer at `max_bytes`. Returns `(slice_owned, truncated)`.
pub fn cap_body(bytes: &[u8], max_bytes: usize) -> (Vec<u8>, bool) {
    if bytes.len() > max_bytes {
        (bytes[..max_bytes].to_vec(), true)
    } else {
        (bytes.to_vec(), false)
    }
}

/// Best-effort HTML sanitizer: strip scripts, iframes, objects, embeds, forms,
/// event-handler attributes, and `javascript:`/`data:` URLs in URL-bearing
/// attributes. Injects a strict CSP meta (real subresource boundary) +
/// `<base href>`. Not a full HTML parser — ammonia is not a dependency.
///
/// **Security boundary:** FE `iframe sandbox=""` (no scripts) + injected CSP
/// `default-src 'none'` (no network subresources). Regex stripping is
/// defense-in-depth only and must not be treated as authoritative.
pub fn sanitize_html(html: &str, final_url: &str) -> String {
    let mut out = html.to_string();
    out = strip_dangerous_tags(&out);
    out = strip_event_handler_attrs(&out);
    out = strip_javascript_urls(&out);
    inject_base_href(&out, final_url)
}

/// Remove dangerous elements (and their bodies for script/noscript).
/// Keeps `<style>` (inline CSS still paints under CSP `style-src 'unsafe-inline'`).
/// External `<link rel=stylesheet>` tags may remain in markup but cannot load
/// under the injected CSP (`default-src 'none'`).
///
/// Built without backreferences: the Rust `regex` crate is linear-time and does
/// not support `\1` / lookaround. Each paired alternative closes with its own
/// literal tag name.
fn strip_dangerous_tags(html: &str) -> String {
    static DANGEROUS: OnceLock<Regex> = OnceLock::new();
    let re = DANGEROUS.get_or_init(|| {
        // Per-tag open…close (no backref) + one void/self-closing/open-only form.
        // Order: body-bearing paired forms first, then residual open/void tags.
        Regex::new(
            r"(?is)<\s*script\b[^>]*>.*?<\s*/\s*script\s*>|<\s*noscript\b[^>]*>.*?<\s*/\s*noscript\s*>|<\s*iframe\b[^>]*>.*?<\s*/\s*iframe\s*>|<\s*object\b[^>]*>.*?<\s*/\s*object\s*>|<\s*embed\b[^>]*>.*?<\s*/\s*embed\s*>|<\s*form\b[^>]*>.*?<\s*/\s*form\s*>|<\s*frame\b[^>]*>.*?<\s*/\s*frame\s*>|<\s*frameset\b[^>]*>.*?<\s*/\s*frameset\s*>|<\s*base\b[^>]*>.*?<\s*/\s*base\s*>|<\s*(?:script|noscript|iframe|object|embed|form|frame|frameset|base|meta)\b[^>]*/?\s*>",
        )
        .expect("dangerous-tag regex")
    });
    re.replace_all(html, "").into_owned()
}

/// Strip `on*=` event-handler attributes (onclick, onerror, …).
///
/// Defense-in-depth only (CSP + sandbox are the boundary). Boundary before
/// `on…` is start-of-string, whitespace, `/`, `"`, or `'` so slash-separated
/// markup like `<img/src=x/onerror=alert(1)>` is caught — not only `\s+on`.
/// The boundary char is preserved via capture group 1 (no lookaround; Rust
/// `regex` is linear-time and forbids lookbehind).
fn strip_event_handler_attrs(html: &str) -> String {
    static ON_ATTR: OnceLock<Regex> = OnceLock::new();
    let re = ON_ATTR.get_or_init(|| {
        // quoted or unquoted values; restore the boundary char ($1).
        Regex::new(r#"(?i)(?:^|([\s/"']))on[a-z]+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)"#)
            .expect("on-attr regex")
    });
    re.replace_all(html, "$1").into_owned()
}

/// Neutralize `javascript:` / `data:` URLs in URL-bearing attributes.
///
/// Defense-in-depth only (CSP + sandbox are the boundary). Covers quoted and
/// unquoted values. Three separate patterns (double-quoted / single-quoted /
/// unquoted) instead of a quote-capturing backreference — Rust `regex` has
/// no `\1` in the pattern. No lookaround.
fn strip_javascript_urls(html: &str) -> String {
    // Shared attr set: classic URL attrs + media/preview surfaces.
    // (Listed per-pattern; no backrefs.)
    static JS_DQ: OnceLock<Regex> = OnceLock::new();
    static JS_SQ: OnceLock<Regex> = OnceLock::new();
    static JS_UQ: OnceLock<Regex> = OnceLock::new();
    let dq = JS_DQ.get_or_init(|| {
        Regex::new(
            r#"(?i)(href|src|action|formaction|xlink:href|srcset|poster|background|cite)\s*=\s*"\s*(?:javascript|data):[^"]*""#,
        )
        .expect("js/data-url double-quote regex")
    });
    let sq = JS_SQ.get_or_init(|| {
        Regex::new(
            r#"(?i)(href|src|action|formaction|xlink:href|srcset|poster|background|cite)\s*=\s*'\s*(?:javascript|data):[^']*'"#,
        )
        .expect("js/data-url single-quote regex")
    });
    let uq = JS_UQ.get_or_init(|| {
        Regex::new(
            r#"(?i)(href|src|action|formaction|xlink:href|srcset|poster|background|cite)\s*=\s*(?:javascript|data):[^\s>]*"#,
        )
        .expect("js/data-url unquoted regex")
    });
    let out = dq.replace_all(html, r##"$1="#""##);
    let out = sq.replace_all(&out, r#"$1='#'"#);
    uq.replace_all(&out, r##"$1="#""##).into_owned()
}

/// Inject strict CSP meta (first) + `<base href="…">` after `<head>` so the
/// sandboxed preview cannot load network subresources, while relative URL
/// resolution still has a base (harmless under `base-uri 'none'` / blocked loads).
///
/// If the document has no `<head>`, creates one containing CSP + base.
pub fn inject_base_href(html: &str, final_url: &str) -> String {
    let escaped = final_url
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;");
    let base = format!(r#"<base href="{escaped}">"#);
    // CSP must be the first thing in <head> so it applies before any other content.
    let head_inject = format!("{PREVIEW_CSP_META}{base}");
    // Drop any existing <base> first (we already strip via dangerous tags, but belt+suspenders).
    static BASE_TAG: OnceLock<Regex> = OnceLock::new();
    let base_re = BASE_TAG.get_or_init(|| {
        Regex::new(r"(?is)<\s*base\b[^>]*/?\s*>").expect("base-tag regex")
    });
    let cleaned = base_re.replace_all(html, "").into_owned();

    static HEAD_OPEN: OnceLock<Regex> = OnceLock::new();
    let head_re = HEAD_OPEN.get_or_init(|| Regex::new(r"(?is)<\s*head\b[^>]*>").expect("head regex"));
    if let Some(m) = head_re.find(&cleaned) {
        let mut s = String::with_capacity(cleaned.len() + head_inject.len() + 1);
        s.push_str(&cleaned[..m.end()]);
        s.push_str(&head_inject);
        s.push_str(&cleaned[m.end()..]);
        return s;
    }
    format!("<head>{head_inject}</head>{cleaned}")
}

/// Extract visible text: strip tags, collapse whitespace, cap at `max_chars`.
pub fn html_to_text_excerpt(html: &str, max_chars: usize) -> (String, bool) {
    // Drop script/style bodies first so their text doesn't leak into the excerpt.
    let cleaned = strip_non_visible_for_text(html);
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag_re = TAG.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("tag regex"));
    let no_tags = tag_re.replace_all(&cleaned, " ");
    let decoded = decode_basic_entities(&no_tags);
    let collapsed: String = decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() > max_chars {
        let truncated: String = collapsed.chars().take(max_chars).collect();
        (truncated, true)
    } else {
        (collapsed, false)
    }
}

/// Like [`strip_dangerous_tags`] but also drops `<style>` bodies (CSS is not visible text).
fn strip_non_visible_for_text(html: &str) -> String {
    static STYLE: OnceLock<Regex> = OnceLock::new();
    let style_re = STYLE.get_or_init(|| {
        Regex::new(r"(?is)<\s*style\b[^>]*>.*?</\s*style\s*>|<\s*style\b[^>]*/?\s*>")
            .expect("style-tag regex")
    });
    let no_style = style_re.replace_all(html, "");
    strip_dangerous_tags(&no_style)
}

/// Best-effort `<title>` extraction.
pub fn extract_title(html: &str) -> String {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let re = TITLE.get_or_init(|| {
        Regex::new(r"(?is)<\s*title\b[^>]*>(.*?)</\s*title\s*>").expect("title regex")
    });
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| {
            let raw = decode_basic_entities(m.as_str());
            raw.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .unwrap_or_default()
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

// ---------------------------------------------------------------------------
// Network path (not unit-tested; SSRF re-validation on every hop).
// ---------------------------------------------------------------------------

/// Resolve host → public addrs only. Refuses if any resolved IP is internal.
fn resolve_public_addrs(url: &reqwest::Url) -> Result<Vec<SocketAddr>, String> {
    let host = url.host_str().ok_or_else(|| "missing host".to_string())?;
    if is_blocked_host(host) {
        return Err("refusing to fetch from a private/loopback/local host".to_string());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "missing port".to_string())?;
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
    Ok(addrs)
}

/// Blocking GET of a single hop (no auto-redirect). Body stream-capped.
fn fetch_one_hop(
    url: &reqwest::Url,
    addrs: &[SocketAddr],
) -> Result<(reqwest::StatusCode, Option<String>, Vec<u8>, bool), String> {
    let host = url.host_str().ok_or_else(|| "missing host".to_string())?;
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT);
    if !addrs.is_empty() {
        builder = builder.resolve_to_addrs(host, addrs);
    }
    let client = builder
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let resp = client
        .get(url.clone())
        .send()
        .map_err(|_| "fetch failed".to_string())?;
    let status = resp.status();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    // Stream with a hard cap (Content-Length can lie; we never buffer unbounded).
    let mut buf = Vec::with_capacity(MAX_BODY_BYTES.min(64 * 1024));
    let mut limited = resp.take(MAX_BODY_BYTES as u64 + 1);
    limited
        .read_to_end(&mut buf)
        .map_err(|_| "read failed".to_string())?;
    let truncated = buf.len() > MAX_BODY_BYTES;
    if truncated {
        buf.truncate(MAX_BODY_BYTES);
    }
    Ok((status, location, buf, truncated))
}

/// Fetch + sanitize a public page. Blocking (call from a worker / Tauri command).
/// Does **not** log the body.
pub fn fetch_page_preview_blocking(raw_url: &str) -> Result<PagePreview, String> {
    let mut current = validate_page_url(raw_url)?;
    let original = current.to_string();
    let mut body = Vec::new();
    let mut body_truncated = false;
    let mut hops = 0u32;

    loop {
        let addrs = resolve_public_addrs(&current)?;
        let (status, location, bytes, truncated) = fetch_one_hop(&current, &addrs)?;
        if status.is_redirection() {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err("too many redirects".to_string());
            }
            let loc = location.ok_or_else(|| "redirect without Location".to_string())?;
            let next = current
                .join(&loc)
                .map_err(|_| "invalid redirect Location".to_string())?;
            // Re-validate every hop (SSRF: 3xx must not bounce to internal).
            current = validate_page_url(next.as_str())?;
            continue;
        }
        if !status.is_success() {
            return Err(format!("fetch failed: HTTP {}", status.as_u16()));
        }
        body = bytes;
        body_truncated = truncated;
        break;
    }

    let byte_len = body.len();
    let html = String::from_utf8_lossy(&body).into_owned();
    // Drop the raw body buffer reference path — we only keep derived fields.
    drop(body);

    let final_url = current.to_string();
    let title = extract_title(&html);
    let sanitized_html = sanitize_html(&html, &final_url);
    let (text_excerpt, text_truncated) = html_to_text_excerpt(&html, TEXT_EXCERPT_MAX_CHARS);
    // Never log `html` / body.

    Ok(PagePreview {
        url: original,
        final_url,
        title,
        sanitized_html,
        text_excerpt,
        byte_len,
        truncated: body_truncated || text_truncated,
    })
}

/// Global cap on concurrent preview fetches (protects the blocking pool).
fn preview_semaphore() -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(PREVIEW_CONCURRENCY)))
        .clone()
}

/// Tauri command: lazy page preview for the Websearch console.
/// Runs the blocking fetch off the async runtime via `spawn_blocking`.
/// Concurrency is capped by [`preview_semaphore`] so a burst of previews
/// cannot starve the blocking pool; on no free permit returns a soft
/// `"preview busy"` error (frontend already falls back to the title card).
#[tauri::command]
pub async fn fetch_page_preview(url: String) -> Result<PagePreview, String> {
    // Validate first on the async side so bad URLs fail fast without a thread.
    validate_page_url(&url)?;
    // Soft-fail when the pool is saturated (no wait — FE falls back to title card).
    let permit = preview_semaphore()
        .try_acquire_owned()
        .map_err(|_| "preview busy".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit; // held for the duration of the blocking fetch
        fetch_page_preview_blocking(&url)
    })
    .await
    .map_err(|_| "preview task failed".to_string())?
}

// ---------------------------------------------------------------------------
// Unit tests — pure helpers only (no network).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SSRF URL validation ---

    #[test]
    fn validate_accepts_public_https() {
        let u = validate_page_url("https://docs.rs/tokio").expect("public https");
        assert_eq!(u.scheme(), "https");
        assert_eq!(u.host_str(), Some("docs.rs"));
    }

    #[test]
    fn validate_accepts_public_http() {
        let u = validate_page_url("http://example.com/path").expect("public http");
        assert_eq!(u.scheme(), "http");
    }

    #[test]
    fn validate_rejects_localhost() {
        assert!(validate_page_url("http://localhost/admin").is_err());
        assert!(validate_page_url("https://localhost:8443/").is_err());
        assert!(validate_page_url("http://foo.localhost/").is_err());
    }

    #[test]
    fn validate_rejects_loopback_ip() {
        assert!(validate_page_url("http://127.0.0.1/").is_err());
        assert!(validate_page_url("http://127.1/").is_err()); // numeric shorthand
        assert!(validate_page_url("http://[::1]/").is_err());
    }

    #[test]
    fn validate_rejects_private_ips() {
        assert!(validate_page_url("http://10.0.0.5/").is_err());
        assert!(validate_page_url("http://192.168.1.1/").is_err());
        assert!(validate_page_url("http://172.16.0.1/").is_err());
        assert!(validate_page_url("http://169.254.169.254/latest/meta-data/").is_err());
    }

    #[test]
    fn validate_rejects_local_tld() {
        assert!(validate_page_url("http://printer.local/").is_err());
        assert!(validate_page_url("https://nas.local:5001/").is_err());
    }

    #[test]
    fn validate_rejects_credentials_and_non_http() {
        assert!(validate_page_url("https://user:pass@example.com/").is_err());
        assert!(validate_page_url("ftp://example.com/file").is_err());
        assert!(validate_page_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn is_blocked_host_covers_ula_and_link_local_v6() {
        assert!(is_blocked_host("fe80::1"));
        assert!(is_blocked_host("fc00::1"));
        assert!(is_blocked_host("fec0::1")); // deprecated site-local
        assert!(!is_blocked_host("example.com"));
        assert!(!is_blocked_host("1.1.1.1")); // public Cloudflare DNS
    }

    // --- HTML sanitize ---

    #[test]
    fn sanitize_strips_script_iframe_and_onclick() {
        let raw = r#"<html><head><title>T</title></head><body>
            <script>alert(1)</script>
            <p onclick="x">hi</p>
            <iframe src="evil"></iframe>
            <a href="javascript:alert(1)">x</a>
            </body></html>"#;
        let out = sanitize_html(raw, "https://example.com/page");
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "script tag must be gone: {out}");
        assert!(!lower.contains("alert(1)"), "script body must be gone: {out}");
        assert!(!lower.contains("<iframe"), "iframe must be gone: {out}");
        assert!(!lower.contains("onclick"), "onclick must be gone: {out}");
        assert!(!lower.contains("javascript:"), "js url must be gone: {out}");
        assert!(out.contains("hi"), "safe text retained: {out}");
        assert!(
            out.contains(r#"<base href="https://example.com/page">"#),
            "base href injected: {out}"
        );
    }

    #[test]
    fn sanitize_strips_object_embed_form() {
        let raw = r#"<object data="x"></object><embed src="y"><form action="/p"><input></form><p>ok</p>"#;
        let out = sanitize_html(raw, "https://ex.com/");
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains("<object"));
        assert!(!lower.contains("<embed"));
        assert!(!lower.contains("<form"));
        assert!(out.contains("ok"));
    }

    /// CSP meta is the real subresource-SSRF boundary for sandboxed srcDoc.
    #[test]
    fn sanitize_injects_strict_csp_meta_before_base() {
        let raw = r#"<html><head><title>T</title></head><body><p style="color:red">hi</p></body></html>"#;
        let out = sanitize_html(raw, "https://example.com/page");
        assert!(
            out.contains(r#"http-equiv="Content-Security-Policy""#),
            "CSP meta must be present: {out}"
        );
        assert!(
            out.contains("default-src 'none'"),
            "CSP must include default-src 'none': {out}"
        );
        assert!(
            out.contains("style-src 'unsafe-inline'"),
            "CSP must allow inline styles: {out}"
        );
        // CSP meta must appear before <base> so it applies first.
        let csp_pos = out.find("Content-Security-Policy").expect("csp");
        let base_pos = out.find(r#"<base href="#).expect("base");
        assert!(csp_pos < base_pos, "CSP must be injected before base: {out}");
        // No <head> → create one with CSP+base.
        let no_head = sanitize_html("<p>x</p>", "https://ex.com/");
        assert!(
            no_head.contains("<head>") && no_head.contains("Content-Security-Policy"),
            "missing head must get a head with CSP: {no_head}"
        );
    }

    /// Slash-separated attrs must not bypass the on* stripper (defense-in-depth).
    #[test]
    fn sanitize_strips_slash_separated_onerror() {
        let raw = r#"<img/src=x/onerror=alert(1)>"#;
        let out = sanitize_html(raw, "https://example.com/");
        let lower = out.to_ascii_lowercase();
        assert!(
            !lower.contains("onerror"),
            "slash-separated onerror must be stripped: {out}"
        );
        assert!(
            !lower.contains("alert(1)"),
            "handler body must be gone: {out}"
        );
    }

    /// Unquoted javascript: and quoted data: in URL attrs must be neutralized.
    #[test]
    fn sanitize_neutralizes_unquoted_js_and_data_urls() {
        let raw = r#"<a href=javascript:alert(1)>x</a><video poster="data:text/html,evil"></video>"#;
        let out = sanitize_html(raw, "https://example.com/");
        let lower = out.to_ascii_lowercase();
        assert!(
            !lower.contains("javascript:"),
            "unquoted javascript: must be neutralized: {out}"
        );
        assert!(
            !lower.contains("data:text/html"),
            "poster data: must be neutralized: {out}"
        );
        assert!(
            lower.contains(r##"href="#""##) || lower.contains("href='#'"),
            "href must be rewritten to #: {out}"
        );
    }

    // --- html → text ---

    #[test]
    fn html_to_text_strips_tags_and_collapses() {
        let (text, trunc) = html_to_text_excerpt(
            "<html><body><h1>Hello</h1><p>world   &amp;  friends</p></body></html>",
            4000,
        );
        assert_eq!(text, "Hello world & friends");
        assert!(!trunc);
    }

    #[test]
    fn html_to_text_ignores_script_body() {
        let (text, _) = html_to_text_excerpt(
            "<p>safe</p><script>var secret = 1;</script><p>end</p>",
            4000,
        );
        assert_eq!(text, "safe end");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn html_to_text_respects_char_cap() {
        let long = format!("<p>{}</p>", "ab ".repeat(3000));
        let (text, trunc) = html_to_text_excerpt(&long, 100);
        assert!(trunc);
        assert!(text.chars().count() <= 100);
    }

    // --- body cap ---

    #[test]
    fn cap_body_flags_truncation() {
        let data = vec![b'x'; 100];
        let (kept, trunc) = cap_body(&data, 50);
        assert_eq!(kept.len(), 50);
        assert!(trunc);
        let (kept2, trunc2) = cap_body(&data, 200);
        assert_eq!(kept2.len(), 100);
        assert!(!trunc2);
    }

    #[test]
    fn extract_title_reads_title_tag() {
        assert_eq!(
            extract_title("<html><head><title>  Foo &amp; Bar  </title></head></html>"),
            "Foo & Bar"
        );
        assert_eq!(extract_title("<p>no title</p>"), "");
    }
}
