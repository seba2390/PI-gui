//! Talking to one remote host through the system `ssh` binary.
//!
//! Every invocation shares one master connection per host (OpenSSH `ControlMaster`), so after
//! the first connection a remote command costs a few tens of milliseconds instead of a full
//! handshake. The master lingers for ten minutes after its last user, and any call quietly
//! starts a new one when needed.
//!
//! Remote scripts are POSIX `sh` only (the host may be Linux or macOS). The user's login shell
//! (`$SHELL`) must accept POSIX quoting, which holds for bash, zsh, dash and ksh.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::config::resolve_full;
use crate::error::meaningful_lines;
use crate::host::Target;
use crate::process::{self, Output};
use crate::{RemoteError, classify};

/// Unix socket paths are limited to 104 bytes (including the NUL) on macOS. ssh binds
/// `<dir>/<40-char %C hash>.<16 random chars>` before renaming it into place, so the control
/// directory itself may be at most 104 - 1 - 1 - 40 - 17 = 45 bytes long.
const MAX_CONTROL_DIR_LEN: usize = 45;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const KEYSCAN_TIMEOUT: Duration = Duration::from_secs(20);

/// Run by the remote login shell to start a program: `$1` is the working folder (empty for
/// none; `~` and `~/…` expand to `$HOME`), the rest is the command. A leading `~/` in the
/// program expands too, and an absolute program's own folder is put in front of `PATH` so a
/// node script installed next to its `node` (nvm, Homebrew) finds it.
const LAUNCH_SCRIPT: &str = r#"d=$1; shift; case $d in "") ;; "~") cd || exit 1 ;; "~/"*) cd -- "$HOME/${d#??}" || exit 1 ;; *) cd -- "$d" || exit 1 ;; esac; case $1 in "~/"*) p=$HOME/${1#??}; shift; set -- "$p" "$@" ;; esac; case $1 in /*) PATH=${1%/*}:$PATH; export PATH ;; esac; exec "$@""#;

