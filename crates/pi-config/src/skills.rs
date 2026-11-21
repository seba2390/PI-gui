//! pi skills: `SKILL.md` files with YAML front matter (`name`, `description`,
//! `disable-model-invocation`) followed by instructions. See pi's `docs/skills.md` and the
//! Agent Skills specification.
//!
//! Only the small subset of YAML that skill front matter uses is understood here: `key: value`
//! lines with plain, single- or double-quoted scalars, block scalars (`|`, `>`), and plain
//! values continued on indented lines. Anything else is kept as is when the file is changed.

use crate::ConfigError;

/// pi's limits.
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_DESCRIPTION_LEN: usize = 1024;

/// The front matter fields the GUI shows and edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: Option<String>,
    pub description: Option<String>,
    /// `disable-model-invocation: true`: only loaded through `/skill:name`.
    pub only_on_request: bool,
}

/// Checks a skill name: lowercase letters, digits and single hyphens (not at either end), at
/// most 64 characters.
pub fn validate_name(name: &str) -> Result<(), ConfigError> {
    let invalid = |message: &str| Err(ConfigError::Invalid(message.to_owned()));
    if name.is_empty() {
        return invalid("Enter a name, like pdf-tools.");
    }
    if name.len() > MAX_NAME_LEN {
        return invalid("The name can have at most 64 characters.");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return invalid("Use only lowercase letters, digits and hyphens, like pdf-tools.");
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return invalid("Hyphens go between words: no leading, trailing or double hyphens.");
    }
    Ok(())
}

/// Checks a description: present and at most 1024 characters.
pub fn validate_description(description: &str) -> Result<(), ConfigError> {
    let description = description.trim();
    if description.is_empty() {
        return Err(ConfigError::Invalid(
            "Describe what the skill does and when to use it: pi decides from this alone \
             when to load it."
                .into(),
        ));
    }
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(ConfigError::Invalid(
            "The description can have at most 1024 characters.".into(),
        ));
    }
    Ok(())
}

/// A new `SKILL.md`.
pub fn render_skill(name: &str, description: &str, only_on_request: bool, body: &str) -> String {
    let mut text = format!(
        "---\nname: {name}\ndescription: {}\n",
        yaml_string(description.trim())
    );
    if only_on_request {
        text.push_str("disable-model-invocation: true\n");
    }
    text.push_str("---\n\n");
    let body = body.trim();
    if body.is_empty() {
        text.push_str("# ");
        text.push_str(name);
        text.push('\n');
    } else {
        text.push_str(body);
        text.push('\n');
    }
    text
}

/// A YAML scalar for `value`: plain when that reads back unchanged, else double-quoted (JSON
/// string syntax, which YAML accepts).
fn yaml_string(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.ends_with(':')
        && !value.contains(['\n', '\t', '"'])
        && !value.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '%', '@',
            '`', ' ',
        ])
        && !value.ends_with(' ')
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
        )
        && value.parse::<f64>().is_err();
    if plain {
        value.to_owned()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
    }
}

/// The front matter block of `text`: its lines (without the `---` fences) and the byte range
/// of the whole block including fences, or `None` when the file has none.
fn front_matter_block(text: &str) -> Option<(Vec<&str>, usize, usize)> {
    let text_start = usize::from(text.starts_with('\u{feff}')) * 3;
    let rest = &text[text_start..];
    let first_end = rest.find('\n')?;
    if rest[..first_end].trim_end() != "---" {
        return None;
    }
    let mut offset = text_start + first_end + 1;
    let mut lines = Vec::new();
    for line in text[offset..].split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.trim_end() == "---" || content.trim_end() == "..." {
            return Some((lines, text_start, offset + line.len()));
        }
        lines.push(content);
        offset += line.len();
    }
    None
}

/// Reads the fields the GUI uses from a skill file's front matter.
pub fn parse_front_matter(text: &str) -> FrontMatter {
    let Some((lines, _, _)) = front_matter_block(text) else {
        return FrontMatter::default();
    };
    let mut front = FrontMatter::default();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        // Continuation lines (indented) belong to this key.
        let mut continuation = Vec::new();
        while index < lines.len()
            && (lines[index].starts_with([' ', '\t']) || lines[index].trim().is_empty())
        {
            continuation.push(lines[index]);
            index += 1;
        }
        let value = scalar(value, &continuation);
        match key {
            "name" => front.name = Some(value),
            "description" => front.description = Some(value),
            "disable-model-invocation" => front.only_on_request = value == "true",
            _ => {}
        }
    }
    front
}

/// The string value of a scalar whose first line is `first` and whose indented continuation
/// lines are `more`.
fn scalar(first: &str, more: &[&str]) -> String {
    if first.starts_with('|') || first.starts_with('>') {
        let folded = first.starts_with('>');
        let lines: Vec<&str> = more.iter().map(|line| line.trim()).collect();
        let joined = if folded {
            lines.join(" ")
        } else {
            lines.join("\n")
        };
        return joined.trim().to_owned();
    }
    let mut text = first.to_owned();
    for line in more {
        let line = line.trim();
        if !line.is_empty() {
            text.push(' ');
            text.push_str(line);
        }
    }
    let text = text.trim();
    if text.starts_with('"') {
        if let Ok(value) = serde_json::from_str::<String>(text) {
            return value;
        }
        return text.trim_matches('"').to_owned();
    }
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    // A plain scalar ends at a comment.
    text.split(" #").next().unwrap_or(text).trim().to_owned()
}

