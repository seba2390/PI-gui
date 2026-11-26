//! The sandbox's settings, `<dir>/settings.json` (Settings → Sandboxing & Security), read by the app and by
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
    /// Any site.
    #[default]
    On,
    /// None at all.
    Off,
    /// Only [`Settings::sites`], through the app's proxy ([`crate::proxy`]).
    Custom,
}

/// A folder (or file) outside the project the agent may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowed {
    pub path: String,
    /// It may change it too, not only read it.
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Commands run in the sandbox (where it is available).
    pub enabled: bool,
    pub network: Network,
    /// Commands may use the folders where common tools keep their programs, settings and caches
    /// ([`TOOL_PROGRAMS`], [`TOOL_CACHES`]).
    #[serde(alias = "tool_caches")]
    pub tool_folders: bool,
    /// Folders and files outside the project the agent may use. The rest of the home folder is
    /// hidden.
    pub allowed: Vec<Allowed>,
    /// The sites commands may reach while the network is [`Network::Custom`]: `github.com`, or
    /// `github.com/*` with its subdomains ([`crate::sites`]).
    pub sites: Vec<String>,
    /// Project folders whose sessions run without the sandbox (also their subfolders).
    pub unsandboxed: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            network: Network::On,
            tool_folders: true,
            allowed: Vec::new(),
            sites: Vec::new(),
            unsandboxed: Vec::new(),
        }
    }
}

/// Keys, logins and credentials: hidden even inside a folder the user allowed (the home folder
/// itself, say).
pub const ALWAYS_HIDDEN: &[&str] = &[
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
    "~/.npmrc",
    "~/.pypirc",
    "~/.password-store",
    "~/.local/share/keyrings",
    "~/.pi/agent/auth.json",
    #[cfg(target_os = "macos")]
    "~/Library/Keychains",
];

/// Where common tools keep their programs and settings in the home folder: readable while
/// [`Settings::tool_folders`] is on, so that commands find their compilers and runtimes.
pub const TOOL_PROGRAMS: &[&str] = &[
    "~/.gitconfig",
    "~/.gitignore_global",
    "~/.config/git",
    "~/.nvm",
    "~/.volta",
    "~/.fnm",
    "~/.pyenv",
    "~/.rbenv",
    "~/.gem",
    "~/.asdf",
    "~/.sdkman",
    "~/.jenv",
    "~/.dotnet",
    "~/.local",
    "~/.config/mise",
    "~/.conda",
    "~/miniconda3",
    "~/anaconda3",
    "~/miniforge3",
    "~/.swiftpm",
    #[cfg(target_os = "macos")]
    "~/Library/Developer",
];

/// The caches and package folders of common tools, writable while [`Settings::tool_folders`] is
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
        assert_eq!(Settings::default().allowed, []);
    }

    #[test]
    fn settings_round_trip_and_keep_unknown_fields_default() {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings = Settings {
            enabled: false,
            network: Network::Off,
            tool_folders: false,
            allowed: vec![Allowed {
                path: "~/work/shared".into(),
                write: true,
            }],
            sites: vec!["github.com/*".into()],
            unsandboxed: vec!["/srv/app".into()],
        };
        settings.save(dir.path()).expect("save");
        assert_eq!(Settings::load(dir.path()), settings);
        std::fs::write(
            Settings::path(dir.path()),
            r#"{"network": "off", "tool_caches": false, "allowed": [{"path": "~/docs"}]}"#,
        )
        .expect("write");
        let partial = Settings::load(dir.path());
        assert_eq!(partial.network, Network::Off);
        assert!(partial.enabled);
        assert!(!partial.tool_folders);
        assert!(!partial.allowed[0].write);
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
