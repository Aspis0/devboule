use serde::Serialize;
use std::collections::HashMap;

use super::catalog::{recommended_censor_models, RecommendTier};

/// View-model entry for one selectable local Censor model.
///
/// The Censor's local AI tier is **opt-in**: nothing here is active until the
/// user explicitly picks a model. Each option carries enough state for the UI
/// to render the picker — whether the model is installed locally and whether
/// the multi-turn DEEP review mode (which requires a tool-capable runtime) is
/// available for it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelOption {
    /// Backend model tag (e.g. an Ollama tag like `"qwen3:8b"`).
    pub tag: &'static str,
    /// Human-readable label shown in the picker.
    pub label: &'static str,
    /// Recommendation tier from the curated catalog.
    pub tier: RecommendTier,
    /// `true` iff this model is currently installed in the local runtime.
    pub installed: bool,
    /// `true` iff DEEP (multi-turn, tool-using) review is available: the model
    /// must be both installed AND advertise the `"tools"` capability.
    pub deep_eligible: bool,
}

/// Build the opt-in Censor model picker view-model.
///
/// Iterates the curated [`recommended_censor_models`] in catalog order and, for
/// each one, resolves runtime state:
///
/// * `installed` — `true` iff the model's `tag` appears in `installed_tags`.
/// * `deep_eligible` — `true` iff the model is installed **and** its runtime
///   capabilities (`caps_by_tag`, e.g. the capability list returned by Ollama's
///   `/api/show`) contain the string `"tools"`. A tool-capable model that is
///   not installed is therefore **not** deep-eligible.
///
/// # Arguments
///
/// * `installed_tags` — tags currently present in the local runtime.
/// * `caps_by_tag` — per-tag runtime capability strings, keyed by model tag.
pub fn censor_model_options(
    installed_tags: &[String],
    caps_by_tag: &HashMap<String, Vec<String>>,
) -> Vec<ModelOption> {
    recommended_censor_models()
        .iter()
        .map(|model| {
            let installed = installed_tags.iter().any(|t| t == model.tag);
            let deep_eligible = installed
                && caps_by_tag
                    .get(model.tag)
                    .is_some_and(|caps| caps.iter().any(|c| c == "tools"));

            ModelOption {
                tag: model.tag,
                label: model.label,
                tier: model.tier,
                installed,
                deep_eligible,
            }
        })
        .collect()
}
