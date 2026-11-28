//! What the Models dialog found out about models' thinking that pi's `models.json` has no place
//! for: which models still think with thinking "off" (some think a little at any effort, others
//! have no way to turn it off). The thinking picker calls their off "lowest", so it says what it
//! does. Kept in `<agent dir>/pistachio/thinking.json`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ThinkingNotes {
    /// `provider/model` of models that think even with thinking off.
    pub off_thinks: BTreeSet<String>,
}

impl Global for ThinkingNotes {}

impl ThinkingNotes {
    pub fn path() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        pi_sessions::app_dir().map(|dir| dir.join("thinking.json"))
    }

    pub fn load(path: Option<&Path>) -> Self {
        path.map_or_default(crate::persist::load)
    }

    /// Whether `model` of `provider` still thinks with thinking off.
    pub fn off_thinks(cx: &App, provider: &str, model: &str) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|notes| notes.off_thinks.contains(&format!("{provider}/{model}")))
    }

    /// Notes whether `model` of `provider` still thinks with thinking off, and saves it in the
    /// background.
    pub fn set(cx: &mut App, provider: &str, model: &str, off_thinks: bool) {
        if !cx.has_global::<Self>() {
            return;
        }
        let key = format!("{provider}/{model}");
        let notes = cx.global_mut::<Self>();
        let changed = if off_thinks {
            notes.off_thinks.insert(key)
        } else {
            notes.off_thinks.remove(&key)
        };
        let Some(path) = Self::path().filter(|_| changed) else {
            return;
        };
        let after = notes.clone();
        crate::persist::save_in_background(cx, path, move |path| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
            std::fs::write(
                &tmp,
                serde_json::to_vec(&after).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(tmp, path)
        });
    }
}
