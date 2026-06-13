use serde::Serialize;

/// Represents a parsed model tag, breaking it down into registry, organization,
/// model name, and quantization components.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelTag {
    /// The registry URL or identifier (e.g., `"hf.co"`).
    pub registry: Option<String>,
    /// The organization or user namespace (e.g., `"nvidia"`).
    pub org: Option<String>,
    /// The base model name.
    pub name: String,
    /// The quantization format (e.g., `"Q4_K_M"`).
    pub quant: Option<String>,
}

/// Parses a model tag string into a `ModelTag` struct.
///
/// This function handles various tag formats commonly found in the wild:
/// - `registry/org/name:quant` (e.g., `hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M`)
/// - `org/name:quant` (e.g., `nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M`)
/// - `name:quant` (e.g., `gemma3:12b`)
/// - `name` (e.g., `granite4`)
///
/// The parser is robust against missing components, extra slashes, and empty segments.
///
/// # Arguments
///
/// * `tag` - The raw model tag string to parse.
///
/// # Returns
///
/// A `ModelTag` struct with the parsed components.
pub fn parse_model_tag(tag: &str) -> ModelTag {
    // Split by the last colon to separate the model path from the quantization suffix.
    // Using rsplit_once ensures we capture the quantization even if the model name contains colons.
    let (model_part, quant_part) = match tag.rsplit_once(':') {
        Some((m, q)) => (m, Some(q.to_string())),
        None => (tag, None),
    };

    // Split the model path by slashes, filtering out any empty segments caused by
    // leading/trailing slashes or consecutive slashes.
    let parts: Vec<&str> = model_part
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    // Determine registry, org, and name based on the number of path segments.
    let (registry, org, name) = match parts.len() {
        0 => (None, None, String::new()),
        1 => (None, None, parts[0].to_string()),
        2 => (None, Some(parts[0].to_string()), parts[1].to_string()),
        // For 3 or more segments, the first is registry, second is org,
        // and the rest form the name (joined by slashes).
        _ => (
            Some(parts[0].to_string()),
            Some(parts[1].to_string()),
            parts[2..].join("/"),
        ),
    };

    ModelTag {
        registry,
        org,
        name,
        quant: quant_part,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_hf_tag() {
        let tag = "hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, Some("hf.co".to_string()));
        assert_eq!(parsed.org, Some("nvidia".to_string()));
        assert_eq!(parsed.name, "NVIDIA-Nemotron-3-Nano-4B-GGUF");
        assert_eq!(parsed.quant, Some("Q4_K_M".to_string()));
    }

    #[test]
    fn test_org_name_quant() {
        let tag = "nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, None);
        assert_eq!(parsed.org, Some("nvidia".to_string()));
        assert_eq!(parsed.name, "NVIDIA-Nemotron-3-Nano-4B-GGUF");
        assert_eq!(parsed.quant, Some("Q4_K_M".to_string()));
    }

    #[test]
    fn test_name_quant() {
        let tag = "gemma3:12b";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, None);
        assert_eq!(parsed.org, None);
        assert_eq!(parsed.name, "gemma3");
        assert_eq!(parsed.quant, Some("12b".to_string()));
    }

    #[test]
    fn test_name_only() {
        let tag = "granite4:tiny-h";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, None);
        assert_eq!(parsed.org, None);
        assert_eq!(parsed.name, "granite4");
        assert_eq!(parsed.quant, Some("tiny-h".to_string()));
    }

    #[test]
    fn test_name_no_quant() {
        let tag = "NVIDIA-Nemotron-3-Nano-4B-GGUF";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, None);
        assert_eq!(parsed.org, None);
        assert_eq!(parsed.name, "NVIDIA-Nemotron-3-Nano-4B-GGUF");
        assert_eq!(parsed.quant, None);
    }

    #[test]
    fn test_empty_quant() {
        let tag = "model:";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.name, "model");
        assert_eq!(parsed.quant, Some("".to_string()));
    }

    #[test]
    fn test_leading_slash() {
        let tag = "/nvidia/model:Q4";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, None);
        assert_eq!(parsed.org, Some("nvidia".to_string()));
        assert_eq!(parsed.name, "model");
        assert_eq!(parsed.quant, Some("Q4".to_string()));
    }

    #[test]
    fn test_extra_slashes_in_name() {
        let tag = "hf.co/nvidia/sub/model:Q4";
        let parsed = parse_model_tag(tag);
        assert_eq!(parsed.registry, Some("hf.co".to_string()));
        assert_eq!(parsed.org, Some("nvidia".to_string()));
        assert_eq!(parsed.name, "sub/model");
        assert_eq!(parsed.quant, Some("Q4".to_string()));
    }
}
