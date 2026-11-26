//! What Settings → Security shows and changes: the sandbox's settings ([`sandbox::Settings`]),
//! which sandbox this computer has, the self-test, and pi's project trust (its default and the
//! saved decisions in `trust.json`).
//!
//! The sandbox's settings are a file that each sandboxed command reads, so a change applies to
//! running sessions from their next command. Checking the sandbox and the test run the app's own
//! helper (`--sandbox-tool status` / `test`) in the background.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use sandbox::{Settings, Status, TestReport};

use crate::config::AppConfig;

/// pi's `defaultProjectTrust`: what it does with a project's `.pi` files without a saved decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrustDefault {
    /// pi asks; in Pistachio (pi in RPC mode, which can't ask) that loads nothing.
    #[default]
    Ask,
    Always,
    Never,
}

impl TrustDefault {
    pub const ALL: [Self; 3] = [Self::Ask, Self::Always, Self::Never];

    pub fn id(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Load only after I trust the project",
            Self::Always => "Always load",
            Self::Never => "Never load",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|value| value.id() == id)
            .unwrap_or_default()
    }
}

/// The self-test.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Test {
    #[default]
    NotRun,
    Running,
    Done(TestReport),
    Failed(String),
}

/// How a session's commands run, for the badge in its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSandbox {
    Sandboxed {
        network: bool,
    },
    /// The user turned it off (for all or this project).
    Off,
    /// This computer has no sandbox the app can use.
    Unavailable,
    /// Not known yet (being checked).
    Checking,
}

pub struct Security {
    dir: PathBuf,
    exe: Option<PathBuf>,
    pub settings: Settings,
    /// `None` while it is checked.
    pub status: Option<Status>,
    pub test: Test,
    pub trust_default: TrustDefault,
    /// Saved trust decisions: folder, trusted.
    pub trusted: Vec<(String, bool)>,
    /// The last thing that could not be saved.
    pub error: Option<String>,
    _check: Task<()>,
    _test: Task<()>,
}

struct SecurityHandle(Entity<Security>);

impl Global for SecurityHandle {}

/// After [`AppConfig`] is set.
pub fn security(cx: &mut App) -> Entity<Security> {
    if let Some(handle) = cx.try_global::<SecurityHandle>() {
        return handle.0.clone();
    }
    let config = cx.global::<AppConfig>();
    let (dir, exe) = (config.sandbox_dir.clone(), config.exe.clone());
    let security = cx.new(|cx| {
        let mut security = Security {
            settings: Settings::load(&dir),
            dir,
            exe,
            status: None,
            test: Test::NotRun,
            trust_default: TrustDefault::Ask,
            trusted: Vec::new(),
            error: None,
            _check: Task::ready(()),
            _test: Task::ready(()),
        };
        security.load_trust();
        security.check(cx);
        security
    });
    cx.set_global(SecurityHandle(security.clone()));
    security
}

/// The entity, once something asked for it.
pub fn existing(cx: &App) -> Option<Entity<Security>> {
    cx.try_global::<SecurityHandle>()
        .map(|handle| handle.0.clone())
}

