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
            label: "NVIDIA Nemotron-3-Nano-4B",
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
    capabilities.iter().any(|c| c == "tools")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_show_capabilities_returns_tools() {
        let body = r#"{"capabilities":["tools","thinking","completion"],"template":"..."}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["tools", "thinking", "completion"]);
        assert!(model_tool_capable(body));
    }

    #[test]
    fn test_parse_show_capabilities_no_tools() {
        let body = r#"{"capabilities":["completion"]}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["completion"]);
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn test_parse_show_capabilities_malformed_json() {
        let body = "not json";
        let caps = parse_show_capabilities(body);
        assert!(caps.is_empty());
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn test_parse_show_capabilities_missing_field() {
        let body = "{}";
        let caps = parse_show_capabilities(body);
        assert!(caps.is_empty());
        assert!(!model_tool_capable(body));
    }

    #[test]
    fn test_catalog_has_exactly_one_recommended() {
        let models = recommended_censor_models();
        let recommended_count = models
            .iter()
            .filter(|m| m.tier == RecommendTier::Recommended)
            .count();
        assert_eq!(recommended_count, 1);
    }

    #[test]
    fn test_recommended_model_is_gemma() {
        let models = recommended_censor_models();
        let recommended = models
            .iter()
            .find(|m| m.tier == RecommendTier::Recommended)
            .expect("Should have exactly one recommended model");
        assert_eq!(recommended.tag, super::gemma::GEMMA_MODEL);
    }

    #[test]
    fn test_catalog_excludes_unusable_models() {
        let models = recommended_censor_models();
        let tags: Vec<&str> = models.iter().map(|m| m.tag).collect();
        let combined_tags = tags.join(" ");
        let lower_tags = combined_tags.to_lowercase();

        assert!(
            !lower_tags.contains("mimo"),
            "Catalog should not contain MiMo models"
        );
        assert!(
            !lower_tags.contains("phi"),
            "Catalog should not contain Phi models"
        );
        assert!(
            !lower_tags.contains("glm"),
            "Catalog should not contain GLM models"
        );
    }
}