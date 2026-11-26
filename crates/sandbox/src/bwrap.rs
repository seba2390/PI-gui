//! Linux: the command runs under bubblewrap (`bwrap`), which gives it its own view of the
//! files: everything read-only, the home folder replaced by an empty one with only what is open
//! in it mounted back (read-only, or writable), the writable folders mounted writable, hidden
//! folders replaced by empty ones and hidden files by an empty file. With the network off it gets
//! a network of its own with nothing in it.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::policy::Policy;
use crate::settings::Network;

/// bubblewrap's arguments for `policy`, up to and including the `--` before the command. What
/// doesn't exist can't be mounted and is left out. `extra` are mounted as well, writable or not
/// (the proxy's socket and the helper that bridges to it).
pub fn args(policy: &Policy, extra: &[(PathBuf, bool)]) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&dyn AsRef<std::ffi::OsStr>]| {
        args.extend(items.iter().map(|item| item.as_ref().to_owned()));
    };
    push(&[&"--ro-bind", &"/", &"/"]);
    push(&[&"--dev", &"/dev"]);
    // Its own processes: when bubblewrap ends, everything the command started ends with it.
    push(&[&"--unshare-pid", &"--proc", &"/proc", &"--die-with-parent"]);
    // Off, or only the proxy: a network of its own (the proxy is reached through `extra`).
    if policy.network != Network::On {
        push(&[&"--unshare-net"]);
    }
    for private in &policy.private {
        push(&[&"--tmpfs", private]);
    }
    // Writable wins over read-only for the same folder and what is inside it; outer folders are
    // mounted before inner ones.
    let mut mounts: Vec<(PathBuf, bool)> = policy
        .writable
        .iter()
        .map(|path| (path.clone(), true))
        .chain(
            policy
                .readable
                .iter()
                .filter(|path| !policy.writable.iter().any(|root| path.starts_with(root)))
                .map(|path| (path.clone(), false)),
        )
        // `--dev` made a new /dev (with its own /dev/shm).
        .filter(|(path, _)| {
            !path.starts_with("/dev") && !path.starts_with("/proc") && path.exists()
        })
        .collect();
    mounts.sort_by_key(|(path, _)| path.components().count());
    for (path, write) in &mounts {
        // Outside the private folders, read-only is what everything already is.
        let in_private = policy
            .private
            .iter()
            .any(|private| path.starts_with(private));
        if *write {
            push(&[&"--bind", path, path]);
        } else if in_private {
            push(&[&"--ro-bind", path, path]);
        }
    }
    for hidden in &policy.hidden {
        // Under a private folder only what was mounted back can be seen.
        let visible = !policy
            .private
            .iter()
            .any(|private| hidden.starts_with(private))
            || mounts.iter().any(|(path, _)| hidden.starts_with(path));
        if !visible {
            continue;
        }
        if hidden.is_dir() {
            push(&[&"--tmpfs", hidden]);
        } else if hidden.exists() {
            push(&[&"--ro-bind", &"/dev/null", hidden]);
        }
    }
    for (path, write) in extra.iter().filter(|(path, _)| path.exists()) {
        push(&[if *write { &"--bind" } else { &"--ro-bind" }, path, path]);
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
    fn hides_the_home_folder_and_mounts_back_what_is_open() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let project = home.join("app");
        let gitconfig = home.join(".gitconfig");
        let local = home.join(".local");
        let pnpm = home.join(".local/share/pnpm");
        let ssh = home.join(".ssh");
        let token = home.join(".local/token");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::create_dir_all(&pnpm).expect("mkdir");
        std::fs::create_dir_all(&ssh).expect("mkdir");
        std::fs::write(&gitconfig, "").expect("write");
        std::fs::write(&token, "secret").expect("write");
        let policy = Policy {
            project: project.clone(),
            private: vec![home.clone()],
            readable: vec![
                gitconfig.clone(),
                local.clone(),
                PathBuf::from("/opt/tools"),
            ],
            writable: vec![
                project.clone(),
                pnpm.clone(),
                home.join("missing-cache"),
                PathBuf::from("/dev/shm"),
            ],
            hidden: vec![ssh.clone(), token.clone()],
            network: Network::Off,
            proxy: None,
        };
        let args: Vec<String> = args(&policy, &[])
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
            "--tmpfs",
            &p(&home),
            "--bind",
            &p(&project),
            &p(&project),
            "--ro-bind",
            &p(&gitconfig),
            &p(&gitconfig),
            "--ro-bind",
            &p(&local),
            &p(&local),
            "--bind",
            &p(&pnpm),
            &p(&pnpm),
            // ~/.ssh was not mounted back: nothing to hide.
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
