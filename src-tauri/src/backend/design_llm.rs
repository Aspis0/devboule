//! Design-LLM backend config (Phase 2 STEP 1) — the single global LLM provider the
//! generative-design module generates node markup with.
//!
//! This is a 1:1 MIRROR of the mini-coder backend (`backend::mini_coder::MiniCoderBackend`)
//! for the shared kinds (`ollama`/`api`/`codex`/`omlx`/`cloud`), the SAME per-field shape
//! (camelCase `kind`/`model?`/`command?`/`baseUrl?`), and the SAME per-kind validation
//! rules. To guarantee the two NEVER drift, the validator here does NOT re-implement any
//! primitive: it reuses the mini-coder's `pub(crate)` helpers
//! ([`is_valid_model`](super::mini_coder::is_valid_model),
//! [`is_forbidden_command_char`](super::mini_coder::is_forbidden_command_char),
//! [`validate_omlx_base_url`](super::mini_coder::validate_omlx_base_url)) and the shared
//! cloud URL validator
//! ([`validate_cloud_base_url`](super::local_coder::validate_cloud_base_url)). The ONLY
//! thing that differs is the user-facing error wording ("design" instead of "mini-coder");
//! the accept/reject SET is byte-for-byte identical for the shared kinds.
//!
//! Persisted in config.json under `designLlmBackend`; absent means no design provider is
//! configured (later generation steps then fail cleanly). NOTHING here streams or
//! generates — that is a later Phase-2 step. This module owns only the config TYPE +
//! validation; the `get_/set_design_llm_backend` commands live in `projects.rs` next to
//! `set_mini_coder_backend`, cloning that atomic read-modify-write idiom.

use super::mini_coder::{
    is_forbidden_command_char, is_valid_model, validate_omlx_base_url, MINI_COMMAND_MAX_LEN,
    MINI_MODEL_MAX_LEN,
};
use serde::{Deserialize, Serialize};

/// The kind of runtime the design LLM runs on. A 1:1 mirror of
/// [`super::mini_coder::MiniCoderBackendKind`] for the shared kinds, plus design-only
/// `claude`/`openai`; snake/lower over the wire to match the TS `DesignLlmBackendKind`
/// and the config.json discriminator exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesignLlmBackendKind {
    /// Local `ollama run <model>` — text-only. `model` REQUIRED.
    Ollama,
    /// A user-provided cheap-API CLI — `command` REQUIRED (run verbatim, prompt over
    /// stdin). The API key MUST come from the CLI's own env, never argv.
    Api,
    /// The user's codex subscription via `codex exec` (one-shot, rides local auth — NOT
    /// an API key). `model` OPTIONAL.
    Codex,
    /// OpenAI via the hosted OpenAI API (rides a local API key from env — NOT local auth).
    /// `model` OPTIONAL.
    Openai,
    /// The user's Claude Code subscription via `claude -p --output-format text` (one-shot,
    /// print/non-interactive mode, rides the user's local auth — NOT an API key). `model`
    /// OPTIONAL (like codex). NOTE: this kind is NOT part of the mini-coder backend set, so
    /// the design validator handles it as a dedicated arm rather than delegating to the
    /// mini-coder primitives.
    Claude,
    /// A local oMLX (MLX) server exposing an OpenAI-compatible HTTP API. `model` AND
    /// `base_url` REQUIRED; `command` is unused. The base URL is constrained to a LOOPBACK
    /// http origin (http only; privacy: the prompt never leaves the device).
    Omlx,
    /// An HTTPS OpenAI-compatible cloud endpoint (OpenRouter). `model` AND `base_url`
    /// REQUIRED; `command` is unused. The base URL must be a PUBLIC https host (validated
    /// by [`super::local_coder::validate_cloud_base_url`]). The API key lives in the vault
    /// (`provider:cloud_llm` / per-role), never on this config struct — it is read at
    /// stream time and sent as `Authorization: Bearer`.
    Cloud,
}

