//! Design-LLM backend config (Phase 2 STEP 1) — the single global LLM provider the
//! generative-design module generates node markup with.
//!
//! This is a 1:1 MIRROR of the mini-coder backend (`backend::mini_coder::MiniCoderBackend`):
//! the SAME four provider kinds (`ollama`/`api`/`codex`/`omlx`), the SAME per-field shape
//! (camelCase `kind`/`model?`/`command?`/`baseUrl?`), and the SAME per-kind validation
//! rules. To guarantee the two NEVER drift, the validator here does NOT re-implement any
//! primitive: it reuses the mini-coder's `pub(crate)` helpers
//! ([`is_valid_model`](super::mini_coder::is_valid_model),
//! [`is_forbidden_command_char`](super::mini_coder::is_forbidden_command_char),
//! [`validate_omlx_base_url`](super::mini_coder::validate_omlx_base_url)) and the shared
//! length caps. The ONLY thing that differs is the user-facing error wording ("design"
//! instead of "mini-coder"); the accept/reject SET is byte-for-byte identical.
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
/// [`super::mini_coder::MiniCoderBackendKind`]; snake/lower over the wire to match the TS
/// `DesignLlmBackendKind` and the config.json discriminator exactly.
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
    /// Model tag/name. Required for `ollama`/`omlx`, optional for `codex`, unused for `api`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The CLI command line. Required for `api`; unused for `ollama`/`codex`/`omlx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The oMLX server base URL (e.g. `http://localhost:8000/v1`). Required for `omlx`;
    /// unused for the other kinds. Validated to a LOOPBACK http origin (http only) and
    /// STORED NORMALIZED (no trailing slash) via the shared
    /// [`super::mini_coder::validate_omlx_base_url`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
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
pub fn validate_design_llm_backend(
    backend: &DesignLlmBackend,
) -> Result<DesignLlmBackend, String> {
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
                return Err(
                    "Design model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some(model),
                command: None,
                base_url: None,
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
                    "Design command must not contain control, bidi or invisible characters."
                        .into(),
                );
            }
            Ok(DesignLlmBackend {
                kind: DesignLlmBackendKind::Api,
                model: None,
                command: Some(command),
                base_url: None,
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
                return Err(
                    "Design model must be a bare tag (letters, digits, . _ : / -).".into(),
                );
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
            (DesignLlmBackendKind::Claude, "claude"),
            (DesignLlmBackendKind::Omlx, "omlx"),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{tok}\""));
            let back: DesignLlmBackendKind =
                serde_json::from_str(&format!("\"{tok}\"")).unwrap();
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

    // -- validation ---------------------------------------------------------

    #[test]
    fn validate_ollama_requires_model_and_keeps_only_model() {
        let bad = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: None,
            command: Some("ignored".into()),
            base_url: None,
        };
        assert!(validate_design_llm_backend(&bad).is_err());

        let ok = DesignLlmBackend {
            kind: DesignLlmBackendKind::Ollama,
            model: Some("  qwen2.5-coder  ".into()),
            command: Some("dropped".into()),
            base_url: Some("http://localhost:1/v1".into()),
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
        };
        assert!(validate_design_llm_backend(&no_cmd).is_err());

        for bad in [
            "mycli chat\nrm -rf /", // newline
            "mycli chat\u{7f}--x",  // DEL (0x7f)
            "mycli chat\u{202e}--x", // RIGHT-TO-LEFT OVERRIDE (bidi)
        ] {
            let ctrl = DesignLlmBackend {
                kind: DesignLlmBackendKind::Api,
                model: None,
                command: Some(bad.into()),
                base_url: None,
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
        };
        let n = validate_design_llm_backend(&bare).unwrap();
        assert_eq!(n.model, None);

        let with_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: Some("  gpt-5-codex  ".into()),
            command: None,
            base_url: None,
        };
        let n = validate_design_llm_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("gpt-5-codex"));

        let bad_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Codex,
            model: Some("bad model!".into()),
            command: None,
            base_url: None,
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
        };
        let n = validate_design_llm_backend(&with_model).unwrap();
        assert_eq!(n.model.as_deref(), Some("claude-sonnet-4-5"));

        let bad_model = DesignLlmBackend {
            kind: DesignLlmBackendKind::Claude,
            model: Some("bad model!".into()),
            command: None,
            base_url: None,
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
        };
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(json, r#"{"kind":"claude"}"#);
        let back: DesignLlmBackend = serde_json::from_str(&json).unwrap();
        assert_eq!(bare, back);
    }

    #[test]
    fn validate_rejects_model_with_whitespace_or_metachars() {
        for bad in ["has space", "with;semicolon", "pipe|here", "$(sub)", "-leadingdash"]
        {
            // -leadingdash actually starts with '-', which is NOT alnum → rejected.
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Ollama,
                model: Some(bad.into()),
                command: None,
                base_url: None,
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
        };
        assert!(validate_design_llm_backend(&b).is_err());

        let long_cmd = "a".repeat(MINI_COMMAND_MAX_LEN + 1);
        let b = DesignLlmBackend {
            kind: DesignLlmBackendKind::Api,
            model: None,
            command: Some(long_cmd),
            base_url: None,
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
        };
        assert!(validate_design_llm_backend(&no_model).is_err());

        let no_base = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("qwen2.5-coder".into()),
            command: None,
            base_url: None,
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
        };
        let n = validate_design_llm_backend(&ok).unwrap();
        assert_eq!(n.model.as_deref(), Some("qwen2.5-coder"));
        assert_eq!(n.command, None); // command dropped for omlx
        assert_eq!(n.base_url.as_deref(), Some("http://127.0.0.1:8000/v1"));
    }

    #[test]
    fn omlx_rejects_https_and_non_loopback_and_userinfo_tricks() {
        for bad in [
            "https://localhost:8000/v1",      // https rejected
            "http://evil.com:8000/v1",        // non-loopback host
            "http://127.0.0.1.evil.com/v1",   // suffix trick
            "http://127.0.0.1@evil.com/v1",   // userinfo trick
            "http://[::1]:8000@evil.com/v1",  // ipv6 userinfo trick
            "ftp://localhost/v1",             // wrong scheme
        ] {
            let b = DesignLlmBackend {
                kind: DesignLlmBackendKind::Omlx,
                model: Some("qwen2.5-coder".into()),
                command: None,
                base_url: Some(bad.into()),
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
        };
        assert!(validate_design_llm_backend(&bad_port).is_err());

        // Trailing slash normalized off.
        let trailing = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some("http://localhost:8000/v1/".into()),
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
        };
        assert!(validate_design_llm_backend(&ctrl).is_err());

        let long = format!("http://localhost:8000/{}", "a".repeat(MINI_BASE_URL_MAX_LEN));
        let overlong = DesignLlmBackend {
            kind: DesignLlmBackendKind::Omlx,
            model: Some("m".into()),
            command: None,
            base_url: Some(long),
        };
        assert!(validate_design_llm_backend(&overlong).is_err());
    }
}
