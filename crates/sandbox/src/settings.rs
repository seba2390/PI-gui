//! The sandbox's settings, `<dir>/settings.json` (Settings → Security), read by the app and by
//! every sandboxed command, so changes apply to the next command.
//!
//! Paths may start with `~/` (the home folder); they are stored as the user wrote them.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The settings file in the sandbox's folder.
pub const FILE: &str = "settings.json";

/// Whether sandboxed commands may use the network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    /// Any host.
    #[default]
    On,
    /// None at all.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Commands run in the sandbox (where it is available).
    pub enabled: bool,
    pub network: Network,
    /// Commands may write to the caches of common tools ([`TOOL_CACHES`]).
    pub tool_caches: bool,
    /// More folders commands may write to.
    pub writable: Vec<String>,
    /// Files and folders commands may not read (or write).
    pub hidden: Vec<String>,
    /// Project folders whose sessions run without the sandbox (also their subfolders).
    pub unsandboxed: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            network: Network::On,
            tool_caches: true,
            writable: Vec::new(),
            hidden: DEFAULT_HIDDEN.iter().map(|&path| path.to_owned()).collect(),
            unsandboxed: Vec::new(),
        }
    }
}

/// Hidden from commands until the user says otherwise: keys, logins and credentials.
pub const DEFAULT_HIDDEN: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.azure",
    "~/.config/gcloud",
    "~/.gnupg",
    "~/.config/gh",
    "~/.docker/config.json",
    "~/.kube",
    "~/.netrc",
    "~/.git-credentials",
    "~/.pi/agent/auth.json",
    #[cfg(target_os = "macos")]
    "~/Library/Keychains",
];

/// The caches and package folders of common tools, writable while [`Settings::tool_caches`] is
/// on, so that builds and installs work.
pub const TOOL_CACHES: &[&str] = &[
    "~/.cargo",
    "~/.rustup",
    "~/.npm",
    "~/.yarn",
    "~/.pnpm-store",
    "~/.bun",
    "~/.deno",
    "~/.cache",
    "~/.gradle",
    "~/.m2",
    "~/.nuget",
    "~/go",
    "~/.local/share/pnpm",
    #[cfg(target_os = "macos")]
    "~/Library/Caches",
    #[cfg(target_os = "macos")]
    "~/Library/pnpm",
    #[cfg(target_os = "macos")]
    "~/Library/Developer/Xcode/DerivedData",
];

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// The saved settings, or the defaults when there are none (or they can't be read).
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves the settings (a whole new file, so a command never reads half of one).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let temp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&temp, text + "\n")?;
        std::fs::rename(temp, Self::path(dir))
    }

    /// Whether sessions in `project` run without the sandbox: it or a folder it is in is listed.
    pub fn is_unsandboxed(&self, project: &Path, home: Option<&Path>) -> bool {
        let project = crate::paths::resolve(project);
        self.unsandboxed.iter().any(|folder| {
            let folder = crate::paths::resolve(&crate::paths::expand(folder, home));
            project.starts_with(folder)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_are_the_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(Settings::load(dir.path()), Settings::default());
        assert!(Settings::default().enabled);
        assert!(Settings::default().hidden.contains(&"~/.ssh".to_owned()));
    }

    #[test]
    fn settings_round_trip_and_keep_unknown_fields_default() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings = Settings {
            enabled: false,
            network: Network::Off,
            tool_caches: false,
            writable: vec!["~/work/shared".into()],
            hidden: vec!["~/.ssh".into()],
            unsandboxed: vec!["/srv/app".into()],
        };
        settings.save(dir.path()).expect("save");
        assert_eq!(Settings::load(dir.path()), settings);
        std::fs::write(Settings::path(dir.path()), r#"{"network": "off"}"#).expect("write");
        let partial = Settings::load(dir.path());
        assert_eq!(partial.network, Network::Off);
        assert!(partial.enabled);
        assert_eq!(partial.hidden, Settings::default().hidden);
    }

    #[test]
    fn unsandboxed_covers_subfolders() {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("app");
        std::fs::create_dir_all(project.join("web")).expect("mkdir");
        let settings = Settings {
            unsandboxed: vec![project.to_string_lossy().into_owned()],
            ..Settings::default()
        };
        assert!(settings.is_unsandboxed(&project, None));
        assert!(settings.is_unsandboxed(&project.join("web"), None));
        assert!(!settings.is_unsandboxed(dir.path(), None));
    }
}
