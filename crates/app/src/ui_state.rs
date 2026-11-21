//! Small pieces of UI state remembered across launches: the sidebar mode, which Code
//! folders are collapsed, and the conversation last open in each mode (so a session on an SSH
//! host comes back after a restart). Stored as JSON next to the chat folder
//! (`<agent dir>/pi-gui/state.json`), written off the UI thread whenever it changes.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use pi_sessions::Mode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub mode: Mode,
    /// Code-mode folders whose session list is collapsed. Folders and hosts on SSH hosts
    /// use keys from [`remote_key`].
    pub collapsed: BTreeSet<PathBuf>,
    /// The conversation last shown in each mode.
    pub last_sessions: HashMap<Mode, LastSession>,
}

/// A conversation to come back to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSession {
    /// pi's session file (a path on `host` for remote sessions).
    pub file: PathBuf,
    /// The saved SSH host it runs on; `None` for this computer.
    #[serde(default)]
    pub host: Option<String>,
    /// Its working folder (on `host`).
    #[serde(default)]
    pub folder: Option<PathBuf>,
}

/// The key under which a remote host (`folder: None`) or a folder on it is remembered as
/// collapsed: `ssh://<host id><folder>`, which can never clash with a local absolute path.
pub fn remote_key(host: &str, folder: Option<&Path>) -> PathBuf {
    let mut key = format!("ssh://{host}");
    if let Some(folder) = folder {
        key.push_str(&folder.to_string_lossy());
    }
    PathBuf::from(key)
}

impl Global for UiState {}

impl UiState {
    /// Where the state lives, or `None` if pi's agent dir is unknown.
    pub fn path() -> Option<PathBuf> {
        pi_sessions::agent_dir().map(|agent| agent.join("pi-gui").join("state.json"))
    }

    /// Reads the saved state; a missing or unreadable file yields the defaults.
    pub fn load(path: Option<&Path>) -> Self {
        path.and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Changes the global state with `f` and saves it in the background if it changed.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Self)) {
        let before = cx.default_global::<Self>().clone();
        f(cx.global_mut::<Self>());
        let after = cx.global::<Self>().clone();
        if before == after {
            return;
        }
        let Some(path) = Self::path() else { return };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = after.write(&path) {
                    log::warn!("could not save UI state to {}: {error}", path.display());
                }
            })
            .detach();
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A temp name of its own per write: saves can overlap (they run in the background).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("json.{}-{n}.tmp", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::UiState;
    use pi_sessions::Mode;
    use std::path::Path;

    #[test]
    fn round_trips_and_tolerates_missing_or_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state.json");
        assert_eq!(UiState::load(Some(&path)), UiState::default());

        let state = UiState {
            mode: Mode::Chat,
            collapsed: [
                "/a".into(),
                super::remote_key("box", Some(Path::new("/srv"))),
            ]
            .into_iter()
            .collect(),
            last_sessions: [(
                Mode::Code,
                super::LastSession {
                    file: "/home/me/.pi/agent/sessions/--srv--/s.jsonl".into(),
                    host: Some("box".into()),
                    folder: Some("/srv".into()),
                },
            )]
            .into_iter()
            .collect(),
        };
        state.write(&path).unwrap();
        assert_eq!(UiState::load(Some(&path)), state);

        std::fs::write(&path, r#"{"mode":"chat"}"#).unwrap();
        assert_eq!(UiState::load(Some(&path)).mode, Mode::Chat);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(UiState::load(Some(&path)), UiState::default());
    }
}