impl Security {
    /// Reads the settings and trust again and checks which sandbox works (the page opened).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.settings = Settings::load(&self.dir);
        self.load_trust();
        self.check(cx);
        cx.notify();
    }

    /// Checks which sandbox works here, in the background.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if !sandbox::SUPPORTED {
            self.status = Some(Status {
                backend: None,
                network: false,
                problem: Some("there is no sandbox on Windows yet".into()),
                fix: None,
            });
            return;
        }
        let Some(exe) = self.exe.clone().filter(|exe| exe.exists()) else {
            self.status = Some(Status {
                backend: None,
                network: false,
                problem: Some("the app's own executable could not be found".into()),
                fix: None,
            });
            return;
        };
        self.status = None;
        let (dir, exe) = (self.dir.clone(), Some(exe));
        self._check = cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async move { helper::<Status>(exe.as_deref(), &dir, "status") })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status = Some(status.unwrap_or_else(|error| Status {
                    backend: None,
                    network: false,
                    problem: Some(error),
                    fix: None,
                }));
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Changes the sandbox's settings and saves them.
    pub fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        if self.settings == before {
            return;
        }
        let (settings, dir) = (self.settings.clone(), self.dir.clone());
        self.error = None;
        crate::persist::save_in_background(cx, Settings::path(&dir), move |_| settings.save(&dir));
        cx.notify();
    }

    /// Tries what the sandbox should allow and block, in the background.
    pub fn run_test(&mut self, cx: &mut Context<Self>) {
        if self.test == Test::Running {
            return;
        }
        self.test = Test::Running;
        let (dir, exe, settings) = (self.dir.clone(), self.exe.clone(), self.settings.clone());
        self._test = cx.spawn(async move |this, cx| {
            let report = cx
                .background_spawn(async move {
                    // The test reads the saved settings: what is shown must be on disk first.
                    settings.save(&dir).map_err(|error| error.to_string())?;
                    helper::<TestReport>(exe.as_deref(), &dir, "test")
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.test = match report {
                    Ok(report) => {
                        this.status = Some(report.status.clone());
                        Test::Done(report)
                    }
                    Err(error) => Test::Failed(error),
                };
                cx.notify();
            });
        });
        cx.notify();
    }

    /// How sessions in `project` run their commands.
    pub fn session(&self, project: &Path) -> SessionSandbox {
        if !sandbox::SUPPORTED {
            return SessionSandbox::Unavailable;
        }
        if !self.settings.enabled
            || self
                .settings
                .is_unsandboxed(project, sandbox::paths::home().as_deref())
        {
            return SessionSandbox::Off;
        }
        match &self.status {
            None => SessionSandbox::Checking,
            Some(status) if status.backend.is_none() => SessionSandbox::Unavailable,
            Some(status) => SessionSandbox::Sandboxed {
                network: self.settings.network == sandbox::Network::On || !status.network,
            },
        }
    }

    /// Turns the sandbox off (or on again) for `project`.
    pub fn set_project_unsandboxed(&mut self, project: &Path, off: bool, cx: &mut Context<Self>) {
        let folder = project.to_string_lossy().into_owned();
        let home = sandbox::paths::home();
        self.update(cx, |settings| {
            if off {
                if !settings.is_unsandboxed(project, home.as_deref()) {
                    settings.unsandboxed.push(folder);
                }
            } else {
                // Also a listed parent: on for the project means on for it.
                settings.unsandboxed.retain(|listed| {
                    let listed =
                        sandbox::paths::resolve(&sandbox::paths::expand(listed, home.as_deref()));
                    !sandbox::paths::resolve(project).starts_with(listed)
                });
            }
        });
    }

    fn agent_dir() -> Option<PathBuf> {
        pi_sessions::agent_dir()
    }

    fn load_trust(&mut self) {
        let Some(agent) = Self::agent_dir() else {
            return;
        };
        self.trust_default = std::fs::read_to_string(agent.join("settings.json"))
            .ok()
            .and_then(|text| pi_config::trust::default_project_trust(&text))
            .map_or_else(TrustDefault::default, |value| TrustDefault::from_id(&value));
        self.trusted = std::fs::read_to_string(agent.join("trust.json"))
            .ok()
            .and_then(|text| pi_config::trust::parse_store(&text).ok())
            .map(|store| {
                store
                    .into_iter()
                    .filter_map(|(folder, value)| value.as_bool().map(|trusted| (folder, trusted)))
                    .collect()
            })
            .unwrap_or_default();
        self.trusted.sort();
    }

    /// Saves pi's `defaultProjectTrust`.
    pub fn set_trust_default(&mut self, value: TrustDefault, cx: &mut Context<Self>) {
        let Some(path) = Self::agent_dir().map(|agent| agent.join("settings.json")) else {
            return;
        };
        self.trust_default = value;
        let saved = (|| -> Result<(), String> {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.to_string()),
            };
            // pi's default needs no setting.
            let setting = (value != TrustDefault::Ask).then(|| value.id());
            let text = pi_config::settings::with_default_project_trust(&text, setting)
                .map_err(|error| error.to_string())?;
            write_replacing(&path, &text)
        })();
        self.error = saved
            .err()
            .map(|error| format!("Could not save pi's settings: {error}"));
        cx.notify();
    }

    /// Removes the saved trust decision for `folder` (pi decides by the default again).
    pub fn forget_trust(&mut self, folder: &str, cx: &mut Context<Self>) {
        let Some(agent) = Self::agent_dir() else {
            return;
        };
        let saved = write_trust(&agent, folder, None);
        self.error = saved
            .err()
            .map(|error| format!("Could not save trust.json: {error}"));
        self.load_trust();
        cx.notify();
    }
}

