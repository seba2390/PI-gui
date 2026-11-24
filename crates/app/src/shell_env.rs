//! Recovers the user's login-shell environment.
//!
//! Apps launched from Finder/Dock or a desktop launcher get a minimal environment: `pi` (a
//! Node script) and `node` itself are often not on its `PATH`, and the API keys, proxies and
//! regions exported in the shell's startup files are missing, so pi would offer fewer models
//! than in the terminal. Like VS Code and Zed, we ask the user's login shell for its
//! environment once at startup and import all of it except [`is_imported`]'s few exclusions.
//!
//! The values are secrets as often as not: they are never logged.

use std::ffi::OsString;

pub type Vars = Vec<(OsString, OsString)>;

/// `(key, value)` pairs from the login shell that differ from ours. Blocks until the shell
/// answered or took too long.
pub fn login_shell_env() -> Vars {
    // Launched from a terminal: the environment is already the user's.
    if std::env::var_os("TERM").is_some() {
        return Vec::new();
    }
    #[cfg(unix)]
    {
        unix::login_shell_env()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::{OsStr, OsString};
    use std::io::{self, ErrorKind, Read as _};
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use super::Vars;

    /// How long the shell may take to report its environment. Generous, because only pi
    /// processes wait for it, never the UI.
    const TIMEOUT: Duration = Duration::from_secs(10);
    /// How long a read waits before the deadline and the shell are looked at again.
    const POLL: Duration = Duration::from_millis(50);
    /// How long the shell may take to exit after it reported its environment (its exit hooks
    /// may be writing the history file).
    const EXIT_GRACE: Duration = Duration::from_secs(2);
    /// No environment is this large (the OS limits it to 2 MB): a startup file prints without
    /// end.
    const MAX_OUTPUT: usize = 4 * 1024 * 1024;

    /// Set for the probing shell, so that startup files can tell (VS Code and Zed set similar
    /// ones).
    const PROBE_ENV: &str = "PISTACHIO_RESOLVING_ENVIRONMENT";

    /// Prints the environment between two markers, as startup files may print too. The
    /// markers are split by quotes, so a shell that echoes its command never prints one. Runs
    /// in sh, bash, zsh, ksh and fish. An `env` without `-0` (macOS before 13) lists one
    /// variable per line.
    const SCRIPT: &str = "echo __PISTACHIO_ENV''_START__; \
                          command env -0 || command env; \
                          echo __PISTACHIO_ENV''_END__";
    const START: &[u8] = b"__PISTACHIO_ENV_START__\n";
    const END: &[u8] = b"__PISTACHIO_ENV_END__";

    /// Variables that describe the probing shell or belong to the app.
    const EXCLUDED: &[&str] = &[
        // Where the probing shell is, how deep it is nested and its options.
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
        "PS1",
        "PS2",
        "PS3",
        "PS4",
        "SHELLOPTS",
        "BASHOPTS",
        // May be a folder made for the probing shell only and removed when it exits; the
        // app's own temporary folder is right for its children.
        "TMPDIR",
        // The terminal, which pi has none of (`GPG_TTY=$(tty)` says "not a tty" here).
        // Programs pi starts would believe they run in that terminal.
        "TERM",
        "TERMCAP",
        "TERMINFO",
        "TERMINFO_DIRS",
        "COLORTERM",
        "COLUMNS",
        "LINES",
        "GPG_TTY",
        "WINDOWID",
        "VTE_VERSION",
        "TMUX",
        "TMUX_PANE",
        "STY",
        "INSIDE_EMACS",
        // The login session the app runs in.
        "SECURITYSESSIONID",
        // The app finds pi's settings and sessions with its own values of these: pi must use
        // the same folders.
        "HOME",
        "PI_CODING_AGENT_DIR",
        "PI_CODING_AGENT_SESSION_DIR",
    ];

    /// Like [`EXCLUDED`], for everything that starts with these.
    const EXCLUDED_PREFIXES: &[&str] = &[
        // The app's own variables (for itself or a debug copy), including [`PROBE_ENV`].
        "PISTACHIO_",
        // Set by launchd and Core Foundation for each process.
        "XPC_",
        "__CF",
        // The terminal and its session.
        "TERM_",
        "SHELL_SESSION_",
        "LC_TERMINAL",
        "ITERM_",
        "KITTY_",
        "WEZTERM_",
        "ALACRITTY_",
        "GHOSTTY_",
        "KONSOLE_",
        "GNOME_TERMINAL_",
        "ZELLIJ",
    ];

    /// Whether to import the variable `key`. `SHELL` and `ZDOTDIR` are: the shell does not set
    /// them for itself, so they are the user's choice, which programs in a terminal see too.
    /// The name check drops what a startup file printed between the variables.
    fn is_imported(key: &OsStr) -> bool {
        let bytes = key.as_bytes();
        !bytes.is_empty()
            && !bytes
                .iter()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            && !EXCLUDED.iter().any(|name| key == *name)
            && !EXCLUDED_PREFIXES
                .iter()
                .any(|prefix| bytes.starts_with(prefix.as_bytes()))
    }

    pub fn login_shell_env() -> Vars {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        match probe(&shell, TIMEOUT) {
            Ok(vars) => differing(vars, |key| std::env::var_os(key)),
            Err(error) => {
                log::warn!(
                    "could not read the environment of the login shell {}: {error}",
                    shell.to_string_lossy()
                );
                let home = std::env::var_os("HOME").map(PathBuf::from);
                fallback_path(std::env::var_os("PATH").as_deref(), home.as_deref())
                    .map(|path| ("PATH".into(), path))
                    .into_iter()
                    .collect()
            }
        }
    }

    /// The variables whose value is not the app's `own` one already.
    fn differing(vars: Vars, own: impl Fn(&OsStr) -> Option<OsString>) -> Vars {
        vars.into_iter()
            .filter(|(key, value)| own(key).as_ref() != Some(value))
            .collect()
    }

    /// `path` followed by the folders pi is usually installed in, for when the login shell
    /// did not tell its `PATH`.
    fn fallback_path(path: Option<&OsStr>, home: Option<&Path>) -> Option<OsString> {
        let mut dirs: Vec<PathBuf> = path
            .map(std::env::split_paths)
            .into_iter()
            .flatten()
            .collect();
        let usual = ["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from);
        let in_home = [".local/bin", ".npm-global/bin", ".volta/bin", ".bun/bin"];
        let in_home = home
            .into_iter()
            .flat_map(|home| in_home.map(|dir| home.join(dir)));
        for dir in usual.into_iter().chain(in_home) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        std::env::join_paths(dirs).ok()
    }

    /// The environment `shell` exports as an interactive login shell, without what is never
    /// imported. Returns within about `timeout`, whatever the shell's startup files do.
    pub(super) fn probe(shell: &OsStr, timeout: Duration) -> io::Result<Vars> {
        let deadline = Instant::now() + timeout;
        // A socket instead of a pipe: its reads can time out, so a process started by the
        // shell that keeps it open holds nothing up and no thread is needed to read it.
        let (mut ours, theirs) = UnixStream::pair()?;
        ours.set_read_timeout(Some(POLL))?;
        // The command is dropped here, and with it our copy of the shell's end.
        let mut child = Command::new(shell)
            .args(["-l", "-i", "-c", SCRIPT])
            .env(PROBE_ENV, "1")
            .stdin(Stdio::null())
            .stdout(OwnedFd::from(theirs))
            .stderr(Stdio::null())
            // Its own process group: away from a terminal the app may have been started
            // from, and stopped together with the commands it waits for.
            .process_group(0)
            .spawn()?;
        let output = read_between_markers(&mut ours, &mut child, deadline);
        let grace = if output.is_ok() {
            EXIT_GRACE
        } else {
            Duration::ZERO
        };
        reap(child, grace);
        let mut vars = parse(&output?);
        vars.retain(|(key, _)| is_imported(key));
        Ok(vars)
    }

    /// What the shell prints between the markers. Stops reading at the end marker, when the
    /// shell exited without one, or at the `deadline`.
    fn read_between_markers(
        stream: &mut UnixStream,
        child: &mut Child,
        deadline: Instant,
    ) -> io::Result<Vec<u8>> {
        let ended = || io::Error::new(ErrorKind::UnexpectedEof, "it did not report it");
        let mut output = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(ErrorKind::TimedOut, "it took too long"));
            }
            if output.len() > MAX_OUTPUT {
                return Err(io::Error::other("it printed too much"));
            }
            // Before the read: whatever an exited shell printed is there to be read.
            let exited = child.try_wait()?.is_some();
            match stream.read(&mut chunk) {
                Ok(0) => return Err(ended()),
                Ok(count) => {
                    let searched = output.len().saturating_sub(END.len());
                    output.extend_from_slice(&chunk[..count]);
                    if let Some(end) = find(&output[searched..], END) {
                        output.truncate(searched + end);
                        let start = find(&output, START).ok_or_else(ended)?;
                        return Ok(output.split_off(start + START.len()));
                    }
                }
                Err(error) => match error.kind() {
                    ErrorKind::Interrupted => {}
                    ErrorKind::WouldBlock | ErrorKind::TimedOut if !exited => {}
                    // Only something the shell left running still has the socket.
                    ErrorKind::WouldBlock | ErrorKind::TimedOut => return Err(ended()),
                    _ => return Err(error),
                },
            }
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Waits for the shell to exit, and kills it and the commands it waits for after `grace`,
    /// on a thread of its own: the environment does not wait for it.
    fn reap(mut child: Child, grace: Duration) {
        let reaper = std::thread::Builder::new()
            .name("login-shell-reaper".into())
            .spawn(move || {
                if exits_within(&mut child, grace) {
                    return;
                }
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = exits_within(&mut child, EXIT_GRACE);
            });
        if let Err(error) = reaper {
            log::warn!("could not wait for the login shell to exit: {error}");
        }
    }

    /// Whether `child` exits (and is reaped) within `time`.
    fn exits_within(child: &mut Child, time: Duration) -> bool {
        let deadline = Instant::now() + time;
        loop {
            match child.try_wait() {
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => return false,
                Ok(Some(_)) | Err(_) => return true,
            }
        }
    }

    /// The variables in what `env -0` (or a plain `env`) printed.
    fn parse(output: &[u8]) -> Vars {
        let vars = if output.contains(&0) {
            output
                .split(|byte| *byte == 0)
                .filter_map(assignment)
                .collect()
        } else {
            parse_lines(output)
        };
        vars.into_iter()
            .map(|(key, value)| (OsString::from_vec(key), OsString::from_vec(value)))
            .collect()
    }

    type ByteVars = Vec<(Vec<u8>, Vec<u8>)>;

    /// One variable per line. A line that does not start with a variable's name continues the
    /// value before it, which is a guess: a value's line may look like a variable.
    fn parse_lines(output: &[u8]) -> ByteVars {
        let output = output.strip_suffix(b"\n").unwrap_or(output);
        let mut vars = ByteVars::new();
        for line in output.split(|byte| *byte == b'\n') {
            match assignment(line).filter(|(key, _)| is_name(key)) {
                Some(var) => vars.push(var),
                None => {
                    if let Some((_, value)) = vars.last_mut() {
                        value.push(b'\n');
                        value.extend_from_slice(line);
                    }
                }
            }
        }
        vars
    }

    fn assignment(text: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let equals = text.iter().position(|byte| *byte == b'=')?;
        Some((text[..equals].to_vec(), text[equals + 1..].to_vec()))
    }

    /// Whether `key` is the name of a variable (or of a function bash exports).
    fn is_name(key: &[u8]) -> bool {
        key.starts_with(b"BASH_FUNC_")
            || key.first().is_some_and(|byte| !byte.is_ascii_digit())
                && key
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    }

    #[cfg(test)]
    mod tests {
        use std::ffi::{OsStr, OsString};
        use std::os::unix::ffi::OsStringExt as _;
        use std::path::Path;
        use std::time::{Duration, Instant};

        use super::{Vars, differing, fallback_path, is_imported, parse, probe};

        const LONG: Duration = Duration::from_secs(20);

        /// A stand-in for the user's shell: `rc` is its startup file. Runs what it is given
        /// with `-c`, like a real shell with `-l -i -c`.
        fn shell(dir: &Path, rc: &str) -> OsString {
            use std::os::unix::fs::PermissionsExt as _;
            let path = dir.join("shell");
            std::fs::write(
                &path,
                format!("#!/bin/sh\n[ \"$1 $2 $3\" = '-l -i -c' ] || exit 9\n{rc}\neval \"$4\"\n"),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.into_os_string()
        }

        fn value<'a>(vars: &'a Vars, key: &str) -> Option<&'a OsStr> {
            vars.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_os_str())
        }

        #[test]
        fn imports_what_the_startup_files_export() {
            let dir = tempfile::tempdir().unwrap();
            let rc = "echo 'Welcome!'; printf 'no newline'; echo\n\
                      export ANTHROPIC_API_KEY=sk-test\n\
                      export HTTPS_PROXY=http://proxy:8080/?a=b\n\
                      export PISTACHIO_PI_BIN=/other/pi\n\
                      export KEY_LINES='first\nLOOKS_LIKE=a variable\n\nlast\n'\n\
                      export NOT_UTF8=\"$(printf 'a\\377b')\"\n\
                      export SEEN=\"$PISTACHIO_RESOLVING_ENVIRONMENT\"\n\
                      export SHLVL=7 TERM=xterm TERM_PROGRAM=x PI_CODING_AGENT_DIR=/elsewhere\n\
                      local_only=1\n\
                      trap 'echo bye; exit 3' EXIT";
            let vars = probe(&shell(dir.path(), rc), LONG).unwrap();
            assert_eq!(value(&vars, "ANTHROPIC_API_KEY"), Some("sk-test".as_ref()));
            assert_eq!(
                value(&vars, "HTTPS_PROXY"),
                Some("http://proxy:8080/?a=b".as_ref())
            );
            assert_eq!(
                value(&vars, "KEY_LINES"),
                Some("first\nLOOKS_LIKE=a variable\n\nlast\n".as_ref())
            );
            assert_eq!(
                value(&vars, "NOT_UTF8"),
                Some(OsString::from_vec(b"a\xffb".to_vec()).as_os_str())
            );
            assert_eq!(value(&vars, "SEEN"), Some("1".as_ref()));
            assert!(value(&vars, "PATH").is_some());
            for key in [
                "LOOKS_LIKE",
                "local_only",
                "SHLVL",
                "PWD",
                "_",
                "TERM",
                "TERM_PROGRAM",
                "PI_CODING_AGENT_DIR",
                "PISTACHIO_PI_BIN",
                "PISTACHIO_RESOLVING_ENVIRONMENT",
            ] {
                assert_eq!(value(&vars, key), None, "{key}");
            }
        }

        #[test]
        fn noise_after_the_markers_and_echoed_commands_do_no_harm() {
            let dir = tempfile::tempdir().unwrap();
            let rc = "export API_KEY=1\necho \"$4\"\nset -v -x\nexec 2>&1";
            let vars = probe(&shell(dir.path(), rc), LONG).unwrap();
            assert_eq!(value(&vars, "API_KEY"), Some("1".as_ref()));
            assert!(
                vars.iter()
                    .all(|(key, _)| !key.to_string_lossy().contains(char::is_whitespace))
            );
        }

        #[test]
        fn a_background_process_that_keeps_stdout_open_holds_nothing_up() {
            let dir = tempfile::tempdir().unwrap();
            let started = Instant::now();
            let vars = probe(&shell(dir.path(), "export A=1\nsleep 15 &"), LONG).unwrap();
            assert_eq!(value(&vars, "A"), Some("1".as_ref()));
            // Also when the shell ends without a report.
            assert!(probe(&shell(dir.path(), "sleep 15 &\nexit 0"), LONG).is_err());
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{:?}",
                started.elapsed()
            );
        }

        #[test]
        fn a_hanging_shell_is_given_up_and_killed() {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("pid");
            // The shell waits for a command, which must go too.
            let rc = format!("echo $$ > '{}'\nsleep 60", pid_file.display());
            let started = Instant::now();
            let error = probe(&shell(dir.path(), &rc), Duration::from_secs(5)).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{:?}",
                started.elapsed()
            );
            let group = format!("-{}", std::fs::read_to_string(&pid_file).unwrap().trim());
            let deadline = Instant::now() + Duration::from_secs(10);
            while std::process::Command::new("kill")
                .args(["-0", "--", &group])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
            {
                assert!(Instant::now() < deadline, "the shell still runs");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        #[test]
        fn a_shell_that_reports_nothing_or_too_much_is_an_error() {
            let dir = tempfile::tempdir().unwrap();
            assert!(probe(OsStr::new("/nonexistent/shell"), LONG).is_err());
            assert!(probe(&shell(dir.path(), "exit 0"), LONG).is_err());
            let error = probe(&shell(dir.path(), "exec yes"), LONG).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::Other, "{error}");
        }

        /// The shells on this machine, with an empty home folder. Not from a terminal: an
        /// interactive bash waits to be in its foreground.
        /// `cargo test -p pistachio -- --ignored real_shells < /dev/null`
        #[test]
        #[ignore = "runs the shells on this machine"]
        fn real_shells_report_their_environment() {
            let dir = tempfile::tempdir().unwrap();
            for name in ["sh", "bash", "zsh", "ksh", "dash", "fish"] {
                let Some(path) = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                    .map(|dir| dir.join(name))
                    .find(|path| path.exists())
                else {
                    continue;
                };
                let rc = format!(
                    "export HOME='{}' API_KEY=1\nexec '{}' \"$@\"",
                    dir.path().display(),
                    path.display()
                );
                let vars = probe(&shell(dir.path(), &rc), LONG)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(value(&vars, "API_KEY"), Some("1".as_ref()), "{name}");
                eprintln!("{name}: {} variables", vars.len());
            }
        }

        #[test]
        fn an_env_without_nul_separators_is_read_by_lines() {
            let vars = parse(b"noise\nA=1\nB=two\n  x = y\n\n}\nBASH_FUNC_f%%=() { :\n}\nC=\n");
            let expected: Vars = [
                ("A", "1"),
                ("B", "two\n  x = y\n\n}"),
                ("BASH_FUNC_f%%", "() { :\n}"),
                ("C", ""),
            ]
            .map(|(key, value)| (key.into(), value.into()))
            .into();
            assert_eq!(vars, expected);
            assert_eq!(parse(b""), Vars::new());
        }

        #[test]
        fn variables_of_the_shell_the_terminal_and_the_app_stay_out() {
            for key in [
                "OLDPWD",
                "COLUMNS",
                "TMPDIR",
                "PS1",
                "XPC_SERVICE_NAME",
                "__CFBundleIdentifier",
                "ITERM_SESSION_ID",
                "SECURITYSESSIONID",
                "HOME",
                "PI_CODING_AGENT_SESSION_DIR",
                "PISTACHIO_PI_BIN",
                "",
                "printed\nA",
            ] {
                assert!(!is_imported(OsStr::new(key)), "{key}");
            }
            for key in [
                "OPENAI_API_KEY",
                "AWS_PROFILE",
                "CLOUD_ML_REGION",
                "SSH_AUTH_SOCK",
                "no_proxy",
                "TERMII_API_KEY",
                "PSQL_HISTORY",
                "PI_OFFLINE",
                "SHELL",
                "ZDOTDIR",
                "BASH_FUNC_f%%",
            ] {
                assert!(is_imported(OsStr::new(key)), "{key}");
            }
        }

        #[test]
        fn what_the_app_already_has_is_skipped() {
            let vars = vec![("A".into(), "1".into()), ("B".into(), "2".into())];
            let own = |key: &OsStr| Some(OsString::from(if key == "A" { "1" } else { "old" }));
            assert_eq!(differing(vars, own), [("B".into(), "2".into())]);
        }

        #[test]
        fn the_fallback_path_only_appends() {
            let path = fallback_path(
                Some(OsStr::new("/usr/local/bin:/bin")),
                Some(Path::new("/home/me")),
            )
            .unwrap();
            assert_eq!(
                path,
                "/usr/local/bin:/bin:/opt/homebrew/bin:/home/me/.local/bin:\
                 /home/me/.npm-global/bin:/home/me/.volta/bin:/home/me/.bun/bin"
            );
        }
    }
}
