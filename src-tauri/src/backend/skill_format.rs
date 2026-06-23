// SKILL.md open-standard FORMAT (agentskills.io / Linux-Foundation AAIF) — the foundation of the
// skills marketplace + unified runtime. A skill package is a dir with a `SKILL.md` whose head is a
// `---`-fenced YAML frontmatter (name/description/license/compatibility/allowed-tools/metadata) and
// whose body is the markdown persona/instructions. This module parses that frontmatter so OUR skills
// are EXPORT-able and external SKILL.md skills are IMPORT-able 1:1.
//
// Dependency-free on purpose (no serde_yaml/gray_matter crate — offline-build safe): a minimal
// line-based scalar parser for the known fields. BACKWARD-COMPATIBLE: a bare `.md` with no
// frontmatter parses to `(None, content)` — byte-identical to today's persona handling, so every
// existing bundled/project skill is unaffected.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
    /// agentskills.io optional `metadata` map (e.g. `author`, `version`) — one level, string→string.
    pub metadata: BTreeMap<String, String>,
}

/// Strip a single layer of matching surrounding single/double quotes from a trimmed scalar value.
/// Shared by the top-level scalar path and the `metadata` map path so both quote-strip identically.
fn strip_quotes(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Split a SKILL.md into its optional frontmatter + the body. No frontmatter (or an unterminated /
/// malformed fence) ⇒ `(None, content)` with the body == the ORIGINAL content (byte-identical). The
/// body is returned as a borrowed slice of `content` (no allocation).
pub fn parse_skill_frontmatter(content: &str) -> (Option<SkillFrontmatter>, &str) {
    let bom_len = if content.starts_with('\u{feff}') { 3 } else { 0 };
    let content_no_bom = &content[bom_len..];

    if !content_no_bom.starts_with("---") {
        return (None, content);
    }
    // The opening fence must be a line that is EXACTLY `---` (as strict as the closing fence). A bare
    // markdown file starting with a `---` HORIZONTAL RULE (`--- text`, `----`, `---` then content —
    // common in READMEs/Hugo/Jekyll) is NOT frontmatter: treat it as body so the return is
    // byte-identical and a later `---` HR can't be mistaken for a closing fence (silent truncation).
    let opening_rest = content_no_bom[3..].split('\n').next().unwrap_or("");
    if !opening_rest.trim_end_matches('\r').is_empty() {
        return (None, content);
    }

    let mut cursor = bom_len + 3;
    if cursor < content.len() && content.as_bytes().get(cursor) == Some(&b'\r') {
        cursor += 1;
    }
    if cursor < content.len() && content.as_bytes().get(cursor) == Some(&b'\n') {
        cursor += 1;
    }

    let block_start = cursor;
    let mut closing_fence_end = None;

    while cursor < content.len() {
        let line_end = content[cursor..]
            .find('\n')
            .map(|i| i + cursor)
            .unwrap_or(content.len());
        let line = &content[cursor..line_end];
        if line.trim_end_matches('\r') == "---" {
            closing_fence_end = Some(line_end);
            break;
        }
        cursor = line_end;
        if cursor < content.len() && content.as_bytes().get(cursor) == Some(&b'\n') {
            cursor += 1;
        }
    }

    let closing_fence_end = match closing_fence_end {
        Some(idx) => idx,
        None => return (None, content),
    };

    let body = if closing_fence_end < content.len() {
        &content[closing_fence_end + 1..]
    } else {
        ""
    };

    let block = &content[block_start..closing_fence_end];
    let mut fm = SkillFrontmatter::default();
    // The `metadata:` block is the one place where INDENTATION is significant, so we must inspect
    // each raw line for leading whitespace BEFORE trimming. While `in_metadata` is set, an indented
    // `key: value` becomes a map entry; the FIRST dedented (or empty/comment) line closes the block
    // and resumes normal top-level scalar parsing — so a `license:` after the block still lands in
    // `fm.license`, and an indented metadata key never leaks into a top-level field.
    let mut in_metadata = false;

    for raw_line in block.split('\n') {
        let raw_line = raw_line.trim_end_matches('\r');
        let is_indented = raw_line.starts_with(' ') || raw_line.starts_with('\t');
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            // Blank/comment lines neither close nor extend the metadata block (YAML treats them as
            // structurally inert); top-level parsing is likewise a no-op for them.
            continue;
        }

        if in_metadata {
            if is_indented {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    if !key.is_empty() {
                        fm.metadata
                            .insert(key.to_string(), strip_quotes(value.trim()).to_string());
                    }
                }
                continue;
            }
            // Dedented non-blank line ⇒ the metadata block is over; fall through to top-level parsing.
            in_metadata = false;
        }

        // A line indented OUTSIDE a metadata block is not a valid top-level scalar (frontmatter
        // fields are flush-left); ignore it so an indented `name:`/`license:` can't shadow a real
        // top-level field. Reached only when not in a metadata block — a dedented block-closer is
        // flush-left, so it still parses below.
        if is_indented {
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = strip_quotes(value.trim());
            match key.to_lowercase().as_str() {
                "name" => fm.name = Some(value.to_string()),
                "description" => fm.description = Some(value.to_string()),
                "license" => fm.license = Some(value.to_string()),
                "compatibility" => fm.compatibility = Some(value.to_string()),
                "allowed-tools" | "allowed_tools" => fm.allowed_tools = Some(value.to_string()),
                "metadata" if value.is_empty() => in_metadata = true,
                _ => {}
            }
        }
    }

    (Some(fm), body)
}

