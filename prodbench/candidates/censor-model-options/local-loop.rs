use serde::Serialize;
use std::collections::HashMap;

use super::catalog::{recommended_censor_models, RecommendTier};

/// Represents a single selectable option for the local Censor model.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelOption {
    /// The unique identifier tag for the model.
    pub tag: &'static str,
    /// Human-readable label for the model.
    pub label: &'static str,
    /// The recommendation tier of the model.
    pub tier: RecommendTier,
    /// Whether the model is currently installed on the local system.
    pub installed: bool,
    /// Whether the model is eligible for multi-turn DEEP review mode.
    /// This requires the model to be installed AND support tool capabilities.
    pub deep_eligible: bool,
}

/// Generates the list of available Censor model options based on the system state.
///
/// # Arguments
///
/// * `installed_tags` - A slice of tags for models currently installed on the system.
/// * `caps_by_tag` - A map of model tags to their runtime capabilities (e.g., from Ollama /api/show).
///
/// # Returns
///
/// A vector of `ModelOption` structs representing the available models in the recommended order.
pub fn censor_model_options(
    installed_tags: &[String],
    caps_by_tag: &HashMap<String, Vec<String>>,
) -> Vec<ModelOption> {
    recommended_censor_models()
        .iter()
        .map(|model| {
            let tag = model.tag;
            let installed = installed_tags.iter().any(|t| t == tag);
            let deep_eligible = if installed {
                caps_by_tag
                    .get(tag)
                    .map(|caps| caps.iter().any(|c| c == "tools"))
                    .unwrap_or(false)
            } else {
                false
            };

            ModelOption {
                tag,
                label: model.label,
                tier: model.tier,
                installed,
                deep_eligible,
            }
        })
        .collect()
}