/// The single, global design-LLM backend config persisted in config.json under
/// `designLlmBackend`. A discriminated struct mirroring [`super::mini_coder::MiniCoderBackend`]
/// field-for-field: `kind` picks the runtime and the relevant field is required per kind
/// (validated by [`validate_design_llm_backend`]).
///
/// camelCase + every optional field `#[serde(default)]`/`skip_serializing_if` so a
/// config.json written by the UI (only the fields its kind uses) round-trips without churn
/// and an older/hand-edited config still parses leniently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignLlmBackend {
    pub kind: DesignLlmBackendKind,
    /// Model tag/name. Required for `ollama`/`omlx`/`cloud`, optional for `codex`, unused for `api`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The CLI command line. Required for `api`; unused for `ollama`/`codex`/`omlx`/`cloud`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// HTTP base URL. For `omlx`: a LOOPBACK http origin (e.g. `http://localhost:8000/v1`),
    /// validated via [`super::mini_coder::validate_omlx_base_url`]. For `cloud`: a PUBLIC
    /// https host (e.g. `https://openrouter.ai/api/v1`), validated via
    /// [`super::local_coder::validate_cloud_base_url`]. Required for those kinds; unused
    /// otherwise. Always STORED NORMALIZED (no trailing slash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Reasoning-effort knob applied to a generation. Accepts `"low"`/`"medium"`/`"high"`
    /// (validated + lowercased by [`validate_design_llm_backend`]); any other value is
    /// REJECTED. Owned by the composer's model popover, NOT the Settings card. Only the
    /// `codex` CLI path actually consumes it today (`-c model_reasoning_effort=<value>`);
    /// the other kinds ignore it. Absent => the provider default. NOT a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-run wall-clock budget (seconds) for a single generation. Bounded to
    /// `[60, 600]` by [`validate_design_llm_backend`] (out-of-range is REJECTED, mirroring
    /// the validator's reject-not-normalize posture for every other field). Absent => the
    /// built-in 180s default. Consumed by `design_generate` (HTTP + CLI wall-clock cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Minimum per-run generation timeout (seconds). Below this a generation is too short to
/// be useful (even a small local model needs warm-up); a smaller value is REJECTED.
pub(crate) const DESIGN_TIMEOUT_SECS_MIN: u64 = 60;

/// Maximum per-run generation timeout (seconds). Bounds a single generation so a hung/
/// trickling provider cannot occupy the channel for an unbounded time; a larger value is
/// REJECTED. 600s (10 min) is generous for a full-page markup generation.
pub(crate) const DESIGN_TIMEOUT_SECS_MAX: u64 = 600;

/// Normalize + validate the OPTIONAL `effort` knob. Trims + lowercases, then accepts ONLY
/// `low`/`medium`/`high`; anything else is rejected with a clear message (mirroring the
/// reject-not-normalize posture of every other field here). `None`/empty-after-trim => the
/// field is simply absent (no effort override). Pure + total: unit-testable.
fn validate_design_effort(effort: Option<&str>) -> Result<Option<String>, String> {
    let raw = effort.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return Ok(None);
    }
    let lowered = raw.to_ascii_lowercase();
    match lowered.as_str() {
        "low" | "medium" | "high" => Ok(Some(lowered)),
        _ => Err("Design effort must be one of: low, medium, high.".into()),
    }
}

/// Validate the OPTIONAL per-run `timeoutSecs`. `None` => absent (use the default). A
/// present value must be in `[DESIGN_TIMEOUT_SECS_MIN, DESIGN_TIMEOUT_SECS_MAX]`; an
/// out-of-range value is REJECTED (mirroring the validator's reject-not-normalize posture).
/// Pure + total: unit-testable.
fn validate_design_timeout_secs(timeout_secs: Option<u64>) -> Result<Option<u64>, String> {
    match timeout_secs {
        None => Ok(None),
        Some(secs) => {
            if !(DESIGN_TIMEOUT_SECS_MIN..=DESIGN_TIMEOUT_SECS_MAX).contains(&secs) {
                return Err(format!(
                    "Design timeout must be between {DESIGN_TIMEOUT_SECS_MIN} and {DESIGN_TIMEOUT_SECS_MAX} seconds."
                ));
            }
            Ok(Some(secs))
        }
    }
}

