//! The host side of the model link: pi on a host uses this computer's models through
//! Pistachio's relay (see the app's `relay` module).
//!
//! The relay listens on a Unix socket here; [`Ssh::forward_to_socket`] makes it reachable on a
//! loopback port of the host over the shared connection, and [`Ssh::write_managed_agent`]
//! writes the pi settings folder that points pi there. The host's own `~/.pi` is never
//! changed: its extensions, skills, prompts and themes are linked into the managed folder.

use std::path::Path;
use std::time::Duration;

use crate::RemoteError;
use crate::error::meaningful_lines;
use crate::process;
use crate::ssh::{Ssh, script_result, sh_quote};

/// The pi settings folder Pistachio manages on a host (relative to `$HOME`).
pub const MANAGED_AGENT_DIR: &str = ".pistachio/agent";

const TIMEOUT: Duration = Duration::from_secs(20);

/// Creates `~/MANAGED_AGENT_DIR` (private), links the entries of the host's own agent folder
/// `$1` into it (but not its models, credentials, settings or sessions), writes `$2` as
/// `settings.json` and stdin as `models.json`, and prints the folder's path.
const WRITE_SCRIPT: &str = r#"umask 077
a=$1; m=$HOME/MANAGED_AGENT_DIR
mkdir -p -- "$m" && chmod 700 "$m" || exit 1
for e in "$m"/* "$m"/.[!.]*; do [ -L "$e" ] && rm -f -- "$e"; done
if [ "$a" != "$m" ]; then
  for e in "$a"/* "$a"/.[!.]*; do
    [ -e "$e" ] || continue
    n=${e##*/}
    case $n in models.json|auth.json|settings.json|models-store.json|sessions|*.bak-pistachio|*.pistachio-tmp) continue ;; esac
    ln -s -- "$e" "$m/$n"
  done
  # What pi looks for, linked even before it exists (the Skills page may create a folder or
  # trust a project).
  for n in extensions skills prompts themes trust.json; do
    [ -e "$m/$n" ] || [ -L "$m/$n" ] || ln -s -- "$a/$n" "$m/$n"
  done
fi
printf '%s' "$2" > "$m/settings.json.pistachio-tmp" && mv -f -- "$m/settings.json.pistachio-tmp" "$m/settings.json" || exit 1
cat > "$m/models.json.pistachio-tmp" && mv -f -- "$m/models.json.pistachio-tmp" "$m/models.json" || exit 1
printf '%s\n' "$m""#;

impl Ssh {
    /// The process id of the shared master connection (`ssh -O check`), or `None` when there
    /// is none. A different id than before means the connection was made again, and its
    /// port forwards are gone.
    pub fn master_pid(&self) -> Option<u32> {
        let mut command = self.ssh_command();
        command.args(["-O", "check"]).args(self.base_args());
        let output = process::run(command, None, TIMEOUT).ok()?;
        if !output.status.success() {
            return None;
        }
        parse_master_pid(&output.stderr_lossy())
    }

    /// Makes the Unix socket `socket` here reachable at `127.0.0.1:<port>` on the host, over
    /// the shared master connection (`ssh -O forward -R`). `port` 0 lets the host pick a free
    /// one. Returns the port. Fails when the host does not allow port forwarding.
    pub fn forward_to_socket(&self, port: u16, socket: &Path) -> Result<u16, RemoteError> {
        let mut command = self.ssh_command();
        command
            .args(["-O", "forward", "-R"])
            .arg(format!("127.0.0.1:{port}:{}", socket.display()))
            .args(self.base_args());
        let output = process::run(command, None, TIMEOUT)?;
        if !output.status.success() {
            let stderr = output.stderr_lossy();
            let lines = meaningful_lines(&stderr);
            return Err(RemoteError::Failed(if lines.is_empty() {
                "the host refused port forwarding".to_owned()
            } else {
                lines.join("\n")
            }));
        }
        if port != 0 {
            return Ok(port);
        }
        output
            .stdout_lossy()
            .trim()
            .parse()
            .map_err(|_| RemoteError::Failed("ssh did not report the forwarded port".to_owned()))
    }