/// Probes the host. `$1` is the pi command. Prints `PIGUI_*=` marker lines so whatever the
/// user's profile scripts print is ignored. pi is looked up in a login shell first, then an
/// interactive one (where `.bashrc` additions such as nvm live).
const PROBE_SCRIPT: &str = r#"printf 'PIGUI_HOME=%s\n' "$HOME"
printf 'PIGUI_SYSTEM=%s\n' "$(uname -sm)"
s=${SHELL:-/bin/sh}
f='c=$1; case $c in "~/"*) c=$HOME/${c#??} ;; esac; command -v "$c"'
p=$("$s" -lc "$f" sh "$1" </dev/null 2>/dev/null | tail -n 1)
case $p in /*) ;; *) p=$("$s" -ic "$f" sh "$1" </dev/null 2>/dev/null | tail -n 1) ;; esac
case $p in /*)
  printf 'PIGUI_PI=%s\n' "$p"
  v=$("$s" -lc 'PATH=${1%/*}:$PATH; export PATH; "$1" --version' sh "$p" </dev/null 2>/dev/null | tail -n 1)
  printf 'PIGUI_VERSION=%s\n' "$v" ;;
esac
exit 0"#;

/// Lists subfolders of `$1` (empty or `~` = home): the canonical path, then every
/// non-hidden directory name, each terminated by a NUL byte.
const LIST_SCRIPT: &str = r#"fail() { printf '%s\n' "$1" >&2; exit 1; }
case $1 in ""|"~") d=$HOME ;; "~/"*) d=$HOME/${1#??} ;; *) d=$1 ;; esac
[ -d "$d" ] || fail "No such folder: $d"
cd -- "$d" 2>/dev/null || fail "Permission denied: $d"
printf '%s\000' "$(pwd -P)"
for e in *; do [ -d "$e" ] && printf '%s\000' "$e"; done
exit 0"#;

/// A connection to one remote host. Cheap to clone; holds no live resources itself (the shared
/// master connection lives in a background `ssh` process).
#[derive(Debug, Clone)]
pub struct Ssh {
    target: Target,
    control_dir: PathBuf,
}

/// What [`Ssh::probe`] found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The remote `$HOME`.
    pub home: String,
    /// Absolute path of the pi executable, if found.
    pub pi_path: Option<String>,
    /// Last line `pi --version` printed, if pi was found and printed anything.
    pub pi_version: Option<String>,
    /// `uname -sm`, e.g. `Linux aarch64` or `Darwin arm64`.
    pub system: String,
}

/// Subfolders of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    /// Canonical absolute path of the listed folder.
    pub path: String,
    /// Its parent, or `None` at `/`.
    pub parent: Option<String>,
    /// Names of non-hidden subfolders (symlinks to folders included), sorted
    /// case-insensitively.
    pub dirs: Vec<String>,
}

impl Ssh {
    /// A connection using the default control-socket directory ([`Ssh::default_control_dir`]).
    pub fn new(target: Target) -> Self {
        Self::with_control_dir(target, Self::default_control_dir())
    }

    /// A connection keeping its control socket in `dir` (at most 45 bytes long; see
    /// [`Ssh::default_control_dir`]).
    pub fn with_control_dir(target: Target, dir: PathBuf) -> Self {
        Self {
            target,
            control_dir: dir,
        }
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn control_dir(&self) -> &Path {
        &self.control_dir
    }

    /// The ssh executable (found on `PATH`).
    pub fn program() -> PathBuf {
        PathBuf::from("ssh")
    }

    /// `<agent dir>/pi-gui/cm`, or `/tmp/pi-gui-cm-<uid>` when that path is too long for a
    /// control socket.
    pub fn default_control_dir() -> PathBuf {
        pi_sessions::agent_dir()
            .map(|agent| agent.join("pi-gui").join("cm"))
            .filter(|dir| dir.is_absolute() && dir.as_os_str().len() <= MAX_CONTROL_DIR_LEN)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/pi-gui-cm-{}", user_tag())))
    }

    /// Options shared by every invocation, then the target's own options and destination:
    ///
    /// `-o ControlMaster=auto -o ControlPath=<dir>/%C -o ControlPersist=600 -o BatchMode=yes
    /// -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
    /// -o LogLevel=ERROR`, plus for a custom target `-p`, `-i` (with `IdentitiesOnly=yes`),
    /// `-J` and `-l`; last the alias or host.
    ///
    /// ssh uses the first value it sees for an option, so options that must win are placed
    /// before these.
    pub fn base_args(&self) -> Vec<OsString> {
        let mut control_path = OsString::from("ControlPath=");
        control_path.push(escape_percent(&self.control_dir));
        control_path.push("/%C");
        let mut args: Vec<OsString> = Vec::new();
        for option in [
            OsString::from("ControlMaster=auto"),
            control_path,
            OsString::from("ControlPersist=600"),
            OsString::from("BatchMode=yes"),
            OsString::from("ConnectTimeout=10"),
            OsString::from("ServerAliveInterval=15"),
            OsString::from("ServerAliveCountMax=3"),
            OsString::from("LogLevel=ERROR"),
        ] {
            args.push("-o".into());
            args.push(option);
        }
        args.extend(Self::target_args(&self.target));
        args
    }

    /// Only the target's own options and its destination (what `ssh -G` needs).
    pub(crate) fn target_args(target: &Target) -> Vec<OsString> {
        match target {
            Target::Alias { alias } => vec![alias.into()],
            Target::Custom {
                host,
                user,
                port,
                identity_file,
                proxy_jump,
            } => {
                let mut args: Vec<OsString> = Vec::new();
                if let Some(port) = port {
                    args.extend(["-p".into(), port.to_string().into()]);
                }
                if let Some(identity) = identity_file {
                    args.extend([
                        "-i".into(),
                        identity.into(),
                        "-o".into(),
                        "IdentitiesOnly=yes".into(),
                    ]);
                }
                if let Some(jump) = proxy_jump.as_deref().filter(|jump| !jump.is_empty()) {
                    args.extend(["-J".into(), jump.into()]);
                }
                if let Some(user) = user.as_deref().filter(|user| !user.is_empty()) {
                    args.extend(["-l".into(), user.into()]);
                }
                args.push(host.into());
                args
            }
        }
    }

    /// Makes sure the shared master connection is up, starting it if needed (blocking, up to
    /// ~45 s). With `accept_new_host_key` an unknown host key is added to `known_hosts`; pass
    /// it only after the user confirmed the fingerprint. A changed key is never accepted.
    ///
    /// Implemented as `ssh -o ControlMaster=auto … true` (with `ControlPersist` the first
    /// client backgrounds a master) rather than `ssh -fN -o ControlMaster=yes`: `auto` replaces
    /// a stale socket and never leaves a second, unshared connection behind when a master
    /// already runs.
    pub fn connect(&self, accept_new_host_key: bool) -> Result<(), RemoteError> {
        if self.is_connected() {
            return Ok(());
        }
        let mut command = self.ssh_command();
        command.arg("-T");
        if accept_new_host_key {
            command.args(["-o", "StrictHostKeyChecking=accept-new"]);
        }
        command.args(self.base_args()).args(["--", "true"]);
        let output = process::run(command, None, CONNECT_TIMEOUT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ssh_failure(&output))
        }
    }

    /// Whether a master connection is running (`ssh -O check`, local only).
    pub fn is_connected(&self) -> bool {
        let mut command = self.ssh_command();
        command.args(["-O", "check"]).args(self.base_args());
        process::run(command, None, CHECK_TIMEOUT).is_ok_and(|output| output.status.success())
    }

    /// Stops the master connection (`ssh -O exit`), which also ends every session running
    /// over it. Errors are ignored.
    pub fn disconnect(&self) {
        let mut command = self.ssh_command();
        command.args(["-O", "exit"]).args(self.base_args());
        if let Err(error) = process::run(command, None, CHECK_TIMEOUT) {
            log::debug!("ssh -O exit for {}: {error}", self.target.display());
        }
    }

    /// Runs a POSIX `sh` script on the host (`sh -c <script>`, stdin `/dev/null`) and returns
    /// its stdout. Times out after 60 s. A failure of ssh itself (exit status 255) is
    /// classified with [`classify`]; any other non-zero exit becomes [`RemoteError::Failed`]
    /// with the script's stderr.
    pub fn exec(&self, script: &str) -> Result<String, RemoteError> {
        self.exec_bytes(script, EXEC_TIMEOUT)
            .map(|output| output.stdout_lossy())
    }

    fn exec_bytes(&self, script: &str, timeout: Duration) -> Result<Output, RemoteError> {
        let mut command = self.ssh_command();
        command
            .arg("-T")
            .args(self.base_args())
            .arg("--")
            .arg(format!("sh -c {}", sh_quote(script)));
        let output = process::run(command, None, timeout)?;
        if output.status.success() {
            Ok(output)
        } else if output.status.code() == Some(255) {
            Err(ssh_failure(&output))
        } else {
            let stderr = output.stderr_lossy();
            let lines = meaningful_lines(&stderr);
            Err(RemoteError::Failed(if lines.is_empty() {
                format!("remote command failed ({})", output.status)
            } else {
                lines.join("\n")
            }))
        }
    }

    /// The program and arguments that run `argv` on the host in `cwd` with piped stdio and no
    /// TTY, for a long-running process such as `pi --mode rpc`.
    ///
    /// The remote command is
    /// `exec "${SHELL:-/bin/sh}" -lc '<launch script>' pi-gui <cwd> <argv…>`: the login shell
    /// provides the user's full `PATH`, and the working folder and arguments travel as
    /// positional parameters, so they are quoted exactly once and the login shell never
    /// re-parses them. `cwd` and a program may start with `~/`; an absolute program's folder is
    /// prepended to `PATH`. Stopping: close the process's stdin (pi exits on EOF) or kill the
    /// local ssh, which closes the channel.
    pub fn command(&self, cwd: Option<&str>, argv: &[String]) -> (PathBuf, Vec<OsString>) {
        self.ensure_control_dir();
        let mut args = vec![OsString::from("-T")];
        args.extend(self.base_args());
        args.push("--".into());
        args.push(remote_launch_command(cwd, argv).into());
        (Self::program(), args)
    }

    /// One round trip that reports the remote home, where `pi_command` resolves under the
    /// user's shells, its `--version`, and `uname -sm`.
    pub fn probe(&self, pi_command: &str) -> Result<Probe, RemoteError> {
        let script = format!("set -- {}\n{PROBE_SCRIPT}", sh_quote(pi_command));
        let output = self.exec_bytes(&script, PROBE_TIMEOUT)?;
        parse_probe(&output.stdout_lossy())
    }

    /// Lists the non-hidden subfolders of `path` (absolute; `""` or `"~"` is the home folder,
    /// `~/…` is relative to it).
    pub fn list_dirs(&self, path: &str) -> Result<DirListing, RemoteError> {
        let script = format!("set -- {}\n{LIST_SCRIPT}", sh_quote(path));
        let output = self.exec_bytes(&script, LIST_TIMEOUT)?;
        parse_listing(&output.stdout)
    }

    /// The SHA256 fingerprints of the host's keys, one per line (e.g.
    /// `256 SHA256:… host (ED25519)`), for asking the user whether to trust it.
    ///
    /// Direct hosts are scanned with `ssh-keyscan -p <port> <hostname>` (from `ssh -G`) piped
    /// into `ssh-keygen -lf -`. Hosts behind `ProxyJump`/`ProxyCommand` cannot be scanned
    /// directly, so ssh itself connects without known hosts or authentication and the key it
    /// was offered is reported instead.
    pub fn host_key_fingerprint(&self) -> Result<String, RemoteError> {
        let resolved = resolve_full(&self.target)?;
        if resolved.proxied {
            return self.fingerprint_via_ssh();
        }
        let host = &resolved.resolved;
        let mut scan = Command::new("ssh-keyscan");
        scan.args([
            "-T",
            "10",
            "-p",
            &host.port.to_string(),
            "--",
            &host.hostname,
        ]);
        let keys = process::run(scan, None, KEYSCAN_TIMEOUT)?;
        if keys.stdout.iter().all(u8::is_ascii_whitespace) {
            let reason = meaningful_lines(&keys.stderr_lossy())
                .into_iter()
                .find(|line| !line.starts_with('#'))
                .unwrap_or("no keys returned")
                .to_owned();
            return Err(RemoteError::Unreachable(format!(
                "could not fetch the host key of {}:{} ({reason})",
                host.hostname, host.port
            )));
        }
        let mut keygen = Command::new("ssh-keygen");
        keygen.args(["-l", "-f", "-"]);
        let prints = process::run(keygen, Some(keys.stdout), KEYSCAN_TIMEOUT)?;
        if !prints.status.success() {
            return Err(RemoteError::Failed(prints.stderr_lossy().trim().to_owned()));
        }
        Ok(prints.stdout_lossy().trim().to_owned())
    }

    fn fingerprint_via_ssh(&self) -> Result<String, RemoteError> {
        let mut command = Command::new(Self::program());
        command
            .args(["-v", "-T"])
            .args(["-o", "ControlMaster=no", "-o", "ControlPath=none"])
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .args(["-o", "StrictHostKeyChecking=no"])
            .args(["-o", "UserKnownHostsFile=/dev/null"])
            .args(["-o", "GlobalKnownHostsFile=/dev/null"])
            .args(["-o", "PreferredAuthentications=none"])
            .args([
                "-o",
                "PubkeyAuthentication=no",
                "-o",
                "PasswordAuthentication=no",
            ])
            .args(["-o", "KbdInteractiveAuthentication=no"])
            .args([
                "-o",
                "HostbasedAuthentication=no",
                "-o",
                "GSSAPIAuthentication=no",
            ])
            .args(Self::target_args(&self.target))
            .args(["--", "true"]);
        let output = process::run(command, None, KEYSCAN_TIMEOUT)?;
        let stderr = output.stderr_lossy();
        parse_server_host_key(&stderr).ok_or_else(|| classify(&stderr))
    }

    fn ssh_command(&self) -> Command {
        self.ensure_control_dir();
        Command::new(Self::program())
    }

    /// Creates the control directory (mode 0700) if needed. Failures are only logged: ssh
    /// then reports a clear error of its own.
    fn ensure_control_dir(&self) {
        if let Err(error) = prepare_private_dir(&self.control_dir) {
            log::warn!(
                "control socket directory {} is unusable: {error}",
                self.control_dir.display()
            );
        }
    }
}

/// Creates `dir` with mode 0700, or checks that the existing one is a real directory owned by
/// the current user (it may live in the shared `/tmp`) and tightens its mode.
fn prepare_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("exists but is not a directory"));
    }
    if current_uid().is_some_and(|uid| uid != meta.uid()) {
        return Err(io::Error::other("owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The current user's uid, taken from the owner of their home directory.
fn current_uid() -> Option<u32> {
    dirs::home_dir()
        .and_then(|home| fs::metadata(home).ok())
        .map(|meta| meta.uid())
}

fn user_tag() -> String {
    current_uid().map_or_else(
        || std::env::var("USER").unwrap_or_else(|_| "user".to_owned()),
        |uid| uid.to_string(),
    )
}

/// `%` starts a token in `ControlPath`; a literal one is written `%%`.
fn escape_percent(path: &Path) -> OsString {
    let text = path.as_os_str().to_string_lossy();
    if text.contains('%') {
        text.replace('%', "%%").into()
    } else {
        path.as_os_str().to_owned()
    }
}

fn ssh_failure(output: &Output) -> RemoteError {
    classify(&output.stderr_lossy())
}

/// The command string ssh hands to the remote user's shell for [`Ssh::command`].
fn remote_launch_command(cwd: Option<&str>, argv: &[String]) -> String {
    let mut remote = String::from(r#"exec "${SHELL:-/bin/sh}" -lc "#);
    remote.push_str(&sh_quote(LAUNCH_SCRIPT));
    remote.push_str(" pi-gui ");
    remote.push_str(&sh_quote(cwd.unwrap_or("")));
    for arg in argv {
        remote.push(' ');
        remote.push_str(&sh_quote(arg));
    }
    remote
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

/// Parses the marker lines printed by [`PROBE_SCRIPT`].
fn parse_probe(stdout: &str) -> Result<Probe, RemoteError> {
    let mut home = None;
    let mut system = None;
    let mut pi_path = None;
    let mut pi_version = None;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        match key {
            "PIGUI_HOME" => home = value,
            "PIGUI_SYSTEM" => system = value,
            "PIGUI_PI" => pi_path = value,
            "PIGUI_VERSION" => pi_version = value,
            _ => {}
        }
    }
    let home = home.ok_or_else(|| {
        RemoteError::Failed(format!("unexpected reply from the host: {}", stdout.trim()))
    })?;
    Ok(Probe {
        home,
        pi_path,
        pi_version,
        system: system.unwrap_or_default(),
    })
}

/// Parses the NUL-separated output of [`LIST_SCRIPT`].
fn parse_listing(stdout: &[u8]) -> Result<DirListing, RemoteError> {
    let mut fields = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let path = fields
        .next()
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| {
            RemoteError::Failed(format!(
                "unexpected folder listing: {}",
                String::from_utf8_lossy(stdout).trim()
            ))
        })?;
    let mut dirs: Vec<String> = fields.filter(|name| !name.is_empty()).collect();
    dirs.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    let parent = parent_of(&path);
    Ok(DirListing { path, parent, dirs })
}

fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_owned()),
        Some(index) => Some(trimmed[..index].to_owned()),
        None => None,
    }
}

/// Finds `debug1: Server host key: ssh-ed25519 SHA256:…` in `ssh -v` output.
fn parse_server_host_key(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let rest = line.split_once("Server host key: ")?.1.trim();
        let (kind, fingerprint) = rest.split_once(' ')?;
        Some(format!("{fingerprint} ({kind})"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("pi"), "pi");
        assert_eq!(sh_quote("/usr/local/bin/pi"), "/usr/local/bin/pi");
        assert_eq!(sh_quote("--mode"), "--mode");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("$HOME"), "'$HOME'");
        assert_eq!(sh_quote("~/x"), "'~/x'");
        assert_eq!(sh_quote("a=b"), "'a=b'");
        assert_eq!(sh_quote("`id`;\n"), "'`id`;\n'");
    }

    #[test]
    fn quoting_survives_a_shell() {
        for word in [
            "", "a b", "it's", "$HOME", "\"q\"", "\\n", "a\nb", "ünï", "*",
        ] {
            let out = Command::new("/bin/sh")
                .args(["-c", &format!("printf %s {}", sh_quote(word))])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), word);
        }
    }

    #[test]
    fn base_args_for_alias_and_custom() {
        let alias = Ssh::with_control_dir(
            Target::Alias {
                alias: "spark".into(),
            },
            "/tmp/cm".into(),
        );
        let common = [
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=/tmp/cm/%C",
            "-o",
            "ControlPersist=600",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "LogLevel=ERROR",
        ];
        let mut expected = os(&common);
        expected.push("spark".into());
        assert_eq!(alias.base_args(), expected);

        let custom = Ssh::with_control_dir(
            Target::Custom {
                host: "box.example.com".into(),
                user: Some("me".into()),
                port: Some(2222),
                identity_file: Some("/k/id key".into()),
                proxy_jump: Some("bastion".into()),
            },
            "/tmp/c%m".into(),
        );
        let mut expected = os(&common);
        expected[3] = "ControlPath=/tmp/c%%m/%C".into();
        expected.extend(os(&[
            "-p",
            "2222",
            "-i",
            "/k/id key",
            "-o",
            "IdentitiesOnly=yes",
            "-J",
            "bastion",
            "-l",
            "me",
            "box.example.com",
        ]));
        assert_eq!(custom.base_args(), expected);
    }

    #[test]
    fn command_args() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = Ssh::with_control_dir(
            Target::Alias {
                alias: "spark".into(),
            },
            dir.path().join("cm"),
        );
        let (program, args) = ssh.command(
            Some("/home/me/my project"),
            &strings(&["pi", "--mode", "rpc"]),
        );
        assert_eq!(program, PathBuf::from("ssh"));
        assert_eq!(args[0], "-T");
        assert_eq!(&args[1..args.len() - 2], ssh.base_args().as_slice());
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(
            args[args.len() - 1],
            OsString::from(format!(
                r#"exec "${{SHELL:-/bin/sh}}" -lc {} pi-gui '/home/me/my project' pi --mode rpc"#,
                sh_quote(LAUNCH_SCRIPT)
            ))
        );
        // The control directory is created privately on demand.
        let meta = fs::metadata(dir.path().join("cm")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o700);
    }

    /// Runs the remote command string the way sshd would (the user's shell, `-c`), locally
    /// with `SHELL=/bin/sh` and a scratch `HOME`.
    fn run_remote(home: &Path, cwd: Option<&str>, argv: &[String]) -> (String, String, bool) {
        let remote = remote_launch_command(cwd, argv);
        let out = Command::new("/bin/sh")
            .args(["-c", &remote])
            .env("SHELL", "/bin/sh")
            .env("HOME", home)
            .env_remove("ENV")
            .output()
            .unwrap();
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
            out.status.success(),
        )
    }

    #[test]
    fn command_quoting_survives_both_shells() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let cwd = home.join("it's a \"dir\" $HOME `x`");
        fs::create_dir(&cwd).unwrap();
        let argv = strings(&[
            "/bin/sh",
            "-c",
            r#"pwd -P; for a; do printf '[%s]\n' "$a"; done"#,
            "argv0",
            "a b",
            "it's",
            "$HOME",
            "",
            "`id`; rm -rf /nope",
            "*",
        ]);
        let (stdout, stderr, ok) = run_remote(&home, cwd.to_str(), &argv);
        assert!(ok, "{stderr}");
        assert_eq!(
            stdout,
            format!(
                "{}\n[a b]\n[it's]\n[$HOME]\n[]\n[`id`; rm -rf /nope]\n[*]\n",
                cwd.display()
            )
        );
    }

    #[test]
    fn command_expands_tilde_and_prepends_program_dir() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("proj")).unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        let tool = home.join("bin/tool");
        fs::write(
            &tool,
            "#!/bin/sh\npwd -P\nprintf '%s\\n' \"$PATH\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

        let (stdout, stderr, ok) =
            run_remote(&home, Some("~/proj"), &strings(&["~/bin/tool", "x y"]));
        assert!(ok, "{stderr}");
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines[0], home.join("proj").to_str().unwrap());
        assert!(
            lines[1].starts_with(&format!("{}:", home.join("bin").display())),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "x y");

        // No working folder: stays in $HOME (where sshd starts).
        let (_, stderr, ok) = run_remote(&home, None, &strings(&["true"]));
        assert!(ok, "{stderr}");
        // A missing folder fails before running anything.
        let (stdout, _, ok) = run_remote(&home, Some("/no/such/dir"), &strings(&["echo", "ran"]));
        assert!(!ok);
        assert_eq!(stdout, "");
    }

    #[test]
    fn list_script_runs_locally() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        for dir in ["b", "A", "c d", ".hidden", "it's"] {
            fs::create_dir(home.join(dir)).unwrap();
        }
        fs::write(home.join("file"), "").unwrap();
        let script = format!("set -- {}\n{LIST_SCRIPT}", sh_quote("~"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(out.status.success());
        let listing = parse_listing(&out.stdout).unwrap();
        assert_eq!(listing.path, home.to_str().unwrap());
        assert_eq!(listing.dirs, strings(&["A", "b", "c d", "it's"]));

        let script = format!("set -- {}\n{LIST_SCRIPT}", sh_quote("/no/such"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "No such folder: /no/such\n"
        );
    }

    #[test]
    fn probe_script_runs_locally() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        let tool = home.join("bin/fakepi");
        fs::write(&tool, "#!/bin/sh\necho 'fakepi 1.2.3'\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

        let run = |command: &str| {
            let script = format!("set -- {}\n{PROBE_SCRIPT}", sh_quote(command));
            let out = Command::new("/bin/sh")
                .args(["-c", &script])
                .env("HOME", &home)
                .env("SHELL", "/bin/sh")
                .env_remove("ENV")
                .output()
                .unwrap();
            assert!(out.status.success());
            parse_probe(&String::from_utf8(out.stdout).unwrap()).unwrap()
        };
        let probe = run("~/bin/fakepi");
        assert_eq!(probe.home, home.to_str().unwrap());
        assert_eq!(probe.pi_path.as_deref(), tool.to_str());
        assert_eq!(probe.pi_version.as_deref(), Some("fakepi 1.2.3"));
        assert_ne!(probe.system, "");

        let missing = run("definitely-not-a-command-pigui");
        assert_eq!(missing.pi_path, None);
        assert_eq!(missing.pi_version, None);
    }

    #[test]
    fn parses_probe_output() {
        let stdout = "Welcome to Ubuntu!\nPIGUI_HOME=/home/me\nPIGUI_SYSTEM=Linux aarch64\n\
                      PIGUI_PI=/home/me/.nvm/versions/node/v22/bin/pi\nPIGUI_VERSION=0.45.3\n";
        assert_eq!(
            parse_probe(stdout).unwrap(),
            Probe {
                home: "/home/me".into(),
                pi_path: Some("/home/me/.nvm/versions/node/v22/bin/pi".into()),
                pi_version: Some("0.45.3".into()),
                system: "Linux aarch64".into(),
            }
        );
        let no_pi = parse_probe("PIGUI_HOME=/Users/me\nPIGUI_SYSTEM=Darwin arm64\n").unwrap();
        assert_eq!(no_pi.pi_path, None);
        assert_eq!(no_pi.pi_version, None);
        let empty_version = parse_probe("PIGUI_HOME=/h\nPIGUI_PI=/h/pi\nPIGUI_VERSION=\n").unwrap();
        assert_eq!(empty_version.pi_version, None);
        assert!(matches!(
            parse_probe("garbage"),
            Err(RemoteError::Failed(_))
        ));
    }

    #[test]
    fn parses_listing_output() {
        let listing = parse_listing(b"/home/me\0zeta\0Alpha\0beta\0alpha\0").unwrap();
        assert_eq!(
            listing,
            DirListing {
                path: "/home/me".into(),
                parent: Some("/home".into()),
                dirs: strings(&["Alpha", "alpha", "beta", "zeta"]),
            }
        );
        let root = parse_listing(b"/\0usr\0").unwrap();
        assert_eq!(root.parent, None);
        assert_eq!(parse_listing(b"/home\0").unwrap().parent, Some("/".into()));
        assert!(parse_listing(b"").is_err());
        assert!(parse_listing(b"relative\0").is_err());
    }

    #[test]
    fn parses_server_host_key() {
        let stderr = "debug1: Connecting to h [1.2.3.4] port 22.\n\
                      debug1: Server host key: ssh-ed25519 SHA256:voqbH8vJ0HAh163ILLUcx2s0xPOE\n\
                      debug1: checking without port identifier\n";
        assert_eq!(
            parse_server_host_key(stderr).as_deref(),
            Some("SHA256:voqbH8vJ0HAh163ILLUcx2s0xPOE (ssh-ed25519)")
        );
        assert_eq!(parse_server_host_key("nothing"), None);
    }

    /// The fallback used for hosts behind a jump host, checked against a direct one.
    #[test]
    #[ignore = "needs PI_GUI_SSH_TEST_HOST and network access"]
    fn live_fingerprint_via_ssh() {
        let Ok(alias) = std::env::var("PI_GUI_SSH_TEST_HOST") else {
            return;
        };
        let ssh = Ssh::new(Target::Alias { alias });
        let via_ssh = ssh.fingerprint_via_ssh().unwrap();
        let scanned = ssh.host_key_fingerprint().unwrap();
        eprintln!("via ssh: {via_ssh}\nscanned:\n{scanned}");
        let hash = via_ssh.split(' ').next().unwrap();
        assert!(scanned.contains(hash));
    }

    #[test]
    fn default_control_dir_fits_a_socket_path() {
        let dir = Ssh::default_control_dir();
        assert!(dir.is_absolute());
        assert!(
            dir.as_os_str().len() <= MAX_CONTROL_DIR_LEN,
            "{}",
            dir.display()
        );
    }

    #[test]
    fn private_dir_is_created_and_tightened() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a/cm");
        prepare_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);

        let file = root.path().join("file");
        fs::write(&file, "").unwrap();
        assert!(prepare_private_dir(&file).is_err());
    }
}
