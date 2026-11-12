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
];

/// Arguments that hand control back to something else even when the rule
/// matches. Each entry: why, the programs it applies to (`*` is every program),
/// and the argument prefixes. `--long` prefixes match case-insensitively.
type ArgGroup = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

const RISKY_ARGS: &[ArgGroup] = &[
    (
        "git: options that run a program or read configuration from elsewhere",
        &["git"],
        &[
            "--upload-pack",
            "--receive-pack",
            "--exec",
            "--config-env",
            "-c",
            "--output",
            "-o",
            "--ext-diff",
            "--textconv",
            "-O",
            "--open-files-in-pager",
            "--git-dir",
            "--work-tree",
        ],
    ),
    (
        "cargo: extra configuration and unstable flags",
        &["cargo"],
        &["--config", "-Z"],
    ),
    (
        "npm, pnpm, yarn: another shell, another project, another config, another registry, global installs",
        &["npm", "pnpm", "yarn"],
        &[
            "--script-shell",
            "--prefix",
            "--userconfig",
            "--globalconfig",
            "--registry",
            "--global",
            "-g",
            "--dir",
            "-C",
            "--cwd",
        ],
    ),
    (
        "go: flags that run another program",
        &["go"],
        &[
            "-exec",
            "-toolexec",
            "-vettool",
            "-C",
            "-overlay",
            "-modfile",
        ],
    ),
    (
        "tools that run or write through an option",
        &["rg", "fd", "fdfind", "sort"],
        &[
            "--pre",
            "--hostname-bin",
            "-x",
            "-X",
            "--exec",
            "--exec-batch",
            "--compress-program",
            "-o",
            "--output",
        ],
    ),
];

/// Programs whose first argument is a sub-command. For these, options before
/// the sub-command (`git -c x status`) leave the rule `None`.
const SUBCOMMAND_PROGRAMS: &[&str] = &[
    "git", "npm", "pnpm", "yarn", "cargo", "go", "dotnet", "rustup", "docker", "podman", "kubectl",
    "bun", "deno",
];

/// All the lists in one place.
pub struct Never {
    pub programs: &'static [Group],
    pub rules: &'static [Group],
    pub args: &'static [ArgGroup],
}

pub const NEVER: Never = Never {
    programs: NEVER_PROGRAMS,
    rules: NEVER_RULES,
    args: RISKY_ARGS,
};

fn listed(groups: &[Group], name: &str) -> bool {
    groups.iter().any(|(_, items)| items.contains(&name))
}

/// `name` must already be lower-case.
pub fn is_never_program(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("python") || name.starts_with("pypy") || listed(NEVER_PROGRAMS, &name)
}

pub fn is_never_rule(rule: &str) -> bool {
    listed(NEVER_RULES, &rule.to_ascii_lowercase())
}

fn risky_arg(program: &str, arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    RISKY_ARGS.iter().any(|(_, programs, prefixes)| {
        (programs.contains(&"*") || programs.contains(&program))
            && prefixes.iter().any(|p| {
                if p.starts_with("--") {
                    lower.starts_with(p)
                } else {
                    arg.starts_with(p)
                }
            })
    })
}

// ---------------------------------------------------------------------------
// Reading a command
// ---------------------------------------------------------------------------