/// Outcome of validating a parsed SKILL.md against the agentskills.io spec. Non-conformance is
/// reported as WARNINGS, never a hard failure — the install path stays tolerant (D2 policy).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SkillValidation {
    pub conformant: bool,
    pub warnings: Vec<String>,
}

/// Validate `fm` (parsed from `<dir_name>/SKILL.md`) against the agentskills.io specification:
/// name (1-64, `a-z0-9-`, no leading/trailing/consecutive hyphen, must match `dir_name`),
/// description (non-empty, ≤1024), compatibility (≤500). Returns warnings; never rejects.
pub fn validate_skill(fm: &SkillFrontmatter, dir_name: &str) -> SkillValidation {
    let mut warnings: Vec<String> = Vec::new();

    // --- name: required; 1-64; lowercase a-z/0-9/-; no leading/trailing/consecutive '-'; == dir ---
    match fm.name.as_deref() {
        None => warnings.push("missing required `name` field".to_string()),
        Some(name) if name.is_empty() => {
            warnings.push("`name` must not be empty".to_string())
        }
        Some(name) => {
            if name.chars().count() > 64 {
                warnings.push("`name` must be at most 64 characters".to_string());
            }
            if !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                warnings.push(
                    "`name` may only contain lowercase letters, digits, and hyphens".to_string(),
                );
            }
            if name.starts_with('-') {
                warnings.push("`name` must not start with a hyphen".to_string());
            }
            if name.ends_with('-') {
                warnings.push("`name` must not end with a hyphen".to_string());
            }
            if name.contains("--") {
                warnings.push("`name` must not contain consecutive hyphens".to_string());
            }
            if name != dir_name {
                warnings.push(format!(
                    "`name` ({name:?}) must match the parent directory name ({dir_name:?})"
                ));
            }
        }
    }

    // --- description: required; non-empty; <= 1024 chars ---
    match fm.description.as_deref() {
        None => warnings.push("missing required `description` field".to_string()),
        Some(desc) if desc.is_empty() => {
            warnings.push("`description` must not be empty".to_string())
        }
        Some(desc) if desc.chars().count() > 1024 => {
            warnings.push("`description` must be at most 1024 characters".to_string())
        }
        Some(_) => {}
    }

    // --- compatibility: optional; if present, <= 500 chars ---
    if let Some(compat) = fm.compatibility.as_deref() {
        if compat.chars().count() > 500 {
            warnings.push("`compatibility` must be at most 500 characters".to_string());
        }
    }

    SkillValidation {
        conformant: warnings.is_empty(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frontmatter_passes_through() {
        let input = "You are a veteran Rust engineer.\nLine 2";
        let (fm, body) = parse_skill_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn parses_name_description_and_body() {
        let input = "---\nname: rust-pro\ndescription: Veteran Rust\n---\nBODY LINE 1\nBODY 2";
        let (fm, body) = parse_skill_frontmatter(input);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.name, Some("rust-pro".to_string()));
        assert_eq!(fm.description, Some("Veteran Rust".to_string()));
        assert_eq!(body, "BODY LINE 1\nBODY 2");
    }

    #[test]
    fn parses_allowed_tools_both_spellings() {
        let input1 = "---\nallowed-tools: Bash(git:*) Read\n---\nBODY";
        let (fm1, _) = parse_skill_frontmatter(input1);
        assert_eq!(
            fm1.unwrap().allowed_tools,
            Some("Bash(git:*) Read".to_string())
        );

        let input2 = "---\nallowed_tools: Bash(git:*) Read\n---\nBODY";
        let (fm2, _) = parse_skill_frontmatter(input2);
        assert_eq!(
            fm2.unwrap().allowed_tools,
            Some("Bash(git:*) Read".to_string())
        );
    }

    #[test]
    fn strips_surrounding_quotes() {
        let input1 = "---\ndescription: \"Quoted desc\"\n---\nBODY";
        let (fm1, _) = parse_skill_frontmatter(input1);
        assert_eq!(fm1.unwrap().description, Some("Quoted desc".to_string()));

        let input2 = "---\ndescription: 'Quoted desc'\n---\nBODY";
        let (fm2, _) = parse_skill_frontmatter(input2);
        assert_eq!(fm2.unwrap().description, Some("Quoted desc".to_string()));
    }

    #[test]
    fn unterminated_frontmatter_is_treated_as_no_frontmatter() {
        let input = "---\nname: x\nno closing fence";
        let (fm, body) = parse_skill_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn empty_frontmatter_block() {
        let input = "---\n---\nBODY";
        let (fm, body) = parse_skill_frontmatter(input);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.name, None);
        assert_eq!(fm.description, None);
        assert_eq!(fm.license, None);
        assert_eq!(fm.compatibility, None);
        assert_eq!(fm.allowed_tools, None);
        assert_eq!(body, "BODY");
    }

    #[test]
    fn crlf_line_endings() {
        let input = "---\r\nname: x\r\n---\r\nBODY\r\nB2";
        let (fm, body) = parse_skill_frontmatter(input);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.name, Some("x".to_string()));
        assert_eq!(body, "BODY\r\nB2");
    }

    #[test]
    fn ignores_unknown_keys_and_comments() {
        let input = "---\n# comment\nweird: y\nname: ok\n---\nBODY";
        let (fm, _) = parse_skill_frontmatter(input);
        let fm = fm.expect("should parse frontmatter");
        assert_eq!(fm.name, Some("ok".to_string()));
        assert_eq!(fm.description, None);
    }

    #[test]
    fn content_on_the_opening_fence_line_is_not_frontmatter() {
        // The opening line must be EXACTLY `---`. A `--- text` (content on the fence line) or a
        // `----` 4-dash HR is NOT frontmatter → byte-identical (None, content), so a later `---` HR
        // can't be mistaken for a closing fence and silently drop the body before it.
        for input in ["--- not a fence\nx\n---\ny", "----\nx\n---\ny", "---hr\n---\nb"] {
            let (fm, body) = parse_skill_frontmatter(input);
            assert!(fm.is_none(), "input {input:?} must not be frontmatter");
            assert_eq!(body, input, "body must be byte-identical for {input:?}");
        }
        // A first line that IS exactly `---` is, by the frontmatter convention, a real fence —
        // it parses (the standard behavior, matching Jekyll/Hugo). Not a backward-compat break for
        // us: no bundled/role persona starts with a bare `---` line.
        let std = "---\nname: x\n---\nbody";
        assert!(parse_skill_frontmatter(std).0.is_some());
    }
}