/// `path` with the home folder written `~`.
pub fn tilde(path: &Path) -> String {
    match sandbox::paths::home()
        .and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The folders to offer allowing for a call the sandbox blocked: for each path it was refused
/// (`paths`, absolute or relative to `project`), the folder to write in, unless that is hidden,
/// already allowed, the home folder or the root.
pub fn folders_to_allow(settings: &Settings, project: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let cx = sandbox::Context::current();
    let Some(policy) = sandbox::Policy::for_project(settings, project, &cx) else {
        return Vec::new();
    };
    let home = cx.home.as_deref().map(sandbox::paths::resolve);
    let mut folders = Vec::new();
    for path in paths {
        let path = sandbox::paths::resolve(&project.join(path));
        if !policy.may_read(&path) {
            continue;
        }
        let folder = if path.is_dir() {
            path.clone()
        } else {
            match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => continue,
            }
        };
        let useless = folder.parent().is_none()
            || home.as_ref() == Some(&folder)
            || policy.may_write(&folder.join(".pistachio-check"));
        if !useless && !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders.truncate(2);
    folders
}

/// Runs the helper's `command` and reads its JSON reply.
fn helper<T: serde::de::DeserializeOwned>(
    exe: Option<&Path>,
    dir: &Path,
    command: &str,
) -> Result<T, String> {
    let exe = exe.ok_or("the app's own executable could not be found")?;
    let output = std::process::Command::new(exe)
        .args([sandbox::cli::FLAG, command])
        .env(sandbox::DIR_ENV, dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("the sandbox could not be checked: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(text.trim().lines().last().unwrap_or(""))
        .map_err(|_| "the sandbox could not be checked".to_owned())
}

/// Writes `text` to `path` whole (a new file moved over the old one, keeping its permissions).
fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension("json.pistachio-tmp");
    std::fs::write(&temp, text).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

/// Saves `decision` for `folder` in pi's `trust.json`, under pi's lock (a `trust.json.lock`
/// folder), as the Skills page does on any host.
fn write_trust(agent: &Path, folder: &str, decision: Option<bool>) -> Result<(), String> {
    let path = agent.join("trust.json");
    let lock = agent.join("trust.json.lock");
    let mut tries = 0;
    while std::fs::create_dir(&lock).is_err() {
        tries += 1;
        if tries > 50 {
            return Err("trust.json is locked by another pi process".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let result = (|| {
        let store = match std::fs::read_to_string(&path) {
            Ok(text) => pi_config::trust::parse_store(&text).map_err(|error| error.to_string())?,
            Err(_) => serde_json::Map::new(),
        };
        let text = pi_config::trust::set_trust(&store, folder, decision);
        write_replacing(&path, &text)
    })();
    let _ = std::fs::remove_dir(&lock);
    result
}

#[cfg(test)]
mod tests {
    use super::{TrustDefault, write_trust};

    #[test]
    fn forgetting_trust_keeps_other_decisions() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join("trust.json"),
            r#"{"/a": true, "/b": false}"#,
        )
        .expect("write");
        write_trust(dir.path(), "/a", None).expect("saved");
        let text = std::fs::read_to_string(dir.path().join("trust.json")).expect("read");
        assert_eq!(text, "{\n  \"/b\": false\n}\n");
        assert!(!dir.path().join("trust.json.lock").exists());
    }

    #[test]
    fn trust_defaults_round_trip() {
        for value in TrustDefault::ALL {
            assert_eq!(TrustDefault::from_id(value.id()), value);
        }
        assert_eq!(TrustDefault::from_id("odd"), TrustDefault::Ask);
    }
}
