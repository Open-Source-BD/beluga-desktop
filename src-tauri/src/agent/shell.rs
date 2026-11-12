//! Which shell runs the model's commands, and how it is invoked.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;

use tokio::process::Command;

use super::rules::ShellFamily;

/// Variables removed from a command's environment. The first is the app's own
/// key; the rest make a non-interactive shell run or change code before the
/// command the user approved. Every variable starting with `BASH_FUNC_`
/// (exported bash functions, which can replace any command) is removed too.
/// Loader and language-option variables (`LD_PRELOAD`, `NODE_OPTIONS`, ...) are
/// left alone: a developer's environment sets them on purpose.
pub const SCRUBBED: &[&str] = &[
    "OPENROUTER_API_KEY",
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "PS4",
    "SHELLOPTS",
    "BASHOPTS",
    "CDPATH",
    "GLOBIGNORE",
];

/// Fixed variables that keep tools from waiting on a person, colouring their
/// output or opening a pager. They are the same for every command.
pub const FIXED_ENV: &[(&str, &str)] = &[
    ("TERM", "dumb"),
    ("NO_COLOR", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
];

/// Put in front of every PowerShell command so non-ASCII output reaches the
/// app as UTF-8 (without a byte-order mark). It is fixed and is not part of
/// what the user approved: it only sets how output is encoded.
const EXPORTED_FUNCTION_PREFIX: &str = "BASH_FUNC_";

pub const POWERSHELL_PREFIX: &str = "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    pub program: PathBuf,
    pub family: ShellFamily,
    /// The text told to the model.
    pub name: &'static str,
}

impl Shell {
    /// The command that runs `text` in this shell: no keyboard input, output
    /// on pipes, and the environment `parent_env` (the app's own, in
    /// production) minus the scrubbed names plus the fixed ones. The caller
    /// sets the working directory.
    pub fn command<I>(&self, text: &str, parent_env: I) -> Command
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let mut cmd = Command::new(&self.program);
        match self.family {
            ShellFamily::Posix => {
                cmd.arg("-c").arg(text);
            }
            ShellFamily::PowerShell => {
                cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
                    .arg(format!("{POWERSHELL_PREFIX}{text}"));
            }
        }
        cmd.env_clear();
        for (name, value) in parent_env {
            if !is_scrubbed(&name) {
                cmd.env(name, value);
            }
        }
        for (name, value) in FIXED_ENV {
            cmd.env(name, value);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
}

fn is_scrubbed(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    // Windows variable names are not case-sensitive.
    let same = |a: &str, b: &str| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    SCRUBBED.iter().any(|s| same(name, s))
        || name
            .get(..EXPORTED_FUNCTION_PREFIX.len())
            .is_some_and(|start| same(start, EXPORTED_FUNCTION_PREFIX))
}

/// The first absolute directory of `path_var` holding a file called `name`.
/// Empty and relative entries mean "the current directory" and are skipped.
#[cfg_attr(not(windows), allow(dead_code))]
fn find_in_path(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(windows))]
fn detect_uncached() -> Shell {
    if std::path::Path::new("/bin/bash").is_file() {
        Shell {
            program: PathBuf::from("/bin/bash"),
            family: ShellFamily::Posix,
            name: "bash",
        }
    } else {
        Shell {
            program: PathBuf::from("/bin/sh"),
            family: ShellFamily::Posix,
            name: "sh",
        }
    }
}

/// Windows PowerShell 5.1 under the system folder: an absolute path, never a
/// bare name that could be resolved against the current directory.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_powershell_path(system_root: Option<OsString>) -> PathBuf {
    let root = system_root
        .filter(|root| !root.is_empty())
        .unwrap_or_else(|| OsString::from("C:\\Windows"));
    PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

#[cfg(windows)]
fn detect_uncached() -> Shell {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(program) = find_in_path("pwsh.exe", &path) {
        return Shell {
            program,
            family: ShellFamily::PowerShell,
            name: "PowerShell 7 (pwsh)",
        };
    }
    Shell {
        program: find_in_path("powershell.exe", &path)
            .unwrap_or_else(|| windows_powershell_path(std::env::var_os("SystemRoot"))),
        family: ShellFamily::PowerShell,
        name: "Windows PowerShell 5.1",
    }
}

