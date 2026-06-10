//! Censor Gemma layer (sub-phase A4) — the OPTIONAL local-AI tier.
//!
//! After the deterministic linters produce a file's findings, this tier asks a
//! LOCAL Gemma model (via Ollama) for a SINGLE forward pass over that one file to
//! flag file-local, non-deterministic semantic smells the linters cannot catch
//! (inverted logic, a copy-paste leftover that still compiles, swapped call args,
//! a comment that contradicts the code, a swallowed error). It is conservative,
//! additive, and degrades cleanly: if Ollama or the model is absent the whole tier
//! is silently disabled and the fine pass behaves exactly like the deterministic-
//! only A3 engine.
//!
//! PRIVACY / LOCAL-FIRST GUARANTEE (read before touching anything here):
//!   - The ONLY endpoint contacted is the loopback Ollama daemon at
//!     `http://127.0.0.1:11434`. There is NO remote API, NO cloud provider, NO
//!     telemetry. The model runs on the user's own machine.
//!   - The file content fed to the model NEVER leaves the device: it travels over
//!     loopback to a local process and back. Nothing is uploaded anywhere.
//!   - We NEVER persist or log the file content or the model's raw output. On a
//!     timeout / error we log the model name + the file's project-relative path
//!     ONLY (mirrors the runner discipline in `runners/mod.rs`). Titles/bodies that
//!     survive parsing are still run through `redact_secrets` + `cap` as a defense:
//!     the model could echo a secret it read from the file, so we strip it before it
//!     can reach a persisted shard (mirrors `agent_pty.rs`'s "never write secrets to
//!     disk" posture).
//!   - No tool-calling, no MCP, no Oracle: a single `POST /api/generate`,
//!     `stream:false`. The model only ever sees the prompt we build; it cannot reach
//!     out anywhere.

use super::runners::{cap, redact_secrets, RawFinding};
use super::schema::{Category, Severity};
use std::path::Path;
use std::time::Duration;

/// The DEFAULT Ollama tag for the model we drive. Both `gemma4:e4b` and `gemma4:e2b`
/// are real Ollama tags (`ollama.com/library/gemma4`, Apache-2.0, Gemma 4 released
/// 2026-03-31). The default is the larger, higher-quality `e4b`.
///
/// RESOLUTION CHAIN (do NOT collapse this back to a single hardcoded tag — the chain is
/// load-bearing for upgrade safety; see [`resolve_gemma_model`]):
///   1. a user-configured `ollamaModel` (validated) wins outright — the user's explicit
///      choice is honored even if it is not in the daemon's `/api/tags` list (so a tag
///      they are about to pull is not silently overridden);
///   2. else this default ([`GEMMA_MODEL`] = `gemma4:e4b`) IF it is present in
///      `/api/tags`;
///   3. else [`GEMMA_FALLBACK_MODEL`] (`gemma4:e2b`) if e2b is present — this is the
///      UPGRADE-SAFETY case: an existing install that only ever pulled `gemma4:e2b` keeps
///      its Gemma tier instead of silently losing it when the default bumped to `e4b`;
///      it lets us migrate the default without forcing every old install to re-pull;
///   4. else the default `gemma4:e4b` (neither present) — the generate call then fails/
///      degrades exactly as today (unavailable tier, never a crash).
///
/// The Ollama client resolves this ONCE per session from the SAME `/api/tags` list its
/// availability probe already fetches, so probe and generate never disagree.
pub const GEMMA_MODEL: &str = "gemma4:e4b";

/// The Ollama tag the resolution chain falls back to when the default ([`GEMMA_MODEL`])
/// is absent but this older tag is present (step 3 above). Kept as a named constant so
/// the resolver and its tests share one source of truth. Ollama-specific: the oMLX
/// provider has no `/api/tags` equivalent and so never applies this fallback.
pub const GEMMA_FALLBACK_MODEL: &str = "gemma4:e2b";

/// PURE resolver for the Ollama-provider Gemma model tag (no IO; the caller passes the
/// `/api/tags` names it already fetched in the availability probe). Implements the
/// [`GEMMA_MODEL`] resolution chain:
///   - `configured` is `Some(non-empty)` → use it verbatim (the user's explicit, already-
///     validated choice wins even if absent from `available_tags` — they may be pulling
///     it; we never silently override an explicit override);
///   - else default [`GEMMA_MODEL`] if it is in `available_tags`;
///   - else [`GEMMA_FALLBACK_MODEL`] if it is in `available_tags` (upgrade safety);
///   - else [`GEMMA_MODEL`] (neither present — let the generate call degrade cleanly).
///
/// `configured` is expected pre-trimmed/validated (an empty/whitespace-only string is
/// treated as absent, matching what [`validate_censor_local_ai`] would have normalized to
/// `None`).
pub fn resolve_gemma_model(configured: Option<&str>, available_tags: &[String]) -> String {
    if let Some(c) = configured {
        let c = c.trim();
        if !c.is_empty() {
            // The user's explicit choice wins outright (documented rule): we do NOT require
            // it to be in `available_tags` — they may be mid-pull, and silently swapping in
            // a different model would be more surprising than a clean degrade.
            return c.to_string();
        }
    }
    let has = |tag: &str| available_tags.iter().any(|t| t == tag);
    if has(GEMMA_MODEL) {
        GEMMA_MODEL.to_string()
    } else if has(GEMMA_FALLBACK_MODEL) {
        // UPGRADE SAFETY: e4b not pulled but the old e2b is — keep the Gemma tier alive.
        GEMMA_FALLBACK_MODEL.to_string()
    } else {
        GEMMA_MODEL.to_string()
    }
}

/// Loopback-only Ollama base URL. NEVER point this at a non-loopback host — the
/// privacy guarantee (file content never leaves the device) depends on it.
pub const OLLAMA_BASE: &str = "http://127.0.0.1:11434";

/// Default loopback base for the oMLX (OpenAI-compatible) provider — oMLX's documented
/// default is `http://localhost:8000/v1`. Used to CLAMP a non-loopback oMLX base in
/// [`OmlxClient::with_config`] (privacy fail-safe, mirroring [`OllamaClient`]'s clamp to
/// [`OLLAMA_BASE`]); it is also the default when a config selects the oMLX provider with
/// no explicit base. NEVER point this at a non-loopback host.
pub const OMLX_DEFAULT_BASE: &str = "http://localhost:8000/v1";

/// Cap on the oMLX base URL length, mirroring `mini_coder::MINI_BASE_URL_MAX_LEN` so the
/// two oMLX entry points agree. A present-but-overlong base fails validation and the
/// config falls back to the safe Ollama default.
pub const OMLX_BASE_URL_MAX_LEN: usize = 200;

/// Cap on the oMLX model id length, mirroring the TS `censorLocalAi.ts`
/// `CENSOR_OMLX_MODEL_MAX_LEN` (200). An oMLX model id can be an `org/name` HF-style
/// path so it is longer than the mini-coder argv `MINI_MODEL_MAX_LEN` (80); a present-
/// but-overlong model fails validation. Keep this in EXACT agreement with the TS cap.
pub const CENSOR_OMLX_MODEL_MAX_LEN: usize = 200;

/// Hard wall-clock timeout for a single Gemma generate call. A 2B model on CPU can
/// be slow; the watcher is allowed to lag, but a single file must never hang the
/// serialized worker forever. On timeout the call yields no findings (clean
/// degrade) and the deterministic findings for the file are unaffected.
pub const GEMMA_GENERATE_TIMEOUT: Duration = Duration::from_secs(60);

/// Shorter timeout for the cheap availability probe (`GET /api/tags`). The probe is
/// a fast metadata read; if it can't answer quickly the daemon is effectively
/// unavailable for our purposes and we disable the tier.
pub const GEMMA_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Cap on how much of a file we feed to the model. A huge file (a generated bundle,
/// a vendored blob) is neither cheap nor useful to review semantically, and a 2B
/// model's context is small; we truncate the content fed in. The orchestrator's own
/// hash size cap (8 MiB) already gates the file before we ever get here, but we cap
/// again at the prompt boundary so the request body stays bounded regardless.
pub const MAX_FILE_CHARS: usize = 24_000;

/// Hard cap on the number of findings we accept from a single Gemma response. The
/// model is untrusted: a malformed or hostile response must not be able to inflate
/// a shard. Real file-local smells in one file are a handful; 20 is generous.
pub const MAX_GEMMA_FINDINGS: usize = 20;

/// Hard cap on the raw HTTP response body we will buffer from the local daemon
/// before deserializing. A runaway / looping local model with `stream:false` could
/// otherwise return hundreds of MiB in a single `response` field and OOM us (the
/// body is read fully into memory to parse). 1 MiB is plenty for a JSON object whose
/// `response` is a small array of ≤20 short findings; anything larger is treated as a
/// decode failure (the tier degrades cleanly to deterministic-only for that file).
pub const RESPONSE_BODY_CAP: usize = 1024 * 1024;

/// Cap on a single finding's title / body length (chars), to bound what a verbose
/// or adversarial model can write into a shard.
const TITLE_CAP: usize = 200;
const BODY_CAP: usize = 1_000;

// ---------------------------------------------------------------------------
// Censor local-AI provider config (`config.json` `censorLocalAi`).
// ---------------------------------------------------------------------------

/// Which local tier-2 (Gemma) provider Censor drives. Default is [`Ollama`], today's
/// behavior — a `config.json` with no `censorLocalAi` key resolves to it with ZERO
/// migration. [`Omlx`] points at a local OpenAI-compatible oMLX server instead.
///
/// [`Ollama`]: CensorAiProvider::Ollama
/// [`Omlx`]: CensorAiProvider::Omlx
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CensorAiProvider {
    Ollama,
    Omlx,
}

/// The parsed `censorLocalAi` config. PRIVACY: for the oMLX provider, `base_url` is a
/// VALIDATED loopback origin (see [`validate_censor_local_ai`]) — file content sent to
/// the model can never leave the device. Fields stay `Option` so the Ollama default
/// (the common case) carries no base/model and serializes to just `{ provider:"ollama" }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CensorLocalAi {
    pub provider: CensorAiProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// OLLAMA-ONLY user override for the Gemma model tag (`ollamaModel` over IPC). When
    /// `Some` and valid it WINS the [`resolve_gemma_model`] chain outright (honored even if
    /// not yet in `/api/tags`). Validated with the SAME bare-tag char-class + length cap as
    /// the oMLX `model` (see [`validate_censor_local_ai`]); an empty/invalid value
    /// normalizes to `None`. NO-CHURN: an absent key parses (serde default), and `None`
    /// serializes to nothing, so an old config is never rewritten with this key. Ignored
    /// for the oMLX provider (oMLX uses `model`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_model: Option<String>,
}

impl Default for CensorLocalAi {
    /// The safe default: the Ollama provider with the built-in base + model. Returned
    /// whenever `censorLocalAi` is absent OR present-but-invalid (fail-safe).
    fn default() -> Self {
        Self {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: None,
        }
    }
}

impl CensorLocalAi {
    /// Resolve the effective base URL: the configured value or the provider default
    /// ([`OLLAMA_BASE`] / [`OMLX_DEFAULT_BASE`]). The client constructor re-clamps a
    /// non-loopback base, but a value here is already validated by
    /// [`validate_censor_local_ai`].
    pub fn effective_base(&self) -> String {
        match (&self.base_url, self.provider) {
            (Some(b), _) => b.clone(),
            (None, CensorAiProvider::Ollama) => OLLAMA_BASE.to_string(),
            (None, CensorAiProvider::Omlx) => OMLX_DEFAULT_BASE.to_string(),
        }
    }

    /// Resolve the effective model: the configured value or the provider default. oMLX
    /// has no built-in model, so a validated oMLX config always carries one; the fallback
    /// to [`GEMMA_MODEL`] only ever applies to the Ollama provider.
    pub fn effective_model(&self) -> String {
        self.model.clone().unwrap_or_else(|| GEMMA_MODEL.to_string())
    }
}