/// Validate + normalize a design-LLM backend config. Applies EXACTLY the same per-kind
/// rules as [`super::mini_coder::validate_mini_coder_backend`] by reusing its `pub(crate)`
/// primitives (no logic duplication). Trims fields and keeps ONLY the field(s) the kind
/// uses (so a kind switch never leaves a stale model/command). Returns the normalized
/// backend or a human error string.
///
/// TRUST MODEL: identical to the mini-coder's — the `api` command is an OPERATOR-CONFIGURED,
/// TRUSTED shell command line, run with the user's own privileges, fed the prompt over
/// stdin (never on argv). We deliberately do NOT block shell metacharacters; we DO reject
/// control/bidi/invisible chars (via the shared `is_forbidden_command_char`) since the
/// command is embedded verbatim into a launch line. Reviewers should not re-flag the lack
/// of metachar filtering as an injection bug — see `mini_coder::validate_mini_coder_backend`.
pub fn validate_design_llm_backend(backend: &DesignLlmBackend) -> Result<DesignLlmBackend, String> {
    let model = backend
        .model
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let command = backend
        .command
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    // The effort/timeout knobs are kind-INDEPENDENT (every arm keeps them), so validate +
    // normalize them ONCE up front. An invalid value is rejected here regardless of kind.
    let effort = validate_design_effort(backend.effort.as_deref())?;
    let timeout_secs = validate_design_timeout_secs(backend.timeout_secs)?;

    match backend.kind {
        DesignLlmBackendKind::Ollama => {
            if model.is_empty() {
                return Err("Ollama design backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err("Design model must be a bare tag (letters, digits, . _ : / -).".into());
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some(model),
                command: None,
                base_url: None,
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Api => {
            if command.is_empty() {
                return Err("API design backend requires a command line.".into());
            }
            // WARNING 4: measure in BYTES (`len()`), matching the model cap (`model.len()`)
            // and the TS `.length` mirror — `chars().count()` would let a multibyte-heavy
            // command exceed the intended byte budget. Not a vuln, just consistency.
            if command.len() > MINI_COMMAND_MAX_LEN {
                return Err(format!(
                    "Design command must be at most {MINI_COMMAND_MAX_LEN} characters."
                ));
            }
            // The command is embedded VERBATIM into a launch line; reject the SAME
            // control/bidi/invisible blocklist as the mini-coder (shared helper).
            if command.chars().any(is_forbidden_command_char) {
                return Err(
                    "Design command must not contain control, bidi or invisible characters.".into(),
                );
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Api,
                model: None,
                command: Some(command),
                base_url: None,
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Codex => {
            // model is OPTIONAL for codex; validate only if provided.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Design model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Codex,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Openai => {
            // model is OPTIONAL for openai; validate only if provided.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Design model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Openai,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Claude => {
            // model is OPTIONAL for claude (same rule as codex); validate only if provided.
            if !model.is_empty() {
                if model.len() > MINI_MODEL_MAX_LEN {
                    return Err(format!(
                        "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                    ));
                }
                if !is_valid_model(&model) {
                    return Err(
                        "Design model must be a bare tag (letters, digits, . _ : / -).".into(),
                    );
                }
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Claude,
                model: if model.is_empty() { None } else { Some(model) },
                command: None,
                base_url: None,
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Omlx => {
            // omlx requires BOTH a model (a bare tag, same rule as ollama) and a loopback
            // http (only) base URL. `command` is ignored/dropped.
            if model.is_empty() {
                return Err("oMLX design backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err("Design model must be a bare tag (letters, digits, . _ : / -).".into());
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("oMLX design backend requires a base URL.".into());
            }
            // Reuse the shared loopback/http-only/port/char validator+normalizer so the
            // design and mini-coder oMLX surfaces accept/reject EXACTLY the same set. The
            // length cap is enforced INSIDE this validator (against MINI_BASE_URL_MAX_LEN),
            // so we do not re-check it here — single source of truth, like the mini-coder.
            let normalized_base = validate_omlx_base_url(&base_url)?;
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Omlx,
                model: Some(model),
                command: None,
                base_url: Some(normalized_base),
                effort,
                timeout_secs,
            })
        }
        DesignLlmBackendKind::Cloud => {
            // Cloud requires BOTH a model (bare tag, same rule as ollama/omlx) and a
            // PUBLIC https base URL. `command` is ignored/dropped. API key is NOT a field
            // here — it lives only in the vault and is read at stream time.
            if model.is_empty() {
                return Err("Cloud design backend requires a model tag.".into());
            }
            if model.len() > MINI_MODEL_MAX_LEN {
                return Err(format!(
                    "Design model must be at most {MINI_MODEL_MAX_LEN} characters."
                ));
            }
            if !is_valid_model(&model) {
                return Err("Design model must be a bare tag (letters, digits, . _ : / -).".into());
            }
            let base_url = backend
                .base_url
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() {
                return Err("Cloud design backend requires a base URL.".into());
            }
            // REUSE — not duplicate — the coder-side cloud URL validator (https public host,
            // no loopback/IP/userinfo). Accept/reject set must match the other cloud surfaces.
            let normalized_base = super::local_coder::validate_cloud_base_url(&base_url)?;
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Cloud,
                model: Some(model),
                command: None,
                base_url: Some(normalized_base),
                effort,
                timeout_secs,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests assert against the base-URL cap directly; the validator delegates the
    // length check to `validate_omlx_base_url`, so this const is test-scoped here.
    use super::super::mini_coder::MINI_BASE_URL_MAX_LEN;

    // -- serde --------------------------------------------------------------

    #[test]
    fn backend_kind_serializes_lowercase_matching_ts() {
        for (kind, tok) in [
            (DesignLlmBackendKind::Ollama, "ollama"),
            (DesignLlmBackendKind::Api, "api"),
            (DesignLlmBackendKind::Codex, "codex"),
            (DesignLlmBackendKind::Openai, "openai"),
            (DesignLlmBackendKind::Claude, "claude"),
            (DesignLlmBackendKind::Omlx, "omlx"),
            (DesignLlmBackendKind::Cloud, "cloud"),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{tok}\""));
            let back: DesignLlmBackendKind = serde_json::from_str(&format!("\"{tok}\"")).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn backend_round_trips_camel_case_and_skips_unused_fields() {
        let ollama = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let json = serde_json::to_string(&ollama).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "json: {json}");
        assert!(json.contains("\"model\":\"qwen2.5-coder\""), "json: {json}");
        assert!(!json.contains("command"), "unused command leaked: {json}");
        assert!(!json.contains("baseUrl"), "unused baseUrl leaked: {json}");
        let back: DesignLlmBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(ollama, back);

        // codex with no model: the model key is absent (no churn).
        let codex_bare = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let cj = serde_json::to_string(&codex_bare).unwrap();
        assert_eq!(cj, r#"{"kind":"codex"}"#);
    }

    #[test]
    fn omlx_round_trips_camel_case_baseurl_and_drops_command() {
        let omlx = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: Some("http://localhost:8000/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        let json = serde_json::to_string(&omlx).unwrap();
        assert!(json.contains("\"kind\":\"omlx\""), "json: {json}");
        assert!(
            json.contains("\"baseUrl\":\"http://localhost:8000/v1\""),
            "json: {json}"
        );
        assert!(!json.contains("command"), "unused command leaked: {json}");
        let back: DesignLlmBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(omlx, back);
    }

    #[test]
    fn partial_json_lenient_parse() {
        // The leanest config the UI could emit for codex.
        let json = r#"{ "kind": "codex" }"#;
        let b: DesignLlmBackend = serde_json::from_str(json).unwrap();
        assert_eq!(b.kind, DesignLlmBackendKind::Codex);
        assert_eq!(b.model, None);
        assert_eq!(b.command, None);
        assert_eq!(b.base_url, None);
    }

    #[test]
    fn old_config_without_effort_or_timeout_parses_unchanged() {
        // A config.json written BEFORE the effort/timeoutSecs fields existed must still
        // deserialize cleanly (serde(default)), with both new fields absent.
        let json = r#"{ "kind": "ollama", "model": "qwen2.5-coder" }"#;
        let b: DesignLlmBackend = serde_json::from_str(json).unwrap();
        assert_eq!(b.kind, DesignLlmBackendKind::Ollama);
        assert_eq!(b.model.as_deref(), Some("qwen2.5-coder"));
        assert_eq!(b.effort, None);
        assert_eq!(b.timeout_secs, None);
        // And it re-serializes WITHOUT the new keys (no churn for an untouched config).
        let out = serde_json::to_string(&b).unwrap();
        assert!(!out.contains("effort"), "effort leaked: {out}");
        assert!(!out.contains("timeoutSecs"), "timeoutSecs leaked: {out}");
    }

    #[test]
    fn backend_round_trips_effort_and_timeout_camel_case() {
        let codex = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            effort: Some("high".into()),
            timeout_secs: Some(300),
        };
        let json = serde_json::to_string(&codex).unwrap();
        assert!(json.contains("\"effort\":\"high\""), "json: {json}");
        assert!(json.contains("\"timeoutSecs\":300"), "json: {json}");
        let back: DesignLlmBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(codex, back);
    }

    // -- validation ---------------------------------------------------------

    #[test]
    fn validate_ollama_requires_model_and_keeps_only_model() {
        let bad = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: None,
            command: Some("ignored".into()),
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&bad).is_err());

        let ok = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: Some("  qwen2.5-coder  ".into()),
            command: Some("dropped".into()),
            base_url: Some("http://localhost:1/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder")); // trimmed
        assert_eq!(n.command, None); // command dropped for ollama
        assert_eq!(n.base_url, None); // base_url dropped for ollama
    }

    #[test]
    fn validate_api_requires_command_rejects_control_chars() {
        let no_cmd = DesignLlmBackend {
            kind: DesignLlmBackendKind::Api,
            model: None,
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&no_cmd).is_err());

        for bad in [
            "mycli chat\nrm -rf /",  // newline
            "mycli chat\u{7f}--x",   // DEL (0x7f)
            "mycli chat\u{202e}--x", // RIGHT-TO-LEFT OVERRIDE (bidi)
        ] {
            let ctrl = DesignLlmBackend {
                kind: DesignLlmBackendKind::Api,
                model: None,
                command: Some(bad.into()),
                base_url: None,
                effort: None,
                timeout_secs: None,
            };
            assert!(
                validate_design_llm_backend(&ctrl).is_err(),
                "control/invisible char in command {bad:?} must be rejected"
            );
        }

        let ok = DesignLlmBackend {
            kind: DesignLlmBackendKind::Api,
            model: Some("dropped".into()),
            command: Some("  mycli chat --json  ".into()),
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&ok).unwrap();
        assert_eq!(n.command.as_deref(), Some("mycli chat --json"));
        assert_eq!(n.model, None); // model dropped for api
    }

    #[test]
    fn validate_codex_ok_bare_and_with_optional_model() {
        let bare = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&bare).unwrap();
        assert_eq!(n.model, None);

        let with_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: Some("  gpt-5-codex  ".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("gpt-5-codex"));

        let bad_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: Some("bad model!".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&bad_model).is_err());
    }

    #[test]
    fn validate_claude_ok_bare_and_with_optional_model() {
        // claude mirrors codex: bare is valid (no model), optional model validated as a tag.
        let bare = DesignLlmBackend {
            kind: DesignLlmBackendKind::Claude,
            model: None,
            command: Some("dropped".into()),
            base_url: Some("http://localhost:1/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&bare).unwrap();
        assert_eq!(n.kind, DesignLlmBackendKind::Claude);
        assert_eq!(n.model, None);
        assert_eq!(n.command, None); // dropped
        assert_eq!(n.base_url, None); // dropped

        let with_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Claude,
            model: Some("  claude-sonnet-4-5  ".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("claude-sonnet-4-5"));

        let bad_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Claude,
            model: Some("bad model!".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&bad_model).is_err());
    }

    #[test]
    fn claude_round_trips_camel_case_bare() {
        let bare = DesignLlmBackend {
            kind: DesignLlmBackendKind::Claude,
            model: None,
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(json, r#"{"kind":"claude"}"#);
        let back: DesignLlmBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(bare, back);
    }

    #[test]
    fn validate_rejects_model_with_whitespace_or_metachars() {
        for bad in [
            "has space",
            "with;semicolon",
            "pipe|here",
            "$(sub)",
            "-leadingdash",
        ] {
            // -leadingdash actually starts with '-', which is NOT alnum → rejected.
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some(bad.into()),
                command: None,
                base_url: None,
                effort: None,
                timeout_secs: None,
            };
            assert!(
                validate_design_llm_backend(&b).is_err(),
                "model {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_overlong_model_and_command() {
        let long_model = "a".repeat(MINI_MODEL_MAX_LEN + 1);
        let b = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: Some(long_model),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&b).is_err());

        let long_cmd = "a".repeat(MINI_COMMAND_MAX_LEN + 1);
        let b = DesignLlmBackend {
            kind: DesignLlmBackendKind::Api,
            model: None,
            command: Some(long_cmd),
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&b).is_err());
    }

    #[test]
    fn omlx_requires_model_and_base_url() {
        let no_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: None,
            command: None,
            base_url: Some("http://localhost:8000/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&no_model).is_err());

        let no_base = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&no_base).is_err());
    }

    #[test]
    fn omlx_accepts_loopback_http_and_drops_command_trims_model() {
        let ok = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("  qwen2.5-coder  ".into()),
            command: Some("dropped".into()),
            base_url: Some("  http://127.0.0.1:8000/v1  ".into()),
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder"));
        assert_eq!(n.command, None); // command dropped for omlx
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
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Omlx,
                model: Some("qwen2.5-coder".into()),
                command: None,
                base_url: Some(bad.into()),
                effort: None,
                timeout_secs: None,
            };
            assert!(
                validate_design_llm_backend(&b).is_err(),
                "oMLX base URL {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn omlx_validates_optional_port_and_normalizes_trailing_slash() {
        // Bad port (out of range) rejected.
        let bad_port = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some("http://localhost:99999/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&bad_port).is_err());

        // Trailing slash normalized off.
        let trailing = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some("http://localhost:8000/v1/".into()),
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&trailing).unwrap();
        assert_eq!(n.base_url.as_deref(), Some("http://localhost:8000/v1"));
    }

    #[test]
    fn omlx_rejects_control_chars_and_overlong_base_url() {
        let ctrl = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some("http://localhost:8000/v1\u{202e}".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&ctrl).is_err());

        let long = format!(
            "http://localhost:8000/{}",
            "a".repeat(MINI_BASE_URL_MAX_LEN)
        );
        let overlong = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some(long),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&overlong).is_err());
    }

    // -- Cloud (OpenRouter / public https) ----------------------------------

    #[test]
    fn cloud_accepts_valid_model_and_https_base_and_drops_command() {
        let ok = DesignLlmBackend {
            kind: DesignLlmBackendKind::Cloud,
            model: Some("  openrouter/auto  ".into()),
            command: Some("dropped".into()),
            base_url: Some("  https://openrouter.ai/api/v1/  ".into()),
            effort: None,
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&ok).unwrap();
        assert_eq!(n.kind, DesignLlmBackendKind::Cloud);
        assert_eq!(n.model.as_deref(), Some("openrouter/auto"));
        assert_eq!(n.command, None); // command dropped for cloud
        assert_eq!(
            n.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1") // trailing slash normalized
        );
    }

    #[test]
    fn cloud_requires_model() {
        let no_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Cloud,
            model: None,
            command: None,
            base_url: Some("https://openrouter.ai/api/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&no_model).is_err());

        let empty_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Cloud,
            model: Some("   ".into()),
            command: None,
            base_url: Some("https://openrouter.ai/api/v1".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&empty_model).is_err());
    }

    #[test]
    fn cloud_requires_base_url() {
        let no_base = DesignLlmBackend {
            kind: DesignLlmBackendKind::Cloud,
            model: Some("openrouter/auto".into()),
            command: None,
            base_url: None,
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&no_base).is_err());

        let empty_base = DesignLlmBackend {
            kind: DesignLlmBackendKind::Cloud,
            model: Some("openrouter/auto".into()),
            command: None,
            base_url: Some("   ".into()),
            effort: None,
            timeout_secs: None,
        };
        assert!(validate_design_llm_backend(&empty_base).is_err());
    }

    #[test]
    fn cloud_rejects_loopback_and_http_base() {
        // validate_cloud_base_url rejects loopback + cleartext http (TLS required).
        for bad in [
            "http://openrouter.ai/api/v1",   // http, not https
            "http://localhost:8000/v1",      // loopback + http
            "https://localhost:8000/v1",     // loopback even with https
            "https://127.0.0.1:8000/v1",     // IP literal
            "https://openrouter.ai@evil.com/v1", // userinfo trick
        ] {
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Cloud,
                model: Some("openrouter/auto".into()),
                command: None,
                base_url: Some(bad.into()),
                effort: None,
                timeout_secs: None,
            };
            assert!(
                validate_design_llm_backend(&b).is_err(),
                "Cloud base URL {bad:?} must be rejected"
            );
        }
    }

    // -- effort + timeout (A2) ----------------------------------------------

    #[test]
    fn validate_normalizes_effort_to_lowercase_and_keeps_it() {
        // Mixed-case + surrounding whitespace normalizes to a bare lowercase token, and the
        // effort/timeout survive validation on any kind.
        let b = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            effort: Some("  HIGH ".into()),
            timeout_secs: Some(120),
        };
        let n = validate_design_llm_backend(&b).unwrap();
        assert_eq!(n.effort.as_deref(), Some("high"));
        assert_eq!(n.timeout_secs, Some(120));

        // An empty/whitespace effort normalizes to absent (no override), not an error.
        let blank = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: None,
            command: None,
            base_url: None,
            effort: Some("   ".into()),
            timeout_secs: None,
        };
        let n = validate_design_llm_backend(&blank).unwrap();
        assert_eq!(n.effort, None);
    }

    #[test]
    fn validate_rejects_unknown_effort() {
        for bad in ["ultra", "none", "highest", "0", "low high"] {
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Codex,
                model: None,
                command: None,
                base_url: None,
                effort: Some(bad.into()),
                timeout_secs: None,
            };
            assert!(
                validate_design_llm_backend(&b).is_err(),
                "effort {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_accepts_in_range_timeout_and_rejects_out_of_range() {
        for ok in [DESIGN_TIMEOUT_SECS_MIN, 180, DESIGN_TIMEOUT_SECS_MAX] {
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some("m".into()),
                command: None,
                base_url: None,
                effort: None,
                timeout_secs: Some(ok),
            };
            assert!(
                validate_design_llm_backend(&b).is_ok(),
                "timeout {ok} must be accepted"
            );
        }
        for bad in [
            DESIGN_TIMEOUT_SECS_MIN - 1,
            0,
            DESIGN_TIMEOUT_SECS_MAX + 1,
            9999,
        ] {
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some("m".into()),
                command: None,
                base_url: None,
                effort: None,
                timeout_secs: Some(bad),
            };
            assert!(
                validate_design_llm_backend(&b).is_err(),
                "timeout {bad} must be rejected"
            );
        }
    }
}