// ---- TDD CONTRACT for F-spec (agentskills.io conformance). These FAIL against the stubs above;
// veteran-coder implements the metadata parser + validator to turn them green. ----
#[cfg(test)]
mod spec_conformance_tests {
    use super::*;

    fn fm(name: &str, desc: &str) -> SkillFrontmatter {
        SkillFrontmatter {
            name: Some(name.to_string()),
            description: Some(desc.to_string()),
            ..Default::default()
        }
    }

    // ---------- D1: metadata map parsing ----------
    #[test]
    fn parses_metadata_map() {
        let input =
            "---\nname: pdf-processing\ndescription: x\nmetadata:\n  author: example-org\n  version: \"1.0\"\n---\nBODY";
        let (parsed, _) = parse_skill_frontmatter(input);
        let parsed = parsed.unwrap();
        assert_eq!(parsed.metadata.get("author").map(String::as_str), Some("example-org"));
        assert_eq!(parsed.metadata.get("version").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn metadata_block_does_not_swallow_following_dedented_scalar() {
        let input = "---\nname: x\nmetadata:\n  author: a\nlicense: Apache-2.0\n---\nBODY";
        let (parsed, _) = parse_skill_frontmatter(input);
        let parsed = parsed.unwrap();
        assert_eq!(parsed.metadata.get("author").map(String::as_str), Some("a"));
        // The dedented `license:` after the metadata block must still parse as a top-level field…
        assert_eq!(parsed.license.as_deref(), Some("Apache-2.0"));
        // …and an indented metadata key must NOT leak into a top-level field.
        assert_eq!(parsed.name.as_deref(), Some("x"));
    }

    #[test]
    fn absent_metadata_is_empty() {
        let input = "---\nname: x\ndescription: d\n---\nBODY";
        let (parsed, _) = parse_skill_frontmatter(input);
        assert!(parsed.unwrap().metadata.is_empty());
    }

    // ---------- D2/D3: validator (warnings, never fatal) ----------
    #[test]
    fn conformant_skill_has_no_warnings() {
        let v = validate_skill(&fm("code-review", "Reviews diffs. Use when reviewing code."), "code-review");
        assert!(v.conformant, "expected conformant, got warnings: {:?}", v.warnings);
        assert!(v.warnings.is_empty());
    }

    #[test]
    fn underscore_in_name_warns() {
        let v = validate_skill(&fm("my_skill", "d"), "my_skill");
        assert!(!v.conformant);
        assert!(v.warnings.iter().any(|w| w.to_lowercase().contains("name")));
    }

    #[test]
    fn consecutive_hyphens_in_name_warn() {
        assert!(!validate_skill(&fm("pdf--proc", "d"), "pdf--proc").conformant);
    }

    #[test]
    fn trailing_hyphen_in_name_warns() {
        assert!(!validate_skill(&fm("pdf-", "d"), "pdf-").conformant);
    }

    #[test]
    fn uppercase_in_name_warns() {
        assert!(!validate_skill(&fm("PDF-Processing", "d"), "PDF-Processing").conformant);
    }

    #[test]
    fn name_longer_than_64_warns() {
        let long = "a".repeat(65);
        assert!(!validate_skill(&fm(&long, "d"), &long).conformant);
    }

    #[test]
    fn name_must_match_parent_dir() {
        let v = validate_skill(&fm("foo", "d"), "bar");
        assert!(!v.conformant);
        assert!(v
            .warnings
            .iter()
            .any(|w| { let l = w.to_lowercase(); l.contains("dir") || l.contains("parent") }));
    }

    #[test]
    fn missing_name_warns() {
        let mut f = fm("placeholder", "d");
        f.name = None;
        assert!(!validate_skill(&f, "placeholder").conformant);
    }

    #[test]
    fn empty_or_missing_description_warns() {
        assert!(!validate_skill(&fm("ok", ""), "ok").conformant);
        let mut f = fm("ok", "d");
        f.description = None;
        assert!(!validate_skill(&f, "ok").conformant);
    }

    #[test]
    fn description_longer_than_1024_warns() {
        assert!(!validate_skill(&fm("ok", &"x".repeat(1025)), "ok").conformant);
    }

    #[test]
    fn compatibility_longer_than_500_warns() {
        let mut f = fm("ok", "d");
        f.compatibility = Some("x".repeat(501));
        assert!(!validate_skill(&f, "ok").conformant);
    }
}
