//! # Censor Model Catalog
//!
//! This module provides the curated list of Ollama models recommended for the Censor's
//! local-AI tier. The Censor is **OPT-IN**: no models run until the user explicitly
//! selects one via Settings.
//!
//! ## Tool-Calling Derivation
//!
//! Whether a model supports the multi-turn "DEEP" review mode (which requires tool-calling)
//! is **NOT** hardcoded in this catalog. Instead, the catalog provides a `tool_capable_hint`
//! for pre-install guidance. The authoritative signal is derived at runtime by probing
//! Ollama's `/api/show` endpoint and inspecting the `capabilities` array.
//!
//! See `model_tool_capable` for the runtime gate logic.

use serde::Serialize;

/// The recommendation tier for a specific model in the Censor catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecommendTier {
    /// The single primary pick for most use cases.
    Recommended,
    /// Works but less precise / less reliable.
    LessRecommended,
}

/// A curated model entry in the Censor catalog.
#[derive(Debug, Clone, Serialize)]
pub struct RecommendedModel {
    /// The Ollama pull tag the user configures.
    pub tag: &'static str,
    /// Short human label for UI display.
    pub label: &'static str,
    /// Recommendation tier.
    pub tier: RecommendTier,
    /// One-line guidance on strengths/weaknesses.
    pub note: &'static str,
    /// Bench-observed tool-calling support.
    ///
    /// Note: This is a pre-install hint. The runtime `/api/show` probe is authoritative.
    pub tool_capable_hint: bool,
}

/// Returns the static list of recommended Censor models.
///
/// Models we found unusable (MiMo, Phi-4-reasoning, GLM-4.x-Flash: verbose/looping/no tool-calling)
/// are deliberately ABSENT from this list, not listed as less recommended.
pub fn recommended_censor_models() -> &'static [RecommendedModel] {
    const CENSOR_MODELS: [RecommendedModel; 4] = [
        RecommendedModel {
            tag: super::gemma::GEMMA_MODEL,
            label: "NVIDIA Nemotron-3-Nano-4B (Q4_K_M)",
            tier: RecommendTier::Recommended,
            note: "Small + agentic: tool-calls reliably and finds in-file bugs concisely. Our pick.",
            tool_capable_hint: true,
        },
        RecommendedModel {
            tag: "gemma3:12b",
            label: "Gemma 3 12B",
            tier: RecommendTier::LessRecommended,
            note: "Works but high-recall/low-precision; over-escalates and does not tool-call.",
            tool_capable_hint: false,
        },
        RecommendedModel {
            tag: "granite4:tiny-h",
            label: "IBM Granite-4.0 H-Tiny",
            tier: RecommendTier::LessRecommended,
            note: "Usable instruct model with tool support; less precise on semantic bugs.",
            tool_capable_hint: true,
        },
        RecommendedModel {
            tag: "deepseek-r1:8b",
            label: "DeepSeek-R1-Distill 8B",
            tier: RecommendTier::LessRecommended,
            note: "Reasons well but verbose and does not tool-call; single-shot FAST mode only.",
            tool_capable_hint: false,
        },
    ];

    &CENSOR_MODELS
}

/// Parses an Ollama `/api/show` JSON response and extracts the `capabilities` array.
///
/// Returns an empty vector if the JSON is malformed, the field is missing, or the field
/// is not an array of strings.
pub fn parse_show_capabilities(show_body: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(show_body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let capabilities = match value.get("capabilities") {
        Some(capabilities) => capabilities,
        None => return Vec::new(),
    };

    let array = match capabilities.as_array() {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    array
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect()
}

/// Determines if a model supports tool-calling based on its `/api/show` capabilities.
///
/// This is the gate for the multi-turn DEEP (tool-calling) Censor mode.
/// A model without the "tools" capability can still run in FAST single-shot mode.
pub fn model_tool_capable(show_body: &str) -> bool {
    let capabilities = parse_show_capabilities(show_body);
    // Case-insensitive: Ollama reports lowercase today, but an Ollama-compatible third-party
    // server emitting "Tools"/"TOOLS" must not silently disable DEEP mode (false negative).
    capabilities
        .iter()
        .any(|c| c.eq_ignore_ascii_case("tools"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_capabilities_returns_tools() {
        let body = r#"{"capabilities":["tools","thinking","completion"],"template":"..."}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["tools", "thinking", "completion"]);
        assert!(model_tool_capable(body));
    }

    #[test]
    fn parse_show_capabilities_no_tools() {
        let body = r#"{"capabilities":["completion"]}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["completion"]);
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn parse_show_capabilities_malformed_json() {
        let body = "not json";
        let caps = parse_show_capabilities(body);
        assert!(caps.is_empty());
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn parse_show_capabilities_missing_field() {
        let body = "{}";
        let caps = parse_show_capabilities(body);
        assert!(caps.is_empty());
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn parse_show_capabilities_drops_non_string_elements() {
        // Schema-robustness: a malformed/evolved `capabilities` array mixing strings with
        // numbers/null must not panic — non-strings are dropped, the "tools" string is kept.
        let body = r#"{"capabilities":["tools",42,null,{"x":1},"thinking"]}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["tools", "thinking"]);
        assert!(model_tool_capable(body));
    }

    #[test]
    fn model_tool_capable_is_case_insensitive() {
        // An Ollama-compatible server emitting non-lowercase capabilities must still enable
        // DEEP mode (guards against a silent false-negative).
        assert!(model_tool_capable(r#"{"capabilities":["Tools"]}"#));
        assert!(model_tool_capable(r#"{"capabilities":["TOOLS"]}"#));
    }

    #[test]
    fn catalog_has_exactly_one_recommended() {
        let models = recommended_censor_models();
        let recommended_count = models
            .iter()
            .filter(|m| m.tier == RecommendTier::Recommended)
            .count();
        assert_eq!(recommended_count, 1);
    }

    #[test]
    fn recommended_model_is_gemma() {
        let models = recommended_censor_models();
        let recommended = models
            .iter()
            .find(|m| m.tier == RecommendTier::Recommended)
            .expect("Should have exactly one recommended model");
        assert_eq!(recommended.tag, crate::backend::censor::gemma::GEMMA_MODEL);
    }

    #[test]
    fn catalog_lists_less_recommended_and_excludes_unusable() {
        let models = recommended_censor_models();

        // POSITIVE invariant: the three LessRecommended alternatives are present exactly.
        // (Deleting one fails here — unlike a vacuous substring scan that passes regardless.)
        let less: std::collections::BTreeSet<&str> = models
            .iter()
            .filter(|m| m.tier == RecommendTier::LessRecommended)
            .map(|m| m.tag)
            .collect();
        let expected: std::collections::BTreeSet<&str> =
            ["gemma3:12b", "granite4:tiny-h", "deepseek-r1:8b"]
                .into_iter()
                .collect();
        assert_eq!(less, expected);

        // NEGATIVE invariant: the unusable families (MiMo, Phi, GLM) never appear. Match the
        // model-name component (after any `org/` prefix, before the `:tag`), case-insensitively,
        // so an unrelated tag that merely CONTAINS "phi"/"glm" as a substring is not a false
        // positive (e.g. "amphion" must not trip the "phi" guard).
        for m in models {
            let name = m.tag.rsplit('/').next().unwrap_or(m.tag);
            let family = name.split(':').next().unwrap_or(name).to_lowercase();
            for bad in ["mimo", "phi", "glm"] {
                assert!(
                    !family.starts_with(bad),
                    "unusable family {bad:?} must not be in the catalog (tag {:?})",
                    m.tag
                );
            }
        }
    }
}