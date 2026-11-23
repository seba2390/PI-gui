//! Installed speech models and the one download that may be running.
//!
//! Models live in `<agent dir>/pistachio/speech/<id>/`; downloading, verifying and removing
//! them is [`crate::model_store`]'s job.

use gpui_kit::{App, AppContext as _, Entity, Global};

use super::catalog::{self, MODELS, Model};
use crate::model_store::{ModelStore, StoreEvent};

pub type SpeechModels = ModelStore<Model>;
pub type SpeechModelsEvent = StoreEvent;

struct SpeechModelsHandle(Entity<SpeechModels>);

impl Global for SpeechModelsHandle {}

pub fn init(cx: &mut App) {
    let dir = pi_sessions::app_dir().map(|dir| dir.join("speech"));
    let models = cx.new(|_| SpeechModels::new(dir, MODELS));
    cx.set_global(SpeechModelsHandle(models));
}

pub fn models(cx: &App) -> Entity<SpeechModels> {
    cx.global::<SpeechModelsHandle>().0.clone()
}

/// The model to use: the chosen one if installed, else any installed one.
pub fn active_model(cx: &App) -> Option<&'static Model> {
    let models = models(cx);
    let models = models.read(cx);
    let chosen = cx
        .global::<crate::settings::AppSettings>()
        .speech_model
        .as_str();
    catalog::model(chosen)
        .filter(|model| models.is_installed(model.id))
        .or_else(|| MODELS.iter().find(|model| models.is_installed(model.id)))
}
