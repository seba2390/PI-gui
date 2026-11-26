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
use std::time::Duration;

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
            Self::Local { .. } => {
                dirs_home().map_or_default(|home| home.to_string_lossy().into_owned())
            }
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
    /// `<path>.bak-pistachio`.
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
                    name.push(".bak-pistachio");
                    std::fs::copy(path, PathBuf::from(name)).map_err(fail)?;
                }
                let mut tmp = path.as_os_str().to_owned();
                tmp.push(".pistachio-tmp");
                let tmp = PathBuf::from(tmp);
                write_private(&tmp, path, text.as_bytes()).map_err(fail)?;
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

    /// Sends an HTTP request from there (so `localhost` is the machine pi uses): a GET, or a POST
    /// of the JSON in `body`. Any status is a reply; what `Err` says is why there was none.
    /// Redirects are not followed, so a key in `headers` goes nowhere else.
    pub fn request(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Option<&str>,
        timeout: Duration,
    ) -> Result<Reply, NoReply> {
        match self {
            Self::Local { .. } => request_here(url, headers, body, timeout),
            Self::Remote { ssh, .. } => {
                // Keys go through stdin, never on a command line.
                let script = "command -v curl >/dev/null 2>&1 || { echo 'curl is not installed' >&2; exit 1; }\n\
                              curl -sS -K -";
                let config = curl_config(url, headers, body, timeout);
                let output = ssh
                    .exec_with_input(
                        &workspace::with_args::<&str>(script, &[]),
                        config.into_bytes(),
                    )
                    .map_err(|error| NoReply::of(&error.to_string()))?;
                parse_curl(&String::from_utf8_lossy(&output))
            }
        }
    }
}

/// An HTTP reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// Why a request got no reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoReply {
    /// Nothing listens there.
    Refused,
    /// There is no such host.
    UnknownHost,
    Timeout,
    Other(String),
}

impl NoReply {
    fn of(error: &str) -> Self {
        let lower = error.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
        if has(&[
            "refused",
            "couldn't connect",
            "failed to connect",
            "could not connect",
        ]) {
            Self::Refused
        } else if has(&["resolve", "host not found", "lookup address", "nodename"]) {
            Self::UnknownHost
        } else if has(&["timeout", "timed out"]) {
            Self::Timeout
        } else {
            Self::Other(error.trim().trim_start_matches("curl: ").to_owned())
        }
    }
}

/// The most that is read of a reply (a list of hundreds of models is 1 MB or so).
const MAX_REPLY: u64 = 16 * 1024 * 1024;