    /// Writes the managed pi settings folder (see [`WRITE_SCRIPT`]) with `settings` and
    /// `models` as its `settings.json` and `models.json`, linking in the rest of the host's
    /// own agent folder `host_agent_dir`. Returns the folder's absolute path.
    pub fn write_managed_agent(
        &self,
        host_agent_dir: &str,
        settings: &str,
        models: Vec<u8>,
    ) -> Result<String, RemoteError> {
        let script = format!(
            "set -- {} {}\n{}",
            sh_quote(host_agent_dir),
            sh_quote(settings),
            WRITE_SCRIPT.replace("MANAGED_AGENT_DIR", MANAGED_AGENT_DIR)
        );
        let output = process::run(self.script_command(&script)?, Some(models), TIMEOUT)?;
        let path = script_result(output)?.stdout_lossy().trim().to_owned();
        if path.starts_with('/') {
            Ok(path)
        } else {
            Err(RemoteError::Failed(
                "could not write pi's settings for this computer's models".to_owned(),
            ))
        }
    }
}

fn parse_master_pid(stderr: &str) -> Option<u32> {
    let start = stderr.find("pid=")? + 4;
    let digits: String = stderr[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn reads_the_master_pid() {
        assert_eq!(
            parse_master_pid("Master running (pid=4242)\r\n"),
            Some(4242)
        );
        assert_eq!(parse_master_pid("No ControlPath specified"), None);
    }

    /// Runs the script the way `write_managed_agent` does, locally with a scratch `HOME`.
    fn write(home: &Path, agent: &Path, settings: &str, models: &str) -> String {
        let script = format!(
            "set -- {} {}\n{}",
            sh_quote(&agent.to_string_lossy()),
            sh_quote(settings),
            WRITE_SCRIPT.replace("MANAGED_AGENT_DIR", MANAGED_AGENT_DIR)
        );
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), models.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn managed_agent_folder_links_the_hosts_own_resources() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let agent = home.join(".pi/agent");
        fs::create_dir_all(agent.join("extensions")).unwrap();
        fs::create_dir_all(agent.join("sessions")).unwrap();
        for name in ["auth.json", "models.json", "settings.json", "AGENTS.md"] {
            fs::write(agent.join(name), "host").unwrap();
        }

        let managed = write(&home, &agent, r#"{"a":1}"#, r#"{"providers":{}}"#);
        let managed = Path::new(&managed);
        assert_eq!(managed, home.join(MANAGED_AGENT_DIR));
        assert_eq!(
            fs::metadata(managed).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::read_to_string(managed.join("settings.json")).unwrap(),
            r#"{"a":1}"#
        );
        assert_eq!(
            fs::read_to_string(managed.join("models.json")).unwrap(),
            r#"{"providers":{}}"#
        );
        assert_eq!(
            fs::metadata(managed.join("models.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_link(managed.join("extensions")).unwrap(),
            agent.join("extensions")
        );
        assert_eq!(
            fs::read_to_string(managed.join("AGENTS.md")).unwrap(),
            "host"
        );
        // Not yet there on the host, but linked for when it is.
        assert_eq!(
            fs::read_link(managed.join("skills")).unwrap(),
            agent.join("skills")
        );
        assert_eq!(
            fs::read_link(managed.join("trust.json")).unwrap(),
            agent.join("trust.json")
        );
        // The host's credentials and sessions stay out.
        assert!(!managed.join("auth.json").exists());
        assert!(!managed.join("sessions").exists());

        // Writing again drops links to what the host no longer has.
        fs::remove_file(agent.join("AGENTS.md")).unwrap();
        write(&home, &agent, "{}", "{}");
        assert!(fs::symlink_metadata(managed.join("AGENTS.md")).is_err());
        assert_eq!(
            fs::read_to_string(agent.join("models.json")).unwrap(),
            "host"
        );
    }
}
