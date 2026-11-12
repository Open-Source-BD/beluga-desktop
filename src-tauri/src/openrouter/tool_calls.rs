//! Collect the pieces of a streamed tool call, and read their arguments leniently.

use crate::agent::ToolCall;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// One streamed piece of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub index: u32,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

/// Most calls kept from one reply.
const MAX_CALLS: usize = 32;
/// Most bytes of arguments kept from one reply, all calls together.
const MAX_ARGUMENT_BYTES: usize = 1_000_000;

struct Partial {
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Joins fragments into whole calls.
///
/// Limits: a 33rd call is refused (and `overflowed()` turns true) while calls
/// already collected keep receiving their fragments and come back whole. If
/// the byte limit trips, any call may be missing its tail, so `finish()`
/// returns no calls at all and `overflowed()` is true. Fragments of one call share an index;
/// a fragment with a different id or name than the call so far starts a new
/// call, so providers that send every call at index 0 still work.
#[derive(Default)]
pub struct Collector {
    /// In the order the calls first appeared.
    calls: Vec<Partial>,
    /// The call each index is currently adding to.
    current: HashMap<u32, usize>,
    bytes: usize,
    overflowed: bool,
    byte_limit_hit: bool,
}

impl Collector {
    pub fn push(&mut self, fragment: Fragment) {
        if self.byte_limit_hit {
            return;
        }
        let id = fragment.id.filter(|s| !s.is_empty());
        let name = fragment.name.filter(|s| !s.is_empty());
        let existing = self.current.get(&fragment.index).copied().filter(|&at| {
            let c = &self.calls[at];
            let differs = |mine: &Option<String>, theirs: &Option<String>| {
                matches!((mine, theirs), (Some(m), Some(t)) if m != t)
            };
            !(differs(&c.id, &id) || differs(&c.name, &name))
        });
        let at = match existing {
            Some(at) => at,
            None => {
                if self.calls.len() >= MAX_CALLS {
                    self.overflowed = true;
                    return;
                }
                self.calls.push(Partial {
                    index: fragment.index,
                    id: None,
                    name: None,
                    arguments: String::new(),
                });
                self.current.insert(fragment.index, self.calls.len() - 1);
                self.calls.len() - 1
            }
        };
        let slot = &mut self.calls[at];
        if slot.id.is_none() {
            slot.id = id;
        }
        if slot.name.is_none() {
            slot.name = name;
        }
        if self.bytes + fragment.arguments.len() > MAX_ARGUMENT_BYTES {
            self.byte_limit_hit = true;
            self.overflowed = true;
            return;
        }
        self.bytes += fragment.arguments.len();
        slot.arguments.push_str(&fragment.arguments);
    }