fn request_here(
    url: &str,
    headers: &[(&str, String)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<Reply, NoReply> {
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent("pistachio")
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false);
    if pi_config::server::is_private_url(url) {
        config = config.proxy(None);
    }
    let agent = config.build().new_agent();
    let fail = |error: ureq::Error| match error {
        ureq::Error::ConnectionFailed => NoReply::Refused,
        ureq::Error::HostNotFound => NoReply::UnknownHost,
        ureq::Error::Timeout(_) => NoReply::Timeout,
        error => NoReply::of(&error.to_string()),
    };
    let response = match body {
        None => {
            let mut request = agent.get(url);
            for (name, value) in headers {
                request = request.header(*name, value);
            }
            request.call()
        }
        Some(body) => {
            let mut request = agent.post(url).header("Content-Type", "application/json");
            for (name, value) in headers {
                request = request.header(*name, value);
            }
            request.send(body)
        }
    };
    let mut response = response.map_err(fail)?;
    Ok(Reply {
        status: response.status().as_u16(),
        body: response
            .body_mut()
            .with_config()
            .limit(MAX_REPLY)
            .lossy_utf8(true)
            .read_to_string()
            .map_err(fail)?,
    })
}

/// A request as a curl config file. Its output is the body, a line break and the status.
fn curl_config(
    url: &str,
    headers: &[(&str, String)],
    body: Option<&str>,
    timeout: Duration,
) -> String {
    use std::fmt::Write as _;
    let quoted = |text: &str| {
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    };
    let mut config = format!(
        "url = \"{}\"\nmax-time = {}\nmax-filesize = {MAX_REPLY}\nwrite-out = \"\\n%{{http_code}}\"\n",
        quoted(url),
        timeout.as_secs().max(1),
    );
    if pi_config::server::is_private_url(url) {
        config.push_str("noproxy = \"*\"\n");
    }
    for (name, value) in headers {
        let _ = writeln!(config, "header = \"{name}: {}\"", quoted(value));
    }
    if let Some(body) = body {
        let _ = writeln!(config, "header = \"Content-Type: application/json\"");
        let _ = writeln!(config, "data-raw = \"{}\"", quoted(body));
    }
    config
}

fn parse_curl(output: &str) -> Result<Reply, NoReply> {
    let (body, status) = output.rsplit_once('\n').unwrap_or(("", output));
    let status = status
        .trim()
        .parse()
        .map_err(|_| NoReply::Other("curl sent no reply".into()))?;
    Ok(Reply {
        status,
        body: body.to_owned(),
    })
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Writes `bytes` to `tmp` with the permissions of `original` (or owner-only for a new file):
/// pi's config can hold API keys, and replacing it must never make it readable by others.
fn write_private(tmp: &Path, original: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let _ = std::fs::remove_file(tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mode =
            std::fs::metadata(original).map_or(0o600, |meta| meta.permissions().mode() & 0o777);
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = original;
    let mut file = options.open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{NoReply, PiPlace, Reply, curl_config, parse_curl};

    #[test]
    fn requests_through_curl() {
        let config = curl_config(
            "http://10.0.0.1/v1/models",
            &[("Authorization", "Bearer a\"b\\c".into())],
            Some("{\"a\": \"x\\n\"}"),
            Duration::from_secs(8),
        );
        assert_eq!(
            config,
            "url = \"http://10.0.0.1/v1/models\"\nmax-time = 8\nmax-filesize = 16777216\n\
             write-out = \"\\n%{http_code}\"\nnoproxy = \"*\"\n\
             header = \"Authorization: Bearer a\\\"b\\\\c\"\n\
             header = \"Content-Type: application/json\"\n\
             data-raw = \"{\\\"a\\\": \\\"x\\\\n\\\"}\"\n"
        );
        assert_eq!(
            parse_curl("{\n}\n200"),
            Ok(Reply {
                status: 200,
                body: "{\n}".into()
            })
        );
        assert!(parse_curl("").is_err());
        assert_eq!(
            NoReply::of("curl: (7) Failed to connect to h port 1: Connection refused"),
            NoReply::Refused
        );
        assert_eq!(
            NoReply::of("curl: (6) Could not resolve host: h"),
            NoReply::UnknownHost
        );
        assert_eq!(
            NoReply::of("curl: (28) Operation timed out"),
            NoReply::Timeout
        );
        assert_eq!(
            NoReply::of("curl: (60) SSL certificate problem"),
            NoReply::Other("(60) SSL certificate problem".into())
        );
    }

    /// What curl makes of the config, where there is curl.
    #[cfg(unix)]
    #[test]
    fn curl_reads_the_config() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let mut read = 0;
            while !String::from_utf8_lossy(&request[..read]).contains("\"hi\\n\"") {
                read += stream.read(&mut request[read..]).unwrap();
            }
            stream
                .write_all(b"HTTP/1.1 418 X\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        let config = curl_config(
            &url,
            &[("x-api-key", "a\"b\\c".into())],
            Some("{\"a\":\"hi\\n\"}"),
            Duration::from_secs(5),
        );
        let place = PiPlace::Local {
            agent_dir: std::env::temp_dir(),
        };
        let output = place
            .run_script("curl -sS -K -", &[], Some(config.into_bytes()))
            .unwrap();
        assert_eq!(
            parse_curl(&String::from_utf8_lossy(&output)),
            Ok(Reply {
                status: 418,
                body: "ok".into()
            })
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /x "), "{request}");
        assert!(request.contains("x-api-key: a\"b\\c\r\n"), "{request}");
    }

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
            std::fs::read_to_string(format!("{file}.bak-pistachio")).unwrap(),
            "one"
        );
        let skill = place.agent_path("skills/x/SKILL.md");
        place.write(&skill, "s", false).unwrap();
        assert!(!std::path::Path::new(&format!("{skill}.bak-pistachio")).exists());

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
