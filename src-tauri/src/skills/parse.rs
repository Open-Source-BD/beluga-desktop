//! Lenient SKILL.md reader. No YAML crate: plain `key: value` lines only.

use super::ParsedSkill;

const DESCRIPTION_MAX_CHARS: usize = 200;

/// Read one SKILL.md. `Err` only when there is no body text at all.
pub fn parse_skill(text: &str, dir_name: &str) -> Result<ParsedSkill, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n");
    let (front, body) = split_frontmatter(&text);
    if body.trim().is_empty() {
        return Err("The skill file has no text in it.".to_string());
    }
    let fields = read_fields(front);
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let name = get("name").unwrap_or_else(|| dir_name.to_string());
    let description = get("description")
        .or_else(|| fallback_description(body))
        .unwrap_or_default();
    Ok(ParsedSkill {
        name,
        description: shorten(&description),
        body: body.trim_start_matches('\n').to_string(),
        license: get("license"),
    })
}

/// Splits `---` frontmatter from the body. Without a closing `---` line the
/// whole text is the body.
fn split_frontmatter(text: &str) -> (&str, &str) {
    let mut lines = text.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return ("", text),
    }
    let mut offset = text.find('\n').map_or(text.len(), |i| i + 1);
    let start = offset;
    for line in lines {
        if line.trim_end() == "---" {
            return (&text[start..offset], &text[offset + line.len()..]);
        }
        offset += line.len();
    }
    ("", text)
}

/// Plain `key: value` lines, quoted or not, plus `|`/`>` block scalars and
/// indented continuation lines. Anything else is ignored.
fn read_fields(front: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let value = value.trim();
        let mut parts: Vec<String> = Vec::new();
        let block = value.starts_with(['|', '>']);
        if !block && !value.is_empty() {
            parts.push(unquote(value));
        }
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() || next.starts_with([' ', '\t']) {
                parts.push(next.trim().to_string());
                lines.next();
            } else {
                break;
            }
        }
        out.push((key.to_string(), collapse(&parts.join(" "))));
    }
    out
}

fn unquote(value: &str) -> String {
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            let inner = &value[1..value.len() - 1];
            return if q == '"' {
                inner.replace("\\\"", "\"").replace("\\\\", "\\")
            } else {
                inner.replace("''", "'")
            };
        }
    }
    value.to_string()
}

/// First non-empty line under a "Description" or "Metadata" heading, else the
/// first paragraph that is not a heading.
fn fallback_description(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if !t.starts_with('#') {
            continue;
        }
        let heading = t.to_lowercase();
        if !(heading.contains("description") || heading.contains("metadata")) {
            continue;
        }
        for next in &lines[i + 1..] {
            if next.trim_start().starts_with('#') {
                break;
            }
            let cleaned = strip_label(strip_list_marker(next.trim()));
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    let mut paragraph: Vec<&str> = Vec::new();
    for line in &lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        paragraph.push(t);
    }
    if paragraph.is_empty() {
        None
    } else {
        Some(paragraph.join(" "))
    }
}

fn strip_list_marker(line: &str) -> &str {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = if digits > 0 {
        line[digits..].strip_prefix(['.', ')'])
    } else {
        line.strip_prefix(['-', '*', '+'])
    };
    match rest {
        Some(rest) if rest.starts_with(' ') => rest.trim_start(),
        _ => line,
    }
}

