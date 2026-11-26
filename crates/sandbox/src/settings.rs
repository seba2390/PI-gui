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
    On,
    /// No site without asking: through the app's proxy ([`crate::proxy`]), which asks the user
    /// the first time a session's commands reach a site. Without the proxy, no network at all.
    #[default]
    Off,
    /// [`Settings::sites`] without asking, others asked about, through the app's proxy.
    Custom,
}

impl Network {
    /// Whether commands go through the app's proxy.
    pub fn uses_proxy(self) -> bool {
        self != Self::On
    }
}

/// The sites a limited network starts with: where code and packages come from, so that git and
/// the common package managers work.
pub const DEFAULT_SITES: &[&str] = &[
    "github.com/*",
    "githubusercontent.com/*",
    "gitlab.com",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    "proxy.golang.org",
    "sum.golang.org",
    "rubygems.org",
    "repo.maven.apache.org",
];

/// A folder (or file) outside the project the agent may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowed {
    pub path: String,
    /// It may be read. Off, the entry stays in the list but allows nothing.
    #[serde(default = "yes")]
    pub read: bool,
    /// It may be changed too (which needs [`Self::read`]).
    #[serde(default)]
    pub write: bool,
}

fn yes() -> bool {
    true
}

impl Allowed {
    /// Readable, and writable when `write`.
    pub fn new(path: impl Into<String>, write: bool) -> Self {
        Self {
            path: path.into(),
            read: true,
            write,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Commands run in the sandbox (where it is available).
    pub enabled: bool,
    pub network: Network,
    /// Commands may use the folders where tools keep their programs, settings and caches.
    #[serde(alias = "tool_caches")]
    pub tool_folders: bool,
    /// Those folders, once the user changed the list; until then what [`default_tools`] finds.
    pub tools: Option<Vec<Allowed>>,
    /// Folders and files outside the project the agent may use. The rest of the home folder is
    /// hidden.
    pub allowed: Vec<Allowed>,
    /// The sites commands may reach while the network is [`Network::Custom`]: `github.com`, or
    /// `github.com/*` with its subdomains ([`crate::sites`]).
    pub sites: Vec<String>,
    /// Project folders whose sessions run without the sandbox (also their subfolders).
    pub unsandboxed: Vec<String>,
    /// What the gate may do with the user's keys without asking: per project and remote
    /// ([`crate::gate`]).
    pub gate: Vec<crate::gate::Rule>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            network: Network::Off,
            tool_folders: true,
            tools: None,
            allowed: Vec::new(),
            sites: DEFAULT_SITES.iter().map(|&site| site.to_owned()).collect(),
            unsandboxed: Vec::new(),
            gate: Vec::new(),
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

/// git's own settings: always readable (also with the tool folders off or changed), since git
/// stops on a settings file it can't read, and without them commits get a made-up author.
pub const GIT_SETTINGS: &[&str] = &["~/.gitconfig", "~/.gitignore_global", "~/.config/git"];

/// Where common tools keep their programs and settings in the home folder: readable while
/// [`Settings::tool_folders`] is on, so that commands find their compilers and runtimes.
pub const TOOL_PROGRAMS: &[&str] = &[
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

/// The tool folders this computer has: the caches of [`TOOL_CACHES`] (to change), the programs and
/// settings of [`TOOL_PROGRAMS`] and the folders of `$PATH` in the home folder (to read; for
/// `…/bin`, the folder it is in, which holds its libraries). With `existing_only`, only what is
/// there; else also what tools may create later. Never the home folder itself.
pub fn default_tools(cx: &crate::Context, existing_only: bool) -> Vec<Allowed> {
    use crate::paths::{expand, resolve};
    let Some(home) = cx.home.as_deref() else {
        return Vec::new();
    };
    let real_home = resolve(home);
    let exists = |path: &str| !existing_only || expand(path, Some(home)).exists();
    let mut tools: Vec<Allowed> = TOOL_CACHES
        .iter()
        .filter(|path| exists(path))
        .map(|&path| Allowed::new(path, true))
        .chain(
            TOOL_PROGRAMS
                .iter()
                .filter(|path| exists(path))
                .map(|&path| Allowed::new(path, false)),
        )
        .collect();
    for dir in &cx.path {
        let dir = resolve(dir);
        if !dir.starts_with(&real_home) || dir == real_home || (existing_only && !dir.exists()) {
            continue;
        }
        let dir = match dir.parent() {
            Some(parent)
                if dir.file_name().is_some_and(|name| name == "bin") && parent != real_home =>
            {
                parent.to_path_buf()
            }
            _ => dir,
        };
        let shown = match dir.strip_prefix(&real_home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => dir.display().to_string(),
        };
        let covered = tools.iter().any(|tool| {
            let tool = resolve(&expand(&tool.path, Some(home)));
            dir.starts_with(tool)
        });
        if !covered {
            tools.push(Allowed::new(shown, false));
        }
    }
    tools
}

impl Settings {
    /// The tool folders in force: the user's list, or what this computer has.
    pub fn tool_list(&self, cx: &crate::Context, existing_only: bool) -> Vec<Allowed> {
        self.tools
            .clone()
            .unwrap_or_else(|| default_tools(cx, existing_only))
    }

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
            tools: Some(vec![Allowed::new("~/.cargo", true)]),
            allowed: vec![Allowed::new("~/work/shared", true)],
            sites: vec!["github.com/*".into()],
            unsandboxed: vec!["/srv/app".into()],
            gate: Vec::new(),
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
        assert!(partial.allowed[0].read && !partial.allowed[0].write);
    }

    #[test]
    fn the_tool_list_is_what_this_computer_has() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        for folder in [".cargo", ".nvm/versions/node/v24/bin", ".local/bin", "bin"] {
            std::fs::create_dir_all(home.join(folder)).expect("mkdir");
        }
        std::fs::write(home.join(".gitconfig"), "").expect("write");
        let cx = crate::Context {
            home: Some(home.clone()),
            temp: Vec::new(),
            agent_dir: None,
            path: vec![
                home.join(".nvm/versions/node/v24/bin"),
                home.join(".local/bin"),
                home.join("bin"),
                home.clone(),
                PathBuf::from("/usr/bin"),
            ],
        };
        let tools = default_tools(&cx, true);
        let shown: Vec<(&str, bool)> = tools
            .iter()
            .map(|tool| (tool.path.as_str(), tool.write))
            .collect();
        pretty_assertions::assert_eq!(
            shown,
            [
                ("~/.cargo", true),
                ("~/.nvm", false),
                ("~/.local", false),
                // `~/bin`'s folder is the home folder, so it stays itself; `~` is never one.
                ("~/bin", false),
            ]
        );
        assert!(default_tools(&cx, false).len() > tools.len());
        let changed = Settings {
            tools: Some(vec![Allowed::new("~/.cargo", false)]),
            ..Settings::default()
        };
        assert_eq!(changed.tool_list(&cx, true).len(), 1);
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
