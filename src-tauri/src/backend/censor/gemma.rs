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
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// The RECOMMENDED censor local model — a UI suggestion the user can pick, **NOT** an
/// auto-default. OPT-IN (owner rule): the censor's local-AI tier runs ONLY a model the
/// user EXPLICITLY configured; with nothing selected the tier is OFF (no censor at all).
/// This is the model the 2026-06 benchmark recommends
/// (`docs/censor-model-benchmark-2026-06.md`): NVIDIA-Nemotron-3-Nano-4B finds in-file
/// semantic bugs deterministic tools miss AND, unlike the reasoning-distills, supports
/// tool-calling for the cross-file DEEP mode.
///
/// The `GEMMA_*` / `*_gemma` naming throughout this module is LEGACY (Gemma was the
/// original local model); the symbols are model-agnostic — only the value changed.
pub const GEMMA_MODEL: &str = "hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M";

/// PURE resolver for the Ollama-provider censor model tag (no IO). OPT-IN: returns the
/// user's configured override verbatim (the explicit choice wins, honored even if not yet
/// in `/api/tags` — they may be mid-pull), or `""` when nothing is configured. There is NO
/// auto-default and NO fallback chain: an empty result means the tier is OFF (the probe
/// finds no `/api/tags` entry equal to ""). `_available_tags` is unused (kept so the
/// probe's call site is unchanged). `configured` is expected pre-trimmed/validated (an
/// empty/whitespace-only string is treated as absent, like [`validate_censor_local_ai`]).
pub fn resolve_gemma_model(configured: Option<&str>, _available_tags: &[String]) -> String {
    // OPT-IN (owner rule): the censor runs ONLY the model the user EXPLICITLY configured —
    // there is NO auto-default. An unconfigured censor resolves to "" (empty), which the
    // probe treats as "tier off" (no `/api/tags` entry equals ""), so it simply does not
    // run; the user must pick a model. [`GEMMA_MODEL`] is the RECOMMENDED suggestion shown
    // in the UI, NOT an auto-default. `_available_tags` is no longer consulted (the
    // default/fallback chain was removed) but kept so the probe's call site is unchanged.
    configured
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .unwrap_or_default()
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
/// `pub(crate)` so the voting layer ([`crate::backend::censor::votes`]) can re-cap a suspect
/// body AFTER prepending its `[unverified …]` marker, using the SAME bound this module caps
/// parsed bodies at.
pub(crate) const BODY_CAP: usize = 1_000;

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
    #[serde(rename = "appleFm")]
    AppleFm,
    /// A remote HTTPS OpenAI-compatible endpoint reached with a `Bearer` API key (the key
    /// lives in the OS vault, NEVER in this config). The ONE Censor provider that sends file
    /// content OFF-device — strictly opt-in (provider + key + Pigeon must all be enabled).
    Cloud,
}

/// The parsed `censorLocalAi` config. PRIVACY: for the oMLX provider, `base_url` is a
/// VALIDATED loopback origin (see [`validate_censor_local_ai`]) — file content sent to
/// the model can never leave the device. Fields stay `Option` so the Ollama default
/// (the common case) carries no base/model and serializes to just `{ provider:"ollama" }`.
// NOTE: `Eq` is intentionally NOT derived — `temperature: Option<f32>` (added for the
// voting tier) is not `Eq`. `PartialEq` is kept (all comparisons/`assert_eq!` in the
// config tests use it); no code path needs `Eq`/`Hash` on this type.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
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
    /// k-sample self-consistency: how many times the model reviews EACH file. Clamped to
    /// `1..=9` by [`validate_censor_local_ai`]; `None`/absent = 1 = legacy single pass (no
    /// voting). NO-CHURN: an absent key parses (serde default) and `None` serializes to
    /// nothing, so an old config is never rewritten with this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_samples: Option<u8>,
    /// Votes needed to CONFIRM a smell (block). Clamped to `1..=n_samples` when resolved;
    /// `None` = `ceil(n_samples/2)` for `n_samples > 1`, else 1 (see
    /// [`CensorLocalAi::review_params`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_votes_block: Option<u8>,
    /// Votes needed to SURFACE a smell as an unverified suspect. `None` = 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_votes_verify: Option<u8>,
    /// Sampling temperature for the review generation. Clamped to `0.0..=1.5`; `None` =
    /// the legacy `0.1`. A non-zero temperature is what makes the k samples DIFFER (and so
    /// makes voting meaningful).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Prompt/parse style: `"gemma"` (default — the fixed [`SYSTEM_INSTRUCTION`] + free-form
    /// JSON) or `"censor_v2"` (the fine-tuned reviewer's system prompt + `<think>`/typed-
    /// category JSON). Unknown values normalize to `None` (⇒ gemma) in validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_style: Option<String>,
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
            n_samples: None,
            min_votes_block: None,
            min_votes_verify: None,
            temperature: None,
            prompt_style: None,
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
            (None, CensorAiProvider::AppleFm) => String::new(),
            // Cloud always needs an explicit https base_url — there is no default.
            (None, CensorAiProvider::Cloud) => String::new(),
        }
    }

    /// Resolve the effective model: the configured value or the provider default. oMLX
    /// has no built-in model, so a validated oMLX config always carries one; the fallback
    /// to [`GEMMA_MODEL`] only ever applies to the Ollama provider.
    pub fn effective_model(&self) -> String {
        // OPT-IN: no configured model → "" (no auto-default). For Ollama the actual tag is
        // the `ollama_model` override resolved via `resolve_gemma_model`; this field is the
        // oMLX/AppleFm model and is empty when unset, keeping the tier off.
        self.model.clone().unwrap_or_default()
    }

    /// Resolve the voting/temperature/prompt-style knobs into a concrete
    /// [`GemmaReviewParams`] for [`run_gemma`]. PURE. Applies the semantic defaults +
    /// cross-field clamps on top of the per-field bounds already applied by
    /// [`validate_censor_local_ai`]:
    ///   - `n_samples`   → `1..=9`, default 1;
    ///   - `min_votes_block`  → `1..=n_samples`, default `ceil(n/2)` for `n>1` else 1;
    ///   - `min_votes_verify` → `1..=n_samples`, default 1;
    ///   - `temperature` → `0.0..=1.5`, default [`LEGACY_GEMMA_TEMPERATURE`];
    ///   - `prompt_style` → `censor_v2` when set (case-insensitive), else `gemma`.
    ///
    /// A config that opts into nothing returns exactly [`GemmaReviewParams::default`] — the
    /// legacy single-sample behavior.
    pub fn review_params(&self) -> GemmaReviewParams {
        // AppleFm cannot thread a temperature (the `fm respond` CLI has no such flag), so the
        // k samples would be identical and voting is meaningless — force a single sample.
        let n = if self.provider == CensorAiProvider::AppleFm {
            1
        } else {
            self.n_samples.map(|v| v.clamp(1, 9)).unwrap_or(1)
        };
        let min_votes_block = self
            .min_votes_block
            .map(|v| v.clamp(1, n))
            .unwrap_or_else(|| if n > 1 { n.div_ceil(2) } else { 1 });
        // Enforce `verify <= block`: an inverted config (verify > block) would make the
        // suspect window `[verify, block)` empty, silently discarding the whole tier.
        let min_votes_verify = self
            .min_votes_verify
            .map(|v| v.clamp(1, n))
            .unwrap_or(1)
            .min(min_votes_block);
        let temperature = self
            .temperature
            .map(|t| t.clamp(0.0, 1.5))
            .unwrap_or(LEGACY_GEMMA_TEMPERATURE);
        let prompt_style = match self.prompt_style.as_deref() {
            Some(s) if s.trim().eq_ignore_ascii_case("censor_v2") => PromptStyle::CensorV2,
            _ => PromptStyle::Gemma,
        };
        GemmaReviewParams {
            vote: crate::backend::censor::votes::VoteParams {
                n_samples: n,
                min_votes_block,
                min_votes_verify,
                line_tolerance: crate::backend::censor::votes::VoteParams::default().line_tolerance,
            },
            temperature,
            prompt_style,
        }
    }
}

/// The legacy review temperature (the value hardcoded before the voting tier). `None`
/// `temperature` in the config resolves to this, and the single-sample fast path in
/// [`run_gemma`] uses it so the default config is byte-for-byte the pre-voting behavior.
pub const LEGACY_GEMMA_TEMPERATURE: f32 = 0.1;

/// Prompt + response-parse style for the review generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    /// The fixed [`SYSTEM_INSTRUCTION`] folded with the file body; response parsed by
    /// [`parse_gemma`] (free-form `{line,title,body,severity}` JSON).
    Gemma,
    /// The fine-tuned reviewer's system prompt ([`CENSOR_V2_SYSTEM`]); response parsed by
    /// [`parse_censor_v2`] (optional `<think>` block + typed-category JSON with
    /// `rationale`).
    CensorV2,
}

/// The resolved, ready-to-run review knobs (see [`CensorLocalAi::review_params`]). Copy so
/// it threads cheaply through [`crate::backend::censor::orchestrator::GemmaCtx`]. The
/// [`Default`] is the LEGACY single-sample behavior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GemmaReviewParams {
    pub vote: crate::backend::censor::votes::VoteParams,
    pub temperature: f32,
    pub prompt_style: PromptStyle,
}

impl Default for GemmaReviewParams {
    fn default() -> Self {
        Self {
            vote: crate::backend::censor::votes::VoteParams::default(),
            temperature: LEGACY_GEMMA_TEMPERATURE,
            prompt_style: PromptStyle::Gemma,
        }
    }
}

