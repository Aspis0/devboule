//! Local main-coder backend config — the single global LLM backend the LOCAL
//! Devboule MAIN coder (the orchestrator binary, `client == "orchestrator"`) runs on.
//!
//! TIER DISTINCTION (read first): this is the ORCHESTRATOR / local-main-coder tier and is
//! a SEPARATE, INDEPENDENT value from the MINI-coder backend
//! (`backend::mini_coder::MiniCoderBackend`, config key `miniCoderBackend`). The mini is the
//! SMALL delegated worker a coder spawns via `spawn_mini_coder`; the orchestrator is the
//! local main coder itself. The two are DISTINCT tiers with DISTINCT models — they must
//! NOT share a config value. (Historically the orchestrator launch wrongly reused the
//! mini's backend; this module removes that conflation.)
//!
//! SHAPE: a discriminated struct mirroring the mini-coder/design-LLM backends field-for-
//! field (`kind`, `model?`, `baseUrl?`). To guarantee the validators NEVER drift, this one
//! does NOT re-implement any primitive: it reuses the mini-coder's `pub(crate)` helpers
//! ([`is_valid_model`](super::mini_coder::is_valid_model),
//! [`validate_omlx_base_url`](super::mini_coder::validate_omlx_base_url)) and the shared
//! `MINI_MODEL_MAX_LEN` cap. The ONLY thing that differs is the accepted KIND set + the
//! user-facing error wording.
//!
//! KIND SET (intentionally narrower than the mini): the orchestrator binary consumes ONLY a
//! loopback HTTP OpenAI-compatible endpoint (`DEVBOULE_OMLX_BASE_URL` + `DEVBOULE_OMLX_MODEL`,
//! POSTed to `<baseUrl>/chat/completions` by its `OmlxModel` client). So the meaningful kinds
//! are the LOCAL ones:
//!   - `ollama`: a local Ollama server. `model` REQUIRED; `baseUrl` OPTIONAL and EDITABLE —
//!     when provided it is validated to a LOOPBACK http origin (same rule as `omlx`) and the
//!     launch points the binary at exactly that URL (e.g. Ollama on a non-default port); when
//!     absent/empty the launch falls back to the [`OLLAMA_OPENAI_BASE_URL`] default
//!     (`http://localhost:11434/v1`). No hardcode lock-in — the default is just an editable
//!     default, not a fixed value.
//!   - `omlx`: a local oMLX (MLX) OpenAI-compatible server. `model` AND `baseUrl` REQUIRED;
//!     `baseUrl` is constrained to a LOOPBACK http origin (http only; privacy: the prompt —
//!     which may carry file content — never leaves the device).
//! There is deliberately NO `api`/`codex`/`appleFm` arm: the binary cannot drive a CLI or a
//! non-HTTP runtime, so offering them would be a config the launch silently ignores.
//!
//! Persisted in config.json under `localCoderBackend`; ABSENT means no local-coder backend
//! is configured, in which case `read_local_coder_backend` returns `None` and the launch
//! passes EMPTY oMLX env so the binary falls back to its safe path (its Mock model). A fresh
//! user must configure the local coder once — it does NOT silently inherit the mini's value.
//!
//! NOTHING here streams or generates. This module owns only the config TYPE + validation;
//! the `get_/set_local_coder_backend` commands live in `projects.rs` next to
//! `set_mini_coder_backend`, cloning that atomic read-modify-write idiom.

use super::mini_coder::{
    is_forbidden_command_char, is_valid_model, is_valid_optional_port, validate_omlx_base_url,
    MINI_BASE_URL_MAX_LEN, MINI_MODEL_MAX_LEN,
};
use serde::{Deserialize, Serialize};

/// The kind of runtime the LOCAL main coder (orchestrator) runs on. snake/lower over the
/// wire to match the TS `LocalCoderBackendKind` and the config.json discriminator exactly.
///
/// Intentionally a SUBSET of [`super::mini_coder::MiniCoderBackendKind`] — only the two
/// LOCAL HTTP runtimes the orchestrator binary can actually drive (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalCoderBackendKind {
    /// A local Ollama server. `model` REQUIRED; `base_url` OPTIONAL + EDITABLE (loopback
    /// http only when set, else the [`OLLAMA_OPENAI_BASE_URL`] default). The launch resolves
    /// the configured-or-default loopback OpenAI-compatible endpoint for the binary's
    /// `OmlxModel` client.
    Ollama,
    /// A local oMLX (MLX) server exposing an OpenAI-compatible HTTP API. `model` AND
    /// `base_url` REQUIRED. The base URL is constrained to a LOOPBACK http origin (http
    /// only; privacy: the prompt never leaves the device).
    Omlx,
    /// CLOUD (opt-in): an HTTPS OpenAI-compatible endpoint (e.g. OpenRouter). `model` AND
    /// `base_url` REQUIRED; the base URL is constrained to an HTTPS, NON-loopback public
    /// host (the inverse of the loopback rule). The API KEY is NOT part of this struct or
    /// config.json — it lives ONLY in the vault (`provider:cloud_llm`) and is read at launch
    /// into `DEVBOULE_CLOUD_API_KEY` (env, off argv, never logged).
    ///
    /// PRIVACY CONTRACT: unlike the two local kinds, Cloud sends the prompt — which may carry
    /// file content — OFF the machine to the configured provider. The host UI shows a
    /// mandatory consent disclosure before this kind can be saved.
    Cloud,
}

