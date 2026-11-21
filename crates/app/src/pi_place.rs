//! Files and small scripts where a session's pi runs: this computer, or the SSH host for a
//! remote session. The models dialog and the Skills page edit pi's configuration through
//! this, so remote sessions get the same features as local ones.
//!
//! Scripts are POSIX `sh` (run with `/bin/sh` here, over the shared connection on a host).
//! All functions block; call them on the background executor.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use gpui_kit::App;
use remote::{HostId, Ssh};

#[derive(Clone)]
pub enum PiPlace {
    Local {
        agent_dir: PathBuf,
    },
    Remote {
        label: String,
        ssh: Arc<Ssh>,
        agent_dir: String,
        home: String,
    },
}

impl PiPlace {
    /// Where pi for a session on `host` (or this computer) runs. `None` for a host that has
    /// not been probed yet (its folders are unknown) or without pi's agent folder here.
    pub fn for_host(host: Option<&HostId>, cx: &App) -> Option<Self> {
        match host {
            None => pi_sessions::agent_dir().map(|agent_dir| Self::Local { agent_dir }),
            Some(host) => {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                let (ssh, probe) = (hosts.ssh(host)?, hosts.probe(host)?);
                Some(Self::Remote {
                    label: hosts.label(host),
                    ssh,
                    agent_dir: probe.agent_dir.clone(),
                    home: probe.home.clone(),
                })
            }
        }
    }

    /// A path inside pi's agent folder (e.g. `models.json`, `skills`).
    pub fn agent_path(&self, name: &str) -> String {
        match self {
            Self::Local { agent_dir } => agent_dir.join(name).to_string_lossy().into_owned(),
            Self::Remote { agent_dir, .. } => format!("{agent_dir}/{name}"),
        }
    }

    /// The user's home folder there.
    pub fn home(&self) -> String {
        match self {
            Self::Local { .. } => dirs_home()
                .map(|home| home.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Self::Remote { home, .. } => home.clone(),
        }
    }

    /// `path` for display: `~/…` here, `host:path` on a host.
    pub fn describe(&self, path: &str, home: Option<&Path>) -> String {
        match self {
            Self::Local { .. } => crate::ui::short_path(Path::new(path), home),
            Self::Remote { label, .. } => format!("{label}:{path}"),
        }
    }

    /// The file's text, or `None` if there is no such file.
    pub fn read(&self, path: &str) -> Result<Option<String>, String> {
        let bytes = match self {
            Self::Local { .. } => match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("Could not read {path}: {error}")),
            },
            Self::Remote { ssh, .. } => ssh.read_file(path).map_err(|e| e.to_string())?,
        };
        Ok(bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Writes atomically, creating folders; with `backup`, the previous file is kept as
    /// `<path>.bak-pi-gui`.
    pub fn write(&self, path: &str, text: &str, backup: bool) -> Result<(), String> {
        match self {
            Self::Local { .. } => {
                let path = Path::new(path);
                let fail =
                    |error: std::io::Error| format!("Could not save {}: {error}", path.display());
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(fail)?;
                }
                if backup && path.is_file() {
                    let mut name = path.as_os_str().to_owned();
                    name.push(".bak-pi-gui");
                    std::fs::copy(path, PathBuf::from(name)).map_err(fail)?;
                }
                let mut tmp = path.as_os_str().to_owned();
                tmp.push(".pi-gui-tmp");
                let tmp = PathBuf::from(tmp);
                std::fs::write(&tmp, text).map_err(fail)?;
                std::fs::rename(&tmp, path).map_err(fail)
            }
            Self::Remote { ssh, .. } => ssh
                .write_file(path, text.as_bytes().to_vec(), backup)
                .map_err(|error| error.to_string()),
        }
    }

    /// Moves a file or folder to the trash (the host's trash for a remote session).
    pub fn trash(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Local { .. } => trash::delete(path).map_err(|error| error.to_string()),
            Self::Remote { ssh, .. } => ssh.trash(path).map_err(|error| error.to_string()),
        }
    }

    /// Runs a POSIX `sh` script with positional `args` and `stdin`; returns its stdout, or
    /// its stderr as the error when it fails.
    pub fn run_script(
        &self,
        script: &str,
        args: &[&str],
        stdin: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, String> {
        match self {
            Self::Local { .. } => {
                let mut child = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .arg("sh")
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|error| format!("Could not run sh: {error}"))?;
                let input = stdin.unwrap_or_default();
                if let Some(mut pipe) = child.stdin.take() {
                    std::thread::spawn(move || {
                        let _ = pipe.write_all(&input);
                    });
                }
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                if output.status.success() {
                    Ok(output.stdout)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    Err(if stderr.is_empty() {
                        format!("the command failed ({})", output.status)
                    } else {
                        stderr
                    })
                }
            }
            Self::Remote { ssh, .. } => ssh
                .exec_with_input(
                    &workspace::with_args(script, args),
                    stdin.unwrap_or_default(),
                )
                .map_err(|error| error.to_string()),
        }
    }

    /// GETs `url` with curl there (so `localhost` is the machine pi uses); the API key goes
    /// through stdin, never on a command line.
    pub fn fetch(&self, url: &str, key: Option<&str>) -> Result<String, String> {
        let header = key
            .map(|key| format!("Authorization: Bearer {key}\n"))
            .unwrap_or_default();
        let script = "command -v curl >/dev/null 2>&1 || { echo 'curl is not installed' >&2; exit 1; }\n\
                      curl -sS -m 8 -H @- \"$1\"";
        self.run_script(script, &[url], Some(header.into_bytes()))
            .map(|stdout| String::from_utf8_lossy(&stdout).into_owned())
            .map_err(|error| error.trim_start_matches("curl: ").to_owned())
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::PiPlace;

    #[test]
    fn local_files_scripts_and_backups() {
        let dir = tempfile::tempdir().unwrap();
        let place = PiPlace::Local {
            agent_dir: dir.path().into(),
        };
        let file = place.agent_path("sub/models.json");
        assert_eq!(place.read(&file).unwrap(), None);
        place.write(&file, "one", true).unwrap();
        place.write(&file, "two", true).unwrap();
        assert_eq!(place.read(&file).unwrap().as_deref(), Some("two"));
        assert_eq!(
            std::fs::read_to_string(format!("{file}.bak-pi-gui")).unwrap(),
            "one"
        );
        let skill = place.agent_path("skills/x/SKILL.md");
        place.write(&skill, "s", false).unwrap();
        assert!(!std::path::Path::new(&format!("{skill}.bak-pi-gui")).exists());

        let out = place
            .run_script(
                "printf '%s|' \"$1\" \"$2\"; cat",
                &["a b", "c"],
                Some(b"in".to_vec()),
            )
            .unwrap();
        assert_eq!(out, b"a b|c|in");
        let error = place
            .run_script("echo oops >&2; exit 3", &[], None)
            .unwrap_err();
        assert_eq!(error, "oops");
    }
}