/// True when `t` is (within float epsilon) the legacy default temperature. Used to gate
/// the byte-identical single-sample fast path in [`run_gemma`]. An abs-difference compare
/// (not `==`) keeps clippy's `float_cmp` lint happy.
fn is_legacy_temperature(t: f32) -> bool {
    (t - LEGACY_GEMMA_TEMPERATURE).abs() < 1e-6
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
    // Voting/temperature/prompt-style knobs are provider-INDEPENDENT: clamp each into its
    // valid range so a hand-edited/hostile config can never persist an absurd value, and
    // normalize an unknown prompt style to `None` (⇒ gemma). The semantic defaulting +
    // cross-field clamp (block ≤ n, etc.) is layered on in `review_params`.
    let n_samples = cfg.n_samples.map(|v| v.clamp(1, 9));
    let min_votes_block = cfg.min_votes_block.map(|v| v.clamp(1, 9));
    let min_votes_verify = cfg.min_votes_verify.map(|v| v.clamp(1, 9));
    let temperature = cfg.temperature.map(|t| t.clamp(0.0, 1.5));
    let prompt_style = normalize_prompt_style(cfg.prompt_style.as_deref());
    match cfg.provider {
        CensorAiProvider::Ollama => {
            // Optional fields; if a base is given it must still be loopback http.
            let base_opt = if base.is_empty() {
                None
            } else {
                if !is_loopback_base(base) {
                    return Err(
                        "censorLocalAi.baseUrl must be a loopback http origin for ollama.".into(),
                    );
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
                n_samples,
                min_votes_block,
                min_votes_verify,
                temperature,
                prompt_style,
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
                n_samples,
                min_votes_block,
                min_votes_verify,
                temperature,
                prompt_style,
            })
        }
        CensorAiProvider::Cloud => {
            // SAME shape as the Omlx arm (base + model REQUIRED, same model char-class +
            // length cap) EXCEPT the base is validated by `validate_cloud_base_for_censor`
            // (https remote allowed — the deliberate off-device egress) instead of the
            // loopback validator.
            if base.is_empty() {
                return Err("Cloud censorLocalAi requires a base URL.".into());
            }
            if model.is_empty() {
                return Err("Cloud censorLocalAi requires a model.".into());
            }
            if model.len() > CENSOR_OMLX_MODEL_MAX_LEN {
                return Err(format!(
                    "Cloud censorLocalAi model must be at most {CENSOR_OMLX_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_omlx_model(model) {
                return Err(
                    "Cloud censorLocalAi model must be a bare tag (letters, digits, . _ : / -)."
                        .into(),
                );
            }
            let normalized_base = validate_cloud_base_for_censor(base)?;
            Ok(CensorLocalAi {
                provider: CensorAiProvider::Cloud,
                base_url: Some(normalized_base),
                model: Some(model.to_string()),
                ollama_model: None,
                n_samples,
                min_votes_block,
                min_votes_verify,
                temperature,
                prompt_style,
            })
        }
        CensorAiProvider::AppleFm => {
            if !model.is_empty() {
                if model.len() > CENSOR_OMLX_MODEL_MAX_LEN {
                    return Err(format!(
                        "Apple on-device model must be at most {CENSOR_OMLX_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_omlx_model(model) {
                    return Err(
                        "Apple on-device model must be a bare tag (letters, digits, . _ : / -)."
                            .into(),
                    );
                }
            }
            Ok(CensorLocalAi {
                provider: CensorAiProvider::AppleFm,
                base_url: None,
                model: if model.is_empty() {
                    None
                } else {
                    Some(model.to_string())
                },
                ollama_model: None,
                n_samples,
                min_votes_block,
                min_votes_verify,
                temperature,
                prompt_style,
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

/// Validate + normalize a CLOUD base URL for Censor. The deliberate exception to the
/// loopback rule (Cloud is the one path that egresses file content off-device, strictly
/// opt-in), but with the SAME SSRF/privacy hardening as the TS `validateCloudBaseUrl` and
/// the Rust `local_coder::validate_cloud_base_url` so a value the UI accepts the backend
/// accepts and vice-versa (config.json is the real trust boundary — the UI is only a gate):
/// REQUIRE `https://` (TLS); reject userinfo (`user@host`), IPv6 literals, `localhost`, bare
/// IPv4 / numeric-quad literals, the cloud-metadata FQDN + `.internal`/`.local` intranet
/// suffixes, and single-label hosts (require a dot); each DNS label must be alnum+hyphen.
/// Same empty / length-cap / forbidden-char guards; trailing slash stripped.
fn validate_cloud_base_for_censor(base: &str) -> Result<String, String> {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return Err("Cloud censorLocalAi requires a base URL.".into());
    }
    if trimmed.len() > OMLX_BASE_URL_MAX_LEN {
        return Err(format!(
            "Cloud base URL must be at most {OMLX_BASE_URL_MAX_LEN} characters."
        ));
    }
    if trimmed
        .chars()
        .any(crate::backend::mini_coder::is_forbidden_command_char)
    {
        return Err(
            "Cloud base URL must not contain control, bidi or invisible characters.".into(),
        );
    }
    let Some(rest) = trimmed.strip_prefix("https://") else {
        return Err("Cloud base URL must be an https origin.".into());
    };
    // Authority = everything before the first path/query/fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    // Userinfo hides the real host + credentials belong in the Authorization header.
    if authority.contains('@') {
        return Err("Cloud base URL must not contain userinfo (user@host).".into());
    }
    // IPv6 literal `[..]`: a cloud provider is addressed by hostname, not a raw IP.
    if authority.starts_with('[') {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }
    // Split off an optional `:port`.
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = port {
        if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Cloud base URL has an invalid :port.".into());
        }
        if p.parse::<u32>().map(|n| n > 65535).unwrap_or(true) {
            return Err("Cloud base URL has an invalid :port.".into());
        }
    }
    if host.is_empty() {
        return Err("Cloud base URL must include a host.".into());
    }
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost" {
        return Err("Cloud base URL must be a remote host (not localhost).".into());
    }
    // Bare IPv4 / numeric dotted-quad literals are an SSRF surface (e.g. 169.254.169.254).
    let labels: Vec<&str> = host.split('.').collect();
    let is_numeric_quad = labels.len() == 4
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if is_numeric_quad {
        return Err("Cloud base URL must be a hostname, not an IP literal.".into());
    }
    // Cloud-metadata FQDN + conventional intranet suffixes (partial SSRF mitigation, mirrors
    // the TS/local_coder rule; full protection needs post-DNS IP filtering — a follow-up).
    if host_lower == "metadata.google.internal"
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".local")
    {
        return Err("Cloud base URL targets a disallowed intranet/metadata host.".into());
    }
    // Require a dot so a single-label intranet name can't be targeted, and each DNS label
    // must be non-empty alphanumeric + hyphen.
    if !host.contains('.') {
        return Err("Cloud base URL must be a fully-qualified host (needs a dot).".into());
    }
    if !labels
        .iter()
        .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    {
        return Err("Cloud base URL has an invalid host label.".into());
    }
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

/// Normalize a configured `promptStyle` to the canonical stored token, or `None` for an
/// absent/unknown value (which resolves to the default `gemma` style in
/// [`CensorLocalAi::review_params`]). Case-insensitive; only `gemma` / `censor_v2` are
/// accepted so a typo can never silently select an unintended style.
fn normalize_prompt_style(raw: Option<&str>) -> Option<String> {
    match raw.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("gemma") => Some("gemma".to_string()),
        Some(s) if s.eq_ignore_ascii_case("censor_v2") => Some("censor_v2".to_string()),
        _ => None,
    }
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

    /// Run ONE generation with a SEPARATE `system` + `user` message pair at an explicit
    /// `temperature` (the voting tier uses a non-zero temperature so the k samples differ).
    ///
    /// The DEFAULT impl (used by providers with only a single-prompt endpoint, e.g.
    /// AppleFm's CLI) folds `system` + "\n\n" + `user` into one prompt and delegates to
    /// [`generate`]; this matches the byte layout [`build_prompt`] produces (its
    /// [`SYSTEM_INSTRUCTION`] is followed by exactly "\n\n" then the user body), so the
    /// concatenating providers see an identical prompt. `temperature` is ignored by the
    /// default because [`generate`]'s endpoint carries no temperature knob for those
    /// providers. Providers WITH a temperature-capable path override this:
    ///   - the OpenAI-compatible oMLX/Cloud client sends TWO real messages
    ///     `[{role:system}, {role:user}]` at the given temperature;
    ///   - the Ollama client concatenates (single `/api/generate` prompt) but threads the
    ///     temperature through `options.temperature`.
    fn generate_chat(
        &self,
        system: &str,
        user: &str,
        temperature: f32,
    ) -> Result<String, GemmaError> {
        // Default: no temperature-capable path — fold into one prompt and reuse `generate`.
        let _ = temperature;
        let prompt = if system.is_empty() {
            user.to_string()
        } else {
            format!("{system}\n\n{user}")
        };
        self.generate(&prompt)
    }

    /// Run ONE MULTIMODAL generation: `prompt` text PLUS one or more base64-encoded images
    /// (the design visual-critique path passes a single captured PNG). Returns the model's
    /// raw text response, parsed defensively by the caller. The DEFAULT impl reports
    /// [`GemmaError::Transport`] (a provider with no vision pathway is treated as
    /// unavailable for this feature) so only providers that genuinely support image input
    /// (Ollama's `/api/generate` `images` field) need override it. PRIVACY: the images
    /// travel ONLY to the loopback endpoint the client is clamped to — same guarantee as
    /// [`generate`]; the bytes are never logged.
    fn generate_with_images(
        &self,
        _prompt: &str,
        _images_b64: &[String],
    ) -> Result<String, GemmaError> {
        Err(GemmaError::Transport)
    }

    /// A stable, content-free IDENTITY label for the provider behind this client
    /// (`"ollama"` / `"omlx"`). Used ONLY for the once-per-session available/unavailable
    /// log line and for testing the factory wiring. NEVER includes the base URL, model,
    /// file content, or any path — identity only (the privacy header forbids logging the
    /// endpoint/content).
    fn provider_label(&self) -> &'static str;

    /// The EFFECTIVE model tag this client drives (the user-configured Ollama/oMLX model,
    /// or `""` when none is configured — opt-in, tier off). Used ONLY alongside [`provider_label`] in
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

    /// Current server load from the backend: `(loaded_model_count, memory_bytes_in_use)`.
    /// Returns `None` when not determinable (AppleFM, Cloud, or unreachable).
    /// Used by the Pigeon censor-pool to skip reviews when another model is loaded.
    fn server_load(&self) -> Option<(usize, u64)> {
        None
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
        let resp = self
            .http
            .get(&url)
            .timeout(self.probe_timeout)
            .send()
            .ok()?;
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

    /// POST `/api/generate` with `prompt` at an explicit `temperature`, returning the
    /// model's raw text. The SINGLE place the text request is built so `generate` (legacy
    /// temperature) and `generate_chat` (configured temperature) can never drift on payload
    /// shape / OOM cap. The model is the resolution-chain result (reused from the probe).
    fn post_generate(&self, prompt: &str, temperature: f32) -> Result<String, GemmaError> {
        let model = self.resolved_model();
        let url = format!("{}/api/generate", self.base);
        let payload = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "options": { "temperature": temperature }
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
        // OOM defense: read the body with a hard size cap BEFORE deserializing (a runaway
        // local model with stream:false could otherwise emit hundreds of MiB).
        let bytes = resp.bytes().map_err(|_| GemmaError::Decode)?;
        parse_generate_body(&bytes)
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
    /// `Some(_)` ONLY for the Cloud provider → a `Bearer` Authorization header is sent AND a
    /// non-loopback https base is permitted (the off-device exception). `None` = local oMLX:
    /// loopback-clamped, no auth header (the privacy fail-safe is unchanged).
    api_key: Option<String>,
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
        Self::with_config(base_url, model, GEMMA_GENERATE_TIMEOUT, GEMMA_PROBE_TIMEOUT)
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
        // LOCAL oMLX: no key → loopback-only clamp (the existing privacy fail-safe). Every
        // pre-cloud caller keeps this exact behavior.
        Self::with_config_and_key(base, model, None, generate_timeout, probe_timeout)
    }

    /// Construct with explicit config AND an optional Cloud API key. The base clamp is
    /// BRANCH-AWARE on `api_key`:
    /// - `None` (LOCAL oMLX): accept `base` ONLY if it is a loopback http origin within the
    ///   length cap and free of control/bidi/invisible chars — the UNCHANGED privacy
    ///   fail-safe; any failure clamps to [`OMLX_DEFAULT_BASE`] so file content can never
    ///   leave the device even on a bad base.
    /// - `Some(_)` (CLOUD): accept `base` only if it passes [`validate_cloud_base_for_censor`]
    ///   (https origin, remote allowed — the deliberate off-device egress). On failure clamp
    ///   to [`OMLX_DEFAULT_BASE`] (an unreachable localhost) so a misconfigured cloud base can
    ///   NEVER silently point at an UNEXPECTED remote host. The key drives a `Bearer` header
    ///   in [`generate`]/[`probe`] and is NEVER logged (excluded from `cache_identity`).
    pub(crate) fn with_config_and_key(
        base: &str,
        model: &str,
        api_key: Option<&str>,
        generate_timeout: Duration,
        probe_timeout: Duration,
    ) -> Self {
        // Shared hardening for BOTH branches: length cap + no control/bidi/invisible chars.
        let base = if base.len() <= OMLX_BASE_URL_MAX_LEN
            && !base
                .chars()
                .any(crate::backend::mini_coder::is_forbidden_command_char)
        {
            match api_key {
                // CLOUD: https remote allowed; invalid → unreachable loopback default.
                Some(_) => match validate_cloud_base_for_censor(base) {
                    Ok(normalized) => normalized,
                    Err(_) => {
                        eprintln!(
                            "censor gemma: refusing invalid cloud base; falling back to loopback default"
                        );
                        OMLX_DEFAULT_BASE.to_string()
                    }
                },
                // LOCAL: loopback only (the unchanged privacy guarantee).
                None => {
                    if is_loopback_omlx_base(base) {
                        base.to_string()
                    } else {
                        eprintln!(
                            "censor gemma: refusing invalid oMLX base; falling back to loopback default"
                        );
                        OMLX_DEFAULT_BASE.to_string()
                    }
                }
            }
        } else {
            eprintln!("censor gemma: refusing invalid oMLX base; falling back to loopback default");
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
            api_key: api_key.map(str::to_string),
            generate_timeout,
            probe_timeout,
        }
    }

    /// POST `<base>/chat/completions` with the given `messages` array + `temperature`,
    /// returning the assistant text. The SINGLE place the request is built so `generate`
    /// (one user message, legacy temperature) and `generate_chat` (system+user, configured
    /// temperature) can never drift on payload shape / OOM cap / Bearer auth. PRIVACY: the
    /// Bearer header is sent ONLY for the Cloud provider (`api_key.is_some()`); local oMLX
    /// sends none. Same hard body-size cap before deserializing as every other call here.
    fn post_chat(
        &self,
        messages: serde_json::Value,
        temperature: f32,
    ) -> Result<String, GemmaError> {
        let url = format!("{}/chat/completions", self.base);
        let payload = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "temperature": temperature
        });
        let mut req = self
            .http
            .post(&url)
            .timeout(self.generate_timeout)
            .json(&payload);
        if let Some(k) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let resp = req.send().map_err(|e| {
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
        let bytes = resp.bytes().map_err(|_| GemmaError::Decode)?;
        parse_openai_chat_body(&bytes)
    }
}

fn apple_fm_respond_args(model: Option<&str>) -> Vec<String> {
    let mut args = vec!["respond".to_string()];
    if let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args
}

fn read_capped_thread<R: std::io::Read + Send + 'static>(
    mut reader: R,
    cap: usize,
) -> std::thread::JoinHandle<Result<Vec<u8>, GemmaError>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut limited = reader.by_ref().take((cap + 1) as u64);
        limited
            .read_to_end(&mut out)
            .map_err(|_| GemmaError::Decode)?;
        if out.len() > cap {
            return Err(GemmaError::Decode);
        }
        Ok(out)
    })
}

fn write_stdin_thread(
    mut stdin: std::process::ChildStdin,
    prompt: Vec<u8>,
) -> std::thread::JoinHandle<Result<(), GemmaError>> {
    std::thread::spawn(move || {
        use std::io::Write;
        stdin
            .write_all(&prompt)
            .map_err(|_| GemmaError::Transport)?;
        Ok(())
    })
}

fn run_apple_fm_respond_process(
    program: &Path,
    args: &[String],
    prompt: &str,
    timeout: Duration,
) -> Result<String, GemmaError> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PATH", crate::backend::provider_detect::augmented_path());

    let mut child = cmd.spawn().map_err(|_| GemmaError::Transport)?;
    let stdout = child.stdout.take().ok_or(GemmaError::Transport)?;
    let stderr = child.stderr.take().ok_or(GemmaError::Transport)?;
    let stdin = child.stdin.take().ok_or(GemmaError::Transport)?;

    let stdout_thread = read_capped_thread(stdout, RESPONSE_BODY_CAP);
    let stderr_thread = read_capped_thread(stderr, 16 * 1024);
    let stdin_thread = write_stdin_thread(stdin, prompt.as_bytes().to_vec());

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().map_err(|_| GemmaError::Transport)? {
            Some(status) => break status,
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdin_thread.join();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(GemmaError::Timeout);
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let stdin_result = stdin_thread.join().map_err(|_| GemmaError::Transport)?;
    let stdout_result = stdout_thread.join().map_err(|_| GemmaError::Decode)?;
    let _ = stderr_thread.join();

    if !status.success() {
        return Err(GemmaError::Status(status.code().unwrap_or(1) as u16));
    }
    stdin_result?;
    let stdout_bytes = stdout_result?;
    String::from_utf8(stdout_bytes).map_err(|_| GemmaError::Decode)
}

#[cfg(target_os = "macos")]
pub struct AppleFmClient {
    model: Option<String>,
    generate_timeout: Duration,
}

#[cfg(target_os = "macos")]
impl AppleFmClient {
    pub(crate) fn with_config(model: Option<&str>, generate_timeout: Duration) -> Self {
        Self {
            model: model
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            generate_timeout,
        }
    }
}

#[cfg(target_os = "macos")]
impl GemmaClient for AppleFmClient {
    fn probe(&self) -> bool {
        crate::backend::provider_detect::resolve_program("fm").is_some()
    }

    fn generate(&self, prompt: &str) -> Result<String, GemmaError> {
        let program =
            crate::backend::provider_detect::resolve_program("fm").ok_or(GemmaError::Transport)?;
        run_apple_fm_respond_process(
            &program,
            &apple_fm_respond_args(self.model.as_deref()),
            prompt,
            self.generate_timeout,
        )
    }

    fn provider_label(&self) -> &'static str {
        "appleFm"
    }

    fn model_label(&self) -> String {
        self.model.clone().unwrap_or_else(|| "default".to_string())
    }

    fn cache_identity(&self) -> String {
        format!("appleFm||{}", self.model.as_deref().unwrap_or(""))
    }
}

impl GemmaClient for OmlxClient {
    fn probe(&self) -> bool {
        // GET <base>/models → OpenAI list-models { "data": [ { "id": "<model>" }, ... ] }.
        // Reachable AND our configured model present → available (mirrors the Ollama
        // probe's "reachable AND model present" so the tier degrades identically when the
        // server is up but the model isn't pulled).
        let url = format!("{}/models", self.base);
        let mut req = self.http.get(&url).timeout(self.probe_timeout);
        // Cloud provider only: authenticate the remote list-models probe.
        if let Some(k) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let resp = match req.send() {
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
        // LEGACY single-message call: one user message at the legacy temperature (kept
        // byte-identical to the pre-voting behavior — the folded `build_prompt` string is
        // sent as a single user turn). `generate_chat` is the voting-tier entry point.
        self.post_chat(
            serde_json::json!([{ "role": "user", "content": prompt }]),
            LEGACY_GEMMA_TEMPERATURE,
        )
    }

    fn generate_chat(
        &self,
        system: &str,
        user: &str,
        temperature: f32,
    ) -> Result<String, GemmaError> {
        // oMLX/Cloud is chat-native: send a REAL system message + the user body (a better
        // shape for an instruct model than folding the system text into the user turn).
        let messages = if system.is_empty() {
            serde_json::json!([{ "role": "user", "content": user }])
        } else {
            serde_json::json!([
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ])
        };
        self.post_chat(messages, temperature)
    }

    fn provider_label(&self) -> &'static str {
        // A keyed client is the Cloud provider; an unkeyed one is local oMLX. Both are
        // 'static — the label is identity-only (never the base/key).
        if self.api_key.is_some() {
            "cloud"
        } else {
            "omlx"
        }
    }

    fn model_label(&self) -> String {
        self.model.clone()
    }

    fn cache_identity(&self) -> String {
        // Fold in the base so changing the base (same provider) re-probes. The PREFIX tracks
        // cloud-vs-local so a cloud client never collides with a local oMLX one in the probe
        // cache. NEVER logged — opaque in-memory key only; the api_key is deliberately
        // EXCLUDED (it could otherwise leak into a log of this identity).
        let prefix = if self.api_key.is_some() {
            "cloud"
        } else {
            "omlx"
        };
        format!("{}|{}|{}", prefix, self.base, self.model)
    }

    fn server_load(&self) -> Option<(usize, u64)> {
        let url = format!("{}/health", self.base);
        let resp = self
            .http
            .get(&url)
            .timeout(self.probe_timeout)
            .send()
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: serde_json::Value = resp.json().ok()?;
        let pool = body.get("engine_pool")?;
        let count = pool.get("loaded_count")?.as_u64()? as usize;
        let mem = pool.get("current_model_memory")?.as_u64()?;
        Some((count, mem))
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
        // it. OPT-IN: a configured override is used (wins regardless of tags); with NO
        // configured model the resolver returns "" and the next line reports unavailable.
        let resolved = resolve_gemma_model(self.configured_model.as_deref(), &tags);
        if let Ok(mut guard) = self.resolved_model.lock() {
            *guard = Some(resolved.clone());
        }
        // Reachable AND the resolved model present → available (a configured override that
        // is not yet pulled correctly reports unavailable: the tier degrades, no crash).
        tags.iter().any(|t| t == &resolved)
    }

    fn generate(&self, prompt: &str) -> Result<String, GemmaError> {
        // LEGACY single-prompt call at the legacy temperature — byte-identical to the
        // pre-voting behavior. `generate_chat` is the voting-tier entry point (it threads
        // a configured temperature through the SAME `post_generate` path).
        self.post_generate(prompt, LEGACY_GEMMA_TEMPERATURE)
    }

    fn generate_chat(
        &self,
        system: &str,
        user: &str,
        temperature: f32,
    ) -> Result<String, GemmaError> {
        // Ollama's `/api/generate` is single-prompt (no chat roles), so fold system + user
        // into one prompt (identical byte layout to `build_prompt`) but thread the
        // configured temperature through so the k samples actually differ.
        let prompt = if system.is_empty() {
            user.to_string()
        } else {
            format!("{system}\n\n{user}")
        };
        self.post_generate(&prompt, temperature)
    }

    fn generate_with_images(
        &self,
        prompt: &str,
        images_b64: &[String],
    ) -> Result<String, GemmaError> {
        // Same loopback `/api/generate` call as `generate`, with the Ollama multimodal
        // `images` field carrying the base64 PNG(s). `stream:false`, low temperature. The
        // body is read with the SAME hard size cap before parsing (OOM defense). The
        // resolved model must be a vision-capable tag for a useful answer; a text-only
        // model simply returns prose ignoring the image (no crash). The images travel only
        // to the loopback daemon (the base is clamped) — never logged.
        let model = self.resolved_model();
        let url = format!("{}/api/generate", self.base);
        let payload = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "images": images_b64,
            "stream": false,
            "options": { "temperature": LEGACY_GEMMA_TEMPERATURE }
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
            .unwrap_or("")
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

    fn server_load(&self) -> Option<(usize, u64)> {
        let url = format!("{}/api/ps", self.base);
        let resp = self
            .http
            .get(&url)
            .timeout(self.probe_timeout)
            .send()
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: serde_json::Value = resp.json().ok()?;
        let models = body.get("models")?.as_array()?;
        let count = models.len();
        let mem: u64 = models
            .iter()
            .filter_map(|m| m.get("size_vram")?.as_u64())
            .sum();
        Some((count, mem))
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
///   - [`CensorAiProvider::Ollama`] (the default provider when `censorLocalAi` is absent)
///     → an [`OllamaClient`] at the effective base, taking the CONFIGURED model override.
///     OPT-IN: with no override the model resolves to "" and the probe reports the tier
///     off, so an unconfigured Ollama censor never runs — the user must pick a model
///     (e.g. the recommended [`GEMMA_MODEL`]). Base + timeouts use [`OLLAMA_BASE`] / the
///     default generate/probe timeouts.
///   - [`CensorAiProvider::Omlx`] → an [`OmlxClient`] at the effective base/model (the
///     base is loopback-clamped inside [`OmlxClient::with_config`], a privacy fail-safe).
///
/// PRIVACY: `cfg` is expected to be the output of [`validate_censor_local_ai`] (via
/// `read_censor_local_ai`), so the base is already validated loopback; the client
/// constructor re-clamps a non-loopback base as defense in depth. The base/model are
/// NEVER logged here — provider identity only (see [`GemmaClient::provider_label`]).
pub(crate) fn build_gemma_client(cfg: &CensorLocalAi) -> Result<Box<dyn GemmaClient>, String> {
    // No key: every local provider (Ollama/oMLX/AppleFm) keeps its exact prior behavior.
    build_gemma_client_with_key(cfg, None)
}

/// Build the Censor tier-2 client, threading an optional Cloud API key. Identical to
/// [`build_gemma_client`] for every local provider (which IGNORE `api_key`); ONLY the
/// `Cloud` provider consumes it (passed to [`OmlxClient::with_config_and_key`] for the
/// `Bearer` header). The key is read from the OS vault by the caller
/// (`censor_review::build_censor_client`) — never from `cfg`.
pub(crate) fn build_gemma_client_with_key(
    cfg: &CensorLocalAi,
    api_key: Option<&str>,
) -> Result<Box<dyn GemmaClient>, String> {
    let base = cfg.effective_base();
    match cfg.provider {
        // The Ollama client takes the CONFIGURED override (`ollama_model`, may be `None`)
        // and resolves the effective tag itself via [`resolve_gemma_model`]. OPT-IN: with
        // no override it resolves to "" and the probe reports the tier unavailable (off),
        // so an unconfigured censor never runs — the user must pick a model.
        CensorAiProvider::Ollama => Ok(Box::new(OllamaClient::with_config(
            &base,
            cfg.ollama_model.as_deref(),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        ))),
        // oMLX has no `/api/tags` equivalent; its model is REQUIRED + validated, so the
        // configured `effective_model` is used verbatim (opt-in: no auto-default). LOCAL —
        // `with_config` delegates to the keyless (`None`) clamp branch, so the loopback
        // privacy fail-safe stays intact.
        CensorAiProvider::Omlx => Ok(Box::new(OmlxClient::with_config(
            &base,
            &cfg.effective_model(),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        ))),
        #[cfg(target_os = "macos")]
        CensorAiProvider::AppleFm => Ok(Box::new(AppleFmClient::with_config(
            cfg.model.as_deref(),
            GEMMA_GENERATE_TIMEOUT,
        ))),
        #[cfg(not(target_os = "macos"))]
        CensorAiProvider::AppleFm => Err("Apple on-device requires macOS 27+.".to_string()),
        // Cloud reuses the OpenAI-compatible OmlxClient WITH the key → `Bearer` auth + the
        // https (non-loopback) base permitted by the keyed clamp branch.
        CensorAiProvider::Cloud => Ok(Box::new(OmlxClient::with_config_and_key(
            &base,
            &cfg.effective_model(),
            api_key,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        ))),
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
NO HALLUCINATIONS -- this is the rule that matters most. Before you report anything, \
RE-READ the lines around it for a guard that ALREADY handles the case: an if/else, an \
early return, a default, a try/catch, a ternary or match arm, a `!= -1`/null check. If \
such a guard exists, it is NOT a finding. Report a smell ONLY when you can point to the \
exact line AND the concrete input or code path that makes it go wrong; ban the words \
might, could, may, possibly. A false positive is WORSE than a miss -- it sends the author \
to break working code -- so when in doubt, output nothing.\n\
Output ONLY a JSON array (no prose, no markdown fences) of objects with EXACTLY these \
keys: {\"line\": <integer 1-based>, \"title\": <short string>, \"body\": <one-sentence \
string>, \"severity\": one of \"high\" | \"medium\" | \"low\"}. If there is nothing to \
report, output [].";

/// The `censor_v2` system prompt — the EXACT system message the fine-tuned reviewer was
/// trained on (copied verbatim from the first row's `system` turn in
/// `review-experts/data_cot/sft_v2fix_7030/train.jsonl`). It asks for a short `<think>`
/// block then a typed-category JSON array with a `rationale` field; the response is parsed
/// by [`parse_censor_v2`] (which strips the `<think>` block and maps the typed categories +
/// `error|warning|info` severities onto our schema). A raw string literal keeps the many
/// embedded quotes readable; DO NOT reword it — it must match the training distribution.
const CENSOR_V2_SYSTEM: &str = r#"You are a Rust code reviewer. Think briefly (<=200 tokens) inside <think></think>, then output ONLY a JSON array of findings. Each: {"line":int,"severity":"error|warning|info","category":"<one of: correctness,logic-error,off-by-one,panic-risk,error-handling,security,performance,style,maintainability,naming,api-misuse>","title":"<=200 chars","rationale":"<=200 chars"}. Use severity error for real bugs/correctness/security, warning for likely issues, info for style/nits. If the code is correct, output []. Do not re-report anything in ALREADY-KNOWN."#;

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
    // Legacy layout: the fixed system instruction, then EXACTLY "\n\n", then the user body.
    // `build_user_body` starts with "FILE: ", so this reproduces the original byte layout
    // (`SYSTEM_INSTRUCTION` + "\n\nFILE: " + …) verbatim — and it also equals what the
    // concatenating `generate_chat` path builds (`system + "\n\n" + user`).
    let body = build_user_body(file_rel_path, file_content, deterministic_findings);
    let mut p = String::with_capacity(SYSTEM_INSTRUCTION.len() + 2 + body.len());
    p.push_str(SYSTEM_INSTRUCTION);
    p.push_str("\n\n");
    p.push_str(&body);
    p
}

/// Build the USER body (everything after the system prompt): the file path + capped
/// content + the ALREADY-KNOWN deterministic list + the final instruction. PURE. Shared by
/// BOTH prompt styles — the `gemma` style pairs it with [`SYSTEM_INSTRUCTION`], the
/// `censor_v2` style with [`CENSOR_V2_SYSTEM`] — and by [`build_prompt`] (which folds it
/// under the system prompt for the legacy single-prompt path). Factored out of the old
/// `build_prompt` unchanged so the rendered body is byte-identical to before.
pub fn build_user_body(
    file_rel_path: &str,
    file_content: &str,
    deterministic_findings: &[RawFinding],
) -> String {
    let mut p = String::with_capacity(file_content.len().min(MAX_FILE_CHARS) + 2048);
    p.push_str("FILE: ");
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

/// Parse a `censor_v2` model response into `RawFinding`s. Same defensive contract as
/// [`parse_gemma`] (never panics; caps at [`MAX_GEMMA_FINDINGS`]; redacts + caps
/// title/body) but for the fine-tuned reviewer's output shape:
///   - the model may emit a leading `<think>…</think>` reasoning block — it is STRIPPED
///     first (via [`strip_think_block`]) so a `[` inside the reasoning can't be mistaken
///     for the findings array; then the balanced array is extracted the same way;
///   - each object uses `rationale` (mapped to `body`), a typed `category` string (mapped
///     best-effort onto our [`Category`] via [`category_from_v2_token`], unknown →
///     `Correctness`), and an `error|warning|info` `severity` (via
///     [`severity_from_v2_token`]).
///
/// `source` stays `"gemma"` so the orchestrator's gemma-source clobber protection is
/// unchanged regardless of which prompt style produced the finding.
pub fn parse_censor_v2(file_rel_path: &str, raw_response: &str) -> Vec<RawFinding> {
    let cleaned = strip_think_block(raw_response);
    let Some(array_text) = extract_json_array(&cleaned) else {
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
            continue;
        };
        let title_raw = obj
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title_raw.is_empty() {
            continue;
        }
        // v2 carries the explanation in `rationale` (not `body`).
        let body_raw = obj
            .get("rationale")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let line = obj.get("line").and_then(|v| v.as_u64()).and_then(|n| {
            if n >= 1 && n <= u32::MAX as u64 {
                Some(n as u32)
            } else {
                None
            }
        });
        let severity =
            severity_from_v2_token(obj.get("severity").and_then(|v| v.as_str()).unwrap_or(""));
        let category =
            category_from_v2_token(obj.get("category").and_then(|v| v.as_str()).unwrap_or(""));

        let title = cap(&redact_secrets(title_raw), TITLE_CAP);
        let body = cap(&redact_secrets(body_raw), BODY_CAP);

        out.push(RawFinding {
            file: file_rel_path.to_string(),
            line,
            severity,
            category,
            source: "gemma".to_string(),
            title,
            body,
        });
    }
    out
}

/// Strip `<think>…</think>` reasoning blocks from a model response before JSON extraction.
/// Removes EVERY `<think>` … matching `</think>` span (inclusive) — not just the first — so
/// a stray second block whose text contains a `[` cannot hijack [`extract_json_array`]. If
/// an opening tag has no matching close (truncated reasoning), everything from that
/// `<think>` onward is dropped (no valid findings array lives inside an unterminated think
/// block). Returns the text unchanged when no `<think>` is present. PURE.
fn strip_think_block(text: &str) -> String {
    let mut s = text.to_string();
    while let Some(start) = s.find("<think>") {
        if let Some(end_rel) = s[start..].find("</think>") {
            let end = start + end_rel + "</think>".len();
            let mut next = String::with_capacity(s.len());
            next.push_str(&s[..start]);
            next.push_str(&s[end..]);
            s = next;
        } else {
            s.truncate(start);
            break;
        }
    }
    s
}

/// Map a `censor_v2` severity token (`error|warning|info`) onto our `Severity`.
/// Case-insensitive; `error`→High, `info`→Low, everything else (`warning` + unknown)→Medium.
fn severity_from_v2_token(token: &str) -> Severity {
    match token.trim().to_ascii_lowercase().as_str() {
        "error" => Severity::High,
        "info" => Severity::Low,
        _ => Severity::Medium,
    }
}

/// Map a `censor_v2` typed category string best-effort onto our coarser [`Category`].
/// Case-insensitive; anything unrecognized (including the correctness-family buckets
/// `logic-error`/`off-by-one`/`panic-risk`/`error-handling`/`api-misuse`) → `Correctness`,
/// the neutral non-security default.
fn category_from_v2_token(token: &str) -> Category {
    match token.trim().to_ascii_lowercase().as_str() {
        "security" => Category::Security,
        "performance" => Category::Complexity,
        "style" | "maintainability" | "naming" => Category::Style,
        _ => Category::Correctness,
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
///   - otherwise generate + parse per `params`.
///
/// `params` carries the resolved voting/temperature/prompt-style knobs (see
/// [`CensorLocalAi::review_params`]).
///
/// LEGACY FAST PATH: with the default single-sample gemma style at the legacy temperature
/// (`params == GemmaReviewParams::default()`) this is BYTE-IDENTICAL to the pre-voting
/// engine — one `client.generate(build_prompt(...))` call parsed by [`parse_gemma`].
///
/// VOTED PATH (any of: `n_samples > 1`, a non-`gemma` style, or a non-legacy temperature):
/// the file is reviewed `n_samples` times via [`GemmaClient::generate_chat`] (a system +
/// user split at the configured temperature so the samples differ), each response parsed
/// ([`parse_gemma`] / [`parse_censor_v2`]); a failed generate counts as an EMPTY sample
/// (never aborts the file). The per-sample findings are clustered + voted
/// ([`crate::backend::censor::votes`]) and the confirmed + (flagged) suspect findings are
/// returned.
///
/// On ANY generate error/timeout the failure is logged ONCE per sample (provider + the
/// model actually in use + the file's project-relative path ONLY; NEVER the file content,
/// prompt, base URL, or model output, per the module privacy header). `_root` is accepted
/// for the future context seam; it is unused today.
pub fn run_gemma(
    client: &dyn GemmaClient,
    available: bool,
    _root: &Path,
    file_rel_path: &str,
    file_content: &str,
    deterministic: &[RawFinding],
    params: &GemmaReviewParams,
) -> Vec<RawFinding> {
    if !available {
        return Vec::new();
    }
    let n_samples = params.vote.n_samples.max(1);

    // LEGACY byte-identical fast path: default single-sample gemma review at the legacy
    // temperature. Preserves the exact pre-voting request (one folded prompt as a single
    // turn) — important for the oMLX path, where `generate_chat` would otherwise split into
    // two messages.
    if n_samples <= 1
        && params.prompt_style == PromptStyle::Gemma
        && is_legacy_temperature(params.temperature)
    {
        let prompt = build_prompt(file_rel_path, file_content, deterministic);
        return match client.generate(&prompt) {
            Ok(response) => parse_gemma(file_rel_path, &response),
            Err(e) => {
                log_generate_failure(client, file_rel_path, &e);
                Vec::new()
            }
        };
    }

    // Voted / non-default path: one shared user body, the style-specific system prompt.
    let user = build_user_body(file_rel_path, file_content, deterministic);
    let system: &str = match params.prompt_style {
        PromptStyle::Gemma => SYSTEM_INSTRUCTION,
        PromptStyle::CensorV2 => CENSOR_V2_SYSTEM,
    };

    // TODO(concurrency): fan the n samples out concurrently. Sequential is fine for v1 — a
    // local single-model server serializes the requests anyway, so parallelism would only
    // add contention without a wall-clock win on the common local setup.
    let mut samples: Vec<Vec<RawFinding>> = Vec::with_capacity(n_samples as usize);
    for _ in 0..n_samples {
        // TODO(cancellation): check the orchestrator stop flag between samples so a mid-file
        // stop aborts the remaining generations (needs a stop-flag param — out of scope here).
        match client.generate_chat(system, &user, params.temperature) {
            Ok(response) => {
                let parsed = match params.prompt_style {
                    PromptStyle::Gemma => parse_gemma(file_rel_path, &response),
                    PromptStyle::CensorV2 => parse_censor_v2(file_rel_path, &response),
                };
                samples.push(parsed);
            }
            Err(e) => {
                // A failed sample simply casts no votes — count it as EMPTY, never abort.
                log_generate_failure(client, file_rel_path, &e);
                samples.push(Vec::new());
            }
        }
    }

    let voted = crate::backend::censor::votes::cluster_and_vote(samples, &params.vote);
    let (mut confirmed, mut suspects) =
        crate::backend::censor::votes::split_by_threshold(voted, &params.vote);
    // Suspects carry an `[unverified …]` body marker so the verifier role sees they are
    // unconfirmed; they follow the confirmed findings in the returned set.
    confirmed.append(&mut suspects);
    confirmed
}

/// Content-free once-per-call failure log for a generate error: provider identity + the
/// model ACTUALLY in use + the file's project-relative path ONLY. NEVER the prompt, file
/// content, base URL, or model output (module privacy header). Logging the real provider/
/// model (not the hardcoded [`GEMMA_MODEL`]) is what makes an oMLX/Cloud failure triageable.
fn log_generate_failure(client: &dyn GemmaClient, file_rel_path: &str, e: &GemmaError) {
    eprintln!(
        "censor gemma: {} model {} generate failed for {file_rel_path} ({e})",
        client.provider_label(),
        client.model_label()
    );
}

/// Re-export the runners' secret-redaction pass at crate scope so OTHER local-AI
/// callers (the design visual-critique command) can run the SAME `[redacted]` scrub
/// over a local model's free-text response before it is surfaced — instead of each
/// caller hand-rolling a divergent redactor. The heuristic lives in exactly one place
/// (`runners::redact_secrets`); this is a thin, content-free forwarder.
pub(crate) fn redact_secrets_text(s: &str) -> String {
    redact_secrets(s)
}

/// Re-export the runners' char-count truncation cap at crate scope (companion to
/// [`redact_secrets_text`]) so the design critique can bound an over-long model
/// response with the SAME multibyte-safe cap the runners use, rather than a private
/// copy that could drift.
pub(crate) fn cap_chars(s: &str, max: usize) -> String {
    cap(s, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const APPLE_FM_DEADLOCK_CANDIDATE_ENV: &str = "ASPIS_TEST_APPLE_FM_DEADLOCK_CANDIDATE";
    const APPLE_FM_DEADLOCK_STUB_ENV: &str = "ASPIS_TEST_APPLE_FM_DEADLOCK_STUB";

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

    /// A stub that returns a DIFFERENT canned response per successive `generate` call
    /// (used to test the voting path where samples disagree). Uses the trait-default
    /// `generate_chat` (fold system+user → `generate`), so each voted sample pops the next
    /// response. Runs out → yields an empty JSON array.
    struct SeqStubClient {
        responses: std::sync::Mutex<std::collections::VecDeque<Result<String, GemmaError>>>,
        calls: Arc<AtomicUsize>,
    }
    impl SeqStubClient {
        fn new(responses: Vec<Result<String, GemmaError>>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into_iter().collect()),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }
    impl GemmaClient for SeqStubClient {
        fn probe(&self) -> bool {
            true
        }
        fn generate(&self, _prompt: &str) -> Result<String, GemmaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("[]".to_string()))
        }
        fn provider_label(&self) -> &'static str {
            "stub"
        }
        fn model_label(&self) -> String {
            "stub-model".to_string()
        }
    }

    // ---- new prompt/body refactor identity ----

    #[test]
    fn build_prompt_equals_system_plus_user_body() {
        // The refactor must be byte-identical: build_prompt == SYSTEM + "\n\n" + user body.
        let dets = [det(3, "known smell")];
        let full = build_prompt("src/a.rs", "let x = 1;", &dets);
        let body = build_user_body("src/a.rs", "let x = 1;", &dets);
        assert_eq!(full, format!("{SYSTEM_INSTRUCTION}\n\n{body}"));
        // And the body carries no system header.
        assert!(!body.contains("You are a careful code reviewer"));
        assert!(body.starts_with("FILE: src/a.rs"));
    }

    // ---- review_params resolution ----

    #[test]
    fn review_params_defaults_to_legacy_single_sample() {
        let p = CensorLocalAi::default().review_params();
        assert_eq!(p, GemmaReviewParams::default());
        assert_eq!(p.vote.n_samples, 1);
        assert_eq!(p.vote.min_votes_block, 1);
        assert_eq!(p.vote.min_votes_verify, 1);
        assert!(is_legacy_temperature(p.temperature));
        assert_eq!(p.prompt_style, PromptStyle::Gemma);
    }

    #[test]
    fn review_params_resolves_votes_temperature_and_clamps() {
        let cfg = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            n_samples: Some(5),
            // Over-large block clamps to n_samples; verify defaults to 1.
            min_votes_block: Some(9),
            temperature: Some(0.8),
            prompt_style: Some("censor_v2".to_string()),
            ..Default::default()
        };
        let p = cfg.review_params();
        assert_eq!(p.vote.n_samples, 5);
        assert_eq!(p.vote.min_votes_block, 5, "block clamped to n_samples");
        assert_eq!(p.vote.min_votes_verify, 1);
        assert!((p.temperature - 0.8).abs() < 1e-6);
        assert_eq!(p.prompt_style, PromptStyle::CensorV2);
        // Default block for n>1 with no explicit value is ceil(n/2).
        let d = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            n_samples: Some(3),
            ..Default::default()
        };
        assert_eq!(d.review_params().vote.min_votes_block, 2);
    }

    #[test]
    fn review_params_clamps_verify_to_not_exceed_block() {
        // Inverted config: verify (3) > block (2). Must clamp verify down to block so the
        // suspect window `[verify, block)` is never silently empty due to inversion.
        let cfg = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            n_samples: Some(3),
            min_votes_block: Some(2),
            min_votes_verify: Some(3),
            ..Default::default()
        };
        let p = cfg.review_params();
        assert_eq!(p.vote.min_votes_block, 2);
        assert_eq!(p.vote.min_votes_verify, 2, "verify clamped down to block");
        assert!(p.vote.min_votes_verify <= p.vote.min_votes_block);
    }

    #[test]
    fn review_params_forces_single_sample_for_apple_fm() {
        // AppleFm cannot vary temperature → voting is meaningless → force n_samples = 1
        // regardless of what the config asks for.
        let cfg = CensorLocalAi {
            provider: CensorAiProvider::AppleFm,
            n_samples: Some(7),
            temperature: Some(1.0),
            ..Default::default()
        };
        let p = cfg.review_params();
        assert_eq!(p.vote.n_samples, 1, "AppleFm forced to single sample");
        assert_eq!(p.vote.min_votes_block, 1);
        assert_eq!(p.vote.min_votes_verify, 1);
    }

    #[test]
    fn validate_clamps_out_of_range_voting_fields() {
        let cfg = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            n_samples: Some(50),
            temperature: Some(9.0),
            prompt_style: Some("bogus".to_string()),
            ..Default::default()
        };
        let v = validate_censor_local_ai(&cfg).expect("ollama config is valid");
        assert_eq!(v.n_samples, Some(9), "n_samples clamped to 9");
        assert_eq!(v.temperature, Some(1.5), "temperature clamped to 1.5");
        assert_eq!(v.prompt_style, None, "unknown style normalizes to None");
    }

    // ---- parse_censor_v2 ----

    #[test]
    fn parse_censor_v2_maps_fields_severity_category_and_strips_think() {
        let resp = "<think>reasoning with a [bracket] that must not fool extraction</think>\n\
            [{\"line\": 3, \"severity\": \"error\", \"category\": \"security\", \
             \"title\": \"SQL injection\", \"rationale\": \"user input reaches the query\"}]";
        let out = parse_censor_v2("src/db.rs", resp);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, Some(3));
        assert_eq!(out[0].severity, Severity::High, "error → High");
        assert_eq!(out[0].category, Category::Security);
        assert_eq!(out[0].title, "SQL injection");
        assert_eq!(
            out[0].body, "user input reaches the query",
            "rationale → body"
        );
        assert_eq!(
            out[0].source, "gemma",
            "source stays gemma for clobber-protection"
        );
    }

    #[test]
    fn parse_censor_v2_severity_and_category_fallbacks() {
        let resp = "[\
            {\"line\":1,\"severity\":\"warning\",\"category\":\"performance\",\"title\":\"a\",\"rationale\":\"r\"},\
            {\"line\":2,\"severity\":\"info\",\"category\":\"naming\",\"title\":\"b\",\"rationale\":\"r\"},\
            {\"line\":3,\"severity\":\"???\",\"category\":\"logic-error\",\"title\":\"c\",\"rationale\":\"r\"}]";
        let out = parse_censor_v2("src/a.rs", resp);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].severity, Severity::Medium); // warning
        assert_eq!(out[0].category, Category::Complexity); // performance
        assert_eq!(out[1].severity, Severity::Low); // info
        assert_eq!(out[1].category, Category::Style); // naming
        assert_eq!(out[2].severity, Severity::Medium); // unknown → Medium
        assert_eq!(out[2].category, Category::Correctness); // logic-error → Correctness
    }

    #[test]
    fn strip_think_block_handles_missing_close() {
        assert_eq!(strip_think_block("no tags [1]"), "no tags [1]");
        assert_eq!(strip_think_block("<think>abc</think>[1]"), "[1]");
        // Unterminated think → everything from the tag is dropped (no array survives).
        assert_eq!(strip_think_block("prefix <think>abc [1]"), "prefix ");
    }

    #[test]
    fn strip_think_block_removes_all_blocks_not_just_first() {
        // A second <think> block containing a `[` must not survive to hijack extraction.
        let raw = "<think>first</think> junk <think>trap [999]</think>[{\"line\":1}]";
        assert_eq!(strip_think_block(raw), " junk [{\"line\":1}]");
        // And parse_censor_v2 must therefore extract the REAL array, not the decoy.
        let out = parse_censor_v2(
            "src/a.rs",
            "<think>reason</think><think>[{\"line\":42,\"severity\":\"error\",\"title\":\"decoy\",\"rationale\":\"x\"}]</think>\n\
             [{\"line\":7,\"severity\":\"error\",\"category\":\"security\",\"title\":\"real\",\"rationale\":\"r\"}]",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "real");
        assert_eq!(out[0].line, Some(7));
    }

    // ---- run_gemma voted path ----

    #[test]
    fn run_gemma_voted_confirms_consistent_finding_and_runs_n_samples() {
        let resp =
            r#"[{"line": 7, "title": "Inverted guard", "body": "backwards", "severity": "high"}]"#;
        let c = StubClient::new(true, Ok(resp.into()));
        let params = GemmaReviewParams {
            vote: crate::backend::censor::votes::VoteParams {
                n_samples: 3,
                min_votes_block: 2,
                min_votes_verify: 1,
                line_tolerance: 2,
            },
            temperature: 0.5,
            prompt_style: PromptStyle::Gemma,
        };
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &params,
        );
        assert_eq!(out.len(), 1, "3 agreeing samples → one confirmed finding");
        assert_eq!(out[0].title, "Inverted guard");
        assert!(
            !out[0].body.starts_with("[unverified"),
            "confirmed → no marker"
        );
        assert_eq!(
            c.generate_calls.load(Ordering::SeqCst),
            3,
            "voted path runs n_samples generations"
        );
    }

    #[test]
    fn run_gemma_voted_marks_single_vote_finding_as_unverified_suspect() {
        // Only ONE of three samples reports a smell → 1 vote. block=2, verify=1 → suspect.
        let hit = r#"[{"line": 4, "title": "swallowed error", "body": "empty catch", "severity": "medium"}]"#;
        let c = SeqStubClient::new(vec![Ok(hit.into()), Ok("[]".into()), Ok("[]".into())]);
        let params = GemmaReviewParams {
            vote: crate::backend::censor::votes::VoteParams {
                n_samples: 3,
                min_votes_block: 2,
                min_votes_verify: 1,
                line_tolerance: 2,
            },
            temperature: 0.5,
            prompt_style: PromptStyle::Gemma,
        };
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &params,
        );
        assert_eq!(out.len(), 1);
        assert!(
            out[0].body.starts_with("[unverified 1/3 votes] "),
            "got: {}",
            out[0].body
        );
    }

    #[test]
    fn run_gemma_voted_failed_samples_count_as_empty_no_panic() {
        // All samples error: the file degrades to empty (each failure is an empty sample).
        let c = StubClient::new(true, Err(GemmaError::Timeout));
        let params = GemmaReviewParams {
            vote: crate::backend::censor::votes::VoteParams {
                n_samples: 3,
                min_votes_block: 2,
                min_votes_verify: 1,
                line_tolerance: 2,
            },
            temperature: 0.5,
            prompt_style: PromptStyle::Gemma,
        };
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &params,
        );
        assert!(out.is_empty());
        assert_eq!(
            c.generate_calls.load(Ordering::SeqCst),
            3,
            "all 3 attempted"
        );
    }

    #[test]
    fn run_gemma_legacy_single_sample_uses_plain_generate() {
        // Default params → the legacy fast path: exactly one generate() call, parsed by
        // parse_gemma (byte-identical to the pre-voting behavior).
        let resp = r#"[{"line": 1, "title": "t", "body": "b", "severity": "low"}]"#;
        let c = StubClient::new(true, Ok(resp.into()));
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &GemmaReviewParams::default(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(c.generate_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn apple_fm_generate_does_not_deadlock_when_child_emits_before_reading_large_prompt() {
        if std::env::var_os(APPLE_FM_DEADLOCK_CANDIDATE_ENV).is_some()
            || std::env::var_os(APPLE_FM_DEADLOCK_STUB_ENV).is_some()
        {
            return;
        }

        let exe = std::env::current_exe().expect("test executable");
        let mut child = Command::new(exe)
            .arg("apple_fm_deadlock_candidate_child")
            .arg("--nocapture")
            .env(APPLE_FM_DEADLOCK_CANDIDATE_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn candidate child");

        let start = Instant::now();
        loop {
            if let Some(_status) = child.try_wait().expect("poll candidate") {
                let output = child.wait_with_output().expect("collect candidate");
                assert!(
                    output.status.success(),
                    "candidate failed\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                return;
            }
            if start.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .expect("collect timed-out candidate");
                panic!(
                    "appleFm process runner deadlocked on large stdin + early stdout\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn apple_fm_deadlock_candidate_child() {
        if std::env::var_os(APPLE_FM_DEADLOCK_CANDIDATE_ENV).is_none() {
            return;
        }
        let exe = std::env::current_exe().expect("test executable");
        let prompt = "p".repeat(160 * 1024);
        std::env::set_var(APPLE_FM_DEADLOCK_STUB_ENV, "1");
        let args = vec![
            "apple_fm_deadlock_stub_child".to_string(),
            "--nocapture".to_string(),
        ];
        let output = run_apple_fm_respond_process(&exe, &args, &prompt, Duration::from_secs(5))
            .expect("large prompt should not deadlock");
        assert!(
            output.contains("stub saw 163840 bytes"),
            "stub output missing stdin length: {output}"
        );
    }

    #[test]
    fn apple_fm_deadlock_stub_child() {
        if std::env::var_os(APPLE_FM_DEADLOCK_STUB_ENV).is_none() {
            return;
        }
        use std::io::{Read, Write};

        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&vec![b'x'; 192 * 1024])
            .expect("write early stdout");
        stdout.write_all(b"\nSTUB_READY\n").expect("write marker");
        stdout.flush().expect("flush early stdout");

        let mut input = Vec::new();
        std::io::stdin()
            .read_to_end(&mut input)
            .expect("read prompt stdin");
        println!("stub saw {} bytes", input.len());
        std::process::exit(0);
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
        // A configured (valid) override is used verbatim, even if not yet in the tags.
        assert_eq!(
            resolve_gemma_model(Some("llama3:8b"), &["gemma4:e4b".to_string()]),
            "llama3:8b"
        );
        assert_eq!(
            resolve_gemma_model(Some("custom:tag"), &[]),
            "custom:tag",
            "configured wins even with an empty tag list"
        );
    }

    #[test]
    fn resolve_gemma_model_unconfigured_is_empty_opt_in() {
        // OPT-IN (owner rule): NO auto-default. With no configured model the resolver returns
        // "" regardless of which tags are present (the probe treats "" as the tier being OFF
        // — no `/api/tags` entry equals ""), so an unconfigured censor never runs. GEMMA_MODEL
        // is the RECOMMENDED suggestion shown in the UI, not an auto-default.
        assert_eq!(resolve_gemma_model(None, &[GEMMA_MODEL.to_string()]), "");
        assert_eq!(resolve_gemma_model(None, &["gemma4:e4b".to_string()]), "");
        assert_eq!(resolve_gemma_model(None, &[]), "");
        // Whitespace-only configured is treated as absent → empty (tier off).
        assert_eq!(
            resolve_gemma_model(Some("   "), &[GEMMA_MODEL.to_string()]),
            ""
        );
    }

    #[test]
    fn gemma_model_constant_is_the_recommended_nemotron() {
        // The RECOMMENDED censor model (a UI suggestion, NOT an auto-default).
        assert_eq!(
            GEMMA_MODEL, "hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M",
            "recommended censor model is Nemotron-3-Nano-4B (docs/censor-model-benchmark-2026-06.md)"
        );
    }

    // ---- run_gemma ----

    #[test]
    fn run_gemma_unavailable_returns_empty_without_calling_generate() {
        let c = StubClient::new(
            true,
            Ok("[{\"line\":1,\"title\":\"x\",\"severity\":\"low\"}]".into()),
        );
        let calls = c.generate_calls.clone();
        let out = run_gemma(
            &c,
            false,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &GemmaReviewParams::default(),
        );
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
            &GemmaReviewParams::default(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "gemma");
        assert_eq!(out[0].title, "Inverted guard");
        assert_eq!(out[0].line, Some(7));
    }

    #[test]
    fn run_gemma_generate_error_returns_empty_no_panic() {
        let c = StubClient::new(true, Err(GemmaError::Timeout));
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &GemmaReviewParams::default(),
        );
        assert!(out.is_empty());
    }

    #[test]
    fn run_gemma_garbage_response_returns_empty() {
        let c = StubClient::new(true, Ok("the model rambled with no json".into()));
        let out = run_gemma(
            &c,
            true,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &GemmaReviewParams::default(),
        );
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
            assert!(
                is_loopback_omlx_base(ok),
                "valid-port base {ok:?} must be accepted"
            );
        }
        for bad in [
            "http://[::1]:",          // empty ipv6 port
            "http://localhost:",      // empty port
            "http://localhost:99999", // > 65535
            "http://localhost:65536", // > 65535
            "http://localhost:abc",   // non-numeric
            "http://127.0.0.1:",      // empty port
            "http://[::1]:abc",       // ipv6 non-numeric
            "http://[::1]:65536",     // ipv6 out of range
        ] {
            assert!(
                !is_loopback_omlx_base(bad),
                "invalid-port base {bad:?} must be rejected"
            );
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
        let overlong = format!(
            "http://localhost:8000/{}",
            "a".repeat(OMLX_BASE_URL_MAX_LEN)
        );
        let c_long =
            OmlxClient::with_config(&overlong, "m", GEMMA_GENERATE_TIMEOUT, GEMMA_PROBE_TIMEOUT);
        assert_eq!(
            c_long.base, OMLX_DEFAULT_BASE,
            "an over-length oMLX base must be clamped to the default"
        );

        // A base carrying a control/bidi/invisible char is clamped too.
        let obfuscated = "http://localhost:8000/\u{202e}v1";
        let c_bidi =
            OmlxClient::with_config(obfuscated, "m", GEMMA_GENERATE_TIMEOUT, GEMMA_PROBE_TIMEOUT);
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
        assert!(!openai_model_present(
            &serde_json::json!({ "data": [] }),
            "m"
        ));
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
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v.provider, CensorAiProvider::Ollama);
        assert_eq!(v.effective_base(), OLLAMA_BASE);
        // OPT-IN: ollama config with no model → effective_model is "" (tier off).
        assert_eq!(v.effective_model(), "");
        // A loopback base + model for ollama is accepted and trimmed.
        let v2 = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: Some("  http://127.0.0.1:11434  ".into()),
            model: Some(" gemma4:e2b ".into()),
            ollama_model: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        })
        .is_err());
        // Missing base.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: None,
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
        // Empty (whitespace) base/model.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("   ".into()),
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        })
        .is_err());
    }

    // ---- Cloud provider (opt-in remote HTTPS egress): validator + branch-aware clamp ----

    #[test]
    fn validate_censor_local_ai_cloud_accepts_https_and_normalizes() {
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: Some("https://openrouter.ai/api/v1/".into()),
            model: Some("openai/gpt-4o-mini".into()),
            ollama_model: None,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v.provider, CensorAiProvider::Cloud);
        // Trailing slash stripped; remote https host preserved (the deliberate exception).
        assert_eq!(v.base_url.as_deref(), Some("https://openrouter.ai/api/v1"));
        assert_eq!(v.model.as_deref(), Some("openai/gpt-4o-mini"));
        assert_eq!(v.effective_base(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn validate_censor_local_ai_cloud_rejects_http() {
        // Cloud requires TLS — a plaintext http base would leak file content in clear.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: Some("http://openrouter.ai/api/v1".into()),
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_cloud_requires_base_and_model() {
        // Missing base.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: None,
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
        // Missing model.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: Some("https://openrouter.ai/api/v1".into()),
            model: None,
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
    }

    #[test]
    fn validate_censor_local_ai_cloud_rejects_ssrf_and_intranet_hosts() {
        // SSRF/privacy parity with the TS validateCloudBaseUrl + local_coder rule: a
        // hand-edited config that points the keyed cloud client at a metadata/loopback/IP/
        // single-label host must be REFUSED at the backend boundary, not just by the UI.
        for base in [
            "https://localhost:8000/v1",
            "https://127.0.0.1/v1",
            "https://169.254.169.254/latest/meta-data",
            "https://metadata.google.internal/v1",
            "https://api.internal/v1",
            "https://router.local/v1",
            "https://intranet/v1",           // single label, no dot
            "https://user@openrouter.ai/v1", // userinfo
            "https://[::1]/v1",              // IPv6 literal
            "http://openrouter.ai/v1",       // not https
        ] {
            assert!(
                validate_censor_local_ai(&CensorLocalAi {
                    provider: CensorAiProvider::Cloud,
                    base_url: Some(base.into()),
                    model: Some("m".into()),
                    ollama_model: None,
                    ..Default::default()
                })
                .is_err(),
                "cloud base {base:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_censor_local_ai_cloud_accepts_remote_host_with_port() {
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: Some("https://api.openai.com:443/v1".into()),
            model: Some("gpt-4o-mini".into()),
            ollama_model: None,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v.base_url.as_deref(), Some("https://api.openai.com:443/v1"));
    }

    #[test]
    fn omlx_with_config_and_key_cloud_keeps_remote_https_base() {
        // SECURITY-CRITICAL: with a key present (Cloud), a VALID https remote base must be
        // PRESERVED (not clamped to the loopback default) so the cloud review can reach it.
        let c = OmlxClient::with_config_and_key(
            "https://openrouter.ai/api/v1",
            "openai/gpt-4o-mini",
            Some("sk-secret"),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        // cache_identity folds in the base; it must show the remote host, prefixed "cloud".
        assert!(c
            .cache_identity()
            .starts_with("cloud|https://openrouter.ai/api/v1|"));
        assert_eq!(c.provider_label(), "cloud");
        // The key MUST NOT appear anywhere identity-bearing (it could be logged).
        assert!(!c.cache_identity().contains("sk-secret"));
    }

    #[test]
    fn omlx_with_config_no_key_still_clamps_non_loopback() {
        // PRIVACY: with NO key (local oMLX), a non-loopback base is STILL clamped to the
        // loopback default — the cloud branch must not weaken the local privacy guarantee.
        let c = OmlxClient::with_config_and_key(
            "https://openrouter.ai/api/v1",
            "m",
            None,
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert_eq!(c.cache_identity(), format!("omlx|{OMLX_DEFAULT_BASE}|m"));
        assert_eq!(c.provider_label(), "omlx");
    }

    #[test]
    fn omlx_with_config_and_key_invalid_cloud_base_clamps_to_default() {
        // Defense in depth: a misconfigured cloud base (http, not https) with a key falls
        // back to the unreachable loopback default — NEVER an unexpected remote host.
        let c = OmlxClient::with_config_and_key(
            "http://evil.example.com/v1",
            "m",
            Some("sk-secret"),
            GEMMA_GENERATE_TIMEOUT,
            GEMMA_PROBE_TIMEOUT,
        );
        assert!(c
            .cache_identity()
            .ends_with(&format!("|{OMLX_DEFAULT_BASE}|m")));
    }

    #[test]
    fn build_gemma_client_with_key_cloud_is_cloud_labeled() {
        let cfg = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Cloud,
            base_url: Some("https://openrouter.ai/api/v1".into()),
            model: Some("openai/gpt-4o-mini".into()),
            ollama_model: None,
            ..Default::default()
        })
        .unwrap();
        let client = build_gemma_client_with_key(&cfg, Some("sk-secret")).unwrap();
        assert_eq!(client.provider_label(), "cloud");
    }

    #[test]
    fn build_gemma_client_default_still_passes_no_key() {
        // The no-key wrapper keeps the existing provider identity for local providers.
        let client = build_gemma_client(&CensorLocalAi::default()).unwrap();
        assert_eq!(client.provider_label(), "ollama");
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
            ..Default::default()
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
            ..Default::default()
        })
        .is_err());
        // Userinfo trick refused.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://127.0.0.1@evil.com".into()),
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
        // Control / bidi chars refused.
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/\u{202e}v1".into()),
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
        })
        .is_err());
        // Overlong base refused.
        let long = format!(
            "http://localhost:8000/{}",
            "a".repeat(OMLX_BASE_URL_MAX_LEN)
        );
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some(long),
            model: Some("m".into()),
            ollama_model: None,
            ..Default::default()
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
                ..Default::default()
            });
            assert!(
                v.is_ok(),
                "valid oMLX model {model:?} must be accepted: {v:?}"
            );
            // The same token must satisfy the mini-coder validator (cross-check parity).
            assert!(
                crate::backend::mini_coder::is_valid_model(model),
                "mini_coder::is_valid_model must agree on {model:?}"
            );
        }

        let invalid = [
            "model name",        // whitespace
            "model;rm -rf",      // shell metachar
            "-leading-dash",     // first char not alnum
            ".dotfirst",         // first char not alnum
            "model\u{202e}evil", // bidi override
            "model\ttab",        // control
            "model@host",        // @ not allowed
            "model\\path",       // backslash not allowed
        ];
        for model in invalid {
            assert!(
                validate_censor_local_ai(&CensorLocalAi {
                    provider: CensorAiProvider::Omlx,
                    base_url: Some("http://localhost:8000/v1".into()),
                    model: Some(model.into()),
                    ollama_model: None,
                    ..Default::default()
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
            ..Default::default()
        })
        .is_ok());
        let over_cap = "a".repeat(CENSOR_OMLX_MODEL_MAX_LEN + 1);
        assert!(validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some(over_cap),
            ollama_model: None,
            ..Default::default()
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
            ..Default::default()
        };
        let j = serde_json::to_string(&omlx).unwrap();
        assert!(
            j.contains("\"baseUrl\":\"http://localhost:8000/v1\""),
            "{j}"
        );
        assert!(!j.contains("base_url"), "snake_case leaked: {j}");
        // NO-CHURN: ollama_model is None → never serialized.
        assert!(
            !j.contains("ollamaModel"),
            "absent ollamaModel must not serialize: {j}"
        );
        let back: CensorLocalAi = serde_json::from_str(&j).unwrap();
        assert_eq!(back, omlx);
        // Deserialize from a minimal camelCase object (provider only).
        let parsed: CensorLocalAi = serde_json::from_str(
            r#"{"provider":"omlx","baseUrl":"http://localhost:8000","model":"x"}"#,
        )
        .unwrap();
        assert_eq!(parsed.provider, CensorAiProvider::Omlx);
        assert_eq!(parsed.base_url.as_deref(), Some("http://localhost:8000"));

        // BACKWARD COMPAT: an OLD ollama config (no ollamaModel key) parses with None.
        let old: CensorLocalAi = serde_json::from_str(r#"{"provider":"ollama"}"#).unwrap();
        assert!(old.ollama_model.is_none());
        // Round-trip WITH a configured ollamaModel (camelCase over IPC).
        let with_model = CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("gemma4:e4b".into()),
            ..Default::default()
        };
        let jm = serde_json::to_string(&with_model).unwrap();
        assert!(jm.contains("\"ollamaModel\":\"gemma4:e4b\""), "{jm}");
        let back_m: CensorLocalAi = serde_json::from_str(&jm).unwrap();
        assert_eq!(back_m, with_model);

        // appleFm uses explicit camelCase discriminator and still omits absent fields.
        let apple = CensorLocalAi {
            provider: CensorAiProvider::AppleFm,
            base_url: None,
            model: Some("apple-default".into()),
            ollama_model: None,
            ..Default::default()
        };
        let aj = serde_json::to_string(&apple).unwrap();
        assert!(aj.contains("\"provider\":\"appleFm\""), "{aj}");
        assert!(!aj.contains("baseUrl"), "{aj}");
        let back_a: CensorLocalAi = serde_json::from_str(&aj).unwrap();
        assert_eq!(back_a, apple);
    }

    #[test]
    fn validate_censor_local_ai_applefm_keeps_optional_model_and_drops_unused() {
        let valid = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::AppleFm,
            base_url: Some("http://evil.example".into()),
            model: Some("  apple-model:v1  ".into()),
            ollama_model: Some("gemma4:e4b".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(valid.provider, CensorAiProvider::AppleFm);
        assert_eq!(valid.model.as_deref(), Some("apple-model:v1"));
        assert!(valid.base_url.is_none());
        assert!(valid.ollama_model.is_none());

        let bad = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::AppleFm,
            base_url: None,
            model: Some("bad model".into()),
            ollama_model: None,
            ..Default::default()
        });
        assert!(bad.is_err(), "appleFm model must use bare-token validation");
    }

    #[test]
    fn apple_fm_respond_args_use_fixed_command_and_never_prompt_text() {
        let args = apple_fm_respond_args(Some("apple-default"));
        assert_eq!(args, vec!["respond", "--model", "apple-default"]);
        let joined = args.join(" ");
        assert!(!joined.contains("TOP_SECRET_PROMPT"));

        let default_args = apple_fm_respond_args(None);
        assert_eq!(default_args, vec!["respond"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn build_gemma_client_applefm_non_macos_clean_error() {
        let cfg = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::AppleFm,
            base_url: None,
            model: None,
            ollama_model: None,
            ..Default::default()
        })
        .unwrap();
        let err = match build_gemma_client(&cfg) {
            Ok(_) => panic!("appleFm must not build on non-macOS"),
            Err(err) => err,
        };
        assert_eq!(err, "Apple on-device requires macOS 27+.");
    }

    #[test]
    fn validate_censor_local_ai_ollama_model_override() {
        // A valid bare ollamaModel tag is kept (and trimmed).
        let v = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("  gemma4:e4b  ".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v.ollama_model.as_deref(), Some("gemma4:e4b"));
        // Empty-after-trim → None (treated as absent).
        let v_empty = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Ollama,
            base_url: None,
            model: None,
            ollama_model: Some("   ".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(v_empty.ollama_model.is_none());
        // Invalid (space / control / overlong) → REJECTED.
        for bad in ["model name", "model;rm", "bad\u{202e}tag", "model\ttab"] {
            assert!(
                validate_censor_local_ai(&CensorLocalAi {
                    provider: CensorAiProvider::Ollama,
                    base_url: None,
                    model: None,
                    ollama_model: Some(bad.into()),
                    ..Default::default()
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
            ..Default::default()
        })
        .is_err());
        // oMLX config: a stray ollama_model is dropped (oMLX uses `model`).
        let omlx = validate_censor_local_ai(&CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://localhost:8000/v1".into()),
            model: Some("mlx-community/gemma".into()),
            ollama_model: Some("gemma4:e4b".into()),
            ..Default::default()
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
            None, // NO override -> the IO branch under test
            GEMMA_GENERATE_TIMEOUT,
            Duration::from_secs(30), // a fetch would block ~30s if it happened
        );

        let start = Instant::now();
        let resolved = client.resolved_model();
        let elapsed = start.elapsed();

        assert_eq!(
            resolved, "",
            "OPT-IN: no override + empty memo yields \"\" (tier off, no auto-default)"
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
            ..Default::default()
        })
        .unwrap();
        let client = build_gemma_client(&cfg).unwrap();
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
        // F1: the log must surface the model ACTUALLY in use. OPT-IN: the default Ollama
        // client has NO configured model, so its label is "" (tier off); the oMLX client
        // drives its configured model — `model_label` returns the right one for each.
        assert_eq!(OllamaClient::new().model_label(), "");
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
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(custom.model_label(), "llama3:8b");
        assert_eq!(custom.provider_label(), "ollama");
    }

    #[test]
    fn build_gemma_client_default_config_is_ollama() {
        // The default (a config with no `censorLocalAi`) resolves to the Ollama client —
        // byte-identical provider to the previous hardcoded OllamaClient::new().
        let client = build_gemma_client(&CensorLocalAi::default()).unwrap();
        assert_eq!(client.provider_label(), "ollama");
    }

    #[test]
    fn build_gemma_client_default_config_uses_ollama_base_and_model() {
        // OPT-IN: the default config has NO model (effective_model ""), so the tier is off
        // until the user picks one; the base is still the Ollama loopback default.
        let cfg = CensorLocalAi::default();
        assert_eq!(cfg.effective_base(), OLLAMA_BASE);
        assert_eq!(cfg.effective_model(), "");
        let client = build_gemma_client(&cfg).unwrap();
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
            ..Default::default()
        })
        .unwrap();
        let client = build_gemma_client(&cfg).unwrap();
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
                ..Default::default()
            })
            .unwrap(),
        ] {
            let probe_client = build_gemma_client(&cfg).unwrap();
            let worker_client = build_gemma_client(&cfg).unwrap();
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
            ..Default::default()
        })
        .unwrap();
        let client = build_gemma_client(&cfg).unwrap();
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
        let out = run_gemma(
            &stub,
            available,
            Path::new("/root"),
            "src/a.ts",
            "code",
            &[],
            &GemmaReviewParams::default(),
        );
        assert!(out.is_empty(), "unavailable tier yields no findings");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "generate must never be called when the provider probe is false"
        );
    }

    /// P9 LIVE verify (network-bound, manual):
    ///   cargo test --lib -- --ignored --nocapture omlx_gemma_tier_live
    /// Proves the tier-2 CLIENT path (build_gemma_client -> probe -> a real
    /// generate) against loopback oMLX Gemma. Scope honesty: the watcher
    /// additionally builds file context from the real project root (here /tmp
    /// has no src/div.ts, so the prompt goes contextless), and run_gemma
    /// degrades parse/transport errors to empty findings — so "0 findings"
    /// proves the call RAN, not that the verdict pipeline is correct. SKIPS
    /// (with a message) when no server answers, so offline runs never break.
    #[test]
    #[ignore]
    fn omlx_gemma_tier_live_probe_and_generate() {
        let cfg = CensorLocalAi {
            provider: CensorAiProvider::Omlx,
            base_url: Some("http://127.0.0.1:8000/v1".into()),
            model: Some("gemma-4-12B-it-qat-4bit".into()),
            ollama_model: None,
            ..Default::default()
        };
        let client = build_gemma_client(&cfg).expect("omlx client builds");
        let available = probe_available(client.as_ref());
        if !available {
            eprintln!("omlx_gemma_tier_live: no oMLX server on 127.0.0.1:8000 — SKIPPED");
            return;
        }
        let code =
            "export function div(a: number, b: number) {\n  return a / b; // no zero check\n}\n";
        let findings = run_gemma(
            client.as_ref(),
            available,
            Path::new("/tmp"),
            "src/div.ts",
            code,
            &[],
            &GemmaReviewParams::default(),
        );
        // Zero findings is a VALID verdict; the assertion is that the live tier
        // ran (availability true) — transport/parse failures inside run_gemma
        // degrade to empty, so print the outcome for the human.
        eprintln!(
            "omlx_gemma_tier_live: {} finding(s): {:?}",
            findings.len(),
            findings
        );
    }

    // ---- server_load trait method ----

    #[test]
    fn server_load_default_returns_none() {
        let client = StubClient::new(true, Ok("[]".to_string()));
        assert_eq!(client.server_load(), None, "default impl returns None");
    }

    #[test]
    fn server_load_stub_can_override() {
        struct LoadStub {
            load: Option<(usize, u64)>,
        }
        impl GemmaClient for LoadStub {
            fn probe(&self) -> bool {
                true
            }
            fn generate(&self, _: &str) -> Result<String, GemmaError> {
                Ok("[]".into())
            }
            fn provider_label(&self) -> &'static str {
                "load-stub"
            }
            fn model_label(&self) -> String {
                "load-stub".into()
            }
            fn server_load(&self) -> Option<(usize, u64)> {
                self.load
            }
        }
        let busy = LoadStub {
            load: Some((1, 8 * 1024 * 1024 * 1024)),
        };
        assert_eq!(busy.server_load(), Some((1, 8 * 1024 * 1024 * 1024)));
        let free = LoadStub { load: Some((0, 0)) };
        assert_eq!(free.server_load(), Some((0, 0)));
        let unknown = LoadStub { load: None };
        assert_eq!(unknown.server_load(), None);
    }
}