/// `text` with `disable-model-invocation` set (`true`) or removed (`false`); everything else
/// is kept. A file without front matter is returned unchanged.
pub fn set_only_on_request(text: &str, only_on_request: bool) -> String {
    let Some((lines, start, end)) = front_matter_block(text) else {
        return text.to_owned();
    };
    let mut kept: Vec<String> = lines
        .iter()
        .filter(|line| {
            line.split_once(':').is_none_or(|(key, _)| {
                key.trim() != "disable-model-invocation" || line.starts_with(' ')
            })
        })
        .map(|line| (*line).to_owned())
        .collect();
    if only_on_request {
        kept.push("disable-model-invocation: true".into());
    }
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str(&text[..start]);
    out.push_str("---\n");
    for line in kept {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&text[end..]);
    out
}

/// Checks a whole `SKILL.md` before saving: front matter with a valid name and a description
/// (pi skips a skill without one).
pub fn validate_skill_file(text: &str) -> Result<FrontMatter, ConfigError> {
    if front_matter_block(text).is_none() {
        return Err(ConfigError::Invalid(
            "SKILL.md must start with front matter: a --- line, name: and description:, \
             then another --- line."
                .into(),
        ));
    }
    let front = parse_front_matter(text);
    validate_description(front.description.as_deref().unwrap_or_default())?;
    if let Some(name) = &front.name {
        validate_name(name)?;
    }
    Ok(front)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names() {
        for good in ["pdf-tools", "a", "x1-y2", &"a".repeat(64)] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "PDF",
            "pdf_tools",
            "-a",
            "a-",
            "a--b",
            "a b",
            &"a".repeat(65),
            "æ",
        ] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert!(validate_description(" ").is_err());
        assert!(validate_description(&"x".repeat(1025)).is_err());
        assert!(validate_description("Extract text. Use for PDFs.").is_ok());
    }

    #[test]
    fn renders_and_reads_back() {
        for description in [
            "Extract text from PDFs. Use when reading PDFs.",
            "Use when: something has a colon",
            "true",
            "#hash first",
            "It's \"quoted\" and a # in it",
            "123",
        ] {
            let text = render_skill("pdf-tools", description, false, "Do things.\n");
            let front = validate_skill_file(&text).unwrap();
            assert_eq!(front.name.as_deref(), Some("pdf-tools"));
            assert_eq!(front.description.as_deref(), Some(description), "{text}");
            assert!(!front.only_on_request);
            assert!(text.ends_with("---\n\nDo things.\n"), "{text}");
        }
        let text = render_skill("commit", "Write commits.", true, "");
        assert!(parse_front_matter(&text).only_on_request);
        assert!(text.ends_with("# commit\n"));
    }

    #[test]
    fn parses_yaml_forms() {
        let text = "---\nname: 'it''s'\ndescription: >\n  Folded line one\n  and two.\nlicense: MIT # c\ndisable-model-invocation: true\nmetadata:\n  a: b\n---\nbody\n";
        let front = parse_front_matter(text);
        assert_eq!(front.name.as_deref(), Some("it's"));
        assert_eq!(
            front.description.as_deref(),
            Some("Folded line one and two.")
        );
        assert!(front.only_on_request);

        let plain = "---\ndescription: Starts here\n  and continues # comment\n---\n";
        assert_eq!(
            parse_front_matter(plain).description.as_deref(),
            Some("Starts here and continues")
        );
        let literal = "---\ndescription: |\n  one\n  two\n---\n";
        assert_eq!(
            parse_front_matter(literal).description.as_deref(),
            Some("one\ntwo")
        );
        assert_eq!(
            parse_front_matter("no front matter"),
            FrontMatter::default()
        );
        assert_eq!(parse_front_matter("---\nname: x\n"), FrontMatter::default());
    }

    #[test]
    fn toggles_the_flag_keeping_the_rest() {
        let text = "---\nname: x\ndescription: \"d: e\"\nmetadata:\n  disable-model-invocation: keep\n---\n\n# Body\n---\nnot front matter\n";
        let on = set_only_on_request(text, true);
        assert!(parse_front_matter(&on).only_on_request);
        assert!(on.contains("  disable-model-invocation: keep\n"));
        assert!(
            on.ends_with("---\n\n# Body\n---\nnot front matter\n"),
            "{on}"
        );
        assert_eq!(parse_front_matter(&on).description.as_deref(), Some("d: e"));
        let off = set_only_on_request(&on, false);
        assert_eq!(off, text);
        assert_eq!(set_only_on_request("plain", true), "plain");
    }

    #[test]
    fn validates_files() {
        assert!(validate_skill_file("# no front matter").is_err());
        assert!(validate_skill_file("---\nname: x\n---\n").is_err());
        assert!(validate_skill_file("---\nname: Bad Name\ndescription: d\n---\n").is_err());
        // pi falls back to the folder name when `name` is missing.
        assert!(validate_skill_file("---\ndescription: d\n---\n").is_ok());
    }
}
