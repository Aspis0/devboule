// GOLD fail-to-pass tests for the `censor-model-options` prodbench sample (a real Devboule
// feature: the opt-in Censor model picker's view-model). Authored as independent ground truth;
// the harness strips the candidate's own tests and appends this module. RED at base (module
// absent); GREEN only when a candidate implements the spec.
#[cfg(test)]
mod gold_tests {
    use super::*;
    use crate::backend::censor::gemma::GEMMA_MODEL;
    use std::collections::HashMap;

    fn caps(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn all_recommended_models_become_options_in_order() {
        let opts = censor_model_options(&[], &HashMap::new());
        assert_eq!(opts.len(), 4);
        assert_eq!(opts[0].tag, GEMMA_MODEL);
        // none installed => none deep-eligible
        assert!(opts.iter().all(|o| !o.installed && !o.deep_eligible));
    }

    #[test]
    fn installed_flag_tracks_installed_tags() {
        let installed = vec!["gemma3:12b".to_string()];
        let opts = censor_model_options(&installed, &HashMap::new());
        let g = opts.iter().find(|o| o.tag == "gemma3:12b").expect("gemma option");
        assert!(g.installed);
        assert!(opts.iter().filter(|o| o.tag != "gemma3:12b").all(|o| !o.installed));
    }

    #[test]
    fn deep_eligible_requires_installed_and_tools_capability() {
        let installed = vec![GEMMA_MODEL.to_string(), "gemma3:12b".to_string()];
        let c = caps(&[
            (GEMMA_MODEL, &["tools", "completion"]),
            ("gemma3:12b", &["completion"]),
        ]);
        let opts = censor_model_options(&installed, &c);
        let nemo = opts.iter().find(|o| o.tag == GEMMA_MODEL).expect("nemotron option");
        let gemma = opts.iter().find(|o| o.tag == "gemma3:12b").expect("gemma option");
        assert!(nemo.installed && nemo.deep_eligible, "installed + tools => deep eligible");
        assert!(gemma.installed && !gemma.deep_eligible, "installed but no tools => not deep");
    }

    #[test]
    fn tool_capable_but_not_installed_is_not_deep_eligible() {
        let c = caps(&[("granite4:tiny-h", &["tools"])]);
        let opts = censor_model_options(&[], &c);
        let gr = opts.iter().find(|o| o.tag == "granite4:tiny-h").expect("granite option");
        assert!(!gr.installed && !gr.deep_eligible, "not installed => never deep, even with tools");
    }
}
