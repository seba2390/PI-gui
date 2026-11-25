//! The app's own settings (`<agent dir>/pistachio/settings.json`) and applying them.
//!
//! pi's settings stay pi's: the Settings page edits the startup model and thinking level in
//! pi's `settings.json` and web search in its own `config.json`; this file only holds what the
//! app itself decides (look, start-up, notifications, updates, which `pi` to run).

mod documents;
mod local_search;
mod models;
pub mod view;
mod web_advanced;

pub use local_search::{open_setup as open_searxng_setup, use_local as use_local_searxng};

use std::path::{Path, PathBuf};

use gpui_kit::component::Theme;
use gpui_kit::{App, Global, Pixels};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

/// The interface's text size, smallest first.
///
/// The sizes were relabelled one step down when the old "Large" became the default, so the
/// stored ids are the old names: each saved setting keeps its exact size (an old "default"
/// is now called Small). Don't rename an id, or saved sizes change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSize {
    #[serde(rename = "small")]
    Smaller,
    #[serde(rename = "default")]
    Small,
    #[default]
    #[serde(rename = "large")]
    Default,
    #[serde(rename = "larger")]
    Large,
    #[serde(rename = "largest")]
    Larger,
}

impl TextSize {
    /// Every size, smallest first (the picker's order).
    pub const ALL: [Self; 5] = [
        Self::Smaller,
        Self::Small,
        Self::Default,
        Self::Large,
        Self::Larger,
    ];

    /// Scale applied to the theme's base text size (which sizes the whole interface).
    pub fn factor(self) -> f32 {
        match self {
            Self::Smaller => 0.9,
            Self::Small => 1.0,
            Self::Default => 1.1,
            Self::Large => 1.2,
            Self::Larger => 1.3,
        }
    }

    /// The name shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            Self::Smaller => "Smaller",
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::Larger => "Larger",
        }
    }

    /// The id it is saved as (also the picker's value).
    pub fn id(self) -> &'static str {
        match self {
            Self::Smaller => "small",
            Self::Small => "default",
            Self::Default => "large",
            Self::Large => "larger",
            Self::Larger => "largest",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    /// The mode used last.
    #[default]
    Last,
    Chat,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // independent on/off settings
pub struct AppSettings {
    pub theme: ThemeChoice,
    /// The palette for code (syntax colors only; the rest of the theme is unchanged).
    pub code_colors: crate::code_colors::CodeColors,
    /// How a working agent is shown (chat and sidebar).
    pub working_indicator: crate::ui::spinner::IndicatorStyle,
    pub text_size: TextSize,
    pub start_mode: StartMode,
    /// Open the last session of the start mode (on this computer or an SSH host) at start,
    /// instead of the welcome screen.
    pub reopen_last_session: bool,
    /// A toast when a chat working in the background finishes or needs an answer.
    pub background_notifications: bool,
    /// Bounce the Dock icon (macOS) or flash the window when that happens while the app is
    /// not in front.
    pub request_attention: bool,
    /// Look for a newer release when the app opens.
    pub check_for_updates: bool,
    /// A release the user chose to skip; not offered again.
    pub skipped_version: Option<String>,
    /// The version that ran last: after an update, the next start shows what changed.
    pub last_run_version: Option<String>,
    /// The `pi` to run (`PISTACHIO_PI_BIN` still wins); `None` finds it on `PATH`.
    pub pi_program: Option<String>,
    /// Connect every saved SSH connection quietly when the app opens.
    pub connect_hosts_at_start: bool,
    /// The speech-to-text model to dictate with (a `stt::catalog` id).
    pub speech_model: String,
    /// The language Whisper models transcribe; empty to detect it.
    pub speech_language: String,
    /// How long a silence ends dictation.
    pub speech_auto_stop: crate::stt::AutoStop,
    /// Name new conversations from their first message (see [`crate::naming`]).
    pub auto_names: bool,
    /// What makes those names.
    pub namer: crate::naming::Namer,
    /// The local model that names conversations (a `naming::catalog` id).
    pub naming_model: String,
    /// Settings this version doesn't know (a newer one wrote them), kept for the next save.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            code_colors: crate::code_colors::CodeColors::default(),
            working_indicator: crate::ui::spinner::IndicatorStyle::default(),
            text_size: TextSize::default(),
            start_mode: StartMode::default(),
            reopen_last_session: false,
            background_notifications: true,
            request_attention: true,
            check_for_updates: true,
            skipped_version: None,
            last_run_version: None,
            pi_program: None,
            connect_hosts_at_start: false,
            speech_model: crate::stt::catalog::DEFAULT_MODEL.to_owned(),
            speech_language: String::new(),
            speech_auto_stop: crate::stt::AutoStop::default(),
            auto_names: false,
            namer: crate::naming::Namer::default(),
            naming_model: crate::naming::catalog::DEFAULT_MODEL.to_owned(),
            unknown: serde_json::Map::new(),
        }
    }
}

