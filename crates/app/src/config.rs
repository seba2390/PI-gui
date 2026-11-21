//! Process-wide configuration, resolved once at startup.

use std::ffi::OsString;
use std::path::PathBuf;

use gpui_kit::Global;

#[derive(Debug, Clone)]
pub struct AppConfig {
    /// The `pi` executable. Override with `PI_GUI_PI_BIN`.
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
    /// Panels to open at startup (`PI_GUI_OPEN=terminal,diff,files`), for scripted
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
            chat_dir: std::env::temp_dir().join("pi-gui-test-chat"),
            deepsearch_dir: std::env::temp_dir().join("pi-gui-test-deepsearch"),
            exe: Some(PathBuf::from("/app/pi-gui")),
            open_at_startup: Vec::new(),
        }
    }

    /// `child_env` is the result of [`crate::shell_env::login_shell_env`].
    pub fn detect(child_env: Vec<(OsString, OsString)>) -> Self {
        let home = dirs_home();
        Self {
            pi_program: std::env::var_os("PI_GUI_PI_BIN")
                .map_or_else(|| "pi".into(), PathBuf::from),
            child_env,
            sessions_dir: pi_sessions::sessions_dir(),
            home,
            chat_dir: pi_sessions::chat_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("pi-gui-chat")),
            deepsearch_dir: deepsearch::config::default_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("pi-gui-deepsearch")),
            exe: std::env::current_exe().ok(),
            open_at_startup: std::env::var("PI_GUI_OPEN")
                .map(|value| {
                    value
                        .split(',')
                        .map(|panel| panel.trim().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
