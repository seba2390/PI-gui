//! macOS: the command runs under Seatbelt, the kernel's sandbox, through `sandbox-exec` with a
//! profile made from the [`Policy`]. Everything is allowed except writing outside the writable
//! folders, touching hidden ones and, when the network is off, Internet connections.

use std::fmt::Write as _;
use std::path::Path;

use crate::policy::Policy;
use crate::settings::Network;

/// The program that applies a profile and runs the command (part of macOS).
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The Seatbelt profile for `policy`. Later rules win over earlier ones, so the hidden paths
/// are denied last, also inside writable folders.
pub fn profile(policy: &Policy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");
    out.push_str("(deny file-write*\n  (require-all\n    (require-not (subpath \"/dev\"))\n");
    for root in &policy.writable {
        let _ = writeln!(out, "    (require-not (subpath {}))", quote(root));
    }
    out.push_str("  ))\n");
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
            project: PathBuf::from("/Users/ana/app"),
            writable: vec![PathBuf::from("/Users/ana/app"), PathBuf::from("/tmp")],
            hidden: vec![PathBuf::from("/Users/ana/.ssh"), PathBuf::from("/x/\"q\"")],
            network,
        }
    }

    #[test]
    fn writes_only_to_writable_folders_and_hides_secrets() {
        let profile = profile(&policy(Network::On));
        pretty_assertions::assert_eq!(
            profile,
            r#"(version 1)
(allow default)
(deny file-write*
  (require-all
    (require-not (subpath "/dev"))
    (require-not (subpath "/Users/ana/app"))
    (require-not (subpath "/tmp"))
  ))
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
        let project = root.join("app");
        let secret = root.join("secret");
        std::fs::create_dir_all(&project).expect("mkdir");
        std::fs::create_dir_all(&secret).expect("mkdir");
        std::fs::write(secret.join("key"), "k").expect("write");
        let policy = Policy {
            project: project.clone(),
            writable: vec![project.clone()],
            hidden: vec![secret.clone()],
            network: Network::Off,
        };
        let sh = |script: String| {
            std::process::Command::new(SANDBOX_EXEC)
                .arg("-p")
                .arg(profile(&policy))
                .args(["/bin/sh", "-c", &script])
                .output()
                .expect("sandbox-exec runs")
                .status
                .success()
        };
        assert!(sh(format!("echo ok > '{}/inside'", project.display())));
        assert!(!sh(format!("echo no > '{}/outside'", root.display())));
        assert!(!root.join("outside").exists());
        assert!(!sh(format!("cat '{}/key'", secret.display())));
    }
}