/// Validate a parsed `censorLocalAi` config, returning the NORMALIZED config or an error
/// describing the first problem. The caller ([`read_censor_local_ai`]) maps any error to
/// the safe [`CensorLocalAi::default`] (fail-safe — never send code to a bad endpoint):
///   - `ollama`: base/model optional; a present base must still be loopback (defense in
///     depth) and a present base/model are trimmed; empty → treated as absent.
///   - `omlx`: `base_url` AND `model` are REQUIRED (non-empty after trim) and `base_url`
///     MUST pass the loopback validator (http-only loopback host, valid optional port,
///     length cap, no control/invisible chars). The base is normalized (trailing slash
///     stripped) so `<base>/chat/completions` never double-slashes.
pub fn validate_censor_local_ai(cfg: &CensorLocalAi) -> Result<CensorLocalAi, String> {
    let base = cfg.base_url.as_deref().map(str::trim).unwrap_or("");
    let model = cfg.model.as_deref().map(str::trim).unwrap_or("");
    match cfg.provider {
        CensorAiProvider::Ollama => {
            // Optional fields; if a base is given it must still be loopback http.
            let base_opt = if base.is_empty() {
                None
            } else {
                if !is_loopback_base(base) {
                    return Err("censorLocalAi.baseUrl must be a loopback http origin for ollama."
                        .into());
                }
                // Strip a single trailing slash (mirror the oMLX normalization) so
                // `<base>/api/generate` never double-slashes (`…11434//api/generate`).
                Some(base.strip_suffix('/').unwrap_or(base).to_string())
            };
            let model_opt = if model.is_empty() {
                None
            } else {
                Some(model.to_string())
            };
            // OLLAMA-ONLY override: validate with the SAME bare-tag char-class + length cap
            // as the oMLX model so every model id on this machine satisfies one rule. An
            // empty-after-trim value normalizes to None; a present-but-invalid value
            // (spaces, control/bidi chars, overlong) is REJECTED so the resolver never
            // drives a malformed tag (the reader maps the Err to the safe default).
            let ollama_model = cfg.ollama_model.as_deref().map(str::trim).unwrap_or("");
            let ollama_model_opt = if ollama_model.is_empty() {
                None
            } else {
                if ollama_model.len() > CENSOR_OMLX_MODEL_MAX_LEN {
                    return Err(format!(
                        "censorLocalAi.ollamaModel must be at most {CENSOR_OMLX_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_omlx_model(ollama_model) {
                    return Err(
                        "censorLocalAi.ollamaModel must be a bare tag (letters, digits, . _ : / -)."
                            .into(),
                    );
                }
                Some(ollama_model.to_string())
            };
            Ok(CensorLocalAi {
                provider: CensorAiProvider::Ollama,
                base_url: base_opt,
                model: model_opt,
                ollama_model: ollama_model_opt,
            })
        }
        CensorAiProvider::Omlx => {
            if base.is_empty() {
                return Err("oMLX censorLocalAi requires a base URL.".into());
            }
            if model.is_empty() {
                return Err("oMLX censorLocalAi requires a model.".into());
            }
            if model.len() > CENSOR_OMLX_MODEL_MAX_LEN {
                return Err(format!(
                    "oMLX censorLocalAi model must be at most {CENSOR_OMLX_MODEL_MAX_LEN} characters."
                ));
            }
            // Same bare-token char-class as the mini-coder oMLX model validator
            // (`mini_coder::is_valid_model`): first char alnum, rest in
            // `[A-Za-z0-9._:/-]`. `org/name` HF-style paths stay valid (the `/` is
            // allowed); whitespace, control, bidi and shell metachars are rejected so
            // all oMLX model validators (mini Rust / Censor Rust / both TS) agree.
            if !is_valid_omlx_model(model) {
                return Err(
                    "oMLX censorLocalAi model must be a bare tag (letters, digits, . _ : / -)."
                        .into(),
                );
            }
            let normalized_base = validate_omlx_base_for_censor(base)?;
            Ok(CensorLocalAi {
                provider: CensorAiProvider::Omlx,
                base_url: Some(normalized_base),
                model: Some(model.to_string()),
                // oMLX uses `model`; the Ollama-only override is dropped so an oMLX config
                // never carries a stray `ollamaModel` (it would never be read).
                ollama_model: None,
            })
        }
    }
}

/// Validate + normalize an oMLX base URL for Censor. MIRRORS
/// `backend::mini_coder::validate_omlx_base_url` (loopback http-only host with a valid
/// optional `:port` via [`is_loopback_omlx_base`], length cap, no control/bidi/invisible
/// chars, trailing slash stripped). Kept here rather than shared because the two live in
/// different modules with different error wording; if the rules ever diverge, reconcile
/// them — every oMLX base on this machine must satisfy the same loopback guarantee.
fn validate_omlx_base_for_censor(base: &str) -> Result<String, String> {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return Err("oMLX censorLocalAi requires a base URL.".into());
    }
    if trimmed.len() > OMLX_BASE_URL_MAX_LEN {
        return Err(format!(
            "oMLX base URL must be at most {OMLX_BASE_URL_MAX_LEN} characters."
        ));
    }
    // Reject control / bidi / invisible chars using the EXACT same blocklist as the
    // mini-coder oMLX validator (a loopback URL is plain ASCII; anything else is suspect).
    if trimmed
        .chars()
        .any(crate::backend::mini_coder::is_forbidden_command_char)
    {
        return Err("oMLX base URL must not contain control, bidi or invisible characters.".into());
    }
    if !is_loopback_omlx_base(trimmed) {
        return Err(
            "oMLX base URL must be a loopback http origin (localhost, 127.0.0.1 or [::1]) with a valid optional :port."
                .into(),
        );
    }
    // Strip a single trailing slash so `<base>/chat/completions` is clean.
    Ok(trimmed.strip_suffix('/').unwrap_or(trimmed).to_string())
}

/// A bare oMLX model token: first char alnum, rest in `[A-Za-z0-9._:/-]`. No
/// whitespace/control/metachars. Delegates to the SHARED `mini_coder::is_valid_model`
/// (rather than a local copy) so the mini-coder and Censor oMLX model validators can
/// NEVER drift — every oMLX model id on this machine (mini Rust / Censor Rust / both TS)
/// satisfies the same char-class. `org/name` HF-style paths are valid (the `/` is
/// allowed). Assumes the input is already trimmed (the caller trims first).
fn is_valid_omlx_model(model: &str) -> bool {
    crate::backend::mini_coder::is_valid_model(model)
}

/// Errors a Gemma client can surface. Deliberately coarse + content-free: the
/// caller logs identity + path only, never the underlying message (which could echo
/// the request/response body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GemmaError {
    /// The daemon was unreachable / the request failed at the transport layer.
    Transport,
    /// The request exceeded [`GEMMA_GENERATE_TIMEOUT`].
    Timeout,
    /// The daemon answered with a non-success status.
    Status(u16),
    /// The response body could not be read / decoded.
    Decode,
}

impl std::fmt::Display for GemmaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GemmaError::Transport => write!(f, "transport error"),
            GemmaError::Timeout => write!(f, "timeout"),
            GemmaError::Status(c) => write!(f, "status {c}"),
            GemmaError::Decode => write!(f, "decode error"),
        }
    }
}

/// Injectable client seam so the layer is testable WITHOUT a network / a running
/// Ollama. The real impl ([`OllamaClient`]) talks loopback HTTP; tests use a stub
/// returning canned probe results / generations.
pub trait GemmaClient: Send + Sync {
    /// Cheap availability probe: is Ollama reachable AND is [`GEMMA_MODEL`] present?
    /// Any failure (unreachable / model absent / decode) → `false`.
    fn probe(&self) -> bool;

    /// Run ONE generation for `prompt`, returning the model's raw text response (the
    /// `response` field of the Ollama `/api/generate` reply). The caller parses it
    /// defensively via [`parse_gemma`].
    fn generate(&self, prompt: &str) -> Result<String, GemmaError>;

    /// A stable, content-free IDENTITY label for the provider behind this client
    /// (`"ollama"` / `"omlx"`). Used ONLY for the once-per-session available/unavailable
    /// log line and for testing the factory wiring. NEVER includes the base URL, model,
    /// file content, or any path — identity only (the privacy header forbids logging the
    /// endpoint/content).
    fn provider_label(&self) -> &'static str;

    /// The EFFECTIVE model tag this client drives (e.g. `"gemma4:e4b"` for the Ollama
    /// default, or the configured oMLX model). Used ONLY alongside [`provider_label`] in
    /// the once-per-session available/unavailable log line so a failure can be triaged
    /// against the model ACTUALLY in use rather than the hardcoded [`GEMMA_MODEL`]
    /// constant. Returns an OWNED `String` because the Ollama client resolves its model at
    /// runtime via the [`resolve_gemma_model`] chain (it may not be a borrow of a stored
    /// field). A MODEL TAG ONLY — never the base URL, file content, or any path (the
    /// privacy header forbids logging the endpoint/content). The model tag is user/
    /// config-supplied, not file content, so it is safe to log as an identity marker.
    fn model_label(&self) -> String;

    /// A FULL, content-free cache-key identity for this client: provider + effective
    /// base URL + effective model, joined as `"{provider}|{base}|{model}"`. Used ONLY as
    /// the in-memory key of `CensorState`'s availability-probe cache so that changing the
    /// oMLX base OR model (even within the SAME provider) forces a re-probe instead of
    /// silently reusing the previous endpoint's availability.
    ///
    /// PRIVACY: unlike [`provider_label`]/[`model_label`] this INCLUDES the base URL, so it
    /// MUST NEVER be logged or surfaced — it lives only as an opaque `String` map key in
    /// process memory (the base is always a validated loopback origin, but the privacy
    /// header still forbids emitting the endpoint anywhere observable). The default
    /// composes provider+model with an empty base; the real clients override it to fold
    /// in their base so a base change is detected.
    fn cache_identity(&self) -> String {
        format!("{}||{}", self.provider_label(), self.model_label())
    }
}

/// The real loopback Ollama client. Holds a `reqwest::blocking::Client` with no
/// global timeout — each request sets its own (probe vs generate have different
/// budgets), mirroring `python_oracle.rs`. Blocking is fine: it runs on the Censor
/// serialized worker thread, never the UI thread.
pub struct OllamaClient {
    http: reqwest::blocking::Client,
    base: String,
    /// The user-configured `ollamaModel` override (already validated), or `None` for the
    /// default. NOT the model actually driven — see [`OllamaClient::resolved_model`], which
    /// runs the [`resolve_gemma_model`] chain over the live `/api/tags` list.
    configured_model: Option<String>,
    /// Memoized result of the resolution chain so the `/api/tags` fetch + resolve happen
    /// AT MOST once per client (the probe fetch is reused; a later `generate`/`model_label`
    /// on the same client reuses the cached resolution instead of re-fetching). `Mutex`
    /// rather than `OnceCell` to keep the type simple and `Send + Sync` without extra deps.
    resolved_model: std::sync::Mutex<Option<String>>,
    generate_timeout: Duration,
    probe_timeout: Duration,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OllamaClient {
    /// Build a client pointed at the loopback Ollama with the default model +
    /// timeouts.
    pub fn new() -> Self {
        Self::with_config(
            OLLAMA_BASE,
            None,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        )
    }

    /// Construct with explicit config (used by `new`; `pub(crate)` — there are NO
    /// external callers, and the loopback guarantee below must not be bypassable from
    /// outside the censor module).
    ///
    /// PRIVACY GUARD: `base` MUST be a loopback URL (see the module privacy header).
    /// A non-loopback base is rejected at the source — we fall back to [`OLLAMA_BASE`]
    /// rather than ever pointing the client at a remote host, so file content can
    /// never leave the device even if a caller passes a bad base. Two hardenings on
    /// the blocking client: NO redirect-following (a redirect could send the request
    /// body off-box; loopback Ollama never legitimately redirects), and a per-request
    /// timeout (set by each call) rather than a global one.
    pub(crate) fn with_config(
        base: &str,
        configured_model: Option<&str>,
        generate_timeout: Duration,
        probe_timeout: Duration,
    ) -> Self {
        // Reject any non-loopback base: clamp to the safe default rather than honoring
        // a host that would let content leave the device.
        let base = if is_loopback_base(base) {
            base.to_string()
        } else {
            eprintln!(
                "censor gemma: refusing non-loopback Ollama base; falling back to loopback default"
            );
            OLLAMA_BASE.to_string()
        };
        // A builder failure (TLS init, etc.) is implausible for a plain loopback
        // client; fall back to the default client so we never panic at startup.
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            http,
            base,
            configured_model: configured_model.map(str::to_string),
            resolved_model: std::sync::Mutex::new(None),
            generate_timeout,
            probe_timeout,
        }
    }

    /// Fetch the `/api/tags` model-name list from the loopback daemon, capped + decoded
    /// the SAME way the probe reads its body. Returns `None` on any failure (unreachable /
    /// non-success / over-cap / undecodable) so the caller degrades cleanly. The names are
    /// the union of each entry's `name` and `model` fields (Ollama reports the tag in
    /// `name`; some versions also carry `model`), de-duplicated by simple membership in the
    /// resolver. PRIVACY: a `GET` of public model metadata — no file content leaves here.
    fn fetch_tags(&self) -> Option<Vec<String>> {
        let url = format!("{}/api/tags", self.base);
        let resp = self.http.get(&url).timeout(self.probe_timeout).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().ok()?;
        if bytes.len() > RESPONSE_BODY_CAP {
            return None;
        }
        let body: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(tag_names(&body))
    }

