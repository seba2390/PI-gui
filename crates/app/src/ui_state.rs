//! Small pieces of UI state remembered across launches: the sidebar mode and which Code
//! folders are collapsed. Stored as JSON next to the chat folder
//! (`<agent dir>/pi-gui/state.json`), written off the UI thread whenever it changes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use pi_sessions::Mode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub mode: Mode,
    /// Code-mode folders whose session list is collapsed.
    pub collapsed: BTreeSet<PathBuf>,
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
        let tmp = path.with_extension("json.tmp");
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

    #[test]
    fn round_trips_and_tolerates_missing_or_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state.json");
        assert_eq!(UiState::load(Some(&path)), UiState::default());

        let state = UiState {
            mode: Mode::Chat,
            collapsed: ["/a".into()].into_iter().collect(),
        };
        state.write(&path).unwrap();
        assert_eq!(UiState::load(Some(&path)), state);

        std::fs::write(&path, r#"{"mode":"chat"}"#).unwrap();
        assert_eq!(UiState::load(Some(&path)).mode, Mode::Chat);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(UiState::load(Some(&path)), UiState::default());
    }
}
