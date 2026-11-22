//! The app's own settings (`<agent dir>/pistachio/settings.json`) and applying them.
//!
//! pi's settings stay pi's: the Settings page edits the startup model and thinking level in
//! pi's `settings.json` and web search in its own `config.json`; this file only holds what the
//! app itself decides (look, start-up, notifications, updates, which `pi` to run).

pub mod view;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    /// Scale applied to the theme's base text size (which sizes the whole interface).
    pub fn factor(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Default => 1.0,
            Self::Large => 1.1,
            Self::Larger => 1.2,
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
    /// The `pi` to run (`PISTACHIO_PI_BIN` still wins); `None` finds it on `PATH`.
    pub pi_program: Option<String>,
    /// Connect every saved SSH connection quietly when the app opens.
    pub connect_hosts_at_start: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            text_size: TextSize::default(),
            start_mode: StartMode::default(),
            reopen_last_session: false,
            background_notifications: true,
            request_attention: true,
            check_for_updates: true,
            skipped_version: None,
            pi_program: None,
            connect_hosts_at_start: false,
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

    /// Reads the settings; a missing or unreadable file gives the defaults.
    pub fn load(path: Option<&Path>) -> Self {
        path.and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
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
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let settings = AppSettings {
            text_size: TextSize::Large,
            start_mode: StartMode::Code,
            skipped_version: Some("0.3.0".into()),
            ..AppSettings::default()
        };
        settings.write(&path).unwrap();
        assert_eq!(AppSettings::load(Some(&path)), settings);
    }
}
