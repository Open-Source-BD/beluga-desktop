//! Collect the pieces of a streamed tool call, and read their arguments leniently.

use crate::agent::ToolCall;
use serde_json::Value;
use std::collections::BTreeMap;

/// One streamed piece of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub index: u32,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

#[derive(Default)]
struct Partial {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Joins fragments by index into whole calls.
#[derive(Default)]
pub struct Collector {
    calls: BTreeMap<u32, Partial>,
}

impl Collector {
    pub fn push(&mut self, fragment: Fragment) {
        let slot = self.calls.entry(fragment.index).or_default();
        // Id and name arrive whole; a later repeat must not be appended.
        if slot.id.is_none() {
            slot.id = fragment.id.filter(|s| !s.is_empty());
        }
        if slot.name.is_none() {
            slot.name = fragment.name.filter(|s| !s.is_empty());
        }
        slot.arguments.push_str(&fragment.arguments);
    }

    /// The calls in index order. A call with no id gets `call_{index}`; a call
    /// with no name is dropped.
    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter_map(|(index, p)| {
                Some(ToolCall {
                    id: p.id.unwrap_or_else(|| format!("call_{index}")),
                    name: p.name?,
                    arguments: p.arguments,
                })
            })
            .collect()
    }
}

/// Read a call's arguments as a JSON object. Valid text is taken as it is;
/// otherwise one repair attempt: empty means `{}`, strip a Markdown fence,
/// remove trailing commas, add one missing closing brace.
pub fn parse_arguments(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return into_object(v);
    }
    let repaired = remove_trailing_commas(strip_fence(raw.trim()));
    match serde_json::from_str::<Value>(&repaired) {
        Ok(v) => into_object(v),
        Err(first) => match serde_json::from_str::<Value>(&format!("{repaired}}}")) {
            Ok(v) => into_object(v),
            Err(_) => Err(format!("the arguments are not valid JSON ({first})")),
        },
    }
}

fn into_object(v: Value) -> Result<Value, String> {
    if v.is_object() {
        Ok(v)
    } else {
        Err("the arguments must be a JSON object".to_string())
    }
}

fn strip_fence(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let body = rest.split_once('\n').map_or("", |(_, b)| b);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}

/// Drop a comma that is followed only by whitespace and a closing bracket,
/// leaving anything inside a string alone.
fn remove_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut in_string, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn part(index: u32, id: Option<&str>, name: Option<&str>, args: &str) -> Fragment {
        Fragment {
            index,
            id: id.map(str::to_string),
            name: name.map(str::to_string),
            arguments: args.to_string(),
        }
    }

    #[test]
    fn fragments_join_in_order() {
        let mut c = Collector::default();
        c.push(part(0, Some("c1"), Some("read_file"), "{\"pa"));
        c.push(part(0, None, None, "th\":1}"));
        let calls = c.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, "{\"path\":1}");
        assert_eq!(calls[0].id, "c1");
    }

    #[test]
    fn repeated_name_is_not_appended_twice() {
        let mut c = Collector::default();
        c.push(part(0, Some("c1"), Some("list_dir"), "{"));
        c.push(part(0, None, Some("list_dir"), "}"));
        assert_eq!(c.finish()[0].name, "list_dir");
    }

    #[test]
    fn ordered_by_index_missing_id_and_name() {
        let mut c = Collector::default();
        c.push(part(2, None, Some("b"), "{}"));
        c.push(part(0, None, Some("a"), "{}"));
        c.push(part(1, Some("x"), None, "{}"));
        let calls = c.finish();
        let ids: Vec<_> = calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["call_0", "call_2"]);
        assert_eq!(calls[0].name, "a");
    }

    #[test]
    fn parse_valid() {
        assert_eq!(parse_arguments("{\"a\":1}").unwrap(), json!({"a": 1}));
    }

    #[test]
    fn parse_empty_is_empty_object() {
        assert_eq!(parse_arguments("").unwrap(), json!({}));
        assert_eq!(parse_arguments("  \n").unwrap(), json!({}));
    }

    #[test]
    fn parse_fenced() {
        assert_eq!(
            parse_arguments("```json\n{\"a\": 1}\n```").unwrap(),
            json!({"a": 1})
        );
    }

    #[test]
    fn parse_trailing_comma() {
        assert_eq!(
            parse_arguments("{\"a\": [1, 2,], \"b\": \"x,}\",}").unwrap(),
            json!({"a": [1, 2], "b": "x,}"})
        );
    }

    #[test]
    fn parse_missing_brace() {
        assert_eq!(parse_arguments("{\"a\": 1").unwrap(), json!({"a": 1}));
    }

    #[test]
    fn parse_array_or_garbage_is_err() {
        let e = parse_arguments("[1, 2]").unwrap_err();
        assert!(!e.contains('\n') && !e.is_empty());
        let e = parse_arguments("not json at all").unwrap_err();
        assert!(!e.contains('\n') && !e.is_empty());
        assert!(parse_arguments("{\"a\": ").is_err());
    }
}
