//! Skills: SKILL.md parsing and the acts-on-your-computer text scan.

pub mod pack;
pub mod parse;
pub mod scan;

use serde::{Deserialize, Serialize};

pub use parse::parse_skill;
pub use scan::{MAX_BODY_CHARS, acts_on_computer};

/// What the lenient reader gets out of one SKILL.md file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub license: Option<String>,
}

/// One skill as the library lists it. `id` is `"{pack}:{name}"`, or `name`
/// alone when `pack == "yours"`. `dir` is relative to the pack root; `files`
/// are relative to `dir` and exclude `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillEntry {
    pub id: String,
    pub pack: String,
    pub name: String,
    pub description: String,
    pub dir: String,
    pub acts_on_computer: bool,
    pub body_chars: u32,
    pub files: Vec<String>,
    pub license: Option<String>,
    pub broken: Option<String>,
}
