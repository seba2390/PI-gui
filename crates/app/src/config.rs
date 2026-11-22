//! Process-wide configuration, resolved once at startup.

use std::ffi::OsString;
use std::path::PathBuf;

use gpui_kit::Global;

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// The `pi` executable. Override with `PISTACHIO_PI_BIN`.
    pub pi_program: PathBuf,
    /// Extra environment for child processes (login-shell `PATH`, …).
    pub child_env: Vec<(OsString, OsString)>,
    /// pi's session directory, if it could be determined.
    pub sessions_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// The folder chat-mode sessions run in (see [`pi_sessions::Mode`]).
    pub chat_dir: PathBuf,
    /// Deep search's settings, research instructions and pi extension (see [`deepsearch`]).
    pub deepsearch_dir: PathBuf,
    /// This executable, which runs deep search's web tools for pi.
    pub exe: Option<PathBuf>,
    /// Panels to open at startup (`PISTACHIO_OPEN=terminal,diff,files`), for scripted
    /// screenshots and responsiveness checks.
    pub open_at_startup: Vec<String>,
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
            child_env: Vec::new(),
            sessions_dir: None,
            home: None,
            chat_dir: std::env::temp_dir().join("pistachio-test-chat"),
            deepsearch_dir: std::env::temp_dir().join("pistachio-test-deepsearch"),
            exe: Some(PathBuf::from("/app/pistachio")),
            open_at_startup: Vec::new(),
        }
    }

    /// `child_env` is the result of [`crate::shell_env::login_shell_env`].
    pub fn detect(child_env: Vec<(OsString, OsString)>) -> Self {
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
        }
    }
}

/// `pi` from `PATH`. On Windows npm installs it as `pi.cmd`, which `Command` only finds (and
/// runs through `cmd.exe`) when given its full path, so it is looked up here.
fn default_pi_program() -> PathBuf {
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