/// The DEFAULT Ollama loopback OpenAI-compatible base URL the orchestrator binary's
/// `OmlxModel` client is pointed at when `kind == ollama` and no `base_url` is configured.
/// Ollama serves an OpenAI-compatible API on its standard loopback port, so the orchestrator
/// can drive it with the SAME HTTP client it uses for oMLX. This is an EDITABLE default, not
/// a fixed value: a user running Ollama on a non-default port can set `base_url` explicitly
/// (validated loopback http) and the launch uses that instead. Kept here (single source of
/// truth) so the launch assembly never hardcodes the URL inline.
pub const OLLAMA_OPENAI_BASE_URL: &str = "http://localhost:11434/v1";

/// Validate + NORMALIZE a CLOUD base URL (trailing slash stripped), or a human error string.
/// This is the OPT-IN, consent-gated counterpart to [`validate_omlx_base_url`]: where the
/// loopback validator FORBIDS leaving the machine, this REQUIRES it — `https://` (TLS, since a
/// real provider is never on loopback) + a NON-loopback, fully-qualified public host. It
/// REUSES the SAME `pub(crate)` primitives the loopback validator uses (the char blocklist,
/// the optional-port rule, the length cap) so the two surfaces never drift on those rules.
///
/// SSRF / privacy hardening: reject loopback hosts (a loopback host in Cloud mode is a
/// misconfiguration — the local kinds are the loopback path), reject bare IP literals
/// (IPv4/IPv6), reject single-label/intranet names (require a dot), and reject userinfo
/// (`user@host` / credentials in the URL — they belong in the Authorization header). Mirrors
/// `devboule_coder::model_client::validate_cloud_base_url` so the host and the binary
/// accept/reject the same set.
pub fn validate_cloud_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("Cloud local-coder backend requires a base URL.".into());
    }
    if trimmed.len() > MINI_BASE_URL_MAX_LEN {
        return Err(format!(
            "Cloud base URL must be at most {MINI_BASE_URL_MAX_LEN} characters."
        ));
    }
    if trimmed.chars().any(is_forbidden_command_char) {
        return Err(
            "Cloud base URL must not contain control, bidi or invisible characters.".into(),
        );
    }

    // Scheme: https ONLY. http would send the prompt (which can carry file content) in clear
    // text off the machine.
    let rest = match trimmed.strip_prefix("https://") {
        Some(r) => r,
        None => return Err("Cloud base URL must start with https:// (TLS required).".into()),
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    // Reject userinfo: credentials never live in the URL, and an `@` hides the real host.
    if authority.contains('@') {
        return Err("Cloud base URL must not contain credentials (no '@' / userinfo).".into());
    }
    // IPv6 literal `[..]` is rejected outright: a cloud provider is addressed by hostname.
    if authority.starts_with('[') {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }

    let mut parts = authority.splitn(2, ':');
    let host = parts.next().unwrap_or("");
    if !is_valid_optional_port(parts.next()) {
        return Err("Cloud base URL has an invalid :port.".into());
    }
    if host.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    if host.eq_ignore_ascii_case("localhost") {
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
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
    {
        return Err("Cloud base URL host must be a public provider host, not an intranet/metadata name.".into());
    }
    if !host.contains('.') {
        return Err("Cloud base URL host must be a fully-qualified domain name.".into());
    }
    let labels_ok = host.split('.').all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if !labels_ok {
        return Err("Cloud base URL host is not a valid domain name.".into());
    }

    Ok(trimmed.strip_suffix('/').unwrap_or(trimmed).to_string())
}

/// The single, global local-coder backend config persisted in config.json under
/// `localCoderBackend`. A discriminated struct mirroring
/// [`super::mini_coder::MiniCoderBackend`]'s relevant fields: `kind` picks the runtime and
/// the relevant field is required per kind (validated by [`validate_local_coder_backend`]).
///
/// camelCase + every optional field `#[serde(default)]`/`skip_serializing_if` so a
/// config.json written by the UI (only the fields its kind uses) round-trips without churn
/// and an older/hand-edited config still parses leniently.
///
/// NO `command`/`max_concurrent` fields: there is exactly ONE local main coder per launch
/// (no concurrency knob) and the binary cannot run an `api` CLI command line (no `command`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCoderBackend {
    pub kind: LocalCoderBackendKind,
    /// Model tag/name. REQUIRED for `ollama`, `omlx` and `cloud`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The server base URL. REQUIRED for `omlx` (LOOPBACK http) and `cloud` (HTTPS public
    /// host); OPTIONAL for `ollama` (absent/`None` => the launch uses the
    /// [`OLLAMA_OPENAI_BASE_URL`] default; present => exactly that, e.g. Ollama on a
    /// non-default port). For the local kinds it is validated to a LOOPBACK http origin via
    /// [`super::mini_coder::validate_omlx_base_url`]; for `cloud` it is validated to an HTTPS
    /// non-loopback host via [`validate_cloud_base_url`]. Always STORED NORMALIZED (no
    /// trailing slash).
    ///
    /// The CLOUD API KEY is deliberately NOT a field here — it never touches config.json. It
    /// lives ONLY in the OS vault and is read at launch into `DEVBOULE_CLOUD_API_KEY` (env).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Ordered fallback chain (primary + fallbacks → Vec<SidecarEnvVars> resolved by
    /// `resolve_coder_chain_for_sidecar`). Emitted to the sidecar as `DEVBOULE_PI_MODEL_CHAIN`.
    /// Ignored by the sidecar until B2.2 consumes it. `None` (the common case) keeps
    /// today's single-model behavior.
    #[serde(default)]
    pub fallbacks: Option<Vec<super::mini_coder::FallbackModel>>,
}