pub fn inspect(command: &str, family: ShellFamily, project_root: &Path) -> Inspected {
    Inspected {
        rule: rule_for(command, family),
        outside: refers_outside(command, project_root),
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
/// shell or the quoting is not simple and balanced.
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

/// The program's lower-cased file name without `.exe`/`.cmd`/`.bat`, or `None`
/// when it is quoted, has a path separator, or is not a plain name.
fn program_name(tok: &Tok) -> Option<String> {
    if tok.quoted || !tok.text.is_ascii() {
        return None;
    }
    let mut name = tok.text.to_ascii_lowercase();
    loop {
        let stripped = [".exe", ".cmd", ".bat"]
            .iter()
            .find_map(|ext| name.strip_suffix(ext));
        match stripped {
            Some(s) => name = s.to_string(),
            None => break,
        }
    }
    let first = name.chars().next()?;
    let ok = first.is_ascii_alphanumeric()
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.+".contains(c));
    ok.then_some(name)
}

fn plain_word(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
}

fn rule_for(command: &str, family: ShellFamily) -> Option<String> {
    let toks = lex(command, family)?;
    let program = program_name(&toks[0])?;
    if is_never_program(&program) {
        return None;
    }
    let args = &toks[1..];
    // Look-alike letters in the position that names the rule.
    if args.first().is_some_and(|a| !a.text.is_ascii()) {
        return None;
    }
    if args.iter().any(|a| risky_arg(&program, &a.text)) {
        return None;
    }
    let plain_first = args
        .first()
        .filter(|a| !a.quoted && plain_word(&a.text))
        .map(|a| a.text.as_str());
    let key = if SUBCOMMAND_PROGRAMS.contains(&program.as_str()) {
        match (args.first(), plain_first) {
            (None, _) => program,
            (Some(_), Some(sub)) => format!("{program} {sub}"),
            // `git -c x status`, `cargo +nightly test`, `git 'status'`
            (Some(_), None) => return None,
        }
    } else {
        match plain_first {
            Some(first) => format!("{program} {first}"),
            None => program,
        }
    };
    if is_never_rule(&key) {
        return None;
    }
    Some(key)
}

// ---------------------------------------------------------------------------
// Outside the project (a hint; false positives are fine)
// ---------------------------------------------------------------------------

/// Words as the shell might see them, split generously so that
/// `>/etc/x`, `--out=/etc/x` and `-C/etc` all expose their path.
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
        for piece in w.split(['=', ',']) {
            if piece.is_empty() {
                continue;
            }
            out.push(piece.to_string());
            if piece.starts_with('-') && !piece.starts_with("--") && piece.len() > 2 {
                out.push(piece[2..].to_string());
            }
        }
        out.push(w);
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

    /// (command, expected rule, expected outside)
    type Row = (&'static str, Option<&'static str>, bool);

    fn root() -> &'static Path {
        Path::new("/home/u/proj")
    }

    fn table(family: ShellFamily, rows: &[Row]) {
        for (cmd, rule, outside) in rows {
            let got = inspect(cmd, family, root());
            assert_eq!(got.rule.as_deref(), *rule, "rule for {cmd:?} ({family:?})");
            assert_eq!(got.outside, *outside, "outside for {cmd:?} ({family:?})");
        }
    }

    fn rule_of(cmd: &str, family: ShellFamily) -> Option<String> {
        inspect(cmd, family, root()).rule
    }

    #[test]
    fn posix_plain_commands_get_a_rule() {
        table(
            P,
            &[
                ("npm install", Some("npm install"), false),
                ("cargo test --all", Some("cargo test"), false),
                ("ls -la", Some("ls"), false),
                ("pytest", Some("pytest"), false),
                ("pytest tests/", Some("pytest"), false),
                ("ls src", Some("ls src"), false),
                ("npm", Some("npm"), false),
                ("git status", Some("git status"), false),
                ("NPM.CMD install", Some("npm install"), false),
                ("npm.exe install", Some("npm install"), false),
                ("npm install @scope/pkg", Some("npm install"), false),
                ("ls --color=auto", Some("ls"), false),
                ("cargo test", Some("cargo test"), false),
                ("go test ./...", Some("go test"), false),
                ("npm test", Some("npm test"), false),
                ("npm ci", Some("npm ci"), false),
                ("git Status", Some("git Status"), false),
                ("echo 'a\\b'", Some("echo"), false),
                ("ls -la ünï", Some("ls"), false),
                ("npm install 'a;b'", Some("npm install"), false),
                ("npm   install", Some("npm install"), false),
            ],
        );
    }

    #[test]
    fn posix_separators_and_substitutions_give_no_rule() {
        let cmds = [
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
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
        }
    }

    #[test]
    fn posix_shell_special_forms_give_no_rule() {
        let cmds = [
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
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
        }
    }

    #[test]
    fn never_programs_give_no_rule() {
        for (_, items) in NEVER.programs {
            for p in *items {
                for fam in [P, W] {
                    for cmd in [format!("{p} x"), p.to_uppercase(), format!("{p}.exe y")] {
                        assert_eq!(rule_of(&cmd, fam), None, "{cmd:?} ({fam:?})");
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
        // The extended lists named in the review message.
        let extra = "make just task ant gradle gradlew mvn docker podman kubectl ssh scp rsync curl wget nc ncat telnet ftp osascript open xdg-open start explorer nohup time timeout watch find awk sed tee tar unzip zip pip pip3 pipx uv conda brew apt apt-get yum dnf pacman choco winget scoop launchctl systemctl service crontab kill killall pkill shutdown reboot diskutil mount umount ln cp touch truncate";
        for p in extra.split(' ') {
            assert!(is_never_program(p), "{p}");
        }
        let rules = "git commit|git add|git rm|git mv|git merge|git rebase|git pull|git fetch|git clone|git config|git stash|git tag|git branch|git switch|git apply|git am|git cherry-pick|git revert|git submodule|git worktree|git gc|git filter-branch|git update-ref|git remote|npm exec|npm x|npm run|npm run-script|npm start|npm publish|npm config|npm set|pnpm exec|pnpm dlx|pnpm run|yarn exec|yarn dlx|yarn run|bun x|bun run|cargo run|cargo install|cargo publish|go run|go install|go generate|dotnet run";
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
    fn never_rules_give_no_rule() {
        for (_, items) in NEVER.rules {
            for r in *items {
                for fam in [P, W] {
                    for cmd in [r.to_string(), format!("{r} x"), r.to_uppercase()] {
                        assert_eq!(rule_of(&cmd, fam), None, "{cmd:?} ({fam:?})");
                    }
                }
            }
        }
        let cmds = [
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
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
        }
    }

    #[test]
    fn paths_and_look_alike_programs_give_no_rule() {
        let cmds = [
            "/bin/rm x",
            "./node_modules/.bin/tsc",
            "bin/tool",
            "..\\x",
            "ls/x",
            "python3.12 x",
            "PYTHON x",
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
        }
    }

    #[test]
    fn options_before_the_subcommand_give_no_rule() {
        let cmds = [
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
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
            assert_eq!(rule_of(c, W), None, "{c:?} (powershell)");
        }
    }

    #[test]
    fn risky_arguments_remove_the_rule_even_when_it_would_match() {
        let cmds = [
            "git status --upload-pack=x",
            "git status --upload-pack x",
            "git status --receive-pack=x",
            "git log --exec=x",
            "git log --exec x",
            "git log -c x=y",
            "git log -cfoo",
            "git log --config-env=a=B",
            "git log --output=/tmp/x",
            "git log --output /tmp/x",
            "git diff -o x",
            "git diff -o/tmp/x",
            "git diff --ext-diff",
            "git diff --textconv",
            "git status --git-dir=/x",
            "cargo build --config x=y",
            "cargo build --config=x=y",
            "cargo build -Zunstable-options",
            "npm install --script-shell=/bin/sh",
            "npm install --script-shell /bin/sh",
            "npm install --prefix /x",
            "npm install --prefix=/x",
            "npm install --userconfig x",
            "npm install --globalconfig x",
            "pnpm install --prefix x",
            "pnpm install --dir /x",
            "yarn install --userconfig x",
            "yarn install --cwd /x",
            "npm install --registry http://x",
            "npm install --global x",
            "npm install -g x",
            "rg foo --pre=x",
            "rg foo --pre x",
            "rg foo --hostname-bin=x",
            "fd . -x ls",
            "fd . --exec ls",
            "sort --compress-program=x f",
            "sqlite3 db .shell",
        ];
        for c in cmds {
            assert_eq!(rule_of(c, P), None, "{c:?}");
        }
        // Not over-reaching: response files and `=` options are fine.
        table(
            P,
            &[
                ("cargo build --release", Some("cargo build"), false),
                ("npm install @scope/pkg", Some("npm install"), false),
                ("cargo test -- --nocapture", Some("cargo test"), false),
                ("ls --sort=size", Some("ls"), false),
                ("rg foo src", Some("rg foo"), false),
            ],
        );
    }

    #[test]
    fn subcommands_are_compared_exactly_and_never_check_ignores_case() {
        assert_eq!(rule_of("git Status", P).as_deref(), Some("git Status"));
        assert_eq!(rule_of("GIT status", P).as_deref(), Some("git status"));
        assert_eq!(rule_of("git PUSH", P), None);
        assert_eq!(rule_of("Git Reset --hard", P), None);
        assert_eq!(rule_of("NPM RUN x", P), None);
        let saved = vec!["git status".to_string()];
        assert!(!allowed("git Status", P, root(), &saved));
        assert!(allowed("GIT status", P, root(), &saved));
    }

    #[test]
    fn powershell_plain_commands_get_a_rule() {
        table(
            W,
            &[
                ("npm install", Some("npm install"), false),
                ("Get-ChildItem", Some("get-childitem"), false),
                ("git status", Some("git status"), false),
                ("echo 'abc'", Some("echo"), false),
                ("echo \"abc\"", Some("echo"), false),
                ("ls -la", Some("ls"), false),
                ("npm.cmd install", Some("npm install"), false),
                ("cargo test --all", Some("cargo test"), false),
                ("ls src\\..\\a", Some("ls"), false),
            ],
        );
    }

    #[test]
    fn powershell_special_forms_give_no_rule() {
        let cmds = [
            "Remove-Item x",
            "remove-item x",
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
        ];
        for c in cmds {
            assert_eq!(rule_of(c, W), None, "{c:?} (powershell)");
        }
    }

    #[test]
    fn extended_never_programs_and_arguments_apply_to_both_shells() {
        for c in [
            "make all",
            "docker run x",
            "curl http://x",
            "find . -exec x",
        ] {
            assert_eq!(rule_of(c, P), None, "{c:?}");
            assert_eq!(rule_of(c, W), None, "{c:?}");
        }
    }

    #[test]
    fn allowed_matches_only_an_exact_saved_rule() {
        let saved = vec!["npm install".to_string(), "ls".to_string()];
        for (cmd, want) in [
            ("npm install", true),
            ("npm install --save-dev x", true),
            ("NPM.CMD install", true),
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
            ("ls src", false),
            ("PATH=. npm install", false),
            ("rm x", false),
        ] {
            assert_eq!(allowed(cmd, P, root(), &saved), want, "{cmd:?}");
        }
        assert!(allowed("npm install", W, root(), &saved));
        assert!(!allowed("npm install | iex", W, root(), &saved));
        assert!(!allowed("npm install", P, root(), &[]));
        assert!(!allowed("rm x", P, root(), &["rm".to_string()]));
        assert!(!allowed("git push", P, root(), &["git push".to_string()]));
    }

    #[test]
    fn outside_detects_places_beyond_the_project() {
        table(
            P,
            &[
                ("cat /etc/hosts", Some("cat"), true),
                ("cat ~/x", None, true),
                ("ls ../../x", Some("ls"), true),
                ("ls src/../a", Some("ls"), false),
                ("cat /home/u/proj/a.txt", Some("cat"), false),
                ("cat /home/u/proj", Some("cat"), false),
                ("cat /home/u/proj/../x", Some("cat"), true),
                ("cat /home/u/projx/a", Some("cat"), true),
                ("cat /dev/null", Some("cat"), false),
                ("cat /dev/null/../x", Some("cat"), true),
                ("cat /dev/sda", Some("cat"), true),
                ("cat /dev/tty", Some("cat"), true),
                ("cat 'C:\\Users\\x'", Some("cat"), true),
                ("cat C:\\x", None, true),
                ("cat '\\\\server\\share\\x'", Some("cat"), true),
                ("cat 'C:/Users/x'", Some("cat"), true),
                ("echo $HOME/x", None, true),
                ("echo ${HOME}/x", None, true),
                ("cat '%USERPROFILE%/x'", Some("cat"), true),
                ("ls --dir=/etc", Some("ls"), true),
                ("ls -C/etc", Some("ls"), true),
                ("ls ..", Some("ls .."), true),
                ("ls .", Some("ls ."), false),
                ("ls a/../..", Some("ls"), true),
                ("ls ..\\..\\x", None, true),
                ("ls a > /etc/x", None, true),
                ("ls a >/etc/x", None, true),
                ("ls 'a b/../c'", Some("ls"), false),
                ("ls src", Some("ls src"), false),
            ],
        );
    }

    #[test]
    fn outside_for_powershell() {
        table(
            W,
            &[
                ("type C:\\x\\y", Some("type"), true),
                ("cd ..\\..", Some("cd"), true),
                ("type \\\\server\\share\\x", Some("type"), true),
                ("ls $env:USERPROFILE", None, true),
                ("ls $HOME\\x", None, true),
                ("ls %USERPROFILE%", None, true),
                ("ls %APPDATA%\\x", None, true),
                ("ls ~\\x", None, true),
                ("ls src\\..\\a", Some("ls"), false),
                ("type /etc/hosts", Some("type"), true),
                ("type /dev/null", Some("type"), false),
            ],
        );
    }
}
