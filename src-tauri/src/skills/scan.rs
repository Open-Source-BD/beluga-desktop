//! The acts-on-your-computer text scan.

/// Longest body (in characters) the library will load for one skill.
pub const MAX_BODY_CHARS: usize = 60_000;

const SHELL_TAGS: [&str; 6] = ["bash", "sh", "shell", "zsh", "powershell", "console"];
const TOOL_NAMES: [&str; 7] = [
    "Bash",
    "Read",
    "Edit",
    "Write",
    "Task",
    "TodoWrite",
    "WebFetch",
];
const PHRASES: [&str; 9] = [
    "run the command",
    "run this command",
    "dispatch a subagent",
    "spawn a subagent",
    "create the file",
    "write the file",
    "edit the file",
    "git commit",
    "git worktree",
];

/// True when the skill's text suggests it makes the assistant act on the
/// user's computer: a shell code fence, a tool name as a whole word
/// (case-sensitive), or one of a few phrases (case-insensitive).
pub fn acts_on_computer(body: &str) -> bool {
    has_shell_fence(body) || has_tool_word(body) || has_phrase(body)
}

fn has_shell_fence(body: &str) -> bool {
    body.lines().any(|line| {
        let Some(rest) = line.trim_start().strip_prefix("```") else {
            return false;
        };
        let tag = rest
            .trim_start()
            .split(|c: char| c.is_whitespace() || c == '{')
            .next()
            .unwrap_or("");
        SHELL_TAGS.iter().any(|t| tag.eq_ignore_ascii_case(t))
    })
}

fn has_tool_word(body: &str) -> bool {
    body.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w.starts_with("mcp__") || TOOL_NAMES.contains(&w))
}

fn has_phrase(body: &str) -> bool {
    let lower = body.to_lowercase();
    PHRASES.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_fence_is_true() {
        assert!(acts_on_computer("Do this:\n\n```bash\nls -la\n```\n"));
        for tag in ["sh", "shell", "zsh", "powershell", "console"] {
            assert!(acts_on_computer(&format!("```{tag}\nx\n```")), "{tag}");
        }
        assert!(!acts_on_computer("```rust\nfn main() {}\n```"));
        assert!(!acts_on_computer("```bashful\nx\n```"));
    }

    #[test]
    fn plain_prose_is_false() {
        assert!(!acts_on_computer(
            "Ask the user three questions and then summarise their answers."
        ));
        assert!(!acts_on_computer(""));
    }

    #[test]
    fn tool_name_as_whole_word_is_true() {
        for t in [
            "Bash",
            "Read",
            "Edit",
            "Write",
            "Task",
            "TodoWrite",
            "WebFetch",
        ] {
            assert!(acts_on_computer(&format!("Use the {t} tool.")), "{t}");
            assert!(acts_on_computer(&format!("({t})")), "{t} in parens");
        }
        assert!(acts_on_computer("call mcp__github__search"));
        assert!(!acts_on_computer("Reading is fun and Tasks are many."));
        assert!(!acts_on_computer("Already Edited and Rewrite it."));
        // Tool names are case-sensitive.
        assert!(!acts_on_computer("read the bash manual and write a task"));
    }

    #[test]
    fn phrases_are_case_insensitive() {
        for p in [
            "Run the command now",
            "RUN THIS COMMAND",
            "then Dispatch a Subagent",
            "spawn a subagent",
            "Create the file first",
            "write the file",
            "EDIT THE FILE",
            "git commit -m x",
            "Git Worktree add",
        ] {
            assert!(acts_on_computer(p), "{p}");
        }
    }
}