/// Validate + normalize a local-coder backend config. Applies the SAME per-field rules as
/// the mini-coder's `ollama`/`omlx` arms by reusing its `pub(crate)` primitives (no logic
/// duplication). Trims fields and keeps ONLY the field(s) the kind uses (so a kind switch
/// never leaves a stale model/base_url). Returns the normalized backend or a human error
/// string.
pub fn validate_local_coder_backend(
    backend: &LocalCoderBackend,
) -> Result<LocalCoderBackend, String> {
    let model = backend
        .model
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    // Validate + normalize the fallback chain (same rules as the mini-coder).
    let validated_fallbacks = super::mini_coder::validate_fallbacks(&backend.fallbacks)?;

    match backend.kind {
        LocalCoderBackendKind::Ollama => {
            if model.is_empty() {
                return Err("Ollama local-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Local-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Local-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            // base_url is OPTIONAL + EDITABLE for ollama. Empty/absent => None (the launch
            // falls back to the OLLAMA_OPENAI_BASE_URL default). A non-empty value is
            // validated with the SAME shared loopback/http-only validator omlx uses (privacy
            // invariant: the prompt — which may carry file content — never leaves the device)
            // and STORED NORMALIZED, so a user can point at Ollama on a non-default port
            // without any hardcode lock-in. The length cap is enforced INSIDE the validator.
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            let base_url = if base_url.is_empty() {
                None
            } else {
                Some(validate_omlx_base_url(&base_url)?)
            };
            Ok(LocalCoderBackend {
                kind: LocalCoderBackendKind::Ollama,
                model: Some(model),
                base_url,
                fallbacks: validated_fallbacks,
            })
        }
        LocalCoderBackendKind::Omlx => {
            // omlx requires BOTH a model (a bare tag, same rule as ollama) and a loopback
            // http (only) base URL.
            if model.is_empty() {
                return Err("oMLX local-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Local-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Local-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("oMLX local-coder backend requires a base URL.".into());
            }
            // Reuse the shared loopback/http-only/port/char validator+normalizer so the
            // local-coder, mini-coder and design oMLX surfaces accept/reject EXACTLY the
            // same set. The length cap is enforced INSIDE this validator (against
            // MINI_BASE_URL_MAX_LEN), so we do not re-check it here — single source of truth.
            let normalized_base = validate_omlx_base_url(&base_url)?;
            Ok(LocalCoderBackend {
                kind: LocalCoderBackendKind::Omlx,
                model: Some(model),
                base_url: Some(normalized_base),
                fallbacks: validated_fallbacks,
            })
        }
        LocalCoderBackendKind::Cloud => {
            // cloud requires BOTH a model (a bare tag, same rule as the local kinds) and an
            // HTTPS NON-loopback base URL. The API KEY is NOT validated here: it is not part
            // of this struct (it lives in the vault). Key PRESENCE is enforced at the
            // command/launch layer (where the vault is reachable), not in this pure
            // config-shape validator — so a saved Cloud config + a separately-saved key stay
            // independent surfaces.
            if model.is_empty() {
                return Err("Cloud local-coder backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Local-coder model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err(
                    "Local-coder model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("Cloud local-coder backend requires a base URL.".into());
            }
            // The cloud (https / non-loopback / FQDN) validator+normalizer. The length cap is
            // enforced INSIDE it (same MINI_BASE_URL_MAX_LEN), so we do not re-check it here.
            let normalized_base = validate_cloud_base_url(&base_url)?;
            Ok(LocalCoderBackend {
                kind: LocalCoderBackendKind::Cloud,
                model: Some(model),
                base_url: Some(normalized_base),
                fallbacks: validated_fallbacks,
            })
        }
    }
}

/// Resolve the loopback OpenAI-compatible base URL + model the orchestrator binary's env
/// (`DEVBOULE_OMLX_BASE_URL` / `DEVBOULE_OMLX_MODEL`) should carry for a validated backend.
///
/// - `ollama` => the CONFIGURED (validated, normalized) loopback base URL if one was set
///   (e.g. Ollama on a non-default port), else the [`OLLAMA_OPENAI_BASE_URL`] DEFAULT — plus
///   the configured model tag.
/// - `omlx`   => the configured (validated, normalized) loopback base URL + model.
///
/// `model` is guaranteed `Some` for a value that went through
/// [`validate_local_coder_backend`]; the `unwrap_or_default` guards a hand-built struct so
/// this never panics (an empty string then yields the binary's safe Mock-model fallback).
/// For omlx, `base_url` is likewise guaranteed `Some` post-validation; for ollama it is
/// genuinely optional and `None` selects the default.
pub fn resolve_omlx_env(backend: &LocalCoderBackend) -> (String, String) {
    let model = backend.model.clone().unwrap_or_default();
    let base_url = match backend.kind {
        // ollama: the configured URL wins; fall back to the editable default when unset.
        LocalCoderBackendKind::Ollama => backend
            .base_url
            .clone()
            .unwrap_or_else(|| OLLAMA_OPENAI_BASE_URL.to_string()),
        LocalCoderBackendKind::Omlx => backend.base_url.clone().unwrap_or_default(),
        // Cloud is NOT a loopback oMLX backend: it resolves to the DEVBOULE_CLOUD_* env set
        // via `resolve_cloud_env`, never the DEVBOULE_OMLX_* set. Returning EMPTY here means a
        // caller that wrongly routed a Cloud backend through this resolver sets NO oMLX env
        // (the binary then runs its safe Mock) rather than mis-pointing the loopback client.
        LocalCoderBackendKind::Cloud => String::new(),
    };
    (base_url, model)
}

/// Resolve the CLOUD env (`DEVBOULE_CLOUD_BASE_URL` + `DEVBOULE_CLOUD_MODEL`) the orchestrator
/// binary should carry for a validated `cloud` backend. Returns `("", "")` for any NON-cloud
/// kind (the local kinds go through [`resolve_omlx_env`] instead), so the launch can call this
/// unconditionally and only the matching env set is non-empty.
///
/// The API KEY is NOT resolved here: it is a SECRET read from the vault at launch
/// (`read_cloud_llm_key`) and injected as `DEVBOULE_CLOUD_API_KEY` via the per-launch process
/// env — never from this struct, never on argv, never logged.
pub fn resolve_cloud_env(backend: &LocalCoderBackend) -> (String, String) {
    match backend.kind {
        LocalCoderBackendKind::Cloud => (
            backend.base_url.clone().unwrap_or_default(),
            backend.model.clone().unwrap_or_default(),
        ),
        LocalCoderBackendKind::Ollama | LocalCoderBackendKind::Omlx => {
            (String::new(), String::new())
        }
    }
}

/// PURE preflight verdict for a LOCAL (loopback) orchestrator model backend.
/// `listed` is the id list from the server's `/v1/models` — `None` when the
/// server could not be reached at all. Errors are user-facing launch errors:
/// the alternative was the "planner is thinking forever" hang (the binary
/// retries a dead/empty backend in silence for minutes before any signal).
///
/// Honesty note: oMLX's `/v1/models` lists AVAILABLE (on-disk) models, not
/// loaded ones — a listed model may still need a long first-request load. This
/// preflight catches "server down", "nothing configured" and "model missing";
/// load latency is the UI watchdog's job, not ours.
/// Shared user-facing copy for "no orchestrator model at all is configured" — used both by
/// the reachable-server preflight below (model configured but blank) AND by the launch-time
/// gate (`orchestrator_model_configured_verdict`) that runs BEFORE any network probe, when
/// there is no backend to probe in the first place. ONE string, so the two call sites can
/// never drift apart.
pub const NO_LOCAL_ORCHESTRATOR_MODEL_MSG: &str =
    "No local orchestrator model is configured — pick one in Settings → Providers, or switch \
     the orchestrator to Claude/Codex.";

fn local_model_preflight_verdict(
    base_url: &str,
    model: &str,
    listed: Option<&[String]>,
) -> Result<(), String> {
    let Some(ids) = listed else {
        return Err(format!(
            "The local model server is not reachable at {base_url}. Start oMLX/Ollama (or fix \
             the base URL in Settings → Providers), then relaunch the orchestrator."
        ));
    };
    if model.trim().is_empty() {
        return Err(NO_LOCAL_ORCHESTRATOR_MODEL_MSG.to_string());
    }
    if ids.is_empty() {
        return Err(format!(
            "The local model server at {base_url} reports no models. Load/pull the model \
             \"{model}\" there, then relaunch the orchestrator."
        ));
    }
    if !ids.iter().any(|id| id == model) {
        let available = ids
            .iter()
            .take(5)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "The configured orchestrator model \"{model}\" is not available on the local model \
             server at {base_url} (it has: {available}). Pick an available model in Settings → \
             Providers, then relaunch."
        ));
    }
    Ok(())
}

/// Blocking fetch of the loopback server's `/v1/models` id list; `None` on any
/// network/HTTP/parse failure (the verdict fn turns that into the user-facing
/// error). Bounded timeouts — this runs synchronously inside the launch command,
/// so the worst case adds ~2.5s to a launch against a dead server, versus the
/// minutes-long silent hang it prevents.
fn fetch_local_models_blocking(base_url: &str) -> Option<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(1500))
        .timeout(std::time::Duration::from_millis(2500))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let resp = client.get(&url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // Bounded read (mirrors provider_detect's MAX_PROBE_BODY_BYTES rationale): a
    // buggy/hostile loopback server must not stream an unbounded body into RAM.
    use std::io::Read;
    let mut body = String::new();
    resp.take(256 * 1024).read_to_string(&mut body).ok()?;
    Some(super::provider_detect::parse_omlx_models(&body))
}

/// Launch-time preflight for the LOCAL orchestrator: verify the configured
/// loopback backend is reachable and actually serves the configured model,
/// BEFORE spawning the binary. A `cloud` backend (empty oMLX env) is not ours
/// to probe — Ok. Called only from the `client == "orchestrator"` launch path.
pub fn preflight_local_orchestrator_backend(backend: &LocalCoderBackend) -> Result<(), String> {
    let (base_url, model) = resolve_omlx_env(backend);
    if base_url.trim().is_empty() {
        return Ok(());
    }
    let listed = fetch_local_models_blocking(&base_url);
    local_model_preflight_verdict(&base_url, &model, listed.as_deref())
}

/// FAIL-LOUD launch gate (bug B2): an orchestrator launch with NEITHER a local (oMLX/
/// Ollama) model NOR a cloud model configured must be REJECTED before the binary spawns —
/// without this gate, `build_model` (devboule-coder/src/config.rs) silently selects its
/// safe MockModel, and the user ends up chatting with a nonsense "Mock reply to: …" with
/// zero signal that anything is wrong.
///
/// Pure: takes the SAME two already-resolved env pairs the launch site builds
/// (`resolve_omlx_env` / `resolve_cloud_env`) — never a `LocalCoderBackend` directly — so
/// this can never drift from what the child process will actually receive. `Ok(())` when
/// EITHER base URL is non-blank (either one keeps the binary off the Mock path).
pub fn orchestrator_model_configured_verdict(
    omlx_base_url: &str,
    cloud_base_url: &str,
) -> Result<(), String> {
    if omlx_base_url.trim().is_empty() && cloud_base_url.trim().is_empty() {
        return Err(NO_LOCAL_ORCHESTRATOR_MODEL_MSG.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests assert against the base-URL cap directly; the validator delegates the
    // length check to `validate_omlx_base_url`, so this const is test-scoped here.
    use super::super::mini_coder::MINI_BASE_URL_MAX_LEN;

    // -- preflight verdict ----------------------------------------------------

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn preflight_unreachable_server_is_a_launch_error() {
        let err = local_model_preflight_verdict("http://127.0.0.1:8000/v1", "m", None)
            .unwrap_err();
        assert!(err.contains("not reachable"), "{err}");
        assert!(err.contains("127.0.0.1:8000"), "{err}");
    }

    #[test]
    fn preflight_empty_model_is_a_launch_error() {
        let err =
            local_model_preflight_verdict("http://x/v1", "  ", Some(&ids(&["a"]))).unwrap_err();
        assert!(err.contains("No local orchestrator model"), "{err}");
    }

    #[test]
    fn preflight_empty_model_list_is_a_launch_error() {
        let err = local_model_preflight_verdict("http://x/v1", "qwen", Some(&[])).unwrap_err();
        assert!(err.contains("reports no models"), "{err}");
        assert!(err.contains("qwen"), "{err}");
    }

    #[test]
    fn preflight_missing_model_lists_available_ones() {
        let err = local_model_preflight_verdict(
            "http://x/v1",
            "qwen-42b",
            Some(&ids(&["small-1", "small-2"])),
        )
        .unwrap_err();
        assert!(err.contains("qwen-42b"), "{err}");
        assert!(err.contains("small-1, small-2"), "{err}");
    }

    #[test]
    fn preflight_listed_model_passes() {
        assert!(
            local_model_preflight_verdict("http://x/v1", "qwen", Some(&ids(&["other", "qwen"])))
                .is_ok()
        );
    }

    #[test]
    fn preflight_cloud_backend_is_skipped() {
        let backend = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            base_url: Some("https://api.example.com/v1".into()),
            model: Some("big".into()),
            fallbacks: None,
        };
        assert!(preflight_local_orchestrator_backend(&backend).is_ok());
    }

    // -- B2: no-model launch gate ---------------------------------------------
    // A `None` local-coder backend yields EMPTY oMLX env AND empty cloud env (both
    // resolvers return `("", "")` for a `None` backend at the call site in
    // `projects.rs`); without a gate, the launch proceeded anyway and the binary's
    // `build_model` silently picked its MockModel — the user chatted with a
    // nonsense "Mock reply to: …" with zero signal (bug B2). This is the pure
    // decision the launch site calls with its two already-resolved env pairs.

    #[test]
    fn orchestrator_gate_rejects_when_both_envs_are_empty() {
        let err = orchestrator_model_configured_verdict("", "").unwrap_err();
        assert_eq!(err, NO_LOCAL_ORCHESTRATOR_MODEL_MSG);
        assert!(err.contains("No local orchestrator model is configured"), "{err}");
    }

    #[test]
    fn orchestrator_gate_rejects_whitespace_only_urls() {
        // Defense in depth: a whitespace-only value must not be treated as "configured".
        assert!(orchestrator_model_configured_verdict("   ", "\t").is_err());
    }

    #[test]
    fn orchestrator_gate_passes_with_omlx_configured() {
        assert!(orchestrator_model_configured_verdict("http://127.0.0.1:8000/v1", "").is_ok());
    }

    #[test]
    fn orchestrator_gate_passes_with_cloud_configured() {
        assert!(orchestrator_model_configured_verdict("", "https://api.example.com/v1").is_ok());
    }

    #[test]
    fn preflight_verdict_uses_the_shared_gate_message() {
        // Guards against copy drift between the launch-time gate and the reachable-
        // server preflight: both must say the exact same thing for "no model".
        let err =
            local_model_preflight_verdict("http://x/v1", "  ", Some(&ids(&["a"]))).unwrap_err();
        assert_eq!(err, NO_LOCAL_ORCHESTRATOR_MODEL_MSG);
    }

    // -- serde --------------------------------------------------------------

    #[test]
    fn backend_kind_serializes_lowercase_matching_ts() {
        for (kind, tok) in [
            (LocalCoderBackendKind::Ollama, "ollama"),
            (LocalCoderBackendKind::Omlx, "omlx"),
            (LocalCoderBackendKind::Cloud, "cloud"),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{tok}\""));
            let back: LocalCoderBackendKind =
                serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn backend_round_trips_camel_case_and_skips_unused_fields() {
        let ollama = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: None,
            fallbacks: None,
        };
        let json = serde_json::to_string(&ollama).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "json: {json}");
        assert!(json.contains("\"model\":\"qwen2.5-coder\""), "json: {json}");
        assert!(!json.contains("baseUrl"), "unused baseUrl leaked: {json}");
        let back: LocalCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(ollama, back);
    }

    #[test]
    fn ollama_with_custom_base_url_round_trips() {
        // A saved ollama config with a custom (non-default) base_url must persist + reload
        // byte-identically — the serde shape is unchanged (baseUrl already Option<String>).
        let ollama = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("http://localhost:11500/v1".into()),
            fallbacks: None,
        };
        let json = serde_json::to_string(&ollama).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "json: {json}");
        assert!(
            json.contains("\"baseUrl\":\"http://localhost:11500/v1\""),
            "json: {json}"
        );
        let back: LocalCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(ollama, back);
    }

    #[test]
    fn omlx_round_trips_camel_case_baseurl() {
        let omlx = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("http://localhost:8000/v1".into()),
            fallbacks: None,
        };
        let json = serde_json::to_string(&omlx).unwrap();
        assert!(json.contains("\"kind\":\"omlx\""), "json: {json}");
        assert!(
            json.contains("\"baseUrl\":\"http://localhost:8000/v1\""),
            "json: {json}"
        );
        let back: LocalCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(omlx, back);
    }

    #[test]
    fn partial_json_lenient_parse() {
        // The leanest config the UI could emit for ollama (no baseUrl key).
        let json = r#"{ "kind": "ollama", "model": "qwen2.5-coder" }"#;
        let b: LocalCoderBackend = serde_json::from_str(json).unwrap();
        assert_eq!(b.kind, LocalCoderBackendKind::Ollama);
        assert_eq!(b.model.as_deref(), Some("qwen2.5-coder"));
        assert_eq!(b.base_url, None);
    }

    #[test]
    fn local_and_mini_keys_parse_to_independent_values() {
        // The two tiers are stored under SEPARATE keys and parse to SEPARATE types with
        // SEPARATE models. This is the exact independence `read_local_coder_backend` and
        // `read_mini_coder_backend` rely on: the orchestrator's model is NOT the mini's.
        // (Mirrors what the readers do: `value.get("<key>")` then `from_value`.)
        use super::super::mini_coder::{MiniCoderBackend, MiniCoderBackendKind};
        let config = serde_json::json!({
            "miniCoderBackend":  { "kind": "ollama", "model": "mini-small-model" },
            "localCoderBackend": { "kind": "ollama", "model": "orchestrator-big-model" },
        });

        let local: LocalCoderBackend =
            serde_json::from_value(config.get("localCoderBackend").unwrap().clone()).unwrap();
        let mini: MiniCoderBackend =
            serde_json::from_value(config.get("miniCoderBackend").unwrap().clone()).unwrap();

        assert_eq!(local.kind, LocalCoderBackendKind::Ollama);
        assert_eq!(local.model.as_deref(), Some("orchestrator-big-model"));
        assert_eq!(mini.kind, MiniCoderBackendKind::Ollama);
        assert_eq!(mini.model.as_deref(), Some("mini-small-model"));
        // The decisive assertion: configuring one tier did not bleed into the other.
        assert_ne!(local.model, mini.model);
    }

    #[test]
    fn absent_local_key_does_not_inherit_mini() {
        // A config that has ONLY the mini key carries NO localCoderBackend, so the reader's
        // `value.get("localCoderBackend")` returns None -> the launch gets empty oMLX env
        // (the binary's safe Mock path). This is the whole point of the fix: no silent
        // inheritance of the mini's model.
        let config = serde_json::json!({
            "miniCoderBackend": { "kind": "ollama", "model": "mini-small-model" },
        });
        assert!(config.get("localCoderBackend").is_none());
    }

    #[test]
    fn unknown_kind_is_rejected_at_parse() {
        // The local coder must NOT accept the mini's extra kinds (api/codex/appleFm): the
        // binary cannot drive them. A config with such a kind fails to deserialize, which
        // `read_local_coder_backend` then maps to None (safe fallback).
        for bad in ["api", "codex", "appleFm", "claude"] {
            let json = format!(r#"{{ "kind": "{bad}", "model": "m" }}"#);
            assert!(
                serde_json::from_str::<LocalCoderBackend>(&json).is_err(),
                "kind {bad:?} must not deserialize for the local coder"
            );
        }
    }

    // -- validation ---------------------------------------------------------

    #[test]
    fn validate_ollama_requires_model_and_base_url_is_optional() {
        let no_model = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: None,
            base_url: None,
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&no_model).is_err());

        // No base_url => stays None (the launch will use the OLLAMA_OPENAI_BASE_URL default).
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("  qwen2.5-coder  ".into()),
            base_url: None,
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder")); // trimmed
        assert_eq!(n.base_url, None); // optional, left as default
    }

    #[test]
    fn validate_ollama_treats_blank_base_url_as_default() {
        // A whitespace-only base_url is "not configured" => None (use the default), NOT an
        // error. This is the leniency that lets the UI send an empty field for "use default".
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("   ".into()),
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.base_url, None);
    }

    #[test]
    fn validate_ollama_keeps_and_normalizes_custom_loopback_base_url() {
        // A user pointing at Ollama on a NON-DEFAULT port: the value is validated + kept
        // (normalized, trailing slash stripped) — no hardcode lock-in to :11434.
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("  http://localhost:11500/v1/  ".into()),
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.base_url.as_deref(), Some("http://localhost:11500/v1"));
    }

    #[test]
    fn validate_ollama_rejects_non_loopback_or_https_base_url() {
        // The privacy invariant applies to ollama's base_url too: a non-loopback host or an
        // https scheme is REJECTED with the same shared validator omlx uses.
        for bad in [
            "https://localhost:11434/v1",   // https rejected
            "http://evil.com:11434/v1",     // non-loopback host
            "http://127.0.0.1.evil.com/v1", // suffix trick
            "http://127.0.0.1@evil.com/v1", // userinfo trick
        ] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Ollama,
                model: Some("qwen2.5-coder".into()),
                base_url: Some(bad.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "ollama base URL {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_model_with_whitespace_or_metachars() {
        for bad in ["has space", "with;semicolon", "pipe|here", "$(sub)", "-leadingdash"] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Ollama,
                model: Some(bad.into()),
                base_url: None,
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "model {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_overlong_model() {
        let long_model = "a".repeat(MINI_MODEL_MAX_LEN + 1);
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some(long_model),
            base_url: None,
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&b).is_err());
    }

    #[test]
    fn omlx_requires_model_and_base_url() {
        let no_model = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: None,
            base_url: Some("http://localhost:8000/v1".into()),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&no_model).is_err());

        let no_base = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            base_url: None,
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&no_base).is_err());
    }

    #[test]
    fn omlx_accepts_loopback_http_and_trims_model() {
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("  qwen2.5-coder  ".into()),
            base_url: Some("  http://127.0.0.1:8000/v1  ".into()),
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder"));
        assert_eq!(n.base_url.as_deref(), Some("http://127.0.0.1:8000/v1"));
    }

    #[test]
    fn omlx_rejects_https_and_non_loopback_and_userinfo_tricks() {
        for bad in [
            "https://localhost:8000/v1",     // https rejected
            "http://evil.com:8000/v1",       // non-loopback host
            "http://127.0.0.1.evil.com/v1",  // suffix trick
            "http://127.0.0.1@evil.com/v1",  // userinfo trick
            "http://[::1]:8000@evil.com/v1", // ipv6 userinfo trick
            "ftp://localhost/v1",            // wrong scheme
        ] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Omlx,
                model: Some("qwen2.5-coder".into()),
                base_url: Some(bad.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "oMLX base URL {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn omlx_normalizes_trailing_slash_and_validates_port() {
        let bad_port = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("m".into()),
            base_url: Some("http://localhost:99999/v1".into()),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&bad_port).is_err());

        let trailing = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("m".into()),
            base_url: Some("http://localhost:8000/v1/".into()),
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&trailing).unwrap();
        assert_eq!(n.base_url.as_deref(), Some("http://localhost:8000/v1"));
    }

    #[test]
    fn omlx_rejects_overlong_base_url() {
        let long = format!("http://localhost:8000/{}", "a".repeat(MINI_BASE_URL_MAX_LEN));
        let overlong = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("m".into()),
            base_url: Some(long),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&overlong).is_err());
    }

    // -- cloud kind ---------------------------------------------------------

    #[test]
    fn cloud_round_trips_camel_case_and_omits_no_key_field() {
        // The API key is NEVER a struct field, so a Cloud config serialized to config.json
        // carries ONLY kind/model/baseUrl — never a key.
        let cloud = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("openrouter/auto".into()),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            fallbacks: None,
        };
        let json = serde_json::to_string(&cloud).unwrap();
        assert!(json.contains("\"kind\":\"cloud\""), "json: {json}");
        assert!(
            json.contains("\"baseUrl\":\"https://openrouter.ai/api/v1\""),
            "json: {json}"
        );
        // The decisive privacy assertion: no key/secret/apiKey field is ever serialized.
        assert!(!json.to_lowercase().contains("key"), "no key field: {json}");
        assert!(!json.to_lowercase().contains("secret"), "no secret field: {json}");
        let back: LocalCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(cloud, back);
    }

    #[test]
    fn cloud_requires_model_and_https_base_url() {
        let no_model = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: None,
            base_url: Some("https://openrouter.ai/api/v1".into()),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&no_model).is_err());

        let no_base = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("openrouter/auto".into()),
            base_url: None,
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&no_base).is_err());
    }

    #[test]
    fn cloud_accepts_https_public_host_and_normalizes() {
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("  openrouter/auto  ".into()),
            base_url: Some("  https://openrouter.ai/api/v1/  ".into()),
            fallbacks: None,
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("openrouter/auto"));
        assert_eq!(n.base_url.as_deref(), Some("https://openrouter.ai/api/v1"));
    }

    #[test]
    fn cloud_rejects_http_loopback_ip_and_userinfo() {
        for bad in [
            "http://openrouter.ai/api/v1",   // http (clear text off-machine)
            "https://localhost:8000/v1",     // loopback as cloud (misconfig)
            "https://127.0.0.1/v1",          // loopback IP
            "https://1.2.3.4/v1",            // bare IPv4 (SSRF)
            "https://[2001:db8::1]/v1",      // IPv6 literal
            "https://internal/v1",           // single-label intranet
            "https://user:pass@openrouter.ai/v1", // userinfo / credentials in URL
            "https://openrouter.ai@evil.com/v1",  // host-confusion userinfo
            "ftp://openrouter.ai/v1",        // wrong scheme
        ] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Cloud,
                model: Some("m".into()),
                base_url: Some(bad.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "cloud base URL {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn cloud_rejects_overlong_base_url() {
        let long = format!("https://openrouter.ai/{}", "a".repeat(MINI_BASE_URL_MAX_LEN));
        let overlong = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("m".into()),
            base_url: Some(long),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&overlong).is_err());
    }

    #[test]
    fn cloud_rejects_numeric_quad_ip_literals_with_leading_zeros() {
        // Rust's Ipv4Addr parser rejects leading-zero / out-of-range dotted-quads, so the
        // all-numeric-4-label fallback is required to reject these IP-literal-disguised hosts.
        for bad in [
            "https://01.02.03.04/v1",
            "https://010.0.0.1/v1",
            "https://0177.0.0.1/v1",
            "https://999.999.999.999/v1",
            "https://01.02.03.04:8443/v1",
        ] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Cloud,
                model: Some("m".into()),
                base_url: Some(bad.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "numeric-quad IP literal {bad:?} must be rejected"
            );
        }
        // A real public hostname is still accepted.
        for ok in ["https://api.openai.com/v1", "https://openrouter.ai/api/v1"] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Cloud,
                model: Some("m".into()),
                base_url: Some(ok.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_ok(),
                "real host {ok:?} must still be accepted"
            );
        }
    }

    #[test]
    fn cloud_rejects_metadata_and_intranet_suffix_hosts() {
        // PARTIAL SSRF mitigation: the cloud-metadata FQDN + `.internal` / `.local` suffixes.
        for bad in [
            "https://metadata.google.internal/computeMetadata/v1",
            "https://foo.internal/v1",
            "https://bar.local/v1",
            "https://METADATA.GOOGLE.INTERNAL/v1",
        ] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Cloud,
                model: Some("m".into()),
                base_url: Some(bad.into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "intranet/metadata host {bad:?} must be rejected"
            );
        }
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("m".into()),
            base_url: Some("https://api.openai.com/v1".into()),
            fallbacks: None,
        };
        assert!(validate_local_coder_backend(&ok).is_ok());
    }

    #[test]
    fn local_kinds_still_reject_https_after_cloud_added() {
        // Regression guard: adding Cloud must NOT loosen the loopback kinds. omlx/ollama
        // still reject https + non-loopback (byte-identical to before).
        for kind in [LocalCoderBackendKind::Omlx, LocalCoderBackendKind::Ollama] {
            let b = LocalCoderBackend {
                kind,
                model: Some("m".into()),
                base_url: Some("https://openrouter.ai/api/v1".into()),
                fallbacks: None,
            };
            assert!(
                validate_local_coder_backend(&b).is_err(),
                "{kind:?} must still reject an https cloud URL"
            );
        }
    }

    #[test]
    fn resolve_cloud_env_returns_base_and_model_for_cloud_only() {
        let cloud = LocalCoderBackend {
            kind: LocalCoderBackendKind::Cloud,
            model: Some("openrouter/auto".into()),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            fallbacks: None,
        };
        let (base, model) = resolve_cloud_env(&cloud);
        assert_eq!(base, "https://openrouter.ai/api/v1");
        assert_eq!(model, "openrouter/auto");
        // resolve_omlx_env must NOT emit a base URL for a Cloud backend (no mis-pointing the
        // loopback client at the cloud host).
        let (omlx_base, _) = resolve_omlx_env(&cloud);
        assert_eq!(omlx_base, "", "Cloud must not produce an oMLX base URL");

        // And the inverse: a local kind yields no cloud env.
        let omlx = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("m".into()),
            base_url: Some("http://localhost:8000/v1".into()),
            fallbacks: None,
        };
        assert_eq!(resolve_cloud_env(&omlx), (String::new(), String::new()));
    }

    // -- resolve_omlx_env ---------------------------------------------------

    #[test]
    fn resolve_env_ollama_without_base_url_uses_default() {
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: None,
            fallbacks: None,
        };
        let (base, model) = resolve_omlx_env(&b);
        assert_eq!(base, OLLAMA_OPENAI_BASE_URL);
        assert_eq!(model, "qwen2.5-coder");
    }

    #[test]
    fn resolve_env_ollama_with_custom_base_url_uses_it() {
        // The configured loopback URL (e.g. Ollama on a non-default port) wins over the
        // default — the whole point of making the endpoint user-settable.
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("http://localhost:11500/v1".into()),
            fallbacks: None,
        };
        let (base, model) = resolve_omlx_env(&b);
        assert_eq!(base, "http://localhost:11500/v1");
        assert_ne!(base, OLLAMA_OPENAI_BASE_URL);
        assert_eq!(model, "qwen2.5-coder");
    }

    #[test]
    fn resolve_env_omlx_uses_configured_base_and_model() {
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("mlx-qwen".into()),
            base_url: Some("http://localhost:8000/v1".into()),
            fallbacks: None,
        };
        let (base, model) = resolve_omlx_env(&b);
        assert_eq!(base, "http://localhost:8000/v1");
        assert_eq!(model, "mlx-qwen");
    }
}
