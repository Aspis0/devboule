//! Local Censor model guidance (opt-in).
//!
//! This module provides curated, opt-in guidance about which local models
//! work well as the Censor's reviewer backend, plus small helpers to probe
//! an Ollama model's advertised capabilities via its `/api/show` response.
//!
//! Nothing here is mandatory: the recommendations are hints surfaced to the
//! user, and the capability probing is best-effort (any malformed input is
//! treated as "no capabilities" rather than an error).

use serde::Serialize;

/// How strongly a given model is recommended for use as the local Censor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecommendTier {
    /// The preferred default for the Censor.
    Recommended,
    /// Usable, but with caveats (size, latency, or weaker tool support).
    LessRecommended,
}

/// A single curated model recommendation for the local Censor.
#[derive(Debug, Clone, Serialize)]
pub struct RecommendedModel {
    /// The model tag as understood by the local runner (e.g. Ollama).
    pub tag: &'static str,
    /// Human-friendly display label.
    pub label: &'static str,
    /// How strongly this model is recommended.
    pub tier: RecommendTier,
    /// Short human note explaining the trade-offs.
    pub note: &'static str,
    /// Best-effort hint that the model is expected to support tool calling.
    ///
    /// This is only a hint; the authoritative signal comes from probing the
    /// model at runtime via [`model_tool_capable`].
    pub tool_capable_hint: bool,
}

/// Curated, opt-in list of models suggested for the local Censor.
///
/// The first entry is the recommended default; the remaining entries are
/// usable alternatives with caveats. The list is intentionally small and
/// ordered by preference.
pub fn recommended_censor_models() -> &'static [RecommendedModel] {
    const MODELS: &[RecommendedModel] = &[
        RecommendedModel {
            tag: super::gemma::GEMMA_MODEL,
            label: "NVIDIA Nemotron-3-Nano-4B (Q4_K_M)",
            tier: RecommendTier::Recommended,
            note: "Default Censor: small, fast, and tool-capable for local review.",
            tool_capable_hint: true,
        },
        RecommendedModel {
            tag: "gemma3:12b",
            label: "Gemma 3 12B",
            tier: RecommendTier::LessRecommended,
            note: "Stronger reasoning but heavier and slower; no native tool calling.",
            tool_capable_hint: false,
        },
        RecommendedModel {
            tag: "granite4:tiny-h",
            label: "Granite 4 Tiny (hybrid)",
            tier: RecommendTier::LessRecommended,
            note: "Lightweight and tool-capable, but less accurate on code review.",
            tool_capable_hint: true,
        },
        RecommendedModel {
            tag: "deepseek-r1:8b",
            label: "DeepSeek-R1 8B",
            tier: RecommendTier::LessRecommended,
            note: "Good reasoning, but verbose thinking and no native tool calling.",
            tool_capable_hint: false,
        },
    ];
    MODELS
}

/// Parse an Ollama `/api/show` JSON body and return its `capabilities` array.
///
/// Returns an empty vector when the body is not valid JSON, when the
/// `capabilities` field is missing, or when it is not an array. Non-string
/// elements within the array are silently dropped. This function never panics.
pub fn parse_show_capabilities(show_body: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(show_body) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };

    match value.get("capabilities").and_then(|caps| caps.as_array()) {
        Some(array) => array
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        None => Vec::new(),
    }
}

/// Return `true` iff the model's advertised capabilities include `"tools"`.
///
/// Built on top of [`parse_show_capabilities`], so any malformed input is
/// treated as "not tool-capable".
pub fn model_tool_capable(show_body: &str) -> bool {
    parse_show_capabilities(show_body)
        .iter()
        .any(|capability| capability == "tools")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommends_exactly_four_models_in_order() {
        let models = recommended_censor_models();
        assert_eq!(models.len(), 4);

        assert_eq!(models[0].tier, RecommendTier::Recommended);
        assert_eq!(models[0].label, "NVIDIA Nemotron-3-Nano-4B (Q4_K_M)");
        assert!(models[0].tool_capable_hint);

        assert_eq!(models[1].tag, "gemma3:12b");
        assert_eq!(models[1].tier, RecommendTier::LessRecommended);
        assert!(!models[1].tool_capable_hint);

        assert_eq!(models[2].tag, "granite4:tiny-h");
        assert_eq!(models[2].tier, RecommendTier::LessRecommended);
        assert!(models[2].tool_capable_hint);

        assert_eq!(models[3].tag, "deepseek-r1:8b");
        assert_eq!(models[3].tier, RecommendTier::LessRecommended);
        assert!(!models[3].tool_capable_hint);
    }

    #[test]
    fn excluded_models_never_appear() {
        for model in recommended_censor_models() {
            let tag = model.tag.to_ascii_lowercase();
            let label = model.label.to_ascii_lowercase();
            for banned in ["mimo", "phi", "glm"] {
                assert!(!tag.contains(banned), "banned tag {banned}");
                assert!(!label.contains(banned), "banned label {banned}");
            }
        }
    }

    #[test]
    fn tier_serializes_kebab_case() {
        let json = serde_json::to_string(&RecommendTier::LessRecommended).expect("serialize tier");
        assert_eq!(json, "\"less-recommended\"");
    }

    #[test]
    fn parses_capabilities_array() {
        let body = r#"{"capabilities": ["completion", "tools", "vision"]}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["completion", "tools", "vision"]);
        assert!(model_tool_capable(body));
    }

    #[test]
    fn drops_non_string_elements() {
        let body = r#"{"capabilities": ["tools", 7, null, {"x": 1}, "vision"]}"#;
        let caps = parse_show_capabilities(body);
        assert_eq!(caps, vec!["tools", "vision"]);
    }

    #[test]
    fn malformed_or_missing_yields_empty() {
        assert!(parse_show_capabilities("not json").is_empty());
        assert!(parse_show_capabilities("{}").is_empty());
        assert!(parse_show_capabilities(r#"{"capabilities": "tools"}"#).is_empty());
        assert!(parse_show_capabilities(r#"{"capabilities": 42}"#).is_empty());
        assert!(!model_tool_capable("not json"));
        assert!(!model_tool_capable("{}"));
    }
}
