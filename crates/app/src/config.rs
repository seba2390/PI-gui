//! Process-wide configuration, resolved once at startup.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use gpui_kit::Global;

/// Extra environment for child processes: the login shell's (see [`crate::shell_env`]),
/// probed in the background at startup and waited for only by the first child process
/// that starts before the probe has finished.
#[derive(Clone, Default)]
pub struct ChildEnv(Arc<ChildEnvInner>);

type Vars = Vec<(OsString, OsString)>;

#[derive(Default)]
struct ChildEnvInner {
    probe: Mutex<Option<std::thread::JoinHandle<Vars>>>,
    vars: OnceLock<Vars>,
}

impl ChildEnv {
    /// Starts probing the login shell on a background thread.
    pub fn probe() -> Self {
        let probe = std::thread::Builder::new()
            .name("login-shell-env".into())
            .spawn(crate::shell_env::login_shell_env)
            .map_err(|error| log::warn!("could not probe the login shell: {error}"))
            .ok();
        Self(Arc::new(ChildEnvInner {
            probe: Mutex::new(probe),
            vars: OnceLock::new(),
        }))
    }

    /// The variables, waiting for the probe if it is still running.
    pub fn vars(&self) -> &[(OsString, OsString)] {
        self.0.vars.get_or_init(|| {
            let probe = self
                .0
                .probe
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            probe
                .and_then(|probe| probe.join().ok())
                .unwrap_or_default()
        })
    }
}

impl From<Vars> for ChildEnv {
    fn from(vars: Vars) -> Self {
        Self(Arc::new(ChildEnvInner {
            probe: Mutex::new(None),
            vars: OnceLock::from(vars),
        }))
    }
}

impl std::fmt::Debug for ChildEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.vars.get() {
            Some(vars) => f.debug_tuple("ChildEnv").field(vars).finish(),
            None => f.write_str("ChildEnv(probing)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// The `pi` executable. Override with `PISTACHIO_PI_BIN`.
    pub pi_program: PathBuf,
    /// Extra environment for child processes (login-shell `PATH`, …).
    pub child_env: ChildEnv,
    /// pi's session directory, if it could be determined.
    pub sessions_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// The folder chat-mode sessions run in (see [`pi_sessions::Mode`]).
    pub chat_dir: PathBuf,
    /// Deep search's settings, research instructions and pi extension (see [`deepsearch`]).
    pub deepsearch_dir: PathBuf,
    /// Where the skills that come with the app are written (see [`crate::builtin_skills`]).
    pub builtin_skills_dir: PathBuf,
    /// This executable, which runs deep search's web tools for pi.
    pub exe: Option<PathBuf>,
    /// Panels to open at startup (`PISTACHIO_OPEN=terminal,diff,files`), for scripted
    /// screenshots and responsiveness checks.
    pub open_at_startup: Vec<String>,
    /// The storyboard to play (`PISTACHIO_DEMO=<file.toml>`, see [`crate::demo`]).
    pub demo: Option<Arc<crate::demo::Storyboard>>,
}

impl Global for AppConfig {}

/// Recently used working folders (newest first), kept current by the sidebar's session scan
/// so the composer's folder menu opens instantly without touching the disk.
#[derive(Debug, Clone, Default)]
pub struct RecentFolders(pub Vec<PathBuf>);

impl Global for RecentFolders {}

/// How many recent folders the folder menu offers.
pub const RECENT_FOLDERS: usize = 8;

impl AppConfig {
    /// The mode of a session running in `cwd`.
    pub fn mode_of(&self, cwd: &std::path::Path) -> pi_sessions::Mode {
        pi_sessions::Mode::of(cwd, &self.chat_dir)
    }

    /// A config for tests: no login-shell environment, temp-dir folders.
    #[cfg(test)]
    pub fn for_tests(pi_program: PathBuf) -> Self {
        Self {
            pi_program,
            child_env: ChildEnv::default(),
            sessions_dir: None,
            home: None,
            chat_dir: std::env::temp_dir().join("pistachio-test-chat"),
            deepsearch_dir: std::env::temp_dir().join("pistachio-test-deepsearch"),
            builtin_skills_dir: std::env::temp_dir().join("pistachio-test-skills"),
            exe: Some(PathBuf::from("/app/pistachio")),
            open_at_startup: Vec::new(),
            demo: None,
        }
    }

    /// `child_env` comes from [`ChildEnv::probe`].
    pub fn detect(child_env: ChildEnv) -> Self {
        let home = dirs_home();
        Self {
            pi_program: std::env::var_os("PISTACHIO_PI_BIN")
                .map_or_else(default_pi_program, PathBuf::from),
            child_env,
            sessions_dir: pi_sessions::sessions_dir(),
            home,
            chat_dir: pi_sessions::chat_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("pistachio-chat")),
            deepsearch_dir: deepsearch::config::default_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("pistachio-deepsearch")),
            builtin_skills_dir: pi_sessions::app_dir().map_or_else(
                || std::env::temp_dir().join("pistachio-skills"),
                |dir| dir.join("skills"),
            ),
            exe: std::env::current_exe().ok(),
            open_at_startup: std::env::var("PISTACHIO_OPEN")
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|panel| !panel.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            demo: demo_storyboard(),
        }
        .with_bench()
    }

    /// The navigation benchmark needs marked controls and no reopened session.
    fn with_bench(mut self) -> Self {
        if crate::perf::bench::requested() {
            crate::demo::enable();
            self.open_at_startup.push("bench".to_owned());
        }
        self
    }

    /// Whether the app runs scripted (`PISTACHIO_OPEN` or a demo): no session reopened at
    /// start and no update check.
    pub fn scripted(&self) -> bool {
        !self.open_at_startup.is_empty() || self.demo.is_some()
    }
}

/// The storyboard named by `PISTACHIO_DEMO`. A broken one ends the app: a recording must not
/// start without it.
fn demo_storyboard() -> Option<Arc<crate::demo::Storyboard>> {
    let path = PathBuf::from(std::env::var_os("PISTACHIO_DEMO")?);
    match crate::demo::Storyboard::load(&path) {
        Ok(storyboard) => {
            crate::demo::enable();
            Some(Arc::new(storyboard))
        }
        Err(error) => {
            log::error!("PISTACHIO_DEMO: {error:#}");
            std::process::exit(2);
        }
    }
}

/// `pi` from `PATH`. On Windows npm installs it as `pi.cmd`, which `Command` only finds (and
/// runs through `cmd.exe`) when given its full path, so it is looked up here.
pub(crate) fn default_pi_program() -> PathBuf {
    if cfg!(windows)
        && let Some(path) = std::env::var_os("PATH")
    {
        for dir in std::env::split_paths(&path) {
            for name in ["pi.exe", "pi.cmd", "pi.bat"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from("pi")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
