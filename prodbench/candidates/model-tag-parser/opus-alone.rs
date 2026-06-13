use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelTag {
    pub registry: Option<String>,
    pub org: Option<String>,
    pub name: String,
    pub quant: Option<String>,
}

fn non_empty(part: &str) -> Option<String> {
    let trimmed = part.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn parse_model_tag(tag: &str) -> ModelTag {
    let tag = tag.trim();

    let (path_part, quant) = match tag.split_once(':') {
        Some((path, suffix)) => (path, non_empty(suffix)),
        None => (tag, None),
    };

    let segments: Vec<&str> = path_part
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut model = ModelTag {
        registry: None,
        org: None,
        name: String::new(),
        quant,
    };

    match segments.len() {
        0 => {}
        1 => {
            model.name = segments[0].to_string();
        }
        2 => {
            model.org = non_empty(segments[0]);
            model.name = segments[1].to_string();
        }
        n => {
            model.name = segments[n - 1].to_string();
            model.org = non_empty(segments[n - 2]);
            let registry = segments[..n - 2].join("/");
            model.registry = non_empty(&registry);
        }
    }

    model
}
