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

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
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

    for line in block.split('\n') {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            match key.to_lowercase().as_str() {
                "name" => fm.name = Some(value.to_string()),
                "description" => fm.description = Some(value.to_string()),
                "license" => fm.license = Some(value.to_string()),
                "compatibility" => fm.compatibility = Some(value.to_string()),
                "allowed-tools" | "allowed_tools" => fm.allowed_tools = Some(value.to_string()),
                _ => {}
            }
        }
    }

    (Some(fm), body)
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
