//! Spawns a real shell in a PTY.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_channel::{Receiver, TryRecvError};
use term::{Terminal, TerminalConfig, TerminalEvent};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Drain events until `done` returns true for the terminal state / an event, or time out.
fn wait_until(
    terminal: &Terminal,
    events: &Receiver<TerminalEvent>,
    mut done: impl FnMut(&Terminal, Option<&TerminalEvent>) -> bool,
) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) => {
                if done(terminal, Some(&event)) {
                    return true;
                }
            }
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
            Err(TryRecvError::Closed) => return false,
        }
        // Also poll directly: Wakeup is coalesced until a snapshot is taken.
        if done(terminal, None) {
            return true;
        }
    }
    false
}

#[test]
fn echo_in_real_shell_then_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = TerminalConfig::new(dir.path());
    config.shell = Some(PathBuf::from("/bin/sh"));
    let (terminal, events) = Terminal::spawn(config).expect("spawn /bin/sh");

    terminal.write(&b"echo hi\r"[..]);
    let saw_output = wait_until(&terminal, &events, |t, _| {
        t.snapshot()
            .lines
            .iter()
            .any(|line| line.text().trim_end() == "hi")
    });
    assert!(
        saw_output,
        "never saw `hi`; screen:\n{}",
        terminal.snapshot().text()
    );

    terminal.write(&b"exit\r"[..]);
    let mut code = None;
    let exited = wait_until(&terminal, &events, |_, event| match event {
        Some(TerminalEvent::Exited(c)) => {
            code = Some(*c);
            true
        }
        _ => false,
    });
    assert!(
        exited,
        "shell did not exit; screen:\n{}",
        terminal.snapshot().text()
    );
    assert_eq!(code, Some(Some(0)));
    assert!(terminal.has_exited());
    // Input after exit is ignored, snapshots still work.
    terminal.write(&b"echo nope\r"[..]);
    assert!(terminal.snapshot().text().contains("hi"));
}

#[test]
fn shell_starts_in_cwd_and_drop_does_not_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("marker-file-xyz");
    std::fs::write(&marker, b"").expect("write marker");
    let mut config = TerminalConfig::new(dir.path());
    config.shell = Some(PathBuf::from("/bin/sh"));
    let (terminal, events) = Terminal::spawn(config).expect("spawn /bin/sh");

    terminal.write(&b"ls\r"[..]);
    let listed = wait_until(&terminal, &events, |t, _| {
        t.snapshot()
            .lines
            .iter()
            .any(|line| line.text().trim_end() == "marker-file-xyz")
    });
    assert!(
        listed,
        "cwd not honored; screen:\n{}",
        terminal.snapshot().text()
    );

    let start = Instant::now();
    drop(terminal);
    assert!(start.elapsed() < Duration::from_millis(500));
}

/// Starts the user's default shell (a login shell via `/usr/bin/login` on macOS). Ignored by
/// default because it runs the user's shell rc files; run with `--ignored`.
#[test]
#[ignore = "runs the user's own shell configuration"]
fn default_shell_starts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (terminal, events) = Terminal::spawn(TerminalConfig::new(dir.path())).expect("spawn");
    terminal.write(&b"echo $TERM-$COLORTERM; pwd\r"[..]);
    let ok = wait_until(&terminal, &events, |t, _| {
        let text = t.snapshot().text();
        text.lines()
            .any(|l| l.trim_end() == "xterm-256color-truecolor")
    });
    assert!(ok, "screen:\n{}", terminal.snapshot().text());
}