impl Global for AppSettings {}

/// The theme's text sizes before [`TextSize`] scaled them.
struct BaseTextSize {
    ui: Pixels,
    mono: Pixels,
}

impl Global for BaseTextSize {}

impl AppSettings {
    pub fn path() -> Option<PathBuf> {
        pi_sessions::app_dir().map(|dir| dir.join("settings.json"))
    }

    /// Reads the settings, with defaults for what is missing or unreadable (see
    /// [`crate::persist::load`]).
    pub fn load(path: Option<&Path>) -> Self {
        path.map(crate::persist::load).unwrap_or_default()
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// Changes the settings with `f`, saves them and applies what changed.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Self)) {
        let before = cx.default_global::<Self>().clone();
        f(cx.global_mut::<Self>());
        let after = cx.global::<Self>().clone();
        if before == after {
            return;
        }
        if let Some(path) = Self::path() {
            let settings = after.clone();
            crate::persist::save_in_background(cx, path, move |path| settings.write(path));
        }
        if before.theme != after.theme {
            // Windows may be mid-update (the change came from a click); restyle afterwards.
            cx.defer(|cx| {
                for handle in cx.windows() {
                    let _ = handle.update(cx, |_, window, cx| {
                        crate::theme::sync_with_window(window, cx);
                    });
                }
            });
        }
        if before.code_colors != after.code_colors {
            crate::theme::apply_code_colors(cx);
        }
        if before.text_size != after.text_size {
            apply_text_size(cx);
        }
        if before.pi_program != after.pi_program {
            apply_pi_program(cx);
        }
    }
}

/// Remembers the theme's base text sizes and applies [`AppSettings::text_size`]. Call once
/// after the theme is loaded.
pub fn init_text_size(cx: &mut App) {
    let theme = Theme::global(cx);
    let base = BaseTextSize {
        ui: theme.font_size,
        mono: theme.mono_font_size,
    };
    cx.set_global(base);
    apply_text_size(cx);
}

/// Applies [`AppSettings::text_size`]; also after a light/dark switch, which resets the theme's
/// text size to the theme file's.
pub fn apply_text_size(cx: &mut App) {
    let factor = cx.default_global::<AppSettings>().text_size.factor();
    let Some((ui, mono)) = cx
        .try_global::<BaseTextSize>()
        .map(|base| (base.ui, base.mono))
    else {
        return;
    };
    let theme = Theme::global_mut(cx);
    theme.font_size = ui * factor;
    theme.mono_font_size = mono * factor;
    cx.refresh_windows();
}

