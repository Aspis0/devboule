// GOLD fail-to-pass tests for `model-tag-parser` — calibrated to PRODUCTION reality (no
// planted/artificial traps, and nothing "made derivable" in the prompt): a normal ticket asks
// for a robust tag parser, and these tests just check it actually WORKS on the real, messy
// inputs a robust parser must handle. Every assertion is a defensible production requirement,
// not an arbitrary interpretation. (The earlier `-GGUF` strip was dropped — even a strong
// reviewer judged the unstripped name correct, so it was ambiguous, not derivable.) Independent
// ground truth; the harness strips the candidate's tests and appends this. RED at base.
#[cfg(test)]
mod gold_tests {
    use super::*;

    fn p(s: &str) -> ModelTag {
        parse_model_tag(s)
    }

    #[test]
    fn canonical_full_tag_splits_registry_org_name_quant() {
        // The natural parse — the file-format suffix in the name is NOT stripped (ambiguous).
        let t = p("hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M");
        assert_eq!(t.registry.as_deref(), Some("hf.co"));
        assert_eq!(t.org.as_deref(), Some("nvidia"));
        assert_eq!(t.name, "NVIDIA-Nemotron-3-Nano-4B-GGUF");
        assert_eq!(t.quant.as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn bare_name_with_suffix_has_no_registry_or_org() {
        let t = p("gemma3:12b");
        assert_eq!(t.registry, None);
        assert_eq!(t.org, None);
        assert_eq!(t.name, "gemma3");
        assert_eq!(t.quant.as_deref(), Some("12b"));
    }

    #[test]
    fn no_colon_means_no_quant() {
        let t = p("granite4");
        assert_eq!(t.name, "granite4");
        assert_eq!(t.quant, None);
    }

    #[test]
    fn trailing_colon_yields_none_not_empty_quant() {
        // A robust parser must not surface an empty-string quant for a tag that ends in ':'.
        let t = p("deepseek-r1:");
        assert_eq!(t.name, "deepseek-r1");
        assert_eq!(t.quant, None);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let t = p("  gemma3:12b  ");
        assert_eq!(t.name, "gemma3");
        assert_eq!(t.quant.as_deref(), Some("12b"));
    }

    #[test]
    fn registry_with_port_uses_the_last_colon_for_quant() {
        // A real "messy wild" tag: an OCI/registry host carrying a :port. The quant is the
        // LAST ':' suffix, so the port must stay part of the path, not be mistaken for quant.
        // (This is the bug a strong reviewer independently derived from "robust to messy tags".)
        let t = p("registry:5000/library/llama3:Q8_0");
        assert_eq!(t.quant.as_deref(), Some("Q8_0"));
        assert_eq!(t.name, "llama3");
        assert_eq!(t.org.as_deref(), Some("library"));
        assert_eq!(t.registry.as_deref(), Some("registry:5000"));
    }
}
