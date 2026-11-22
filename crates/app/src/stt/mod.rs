//! Local speech to text: dictate into the composer with a model that runs on this computer.
//!
//! - [`catalog`]: the models offered (none ship with the app).
//! - [`models`]: what is installed, and the download that may be running.
//! - [`engine`]: the recognizer the composer's `SpeechState` uses.
//! - [`ui`]: the download dialog, the progress banner and the settings rows.

pub mod catalog;
pub mod engine;
pub mod models;
pub mod ui;

use std::time::Duration;

use gpui_kit::App;
use serde::{Deserialize, Serialize};

use crate::settings::AppSettings;

pub fn init(cx: &mut App) {
    models::init(cx);
}

/// How long a silence ends dictation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoStop {
    Short,
    #[default]
    Medium,
    Long,
    /// Only the stop button ends dictation.
    Never,
}

impl AutoStop {
    pub const ALL: [Self; 4] = [Self::Short, Self::Medium, Self::Long, Self::Never];

    pub fn duration(self) -> Option<Duration> {
        match self {
            Self::Short => Some(Duration::from_millis(1500)),
            Self::Medium => Some(Duration::from_millis(2500)),
            Self::Long => Some(Duration::from_secs(4)),
            Self::Never => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Medium => "medium",
            Self::Long => "long",
            Self::Never => "never",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "After 1.5 seconds",
            Self::Medium => "After 2.5 seconds",
            Self::Long => "After 4 seconds",
            Self::Never => "Only when I press stop",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|stop| stop.id() == id)
            .unwrap_or_default()
    }
}

/// What a dictation session runs with, or `None` while no model is installed.
pub fn session_config(cx: &App) -> Option<engine::SessionConfig> {
    let model = models::active_model(cx)?;
    let dir = models::models(cx).read(cx).model_dir(model.id)?;
    let settings = cx.global::<AppSettings>();
    Some(engine::SessionConfig {
        model,
        dir,
        language: settings.speech_language.clone(),
        auto_stop: settings.speech_auto_stop.duration(),
    })
}

#[cfg(test)]
mod tests {
    use super::AutoStop;

    #[test]
    fn auto_stop_ids_round_trip() {
        for stop in AutoStop::ALL {
            assert_eq!(AutoStop::from_id(stop.id()), stop);
        }
        assert_eq!(AutoStop::from_id("?"), AutoStop::Medium);
    }
}