    /// True when a limit cut the reply short: the calls from `finish` are
    /// then only those collected so far.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The calls in the order they appeared, with ids unique within the
    /// reply. A call with no id gets `call_{index}`; a call with no name is
    /// dropped. After a byte-limit trip nothing is returned.
    pub fn finish(self) -> Vec<ToolCall> {
        if self.byte_limit_hit {
            return Vec::new();
        }
        let kept: Vec<Partial> = self
            .calls
            .into_iter()
            .filter(|p| p.name.is_some())
            .collect();
        let taken: HashSet<String> = kept.iter().filter_map(|p| p.id.clone()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for p in kept {
            let synthesised = p.id.is_none();
            let wanted = p.id.unwrap_or_else(|| format!("call_{}", p.index));
            let mut id = wanted.clone();
            let mut n = 2;
            // A suffixed or synthesised id must avoid every real id too.
            while seen.contains(&id) || ((synthesised || id != wanted) && taken.contains(&id)) {
                id = format!("{wanted}_{n}");
                n += 1;
            }
            seen.insert(id.clone());
            out.push(ToolCall {
                id,
                name: p.name.unwrap_or_default(),
                arguments: p.arguments,
            });
        }
        out
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
        Err(first) => {
            // A missing brace is only added after a finished value: text cut
            // in the middle of a number could otherwise become another number.
            let t = repaired.trim_end();
            let complete = ["\"", "}", "]", "true", "false", "null"]
                .iter()
                .any(|end| t.ends_with(end));
            match complete.then(|| serde_json::from_str::<Value>(&format!("{t}}}"))) {
                Some(Ok(v)) => into_object(v),
                _ => Err(format!("the arguments are not valid JSON ({first})")),
            }
        }
    }
}

fn into_object(v: Value) -> Result<Value, String> {
    if v.is_null() {
        Ok(Value::Object(Default::default()))
    } else if v.is_object() {
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
    fn calls_come_out_in_first_appearance_order_with_synthesised_ids_and_nameless_dropped() {
        let mut c = Collector::default();
        c.push(part(2, None, Some("b"), "{}"));
        c.push(part(0, None, Some("a"), "{}"));
        c.push(part(1, Some("x"), None, "{}"));
        let calls = c.finish();
        let ids: Vec<_> = calls.iter().map(|c| c.id.as_str()).collect();
        // Calls come out in the order they first appeared.
        assert_eq!(ids, ["call_2", "call_0"]);
        assert_eq!(calls[1].name, "a");
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
        assert_eq!(parse_arguments("{\"a\": \"b\"").unwrap(), json!({"a": "b"}));
    }

    #[test]
    fn two_calls_at_index_0_with_different_ids_stay_two() {
        let mut c = Collector::default();
        c.push(part(0, Some("a"), Some("read_file"), "{\"a\":"));
        c.push(part(0, None, None, "1}"));
        c.push(part(0, Some("b"), Some("read_file"), "{\"b\":2}"));
        let calls = c.finish();
        let got: Vec<_> = calls
            .iter()
            .map(|c| (c.id.as_str(), c.arguments.as_str()))
            .collect();
        assert_eq!(got, [("a", "{\"a\":1}"), ("b", "{\"b\":2}")]);
    }

    #[test]
    fn two_calls_at_index_0_without_ids_but_different_names_stay_two() {
        let mut c = Collector::default();
        c.push(part(0, None, Some("list_dir"), "{}"));
        c.push(part(0, None, Some("read_file"), "{\"path\":\"a\"}"));
        let calls = c.finish();
        let got: Vec<_> = calls
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(got, [("call_0", "list_dir"), ("call_0_2", "read_file")]);
    }

    #[test]
    fn repeated_ids_get_a_suffix() {
        let mut c = Collector::default();
        c.push(part(0, Some("x"), Some("a"), "{}"));
        c.push(part(1, Some("x"), Some("b"), "{}"));
        c.push(part(2, Some("x"), Some("c"), "{}"));
        let ids: Vec<_> = c.finish().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["x", "x_2", "x_3"]);
    }

    #[test]
    fn a_synthesised_id_never_collides_with_a_real_one() {
        let mut c = Collector::default();
        c.push(part(0, None, Some("a"), "{}"));
        c.push(part(1, Some("call_0"), Some("b"), "{}"));
        let calls = c.finish();
        assert_eq!(calls[1].id, "call_0");
        assert_ne!(calls[0].id, "call_0");
        assert_ne!(calls[0].id, calls[1].id);
    }

    #[test]
    fn more_than_32_calls_overflow_and_keep_the_first_32() {
        let mut c = Collector::default();
        for i in 0..33 {
            c.push(part(i, Some(&format!("c{i}")), Some("n"), "{}"));
        }
        assert!(c.overflowed());
        let calls = c.finish();
        assert_eq!(calls.len(), 32);
        assert_eq!(calls[31].id, "c31");
    }

    #[test]
    fn the_byte_limit_returns_no_calls_and_reports_overflow() {
        let mut c = Collector::default();
        c.push(part(0, Some("a"), Some("n"), "{\"a\":\"b\""));
        let big = "x".repeat(600_000);
        c.push(part(1, Some("big"), Some("n"), &big));
        assert!(!c.overflowed());
        c.push(part(1, None, None, &big));
        assert!(c.overflowed());
        // Call a is still open and loses its tail: it must not come back cut.
        c.push(part(0, None, None, ",\"c\":\"d\"}"));
        c.push(part(2, Some("late"), Some("n"), "{}"));
        assert!(c.finish().is_empty());
    }

    #[test]
    fn a_33rd_call_is_refused_while_the_32nd_still_completes() {
        let mut c = Collector::default();
        for i in 0..31 {
            c.push(part(i, Some(&format!("c{i}")), Some("n"), "{}"));
        }
        c.push(part(31, Some("c31"), Some("n"), "{\"a\":"));
        c.push(part(32, Some("c32"), Some("n"), "{}"));
        assert!(c.overflowed());
        c.push(part(31, None, None, "\"b\"}"));
        let calls = c.finish();
        assert_eq!(calls.len(), 32);
        assert_eq!(calls[31].id, "c31");
        assert_eq!(calls[31].arguments, "{\"a\":\"b\"}");
    }

    #[test]
    fn missing_brace_repairs_only_after_a_complete_value() {
        assert!(parse_arguments("{\"limit\": 40").is_err());
        assert_eq!(parse_arguments("{\"a\":\"b\"").unwrap(), json!({"a": "b"}));
        assert_eq!(
            parse_arguments("{\"a\":[1,2]").unwrap(),
            json!({"a": [1, 2]})
        );
        assert_eq!(parse_arguments("{\"a\":true").unwrap(), json!({"a": true}));
        assert_eq!(parse_arguments("null").unwrap(), json!({}));
        assert_eq!(parse_arguments("```json\nnull\n```").unwrap(), json!({}));
    }

    #[test]
    fn non_object_arguments_are_errors() {
        assert!(parse_arguments("[1]").is_err());
        assert!(parse_arguments("5").is_err());
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
