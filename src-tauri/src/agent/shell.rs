//! Which shell runs the model's commands, and how it is invoked.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use tokio::process::Command;

use super::rules::ShellFamily;

/// Variables removed from a command's environment. The first is the app's own
/// key; the rest make a non-interactive shell run or change code before the
/// command the user approved.
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
    /// on pipes, the scrubbed and fixed environment. The caller sets the
    /// working directory.
    pub fn command(&self, text: &str) -> Command {
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
        for name in SCRUBBED {
            cmd.env_remove(name);
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

/// The first directory of `path_var` holding a file called `name`.
#[cfg_attr(not(windows), allow(dead_code))]
fn find_in_path(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(windows))]
fn detect_uncached() -> Shell {
    if Path::new("/bin/bash").is_file() {
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
            .unwrap_or_else(|| PathBuf::from("powershell.exe")),
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
            if Path::new("/bin/bash").is_file() {
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

    #[test]
    fn posix_command_is_dash_c_with_the_text_as_one_argument() {
        let shell = Shell {
            program: PathBuf::from("/bin/sh"),
            family: ShellFamily::Posix,
            name: "sh",
        };
        let cmd = shell.command("echo 'a b'; ls \"x\"");
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
        let cmd = shell.command(text);
        let args = args_of(&cmd);
        assert_eq!(
            &args[..4],
            ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
        );
        assert_eq!(args.len(), 5);
        assert_eq!(args[4], format!("{POWERSHELL_PREFIX}{text}"));
    }

    #[test]
    fn environment_is_scrubbed_and_fixed() {
        let shell = Shell {
            program: PathBuf::from("/bin/sh"),
            family: ShellFamily::Posix,
            name: "sh",
        };
        let cmd = shell.command("true");
        let envs: Vec<(String, Option<String>)> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for name in SCRUBBED {
            assert!(
                envs.contains(&(name.to_string(), None)),
                "{name} must be removed"
            );
        }
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
        for (name, value) in FIXED_ENV {
            assert!(envs.contains(&(name.to_string(), Some(value.to_string()))));
        }
        assert_eq!(FIXED_ENV.len(), 5, "nothing else is added");
    }
}
