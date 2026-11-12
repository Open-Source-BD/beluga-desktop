//! `update_plan`: the task's checklist, shown to the user.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ToolOutcome;
use super::read::{fail, ok};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub text: String,
    pub state: PlanState,
}

const MAX_ITEMS: usize = 30;
const MAX_TEXT_CHARS: usize = 200;
const SUMMARY: &str = "Updated the plan";

fn state_of(name: &str) -> Option<PlanState> {
    match name {
        "pending" => Some(PlanState::Pending),
        "in_progress" => Some(PlanState::InProgress),
        "done" => Some(PlanState::Done),
        _ => None,
    }
}

fn parse(args: &Value) -> Result<Vec<PlanItem>, String> {
    let list = match args.get("items") {
        Some(Value::Array(a)) => a,
        Some(_) => return Err("The argument \"items\" must be a list.".to_string()),
        None => return Err("The argument \"items\" is missing.".to_string()),
    };
    if list.len() > MAX_ITEMS {
        return Err(format!(
            "The argument \"items\" has {} entries; at most {MAX_ITEMS} are allowed.",
            list.len()
        ));
    }
    let mut items = Vec::with_capacity(list.len());
    for (i, raw) in list.iter().enumerate() {
        let n = i + 1;
        let text = match raw.get("text") {
            Some(Value::String(t)) if !t.trim().is_empty() => t.trim(),
            _ => {
                return Err(format!(
                    "Item {n} of \"items\" needs a \"text\" that is not empty."
                ));
            }
        };
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!(
                "Item {n} of \"items\" has a \"text\" longer than {MAX_TEXT_CHARS} characters."
            ));
        }
        let state = raw
            .get("state")
            .and_then(Value::as_str)
            .and_then(state_of)
            .ok_or_else(|| {
                format!("Item {n} of \"items\" needs a \"state\" of pending, in_progress or done.")
            })?;
        items.push(PlanItem {
            text: text.to_string(),
            state,
        });
    }
    Ok(items)
}

/// Replaces the task's checklist. On an error the list is empty and the
/// caller keeps the plan it had.
pub fn update_plan(args: &Value) -> (ToolOutcome, Vec<PlanItem>) {
    match parse(args) {
        Ok(items) => {
            let text = format!("The plan now has {} items.", items.len());
            (ok(text, SUMMARY.to_string()), items)
        }
        Err(e) => (fail(e, SUMMARY.to_string()), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ToolStatus;
    use serde_json::json;

    #[test]
    fn a_valid_plan_is_returned_in_order() {
        let (o, items) = update_plan(&json!({"items": [
            {"text": "Read the code", "state": "done"},
            {"text": "Fix it", "state": "in_progress"},
            {"text": "Test it", "state": "pending"},
        ]}));
        assert_eq!(o.status, ToolStatus::Ok);
        assert_eq!(o.summary, "Updated the plan");
        assert_eq!(o.detail, None);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].state, PlanState::InProgress);
        assert_eq!(items[0].text, "Read the code");
        assert_eq!(
            serde_json::to_value(&items[1]).unwrap()["state"],
            "in_progress"
        );
    }

    #[test]
    fn an_unknown_state_31_items_or_a_201_character_text_is_an_error() {
        let (o, items) = update_plan(&json!({"items": [{"text": "x", "state": "started"}]}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("state"), "{}", o.text);
        assert_eq!(o.summary, "Updated the plan");
        assert!(items.is_empty());

        let many: Vec<_> = (0..31)
            .map(|_| json!({"text": "x", "state": "pending"}))
            .collect();
        let (o, _) = update_plan(&json!({"items": many}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("30"), "{}", o.text);

        let long = "é".repeat(201);
        let (o, _) = update_plan(&json!({"items": [{"text": long, "state": "pending"}]}));
        assert_eq!(o.status, ToolStatus::Error);
        assert!(o.text.contains("200"), "{}", o.text);

        let ok = "é".repeat(200);
        let (o, _) = update_plan(&json!({"items": [{"text": ok, "state": "pending"}]}));
        assert_eq!(o.status, ToolStatus::Ok);
    }

    #[test]
    fn missing_or_wrong_typed_items_name_the_argument() {
        for args in [
            json!({}),
            json!({"items": "x"}),
            json!({"items": [1]}),
            json!({"items": [{"state": "done"}]}),
            json!({"items": [{"text": "a"}]}),
            json!({"items": [{"text": "", "state": "done"}]}),
        ] {
            let (o, items) = update_plan(&args);
            assert_eq!(o.status, ToolStatus::Error, "{args}");
            assert!(o.text.contains("items"), "{}", o.text);
            assert!(items.is_empty());
        }
    }
}
