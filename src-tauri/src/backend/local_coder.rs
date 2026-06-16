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
//!   - `ollama`: a local Ollama server. `model` REQUIRED; the launch points the binary at
//!     Ollama's loopback OpenAI-compatible endpoint (`http://localhost:11434/v1` by default).
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

use super::mini_coder::{is_valid_model, validate_omlx_base_url, MINI_MODEL_MAX_LEN};
use serde::{Deserialize, Serialize};

/// The kind of runtime the LOCAL main coder (orchestrator) runs on. snake/lower over the
/// wire to match the TS `LocalCoderBackendKind` and the config.json discriminator exactly.
///
/// Intentionally a SUBSET of [`super::mini_coder::MiniCoderBackendKind`] — only the two
/// LOCAL HTTP runtimes the orchestrator binary can actually drive (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalCoderBackendKind {
    /// A local Ollama server. `model` REQUIRED. The launch resolves Ollama's loopback
    /// OpenAI-compatible endpoint for the binary's `OmlxModel` client.
    Ollama,
    /// A local oMLX (MLX) server exposing an OpenAI-compatible HTTP API. `model` AND
    /// `base_url` REQUIRED. The base URL is constrained to a LOOPBACK http origin (http
    /// only; privacy: the prompt never leaves the device).
    Omlx,
}

/// The Ollama loopback OpenAI-compatible base URL the orchestrator binary's `OmlxModel`
/// client is pointed at when `kind == ollama`. Ollama serves an OpenAI-compatible API on
/// its standard loopback port, so the orchestrator can drive it with the SAME HTTP client
/// it uses for oMLX. Kept here (single source of truth) so the launch assembly never
/// hardcodes the URL inline.
pub const OLLAMA_OPENAI_BASE_URL: &str = "http://localhost:11434/v1";

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
    /// Model tag/name. REQUIRED for both `ollama` and `omlx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The oMLX server base URL (e.g. `http://localhost:8000/v1`). Required for `omlx`;
    /// unused for `ollama` (which resolves Ollama's fixed loopback OpenAI endpoint).
    /// Validated to a LOOPBACK http origin (http only) and STORED NORMALIZED (no trailing
    /// slash) via the shared [`super::mini_coder::validate_omlx_base_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
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
            Ok(LocalCoderBackend {
                kind: LocalCoderBackendKind::Ollama,
                model: Some(model),
                // base_url is dropped for ollama (the launch resolves the fixed Ollama
                // loopback OpenAI endpoint), so a kind switch never leaves a stale URL.
                base_url: None,
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
            })
        }
    }
}

/// Resolve the loopback OpenAI-compatible base URL + model the orchestrator binary's env
/// (`DEVBOULE_OMLX_BASE_URL` / `DEVBOULE_OMLX_MODEL`) should carry for a validated backend.
///
/// - `ollama` => Ollama's fixed loopback OpenAI endpoint + the configured model tag.
/// - `omlx`   => the configured (validated, normalized) loopback base URL + model.
///
/// `model`/`base_url` are guaranteed `Some` for a value that went through
/// [`validate_local_coder_backend`]; the `unwrap_or_default` guards a hand-built struct so
/// this never panics (an empty string then yields the binary's safe Mock-model fallback).
pub fn resolve_omlx_env(backend: &LocalCoderBackend) -> (String, String) {
    let model = backend.model.clone().unwrap_or_default();
    let base_url = match backend.kind {
        LocalCoderBackendKind::Ollama => OLLAMA_OPENAI_BASE_URL.to_string(),
        LocalCoderBackendKind::Omlx => backend.base_url.clone().unwrap_or_default(),
    };
    (base_url, model)
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
            (LocalCoderBackendKind::Ollama, "ollama"),
            (LocalCoderBackendKind::Omlx, "omlx"),
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
        };
        let json = serde_json::to_string(&ollama).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""), "json: {json}");
        assert!(json.contains("\"model\":\"qwen2.5-coder\""), "json: {json}");
        assert!(!json.contains("baseUrl"), "unused baseUrl leaked: {json}");
        let back: LocalCoderBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(ollama, back);
    }

    #[test]
    fn omlx_round_trips_camel_case_baseurl() {
        let omlx = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            base_url: Some("http://localhost:8000/v1".into()),
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
    fn validate_ollama_requires_model_and_drops_base_url() {
        let no_model = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: None,
            base_url: None,
        };
        assert!(validate_local_coder_backend(&no_model).is_err());

        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("  qwen2.5-coder  ".into()),
            base_url: Some("http://localhost:1/v1".into()),
        };
        let n = validate_local_coder_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder")); // trimmed
        assert_eq!(n.base_url, None); // base_url dropped for ollama
    }

    #[test]
    fn validate_rejects_model_with_whitespace_or_metachars() {
        for bad in ["has space", "with;semicolon", "pipe|here", "$(sub)", "-leadingdash"] {
            let b = LocalCoderBackend {
                kind: LocalCoderBackendKind::Ollama,
                model: Some(bad.into()),
                base_url: None,
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
        };
        assert!(validate_local_coder_backend(&b).is_err());
    }

    #[test]
    fn omlx_requires_model_and_base_url() {
        let no_model = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: None,
            base_url: Some("http://localhost:8000/v1".into()),
        };
        assert!(validate_local_coder_backend(&no_model).is_err());

        let no_base = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            base_url: None,
        };
        assert!(validate_local_coder_backend(&no_base).is_err());
    }

    #[test]
    fn omlx_accepts_loopback_http_and_trims_model() {
        let ok = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("  qwen2.5-coder  ".into()),
            base_url: Some("  http://127.0.0.1:8000/v1  ".into()),
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
        };
        assert!(validate_local_coder_backend(&bad_port).is_err());

        let trailing = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("m".into()),
            base_url: Some("http://localhost:8000/v1/".into()),
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
        };
        assert!(validate_local_coder_backend(&overlong).is_err());
    }

    // -- resolve_omlx_env ---------------------------------------------------

    #[test]
    fn resolve_env_ollama_points_at_loopback_openai_endpoint() {
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Ollama,
            model: Some("qwen2.5-coder".into()),
            base_url: None,
        };
        let (base, model) = resolve_omlx_env(&b);
        assert_eq!(base, OLLAMA_OPENAI_BASE_URL);
        assert_eq!(model, "qwen2.5-coder");
    }

    #[test]
    fn resolve_env_omlx_uses_configured_base_and_model() {
        let b = LocalCoderBackend {
            kind: LocalCoderBackendKind::Omlx,
            model: Some("mlx-qwen".into()),
            base_url: Some("http://localhost:8000/v1".into()),
        };
        let (base, model) = resolve_omlx_env(&b);
        assert_eq!(base, "http://localhost:8000/v1");
        assert_eq!(model, "mlx-qwen");
    }
}
