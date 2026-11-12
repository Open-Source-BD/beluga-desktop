//! Helpers shared by the tests that run real commands.

use std::path::Path;
use std::time::Duration;

use super::rules::ShellFamily;
use super::shell::Shell;

/// Generous on Windows, where PowerShell start-up alone can take seconds.
pub fn slow() -> bool {
    cfg!(windows)
}

pub fn long(secs: u64) -> u64 {
    if slow() { secs * 6 } else { secs }
}

pub fn pick<'a>(shell: &Shell, posix: &'a str, ps: &'a str) -> &'a str {
    match shell.family {
        ShellFamily::Posix => posix,
        ShellFamily::PowerShell => ps,
    }
}

pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
        !stat.is_empty() && !stat.starts_with('Z')
    }
    #[cfg(windows)]
    {
        let script = format!(
            "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ 'alive' }} else {{ 'gone' }}"
        );
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains("alive")
    }
}

pub async fn wait_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

pub async fn wait_for_pid_file(path: &Path) -> u32 {
    for _ in 0..400 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("pid file {} never appeared", path.display());
}
