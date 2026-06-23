//! The real async model client (L2.3): an OpenAI-compatible chat-completions
//! client against the LOOPBACK oMLX server.
//!
//! [`OmlxModel`] implements [`CoderModel`]: each `next_output` builds the burst
//! transcript + the system prompt into a chat-completions request and POSTs it
//! to the configured loopback base URL, returning the assistant message text for
//! the [`crate::action`] parser.
//!
//! Privacy: the request carries the human message, the transcript (which can
//! contain file content), and the system prompt — so the endpoint MUST be
//! loopback and HTTP-only. [`validate_omlx_base_url`] mirrors the
//! `mini_coder::validate_omlx_base_url` semantics: http-only (a self-signed TLS
//! cert on a loopback server would silently fail verification), a loopback host
//! (`localhost` / `127.0.0.0/8` / `[::1]`) with a valid optional `:port`, and a
//! userinfo-trick (`127.0.0.1@evil.com`) rejection.
//!
//! We use a thin `reqwest` client rather than `async-openai`: the heavy SDK
//! would pull a SECOND reqwest + a backoff/derive tree for a single loopback
//! call, and we ALREADY depend on `reqwest` for the Exa backend — one HTTP stack
//! is the clean minimal build. Live inference is GPU-deferred; only the request
//! building + the loopback validator are unit-tested here.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::agent_loop::{Transcript, TranscriptEntry};
use crate::model::CoderModel;
use crate::prompt::{build_system_prompt_with_lang, UserMcpServerTools};

/// Max length of the oMLX base URL. Mirrors `mini_coder::MINI_BASE_URL_MAX_LEN`.
const OMLX_BASE_URL_MAX_LEN: usize = 200;

/// Default sampling cap for one model turn. Bounded so a runaway generation
/// cannot stall a burst round; the burst loop additionally re-caps the parsed
/// result text.
const MAX_TOKENS: u32 = 2048;

/// Aggregate cap (chars) on the ACTION/RESULT history re-fed to the model each
/// round. Each result is already capped at 16 KB, but across ~14 rounds the
/// transcript reaches hundreds of KB re-serialized into EVERY request. We keep
/// the system prompt + the original human message ALWAYS, and roll off the
/// OLDEST action+result pairs once the rendered history would exceed this — so
/// the model always sees the task and the most-recent rounds, never an unbounded
/// blob. Sized to comfortably hold several full-size rounds.
const MAX_TRANSCRIPT_CHARS: usize = 100_000;

/// Is a char forbidden in a launch/URL string? Mirrors
/// `mini_coder::is_forbidden_command_char`: control chars plus the bidi /
/// invisible / format blocklist, so a right-to-left override or zero-width char
/// cannot smuggle hidden semantics into the URL.
fn is_forbidden_url_char(ch: char) -> bool {
    if ch.is_control() {
        return true;
    }
    matches!(ch,
        '\u{00ad}'
        | '\u{061c}'
        | '\u{180e}'
        | '\u{200b}'..='\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{feff}'
    )
}

/// Is an optional `:port` valid? `None` is fine; a present port is 1-5 digits
/// and parses to <= 65535; an empty port (`host:`) is rejected. Mirrors
/// `mini_coder::is_valid_optional_port`.
fn is_valid_optional_port(port: Option<&str>) -> bool {
    match port {
        None => true,
        Some(p) => {
            !p.is_empty()
                && p.len() <= 5
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().map(|n| n <= 65535).unwrap_or(false)
        }
    }
}

/// Validate + NORMALIZE an oMLX base URL (trailing slash stripped), or a human
/// error string. LOOPBACK-ONLY + HTTP-ONLY by design (privacy): the prompt may
/// carry file content, so a non-loopback host could route it off the machine,
/// and a self-signed TLS cert on a loopback server would silently fail
/// verification (so `https://` is rejected). Mirrors
/// `mini_coder::validate_omlx_base_url`.
pub fn validate_omlx_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("oMLX base URL must not be empty.".into());
    }
    if trimmed.len() > OMLX_BASE_URL_MAX_LEN {
        return Err(format!(
            "oMLX base URL must be at most {OMLX_BASE_URL_MAX_LEN} characters."
        ));
    }
    if trimmed.chars().any(is_forbidden_url_char) {
        return Err("oMLX base URL must not contain control, bidi or invisible characters.".into());
    }

    // Scheme: http only (loopback, like Ollama). `https://` is rejected.
    let rest = match trimmed.strip_prefix("http://") {
        Some(r) => r,
        None => return Err("oMLX base URL must start with http:// (loopback, http only).".into()),
    };

    // Authority = up to the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("oMLX base URL must include a loopback host.".into());
    }

    let is_loopback = if let Some(after) = authority.strip_prefix("[::1]") {
        // IPv6 loopback `[::1]` optionally followed by `:port`. Reject a userinfo
        // trick (`[::1]:8000@evil.com`): an `@` in the remainder means the real
        // host is after the `@`.
        !after.contains('@')
            && (after.is_empty() || after.starts_with(':'))
            && is_valid_optional_port(after.strip_prefix(':'))
    } else if authority.contains('@') {
        // `127.0.0.1@evil.com`: real host is after the `@`.
        false
    } else {
        let mut parts = authority.splitn(2, ':');
        let host = parts.next().unwrap_or("");
        let host_is_loopback = host == "localhost"
            || host
                .parse::<std::net::Ipv4Addr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false);
        host_is_loopback && is_valid_optional_port(parts.next())
    };
    if !is_loopback {
        return Err(
            "oMLX base URL host must be loopback (localhost, 127.0.0.1 or [::1]) with a valid optional :port."
                .into(),
        );
    }

    // Normalize: strip a single trailing slash so `<base>/chat/completions` is clean.
    Ok(trimmed.strip_suffix('/').unwrap_or(trimmed).to_string())
}

/// Validate + NORMALIZE a CLOUD base URL (trailing slash stripped), or a human error
/// string. This is the OPT-IN, consent-gated counterpart to
/// [`validate_omlx_base_url`]: where the loopback validator FORBIDS leaving the
/// machine, the cloud validator requires it — `https://` (TLS, since a real provider
/// is never on loopback) and a NON-loopback host (a loopback host here is a
/// misconfiguration, not a cloud endpoint). The same control/bidi/invisible char
/// rejection and the same userinfo-trick (`user@host`) rejection apply, so a
/// credential or a hidden-semantics host can never be smuggled into the URL.
///
/// This NEVER touches the loopback path: a Local-mode user goes through
/// `validate_omlx_base_url` exclusively and sends nothing off-machine. The cloud
/// path is reached ONLY when the operator explicitly configured Cloud mode and
/// consented (see the host UI's mandatory disclosure).
pub fn validate_cloud_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("Cloud base URL must not be empty.".into());
    }
    if trimmed.len() > OMLX_BASE_URL_MAX_LEN {
        return Err(format!(
            "Cloud base URL must be at most {OMLX_BASE_URL_MAX_LEN} characters."
        ));
    }
    if trimmed.chars().any(is_forbidden_url_char) {
        return Err(
            "Cloud base URL must not contain control, bidi or invisible characters.".into(),
        );
    }

    // Scheme: https ONLY. A cloud provider is reached over TLS; http would send the
    // prompt (which can carry file content) in clear text off the machine.
    let rest = match trimmed.strip_prefix("https://") {
        Some(r) => r,
        None => return Err("Cloud base URL must start with https:// (TLS required).".into()),
    };

    // Authority = up to the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    // Reject a userinfo trick (`user:pass@host`, `host@evil.com`): credentials must
    // never live in the URL (they go in the Authorization header), and an `@` also
    // hides the real host after it.
    if authority.contains('@') {
        return Err("Cloud base URL must not contain credentials (no '@' / userinfo).".into());
    }

    // Split off an optional `:port`. An IPv6 literal `[..]` is rejected outright: a
    // cloud provider is addressed by hostname, and a bare IP literal here is far more
    // likely an SSRF attempt than a real endpoint.
    if authority.starts_with('[') {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }
    let mut parts = authority.splitn(2, ':');
    let host = parts.next().unwrap_or("");
    if !is_valid_optional_port(parts.next()) {
        return Err("Cloud base URL has an invalid :port.".into());
    }

    // The host must be a real public hostname: reject loopback (a loopback host in
    // Cloud mode is a misconfiguration — Local mode is the loopback path), reject a
    // bare IPv4 literal (SSRF surface), and require at least one dot so a single-label
    // intranet name (`internal`, `metadata`) cannot be targeted.
    if host.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost" {
        return Err("Cloud base URL host must be a public provider host, not loopback.".into());
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }
    // Rust's `Ipv4Addr` parser REJECTS leading-zero dotted-quads (`01.02.03.04`,
    // `010.0.0.1`, `0177.0.0.1`) and out-of-range quads (`999.999.999.999`), so those
    // slip past the parse above and look like a hostname (all labels are alphanumeric).
    // Reject any host that is exactly 4 dot-separated all-ASCII-digit labels: a numeric
    // dotted-quad is always an IP-literal-disguised target, never a real provider host.
    let numeric_quad: Vec<&str> = host.split('.').collect();
    if numeric_quad.len() == 4
        && numeric_quad
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }
    // PARTIAL SSRF mitigation: deny the well-known cloud-metadata FQDN and the conventional
    // intranet suffixes `.internal` / `.local`. This is NOT complete SSRF protection —
    // COMPLETE protection requires post-DNS-resolution IP filtering (reject RFC1918 /
    // link-local / loopback RESOLVED IPs) in the HTTP client's connect layer (a custom
    // reqwest resolver). That is a deliberate follow-up and is intentionally NOT done here.
    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
    {
        return Err("Cloud base URL host must be a public provider host, not an intranet/metadata name.".into());
    }
    if !host.contains('.') {
        return Err("Cloud base URL host must be a fully-qualified domain name.".into());
    }
    // Each label must be a sane DNS label (alnum + hyphen, not empty). This bars
    // smuggled path/query bytes and keeps the SigV4-style key surface clean.
    let labels_ok = host.split('.').all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if !labels_ok {
        return Err("Cloud base URL host is not a valid domain name.".into());
    }

    // Normalize: strip a single trailing slash so `<base>/chat/completions` is clean.
    Ok(trimmed.strip_suffix('/').unwrap_or(trimmed).to_string())
}

