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
    /// Folder new chats start in until the user picks one.
    pub default_cwd: PathBuf,
    pub home: Option<PathBuf>,
    /// Panels to open at startup (`PI_GUI_OPEN=terminal,diff,files`), for scripted
    /// screenshots and responsiveness checks.
    pub open_at_startup: Vec<String>,
}

impl Global for AppConfig {}

impl AppConfig {
    /// `child_env` is the result of [`crate::shell_env::login_shell_env`].
    pub fn detect(child_env: Vec<(OsString, OsString)>) -> Self {
        let home = dirs_home();
        let default_cwd = std::env::current_dir()
            .ok()
            .filter(|dir| dir.as_os_str() != "/")
            .or_else(|| home.clone())
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            pi_program: std::env::var_os("PI_GUI_PI_BIN")
                .map_or_else(|| "pi".into(), PathBuf::from),
            child_env,
            sessions_dir: pi_sessions::sessions_dir(),
            default_cwd,
            home,
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
