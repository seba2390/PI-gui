//! macOS: the command runs under Seatbelt, the kernel's sandbox, through `sandbox-exec` with a
//! profile made from the [`Policy`]. Everything is allowed except writing outside the writable
//! folders, touching hidden ones and, when the network is off, Internet connections.

use std::fmt::Write as _;
use std::path::Path;

use crate::policy::Policy;
use crate::settings::Network;

/// The program that applies a profile and runs the command (part of macOS).
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The Seatbelt profile for `policy`. Later rules win over earlier ones: the private folders
/// are denied except what is open in them, then the folders on the way there may be looked up
/// and listed (macOS finds a working folder that way: `getcwd` fails without), not what is in
/// them, and the hidden paths are denied last, also inside open folders.
pub fn profile(policy: &Policy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");
    out.push_str("(deny file-write*\n  (require-all\n    (require-not (subpath \"/dev\"))\n");
    for root in &policy.writable {
        let _ = writeln!(out, "    (require-not (subpath {}))", quote(root));
    }
    out.push_str("  ))\n");
    let open = policy.open_in_private();
    for private in &policy.private {
        let _ = writeln!(
            out,
            "(deny file-read*\n  (require-all\n    (subpath {})",
            quote(private)
        );
        for root in &open {
            let _ = writeln!(out, "    (require-not (subpath {}))", quote(root));
        }
        out.push_str("  ))\n");
    }
    let passages = policy.passages();
    if !passages.is_empty() {
        out.push_str("(allow file-read-metadata file-read-data\n");
        for passage in &passages {
            let _ = writeln!(out, "  (literal {})", quote(passage));
        }
        out.push_str(")\n");
    }
    if !policy.hidden.is_empty() {
        out.push_str("(deny file-read* file-write*\n");
        for hidden in &policy.hidden {
            let _ = writeln!(out, "  (subpath {})", quote(hidden));
        }
        out.push_str(")\n");
    }
    if policy.network == Network::Off {
        // Local (Unix) sockets stay allowed: system services answer on them.
        out.push_str("(deny network-outbound (remote ip \"*:*\"))\n");
        out.push_str("(deny network-bind (local ip \"*:*\"))\n");
    }
    out
}

/// A path as a profile string.
fn quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn policy(network: Network) -> Policy {
        Policy {
            project: PathBuf::from("/Users/ana/code/app"),
            private: vec![PathBuf::from("/Users/ana")],
            readable: vec![PathBuf::from("/Users/ana/.gitconfig")],
            writable: vec![PathBuf::from("/Users/ana/code/app"), PathBuf::from("/tmp")],
            hidden: vec![PathBuf::from("/Users/ana/.ssh"), PathBuf::from("/x/\"q\"")],
            network,
        }
    }

    #[test]
    fn the_home_folder_is_private_except_what_is_open() {
        let profile = profile(&policy(Network::On));
        pretty_assertions::assert_eq!(
            profile,
            r#"(version 1)
(allow default)
(deny file-write*
  (require-all
    (require-not (subpath "/dev"))
    (require-not (subpath "/Users/ana/code/app"))
    (require-not (subpath "/tmp"))
  ))
(deny file-read*
  (require-all
    (subpath "/Users/ana")
    (require-not (subpath "/Users/ana/.gitconfig"))
    (require-not (subpath "/Users/ana/code/app"))
  ))
(allow file-read-metadata file-read-data
  (literal "/Users/ana")
  (literal "/Users/ana/code")
)
(deny file-read* file-write*
  (subpath "/Users/ana/.ssh")
  (subpath "/x/\"q\"")
)
"#
        );
    }

    #[test]
    fn network_off_denies_internet_connections() {
        let profile = profile(&policy(Network::Off));
        assert!(profile.contains("(deny network-outbound (remote ip \"*:*\"))"));
        assert!(profile.contains("(deny network-bind (local ip \"*:*\"))"));
    }

    /// The real thing: macOS applies the profile.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_enforces_the_profile() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonical");
        let home = root.join("home");
        let project = home.join("code/app");
        let other = home.join("code/other");
        let keys = home.join("allowed/.keys");
        for folder in [&project, &other, &keys] {
            std::fs::create_dir_all(folder).expect("mkdir");
        }
        std::fs::write(other.join("secret.txt"), "s").expect("write");
        std::fs::write(keys.join("key"), "k").expect("write");
        std::fs::write(home.join("allowed/notes.txt"), "n").expect("write");
        let policy = Policy {
            project: project.clone(),
            private: vec![home.clone()],
            readable: vec![home.join("allowed")],
            writable: vec![project.clone()],
            hidden: vec![keys.clone()],
            network: Network::Off,
        };
        let sh = |script: String| {
            std::process::Command::new(SANDBOX_EXEC)
                .arg("-p")
                .arg(profile(&policy))
                .args(["/bin/sh", "-c", &script])
                .current_dir(&project)
                .output()
                .expect("sandbox-exec runs")
                .status
                .success()
        };
        assert!(sh("echo ok > inside && pwd && git --version".to_owned()));
        assert!(!sh(format!("echo no > '{}/outside'", root.display())));
        assert!(!sh(format!("cat '{}/secret.txt'", other.display())));
        // The way to the project can be listed (getcwd needs it), nothing else in it read.
        assert!(sh("/bin/pwd -P".to_owned()));
        assert!(!sh(format!("ls '{}/code/other'", home.display())));
        assert!(sh(format!("cat '{}/allowed/notes.txt'", home.display())));
        assert!(!sh(format!(
            "echo x > '{}/allowed/notes.txt'",
            home.display()
        )));
        assert!(!sh(format!("cat '{}/key'", keys.display())));
        assert!(sh("cat /etc/hosts > /dev/null".to_owned()));
    }
}
