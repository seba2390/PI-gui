//! Process-wide configuration, resolved once at startup.

use std::convert::Infallible;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::Global;

use crate::shell_env::Vars;

/// Extra environment for child processes: the login shell's (see [`crate::shell_env`]),
/// probed in the background at startup. Until then it is not [`ready`](Self::ready), and a pi
/// process that starts waits for it; the UI never does.
#[derive(Clone)]
pub struct ChildEnv(Arc<ChildEnvInner>);

struct ChildEnvInner {
    vars: OnceLock<Vars>,
    /// Closes when the probe has finished (or could not run).
    probed: async_channel::Receiver<Infallible>,
}

/// The variables a terminal's shell gets: enough to find programs. It reads the user's startup
/// files itself, and some of those skip what their variables say was done already.
const TERMINAL_VARS: &[&str] = &[
    "PATH",
    "NVM_DIR",
    "NODE_PATH",
    "VOLTA_HOME",
    "FNM_DIR",
    "LANG",
];

impl ChildEnv {
    /// Starts probing the login shell on a background thread.
    pub fn probe() -> Self {
        Self::resolve(crate::shell_env::login_shell_env)
    }

    /// Runs `probe` on a background thread. Without its variables (the thread could not
    /// start, or panicked) there are none.
    pub(crate) fn resolve(probe: impl FnOnce() -> Vars + Send + 'static) -> Self {
        // Closes when the thread drops the sender, however it ends.
        let (sender, probed) = async_channel::bounded(1);
        let env = Self(Arc::new(ChildEnvInner {
            vars: OnceLock::new(),
            probed,
        }));
        let inner = Arc::clone(&env.0);
        let spawned = std::thread::Builder::new()
            .name("login-shell-env".into())
            .spawn(move || {
                let _ = inner.vars.set(probe());
                drop(sender);
            });
        if let Err(error) = spawned {
            log::warn!("could not probe the login shell: {error}");
        }
        env
    }

    /// The variables, or `None` while the probe is still running. Never blocks.
    pub fn ready(&self) -> Option<&[(OsString, OsString)]> {
        self.0
            .probed
            .is_closed()
            .then(|| self.0.vars.get().map(Vec::as_slice).unwrap_or_default())
    }

    /// Resolves once the variables are [`ready`](Self::ready).
    pub async fn wait(&self) {
        let _ = self.0.probed.recv().await;
    }

    /// The variables, waiting for the probe if it is still running (which takes seconds with
    /// a slow shell): for background threads only.
    pub fn vars(&self) -> &[(OsString, OsString)] {
        let _ = self.0.probed.recv_blocking();
        self.ready().unwrap_or_default()
    }

    /// The variables for a terminal's shell ([`TERMINAL_VARS`]). Never blocks: none of them
    /// while the probe is still running.
    pub fn terminal_vars(&self) -> impl Iterator<Item = &(OsString, OsString)> {
        self.ready()
            .unwrap_or_default()
            .iter()
            .filter(|(key, _)| TERMINAL_VARS.iter().any(|name| key == name))
    }

    /// What child processes see as `key`: the login shell's value or else the app's own.
    /// Waits like [`vars`](Self::vars).
    pub fn var(&self, key: &str) -> Option<OsString> {
        self.vars()
            .iter()
            .find(|(name, _)| name == OsStr::new(key))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os(key))
    }
}

impl Default for ChildEnv {
    /// No variables, ready right away.
    fn default() -> Self {
        Vars::new().into()
    }
}

impl From<Vars> for ChildEnv {
    /// Ready right away.
    fn from(vars: Vars) -> Self {
        Self(Arc::new(ChildEnvInner {
            vars: OnceLock::from(vars),
            probed: async_channel::bounded(1).1,
        }))
    }
}

/// The names only: the values hold API keys and other secrets.
impl std::fmt::Debug for ChildEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.ready() {
            Some(vars) => {
                let names: Vec<_> = vars.iter().map(|(name, _)| name).collect();
                f.debug_tuple("ChildEnv").field(&names).finish()
            }
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
    /// Document parsing's settings, parsers and pi extension (see [`documents`]).
    pub documents_dir: PathBuf,
    /// Where the skills that come with the app are written (see [`crate::builtin_skills`]).
    pub builtin_skills_dir: PathBuf,
    /// This executable, which runs deep search's web tools for pi and reads documents.
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
            documents_dir: std::env::temp_dir().join("pistachio-test-documents"),
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
            documents_dir: documents::config::default_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("pistachio-documents")),
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

#[cfg(test)]
mod tests {
    use super::{AppConfig, ChildEnv};

    #[test]
    fn the_environment_is_ready_once_probed() {
        let (release, held) = std::sync::mpsc::channel::<()>();
        let env = ChildEnv::resolve(move || {
            let _ = held.recv();
            vec![("API_KEY".into(), "secret".into())]
        });
        assert_eq!(env.ready(), None);
        assert_eq!(env.terminal_vars().count(), 0);
        assert_eq!(format!("{env:?}"), "ChildEnv(probing)");
        release.send(()).unwrap();
        futures::executor::block_on(env.wait());
        assert_eq!(
            env.ready(),
            Some(&[("API_KEY".into(), "secret".into())][..])
        );
        assert_eq!(env.vars().len(), 1);
        assert_eq!(env.var("API_KEY"), Some("secret".into()));
    }

    #[test]
    fn a_probe_that_fails_leaves_no_variables() {
        let env = ChildEnv::resolve(|| std::panic::resume_unwind(Box::new(())));
        assert_eq!(env.vars(), []);
        assert_eq!(ChildEnv::default().ready(), Some(&[][..]));
    }

    #[test]
    fn debug_shows_no_values() {
        let mut config = AppConfig::for_tests("pi".into());
        config.child_env = vec![("API_KEY".into(), "secret".into())].into();
        let shown = format!("{config:?}");
        assert!(shown.contains("API_KEY") && !shown.contains("secret"));
    }

    #[test]
    fn terminals_get_only_what_finds_programs() {
        let env: ChildEnv = vec![
            ("PATH".into(), "/bin".into()),
            ("API_KEY".into(), "secret".into()),
        ]
        .into();
        let names: Vec<_> = env.terminal_vars().map(|(name, _)| name).collect();
        assert_eq!(names, ["PATH"]);
    }
}