// ---------------------------------------------------------------------------
// SSRF guard: pure IP classifier + reqwest DNS resolver
// ---------------------------------------------------------------------------

/// Returns `true` for any IPv4 address that is not safely routable on the public Internet.
/// Pure function — no I/O, no allocations, deterministic. Safe to unit-test without network.
fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();

    // 0.0.0.0/8 — "this network" (RFC 1122). Covers `is_unspecified()` (0.0.0.0).
    if o[0] == 0 {
        return true;
    }

    // 10/8, 172.16/12, 192.168/16 — RFC 1918 private.
    if ip.is_private() {
        return true;
    }

    // 127/8 loopback (covers 127.0.0.1).
    if ip.is_loopback() {
        return true;
    }

    // 169.254/16 link-local — also covers 169.254.169.254 cloud metadata.
    if ip.is_link_local() {
        return true;
    }

    // 100.64.0.0/10 CGNAT (RFC 6598) — std has no helper.
    if o[0] == 100 && (64..=127).contains(&o[1]) {
        return true;
    }

    // 192.0.0.0/24 — IETF Protocol Assignments (RFC 6890).
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }

    // 192.88.99.0/24 — 6to4 relay anycast (RFC 3068).
    if o[0] == 192 && o[1] == 88 && o[2] == 99 {
        return true;
    }

    // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24 — TEST-NET (RFC 5737).
    if ip.is_documentation() {
        return true;
    }

    // 198.18.0.0/15 — benchmarking (RFC 2544). std `is_documentation()` does not cover this.
    if o[0] == 198 && (18..=19).contains(&o[1]) {
        return true;
    }

    // 224.0.0.0/4 multicast (RFC 5771).
    if ip.is_multicast() {
        return true;
    }

    // 240.0.0.0/4 reserved for future use (RFC 1112). Includes broadcast 255.255.255.255.
    if o[0] >= 240 {
        return true;
    }

    false
}

/// Returns `true` for any IPv6 address that is not safely routable on the public Internet.
/// Pure function — no I/O, no allocations, deterministic.
fn is_blocked_ipv6(ip: std::net::Ipv6Addr) -> bool {
    let s = ip.segments();

    // IPv4-mapped (::ffff:a.b.c.d) — most common rebinding attack vector. Recurse into the v4
    // rules so `::ffff:127.0.0.1` and `::ffff:169.254.169.254` are blocked.
    // `Ipv6Addr::to_ipv4_mapped()` is stable since Rust 1.63.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }

    // ::1 loopback.
    if ip.is_loopback() {
        return true;
    }

    // :: unspecified.
    if ip.is_unspecified() {
        return true;
    }

    // ff00::/8 multicast (RFC 4291).
    if ip.is_multicast() {
        return true;
    }

    // fc00::/7 unique-local (RFC 4193). `Ipv6Addr::is_unique_local()` only stabilized in
    // Rust 1.84 — implement manually for portability across toolchains.
    if (s[0] & 0xfe00) == 0xfc00 {
        return true;
    }

    // fe80::/10 unicast link-local (RFC 4291). `Ipv6Addr::is_unicast_link_local()` only
    // stabilized in Rust 1.84 — implement manually for portability.
    if (s[0] & 0xffc0) == 0xfe80 {
        return true;
    }

    // fec0::/10 site-local (RFC 3879, deprecated). Not globally routable.
    if (s[0] & 0xffc0) == 0xfec0 {
        return true;
    }

    // 2001:db8::/32 — IPv6 documentation (RFC 3849).
    if s[0] == 0x2001 && s[1] == 0x0db8 {
        return true;
    }

    // 64:ff9b::/96 — NAT64 (RFC 6052). Decode the embedded IPv4 from the low 32 bits
    // and recurse so `64:ff9b::169.254.169.254` is blocked.
    if s[0] == 0x0064 && s[1] == 0xff9b {
        let v4 = std::net::Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        );
        return is_blocked_ipv4(v4);
    }

    // IPv4-compatible (::a.b.c.d, RFC 4291, deprecated). First 96 bits are zero; the trailing
    // 32 bits form a v4. (`to_ipv4_mapped()` does NOT match these — they lack the `::ffff:`
    // marker.) Apply the v4 rules. Note: ::1 and :: already short-circuit above.
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        let v4 = std::net::Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        );
        return is_blocked_ipv4(v4);
    }

    false
}

/// Exhaustive SSRF guard for the cloud LLM HTTP client: returns `true` for any IP address
/// that must never be connected to (loopback, private, link-local incl. cloud metadata,
/// CGNAT, documentation, benchmarking, multicast, reserved, IPv4-mapped-into-IPv6 forms,
/// IPv4-compatible IPv6, ULA, site-local, etc.).
/// Pure & allocation-free — fully unit-testable without network access.
pub fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_blocked_ipv4(v4),
        std::net::IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

/// Stateless DNS resolver that drops any address `is_blocked_ip` flags as non-public.
/// Reqwest's connector connects ONLY to the `SocketAddr`s this returns, so a public-looking
/// hostname that resolves (at request time, via DNS rebinding / internal DNS / CNAME) to a
/// private/loopback/metadata IP cannot be reached. Fails closed.
#[derive(Clone, Debug)]
pub struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // `tokio::net::lookup_host` requires a host:port pair; port 0 is a placeholder —
            // reqwest's connector supplies the real per-request port when dialing.
            let resolved: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0u16))
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("SSRF resolver: DNS lookup failed for '{host}': {e}").into()
                    })?
                    .collect();

            let survivors: Vec<std::net::SocketAddr> =
                resolved.into_iter().filter(|sa| !is_blocked_ip(sa.ip())).collect();

            if survivors.is_empty() {
                // Fail closed: an attacker-controlled DNS record that resolves only to
                // private/metadata IPs must NOT produce a connection.
                return Err(format!(
                    "SSRF resolver: every resolved IP for '{host}' is blocked \
                     (private/loopback/link-local/metadata/CGNAT/reserved/ULA)"
                )
                .into());
            }

            Ok(Box::new(survivors.into_iter())
                as Box<dyn Iterator<Item = std::net::SocketAddr> + Send>)
        })
    }
}

/// The real loopback chat-completions model. Holds the validated+normalized base
/// URL, the model id, a rustls reqwest client, and the plan-first prompt bias.
pub struct OmlxModel {
    base_url: String,
    model: String,
    client: reqwest::Client,
    /// 3b — the operator's "Plan first" launch bias (from `DEVBOULE_PLAN_FIRST`,
    /// resolved in `config.rs`). Threaded into `build_system_prompt` so the model's
    /// standing prompt gains the PLAN-FIRST directive when set. `false` keeps the
    /// prompt byte-identical to the pre-3b default.
    plan_first: bool,
    /// B.3 — the connected user MCP servers + their tools, appended to the system
    /// prompt as the external-tools catalog. EMPTY (the default) keeps the prompt
    /// byte-identical to the pre-B default. Set via [`OmlxModel::with_user_mcp_tools`]
    /// after the executor connects (the tool list is fetched at connect time).
    /// `Arc<[…]>` so a `Clone` of the model is cheap and the slice is shared.
    user_mcp: std::sync::Arc<[UserMcpServerTools]>,
    /// The host-rendered LANGUAGE persona block (from `DEVBOULE_LANG_SKILL`, resolved in
    /// `config.rs`) — already fenced + sentinel-neutralized by the trusted app. Appended to the
    /// system prompt after the user-MCP catalog. `None` keeps the prompt byte-identical.
    lang_skill: Option<String>,
}

