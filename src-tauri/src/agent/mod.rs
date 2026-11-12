//! Project work: the model's tool calls and what comes back from them.

use serde::{Deserialize, Serialize};

/// One call the model made. `arguments` is the raw JSON text it sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Error,
    Declined,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub status: ToolStatus,
    /// What the model receives.
    pub text: String,
    /// One trail line, e.g. "Read src/app.ts".
    pub summary: String,
    /// Change or output shown when the line opens.
    pub detail: Option<String>,
}

/// An approval card's contents. The core builds every field; the interface
/// only shows them. `id` is set by `Approvals::ask`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Approval {
    Command {
        id: String,
        /// The complete command, exactly as it will run.
        command: String,
        /// The project folder it runs in.
        folder: String,
        /// The rule "Always allow" would save; `None` offers no rule.
        rule: Option<String>,
        /// The text refers to a place outside the project (a hint).
        outside: bool,
        /// No command has been allowed in this folder before.
        first_in_project: bool,
        background: bool,
    },
    Edit {
        id: String,
        /// The real path relative to the project folder.
        path: String,
        created: bool,
        removed: Vec<String>,
        added: Vec<String>,
        /// The true counts behind the (possibly cut) lists.
        removed_total: u32,
        added_total: u32,
        total_lines: u32,
    },
    Secret {
        id: String,
        path: String,
    },
    KeepGoing {
        id: String,
        requests: u32,
        free_left: Option<u32>,
        spent_today_usd: Option<f64>,
    },
}

impl Approval {
    pub fn id(&self) -> &str {
        match self {
            Approval::Command { id, .. }
            | Approval::Edit { id, .. }
            | Approval::Secret { id, .. }
            | Approval::KeepGoing { id, .. } => id,
        }
    }

    pub(crate) fn set_id(&mut self, new: String) {
        match self {
            Approval::Command { id, .. }
            | Approval::Edit { id, .. }
            | Approval::Secret { id, .. }
            | Approval::KeepGoing { id, .. } => *id = new,
        }
    }
}

/// The user's answer to an approval card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Answer {
    Once,
    Always,
    AllEdits,
    Decline { note: Option<String> },
    Message { text: String },
    KeepGoing,
    StopHere,
}

pub mod approval;
pub mod background;
pub mod change;
pub mod dispatch;
pub mod paths;
pub mod plan;
pub mod read;
pub mod rules;
pub mod run;
pub mod schema;
pub mod search;
pub mod secrets;
pub mod shell;

#[cfg(test)]
pub(crate) mod test_support;