/// Drops a leading `**Description:**` or `**Description**:` style label.
fn strip_label(line: &str) -> &str {
    let Some(rest) = line.strip_prefix("**") else {
        return line;
    };
    let Some(end) = rest.find("**") else {
        return line;
    };
    let label = &rest[..end];
    let after = &rest[end + 2..];
    if label.ends_with(':') {
        after.trim()
    } else if let Some(after) = after.strip_prefix(':') {
        after.trim()
    } else {
        line
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line, at most 200 characters, cut on a word boundary. Counts Unicode
/// characters, so a multi-byte character is never split.
fn shorten(description: &str) -> String {
    let one_line = collapse(description);
    let Some((cut, _)) = one_line.char_indices().nth(DESCRIPTION_MAX_CHARS) else {
        return one_line;
    };
    let head = &one_line[..cut];
    if one_line[cut..].starts_with(char::is_whitespace) {
        return head.trim_end().to_string();
    }
    match head.rfind(char::is_whitespace) {
        Some(i) => head[..i].trim_end().to_string(),
        None => head.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str = include_str!("../../tests/fixtures/skills/normal.md");
    const QUOTED: &str = include_str!("../../tests/fixtures/skills/quoted_block.md");
    const NO_FM: &str = include_str!("../../tests/fixtures/skills/no_frontmatter.md");
    const BROKEN: &str = include_str!("../../tests/fixtures/skills/broken_line.md");

    #[test]
    fn frontmatter_name_and_description() {
        let p = parse_skill(NORMAL, "dir").unwrap();
        assert_eq!(p.name, "brainstorming");
        assert!(
            p.description
                .starts_with("You MUST use this before any creative work")
        );
        assert!(p.description.ends_with("planning anything new."));
        assert!(p.body.contains("# Brainstorming"));
        assert!(!p.body.contains("---"));
        assert_eq!(p.license, None);
    }

    #[test]
    fn quoted_values_and_block_scalar_description() {
        let p = parse_skill(QUOTED, "dir").unwrap();
        assert_eq!(p.name, "quoted-name");
        assert_eq!(
            p.description,
            "Helps plan a trip by asking about dates, budget and interests. Second paragraph is folded too."
        );
        let lit = "---\nname: x\ndescription: |\n  line one\n  line two\n---\nbody\n";
        assert_eq!(
            parse_skill(lit, "d").unwrap().description,
            "line one line two"
        );
        let dq = "---\nname: \"a b\"\ndescription: \"say \\\"hi\\\"\"\n---\nbody\n";
        assert_eq!(parse_skill(dq, "d").unwrap().name, "a b");
    }

    #[test]
    fn missing_frontmatter_uses_dir_name_and_first_paragraph() {
        let p = parse_skill(NO_FM, "garden-planner").unwrap();
        assert_eq!(p.name, "garden-planner");
        assert_eq!(
            p.description,
            "Plans a vegetable garden layout by season and available space."
        );
        assert!(p.body.contains("## Metadata"));
        let plain = "# Title\n\nFirst paragraph\nspans two lines.\n\nSecond.\n";
        assert_eq!(
            parse_skill(plain, "d").unwrap().description,
            "First paragraph spans two lines."
        );
    }

    #[test]
    fn name_in_frontmatter_differs_from_dir() {
        let p = parse_skill(NORMAL, "some-other-dir").unwrap();
        assert_eq!(p.name, "brainstorming");
    }

    #[test]
    fn broken_yaml_line_is_ignored_and_other_fields_still_read() {
        let p = parse_skill(BROKEN, "dir").unwrap();
        assert_eq!(p.name, "sturdy");
        assert_eq!(p.description, "Still read");
        assert_eq!(p.body.trim(), "Body text here.");
    }

    #[test]
    fn bom_and_crlf_are_tolerated() {
        // Built here: .gitattributes would rewrite CRLF in a fixture file.
        let crlf_bom = "\u{feff}---\nname: crlf-skill\ndescription: Works with CRLF\n---\n\n# Body\n\nSome text.\n"
            .replace('\n', "\r\n");
        let p = parse_skill(&crlf_bom, "dir").unwrap();
        assert_eq!(p.name, "crlf-skill");
        assert_eq!(p.description, "Works with CRLF");
        assert!(!p.body.contains('\r'));
        assert!(p.body.contains("Some text."));
    }

    #[test]
    fn description_is_one_line_and_at_most_200_chars() {
        let long = "word ".repeat(100);
        let text = format!("---\nname: x\ndescription: {long}\n---\nbody\n");
        let d = parse_skill(&text, "d").unwrap().description;
        assert!(d.chars().count() <= 200);
        assert!(!d.contains('\n'));
        assert!(d.ends_with("word"), "cut on a word boundary: {d:?}");
        // One unbroken word longer than the limit is hard-cut.
        let text = format!(
            "---\nname: x\ndescription: {}\n---\nbody\n",
            "a".repeat(500)
        );
        assert_eq!(
            parse_skill(&text, "d").unwrap().description.chars().count(),
            200
        );
    }

    #[test]
    fn description_cut_never_splits_multibyte_characters() {
        let long = "héllo wörld 日本語 ".repeat(40);
        let text = format!("---\nname: x\ndescription: {long}\n---\nbody\n");
        let d = parse_skill(&text, "d").unwrap().description;
        assert!(d.chars().count() <= 200);
        assert!(d.ends_with("日本語") || d.ends_with("wörld") || d.ends_with("héllo"));
        let text = format!(
            "---\nname: x\ndescription: {}\n---\nbody\n",
            "日".repeat(500)
        );
        let d = parse_skill(&text, "d").unwrap().description;
        assert_eq!(d.chars().count(), 200);
    }

    #[test]
    fn empty_file_is_err() {
        assert!(parse_skill("", "d").is_err());
        assert!(parse_skill("\u{feff}  \n\n", "d").is_err());
    }

    #[test]
    fn frontmatter_only_is_err() {
        assert!(parse_skill("---\nname: x\ndescription: y\n---\n", "d").is_err());
        assert!(parse_skill("---\nname: x\ndescription: y\n---\n  \n\n", "d").is_err());
    }

    #[test]
    fn five_megabyte_body_parses_without_panic() {
        let body = "line of text for the body\n".repeat(200_000);
        assert!(body.len() > 5_000_000);
        let text = format!("---\nname: big\ndescription: huge\n---\n{body}");
        let p = parse_skill(&text, "d").unwrap();
        assert_eq!(p.name, "big");
        assert!(p.body.len() > 5_000_000);
        let p = parse_skill(&body, "d").unwrap();
        assert!(!p.description.is_empty());
    }

    #[test]
    fn license_field_is_read() {
        let p = parse_skill(QUOTED, "d").unwrap();
        assert_eq!(p.license.as_deref(), Some("MIT"));
    }
}