/// The shell for this machine, looked up once.
pub fn detect() -> Shell {
    static SHELL: OnceLock<Shell> = OnceLock::new();
    SHELL.get_or_init(detect_uncached).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn detect_returns_a_shell_that_exists() {
        let shell = detect();
        #[cfg(unix)]
        {
            assert_eq!(shell.family, ShellFamily::Posix);
            assert!(shell.program.is_file());
            assert!(["bash", "sh"].contains(&shell.name));
            assert!(shell.program.is_absolute());
            if std::path::Path::new("/bin/bash").is_file() {
                assert_eq!(shell.name, "bash");
            }
        }
        #[cfg(windows)]
        {
            assert_eq!(shell.family, ShellFamily::PowerShell);
            let file = shell
                .program
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_lowercase();
            assert!(file == "pwsh.exe" || file == "powershell.exe", "{file}");
            assert!(["PowerShell 7 (pwsh)", "Windows PowerShell 5.1"].contains(&shell.name));
        }
        assert_eq!(detect(), shell, "the answer is cached");
    }

    #[test]
    fn find_in_path_takes_the_first_directory_with_the_file() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("tool"), "x").unwrap();
        std::fs::create_dir(a.path().join("other")).unwrap();
        let joined = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(find_in_path("tool", &joined), Some(b.path().join("tool")));
        assert_eq!(
            find_in_path("other", &joined),
            None,
            "a directory is not a program"
        );
        assert_eq!(find_in_path("missing", &joined), None);
    }

    fn posix() -> Shell {
        Shell {
            program: PathBuf::from("/bin/sh"),
            family: ShellFamily::Posix,
            name: "sh",
        }
    }

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    fn envs_of(cmd: &Command) -> Vec<(String, Option<String>)> {
        cmd.as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn find_in_path_ignores_empty_and_relative_entries() {
        // Cargo.toml is in the working directory of the tests.
        let joined = std::env::join_paths([Path::new(""), Path::new(".")]).unwrap();
        assert_eq!(find_in_path("Cargo.toml", &joined), None);
    }

    #[test]
    fn the_fallback_powershell_is_an_absolute_path_under_the_system_root() {
        let p = windows_powershell_path(Some(OsString::from("D:\\Win")));
        assert!(p.starts_with("D:\\Win"));
        assert!(p.ends_with("System32/WindowsPowerShell/v1.0/powershell.exe"));
        let d = windows_powershell_path(None);
        assert!(d.starts_with("C:\\Windows"));
        let e = windows_powershell_path(Some(OsString::new()));
        assert!(e.starts_with("C:\\Windows"));
    }

    #[test]
    fn posix_command_is_dash_c_with_the_text_as_one_argument() {
        let cmd = posix().command("echo 'a b'; ls \"x\"", os(&[]));
        assert_eq!(args_of(&cmd), vec!["-c", "echo 'a b'; ls \"x\""]);
    }

    #[test]
    fn powershell_command_is_one_argument_after_command_with_the_fixed_prefix() {
        let shell = Shell {
            program: PathBuf::from("pwsh"),
            family: ShellFamily::PowerShell,
            name: "PowerShell 7 (pwsh)",
        };
        let text = "Write-Output \"a\\\" 100% C:\\dir\\";
        let cmd = shell.command(text, os(&[]));
        let args = args_of(&cmd);
        assert_eq!(
            &args[..4],
            ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
        );
        assert_eq!(args.len(), 5);
        assert_eq!(args[4], format!("{POWERSHELL_PREFIX}{text}"));
    }

    #[test]
    fn environment_is_the_parent_minus_the_scrubbed_names_plus_the_fixed_ones() {
        let mut parent = vec![("KEEP_ME", "1"), ("PATH", "/usr/bin")];
        for name in SCRUBBED {
            parent.push((name, "x"));
        }
        parent.push(("BASH_FUNC_echo%%", "() { echo HIJACKED; }"));
        parent.push(("BASH_FUNC_ls", "() { :; }"));
        parent.push(("TERM", "xterm"));
        let cmd = posix().command("true", os(&parent));
        let envs = envs_of(&cmd);
        let has = |name: &str| envs.iter().any(|(k, v)| k == name && v.is_some());
        assert!(has("KEEP_ME") && has("PATH"));
        for name in SCRUBBED {
            assert!(!has(name), "{name} must be removed");
        }
        assert!(!envs.iter().any(|(k, _)| k.starts_with("BASH_FUNC_")));
        for (name, value) in FIXED_ENV {
            assert!(envs.contains(&(name.to_string(), Some(value.to_string()))));
        }
        let term: Vec<_> = envs.iter().filter(|(k, _)| k == "TERM").collect();
        assert_eq!(term.len(), 1);
        assert_eq!(term[0].1.as_deref(), Some("dumb"));
        for name in [
            "OPENROUTER_API_KEY",
            "BASH_ENV",
            "ENV",
            "PROMPT_COMMAND",
            "PS4",
            "SHELLOPTS",
            "BASHOPTS",
            "CDPATH",
            "GLOBIGNORE",
        ] {
            assert!(SCRUBBED.contains(&name));
        }
        // Loader and language-option variables are the developer's own.
        for name in ["LD_PRELOAD", "NODE_OPTIONS", "DYLD_INSERT_LIBRARIES"] {
            assert!(!SCRUBBED.contains(&name));
        }
        assert_eq!(FIXED_ENV.len(), 5, "nothing else is added");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_exported_bash_function_cannot_replace_a_command() {
        let shell = detect();
        let parent = os(&[
            ("PATH", "/usr/bin:/bin"),
            ("BASH_FUNC_echo%%", "() { builtin echo HIJACKED; }"),
            ("OPENROUTER_API_KEY", "sk-test-not-a-real-key"),
        ]);
        let mut cmd = shell.command("echo hi; echo \"key=[$OPENROUTER_API_KEY]\"", parent);
        let out = cmd.output().await.unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(text, "hi\nkey=[]\n");
    }
}