impl OmlxModel {
    /// Build from a base URL + model id + the plan-first bias. The base URL is
    /// validated to be a loopback http endpoint (privacy); an invalid endpoint is an
    /// error so a misconfiguration never silently routes the prompt off-machine.
    pub fn new(base_url: &str, model: impl Into<String>, plan_first: bool) -> Result<Self, String> {
        let base_url = validate_omlx_base_url(base_url)?;
        let model = model.into();
        if model.trim().is_empty() {
            return Err("oMLX model id must not be empty.".into());
        }
        let client = reqwest::Client::builder()
            // Bound a stalled oMLX call: without a timeout the `.await` in
            // `run_completion` can hang forever and the burst's wall-clock check
            // (top of the loop) never runs again. 60s comfortably covers a slow
            // local generation while still capping a wedged server.
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            base_url,
            model,
            client,
            plan_first,
            user_mcp: std::sync::Arc::from(Vec::new()),
            lang_skill: None,
        })
    }

    /// Attach the connected user MCP servers' tool catalog (B.3) for the system prompt.
    /// Builder-style so the default (no user servers) construction stays unchanged and
    /// byte-identical. Called from `config.rs` after the `MultiMcpBackend` connects.
    pub fn with_user_mcp_tools(mut self, user_mcp: Vec<UserMcpServerTools>) -> Self {
        self.user_mcp = std::sync::Arc::from(user_mcp);
        self
    }

    /// Attach the host-rendered LANGUAGE persona block (DEVBOULE_LANG_SKILL). Builder-style; the
    /// default (`None`) construction stays byte-identical.
    pub fn with_lang_skill(mut self, lang_skill: Option<String>) -> Self {
        self.lang_skill = lang_skill;
        self
    }

    /// The chat-completions endpoint URL.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// PURE request-body builder: render the system prompt + the transcript into
    /// OpenAI-compatible `messages` and a small request envelope. Separated from
    /// the live POST so the request shape is unit-testable without inference.
    pub fn build_request_body(&self, transcript: &Transcript) -> Value {
        build_chat_body(
            &self.model,
            transcript,
            self.plan_first,
            &self.user_mcp,
            self.lang_skill.as_deref(),
        )
    }
}

/// PURE, model-agnostic chat-completions body builder shared by the loopback
/// [`OmlxModel`] and the [`CloudModel`]: the request SHAPE (model id + rendered
/// messages + the sampling envelope) is identical across backends — only the
/// transport (loopback http vs. https + Authorization header) differs. Centralizing
/// it here guarantees the cloud path sends EXACTLY the body the loopback path builds,
/// so a body change can never drift between the two backends.
fn build_chat_body(
    model: &str,
    transcript: &Transcript,
    plan_first: bool,
    user_mcp: &[UserMcpServerTools],
    lang_skill: Option<&str>,
) -> Value {
    json!({
        "model": model,
        "messages": build_messages(transcript, plan_first, user_mcp, lang_skill),
        "max_tokens": MAX_TOKENS,
        "temperature": 0.2,
        "stream": false,
    })
}

/// Render a [`Transcript`] into OpenAI chat `messages`: the system prompt, the
/// human message, then each prior action (as an assistant turn carrying its
/// emitted action block) and its tool result / format feedback (as a user turn).
/// The model thus sees the full local burst context the way it produced it.
fn build_messages(
    transcript: &Transcript,
    plan_first: bool,
    user_mcp: &[UserMcpServerTools],
    lang_skill: Option<&str>,
) -> Value {
    // The system prompt + the human message are NON-evictable framing: the model
    // must always see who it is and what was asked. `plan_first` (3b) appends the
    // PLAN-FIRST directive to the standing prompt when the operator requested it;
    // `user_mcp` (B.3) appends the external user-MCP tool catalog when non-empty.
    // System prompt + the Level-1 SKILLS catalog: discover installed library skills from the project
    // root (env DEVBOULE_PROJECT_ROOT, else cwd — same source as the FS backend) and append the
    // catalog so the model knows what it can `load_skill`. Role-skill dirs are excluded (injected
    // directly). No skills ⇒ nothing appended (byte-identical). Discovered per-turn (a few stats).
    // Project-context (AGENTS.md/CLAUDE.md): host-rendered, ALREADY fenced + sentinel-neutralized,
    // passed via DEVBOULE_PROJECT_CONTEXT. It is the FIXED prefix, so it goes BEFORE the (mobile) lang
    // skill — threaded into build_system_prompt_with_lang, not appended after (which would put it
    // behind the lang persona + break the cache prefix). Absent/empty ⇒ nothing added.
    let project_context = std::env::var("DEVBOULE_PROJECT_CONTEXT").ok();
    let project_context_ref = project_context.as_deref().filter(|c| !c.trim().is_empty());
    let mut system_prompt =
        build_system_prompt_with_lang(plan_first, user_mcp, project_context_ref, lang_skill);
    {
        let root = std::env::var("DEVBOULE_PROJECT_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let roots = vec![root.join(".claude").join("skills")];
        let skills = crate::skills::discover_library_skills(
            &roots,
            &["mini", "coder", "design", "orchestrator"],
        );
        if let Some(catalog) = crate::skills::skills_catalog_block(&skills) {
            system_prompt.push_str(&catalog);
        }
    }
    let mut messages = vec![
        json!({ "role": "system", "content": system_prompt }),
        json!({ "role": "user", "content": transcript.human_message() }),
    ];

    // Render each entry to a (role, content) message and roll off the OLDEST once
    // the cumulative history would exceed MAX_TRANSCRIPT_CHARS — keeping the most
    // recent rounds. We measure by `content` chars (the dominant cost) and keep
    // entries from the BACK (newest) until the budget is spent, then restore
    // chronological order.
    let rendered: Vec<(&'static str, String)> = transcript
        .entries()
        .iter()
        .map(|entry| match entry {
            // Re-render the action as the block the model emitted, so the
            // assistant-side history is faithful to its own prior output.
            TranscriptEntry::Action(action) => ("assistant", render_action_block(action)),
            TranscriptEntry::Result(result) => {
                let tag = if result.ok {
                    "TOOL RESULT"
                } else {
                    "TOOL ERROR"
                };
                ("user", format!("{tag}:\n{}", result.output))
            }
            TranscriptEntry::FormatFeedback(feedback) => ("user", feedback.clone()),
            // A live app steer, rendered as a plain user turn so the model treats it
            // like any other human instruction on its next call.
            TranscriptEntry::Human(body) => ("user", body.clone()),
        })
        .collect();

    // Walk newest-first, accumulating until the next entry would blow the budget.
    let mut kept_rev: Vec<&(&'static str, String)> = Vec::new();
    let mut used = 0usize;
    for item in rendered.iter().rev() {
        let cost = item.1.chars().count();
        if !kept_rev.is_empty() && used + cost > MAX_TRANSCRIPT_CHARS {
            break; // older than this is evicted; keep at least the newest entry
        }
        used += cost;
        kept_rev.push(item);
    }
    kept_rev.reverse();

    // After trimming, the kept window may START on a tool result/feedback whose
    // preceding action was evicted — a stranded result confuses the model. Drop
    // such leading orphans so the window opens on an assistant action (or is
    // empty). The newest round is always a full pair (action then result), so the
    // tail is never affected.
    let first_action = kept_rev.iter().position(|(role, _)| *role == "assistant");
    let window: &[&(&'static str, String)] = match first_action {
        Some(idx) => &kept_rev[idx..],
        None => &[],
    };

    for (role, content) in window {
        messages.push(json!({ "role": role, "content": content }));
    }
    Value::Array(messages)
}

/// Re-serialize a parsed action back into its fenced `action` block. Used to
/// reconstruct the assistant-side transcript for the model.
fn render_action_block(action: &crate::action::AgentAction) -> String {
    // The action implements Serialize-equivalent shape via tool_name + target is
    // lossy, so build the JSON object explicitly per variant to stay faithful.
    use crate::action::AgentAction as A;
    let body = match action {
        A::OracleAsk { query } => json!({"tool":"oracle_ask","query":query}),
        A::OracleContext { query, limit } => match limit {
            Some(l) => json!({"tool":"oracle_context","query":query,"limit":l}),
            None => json!({"tool":"oracle_context","query":query}),
        },
        A::Plan { steps } => json!({"tool":"plan","steps":steps}),
        A::RunPlan {} => json!({"tool":"run_plan"}),
        A::SpawnMini { task, files, write } => {
            json!({"tool":"spawn_mini","task":task,"files":files,"write":write})
        }
        A::Read { path } => json!({"tool":"read","path":path}),
        A::Grep { pattern, glob } => match glob {
            Some(g) => json!({"tool":"grep","pattern":pattern,"glob":g}),
            None => json!({"tool":"grep","pattern":pattern}),
        },
        A::Glob { pattern } => json!({"tool":"glob","pattern":pattern}),
        A::LoadSkill { name } => json!({"tool":"load_skill","name":name}),
        A::Fetch { url } => json!({"tool":"fetch","url":url}),
        A::Websearch { query } => json!({"tool":"websearch","query":query}),
        A::McpTool {
            server,
            tool,
            params,
        } => json!({"tool":"mcp_tool","server":server,"name":tool,"params":params}),
        A::AskUser { question } => json!({"tool":"ask_user","question":question}),
        A::Done { reply } => json!({"tool":"done","reply":reply}),
        A::Escalate { reason } => json!({"tool":"escalate","reason":reason}),
    };
    format!("```action\n{body}\n```")
}

/// Extract the assistant message text from an OpenAI-compatible chat-completions
/// response body. PURE + total so it is unit-testable against a fixture.
pub fn parse_chat_response(body: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("bad response JSON: {e}"))?;
    value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "response missing choices[0].message.content".to_string())
}

#[async_trait]
impl CoderModel for OmlxModel {
    /// L2.1 streaming path: unused by the burst loop (the loop drives
    /// `next_output`). A faithful single-shot: run one completion and send the
    /// whole text as one chunk. Kept so the trait stays fully usable.
    async fn reply(&self, prompt: String, tx: mpsc::Sender<String>) {
        let t = Transcript::new(prompt);
        let text = match self.run_completion(&t).await {
            Ok(s) => s,
            Err(e) => format!("[model error: {e}]"),
        };
        let _ = tx.send(text).await;
    }

    async fn next_output(&self, transcript: &Transcript) -> String {
        match self.run_completion(transcript).await {
            Ok(text) => text,
            // A transport/parse failure becomes a non-action string; the burst
            // loop's parser turns it into a FORMAT ERROR fed back to the model,
            // and three in a row escalate cleanly. We never panic on I/O.
            Err(e) => format!("[model request failed: {e}]"),
        }
    }
}

impl OmlxModel {
    /// POST one chat-completions request and return the assistant text.
    async fn run_completion(&self, transcript: &Transcript) -> Result<String, String> {
        let body = self.build_request_body(transcript);
        let resp = self
            .client
            .post(self.endpoint())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        // Bound the body read: a runaway/buggy server returning a huge body must
        // not be buffered whole into RAM. The shared cap streams at most a few
        // result-lengths then stops (see [`crate::executor::read_body_capped`]).
        let text = crate::executor::read_body_capped(resp, crate::executor::HTTP_BODY_CAP).await?;
        if !status.is_success() {
            return Err(format!("HTTP {}", status.as_u16()));
        }
        parse_chat_response(&text)
    }
}

/// The CLOUD chat-completions model: the OPT-IN, consent-gated counterpart to
/// [`OmlxModel`]. It talks to an HTTPS OpenAI-compatible endpoint (e.g. OpenRouter)
/// with an `Authorization: Bearer <key>` header, sending the SAME request body the
/// loopback client builds (via the shared [`build_chat_body`]) and parsing the SAME
/// response shape (via [`parse_chat_response`]). Only the transport differs:
/// https + TLS + the bearer header instead of plain loopback http.
///
/// PRIVACY: unlike the loopback path, this DOES send the prompt (which can carry file
/// content) off the machine to the configured provider. It is constructed ONLY when
/// the operator explicitly configured Cloud mode (the host UI shows a mandatory
/// disclosure before this is reachable). The API key is held in memory, set ONLY from
/// the environment, and NEVER logged: the [`std::fmt::Debug`] impl redacts it, and no
/// error string ever embeds it.
pub struct CloudModel {
    base_url: String,
    model: String,
    /// The bearer credential. Set ONLY from `DEVBOULE_CLOUD_API_KEY` (env). Never
    /// logged, never serialized, redacted in Debug.
    api_key: String,
    client: reqwest::Client,
    plan_first: bool,
    /// B.3 — the connected user MCP servers + their tools (see [`OmlxModel`] field).
    /// EMPTY (default) keeps the prompt byte-identical; set via
    /// [`CloudModel::with_user_mcp_tools`].
    user_mcp: std::sync::Arc<[UserMcpServerTools]>,
    /// The host-rendered LANGUAGE persona block (from `DEVBOULE_LANG_SKILL`) — see the
    /// [`OmlxModel`] field. `None` keeps the prompt byte-identical.
    lang_skill: Option<String>,
}

impl std::fmt::Debug for CloudModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the key so a `{:?}` of the model (or anything holding it) can never
        // print the credential to a log/diagnostic.
        f.debug_struct("CloudModel")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .field("plan_first", &self.plan_first)
            .finish()
    }
}

