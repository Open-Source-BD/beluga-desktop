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

pub mod paths;
pub mod secrets;