/// Points new sessions at the configured `pi` (unless `PISTACHIO_PI_BIN` is set).
pub fn apply_pi_program(cx: &mut App) {
    if std::env::var_os("PISTACHIO_PI_BIN").is_some() {
        return;
    }
    let configured = cx
        .default_global::<AppSettings>()
        .pi_program
        .clone()
        .filter(|path| !path.trim().is_empty());
    let program = configured.map_or_else(crate::config::default_pi_program, PathBuf::from);
    cx.global_mut::<crate::config::AppConfig>().pi_program = program;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_files_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(AppSettings::load(Some(&path)), AppSettings::default());
        std::fs::write(&path, r#"{"theme": "dark", "unknown": 1}"#).unwrap();
        let loaded = AppSettings::load(Some(&path));
        assert_eq!(loaded.theme, ThemeChoice::Dark);
        assert!(loaded.check_for_updates, "unset keys keep their defaults");
        assert!(
            !loaded.auto_names,
            "automatic names are off unless turned on"
        );
    }

    #[test]
    fn an_unreadable_value_loses_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let text = r#"{"theme": "sepia", "text_size": "larger", "check_for_updates": false,
            "auto_names": "often", "skipped_version": "0.9.0", "from_the_future": {"a": 1}}"#;
        std::fs::write(&path, text).unwrap();
        let loaded = AppSettings::load(Some(&path));
        assert_eq!(loaded.theme, ThemeChoice::System);
        assert!(!loaded.auto_names);
        assert_eq!(loaded.text_size, TextSize::Large);
        assert!(!loaded.check_for_updates);
        assert_eq!(loaded.skipped_version.as_deref(), Some("0.9.0"));
        // What the next save replaces is kept.
        let backup = dir.path().join("settings.json.corrupt");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), text);
        // Settings of a newer version survive that save.
        loaded.write(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["from_the_future"]["a"], 1);
        assert_eq!(saved["theme"], "system");
        assert_eq!(AppSettings::load(Some(&path)), loaded);
    }

    #[test]
    fn a_file_that_is_not_json_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let backup = dir.path().join("settings.json.corrupt");
        for text in ["{ not json", "[]", ""] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(AppSettings::load(Some(&path)), AppSettings::default());
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), text);
        }
        // A readable file needs no copy, also with settings this version doesn't know.
        std::fs::remove_file(&backup).unwrap();
        std::fs::write(&path, r#"{"theme": "dark", "unknown": 1}"#).unwrap();
        AppSettings::load(Some(&path));
        assert!(!backup.exists());
    }

    #[test]
    fn naming_settings_round_trip() {
        let defaults = AppSettings::default();
        assert!(!defaults.auto_names);
        assert_eq!(defaults.namer, crate::naming::Namer::Conversation);
        assert_eq!(defaults.naming_model, crate::naming::catalog::DEFAULT_MODEL);
        let text = r#"{"auto_names": true, "namer": "local", "naming_model": "smollm2-360m"}"#;
        let loaded: AppSettings = serde_json::from_str(text).unwrap();
        assert!(loaded.auto_names);
        assert_eq!(loaded.namer, crate::naming::Namer::Local);
        assert_eq!(loaded.naming_model, "smollm2-360m");
    }

    #[test]
    fn text_size_defaults_to_the_old_large() {
        assert_eq!(TextSize::default(), TextSize::Default);
        assert!((TextSize::default().factor() - 1.1).abs() < f32::EPSILON);
        let loaded: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.text_size, TextSize::Default);
    }

    #[test]
    fn saved_text_sizes_keep_their_scale() {
        // The ids and scales saved before the sizes were relabelled.
        for (id, factor, size) in [
            ("small", 0.9, TextSize::Smaller),
            ("default", 1.0, TextSize::Small),
            ("large", 1.1, TextSize::Default),
            ("larger", 1.2, TextSize::Large),
        ] {
            let loaded: TextSize = serde_json::from_str(&format!("\"{id}\"")).unwrap();
            assert_eq!(loaded, size, "{id}");
            assert!((loaded.factor() - factor).abs() < f32::EPSILON, "{id}");
        }
    }

    #[test]
    fn the_new_largest_step_round_trips() {
        let loaded: TextSize = serde_json::from_str("\"largest\"").unwrap();
        assert_eq!(loaded, TextSize::Larger);
        assert!(loaded.factor() > TextSize::Large.factor());
        for size in TextSize::ALL {
            let text = serde_json::to_string(&size).unwrap();
            assert_eq!(text, format!("\"{}\"", size.id()));
            assert_eq!(serde_json::from_str::<TextSize>(&text).unwrap(), size);
        }
        assert!(
            TextSize::ALL
                .windows(2)
                .all(|pair| pair[0].factor() < pair[1].factor())
        );
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let settings = AppSettings {
            text_size: TextSize::Large,
            code_colors: crate::code_colors::CodeColors::Gruvbox,
            start_mode: StartMode::Code,
            skipped_version: Some("0.3.0".into()),
            ..AppSettings::default()
        };
        settings.write(&path).unwrap();
        assert_eq!(AppSettings::load(Some(&path)), settings);
    }
}