    /// The EFFECTIVE model tag this client drives. IO-FREE: [`probe`] is the SOLE
    /// memoization point — it fetches `/api/tags` once and stores the resolved tag — so this
    /// method only ever reads the memo (set by probe) or resolves WITHOUT a second network
    /// call. A configured override short-circuits to itself (no tags needed). When the memo
    /// is empty AND there is no override (probe has not run or could not reach the daemon),
    /// it returns the pessimistic default [`GEMMA_MODEL`] rather than firing a 5s `/api/tags`
    /// fetch off the worker path — generate would then surface the unavailable model exactly
    /// as today, never a crash.
    ///
    /// INVARIANT: [`probe`] MUST run before [`generate`] (the worker probes for availability
    /// before generating), so the memo is populated on the live path; this method never does
    /// IO so it is safe on the degraded-log path too.
    fn resolved_model(&self) -> String {
        if let Ok(guard) = self.resolved_model.lock() {
            if let Some(m) = guard.as_ref() {
                return m.clone();
            }
        }
        // No memo yet. A configured override wins without any IO (it does not need to be in
        // the tags). Otherwise return the default WITHOUT fetching — probe() is the only
        // place allowed to hit the network (see the invariant above).
        let resolved = resolve_gemma_model(self.configured_model.as_deref(), &[]);
        if let Ok(mut guard) = self.resolved_model.lock() {
            *guard = Some(resolved.clone());
        }
        resolved
    }
}

/// The loopback oMLX client — an alternative tier-2 provider that talks to a local
/// oMLX (MLX) server exposing an OpenAI-compatible HTTP API at `<base>/models` and
/// `<base>/chat/completions`. It implements the SAME [`GemmaClient`] trait as
/// [`OllamaClient`] so the rest of Censor is provider-agnostic; the model's raw text is
/// fed to the EXISTING [`parse_gemma`] unchanged (same parsing, same secret redaction).
///
/// PRIVACY: identical posture to [`OllamaClient`]. The base is loopback-clamped at the
/// source (see [`OmlxClient::with_config`]) so file content sent in the prompt can never
/// leave the device; the blocking client follows NO redirects and sets a per-request
/// timeout; the response body is size-capped before parsing.
pub struct OmlxClient {
    http: reqwest::blocking::Client,
    base: String,
    model: String,
    generate_timeout: Duration,
    probe_timeout: Duration,
}

impl OmlxClient {
    /// Build an oMLX client for `base_url` + `model` with the default Gemma timeouts.
    /// Both are caller-supplied (from the validated `censorLocalAi` config); the base is
    /// loopback-clamped inside [`Self::with_config`].
    ///
    /// TEST-ONLY: the live factory ([`build_gemma_client`]) calls [`Self::with_config`]
    /// directly (it threads the shared timeouts), so this convenience constructor has no
    /// non-test caller. `#[cfg(test)]` (rather than `#[allow(dead_code)]`) keeps the
    /// dead-code detector live on the rest of the type — this is NOT a wiring gap, just a
    /// test ergonomic mirroring `OllamaClient::new` (which stays alive only via its
    /// `impl Default`, which `OmlxClient` cannot have — it needs base/model args).
    #[cfg(test)]
    pub fn new(base_url: &str, model: &str) -> Self {
        Self::with_config(
            base_url,
            model,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        )
    }

    /// Construct with explicit config. `pub(crate)` for the SAME reason as
    /// [`OllamaClient::with_config`]: the loopback guarantee must not be bypassable from
    /// outside the censor module.
    ///
    /// PRIVACY GUARD: `base` MUST be a loopback URL (http only). A non-loopback base is
    /// clamped to [`OMLX_DEFAULT_BASE`] rather than ever pointing the client at a remote
    /// host — file content can never leave the device even if a caller passes a bad base.
    /// Same two hardenings as the Ollama client: NO redirect-following and a per-request
    /// timeout set by each call.
    pub(crate) fn with_config(
        base: &str,
        model: &str,
        generate_timeout: Duration,
        probe_timeout: Duration,
    ) -> Self {
        // SELF-CONTAINED clamp (max-recall FIX 7): accept `base` ONLY if it satisfies the
        // FULL config-time validator's rules — loopback http origin AND within the length
        // cap AND free of control/bidi/invisible chars — rather than relying on the caller
        // having already validated it (the old check tested loopback only, "safe by
        // emergent caller invariant"). Any failure clamps to the safe loopback default so
        // the type can NEVER be constructed pointing at a remote/oversized/obfuscated base,
        // regardless of how it is called.
        let base = if base.len() <= OMLX_BASE_URL_MAX_LEN
            && !base
                .chars()
                .any(crate::backend::mini_coder::is_forbidden_command_char)
            && is_loopback_omlx_base(base)
        {
            base.to_string()
        } else {
            eprintln!(
                "censor gemma: refusing invalid oMLX base; falling back to loopback default"
            );
            OMLX_DEFAULT_BASE.to_string()
        };
        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            http,
            base,
            model: model.to_string(),
            generate_timeout,
            probe_timeout,
        }
    }
}

impl GemmaClient for OmlxClient {
    fn probe(&self) -> bool {
        // GET <base>/models → OpenAI list-models { "data": [ { "id": "<model>" }, ... ] }.
        // Reachable AND our configured model present → available (mirrors the Ollama
        // probe's "reachable AND model present" so the tier degrades identically when the
        // server is up but the model isn't pulled).
        let url = format!("{}/models", self.base);
        let resp = match self.http.get(&url).timeout(self.probe_timeout).send() {
            Ok(r) => r,
            Err(_) => return false,
        };
        if !resp.status().is_success() {
            return false;
        }
        // Read the body with the SAME hard size cap as generate() before deserializing —
        // `resp.json()` would buffer a runaway `/models` response unbounded.
        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(_) => return false,
        };
        if bytes.len() > RESPONSE_BODY_CAP {
            return false;
        }
        let body: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return false,
        };
        openai_model_present(&body, &self.model)
    }

    fn generate(&self, prompt: &str) -> Result<String, GemmaError> {
        // POST <base>/chat/completions { model, messages:[{role:user, content:prompt}],
        // stream:false, temperature:0.1 }. Same low temperature as the Ollama call for
        // conservative, deterministic-ish output.
        let url = format!("{}/chat/completions", self.base);
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [ { "role": "user", "content": prompt } ],
            "stream": false,
            "temperature": 0.1
        });
        let resp = self
            .http
            .post(&url)
            .timeout(self.generate_timeout)
            .json(&payload)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    GemmaError::Timeout
                } else {
                    GemmaError::Transport
                }
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GemmaError::Status(status.as_u16()));
        }
        // Same OOM defense as the Ollama path: read the body with a hard size cap BEFORE
        // deserializing (`resp.json()` would buffer unbounded).
        let bytes = resp.bytes().map_err(|_| GemmaError::Decode)?;
        parse_openai_chat_body(&bytes)
    }

    fn provider_label(&self) -> &'static str {
        "omlx"
    }

    fn model_label(&self) -> String {
        self.model.clone()
    }

    fn cache_identity(&self) -> String {
        // Fold in the base so changing the oMLX base (same provider) re-probes. NEVER
        // logged — opaque in-memory cache key only (see the trait doc).
        format!("omlx|{}|{}", self.base, self.model)
    }
}

/// Is `base` a loopback HTTP origin? Only `http://127.x`, `http://[::1]`, and
/// `http://localhost` are accepted (with or without a `:port` / trailing path). The
/// privacy guarantee (file content never leaves the device) depends on this: any
/// other host could route the request — and the file content in the prompt — off the
/// machine. PURE + conservative: anything we don't positively recognize as loopback
/// is rejected. We deliberately do NOT accept `https` (loopback Ollama is plain HTTP)
/// or a bare host without the `http://` scheme.
pub(crate) fn is_loopback_base(base: &str) -> bool {
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    // Strip an optional path / query so we look only at the authority component.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority_is_loopback(authority)
}

/// Is `authority` (the host[:port] component, path/query already stripped) a loopback
/// origin? PURE + conservative — shared by [`is_loopback_base`] (the Ollama path) and
/// [`is_loopback_omlx_base`] (the oMLX path; both http-only) so the HOST rule is defined
/// in EXACTLY one place and the two callers can never drift. The oMLX path layers an
/// additional `:port` validation on top (see [`is_loopback_omlx_base`]) that the Ollama
/// path deliberately does NOT apply, so this shared fn stays port-agnostic. Accepts only
/// `localhost`, IPv4 in `127.0.0.0/8` (parsed, so `127.0.0.1.evil.com` is rejected),
/// and IPv6 `[::1]`, each with an optional `:port`. Rejects any `@` userinfo trick
/// (`127.0.0.1@evil.com`, `[::1]:8000@evil.com`) — the real host would be after the `@`.
fn authority_is_loopback(authority: &str) -> bool {
    // IPv6 loopback: `[::1]` optionally followed by `:port`. Reject a userinfo trick
    // (`[::1]:8000@evil.com` / `[::1]:@evil.com`): an `@` in the remainder means the real
    // host is after the `@`, which would route file content off-box (privacy hole).
    if let Some(after) = authority.strip_prefix("[::1]") {
        return !after.contains('@') && (after.is_empty() || after.starts_with(':'));
    }
    // Reject a userinfo trick (`127.0.0.1@evil.com`): the real host is after the `@`.
    if authority.contains('@') {
        return false;
    }
    // Split off an optional `:port` from a host:port authority. IPv4/hostname hosts
    // contain no `:` in the host part, so the first `:` (if any) starts the port.
    let host = authority.split(':').next().unwrap_or("");
    if host == "localhost" {
        return true;
    }
    // IPv4 loopback: the host must PARSE as an Ipv4Addr in 127.0.0.0/8. A `starts_with
    // ("127.")` check would wrongly accept `127.0.0.1.evil.com`; parsing rejects it.
    host.parse::<std::net::Ipv4Addr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Is `base` a loopback origin acceptable for the oMLX (OpenAI-compatible) client?
/// Same HOST rule as [`is_loopback_base`] (via [`authority_is_loopback`]) but layered
/// with a `:port` check (via [`crate::backend::mini_coder::is_valid_optional_port`]) that
/// the Ollama path deliberately does NOT apply. oMLX is HTTP-ONLY on loopback (like
/// Ollama): `https://` is rejected because a self-signed TLS cert on a loopback oMLX
/// server would fail reqwest's default verification and silently disable the tier. This
/// intentionally MIRRORS `backend::mini_coder::validate_omlx_base_url` (same host rule,
/// same http-only scheme, same `:port` rule); if the two ever diverge, reconcile them —
/// every oMLX base on this machine must satisfy the same loopback guarantee, so file
/// content sent to the model can never leave the device.
pub(crate) fn is_loopback_omlx_base(base: &str) -> bool {
    // http only (loopback, like Ollama) — reject https (self-signed-TLS silent-degrade trap).
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Shared HOST rule (kept byte-identical with the Ollama path), PLUS the oMLX-only
    // port validation layered on top — leaving `authority_is_loopback` / the Ollama path
    // untouched.
    authority_is_loopback(authority) && omlx_authority_port_is_valid(authority)
}

/// oMLX-only `:port` validation layered on top of [`authority_is_loopback`]. Extracts the
/// optional port from a loopback `authority` (`[::1]`, `[::1]:8000`, `localhost:8000`,
/// `127.0.0.1`, …) and defers to [`crate::backend::mini_coder::is_valid_optional_port`] so
/// the two oMLX validators (Censor + mini_coder) apply the SAME rule. PRECONDITION: only
/// meaningful for an authority `authority_is_loopback` already accepted (no `@` userinfo).
fn omlx_authority_port_is_valid(authority: &str) -> bool {
    use crate::backend::mini_coder::is_valid_optional_port;
    // IPv6 loopback: the host is `[::1]`; an optional `:port` follows the closing bracket.
    if let Some(after) = authority.strip_prefix("[::1]") {
        // `after` is "" (no port) or ":<port>"; strip the ':' before validating.
        return is_valid_optional_port(after.strip_prefix(':'));
    }
    // IPv4 / hostname: the host has no ':' so the first ':' (if any) starts the port.
    let mut parts = authority.splitn(2, ':');
    let _host = parts.next();
    is_valid_optional_port(parts.next())
}

impl GemmaClient for OllamaClient {
    fn probe(&self) -> bool {
        // GET /api/tags → { "models": [ { "name": "gemma4:e4b", ... }, ... ] }.
        // Reachable AND the RESOLVED model present → available. The fetch here also feeds
        // (and memoizes) the resolution chain, so probe and the worker's generate agree on
        // ONE model without a second fetch.
        let Some(tags) = self.fetch_tags() else {
            return false;
        };
        // Resolve over the just-fetched tags and memoize so generate()/model_label() reuse
        // it. A configured override short-circuits (wins regardless of tags); the default
        // path picks e4b → e2b (upgrade safety) → e4b.
        let resolved = resolve_gemma_model(self.configured_model.as_deref(), &tags);
        if let Ok(mut guard) = self.resolved_model.lock() {
            *guard = Some(resolved.clone());
        }
        // Reachable AND the resolved model present → available (a configured override that
        // is not yet pulled correctly reports unavailable: the tier degrades, no crash).
        tags.iter().any(|t| t == &resolved)
    }

    fn generate(&self, prompt: &str) -> Result<String, GemmaError> {
        // POST /api/generate { model, prompt, stream:false, options:{temperature} }.
        // A low temperature keeps the model conservative + deterministic-ish. The model is
        // the resolution-chain result (reused from the probe's fetch when available).
        let model = self.resolved_model();
        let url = format!("{}/api/generate", self.base);
        let payload = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "options": { "temperature": 0.1 }
        });
        let resp = self
            .http
            .post(&url)
            .timeout(self.generate_timeout)
            .json(&payload)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    GemmaError::Timeout
                } else {
                    GemmaError::Transport
                }
            })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(GemmaError::Status(status.as_u16()));
        }
        // BLOCKER: read the body into memory with a hard size cap BEFORE deserializing.
        // `resp.json()` would buffer the WHOLE body unbounded — a runaway/looping local
        // model can emit hundreds of MiB and OOM us. `resp.bytes()` still buffers, but
        // we reject anything over the cap before parsing (and Ollama with `stream:false`
        // sends a single small JSON object, so a legitimate body is far under 1 MiB).
        let bytes = resp.bytes().map_err(|_| GemmaError::Decode)?;
        parse_generate_body(&bytes)
    }

    fn provider_label(&self) -> &'static str {
        "ollama"
    }

    fn model_label(&self) -> String {
        // Log identity ONLY — must NOT trigger IO (it runs on the degraded-log path). Prefer
        // the memoized resolution (set by probe()); else the configured override; else the
        // default constant. The resolved value is what probe() already computed, so the log
        // surfaces the model ACTUALLY in use without a second `/api/tags` fetch.
        if let Ok(guard) = self.resolved_model.lock() {
            if let Some(m) = guard.as_ref() {
                return m.clone();
            }
        }
        self.configured_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(GEMMA_MODEL)
            .to_string()
    }

    fn cache_identity(&self) -> String {
        // Fold in the base + configured override so a custom Ollama base OR model change
        // re-probes. Uses the CONFIGURED override (not the resolved tag) so the key is
        // IO-free and stable regardless of probe ordering. NEVER logged — opaque in-memory
        // cache key only (see the trait doc).
        format!(
            "ollama|{}|{}",
            self.base,
            self.configured_model.as_deref().unwrap_or("")
        )
    }
}

