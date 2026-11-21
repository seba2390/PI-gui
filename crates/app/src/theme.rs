//! The visual theme: a warm, Claude-desktop-like palette in light and dark.
//!
//! gpui-kit components read the semantic tokens from `assets/themes/pi.json`.
//! A few app-specific colours (diff lines, the clay accent, sidebar surface) live
//! in [`Palette`], chosen by the active light/dark mode.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Hsla, Window, rgb, rgba};

use crate::assets::AppAssets;

const LIGHT: &str = "Pi Light";
const DARK: &str = "Pi Dark";

pub fn init(cx: &mut App) {
    let json = AppAssets::get("themes/pi.json").and_then(|bytes| std::str::from_utf8(bytes).ok());
    if let Some(json) = json
        && let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(json)
    {
        log::error!("failed to load the pi theme: {error:#}");
    }
    let themes = ThemeRegistry::global(cx).themes();
    let (light, dark) = (themes.get(LIGHT).cloned(), themes.get(DARK).cloned());
    if let Some(light) = light {
        Theme::global_mut(cx).light_theme = light;
    }
    if let Some(dark) = dark {
        Theme::global_mut(cx).dark_theme = dark;
    }
    Theme::sync_system_appearance(None, cx);
}

/// Follows the OS light/dark setting for `window`.
pub fn sync_with_window(window: &mut Window, cx: &mut App) {
    let mode = match window.appearance() {
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark => {
            ThemeMode::Dark
        }
        gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
            ThemeMode::Light
        }
    };
    if cx.theme().mode != mode {
        Theme::change(mode, Some(window), cx);
    }
}

/// App-specific colours not covered by gpui-kit's semantic tokens.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub accent: Hsla,
    pub subtle: Hsla,
    pub code_bg: Hsla,
    pub added: Hsla,
    pub added_bg: Hsla,
    pub removed: Hsla,
    pub removed_bg: Hsla,
    pub success: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
}

pub fn palette(cx: &App) -> Palette {
    if cx.theme().mode.is_dark() {
        Palette {
            sidebar: rgb(0x001F_1E1D).into(),
            surface: rgb(0x0030_302E).into(),
            accent: rgb(0x00D9_7757).into(),
            subtle: rgb(0x0077_746C).into(),
            code_bg: rgb(0x001F_1E1D).into(),
            added: rgb(0x007F_B069).into(),
            added_bg: rgba(0x7FB0_6926).into(),
            removed: rgb(0x00E5_675B).into(),
            removed_bg: rgba(0xE567_5B26).into(),
            success: rgb(0x007F_B069).into(),
            error: rgb(0x00E5_675B).into(),
            warning: rgb(0x00D9_A63C).into(),
        }
    } else {
        Palette {
            sidebar: rgb(0x00F3_F1EA).into(),
            surface: rgb(0x00FF_FFFF).into(),
            accent: rgb(0x00C9_6442).into(),
            subtle: rgb(0x00A3_9F95).into(),
            code_bg: rgb(0x00F3_F1EA).into(),
            added: rgb(0x004E_8A3A).into(),
            added_bg: rgba(0x4E8A_3A1F).into(),
            removed: rgb(0x00C2_463A).into(),
            removed_bg: rgba(0xC246_3A1F).into(),
            success: rgb(0x004E_8A3A).into(),
            error: rgb(0x00C2_463A).into(),
            warning: rgb(0x00A8_7A16).into(),
        }
    }
}
