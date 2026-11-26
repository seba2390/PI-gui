//! Linux: the command runs under bubblewrap (`bwrap`), which gives it its own view of the
//! files: everything read-only, the writable folders mounted writable on top, hidden folders
//! replaced by empty ones and hidden files by an empty file. With the network off it gets a
//! network of its own with nothing in it.

use std::ffi::OsString;

use crate::policy::Policy;
use crate::settings::Network;

/// bubblewrap's arguments for `policy`, up to and including the `--` before the command.
/// Writable folders that don't exist can't be mounted and are left out.
pub fn args(policy: &Policy) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&dyn AsRef<std::ffi::OsStr>]| {
        args.extend(items.iter().map(|item| item.as_ref().to_owned()));
    };
    push(&[&"--ro-bind", &"/", &"/"]);
    push(&[&"--dev", &"/dev"]);
    // Its own processes: when bubblewrap ends, everything the command started ends with it.
    push(&[&"--unshare-pid", &"--proc", &"/proc", &"--die-with-parent"]);
    if policy.network == Network::Off {
        push(&[&"--unshare-net"]);
    }
    for root in &policy.writable {
        // `--dev` made a new /dev (with its own /dev/shm).
        if root.starts_with("/dev") || root.starts_with("/proc") || !root.exists() {
            continue;
        }
        push(&[&"--bind", root, root]);
    }
    for hidden in &policy.hidden {
        if hidden.is_dir() {
            push(&[&"--tmpfs", hidden]);
        } else if hidden.exists() {
            push(&[&"--ro-bind", &"/dev/null", hidden]);
        }
    }
    push(&[&"--chdir", &policy.project]);
    push(&[&"--"]);
    args
}

/// Whether bubblewrap at `bwrap` can make a sandbox here: it is missing permissions on some
/// systems (Ubuntu's `AppArmor`, no user namespaces in a container).
#[cfg(target_os = "linux")]
pub fn works(bwrap: &std::path::Path) -> bool {
    let true_program = ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .unwrap_or("true");
    std::process::Command::new(bwrap)
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--unshare-net",
            "--",
            true_program,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn mounts_writable_folders_over_a_read_only_root_and_hides_secrets() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonical");
        let project = root.join("app");
        let ssh = root.join(".ssh");
        let token = root.join("token");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::create_dir_all(&ssh).expect("mkdir");
        std::fs::write(&token, "secret").expect("write");
        let policy = Policy {
            project: project.clone(),
            writable: vec![
                project.clone(),
                root.join("missing-cache"),
                PathBuf::from("/dev/shm"),
            ],
            hidden: vec![ssh.clone(), token.clone(), root.join("missing")],
            network: Network::Off,
        };
        let args: Vec<String> = args(&policy)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let p = |path: &Path| path.to_string_lossy().into_owned();
        let expected: Vec<String> = [
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--die-with-parent",
            "--unshare-net",
            "--bind",
            &p(&project),
            &p(&project),
            "--tmpfs",
            &p(&ssh),
            "--ro-bind",
            "/dev/null",
            &p(&token),
            "--chdir",
            &p(&project),
            "--",
        ]
        .map(str::to_owned)
        .to_vec();
        pretty_assertions::assert_eq!(args, expected);
    }
}