impl CloudModel {
    /// Build from an HTTPS base URL + model id + bearer key + the plan-first bias.
    /// The base URL is validated as an https NON-loopback host (the cloud invariant);
    /// the key is validated non-empty (a blank key would produce silent 401s). An
    /// invalid input is an error so a misconfiguration fails LOUD rather than silently
    /// sending an unauthenticated request off-machine.
    pub fn new(
        base_url: &str,
        model: impl Into<String>,
        api_key: impl Into<String>,
        plan_first: bool,
    ) -> Result<Self, String> {
        let base_url = validate_cloud_base_url(base_url)?;
        let model = model.into();
        if model.trim().is_empty() {
            return Err("Cloud model id must not be empty.".into());
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err("Cloud API key must not be empty.".into());
        }
        let client = reqwest::Client::builder()
            // Same bound as the loopback client so a stalled cloud call cannot wedge
            // the burst's wall-clock check. A cloud round-trip can be slower than
            // loopback, but 60s still caps a hung connection.
            .timeout(std::time::Duration::from_secs(60))
            .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            base_url,
            model,
            api_key,
            client,
            plan_first,
            user_mcp: std::sync::Arc::from(Vec::new()),
            lang_skill: None,
        })
    }

    /// Attach the connected user MCP servers' tool catalog (B.3) for the system prompt.
    /// Builder-style; the default (no user servers) construction stays byte-identical.
    pub fn with_user_mcp_tools(mut self, user_mcp: Vec<UserMcpServerTools>) -> Self {
        self.user_mcp = std::sync::Arc::from(user_mcp);
        self
    }

    /// Attach the host-rendered LANGUAGE persona block (DEVBOULE_LANG_SKILL). Builder-style; the
    /// default (`None`) construction stays byte-identical.
    pub fn with_lang_skill(mut self, lang_skill: Option<String>) -> Self {
        self.lang_skill = lang_skill;
        self
    }

    /// The chat-completions endpoint URL.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// PURE request-body builder: identical shape to the loopback client's, via the
    /// shared [`build_chat_body`]. Unit-testable without inference or a network.
    pub fn build_request_body(&self, transcript: &Transcript) -> Value {
        build_chat_body(
            &self.model,
            transcript,
            self.plan_first,
            &self.user_mcp,
            self.lang_skill.as_deref(),
        )
    }

    /// POST one chat-completions request (https + bearer) and return the assistant
    /// text. The bearer header carries the key; it is NEVER put on the URL or logged.
    async fn run_completion(&self, transcript: &Transcript) -> Result<String, String> {
        let body = self.build_request_body(transcript);
        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        let text =
            crate::executor::read_body_capped(resp, crate::executor::HTTP_BODY_CAP).await?;
        if !status.is_success() {
            // Status code only — never the body, which a provider may echo the prompt
            // or key fragment into.
            return Err(format!("HTTP {}", status.as_u16()));
        }
        parse_chat_response(&text)
    }
}

#[async_trait]
impl CoderModel for CloudModel {
    async fn reply(&self, prompt: String, tx: mpsc::Sender<String>) {
        let t = Transcript::new(prompt);
        let text = match self.run_completion(&t).await {
            Ok(s) => s,
            Err(e) => format!("[model error: {e}]"),
        };
        let _ = tx.send(text).await;
    }

