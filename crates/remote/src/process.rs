//! Running a short-lived child process with captured output and a deadline.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::RemoteError;

/// What a finished process printed.
#[derive(Debug)]
pub(crate) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Runs `command` to completion, feeding it `stdin` (or `/dev/null`), and kills it once
/// `timeout` has passed, reporting [`RemoteError::Unreachable`].
///
/// Output pipes are drained on helper threads so a chatty child cannot block, and a
/// grandchild that keeps a pipe open (a backgrounded ssh master) cannot hang the caller.
pub(crate) fn run(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Output, RemoteError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;

    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        thread::spawn(move || {
            // A child that exits without reading its input is not an error here.
            let _ = pipe.write_all(&bytes);
        });
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let status = wait_with_deadline(&mut child, timeout)?;
    let collect =
        |rx: mpsc::Receiver<Vec<u8>>| rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    Ok(Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Like [`run`], but reads at most `max_stdout` bytes of stdout: once more arrive the process
/// is killed and the output reported as truncated (`true`).
pub(crate) fn run_capped(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    max_stdout: usize,
) -> Result<(Output, bool), RemoteError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
    let stderr = drain(child.stderr.take());
    let (tx, rx) = mpsc::channel();
    if let Some(pipe) = child.stdout.take() {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let limit = u64::try_from(max_stdout)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let _ = pipe.take(limit).read_to_end(&mut bytes);
            let _ = tx.send(bytes);
        });
    }
    let deadline = Instant::now() + timeout;
    let mut stdout = None;
    let mut pause = Duration::from_millis(1);
    let status = loop {
        if stdout.is_none()
            && let Ok(bytes) = rx.try_recv()
        {
            if bytes.len() > max_stdout {
                let _ = child.kill();
            }
            stdout = Some(bytes);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RemoteError::Unreachable(format!(
                "no response after {} seconds",
                timeout.as_secs()
            )));
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(20));
    };
    let mut stdout = stdout
        .or_else(|| rx.recv_timeout(Duration::from_secs(2)).ok())
        .unwrap_or_default();
    let truncated = stdout.len() > max_stdout;
    stdout.truncate(max_stdout);
    Ok((
        Output {
            status,
            stdout,
            stderr: stderr
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_default(),
        },
        truncated,
    ))
}

/// Runs `command` and calls `on_line` with each line of its stdout as it arrives (stderr is
/// collected), killing it after `timeout`.
pub(crate) fn run_streaming(
    mut command: Command,
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<Output, RemoteError> {
    use std::io::BufRead as _;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stderr = drain(child.stderr.take());
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(pipe) = child.stdout.take() {
        thread::spawn(move || {
            for line in std::io::BufReader::new(pipe).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let deadline = Instant::now() + timeout;
    let mut stdout = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(line) => {
                on_line(&line);
                stdout.extend_from_slice(line.as_bytes());
                stdout.push(b'\n');
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RemoteError::Unreachable(format!(
                        "no response after {} seconds",
                        timeout.as_secs()
                    )));
                }
            }
        }
    }
    let status = wait_with_deadline(&mut child, Duration::from_secs(10))?;
    Ok(Output {
        status,
        stdout,
        stderr: stderr
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default(),
    })
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = tx.send(bytes);
        });
    }
    rx
}

fn wait_with_deadline(child: &mut Child, timeout: Duration) -> Result<ExitStatus, RemoteError> {
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RemoteError::Unreachable(format!(
                "no response after {} seconds",
                timeout.as_secs()
            )));
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_feeds_stdin() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat; echo err >&2; exit 3"]);
        let output = run(command, Some(b"hello".to_vec()), Duration::from_secs(10)).unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout_lossy(), "hello");
        assert_eq!(output.stderr_lossy(), "err\n");
    }

    #[test]
    fn caps_stdout_and_streams_lines() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "yes 0123456789"]);
        let (output, truncated) = run_capped(command, None, Duration::from_secs(10), 1000).unwrap();
        assert!(truncated);
        assert_eq!(output.stdout.len(), 1000);

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo a; echo b >&2; echo c"]);
        let (output, truncated) = run_capped(command, None, Duration::from_secs(10), 1000).unwrap();
        assert!(!truncated);
        assert_eq!(output.stdout_lossy(), "a\nc\n");

        let mut lines = Vec::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo one; echo two; exit 4"]);
        let output = run_streaming(command, Duration::from_secs(10), |line| {
            lines.push(line.to_owned());
        })
        .unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(output.status.code(), Some(4));
    }

    #[test]
    fn kills_on_timeout() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 5"]);
        let started = Instant::now();
        let error = run(command, None, Duration::from_millis(200)).unwrap_err();
        assert!(matches!(error, RemoteError::Unreachable(_)));
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
