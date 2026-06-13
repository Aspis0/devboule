// GOLD fail-to-pass tests for the `censor-catalog` prodbench sample.
//
// Authored as the benchmark GROUND TRUTH — independent of any candidate's own tests (the
// harness strips the candidate's #[cfg(test)] module and appends THIS one), so a pipeline
// cannot pass by writing self-serving tests. Asserts the public contract from the task
// prompt. RED at base (the module/functions do not exist → will not compile); GREEN only
// when a candidate implements the spec.
#[cfg(test)]
mod gold_tests {
    use super::*;

    #[test]
    fn exactly_one_recommended_and_it_is_the_gemma_const() {
        let models = recommended_censor_models();
        let rec: Vec<_> = models
            .iter()
            .filter(|m| m.tier == RecommendTier::Recommended)
            .collect();
        assert_eq!(rec.len(), 1, "exactly one Recommended entry");
        assert_eq!(rec[0].tag, crate::backend::censor::gemma::GEMMA_MODEL);
    }

    #[test]
    fn less_recommended_are_the_three_expected_tags() {
        let less: std::collections::BTreeSet<&str> = recommended_censor_models()
            .iter()
            .filter(|m| m.tier == RecommendTier::LessRecommended)
            .map(|m| m.tag)
            .collect();
        let expected: std::collections::BTreeSet<&str> =
            ["gemma3:12b", "granite4:tiny-h", "deepseek-r1:8b"]
                .into_iter()
                .collect();
        assert_eq!(less, expected);
    }

    #[test]
    fn unusable_families_absent() {
        for m in recommended_censor_models() {
            let name = m.tag.rsplit('/').next().unwrap_or(m.tag);
            let fam = name.split(':').next().unwrap_or(name).to_lowercase();
            for bad in ["mimo", "phi", "glm"] {
                assert!(!fam.starts_with(bad), "unusable family {bad:?} present: {}", m.tag);
            }
        }
    }

    #[test]
    fn parse_capabilities_extracts_tools_array() {
        let caps = parse_show_capabilities(r#"{"capabilities":["tools","thinking","completion"]}"#);
        assert_eq!(caps, vec!["tools", "thinking", "completion"]);
    }

    #[test]
    fn parse_capabilities_robust_on_garbage_and_mixed() {
        assert!(parse_show_capabilities("not json").is_empty());
        assert!(parse_show_capabilities("{}").is_empty());
        // non-string elements dropped, no panic
        assert_eq!(
            parse_show_capabilities(r#"{"capabilities":["tools",1,null,"thinking"]}"#),
            vec!["tools", "thinking"]
        );
    }

    #[test]
    fn tool_capable_true_only_when_tools_present() {
        assert!(model_tool_capable(r#"{"capabilities":["tools"]}"#));
        assert!(!model_tool_capable(r#"{"capabilities":["completion"]}"#));
        assert!(!model_tool_capable("{}"));
    }
}
