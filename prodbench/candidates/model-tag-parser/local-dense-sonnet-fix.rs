use serde::Serialize;

/// Represents a parsed model tag, breaking down a raw string into its constituent parts.
///
/// Supports formats like `registry/org/name:quant`, `org/name:quant`, `name:quant`, or `name`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelTag {
    /// The model registry or host (e.g., `hf.co`, `ollama.com`).
    pub registry: Option<String>,
    /// The organization or namespace (e.g., `nvidia`, `meta`).
    pub org: Option<String>,
    /// The base model name (e.g., `NVIDIA-Nemotron-3-Nano-4B-GGUF`, `gemma3`).
    pub name: String,
    /// The quantization or version suffix (e.g., `Q4_K_M`, `12b`, `tiny-h`).
    pub quant: Option<String>,
}

/// Parses a raw model tag string into a structured [`ModelTag`].
///
/// Handles common formats found in local model pickers, gracefully handling missing components,
/// extra whitespace, and redundant slashes.
pub fn parse_model_tag(tag: &str) -> ModelTag {
    let tag = tag.trim();
    let (path, quant) = match tag.rsplit_once(':') {
        Some((p, q)) => {
            let q_trimmed = q.trim();
            (p.trim(), if q_trimmed.is_empty() { None } else { Some(q_trimmed.to_string()) })
        }
        None => (tag, None),
    };

    let parts: Vec<&str> = path.split('/')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let (registry, org, name) = match parts.len() {
        0 => (None, None, String::new()),
        1 => (None, None, parts[0].to_string()),
        2 => (None, Some(parts[0].to_string()), parts[1].to_string()),
        _ => (Some(parts[0].to_string()), Some(parts[1].to_string()), parts[2..].join("/")),
    };

    ModelTag { registry, org, name, quant }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_tag() {
        let tag = parse_model_tag("hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M");
        assert_eq!(tag.registry, Some("hf.co".into()));
        assert_eq!(tag.org, Some("nvidia".into()));
        assert_eq!(tag.name, "NVIDIA-Nemotron-3-Nano-4B-GGUF");
        assert_eq!(tag.quant, Some("Q4_K_M".into()));
    }

    #[test]
    fn test_org_and_quant() {
        let tag = parse_model_tag("nvidia/gemma3:12b");
        assert_eq!(tag.registry, None);
        assert_eq!(tag.org, Some("nvidia".into()));
        assert_eq!(tag.name, "gemma3");
        assert_eq!(tag.quant, Some("12b".into()));
    }

    #[test]
    fn test_name_only() {
        let tag = parse_model_tag("granite4");
        assert_eq!(tag.registry, None);
        assert_eq!(tag.org, None);
        assert_eq!(tag.name, "granite4");
        assert_eq!(tag.quant, None);
    }

    #[test]
    fn test_name_and_quant() {
        let tag = parse_model_tag("granite4:tiny-h");
        assert_eq!(tag.registry, None);
        assert_eq!(tag.org, None);
        assert_eq!(tag.name, "granite4");
        assert_eq!(tag.quant, Some("tiny-h".into()));
    }

    #[test]
    fn test_messy_whitespace_and_slashes() {
        let tag = parse_model_tag("  hf.co//nvidia//model  :  q4  ");
        assert_eq!(tag.registry, Some("hf.co".into()));
        assert_eq!(tag.org, Some("nvidia".into()));
        assert_eq!(tag.name, "model");
        assert_eq!(tag.quant, Some("q4".into()));
    }

    #[test]
    fn test_host_port_and_trailing_colon() {
        let tag = parse_model_tag("localhost:11434/org/model:quant");
        assert_eq!(tag.registry, Some("localhost:11434".into()));
        assert_eq!(tag.org, Some("org".into()));
        assert_eq!(tag.name, "model");
        assert_eq!(tag.quant, Some("quant".into()));

        let tag2 = parse_model_tag("gemma3:");
        assert_eq!(tag2.registry, None);
        assert_eq!(tag2.org, None);
        assert_eq!(tag2.name, "gemma3");
        assert_eq!(tag2.quant, None);
    }
}
