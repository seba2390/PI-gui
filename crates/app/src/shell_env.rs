//! Recovers the user's login-shell environment.
//!
//! Apps launched from Finder/Dock or a desktop launcher get a minimal `PATH`, so
//! `pi` (a Node script) and `node` itself are often not found. Like VS Code and
//! Zed, we ask the user's login shell for its environment once at startup.

use std::ffi::OsString;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::time::Duration;

/// Environment variables worth importing from the login shell.
#[cfg(unix)]
const IMPORTED: &[&str] = &[
    "PATH",
    "NVM_DIR",
    "NODE_PATH",
    "VOLTA_HOME",
    "FNM_DIR",
    "LANG",
];

/// `(key, value)` pairs from the login shell that differ from ours.
pub fn login_shell_env() -> Vec<(OsString, OsString)> {
    // Launched from a terminal: the environment is already the user's.
    if std::env::var_os("TERM").is_some() {
        return Vec::new();
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let marker = "__PISTACHIO_ENV__";
        let script = format!("echo {marker}; env; echo {marker}");
        let output = run_with_timeout(
            Command::new(&shell).args(["-l", "-i", "-c", &script]),
            Duration::from_secs(5),
        );
        let Some(stdout) = output else {
            return Vec::new();
        };
        let mut sections = stdout.split(marker);
        let body = sections.nth(1).unwrap_or_default();
        body.lines()
            .filter_map(|line| line.split_once('='))
            .filter(|(key, _)| IMPORTED.contains(key))
            .filter(|(key, value)| std::env::var(key).ok().as_deref() != Some(*value))
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(unix)]
fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<String> {
    use std::process::Stdio;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::BufReader::new(stdout), &mut text).ok();
        text
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                log::warn!("login shell did not report its environment in time");
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}
