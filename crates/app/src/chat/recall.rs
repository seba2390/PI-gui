//! What pi last said about the composer of each conversation on this computer: its model,
//! thinking level, context usage, web search mode, and the commands of its folder. A
//! conversation opened again shows them at once, instead of "Model" and no globe until pi has
//! started and answered (a few hundred milliseconds); pi's own answers replace them as they
//! arrive. A new conversation shows what the last new one in its folder started with.
//!
//! Kept in memory and in `<agent dir>/pistachio/recall.json` (written off the UI thread), so
//! this works right after a launch too. Only the most recent conversations and folders are
//! kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use pi_rpc::protocol::{Model, SessionStats, SlashCommand, ThinkingLevel};
use serde::{Deserialize, Serialize};

/// Conversations remembered (most recently used first).
const MAX_SESSIONS: usize = 300;
/// Folders whose commands and new-conversation state are remembered.
const MAX_FOLDERS: usize = 40;

/// What a conversation's composer showed when pi last answered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub thinking_levels: Vec<ThinkingLevel>,
    /// For a conversation that has messages: its context usage and cost.
    pub stats: Option<SessionStats>,
    /// Extension statuses that the composer shows (web search mode and effort).
    pub status: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Folder {
    path: PathBuf,
    commands: Vec<SlashCommand>,
    /// What a new conversation in this folder started with.
    new: Option<Snapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Session {
    file: PathBuf,
    snapshot: Snapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recall {
    /// The models pi listed last (the same for every local conversation).
    models: Vec<Model>,
    folders: Vec<Folder>,
    sessions: Vec<Session>,
}

/// What a conversation shows before pi has answered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Early {
    pub models: Vec<Model>,
    pub commands: Vec<SlashCommand>,
    /// What pi said about this very conversation (for a new one: about the last new one in
    /// its folder), if it was remembered.
    pub snapshot: Option<Snapshot>,
    /// What a new conversation in the folder started with: pi's defaults there.
    pub defaults: Option<Snapshot>,
}

impl Global for Recall {}

impl Recall {
    pub fn path() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        pi_sessions::app_dir().map(|dir| dir.join("recall.json"))
    }

    pub fn load(path: Option<&Path>) -> Self {
        path.map_or_default(crate::persist::load)
    }

    /// What to show for the conversation in `file` (`None`: a new one) in `folder`, if pi
    /// answered for it, or for a new one in that folder, before.
    pub fn early(&self, folder: &Path, file: Option<&Path>) -> Option<Early> {
        let place = self.folders.iter().find(|place| place.path == folder);
        // A folder with no new conversation yet: the most recent other one's defaults (pi's
        // settings are the same, unless a project has its own).
        let defaults = place
            .and_then(|place| place.new.clone())
            .or_else(|| self.folders.iter().find_map(|place| place.new.clone()));
        let snapshot = match file {
            Some(file) => self
                .sessions
                .iter()
                .find(|session| session.file == file)
                .map(|session| session.snapshot.clone()),
            None => defaults.clone(),
        };
        if snapshot.is_none() && place.is_none() && self.models.is_empty() {
            return None;
        }
        Some(Early {
            models: self.models.clone(),
            commands: place.map_or_else(Vec::new, |place| place.commands.clone()),
            snapshot,
            defaults,
        })
    }

    /// Remembers what pi answered for a conversation in `folder`: for `file` when it has one,
    /// and as a new conversation's start when `new`. Returns whether anything changed.
    pub fn remember(
        &mut self,
        folder: &Path,
        file: Option<&Path>,
        new: bool,
        models: &[Model],
        commands: &[SlashCommand],
        snapshot: Snapshot,
    ) -> bool {
        let mut changed = false;
        if !models.is_empty() && self.models != models {
            self.models = models.to_vec();
            changed = true;
        }
        let at = self.folders.iter().position(|place| place.path == folder);
        let before = at.map(|at| self.folders.remove(at));
        let mut place = before.clone().unwrap_or(Folder {
            path: folder.to_path_buf(),
            ..Folder::default()
        });
        if !commands.is_empty() {
            commands.clone_into(&mut place.commands);
        }
        if new {
            place.new = Some(Snapshot {
                stats: None,
                ..snapshot.clone()
            });
        }
        changed |= at != Some(0) || before.as_ref() != Some(&place);
        self.folders.insert(0, place);
        self.folders.truncate(MAX_FOLDERS);
        if let Some(file) = file {
            let at = self
                .sessions
                .iter()
                .position(|session| session.file == file);
            let before = at.map(|at| self.sessions.remove(at));
            changed |= at != Some(0) || before.is_none_or(|before| before.snapshot != snapshot);
            self.sessions.insert(
                0,
                Session {
                    file: file.to_path_buf(),
                    snapshot,
                },
            );
            self.sessions.truncate(MAX_SESSIONS);
        }
        changed
    }

    /// Forgets a deleted conversation. Returns whether it was remembered.
    pub fn forget(&mut self, file: &Path) -> bool {
        let at = self
            .sessions
            .iter()
            .position(|session| session.file == file);
        at.map(|at| self.sessions.remove(at)).is_some()
    }

    /// Changes the global recall with `f` and saves it in the background if `f` says it
    /// changed.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Self) -> bool) {
        if !cx.has_global::<Self>() || !f(cx.global_mut::<Self>()) {
            return;
        }
        let after = cx.global::<Self>().clone();
        let Some(path) = Self::path() else { return };
        crate::persist::save_in_background(cx, path, move |path| after.write(path));
    }

    /// Writes atomically (temp file + rename), compactly: it is only read by the app.
    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> Model {
        serde_json::from_value(serde_json::json!({"id": id, "provider": "p", "name": id})).unwrap()
    }

    fn command(name: &str) -> SlashCommand {
        serde_json::from_value(serde_json::json!({"name": name, "source": "extension"})).unwrap()
    }

    fn snapshot(id: &str) -> Snapshot {
        Snapshot {
            model: Some(model(id)),
            thinking_level: Some(ThinkingLevel::High),
            thinking_levels: vec![ThinkingLevel::Off, ThinkingLevel::High],
            stats: Some(SessionStats {
                cost: 0.5,
                ..SessionStats::default()
            }),
            status: BTreeMap::from([("web".into(), "deep".into())]),
        }
    }

    #[test]
    fn nothing_known_at_first() {
        let recall = Recall::default();
        assert_eq!(recall.early(Path::new("/f"), None), None);
        assert_eq!(recall.early(Path::new("/f"), Some(Path::new("/s"))), None);
    }

    #[test]
    fn a_conversation_comes_back_as_pi_left_it() {
        let mut recall = Recall::default();
        let (folder, file) = (Path::new("/f"), Path::new("/f/s.jsonl"));
        let models = [model("a"), model("b")];
        recall.remember(
            folder,
            Some(file),
            false,
            &models,
            &[command("websearch")],
            snapshot("b"),
        );
        let early = recall.early(folder, Some(file)).expect("remembered");
        assert_eq!(early.models, models);
        assert_eq!(early.commands, [command("websearch")]);
        assert_eq!(early.snapshot, Some(snapshot("b")));
        // Another conversation in the folder: the folder's commands and the models only.
        let other = recall
            .early(folder, Some(Path::new("/f/t.jsonl")))
            .expect("folder");
        assert_eq!(other.snapshot, None);
        assert_eq!(other.commands, [command("websearch")]);
        // A new one: nothing started there yet.
        assert_eq!(recall.early(folder, None).expect("folder").snapshot, None);
    }

    #[test]
    fn says_whether_anything_changed() {
        let mut recall = Recall::default();
        let (folder, file) = (Path::new("/f"), Path::new("/f/s.jsonl"));
        assert!(recall.remember(folder, Some(file), false, &[model("a")], &[], snapshot("a")));
        assert!(!recall.remember(folder, Some(file), false, &[model("a")], &[], snapshot("a")));
        assert!(recall.remember(folder, Some(file), false, &[model("a")], &[], snapshot("b")));
        let other = Path::new("/f/t.jsonl");
        assert!(recall.remember(folder, Some(other), false, &[], &[], snapshot("b")));
        // Used again: moves to the front.
        assert!(recall.remember(folder, Some(file), false, &[], &[], snapshot("b")));
    }

    #[test]
    fn a_new_conversation_starts_like_the_last_new_one() {
        let mut recall = Recall::default();
        let folder = Path::new("/f");
        recall.remember(folder, None, true, &[model("a")], &[], snapshot("a"));
        let early = recall.early(folder, None).expect("remembered");
        let snapshot = early.snapshot.expect("the new one's");
        assert_eq!(snapshot.model, Some(model("a")));
        assert_eq!(snapshot.stats, None, "a new conversation has used nothing");
        assert_eq!(early.defaults, Some(snapshot));
        // An unknown conversation in the folder gets the defaults to fall back on.
        let other = recall.early(folder, Some(Path::new("/f/u.jsonl"))).unwrap();
        assert_eq!((other.snapshot, other.defaults.is_some()), (None, true));
        // Another folder: the same defaults and models, none of the folder's commands.
        let elsewhere = recall.early(Path::new("/g"), None).expect("models known");
        assert_eq!(elsewhere.defaults.map(|d| d.model), Some(Some(model("a"))));
        assert_eq!(elsewhere.models, [model("a")]);
    }

    #[test]
    fn only_the_most_recent_are_kept() {
        let mut recall = Recall::default();
        for n in 0..MAX_SESSIONS + 5 {
            let file = PathBuf::from(format!("/f/{n}.jsonl"));
            recall.remember(Path::new("/f"), Some(&file), false, &[], &[], snapshot("a"));
        }
        assert_eq!(recall.sessions.len(), MAX_SESSIONS);
        let early = |n: usize| {
            let file = PathBuf::from(format!("/f/{n}.jsonl"));
            recall.early(Path::new("/f"), Some(&file)).unwrap().snapshot
        };
        assert_eq!(early(0), None, "the oldest is gone");
        assert!(early(MAX_SESSIONS + 4).is_some());
        assert!(recall.forget(Path::new(&format!("/f/{}.jsonl", MAX_SESSIONS + 4))));
        assert_eq!(recall.sessions.len(), MAX_SESSIONS - 1);
    }
}
