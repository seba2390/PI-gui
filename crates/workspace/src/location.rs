//! Where a working folder lives: on this computer, or on another machine reached through a
//! [`RemoteShell`] (in PI-gui: an SSH connection).
//!
//! Remote operations are written as POSIX `sh` scripts that run on the other machine, so the
//! same git CLI and directory listings work there without installing anything.

use std::fmt;
use std::io;
use std::sync::Arc;

/// Runs POSIX `sh` scripts on another machine.
pub trait RemoteShell: Send + Sync + fmt::Debug {
    /// Runs `script` with `sh` on the remote machine, feeding it `stdin` (or nothing), and
    /// returns what it printed. When `max_stdout` is set, stops reading (and stops the
    /// command) once that many bytes arrived, reporting `truncated`.
    ///
    /// `Err` means the remote machine could not be reached (the script did not run); a
    /// script that fails is reported through [`ShellOutput::code`].
    fn run(
        &self,
        script: &str,
        stdin: Option<Vec<u8>>,
        max_stdout: Option<usize>,
    ) -> io::Result<ShellOutput>;
}

/// What a remote script printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellOutput {
    /// Exit code; `None` if it was killed (e.g. after `max_stdout`).
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// `stdout` was cut off at `max_stdout`.
    pub truncated: bool,
}

/// Where a working folder lives.
#[derive(Clone, Default)]
pub enum Location {
    /// This computer.
    #[default]
    Local,
    /// Another machine; paths are paths there.
    Remote(Arc<dyn RemoteShell>),
}

impl Location {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

impl fmt::Debug for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Remote(shell) => f.debug_tuple("Remote").field(shell).finish(),
        }
    }
}

impl PartialEq for Location {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Local, Self::Local) => true,
            (Self::Remote(a), Self::Remote(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Quotes `s` as one word for a POSIX shell: words made only of safe characters stay as they
/// are, everything else is wrapped in single quotes (`'` becomes `'\''`).
pub fn sh_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+:,./_-".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        return s.to_owned();
    }
    let mut quoted = String::with_capacity(s.len() + 2);
    quoted.push('\'');
    for c in s.chars() {
        if c == '\'' {
            quoted.push_str(r"'\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

/// `script` preceded by `set -- <args>`, so the script sees them as `$1`, `$2`, ….
pub fn with_args<S: AsRef<str>>(script: &str, args: &[S]) -> String {
    let mut out = String::from("set --");
    for arg in args {
        out.push(' ');
        out.push_str(&sh_quote(arg.as_ref()));
    }
    out.push('\n');
    out.push_str(script);
    out
}

#[cfg(test)]
pub(crate) mod testing {
    //! A [`RemoteShell`] that runs scripts with the local `/bin/sh`, so the remote code paths
    //! can be tested without a second machine.

    use std::io::{Read as _, Write as _};
    use std::process::{Command, Stdio};

    use super::{RemoteShell, ShellOutput};

    #[derive(Debug)]
    pub struct LocalSh;

    impl RemoteShell for LocalSh {
        fn run(
            &self,
            script: &str,
            stdin: Option<Vec<u8>>,
            max_stdout: Option<usize>,
        ) -> std::io::Result<ShellOutput> {
            let mut child = Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let mut input = child.stdin.take().expect("stdin");
            let bytes = stdin.unwrap_or_default();
            let writer = std::thread::spawn(move || {
                let _ = input.write_all(&bytes);
            });
            let mut stdout = Vec::new();
            let limit = max_stdout.map_or(u64::MAX, |max| max as u64 + 1);
            child
                .stdout
                .take()
                .expect("stdout")
                .take(limit)
                .read_to_end(&mut stdout)?;
            let truncated = max_stdout.is_some_and(|max| stdout.len() > max);
            if truncated {
                let _ = child.kill();
                stdout.truncate(max_stdout.unwrap_or(0));
            }
            let mut stderr = String::new();
            let _ = child
                .stderr
                .take()
                .expect("stderr")
                .read_to_string(&mut stderr);
            let status = child.wait()?;
            let _ = writer.join();
            Ok(ShellOutput {
                code: if truncated { None } else { status.code() },
                stdout,
                stderr,
                truncated,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_compare_by_identity() {
        let shell: Arc<dyn RemoteShell> = Arc::new(testing::LocalSh);
        let a = Location::Remote(shell.clone());
        assert_eq!(a, Location::Remote(shell));
        assert_ne!(a, Location::Remote(Arc::new(testing::LocalSh)));
        assert_eq!(Location::Local, Location::default());
        assert_ne!(a, Location::Local);
    }

    #[test]
    fn args_are_quoted_once() {
        let out = testing::LocalSh
            .run(
                &with_args(
                    r#"for a; do printf '[%s]' "$a"; done"#,
                    &["a b", "it's", ""],
                ),
                None,
                None,
            )
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "[a b][it's][]");
    }
}