/// Parse an Ollama `/api/generate` (`stream:false`) response body into the model's
/// `response` text, enforcing [`RESPONSE_BODY_CAP`]. PURE + DEFENSIVE:
///   - a body over the cap → [`GemmaError::Decode`] (an over-large body is treated as
///     undecodable rather than slurped/parsed — bounds memory + parse cost);
///   - non-JSON / a missing `response` field → empty string (the downstream parser
///     yields no findings; never a panic).
fn parse_generate_body(bytes: &[u8]) -> Result<String, GemmaError> {
    if bytes.len() > RESPONSE_BODY_CAP {
        return Err(GemmaError::Decode);
    }
    let body: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| GemmaError::Decode)?;
    // The non-streaming reply puts the text in `response`.
    Ok(body
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Parse an OpenAI-compatible `/chat/completions` (`stream:false`) response body into
/// the assistant message text, enforcing [`RESPONSE_BODY_CAP`]. PURE + DEFENSIVE,
/// mirroring [`parse_generate_body`]:
///   - a body over the cap → [`GemmaError::Decode`] (rejected before deserializing —
///     bounds memory + parse cost against a runaway local model);
///   - non-JSON → [`GemmaError::Decode`];
///   - a missing/empty `choices[0].message.content` → empty string (the downstream
///     parser yields no findings; never a panic). The extracted text feeds the EXISTING
///     [`parse_gemma`] unchanged, so the same secret redaction + caps apply.
fn parse_openai_chat_body(bytes: &[u8]) -> Result<String, GemmaError> {
    if bytes.len() > RESPONSE_BODY_CAP {
        return Err(GemmaError::Decode);
    }
    let body: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| GemmaError::Decode)?;
    // OpenAI envelope: { "choices": [ { "message": { "content": "..." } } ] }.
    Ok(body
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Is `model` present in an OpenAI list-models body (`{ "data": [ { "id": "..." } ] }`)?
/// Matches the configured id exactly (we do not loosely match a different model). Pure +
/// defensive: a missing/empty `data` array → false (server up but model not loaded ⇒
/// tier disabled, same as Ollama-model-absent).
fn openai_model_present(body: &serde_json::Value, model: &str) -> bool {
    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return false;
    };
    data.iter().any(|m| {
        m.get("id")
            .and_then(|n| n.as_str())
            .map(|id| id == model)
            .unwrap_or(false)
    })
}

/// Collect every model tag NAME from an Ollama `/api/tags` body
/// (`{ "models": [ { "name": "gemma4:e4b", "model": "gemma4:e4b" }, ... ] }`). Returns the
/// union of each entry's `name` and `model` string fields (Ollama reports the tag in
/// `name`; some versions also echo `model`), so the [`resolve_gemma_model`] chain sees
/// every available tag. PURE + defensive: a missing/empty `models` array → empty Vec.
/// Empty/absent fields are skipped. The result feeds the resolver's exact-match membership.
fn tag_names(body: &serde_json::Value) -> Vec<String> {
    let Some(models) = body.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::with_capacity(models.len());
    for m in models {
        for key in ["name", "model"] {
            if let Some(s) = m.get(key).and_then(|v| v.as_str()) {
                let s = s.trim();
                if !s.is_empty() && !names.iter().any(|n| n == s) {
                    names.push(s.to_string());
                }
            }
        }
    }
    names
}

/// Build the tier-2 (Gemma) client for a RESOLVED [`CensorLocalAi`] config. This is the
/// single provider-selection point Censor uses at its probe + worker construction sites
/// (oMLX-P5), so the probe and the worker that follows always agree on ONE provider when
/// handed the SAME config snapshot:
///   - [`CensorAiProvider::Ollama`] (the default — a config with no `censorLocalAi`
///     resolves here) → an [`OllamaClient`] at the effective base/model. With the default
///     config this is byte-identical to the previous hardcoded `OllamaClient::new()`
///     (same [`OLLAMA_BASE`], [`GEMMA_MODEL`], and the default generate/probe timeouts).
///   - [`CensorAiProvider::Omlx`] → an [`OmlxClient`] at the effective base/model (the
///     base is loopback-clamped inside [`OmlxClient::with_config`], a privacy fail-safe).
///
/// PRIVACY: `cfg` is expected to be the output of [`validate_censor_local_ai`] (via
/// `read_censor_local_ai`), so the base is already validated loopback; the client
/// constructor re-clamps a non-loopback base as defense in depth. The base/model are
/// NEVER logged here — provider identity only (see [`GemmaClient::provider_label`]).
pub(crate) fn build_gemma_client(cfg: &CensorLocalAi) -> Box<dyn GemmaClient> {
    let base = cfg.effective_base();
    match cfg.provider {
        // The Ollama client takes the CONFIGURED override (`ollama_model`, may be `None`)
        // and resolves the effective tag itself via the [`resolve_gemma_model`] chain over
        // the live `/api/tags` list — so an existing install that only pulled `gemma4:e2b`
        // keeps its tier after the default bumped to `gemma4:e4b`.
        CensorAiProvider::Ollama => Box::new(OllamaClient::with_config(
            &base,
            cfg.ollama_model.as_deref(),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        )),
        // oMLX has no `/api/tags` equivalent; its model is REQUIRED + validated, so the
        // configured-or-default `effective_model` is used verbatim (no e2b fallback).
        CensorAiProvider::Omlx => Box::new(OmlxClient::with_config(
            &base,
            &cfg.effective_model(),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        )),
    }
}

/// Probe availability via `client`, returning `true` iff Ollama is reachable AND
/// [`GEMMA_MODEL`] is present. Any failure → `false` (tier disabled). This is the
/// single decision point the orchestrator/state caches so the cost is paid ONCE per
/// watch session, not per file. Logging is the CALLER's job (so it can log once).
pub fn probe_available(client: &dyn GemmaClient) -> bool {
    client.probe()
}

// ---------------------------------------------------------------------------
// PURE prompt builder.
// ---------------------------------------------------------------------------

/// The fixed, conservative system instruction. Deliberately narrow: only file-local
/// non-deterministic semantic smells a linter cannot find, strict JSON-array output,
/// say nothing when unsure. Kept as a constant so the policy is auditable in one
/// place and never drifts between builds.
const SYSTEM_INSTRUCTION: &str = "\
You are a careful code reviewer. You review exactly ONE file. Report ONLY file-local, \
non-deterministic semantic smells that a linter or type checker CANNOT catch, such as:\n\
- inverted logic (a condition or boolean that is backwards)\n\
- an off-by-one in custom (non-library) logic\n\
- a copy-paste leftover: the wrong variable used that still compiles\n\
- swapped call arguments (right types, wrong order)\n\
- a comment that contradicts what the code actually does\n\
- a swallowed error / empty catch that hides a failure\n\
- a missing guard on a logically-empty or null value\n\
Do NOT report style, formatting, types, dead code, duplication, naming, performance, \
or anything a linter or compiler already finds. If you are not confident, report \
nothing. Prefer silence over a weak guess.\n\
Output ONLY a JSON array (no prose, no markdown fences) of objects with EXACTLY these \
keys: {\"line\": <integer 1-based>, \"title\": <short string>, \"body\": <one-sentence \
string>, \"severity\": one of \"high\" | \"medium\" | \"low\"}. If there is nothing to \
report, output [].";

/// Build the full prompt for one file. PURE (no IO). Layers:
///   1. the fixed [`SYSTEM_INSTRUCTION`];
///   2. the file path + its (capped) content, fenced so the model can locate lines;
///   3. the deterministic findings rendered as "ALREADY KNOWN — do NOT repeat", so
///      the model spends its small budget on the residual rather than re-reporting
///      what the linters already caught.
///
/// The file content is capped at [`MAX_FILE_CHARS`] (a truncation marker is appended
/// so the model knows it is partial). Deterministic findings are listed compactly as
/// `line: title` (already-redacted by the runners, but we never echo their body to
/// keep the prompt small and to avoid re-injecting any tool text).
pub fn build_prompt(
    file_rel_path: &str,
    file_content: &str,
    deterministic_findings: &[RawFinding],
) -> String {
    let mut p = String::with_capacity(file_content.len().min(MAX_FILE_CHARS) + 2048);
    p.push_str(SYSTEM_INSTRUCTION);
    p.push_str("\n\nFILE: ");
    p.push_str(file_rel_path);
    p.push_str("\n--- BEGIN FILE CONTENT ---\n");
    if file_content.chars().count() > MAX_FILE_CHARS {
        let truncated: String = file_content.chars().take(MAX_FILE_CHARS).collect();
        p.push_str(&truncated);
        p.push_str("\n--- FILE CONTENT TRUNCATED ---\n");
    } else {
        p.push_str(file_content);
        p.push('\n');
    }
    p.push_str("--- END FILE CONTENT ---\n\n");

    if deterministic_findings.is_empty() {
        p.push_str("ALREADY KNOWN — do NOT repeat these: (none)\n");
    } else {
        p.push_str("ALREADY KNOWN — do NOT repeat these:\n");
        for f in deterministic_findings {
            let line = f
                .line
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string());
            // DEFENSE-IN-DEPTH: re-run the title through `redact_secrets` rather than
            // trusting each runner to have redacted it — a secret must never reach the
            // prompt we hand the model. We deliberately omit the `body` entirely (a
            // second privacy measure: less tool text re-injected, and a smaller prompt
            // for the small-context model).
            p.push_str(&format!("- line {}: {}\n", line, redact_secrets(&f.title)));
        }
    }
    p.push_str("\nNow output ONLY the JSON array of NEW file-local smells you found.");
    p
}

// ---------------------------------------------------------------------------
// PURE output parser.
// ---------------------------------------------------------------------------