    async fn next_output(&self, transcript: &Transcript) -> String {
        match self.run_completion(transcript).await {
            Ok(text) => text,
            Err(e) => format!("[model request failed: {e}]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- loopback validator ---------------------------------------------------

    #[test]
    fn validator_accepts_loopback_http_with_port() {
        for ok in [
            "http://127.0.0.1:8000/v1",
            "http://127.0.0.1",
            "http://localhost:11434",
            "http://localhost",
            "http://[::1]:8080",
            "http://127.0.0.1:8000/v1/", // trailing slash normalized off
        ] {
            assert!(validate_omlx_base_url(ok).is_ok(), "should accept {ok}");
        }
        // Trailing slash is stripped.
        assert_eq!(
            validate_omlx_base_url("http://127.0.0.1:8000/v1/").unwrap(),
            "http://127.0.0.1:8000/v1"
        );
    }

    #[test]
    fn validator_rejects_non_loopback_host() {
        // The task's explicit reject case: a non-loopback IP must fail.
        assert!(validate_omlx_base_url("http://1.2.3.4").is_err());
        assert!(validate_omlx_base_url("http://192.168.0.1:8000/v1").is_err());
        assert!(validate_omlx_base_url("http://evil.com/v1").is_err());
    }

    #[test]
    fn validator_rejects_https_even_on_loopback() {
        // The task's explicit reject case: https must fail.
        assert!(validate_omlx_base_url("https://1.2.3.4").is_err());
        assert!(validate_omlx_base_url("https://127.0.0.1:8000").is_err());
    }

    #[test]
    fn validator_accepts_the_task_loopback_example() {
        // The task's explicit accept case: http://127.0.0.1:port.
        assert!(validate_omlx_base_url("http://127.0.0.1:8000").is_ok());
    }

    #[test]
    fn validator_rejects_userinfo_trick() {
        assert!(validate_omlx_base_url("http://127.0.0.1@evil.com").is_err());
        assert!(validate_omlx_base_url("http://[::1]:8000@evil.com").is_err());
    }

    #[test]
    fn validator_rejects_bad_port() {
        assert!(validate_omlx_base_url("http://127.0.0.1:").is_err());
        assert!(validate_omlx_base_url("http://127.0.0.1:99999").is_err());
        assert!(validate_omlx_base_url("http://127.0.0.1:abc").is_err());
    }

    #[test]
    fn validator_rejects_empty_and_overlong() {
        assert!(validate_omlx_base_url("   ").is_err());
        let long = format!(
            "http://127.0.0.1:8000/{}",
            "a".repeat(OMLX_BASE_URL_MAX_LEN)
        );
        assert!(validate_omlx_base_url(&long).is_err());
    }

    // --- request building -----------------------------------------------------

    #[test]
    fn build_request_body_has_system_prompt_and_human_message() {
        let model = OmlxModel::new("http://127.0.0.1:8000/v1", "test-model", false).unwrap();
        let transcript = Transcript::new("do the thing".to_string());
        let body = model.build_request_body(&transcript);

        assert_eq!(body["model"], json!("test-model"));
        assert_eq!(body["stream"], json!(false));
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages[0]["role"], json!("system"));
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("oracle_ask"),
            "system prompt is the orchestrator prompt"
        );
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"], json!("do the thing"));
    }

    #[test]
    fn empty_burst_is_system_plus_human_only() {
        // An opening burst (no actions run yet) renders exactly the system prompt
        // and the human message. The action/result turn rendering is covered by
        // `render_action_block_round_trips_through_the_parser` (the entries() push
        // surface is private to the loop, so the per-entry mapping is exercised
        // via the block renderer it delegates to).
        let model = OmlxModel::new("http://127.0.0.1:8000", "m", false).unwrap();
        let body = model.build_request_body(&Transcript::new("find it".to_string()));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "system + human only for an empty burst");
        assert_eq!(messages[1]["content"], json!("find it"));
    }

    #[test]
    fn over_cap_transcript_is_trimmed_oldest_first_keeping_human_and_latest() {
        // FIX 6: an over-cap transcript must roll off the OLDEST action+result
        // pairs, ALWAYS keeping the system framing + the original human message +
        // the most-recent round. Build many large rounds; the first must be
        // evicted, the last must survive, and the rendered history must be bounded.
        use crate::agent_loop::{ToolResult, TranscriptEntry};

        let model = OmlxModel::new("http://127.0.0.1:8000", "m", false).unwrap();
        let mut transcript = Transcript::new("ORIGINAL HUMAN TASK".to_string());

        // Each round: a read action + a ~16 KB result (the per-result cap). With
        // MAX_TRANSCRIPT_CHARS = 100_000 only a handful of rounds fit, so the
        // earliest are evicted.
        let big = "Z".repeat(16_000);
        let rounds = 20;
        for i in 0..rounds {
            transcript.push_entry_for_test(TranscriptEntry::Action(
                crate::action::AgentAction::Read {
                    path: format!("file_{i}.rs"),
                },
            ));
            transcript.push_entry_for_test(TranscriptEntry::Result(ToolResult::ok(format!(
                "ROUND{i}_RESULT {big}"
            ))));
        }

        let body = model.build_request_body(&transcript);
        let messages = body["messages"].as_array().expect("messages array");

        // System + human are always present and first.
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[1]["role"], json!("user"));
        assert_eq!(messages[1]["content"], json!("ORIGINAL HUMAN TASK"));

        // The full thing concatenated, to assert presence/absence of rounds.
        let joined: String = messages
            .iter()
            .map(|m| m["content"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        // The latest round survives; the oldest is evicted.
        assert!(
            joined.contains(&format!("ROUND{}_RESULT", rounds - 1)),
            "the most-recent round must be kept"
        );
        assert!(
            joined.contains(&format!("file_{}.rs", rounds - 1)),
            "the most-recent action must be kept"
        );
        assert!(
            !joined.contains("ROUND0_RESULT"),
            "the oldest round must be evicted"
        );

        // The HUMAN message is never evicted.
        assert!(joined.contains("ORIGINAL HUMAN TASK"), "human message kept");

        // The rendered ACTION/RESULT history (everything after system+human) is
        // bounded by the aggregate cap (allowing one over-cap newest entry).
        let history_chars: usize = messages[2..]
            .iter()
            .map(|m| m["content"].as_str().unwrap_or("").chars().count())
            .sum();
        assert!(
            history_chars <= MAX_TRANSCRIPT_CHARS + 16_064,
            "history is bounded near the cap, got {history_chars} chars"
        );

        // The kept window opens on an assistant action, never a stranded result.
        if let Some(first_history) = messages.get(2) {
            assert_eq!(
                first_history["role"],
                json!("assistant"),
                "kept window starts on an action, not an orphaned result"
            );
        }
    }

    #[test]
    fn render_action_block_round_trips_through_the_parser() {
        use crate::action::{parse_action, AgentAction};
        let cases = vec![
            AgentAction::OracleAsk { query: "q".into() },
            AgentAction::OracleContext {
                query: "q".into(),
                limit: Some(4),
            },
            AgentAction::OracleContext {
                query: "q".into(),
                limit: None,
            },
            AgentAction::RunPlan {},
            AgentAction::SpawnMini {
                task: "t".into(),
                files: vec!["a.rs".into()],
                write: true,
            },
            AgentAction::Read {
                path: "src/a.rs".into(),
            },
            AgentAction::Grep {
                pattern: "TODO".into(),
                glob: Some("*.rs".into()),
            },
            AgentAction::Done { reply: "ok".into() },
        ];
        for a in cases {
            let block = render_action_block(&a);
            let parsed = parse_action(&block).expect("rendered block re-parses");
            assert_eq!(parsed, a, "round-trip mismatch for {a:?}");
        }
    }

    #[test]
    fn render_mcp_tool_round_trips_through_the_parser() {
        // The mcp_tool wire form uses `name` (not `tool`) for the tool-to-call (the enum
        // is tagged on `tool`). Assert render→parse symmetry, validating against a known
        // server so the unknown-server gate does not reject it.
        use crate::action::{parse_action_with_servers, AgentAction};
        let a = AgentAction::McpTool {
            server: "my-db".into(),
            tool: "query".into(),
            params: serde_json::json!({"sql": "SELECT 1", "limit": 5}),
        };
        let block = render_action_block(&a);
        // The rendered block carries the `name` wire key, not a second `tool` key.
        assert!(block.contains("\"name\":\"query\""), "uses the `name` wire key: {block}");
        let servers = vec!["my-db".to_string()];
        let parsed =
            parse_action_with_servers(&block, &servers).expect("rendered mcp_tool re-parses");
        assert_eq!(parsed, a, "mcp_tool round-trip mismatch");
    }

    // --- response parsing -----------------------------------------------------

    #[test]
    fn parse_chat_response_extracts_content() {
        let fixture = r#"{
            "choices": [
                {"message": {"role": "assistant", "content": "```action\n{\"tool\":\"done\",\"reply\":\"hi\"}\n```"}}
            ]
        }"#;
        let text = parse_chat_response(fixture).unwrap();
        assert!(text.contains("\"tool\":\"done\""), "got: {text}");
    }

