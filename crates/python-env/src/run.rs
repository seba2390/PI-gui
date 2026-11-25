//! Running a program of an installation.

use std::io::{BufRead as _, BufReader};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::InstallError;

/// Runs `command` (with its standard error piped) to its end, or kills it when cancelled. Each
/// line it writes there goes to `line` as it comes.
pub fn run(
    command: &mut Command,
    what: &str,
    cancel: &AtomicBool,
    line: &mut dyn FnMut(&str),
) -> Result<(), InstallError> {
    let failed = |error: &str| InstallError::Failed(format!("{what} failed: {error}"));
    let mut child = command
        .spawn()
        .map_err(|error| failed(&error.to_string()))?;
    let stderr = child.stderr.take().expect("stderr is piped");
    let (sender, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for text in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            let _ = sender.send(String::from_utf8_lossy(&text).trim().to_owned());
        }
    });
    let mut text = Vec::new();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(InstallError::Cancelled);
        }
        for next in lines.try_iter() {
            line(&next);
            text.push(next);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(failed(&error.to_string())),
        }
    };
    let _ = reader.join();
    for next in lines.try_iter() {
        line(&next);
        text.push(next);
    }
    if status.success() {
        return Ok(());
    }
    let error: Vec<_> = text
        .iter()
        .map(String::as_str)
        .filter(|line| !line.is_empty() && !line.starts_with("warning:"))
        .collect();
    let last = &error[error.len().saturating_sub(4)..];
    Err(failed(&match last {
        [] => status.to_string(),
        lines => lines.join(" "),
    }))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::process::Stdio;

    use pretty_assertions::assert_eq;

    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    #[test]
    fn programs_are_stopped_or_say_why_they_failed() {
        let go = AtomicBool::new(false);
        let try_it = |script: &str| run(&mut sh(script), "Trying", &go, &mut |_| {});
        assert_eq!(try_it("exit 0"), Ok(()));
        let script = "echo 'warning: minor' >&2; echo 'error: no network' >&2; exit 2";
        let error = try_it(script).unwrap_err();
        assert_eq!(error.to_string(), "Trying failed: error: no network");
        let error = try_it("exit 3").unwrap_err();
        assert!(error.to_string().contains('3'), "{error}");
        let stop = AtomicBool::new(true);
        assert_eq!(
            run(&mut sh("exec sleep 60"), "Trying", &stop, &mut |_| {}),
            Err(InstallError::Cancelled)
        );
    }

    #[test]
    fn passes_on_what_a_program_writes() {
        let go = AtomicBool::new(false);
        let mut lines = Vec::new();
        run(
            &mut sh("echo one >&2; echo two >&2"),
            "Trying",
            &go,
            &mut |line| lines.push(line.to_owned()),
        )
        .unwrap();
        assert_eq!(lines, ["one", "two"]);
    }
}
