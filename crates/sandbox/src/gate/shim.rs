//! The wrappers in the sandbox (`git`, `gh`, `gpg`, `ssh-keygen` in `<sandbox folder>/bin`, first
//! on `$PATH`) and how they reach the gate.
//!
//! A wrapper runs the real program as usual unless the request is one the gate does
//! ([`needs_gate`]). Then it sends the request over the socket the sandbox extension listens on
//! (`$PISTACHIO_GATE_SOCKET`) and passes on what comes back: lines of JSON, `{"stdout": …}`,
//! `{"stderr": …}` and last `{"exit": n}` (or `{"local": true}`: run the real program after all).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};

use super::Request;

/// The environment variable with the gate's socket, set for sandboxed commands.
pub const SOCKET_ENV: &str = "PISTACHIO_GATE_SOCKET";
/// The programs that have a wrapper.
pub const TOOLS: [&str; 4] = ["git", "gh", "gpg", "ssh-keygen"];

/// The wrappers' folder in the sandbox's folder.
pub fn dir(sandbox_dir: &Path) -> PathBuf {
    sandbox_dir.join("bin")
}

/// Writes the wrappers (when they changed): small scripts that start `exe` as the wrapper.
pub fn install(sandbox_dir: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    let dir = dir(sandbox_dir);
    std::fs::create_dir_all(&dir)?;
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    for tool in TOOLS {
        let path = dir.join(tool);
        let script = format!(
            "#!/bin/sh\nexec '{exe}' {} shim {tool} \"$@\"\n",
            crate::cli::FLAG
        );
        if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
            let temp = dir.join(format!(".{tool}.tmp"));
            std::fs::write(&temp, script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::rename(temp, &path)?;
        }
    }
    Ok(dir)
}

/// Whether `tool args` is something the gate does (quickly, without reading the repository;
/// the gate decides for itself).
pub fn needs_gate(tool: &str, args: &[String]) -> bool {
    match tool {
        "git" => {
            let mut iter = args.iter();
            let mut command = None;
            while let Some(arg) = iter.next() {
                if matches!(arg.as_str(), "-C" | "-c" | "--config-env") {
                    iter.next();
                } else if !arg.starts_with('-') {
                    command = Some(arg.as_str());
                    break;
                }
            }
            let next = iter.next().map(String::as_str);
            match command {
                Some("clone" | "fetch" | "pull" | "push" | "ls-remote") => true,
                Some("submodule") => next == Some("update"),
                Some("lfs") => matches!(next, Some("fetch" | "pull" | "push")),
                _ => false,
            }
        }
        "gh" => {
            !(args.is_empty()
                || args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version"))
                || matches!(args[0].as_str(), "help" | "version" | "completion"))
        }
        "gpg" => args.len() == 3 && args[0] == "--status-fd=2" && args[1] == "-bsau",
        "ssh-keygen" => args
            .windows(2)
            .any(|pair| pair[0] == "-Y" && pair[1] == "sign"),
        _ => false,
    }
}

/// The real `tool` on `$PATH`, past the wrappers' folder.
pub fn real_program(tool: &str, wrappers: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| crate::paths::resolve(dir) != crate::paths::resolve(wrappers))
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
}

/// The wrapper for `tool`: returns the exit code.
pub fn run(tool: &str, args: Vec<String>, sandbox_dir: &Path) -> i32 {
    let wrappers = dir(sandbox_dir);
    let socket = std::env::var_os(SOCKET_ENV).map(PathBuf::from);
    if needs_gate(tool, &args)
        && let Some(socket) = socket
        && let Some(code) = through_gate(tool, &args, &socket)
    {
        return code;
    }
    exec_real(tool, &args, &wrappers)
}

/// Asks the gate; `None` when it says to run the real program (or can't be reached).
fn through_gate(tool: &str, args: &[String], socket: &Path) -> Option<i32> {
    #[cfg(unix)]
    {
        let mut stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
        let stdin = (tool == "gpg").then(|| {
            let mut text = String::new();
            let _ = std::io::stdin().read_to_string(&mut text);
            text
        });
        let request = Request {
            tool: tool.to_owned(),
            args: args.to_vec(),
            cwd: std::env::current_dir().ok()?,
            project: None,
            stdin,
            account: None,
        };
        let mut line = serde_json::to_string(&request).ok()?;
        line.push('\n');
        stream.write_all(line.as_bytes()).ok()?;
        let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if frame["local"].as_bool() == Some(true) {
                return None;
            }
            if let Some(text) = frame["stdout"].as_str() {
                let _ = out.write_all(text.as_bytes());
                let _ = out.flush();
            }
            if let Some(text) = frame["stderr"].as_str() {
                let _ = err.write_all(text.as_bytes());
                let _ = err.flush();
            }
            if let Some(code) = frame["exit"].as_i64() {
                return Some(i32::try_from(code).unwrap_or(1));
            }
        }
        let _ = writeln!(
            err,
            "[Pistachio] The connection to the app ended before the command did."
        );
        Some(1)
    }
    #[cfg(not(unix))]
    {
        let _ = (tool, args, socket);
        None
    }
}

fn exec_real(tool: &str, args: &[String], wrappers: &Path) -> i32 {
    let Some(program) = real_program(tool, wrappers) else {
        eprintln!("{tool}: command not found");
        return 127;
    };
    let mut command = std::process::Command::new(&program);
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = command.exec();
        eprintln!("{tool}: {error}");
        126
    }
    #[cfg(not(unix))]
    {
        command
            .status()
            .map_or(126, |status| status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|&arg| arg.to_owned()).collect()
    }

    #[test]
    fn only_what_needs_keys_goes_to_the_gate() {
        for (tool, list, gated) in [
            ("git", &["push"][..], true),
            ("git", &["-C", "sub", "fetch", "--all"], true),
            ("git", &["-c", "x=y", "pull"], true),
            ("git", &["submodule", "update", "--init"], true),
            ("git", &["submodule", "status"], false),
            ("git", &["lfs", "push", "origin"], true),
            ("git", &["lfs", "ls-files"], false),
            ("git", &["commit", "-m", "push"], false),
            ("git", &["status"], false),
            ("gh", &["pr", "list"], true),
            ("gh", &["--version"], false),
            ("gpg", &["--status-fd=2", "-bsau", "KEY"], true),
            ("gpg", &["--verify", "x"], false),
            ("ssh-keygen", &["-Y", "sign", "-n", "git"], true),
            ("ssh-keygen", &["-l", "-f", "k"], false),
        ] {
            assert_eq!(needs_gate(tool, &args(list)), gated, "{tool} {list:?}");
        }
    }

    #[test]
    fn wrappers_start_the_app() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bin = install(dir.path(), Path::new("/Apps/It's/pistachio")).expect("install");
        let git = std::fs::read_to_string(bin.join("git")).expect("read");
        assert_eq!(
            git,
            "#!/bin/sh\nexec '/Apps/It'\\''s/pistachio' --sandbox-tool shim git \"$@\"\n"
        );
        assert_eq!(real_program("definitely-not-a-program", &bin), None);
    }
}