    #[test]
    fn parse_chat_response_errors_on_missing_content() {
        assert!(parse_chat_response(r#"{"choices": []}"#).is_err());
        assert!(parse_chat_response("not json").is_err());
    }

    #[test]
    fn new_rejects_invalid_endpoint() {
        assert!(OmlxModel::new("https://1.2.3.4", "m", false).is_err());
        assert!(OmlxModel::new("http://127.0.0.1:8000", "", false).is_err());
    }

    // --- cloud base-url validator ---------------------------------------------

    #[test]
    fn cloud_validator_accepts_https_public_host() {
        for ok in [
            "https://openrouter.ai/api/v1",
            "https://api.openai.com/v1",
            "https://api.together.xyz/v1",
            "https://gateway.example.com:8443/v1",
            "https://openrouter.ai/api/v1/", // trailing slash normalized off
        ] {
            assert!(validate_cloud_base_url(ok).is_ok(), "should accept {ok}");
        }
        assert_eq!(
            validate_cloud_base_url("https://openrouter.ai/api/v1/").unwrap(),
            "https://openrouter.ai/api/v1"
        );
    }

    #[test]
    fn cloud_validator_rejects_non_https() {
        // http (clear text off-machine) and other schemes must fail.
        assert!(validate_cloud_base_url("http://openrouter.ai/api/v1").is_err());
        assert!(validate_cloud_base_url("ftp://openrouter.ai").is_err());
        assert!(validate_cloud_base_url("openrouter.ai/api/v1").is_err());
    }

    #[test]
    fn cloud_validator_rejects_loopback_as_cloud() {
        // A loopback host in CLOUD mode is a misconfiguration (Local mode is the
        // loopback path). Reject localhost, 127.0.0.0/8 and [::1].
        for bad in [
            "https://localhost/v1",
            "https://localhost:8000/v1",
            "https://127.0.0.1/v1",
            "https://127.0.0.1:8000/v1",
            "https://[::1]:8000/v1",
        ] {
            assert!(
                validate_cloud_base_url(bad).is_err(),
                "loopback {bad} must be rejected as a cloud host"
            );
        }
    }

    #[test]
    fn cloud_validator_rejects_ip_literals_and_single_label_hosts() {
        for bad in [
            "https://1.2.3.4/v1",          // bare IPv4 -> SSRF surface
            "https://1.2.3.4:8443/v1",     // bare IPv4 with port
            "https://[2001:db8::1]/v1",    // IPv6 literal
            "https://internal/v1",         // single-label intranet name
            "https://metadata/v1",         // single-label (cloud-metadata-ish)
        ] {
            assert!(
                validate_cloud_base_url(bad).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn cloud_validator_rejects_numeric_quad_ip_literals_with_leading_zeros() {
        // Rust's Ipv4Addr parser REJECTS leading-zero / out-of-range dotted-quads, so
        // without the all-numeric-4-label fallback these slip past `parse::<Ipv4Addr>()`
        // and look like a hostname. They must be rejected as IP literals.
        for bad in [
            "https://01.02.03.04/v1",
            "https://010.0.0.1/v1",
            "https://0177.0.0.1/v1",
            "https://999.999.999.999/v1",
            "https://01.02.03.04:8443/v1",
        ] {
            assert!(
                validate_cloud_base_url(bad).is_err(),
                "numeric-quad IP literal {bad} must be rejected"
            );
        }
        // A real public hostname is still accepted (the fallback must not over-reject).
        assert!(validate_cloud_base_url("https://api.openai.com/v1").is_ok());
        assert!(validate_cloud_base_url("https://openrouter.ai/api/v1").is_ok());
    }

    #[test]
    fn cloud_validator_rejects_metadata_and_intranet_suffix_hosts() {
        // PARTIAL SSRF mitigation: the cloud-metadata FQDN + `.internal` / `.local`
        // suffixes are denied even though they are FQDN-shaped.
        for bad in [
            "https://metadata.google.internal/computeMetadata/v1",
            "https://foo.internal/v1",
            "https://bar.local/v1",
            "https://METADATA.GOOGLE.INTERNAL/v1", // case-insensitive
        ] {
            assert!(
                validate_cloud_base_url(bad).is_err(),
                "intranet/metadata host {bad} must be rejected"
            );
        }
        // Real public hosts still pass.
        assert!(validate_cloud_base_url("https://api.openai.com/v1").is_ok());
        assert!(validate_cloud_base_url("https://openrouter.ai/api/v1").is_ok());
    }

    #[test]
    fn cloud_validator_rejects_userinfo_and_bad_chars() {
        assert!(validate_cloud_base_url("https://user:pass@openrouter.ai/v1").is_err());
        assert!(validate_cloud_base_url("https://openrouter.ai@evil.com/v1").is_err());
        assert!(validate_cloud_base_url("https://open\u{202e}router.ai/v1").is_err());
        assert!(validate_cloud_base_url("   ").is_err());
        let long = format!("https://openrouter.ai/{}", "a".repeat(OMLX_BASE_URL_MAX_LEN));
        assert!(validate_cloud_base_url(&long).is_err());
    }

    #[test]
    fn cloud_validator_rejects_bad_port() {
        assert!(validate_cloud_base_url("https://openrouter.ai:/v1").is_err());
        assert!(validate_cloud_base_url("https://openrouter.ai:99999/v1").is_err());
        assert!(validate_cloud_base_url("https://openrouter.ai:abc/v1").is_err());
    }

    // --- cloud model -----------------------------------------------------------

    #[test]
    fn cloud_new_rejects_bad_inputs() {
        // Loopback / http base URL, empty model, empty key all fail loud.
        assert!(CloudModel::new("http://openrouter.ai/v1", "m", "k", false).is_err());
        assert!(CloudModel::new("https://127.0.0.1/v1", "m", "k", false).is_err());
        assert!(CloudModel::new("https://openrouter.ai/v1", "", "k", false).is_err());
        assert!(CloudModel::new("https://openrouter.ai/v1", "m", "  ", false).is_err());
        assert!(CloudModel::new("https://openrouter.ai/api/v1", "m", "k", false).is_ok());
    }

    #[test]
    fn cloud_body_is_byte_identical_to_loopback_body() {
        // The whole point of the shared builder: a cloud request body matches the
        // loopback one for the same model + transcript + plan_first. Only the
        // transport (https + bearer) differs, NOT the body.
        let omlx = OmlxModel::new("http://127.0.0.1:8000/v1", "shared-model", true).unwrap();
        let cloud =
            CloudModel::new("https://openrouter.ai/api/v1", "shared-model", "sk-test", true)
                .unwrap();
        let transcript = Transcript::new("do the thing".to_string());
        assert_eq!(
            omlx.build_request_body(&transcript),
            cloud.build_request_body(&transcript),
            "cloud body must equal the loopback body for the same inputs"
        );
    }

    #[test]
    fn cloud_debug_redacts_the_api_key() {
        // A `{:?}` of the model must never print the credential.
        let cloud =
            CloudModel::new("https://openrouter.ai/api/v1", "m", "sk-super-secret", false)
                .unwrap();
        let dbg = format!("{cloud:?}");
        assert!(dbg.contains("<redacted>"), "key must be redacted: {dbg}");
        assert!(
            !dbg.contains("sk-super-secret"),
            "the raw key must never appear in Debug: {dbg}"
        );
    }

    // -------------------------------------------------------------------------
    // Live integration tests — require network + credentials.
    // Run manually:
    //   OPENROUTER_KEY=$(cat ~/.openrouter_key) \
    //     cargo test -p devboule-coder --ignored live_ -- --nocapture
    // -------------------------------------------------------------------------

    /// Verify that `PublicOnlyResolver` allows a real public host (openrouter.ai)
    /// while blocking a public DNS name that resolves to 127.0.0.1 (nip.io trick).
    ///
    /// The client is built EXACTLY as `CloudModel::new` builds it:
    ///   timeout(15s) + dns_resolver(PublicOnlyResolver)
    ///
    /// Fallback for the private-hostname check: if `127.0.0.1.nip.io` is unavailable,
    /// `127.0.0.1.localtest.me` also resolves to 127.0.0.1 and exercises the same path.
    #[tokio::test]
    #[ignore]
    async fn live_ssrf_resolver_allows_public_blocks_private() {
        use std::sync::Arc;
        use std::time::Duration;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .dns_resolver(Arc::new(PublicOnlyResolver))
            .build()
            .expect("failed to build SSRF-guarded client");

        // A real public endpoint must resolve and connect through the guard.
        // `.send()` is Ok on any HTTP status; Err only on connect / DNS failure.
        let pub_res = client.get("https://openrouter.ai/").send().await;
        assert!(
            pub_res.is_ok(),
            "PublicOnlyResolver must allow openrouter.ai (a real public host): {:?}",
            pub_res.err()
        );

        // `127.0.0.1.nip.io` is a public FQDN whose DNS answer is 127.0.0.1.
        // The resolver must block every resolved IP (all loopback) and return Err.
        // Fallback: `127.0.0.1.localtest.me` has the same property if nip.io is down.
        let priv_res = client.get("https://127.0.0.1.nip.io/").send().await;
        eprintln!("blocked-host error: {:?}", priv_res.as_ref().err());
        assert!(
            priv_res.is_err(),
            "PublicOnlyResolver must block 127.0.0.1.nip.io \
             (FQDN that resolves to a loopback address)"
        );
    }

    /// End-to-end cloud-mode completion via OpenRouter.
    ///
    /// Requires `OPENROUTER_KEY` to be set in the environment.
    /// Uses the cheapest available model (`z-ai/glm-5.2`) with a trivial prompt
    /// to minimise token cost while still exercising the full transport + parsing
    /// path: `CloudModel::new` → SSRF resolver + TLS → `/chat/completions` →
    /// `parse_chat_response` → non-empty string.
    #[tokio::test]
    #[ignore]
    async fn live_cloud_model_completion_via_openrouter() {
        use crate::model::CoderModel;

        let key = std::env::var("OPENROUTER_KEY")
            .expect("set OPENROUTER_KEY to run this live test");

        let model = CloudModel::new(
            "https://openrouter.ai/api/v1",
            "z-ai/glm-5.2",
            key,
            false,
        )
        .expect("CloudModel::new must accept a valid https FQDN + non-empty key");

        let transcript = Transcript::new(
            "Reply with exactly the word: ok".to_string(),
        );

        let completion = model.next_output(&transcript).await;
        eprintln!("GLM said: {completion}");
        assert!(
            !completion.is_empty(),
            "completion must be non-empty; got an empty string \
             (possible auth / network failure — check OPENROUTER_KEY)"
        );
    }
}

#[cfg(test)]
mod ssrf_classifier_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }
    fn v6(a: u16, b: u16, c: u16, d: u16, e: u16, f: u16, g: u16, h: u16) -> IpAddr {
        IpAddr::V6(Ipv6Addr::new(a, b, c, d, e, f, g, h))
    }
    /// ::ffff:a.b.c.d — segments: [0,0,0,0,0,0xffff, (a<<8)|b, (c<<8)|d]
    fn mapped(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V6(Ipv6Addr::new(
            0,
            0,
            0,
            0,
            0,
            0xffff,
            ((a as u16) << 8) | b as u16,
            ((c as u16) << 8) | d as u16,
        ))
    }

    // --- Must be blocked: required by spec ---
    #[test]
    fn blocks_127_0_0_1() {
        assert!(is_blocked_ip(v4(127, 0, 0, 1)));
    }
    #[test]
    fn blocks_loopback_v6() {
        assert!(is_blocked_ip(v6(0, 0, 0, 0, 0, 0, 0, 1))); // ::1
    }
    #[test]
    fn blocks_10_0_0_1() {
        assert!(is_blocked_ip(v4(10, 0, 0, 1)));
    }
    #[test]
    fn blocks_172_16_0_1() {
        assert!(is_blocked_ip(v4(172, 16, 0, 1)));
    }
    #[test]
    fn blocks_172_31_255_255() {
        assert!(is_blocked_ip(v4(172, 31, 255, 255)));
    }
    #[test]
    fn blocks_192_168_1_1() {
        assert!(is_blocked_ip(v4(192, 168, 1, 1)));
    }
    #[test]
    fn blocks_metadata_169_254_169_254() {
        assert!(is_blocked_ip(v4(169, 254, 169, 254)));
    }
    #[test]
    fn blocks_0_0_0_0() {
        assert!(is_blocked_ip(v4(0, 0, 0, 0)));
    }
    #[test]
    fn blocks_unspecified_v6() {
        assert!(is_blocked_ip(v6(0, 0, 0, 0, 0, 0, 0, 0))); // ::
    }
    #[test]
    fn blocks_fe80_1() {
        assert!(is_blocked_ip(v6(0xfe80, 0, 0, 0, 0, 0, 0, 1)));
    }
    #[test]
    fn blocks_fc00_1() {
        assert!(is_blocked_ip(v6(0xfc00, 0, 0, 0, 0, 0, 0, 1)));
    }
    #[test]
    fn blocks_fd00_1() {
        assert!(is_blocked_ip(v6(0xfd00, 0, 0, 0, 0, 0, 0, 1)));
    }
    #[test]
    fn blocks_cgnat_100_64_0_1() {
        assert!(is_blocked_ip(v4(100, 64, 0, 1)));
    }
    #[test]
    fn blocks_mapped_127_0_0_1() {
        assert!(is_blocked_ip(mapped(127, 0, 0, 1)));
    }
    #[test]
    fn blocks_mapped_10_0_0_1() {
        assert!(is_blocked_ip(mapped(10, 0, 0, 1)));
    }
    #[test]
    fn blocks_mapped_metadata() {
        assert!(is_blocked_ip(mapped(169, 254, 169, 254)));
    }

    // --- Must NOT be blocked: required by spec ---
    #[test]
    fn allows_8_8_8_8() {
        assert!(!is_blocked_ip(v4(8, 8, 8, 8)));
    }
    #[test]
    fn allows_1_1_1_1() {
        assert!(!is_blocked_ip(v4(1, 1, 1, 1)));
    }
    #[test]
    fn allows_93_184_216_34() {
        assert!(!is_blocked_ip(v4(93, 184, 216, 34)));
    }
    #[test]
    fn allows_2606_4700_1111() {
        assert!(!is_blocked_ip(v6(0x2606, 0x4700, 0, 0, 0, 0, 0, 0x1111)));
    }

    // --- Edge cases: CGNAT (100.64.0.0/10) boundaries ---
    #[test]
    fn allows_100_63_255_255_below_cgnat() {
        assert!(!is_blocked_ip(v4(100, 63, 255, 255)));
    }
    #[test]
    fn allows_100_128_0_0_above_cgnat() {
        assert!(!is_blocked_ip(v4(100, 128, 0, 0)));
    }
    #[test]
    fn blocks_100_64_0_0_cgnat_start() {
        assert!(is_blocked_ip(v4(100, 64, 0, 0)));
    }
    #[test]
    fn blocks_100_127_255_255_cgnat_end() {
        assert!(is_blocked_ip(v4(100, 127, 255, 255)));
    }

    // --- Edge cases: RFC 1918 boundaries ---
    #[test]
    fn allows_172_15_255_255_below_private() {
        assert!(!is_blocked_ip(v4(172, 15, 255, 255)));
    }
    #[test]
    fn allows_172_32_0_0_above_private() {
        assert!(!is_blocked_ip(v4(172, 32, 0, 0)));
    }
    #[test]
    fn allows_11_0_0_1_above_10() {
        assert!(!is_blocked_ip(v4(11, 0, 0, 1)));
    }
    #[test]
    fn allows_9_255_255_255_below_10() {
        assert!(!is_blocked_ip(v4(9, 255, 255, 255)));
    }
    #[test]
    fn allows_192_167_255_255_below_private() {
        assert!(!is_blocked_ip(v4(192, 167, 255, 255)));
    }
    #[test]
    fn allows_192_169_0_0_above_private() {
        assert!(!is_blocked_ip(v4(192, 169, 0, 0)));
    }

    // --- 0.0.0.0/8 "this network" beyond just 0.0.0.0 ---
    #[test]
    fn blocks_0_0_0_1() {
        assert!(is_blocked_ip(v4(0, 0, 0, 1)));
    }
    #[test]
    fn blocks_0_255_255_255() {
        assert!(is_blocked_ip(v4(0, 255, 255, 255)));
    }

    // --- Loopback variants ---
    #[test]
    fn blocks_127_0_0_0() {
        assert!(is_blocked_ip(v4(127, 0, 0, 0)));
    }
    #[test]
    fn blocks_127_255_255_254() {
        assert!(is_blocked_ip(v4(127, 255, 255, 254)));
    }

    // --- Link-local boundaries ---
    #[test]
    fn blocks_169_254_0_0() {
        assert!(is_blocked_ip(v4(169, 254, 0, 0)));
    }
    #[test]
    fn blocks_169_254_255_255() {
        assert!(is_blocked_ip(v4(169, 254, 255, 255)));
    }
    #[test]
    fn allows_169_253_255_255_below_linklocal() {
        assert!(!is_blocked_ip(v4(169, 253, 255, 255)));
    }
    #[test]
    fn allows_169_255_255_255_above_linklocal() {
        assert!(!is_blocked_ip(v4(169, 255, 255, 255)));
    }

    // --- Multicast ---
    #[test]
    fn blocks_224_0_0_1() {
        assert!(is_blocked_ip(v4(224, 0, 0, 1)));
    }
    #[test]
    fn blocks_239_255_255_255() {
        assert!(is_blocked_ip(v4(239, 255, 255, 255)));
    }
    #[test]
    fn allows_223_255_255_255_below_multicast() {
        assert!(!is_blocked_ip(v4(223, 255, 255, 255)));
    }
    #[test]
    fn blocks_v6_multicast_ff02_1() {
        assert!(is_blocked_ip(v6(0xff02, 0, 0, 0, 0, 0, 0, 1)));
    }
    #[test]
    fn blocks_v6_multicast_ff00_0() {
        assert!(is_blocked_ip(v6(0xff00, 0, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_v6_multicast_fffe_high() {
        assert!(is_blocked_ip(v6(0xffff, 0xffff, 0, 0, 0, 0, 0, 1)));
    }

    // --- Reserved / broadcast (240.0.0.0/4) ---
    #[test]
    fn blocks_240_0_0_0() {
        assert!(is_blocked_ip(v4(240, 0, 0, 0)));
    }
    #[test]
    fn blocks_250_0_0_1() {
        assert!(is_blocked_ip(v4(250, 0, 0, 1)));
    }
    #[test]
    fn blocks_255_255_255_255_broadcast() {
        assert!(is_blocked_ip(v4(255, 255, 255, 255)));
    }

    // --- Documentation (RFC 5737) and benchmarking (RFC 2544) ---
    #[test]
    fn blocks_doc_192_0_2_x() {
        assert!(is_blocked_ip(v4(192, 0, 2, 123)));
    }
    #[test]
    fn blocks_doc_198_51_100_x() {
        assert!(is_blocked_ip(v4(198, 51, 100, 5)));
    }
    #[test]
    fn blocks_doc_203_0_113_x() {
        assert!(is_blocked_ip(v4(203, 0, 113, 99)));
    }
    #[test]
    fn blocks_benchmark_198_18_x() {
        assert!(is_blocked_ip(v4(198, 18, 0, 1)));
    }
    #[test]
    fn blocks_benchmark_198_19_x() {
        assert!(is_blocked_ip(v4(198, 19, 255, 255)));
    }
    #[test]
    fn allows_198_17_255_255_below_benchmark() {
        assert!(!is_blocked_ip(v4(198, 17, 255, 255)));
    }
    #[test]
    fn allows_198_20_0_0_above_benchmark() {
        assert!(!is_blocked_ip(v4(198, 20, 0, 0)));
    }

    // --- IETF Protocol Assignments (192.0.0.0/24, RFC 6890) ---
    #[test]
    fn blocks_192_0_0_1() {
        assert!(is_blocked_ip(v4(192, 0, 0, 1)));
    }
    #[test]
    fn blocks_192_0_0_171() {
        assert!(is_blocked_ip(v4(192, 0, 0, 171)));
    }
    #[test]
    fn allows_192_0_1_1_boundary() {
        assert!(!is_blocked_ip(v4(192, 0, 1, 1)));
    }

    // --- 6to4 relay anycast (192.88.99.0/24, RFC 3068) ---
    #[test]
    fn blocks_192_88_99_1() {
        assert!(is_blocked_ip(v4(192, 88, 99, 1)));
    }
    #[test]
    fn allows_192_89_0_0_boundary() {
        assert!(!is_blocked_ip(v4(192, 89, 0, 0)));
    }

    // --- IPv4-mapped (::ffff:a.b.c.d): public maps allowed, private maps blocked ---
    #[test]
    fn allows_mapped_8_8_8_8() {
        assert!(!is_blocked_ip(mapped(8, 8, 8, 8)));
    }
    #[test]
    fn allows_mapped_1_1_1_1() {
        assert!(!is_blocked_ip(mapped(1, 1, 1, 1)));
    }
    #[test]
    fn blocks_mapped_0_0_0_0() {
        assert!(is_blocked_ip(mapped(0, 0, 0, 0)));
    }
    #[test]
    fn blocks_mapped_192_168_1_1() {
        assert!(is_blocked_ip(mapped(192, 168, 1, 1)));
    }
    #[test]
    fn blocks_mapped_100_64_0_1_cgnat() {
        assert!(is_blocked_ip(mapped(100, 64, 0, 1)));
    }
    #[test]
    fn blocks_mapped_172_31_255_255() {
        assert!(is_blocked_ip(mapped(172, 31, 255, 255)));
    }
    #[test]
    fn blocks_mapped_255_255_255_255() {
        assert!(is_blocked_ip(mapped(255, 255, 255, 255)));
    }

    // --- IPv4-compatible (::a.b.c.d, deprecated) ---
    #[test]
    fn blocks_v4_compatible_127_0_0_1() {
        // ::127.0.0.1 -> segments [0,0,0,0,0,0,0x7f00,0x0001]
        assert!(is_blocked_ip(v6(0, 0, 0, 0, 0, 0, 0x7f00, 0x0001)));
    }
    #[test]
    fn blocks_v4_compatible_10_0_0_1() {
        assert!(is_blocked_ip(v6(0, 0, 0, 0, 0, 0, 0x0a00, 0x0001)));
    }
    #[test]
    fn allows_v4_compatible_8_8_8_8() {
        assert!(!is_blocked_ip(v6(0, 0, 0, 0, 0, 0, 0x0808, 0x0808)));
    }

    // --- NAT64 (64:ff9b::/96, RFC 6052) ---
    #[test]
    fn blocks_nat64_metadata() {
        // 64:ff9b::169.254.169.254
        assert!(is_blocked_ip(v6(0x0064, 0xff9b, 0, 0, 0, 0, 0xa9fe, 0xa9fe)));
    }

    // --- IPv6 ULA (fc00::/7) & link-local (fe80::/10) boundaries ---
    #[test]
    fn blocks_fc00_0_0() {
        assert!(is_blocked_ip(v6(0xfc00, 0, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_fdff_ff_ff() {
        assert!(is_blocked_ip(v6(0xfdff, 0xffff, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn allows_fbff_ffff_below_ula() {
        assert!(!is_blocked_ip(v6(0xfbff, 0xffff, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_fe80_0_0() {
        assert!(is_blocked_ip(v6(0xfe80, 0, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_febf_ffff() {
        assert!(is_blocked_ip(v6(0xfebf, 0xffff, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn allows_fe7f_ffff_below_linklocal() {
        assert!(!is_blocked_ip(v6(0xfe7f, 0xffff, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_fec0_0_0_site_local() {
        // fec0::/10 is its own (deprecated site-local) block — intentionally blocked.
        assert!(is_blocked_ip(v6(0xfec0, 0, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_site_local_feff_high() {
        assert!(is_blocked_ip(v6(0xfeff, 0xffff, 0, 0, 0, 0, 0, 0)));
    }
    #[test]
    fn blocks_2001_db8_1_doc_prefix() {
        // 2001:db8::/32 is IPv6 documentation (RFC 3849) — now blocked.
        assert!(is_blocked_ip(v6(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)));
    }

    // --- Smoke test: spec-required full block list all TRUE ---
    #[test]
    fn required_blocklist_all_true() {
        let blocked = [
            v4(127, 0, 0, 1),
            v6(0, 0, 0, 0, 0, 0, 0, 1), // ::1
            v4(10, 0, 0, 1),
            v4(172, 16, 0, 1),
            v4(172, 31, 255, 255),
            v4(192, 168, 1, 1),
            v4(169, 254, 169, 254),
            v4(0, 0, 0, 0),
            v6(0, 0, 0, 0, 0, 0, 0, 0), // ::
            v6(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            v6(0xfc00, 0, 0, 0, 0, 0, 0, 1),
            v6(0xfd00, 0, 0, 0, 0, 0, 0, 1),
            v4(100, 64, 0, 1),
            mapped(127, 0, 0, 1),
            mapped(10, 0, 0, 1),
            mapped(169, 254, 169, 254),
        ];
        for ip in blocked {
            assert!(is_blocked_ip(ip), "{ip} should be blocked");
        }
    }

    // --- Smoke test: spec-required full allow list all FALSE ---
    #[test]
    fn required_allowlist_all_false() {
        let allowed = [
            v4(8, 8, 8, 8),
            v4(1, 1, 1, 1),
            v4(93, 184, 216, 34),
            v6(0x2606, 0x4700, 0, 0, 0, 0, 0, 0x1111),
        ];
        for ip in allowed {
            assert!(!is_blocked_ip(ip), "{ip} should be allowed");
        }
    }
}