/// Parse a Gemma response into `RawFinding`s. PURE + DEFENSIVE — never panics on
/// adversarial / empty / non-JSON input:
///   - extracts the FIRST balanced `[` … `]` JSON array from the text (the model may
///     wrap it in prose or ```json fences);
///   - parses it as a JSON array; a parse failure → empty;
///   - drops malformed entries (missing keys, wrong types) silently;
///   - caps the count at [`MAX_GEMMA_FINDINGS`];
///   - maps `severity` via [`severity_from_token`] (defaulting Medium), category =
///     `Correctness` (Gemma findings are semantic), source = `"gemma"`;
///   - runs `title`/`body` through `redact_secrets` then `cap` (the model could echo
///     a secret it read from the file — strip it before it reaches a shard).
///
/// `file_rel_path` is stamped as the finding's file (forward-slash normalized by the
/// downstream `into_finding`); the model is not trusted to report a path.
pub fn parse_gemma(file_rel_path: &str, raw_response: &str) -> Vec<RawFinding> {
    let Some(array_text) = extract_json_array(raw_response) else {
        return Vec::new();
    };
    let value: serde_json::Value = match serde_json::from_str(&array_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    let mut out: Vec<RawFinding> = Vec::new();
    for item in items {
        if out.len() >= MAX_GEMMA_FINDINGS {
            break;
        }
        let Some(obj) = item.as_object() else {
            continue; // not an object → malformed, drop
        };
        // title is REQUIRED + non-empty (a finding with no title is noise).
        let title_raw = obj
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title_raw.is_empty() {
            continue;
        }
        let body_raw = obj
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        // line: accept an integer; clamp/ignore non-positive or absurd values → None.
        let line = obj.get("line").and_then(|v| v.as_u64()).and_then(|n| {
            if n >= 1 && n <= u32::MAX as u64 {
                Some(n as u32)
            } else {
                None
            }
        });
        let severity =
            severity_from_token(obj.get("severity").and_then(|v| v.as_str()).unwrap_or(""));

        // DEFENSE: redact a secret the model may have echoed from the file, THEN cap.
        let title = cap(&redact_secrets(title_raw), TITLE_CAP);
        let body = cap(&redact_secrets(body_raw), BODY_CAP);

        out.push(RawFinding {
            file: file_rel_path.to_string(),
            line,
            severity,
            category: Category::Correctness,
            source: "gemma".to_string(),
            title,
            body,
        });
    }
    out
}

/// Map a severity token from the model onto our `Severity`. Case-insensitive;
/// unknown / empty → `Medium` (conservative middle, matching the A1 normalizers).
fn severity_from_token(token: &str) -> Severity {
    match token.trim().to_ascii_lowercase().as_str() {
        "high" => Severity::High,
        "low" => Severity::Low,
        _ => Severity::Medium,
    }
}

/// Extract the first balanced top-level JSON array (`[` … matching `]`) from `text`,
/// tolerating surrounding prose / markdown fences. Tracks string + escape state so a
/// `]` INSIDE a string literal doesn't prematurely close the array. Returns the
/// array substring (inclusive of the brackets) or `None` if no balanced array exists.
fn extract_json_array(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    // `i` is the closing bracket; slice on the byte range (ASCII
                    // brackets, so this lands on char boundaries).
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// SEAM (design only — NOT wired): future bounded context injection.
// ---------------------------------------------------------------------------

/// SEAM for a FUTURE enhancement, intentionally returning `None` so it is a no-op
/// today. The idea: an orchestrator-side step would pre-fetch a small, bounded set
/// of symbol DEFINITIONS referenced by the file (resolved from the project index)
/// and inject them into the prompt as read-only context, so Gemma can reason about a
/// call it can't see the definition of — WITHOUT the model ever calling a tool
/// (the orchestrator does the fetch + injection; the model still sees only a static
/// prompt and reaches out nowhere). Left returning `None` until that capability is
/// designed; `build_prompt` would append the block when present.
#[allow(dead_code)] // SEAM: intentionally uncalled until the future context-injection step.
pub fn build_context_block(_root: &Path, _file_rel_path: &str) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// run_gemma: the thin glue (probe-gated → build → generate → parse).
// ---------------------------------------------------------------------------

/// Run the Gemma tier for ONE file. `available` is the cached probe result (the
/// orchestrator/state computes it once per watch session, never per file):
///   - `available == false` → return empty immediately (the tier is disabled; the
///     fine pass behaves exactly like deterministic-only A3);
///   - otherwise build the prompt, call `client.generate`, and parse the response.
///
/// On ANY generate error/timeout → empty + log ONCE (model name + the file's
/// project-relative path ONLY; NEVER the file content or the model output, per the
/// module privacy header). `_root` is accepted for the future context seam; it is
/// unused today.
pub fn run_gemma(
    client: &dyn GemmaClient,
    available: bool,
    _root: &Path,
    file_rel_path: &str,
    file_content: &str,
    deterministic: &[RawFinding],
) -> Vec<RawFinding> {
    if !available {
        return Vec::new();
    }
    let prompt = build_prompt(file_rel_path, file_content, deterministic);
    match client.generate(&prompt) {
        Ok(response) => parse_gemma(file_rel_path, &response),
        Err(e) => {
            // Identity (provider + the model ACTUALLY in use) + path ONLY — never the
            // prompt, file content, base URL, or model output. Logging the real provider/
            // model (not the hardcoded GEMMA_MODEL constant) is what makes an oMLX failure
            // triageable: with oMLX active the constant would show the wrong model.
            eprintln!(
                "censor gemma: {} model {} generate failed for {file_rel_path} ({e})",
                client.provider_label(),
                client.model_label()
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn det(line: u32, title: &str) -> RawFinding {
        RawFinding {
            file: "src/a.ts".into(),
            line: Some(line),
            severity: Severity::Medium,
            category: Category::Correctness,
            source: "eslint".into(),
            title: title.into(),
            body: "det body".into(),
        }
    }

    // ---- A stub client: canned probe + generate, no network. ----

    struct StubClient {
        probe_result: bool,
        generate_result: Result<String, GemmaError>,
        generate_calls: Arc<AtomicUsize>,
    }

    impl StubClient {
        fn new(probe: bool, gen: Result<String, GemmaError>) -> Self {
            Self {
                probe_result: probe,
                generate_result: gen,
                generate_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl GemmaClient for StubClient {
        fn probe(&self) -> bool {
            self.probe_result
        }
        fn generate(&self, _prompt: &str) -> Result<String, GemmaError> {
            self.generate_calls.fetch_add(1, Ordering::SeqCst);
            self.generate_result.clone()
        }
        fn provider_label(&self) -> &'static str {
            "stub"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
    }

    // ---- build_prompt ----

    #[test]
    fn build_prompt_includes_system_instruction_and_file() {
        let p = build_prompt("src/a.ts", "const x = 1;\n", &[]);
        assert!(
            p.contains("review exactly ONE file"),
            "system instruction present"
        );
        assert!(p.contains("FILE: src/a.ts"));
        assert!(p.contains("const x = 1;"));
        // No deterministic findings → the "(none)" marker.
        assert!(p.contains("ALREADY KNOWN — do NOT repeat these: (none)"));
        assert!(p.contains("JSON array"));
    }

    #[test]
    fn build_prompt_renders_deterministic_findings_as_already_known() {
        let dets = vec![det(10, "no-unused-vars"), det(20, "eqeqeq")];
        let p = build_prompt("src/a.ts", "code", &dets);
        assert!(p.contains("ALREADY KNOWN — do NOT repeat these:"));
        assert!(p.contains("line 10: no-unused-vars"));
        assert!(p.contains("line 20: eqeqeq"));
        // The "(none)" marker must NOT appear when findings exist.
        assert!(!p.contains("(none)"));
    }

    #[test]
    fn build_prompt_truncates_huge_file() {
        // Use a content char ('Z') that does NOT appear in the fixed instruction, so
        // the count reflects ONLY the embedded file content (the instruction contains
        // letters like 'x' in "exactly", which would otherwise inflate the count).
        let huge = "Z".repeat(MAX_FILE_CHARS + 5_000);
        let p = build_prompt("big.ts", &huge, &[]);
        assert!(p.contains("--- FILE CONTENT TRUNCATED ---"));
        // The embedded content must be truncated to at most MAX_FILE_CHARS chars.
        let z_count = p.matches('Z').count();
        assert_eq!(
            z_count, MAX_FILE_CHARS,
            "content truncated to exactly the cap"
        );
        // And a file UNDER the cap is embedded whole (no truncation marker).
        let small = build_prompt("small.ts", "ZZZ", &[]);
        assert!(!small.contains("--- FILE CONTENT TRUNCATED ---"));
        assert_eq!(small.matches('Z').count(), 3);
    }

    // ---- parse_gemma ----

    #[test]
    fn parse_gemma_valid_array_maps_to_raw_findings() {
        let resp = r#"[
            {"line": 12, "title": "Inverted condition", "body": "The guard is backwards.", "severity": "high"},
            {"line": 30, "title": "Swapped args", "body": "Order looks wrong.", "severity": "low"}
        ]"#;
        let out = parse_gemma("src/a.ts", resp);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].source, "gemma");
        assert_eq!(out[0].category, Category::Correctness);
        assert_eq!(out[0].file, "src/a.ts");
        assert_eq!(out[0].line, Some(12));
        assert_eq!(out[0].severity, Severity::High);
        assert_eq!(out[0].title, "Inverted condition");
        assert_eq!(out[1].severity, Severity::Low);
    }

    #[test]
    fn parse_gemma_extracts_array_wrapped_in_markdown_and_prose() {
        let resp = "Sure! Here is what I found:\n\n```json\n[{\"line\": 5, \"title\": \"Off-by-one\", \"body\": \"loop bound\", \"severity\": \"medium\"}]\n```\nHope that helps.";
        let out = parse_gemma("src/a.ts", resp);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Off-by-one");
        assert_eq!(out[0].line, Some(5));
        assert_eq!(out[0].severity, Severity::Medium);
    }

    #[test]
    fn parse_gemma_drops_malformed_entries_keeps_good() {
        let resp = r#"[
            {"line": 1, "title": "Good one", "body": "ok", "severity": "high"},
            "not an object",
            {"body": "no title here", "severity": "low"},
            {"line": 2, "title": "", "body": "empty title dropped", "severity": "low"},
            42,
            {"line": 3, "title": "Also good", "severity": "weird-severity"}
        ]"#;
        let out = parse_gemma("src/a.ts", resp);
        // Only the two with a non-empty title survive.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Good one");
        assert_eq!(out[1].title, "Also good");
        // Unknown severity defaults to Medium.
        assert_eq!(out[1].severity, Severity::Medium);
        // Missing body → empty string, not a panic.
        assert_eq!(out[1].body, "");
    }

    #[test]
    fn parse_gemma_non_json_and_empty_yield_empty() {
        assert!(parse_gemma("f", "").is_empty());
        assert!(parse_gemma("f", "I could not find anything to report.").is_empty());
        assert!(parse_gemma("f", "[ this is not valid json").is_empty());
        assert!(parse_gemma("f", "{\"not\": \"an array\"}").is_empty());
        // An empty array is valid → empty.
        assert!(parse_gemma("f", "[]").is_empty());
    }

    #[test]
    fn parse_gemma_caps_finding_count() {
        // 30 valid findings → capped at MAX_GEMMA_FINDINGS.
        let entries: Vec<String> = (0..30)
            .map(|i| {
                format!(
                    "{{\"line\": {}, \"title\": \"t{}\", \"severity\": \"low\"}}",
                    i + 1,
                    i
                )
            })
            .collect();
        let resp = format!("[{}]", entries.join(","));
        let out = parse_gemma("f", &resp);
        assert_eq!(out.len(), MAX_GEMMA_FINDINGS);
    }

    #[test]
    fn parse_gemma_redacts_secret_in_title_and_body() {
        // The model echoes an AWS-key-shaped secret it read from the file. It must be
        // redacted before it can reach a shard.
        let resp = r#"[{"line": 3, "title": "Hardcoded key AKIAIOSFODNN7EXAMPLE found", "body": "secret AKIAIOSFODNN7EXAMPLE in source", "severity": "high"}]"#;
        let out = parse_gemma("src/a.ts", resp);
        assert_eq!(out.len(), 1);
        assert!(
            !out[0].title.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked in title: {}",
            out[0].title
        );
        assert!(
            !out[0].body.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked in body: {}",
            out[0].body
        );
        assert!(out[0].title.contains("[redacted]"));
    }

    #[test]
    fn parse_gemma_array_inside_string_does_not_close_early() {
        // A `]` inside a string literal must not prematurely close the array.
        let resp = r#"[{"line": 1, "title": "arr[i] access wrong", "body": "uses x[0]", "severity": "low"}]"#;
        let out = parse_gemma("f", resp);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "arr[i] access wrong");
    }

    // ---- extract_json_array unit edges ----

    #[test]
    fn extract_json_array_handles_nested_and_strings() {
        assert_eq!(
            extract_json_array("foo [1,[2,3]] bar").as_deref(),
            Some("[1,[2,3]]")
        );
        assert_eq!(extract_json_array("no array here"), None);
        assert_eq!(extract_json_array("[unbalanced"), None);
    }

    // ---- probe_available ----

    #[test]
    fn probe_available_true_when_stub_reports_present() {
        let c = StubClient::new(true, Ok(String::new()));
        assert!(probe_available(&c));
    }

    #[test]
    fn probe_available_false_when_stub_reports_absent() {
        let c = StubClient::new(false, Ok(String::new()));
        assert!(!probe_available(&c));
    }

    #[test]
    fn tag_names_collects_name_and_model_fields() {
        // The resolver keys off exact membership, so `tag_names` must surface every tag the
        // daemon reports — the union of `name` and `model`, de-duplicated.
        let body = serde_json::json!({
            "models": [
                { "name": "gemma4:e4b", "model": "gemma4:e4b" },
                { "name": "llama3:8b" },
                { "model": "gemma4:e2b" }
            ]
        });
        let names = tag_names(&body);
        assert!(names.contains(&"gemma4:e4b".to_string()));
        assert!(names.contains(&"llama3:8b".to_string()));
        assert!(names.contains(&"gemma4:e2b".to_string()));
        // De-duplicated: e4b appears once even though name == model.
        assert_eq!(
            names.iter().filter(|n| *n == "gemma4:e4b").count(),
            1,
            "duplicate name/model collapses to one entry"
        );
        // Missing/empty models array → empty.
        assert!(tag_names(&serde_json::json!({})).is_empty());
        assert!(tag_names(&serde_json::json!({ "models": [] })).is_empty());
    }

    // ---- resolve_gemma_model (the resolution chain) ----

    #[test]
    fn resolve_gemma_model_configured_wins_outright() {
        // A configured (valid) override is used verbatim, EVEN IF it is not in the tags
        // (documented rule: the user's explicit choice wins; they may be mid-pull).
        assert_eq!(
            resolve_gemma_model(Some("llama3:8b"), &["gemma4:e4b".to_string()]),
            "llama3:8b"
        );
        assert_eq!(
            resolve_gemma_model(Some("custom:tag"), &[]),
            "custom:tag",
            "configured wins even with an empty tag list"
        );
        // Whitespace-only / empty configured is treated as absent (falls to the chain).
        assert_eq!(
            resolve_gemma_model(Some("   "), &["gemma4:e2b".to_string()]),
            "gemma4:e2b"
        );
    }

    #[test]
    fn resolve_gemma_model_default_present_uses_e4b() {
        // No override + e4b present → the new default.
        assert_eq!(
            resolve_gemma_model(
                None,
                &["gemma4:e4b".to_string(), "gemma4:e2b".to_string()]
            ),
            "gemma4:e4b"
        );
        assert_eq!(GEMMA_MODEL, "gemma4:e4b", "default bumped to e4b");
    }

    #[test]
    fn resolve_gemma_model_upgrade_safety_falls_back_to_e2b() {
        // THE upgrade-safety case: an old install pulled only e2b. After the default bumped
        // to e4b, the tier must NOT silently disappear — fall back to the present e2b.
        assert_eq!(
            resolve_gemma_model(None, &["gemma4:e2b".to_string()]),
            "gemma4:e2b"
        );
        assert_eq!(GEMMA_FALLBACK_MODEL, "gemma4:e2b");
    }

    #[test]
    fn resolve_gemma_model_neither_present_defaults_to_e4b() {
        // Neither tag present (or no tags at all) → the default e4b; the generate call then
        // degrades cleanly (unavailable model), never a crash.
        assert_eq!(
            resolve_gemma_model(None, &["llama3:8b".to_string()]),
            "gemma4:e4b"
        );
        assert_eq!(resolve_gemma_model(None, &[]), "gemma4:e4b");
    }

    // ---- run_gemma ----

    #[test]
    fn run_gemma_unavailable_returns_empty_without_calling_generate() {
        let c = StubClient::new(
            true,
            Ok("[{\"line\":1,\"title\":\"x\",\"severity\":\"low\"}]".into()),
        );
        let calls = c.generate_calls.clone();
        let out = run_gemma(&c, false, Path::new("/root"), "src/a.ts", "code", &[]);
        assert!(out.is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "generate must not be called when unavailable"
        );
    }

    #[test]
    fn run_gemma_available_returns_parsed_findings() {
        let resp =
            r#"[{"line": 7, "title": "Inverted guard", "body": "backwards", "severity": "high"}]"#;
        let c = StubClient::new(true, Ok(resp.into()));
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[det(1, "known")],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "gemma");
        assert_eq!(out[0].title, "Inverted guard");
        assert_eq!(out[0].line, Some(7));
    }

    #[test]
    fn run_gemma_generate_error_returns_empty_no_panic() {
        let c = StubClient::new(true, Err(GemmaError::Timeout));
        let out = run_gemma(&c, true, Path::new("/root"), "src/a.ts", "code", &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn run_gemma_garbage_response_returns_empty() {
        let c = StubClient::new(true, Ok("the model rambled with no json".into()));
        let out = run_gemma(&c, true, Path::new("/root"), "src/a.ts", "code", &[]);
        assert!(out.is_empty());
    }

    // ---- build_context_block seam ----

    #[test]
    fn build_context_block_returns_none() {
        assert!(build_context_block(Path::new("/root"), "src/a.ts").is_none());
    }

    // ---- BLOCKER: response-body size cap ----

    #[test]
    fn parse_generate_body_under_cap_extracts_response() {
        let body = br#"{"response":"hello","done":true}"#;
        assert_eq!(parse_generate_body(body).unwrap(), "hello");
    }

    #[test]
    fn parse_generate_body_over_cap_is_rejected_not_parsed() {
        // A runaway model returns a body > RESPONSE_BODY_CAP. It must be rejected as a
        // Decode error WITHOUT being deserialized (no OOM, no parse cost). We build a
        // valid-JSON body whose size alone exceeds the cap, proving the size guard
        // fires before parsing (a body this size with a giant `response` is exactly
        // the OOM vector).
        let filler = "A".repeat(RESPONSE_BODY_CAP + 1024);
        let body = format!(r#"{{"response":"{filler}"}}"#);
        assert!(body.len() > RESPONSE_BODY_CAP);
        assert_eq!(
            parse_generate_body(body.as_bytes()),
            Err(GemmaError::Decode)
        );
    }

    #[test]
    fn parse_generate_body_just_under_cap_with_huge_response_is_ok() {
        // A body whose `response` is large but the WHOLE body is under the cap parses
        // fine (the cap is on the body, not the finding count — that is capped later).
        let filler = "B".repeat(RESPONSE_BODY_CAP - 64);
        let body = format!(r#"{{"response":"{filler}"}}"#);
        assert!(
            body.len() <= RESPONSE_BODY_CAP,
            "fixture must be under the cap"
        );
        assert_eq!(
            parse_generate_body(body.as_bytes()).unwrap().len(),
            filler.len()
        );
    }

    #[test]
    fn parse_generate_body_non_json_is_decode_error() {
        assert_eq!(parse_generate_body(b"not json"), Err(GemmaError::Decode));
    }

    #[test]
    fn parse_generate_body_missing_response_field_is_empty_string() {
        // Valid JSON, no `response` key → empty string (downstream yields no findings).
        assert_eq!(parse_generate_body(br#"{"done":true}"#).unwrap(), "");
    }

    // ---- WARNING: loopback-only base validation ----

    #[test]
    fn is_loopback_base_accepts_only_loopback_origins() {
        assert!(is_loopback_base("http://127.0.0.1:11434"));
        assert!(is_loopback_base("http://127.0.0.1"));
        assert!(is_loopback_base("http://127.5.6.7:8080"));
        assert!(is_loopback_base("http://localhost:11434"));
        assert!(is_loopback_base("http://localhost"));
        assert!(is_loopback_base("http://[::1]:11434"));
        assert!(is_loopback_base("http://[::1]"));
        assert!(is_loopback_base("http://127.0.0.1:11434/api/generate"));
    }

    #[test]
    fn is_loopback_base_rejects_remote_and_tricky_bases() {
        // Plain remote hosts.
        assert!(!is_loopback_base("http://evil.com:11434"));
        assert!(!is_loopback_base("http://10.0.0.5:11434"));
        assert!(!is_loopback_base("http://192.168.1.10"));
        // https is rejected (loopback Ollama is plain HTTP).
        assert!(!is_loopback_base("https://127.0.0.1:11434"));
        // A hostname that merely CONTAINS "localhost"/"127." but is not loopback.
        assert!(!is_loopback_base("http://localhost.evil.com"));
        assert!(!is_loopback_base("http://127.0.0.1.evil.com"));
        // Userinfo trick: the real host is evil.com.
        assert!(!is_loopback_base("http://127.0.0.1@evil.com"));
        // F1 regression: IPv6 userinfo bypass — `[::1]:port@evil.com` routes off-box.
        assert!(!is_loopback_base("http://[::1]:8000@evil.com"));
        assert!(!is_loopback_base("http://[::1]:@evil.com"));
        assert!(!is_loopback_base("http://[::1]@evil.com"));
        // No scheme at all.
        assert!(!is_loopback_base("127.0.0.1:11434"));
        assert!(!is_loopback_base(""));
    }

    #[test]
    fn with_config_clamps_non_loopback_base_to_default() {
        // A caller passing a remote base must NOT produce a client that points off-box;
        // it falls back to the loopback default.
        let c = OllamaClient::with_config(
            "http://evil.com:11434",
            None,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(
            c.base, OLLAMA_BASE,
            "non-loopback base must be clamped to the default"
        );
        // A legitimate loopback base is preserved verbatim.
        let c2 = OllamaClient::with_config(
            "http://localhost:11434",
            None,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(c2.base, "http://localhost:11434");
    }

    // ---- WARNING: build_prompt redacts a secret in a deterministic title ----

    #[test]
    fn build_prompt_redacts_secret_in_deterministic_title() {
        // A deterministic finding whose title carries an (un-redacted) AWS-key-shaped
        // secret must be redacted before it reaches the prompt we send the model.
        let dets = vec![det(7, "leaked key AKIAIOSFODNN7EXAMPLE in config")];
        let p = build_prompt("src/a.ts", "code", &dets);
        assert!(
            !p.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked into prompt: {p}"
        );
        assert!(
            p.contains("[redacted]"),
            "redaction marker present in prompt"
        );
    }

    // =====================================================================
    // oMLX provider (P4): config + OmlxClient + OpenAI-envelope parse helper.
    // =====================================================================

    // ---- is_loopback_omlx_base: same host rule as Ollama, http-only, +port check ----

    #[test]
    fn is_loopback_omlx_base_accepts_http_loopback() {
        assert!(is_loopback_omlx_base("http://localhost:8000/v1"));
        assert!(is_loopback_omlx_base("http://127.0.0.1:8000"));
        assert!(is_loopback_omlx_base("http://127.5.6.7:8080/v1"));
        assert!(is_loopback_omlx_base("http://[::1]:8000/v1"));
        assert!(is_loopback_omlx_base("http://[::1]"));
        assert!(is_loopback_omlx_base("http://localhost/v1")); // no port
    }

    #[test]
    fn is_loopback_omlx_base_rejects_https() {
        // F3: oMLX is http-only on loopback (like Ollama) — a self-signed TLS cert on a
        // loopback oMLX server would silently disable the tier, so https is REJECTED.
        assert!(!is_loopback_omlx_base("https://localhost:8000/v1"));
        assert!(!is_loopback_omlx_base("https://127.0.0.1:8000"));
        assert!(!is_loopback_omlx_base("https://[::1]:8000/v1"));
        assert!(!is_loopback_omlx_base("https://[::1]"));
    }

    #[test]
    fn is_loopback_omlx_base_rejects_remote_and_tricky() {
        assert!(!is_loopback_omlx_base("http://evil.com:8000/v1"));
        assert!(!is_loopback_omlx_base("http://10.0.0.5:8000"));
        // Suffix trick + userinfo trick.
        assert!(!is_loopback_omlx_base("http://127.0.0.1.evil.com:8000"));
        assert!(!is_loopback_omlx_base("http://localhost.evil.com"));
        assert!(!is_loopback_omlx_base("http://127.0.0.1@evil.com"));
        assert!(!is_loopback_omlx_base("http://[::1]:8000@evil.com"));
        // No scheme / unsupported scheme.
        assert!(!is_loopback_omlx_base("localhost:8000"));
        assert!(!is_loopback_omlx_base("ftp://localhost:8000"));
        assert!(!is_loopback_omlx_base(""));
    }

    #[test]
    fn is_loopback_omlx_base_validates_optional_port() {
        // F1+F2: parity with mini_coder — a present port must be 1-5 digits and <= 65535;
        // an EMPTY port is rejected. Layered on top of the (Ollama-shared) host rule.
        for ok in [
            "http://[::1]:8000",
            "http://localhost:8000",
            "http://127.0.0.1:8000",
            "http://127.0.0.1:1",
            "http://127.0.0.1:65535",
            "http://[::1]", // no port at all is fine
            "http://localhost",
        ] {
            assert!(is_loopback_omlx_base(ok), "valid-port base {ok:?} must be accepted");
        }
        for bad in [
            "http://[::1]:",         // empty ipv6 port
            "http://localhost:",     // empty port
            "http://localhost:99999", // > 65535
            "http://localhost:65536", // > 65535
            "http://localhost:abc",  // non-numeric
            "http://127.0.0.1:",     // empty port
            "http://[::1]:abc",      // ipv6 non-numeric
            "http://[::1]:65536",    // ipv6 out of range
        ] {
            assert!(!is_loopback_omlx_base(bad), "invalid-port base {bad:?} must be rejected");
        }
    }

    // ---- PRIVACY: OmlxClient clamps a non-loopback base to the default ----

    #[test]
    fn omlx_with_config_clamps_non_loopback_base_to_default() {
        // A caller passing a remote base must NOT produce a client that points off-box;
        // it falls back to the loopback oMLX default (privacy fail-safe).
        let c = OmlxClient::with_config(
            "http://evil.com:8000/v1",
            "mlx-community/model",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(
            c.base, OMLX_DEFAULT_BASE,
            "non-loopback oMLX base must be clamped to the default"
        );
        // An https remote is clamped too (the scheme doesn't rescue a remote host).
        let c_https = OmlxClient::with_config(
            "https://evil.com:8000/v1",
            "m",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(c_https.base, OMLX_DEFAULT_BASE);
        // F3: an https LOOPBACK base is also clamped now — oMLX is http-only on loopback.
        let c_https_loopback = OmlxClient::with_config(
            "https://localhost:8000/v1",
            "m",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(
            c_https_loopback.base, OMLX_DEFAULT_BASE,
            "https loopback oMLX base must be clamped (http only)"
        );
        // A legitimate loopback HTTP base is preserved verbatim.
        let c2 = OmlxClient::with_config(
            "http://localhost:8000/v1",
            "m",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(c2.base, "http://localhost:8000/v1");
        assert_eq!(c2.model, "m");
    }

    #[test]
    fn omlx_with_config_clamp_is_self_contained() {
        // max-recall FIX 7: the clamp must apply the FULL config-time validator's rules —
        // not loopback alone — so the type is safe regardless of how it is called.

        // An over-length base (even an otherwise-loopback one) is clamped to the default.
        let overlong = format!("http://localhost:8000/{}", "a".repeat(OMLX_BASE_URL_MAX_LEN));
        let c_long = OmlxClient::with_config(
            &overlong,
            "m",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(
            c_long.base, OMLX_DEFAULT_BASE,
            "an over-length oMLX base must be clamped to the default"
        );

        // A base carrying a control/bidi/invisible char is clamped too.
        let obfuscated = "http://localhost:8000/\u{202e}v1";
        let c_bidi = OmlxClient::with_config(
            obfuscated,
            "m",
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(
            c_bidi.base, OMLX_DEFAULT_BASE,
            "a base with a forbidden char must be clamped to the default"
        );
    }

    // ---- parse_openai_chat_body: extract content, size cap, defensive ----

    #[test]
    fn parse_openai_chat_body_extracts_message_content() {
        let body = br#"{"choices":[{"message":{"role":"assistant","content":"hello world"}}]}"#;
        assert_eq!(parse_openai_chat_body(body).unwrap(), "hello world");
    }

    #[test]
    fn parse_openai_chat_body_over_cap_is_rejected_not_parsed() {
        // A runaway model returns a body > RESPONSE_BODY_CAP: rejected as Decode WITHOUT
        // deserializing (no OOM), mirroring parse_generate_body.
        let filler = "A".repeat(RESPONSE_BODY_CAP + 1024);
        let body = format!(r#"{{"choices":[{{"message":{{"content":"{filler}"}}}}]}}"#);
        assert!(body.len() > RESPONSE_BODY_CAP);
        assert_eq!(
            parse_openai_chat_body(body.as_bytes()),
            Err(GemmaError::Decode)
        );
    }

    #[test]
    fn parse_openai_chat_body_just_under_cap_ok() {
        let filler = "B".repeat(RESPONSE_BODY_CAP - 128);
        let body = format!(r#"{{"choices":[{{"message":{{"content":"{filler}"}}}}]}}"#);
        assert!(body.len() <= RESPONSE_BODY_CAP, "fixture must be under cap");
        assert_eq!(
            parse_openai_chat_body(body.as_bytes()).unwrap().len(),
            filler.len()
        );
    }

    #[test]
    fn parse_openai_chat_body_non_json_is_decode_error() {
        assert_eq!(parse_openai_chat_body(b"not json"), Err(GemmaError::Decode));
    }

    #[test]
    fn parse_openai_chat_body_missing_fields_yield_empty_string() {
        // Valid JSON but no choices / message / content → empty string (no panic).
        assert_eq!(parse_openai_chat_body(br#"{"id":"x"}"#).unwrap(), "");
        assert_eq!(parse_openai_chat_body(br#"{"choices":[]}"#).unwrap(), "");
        assert_eq!(
            parse_openai_chat_body(br#"{"choices":[{"message":{}}]}"#).unwrap(),
            ""
        );
        // content present but null → empty string.
        assert_eq!(
            parse_openai_chat_body(br#"{"choices":[{"message":{"content":null}}]}"#).unwrap(),
            ""
        );
    }

    // ---- INTEGRATION: oMLX content still flows through parse_gemma (redaction!) ----

    #[test]
    fn omlx_extracted_content_flows_through_parse_gemma_with_redaction() {
        // Build a realistic OpenAI envelope whose assistant content is the JSON-array
        // Gemma output, INCLUDING an echoed secret. The extracted content must parse via
        // the EXISTING parse_gemma AND have the secret redacted (same posture as Ollama).
        let inner = r#"[{"line": 3, "title": "Hardcoded key AKIAIOSFODNN7EXAMPLE", "body": "secret AKIAIOSFODNN7EXAMPLE in source", "severity": "high"}]"#;
        let envelope = serde_json::json!({
            "choices": [ { "message": { "role": "assistant", "content": inner } } ]
        });
        let bytes = serde_json::to_vec(&envelope).unwrap();
        let content = parse_openai_chat_body(&bytes).unwrap();
        let out = parse_gemma("src/a.ts", &content);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "gemma");
        assert_eq!(out[0].line, Some(3));
        assert_eq!(out[0].severity, Severity::High);
        assert!(
            !out[0].title.contains("AKIAIOSFODNN7EXAMPLE"),
            "secret leaked through oMLX path: {}",
            out[0].title
        );
        assert!(out[0].title.contains("[redacted]"));
        assert!(!out[0].body.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    // ---- openai_model_present ----

    #[test]
    fn openai_model_present_matches_exact_id_only() {
        let body = serde_json::json!({
            "data": [ { "id": "mlx-community/gemma" }, { "id": "other" } ]
        });
        assert!(openai_model_present(&body, "mlx-community/gemma"));
        assert!(!openai_model_present(&body, "mlx-community/other-model"));
        // Missing / empty data → false.
        assert!(!openai_model_present(&serde_json::json!({}), "m"));
        assert!(!openai_model_present(&serde_json::json!({ "data": [] }), "m"));
    }

    // ---- validate_censor_local_ai: defaults + omlx requirements + privacy ----

    #[test]
    fn validate_censor_local_ai_ollama_optional_fields() {
        // Bare ollama is valid (defaults applied at use-site).
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: None,
        })
        .unwrap();
        assert_eq!(v.provider, CensorAiProvider::Ollama);
        assert_eq!(v.effective_base(), OLLAMA_BASE);
        assert_eq!(v.effective_model(), GEMMA_MODEL);
        // A loopback base + model for ollama is accepted and trimmed.
        let v2 = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("  http://127.0.0.1:11434  ".into()),
            model: Some(" gemma4:e2b ".into()),
            ollama_model: None,
        })
        .unwrap();
        assert_eq!(v2.base_url.as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(v2.model.as_deref(), Some("gemma4:e2b"));
    }

    #[test]
    fn validate_censor_local_ai_ollama_rejects_non_loopback_base() {
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("http://evil.com:11434".into()),
            model: None,
            ollama_model: None,
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_omlx_requires_base_and_model() {
        // Missing model.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: None,
            ollama_model: None,
        })
        .is_err());
        // Missing base.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: None,
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
        // Empty (whitespace) base/model.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("   ".into()),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_omlx_accepts_valid_and_normalizes() {
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1/".into()),
            model: Some("mlx-community/gemma".into()),
            ollama_model: None,
        })
        .unwrap();
        assert_eq!(v.provider, CensorAiProvider::Omlx);
        // Trailing slash stripped.
        assert_eq!(v.base_url.as_deref(), Some("http://localhost:8000/v1"));
        assert_eq!(v.model.as_deref(), Some("mlx-community/gemma"));
        assert_eq!(v.effective_base(), "http://localhost:8000/v1");
        assert_eq!(v.effective_model(), "mlx-community/gemma");
    }

    #[test]
    fn validate_censor_local_ai_omlx_rejects_https() {
        // F3: oMLX is http-only on loopback. A loopback https base is refused at config
        // time (would otherwise pass validation then silently fail probe() over TLS).
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("https://localhost:8000/v1".into()),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_ollama_strips_trailing_slash() {
        // F5: a user-supplied Ollama base with a trailing slash must be normalized so
        // `<base>/api/generate` never double-slashes (`…11434//api/generate`).
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("http://127.0.0.1:11434/".into()),
            model: None,
            ollama_model: None,
        })
        .unwrap();
        assert_eq!(v.base_url.as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(v.effective_base(), "http://127.0.0.1:11434");
    }

    #[test]
    fn validate_censor_local_ai_omlx_rejects_non_loopback_and_bad_chars() {
        // PRIVACY: a non-loopback oMLX base is refused (would route code off-box).
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://evil.com:8000/v1".into()),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
        // Userinfo trick refused.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://127.0.0.1@evil.com".into()),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
        // Control / bidi chars refused.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/\u{202e}v1".into()),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
        // Overlong base refused.
        let long = format!("http://localhost:8000/{}", "a".repeat(OMLX_BASE_URL_MAX_LEN));
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some(long),
            model: Some("m".into()),
            ollama_model: None,
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_omlx_model_char_class_matches_mini() {
        // PARITY (max-recall FIX 3): the Censor oMLX model must use the SAME bare-token
        // char-class as `mini_coder::is_valid_model`. `org/name` HF paths stay valid;
        // whitespace / control / shell-metachars are rejected so all oMLX model
        // validators agree.
        let valid = [
            "m",
            "gemma4:e2b",
            "mlx-community/gemma-2-2b-it",
            "Org_Name/model.v1-2:tag",
        ];
        for model in valid {
            let v = validate_censor_local_ai(&CensorLocalAi {
                provider: CensorAiProvider::Omlx,
                base_url: Some("http://localhost:8000/v1".into()),
                model: Some(model.into()),
                ollama_model: None,
            });
            assert!(v.is_ok(), "valid oMLX model {model:?} must be accepted: {v:?}");
            // The same token must satisfy the mini-coder validator (cross-check parity).
            assert!(
                crate::backend::mini_coder::is_valid_model(model),
                "mini_coder::is_valid_model must agree on {model:?}"
            );
        }

        let invalid = [
            "model name",       // whitespace
            "model;rm -rf",     // shell metachar
            "-leading-dash",    // first char not alnum
            ".dotfirst",        // first char not alnum
            "model\u{202e}evil", // bidi override
            "model\ttab",       // control
            "model@host",       // @ not allowed
            "model\\path",      // backslash not allowed
        ];
        for model in invalid {
            assert!(
                validate_censor_local_ai(&CensorLocalAi {
                    provider: CensorAiProvider::Omlx,
                    base_url: Some("http://localhost:8000/v1".into()),
                    model: Some(model.into()),
                    ollama_model: None,
                })
                .is_err(),
                "invalid oMLX model {model:?} must be rejected"
            );
            // The same token must FAIL the mini-coder validator too (parity).
            assert!(
                !crate::backend::mini_coder::is_valid_model(model),
                "mini_coder::is_valid_model must also reject {model:?}"
            );
        }
    }

    #[test]
    fn validate_censor_local_ai_omlx_model_length_cap_matches_ts() {
        // PARITY (max-recall FIX 2): the Censor oMLX model is capped at
        // CENSOR_OMLX_MODEL_MAX_LEN (200), matching the TS cap. A bare-token model at
        // exactly the cap passes; one char over is refused.
        let at_cap = "a".repeat(CENSOR_OMLX_MODEL_MAX_LEN);
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some(at_cap),
            ollama_model: None,
        })
        .is_ok());
        let over_cap = "a".repeat(CENSOR_OMLX_MODEL_MAX_LEN + 1);
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some(over_cap),
            ollama_model: None,
        })
        .is_err());
    }

    #[test]
    fn censor_local_ai_default_is_ollama() {
        let d = CensorLocalAi::default();
        assert_eq!(d.provider, CensorAiProvider::Ollama);
        assert!(d.base_url.is_none());
        assert!(d.model.is_none());
        assert!(d.ollama_model.is_none());
    }

    #[test]
    fn censor_local_ai_serde_camelcase_and_no_churn() {
        // Provider serializes lowercase; absent optionals are omitted (no-churn).
        let d = CensorLocalAi::default();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, r#"{"provider":"ollama"}"#);
        // omlx round-trips camelCase keys.
        let omlx = CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some("m".into()),
            ollama_model: None,
        };
        let j = serde_json::to_string(&omlx).unwrap();
        assert!(j.contains("\"baseUrl\":\"http://localhost:8000/v1\""), "{j}");
        assert!(!j.contains("base_url"), "snake_case leaked: {j}");
        // NO-CHURN: ollama_model is None → never serialized.
        assert!(!j.contains("ollamaModel"), "absent ollamaModel must not serialize: {j}");
        let back: CensorLocalAi = serde_json::from_str(&j).unwrap();
        assert_eq!(back, omlx);
        // Deserialize from a minimal camelCase object (provider only).
        let parsed: CensorLocalAi = serde_json::from_str(r#"{"provider":"omlx","baseUrl":"http://localhost:8000","model":"x"}"#).unwrap();
        assert_eq!(parsed.provider, CensorAiProvider::Omlx);
        assert_eq!(parsed.base_url.as_deref(), Some("http://localhost:8000"));

        // BACKWARD COMPAT: an OLD ollama config (no ollamaModel key) parses with None.
        let old: CensorLocalAi =
            serde_json::from_str(r#"{"provider":"ollama"}"#).unwrap();
        assert!(old.ollama_model.is_none());
        // Round-trip WITH a configured ollamaModel (camelCase over IPC).
        let with_model = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("gemma4:e4b".into()),
        };
        let jm = serde_json::to_string(&with_model).unwrap();
        assert!(jm.contains("\"ollamaModel\":\"gemma4:e4b\""), "{jm}");
        let back_m: CensorLocalAi = serde_json::from_str(&jm).unwrap();
        assert_eq!(back_m, with_model);
    }

    #[test]
    fn validate_censor_local_ai_ollama_model_override() {
        // A valid bare ollamaModel tag is kept (and trimmed).
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("  gemma4:e4b  ".into()),
        })
        .unwrap();
        assert_eq!(v.ollama_model.as_deref(), Some("gemma4:e4b"));
        // Empty-after-trim → None (treated as absent).
        let v_empty = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("   ".into()),
        })
        .unwrap();
        assert!(v_empty.ollama_model.is_none());
        // Invalid (space / control / overlong) → REJECTED.
        for bad in [
            "model name",
            "model;rm",
            "bad\u{202e}tag",
            "model\ttab",
        ] {
            assert!(
                validate_censor_local_ai(&CensorLocalAi {
                    provider: CensorAiProvider::Ollama,
                    base_url: None,
                    model: None,
                    ollama_model: Some(bad.into()),
                })
                .is_err(),
                "invalid ollamaModel {bad:?} must be rejected"
            );
        }
        let overlong = "a".repeat(CENSOR_OMLX_MODEL_MAX_LEN + 1);
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some(overlong),
        })
        .is_err());
        // oMLX config: a stray ollama_model is dropped (oMLX uses `model`).
        let omlx = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some("mlx-community/gemma".into()),
            ollama_model: Some("gemma4:e4b".into()),
        })
        .unwrap();
        assert!(omlx.ollama_model.is_none());
    }

    #[test]
    fn resolved_model_does_no_io_when_memo_empty_and_no_override() {
        // WARNING 5 (memoization second-fetch): with an EMPTY memo and NO configured
        // override, resolved_model() must return the pessimistic default WITHOUT performing
        // any `/api/tags` IO — probe() is the sole memoization/fetch point. We prove "no IO"
        // by pointing the client at a loopback BLACKHOLE (a listener that accepts the TCP
        // connection but NEVER replies) with a LONG probe timeout: if resolved_model fetched,
        // it would block until that timeout; instead it returns instantly.
        use std::io::Read;
        use std::net::TcpListener;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind blackhole listener");
        let addr = listener.local_addr().expect("blackhole addr");
        // Accept connections and hang (read forever, never write a response) so any HTTP
        // fetch against this base would stall for the full probe timeout.
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(mut s) => {
                        let mut buf = [0u8; 64];
                        // Block reading; never respond. Returns when the client drops.
                        let _ = s.read(&mut buf);
                    }
                    Err(_) => break,
                }
            }
        });

        let base = format!("http://127.0.0.1:{}", addr.port());
        let client = OllamaClient::with_config(
            &base,
            None,                          // NO override -> the IO branch under test
            GEMMA_GENERATE_TIMEOUT,
            Duration::from_secs(30),       // a fetch would block ~30s if it happened
        );

        let start = Instant::now();
        let resolved = client.resolved_model();
        let elapsed = start.elapsed();

        assert_eq!(
            resolved, GEMMA_MODEL,
            "no override + empty memo must yield the pessimistic default"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "resolved_model must NOT perform the 30s blackhole fetch (took {elapsed:?})"
        );
        // Dropping the listener thread is best-effort; the test process exits regardless.
        drop(handle);
    }

    #[test]
    fn resolved_model_returns_override_verbatim_without_io() {
        // WARNING 5 companion: a configured override short-circuits the resolution with NO
        // IO (used verbatim even if not in /api/tags). Same blackhole base proves no fetch.
        use std::net::TcpListener;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind blackhole listener");
        let addr = listener.local_addr().expect("blackhole addr");
        let base = format!("http://127.0.0.1:{}", addr.port());
        let client = OllamaClient::with_config(
            &base,
            Some("gemma4:custom"),
            GEMMA_GENERATE_TIMEOUT,
            Duration::from_secs(30),
        );
        let start = Instant::now();
        let resolved = client.resolved_model();
        assert_eq!(resolved, "gemma4:custom", "override used verbatim");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "an override must short-circuit without any /api/tags fetch"
        );
    }

    #[test]
    fn build_gemma_client_ollama_override_drives_configured_model() {
        // An ollamaModel override is used by the Ollama client (resolution chain: configured
        // wins). With no probe yet (no daemon), model_label falls back to the configured
        // override (IO-free), proving the override is threaded through build_gemma_client.
        let cfg = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("http://127.0.0.1:11434".into()),
            model: None,
            ollama_model: Some("gemma4:e4b".into()),
        })
        .unwrap();
        let client = build_gemma_client(&cfg);
        assert_eq!(client.provider_label(), "ollama");
        assert_eq!(client.model_label(), "gemma4:e4b");
    }

    // =====================================================================
    // oMLX-P5: provider factory (build_gemma_client) + provider_label identity.
    // =====================================================================

    #[test]
    fn provider_label_identifies_real_clients() {
        // Identity-only labels for the once-per-session log + factory testing.
        assert_eq!(OllamaClient::new().provider_label(), "ollama");
        assert_eq!(
            OmlxClient::new("http://localhost:8000/v1", "m").provider_label(),
            "omlx"
        );
    }

    #[test]
    fn model_label_reports_effective_model_per_client() {
        // F1: the log must surface the model ACTUALLY in use, not the GEMMA_MODEL constant.
        // The default Ollama client drives GEMMA_MODEL; the oMLX client drives its
        // configured model — `model_label` returns the right one for each.
        assert_eq!(OllamaClient::new().model_label(), GEMMA_MODEL);
        assert_eq!(
            OmlxClient::new("http://localhost:8000/v1", "mlx-community/gemma").model_label(),
            "mlx-community/gemma"
        );
        // A custom Ollama model is reported too (not the constant).
        let custom = build_gemma_client(
            &validate_censor_local_ai(&CensorLocalAi {
                provider: CensorAiProvider::Ollama,
                base_url: Some("http://127.0.0.1:11434".into()),
                model: None,
                ollama_model: Some("llama3:8b".into()),
            })
            .unwrap(),
        );
        assert_eq!(custom.model_label(), "llama3:8b");
        assert_eq!(custom.provider_label(), "ollama");
    }

    #[test]
    fn build_gemma_client_default_config_is_ollama() {
        // The default (a config with no `censorLocalAi`) resolves to the Ollama client —
        // byte-identical provider to the previous hardcoded OllamaClient::new().
        let client = build_gemma_client(&CensorLocalAi::default());
        assert_eq!(client.provider_label(), "ollama");
    }

    #[test]
    fn build_gemma_client_default_config_uses_ollama_base_and_model() {
        // The default config's effective base/model are the Ollama defaults, so the built
        // client points at the SAME loopback endpoint/model as before (no behavior change).
        let cfg = CensorLocalAi::default();
        assert_eq!(cfg.effective_base(), OLLAMA_BASE);
        assert_eq!(cfg.effective_model(), GEMMA_MODEL);
        let client = build_gemma_client(&cfg);
        assert_eq!(client.provider_label(), "ollama");
    }

    #[test]
    fn build_gemma_client_valid_omlx_config_is_omlx() {
        // A validated oMLX config selects the oMLX client.
        let cfg = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some("mlx-community/gemma".into()),
            ollama_model: None,
        })
        .unwrap();
        let client = build_gemma_client(&cfg);
        assert_eq!(client.provider_label(), "omlx");
    }

    #[test]
    fn build_gemma_client_same_snapshot_yields_same_provider_no_split_brain() {
        // SPLIT-BRAIN GUARD: `censor_start_watch` resolves ONE `CensorLocalAi` snapshot and
        // builds BOTH the probe client and the worker client from it. This proves the
        // factory is deterministic per snapshot: two clients built from the SAME config
        // (the probe-side + the worker-side) always agree on the provider — the probe can
        // never run on one provider while the worker runs on another.
        for cfg in [
            CensorLocalAi::default(),
            validate_censor_local_ai(&CensorLocalAi {
                provider: CensorAiProvider::Omlx,
                base_url: Some("http://localhost:8000/v1".into()),
                model: Some("mlx-community/gemma".into()),
                ollama_model: None,
            })
            .unwrap(),
        ] {
            let probe_client = build_gemma_client(&cfg);
            let worker_client = build_gemma_client(&cfg);
            assert_eq!(
                probe_client.provider_label(),
                worker_client.provider_label(),
                "probe and worker built from one snapshot must share a provider"
            );
        }
    }

    #[test]
    fn build_gemma_client_ollama_config_with_custom_loopback_base_is_ollama() {
        // A validated Ollama config with an explicit loopback base still builds an Ollama
        // client (provider selection keys off `provider`, not the base).
        let cfg = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("http://127.0.0.1:11434".into()),
            model: Some("gemma4:e2b".into()),
            ollama_model: None,
        })
        .unwrap();
        let client = build_gemma_client(&cfg);
        assert_eq!(client.provider_label(), "ollama");
    }

    #[test]
    fn provider_probe_false_takes_unavailable_path_no_generate() {
        // DEGRADATION PARITY (oMLX-P5): the worker computes `available` from the
        // factory-built client's probe, then drives `run_gemma`. Whatever the provider,
        // a probe=false (server/model absent) yields available=false ⇒ NO generate call ⇒
        // empty findings (identical to Ollama-absent today). This is the exact contract
        // the watch worker and the one-shot fallback rely on for clean degradation.
        let stub = StubClient::new(
            false,
            Ok("[{\"line\":1,\"title\":\"x\",\"severity\":\"low\"}]".into()),
        );
        let calls = stub.generate_calls.clone();
        // The worker resolves availability via probe_available(client) (same call the
        // probe site uses), then passes it to run_gemma — exactly the worker's flow.
        let available = probe_available(&stub);
        assert!(!available, "probe=false must disable the tier");
        let out = run_gemma(&stub, available, Path::new("/root"), "src/a.ts", "code", &[]);
        assert!(out.is_empty(), "unavailable tier yields no findings");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "generate must never be called when the provider probe is false"
        );
    }
}
