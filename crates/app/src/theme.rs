//! The visual theme: warm neutrals with a deep green accent, in light and dark.
//!
//! gpui-kit components read the semantic tokens from `assets/themes/pi.json`.
//! A few app-specific colours (diff lines, the green accent, sidebar surface) live
//! in [`Palette`], chosen by the active light/dark mode.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Hsla, Window, rgb, rgba};

use crate::assets::AppAssets;
use crate::settings::AppSettings;

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
    if let (Some(light), Some(dark)) = (light, dark) {
        cx.set_global(BaseThemes { light, dark });
        set_code_colors(cx);
    }
    // What is focused has a thin outline, without a glow around it.
    Theme::global_mut(cx).focus_ring = false;
    Theme::sync_system_appearance(None, cx);
}

/// The light and dark themes as `pi.json` defines them, before Code colors are applied.
struct BaseThemes {
    light: Rc<ThemeConfig>,
    dark: Rc<ThemeConfig>,
}

impl Global for BaseThemes {}

/// Puts the chosen Code colors palette into the light and dark themes. Only their syntax
/// colors change; the app's colors, the editor background and the rest stay `pi.json`'s.
fn set_code_colors(cx: &mut App) {
    let colors = cx.default_global::<AppSettings>().code_colors;
    let Some(base) = cx.try_global::<BaseThemes>() else {
        return;
    };
    let with_colors = |config: &Rc<ThemeConfig>, dark: bool| match colors.syntax(dark) {
        Some(syntax) => {
            let mut config = (**config).clone();
            config.highlight.get_or_insert_default().syntax = syntax;
            Rc::new(config)
        }
        None => config.clone(),
    };
    let (light, dark) = (
        with_colors(&base.light, false),
        with_colors(&base.dark, true),
    );
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
}

/// The syntax colors `palette` uses in light or dark mode, without applying them: the
/// Settings preview shows a palette while the pointer is on it.
pub fn code_colors_syntax(
    palette: crate::code_colors::CodeColors,
    dark: bool,
    cx: &App,
) -> gpui_kit::component::highlighter::SyntaxColors {
    palette.syntax(dark).unwrap_or_else(|| {
        cx.try_global::<BaseThemes>()
            .and_then(|base| {
                let config = if dark { &base.dark } else { &base.light };
                config.highlight.as_ref().map(|style| style.syntax.clone())
            })
            .unwrap_or_default()
    })
}

/// Applies a changed Code colors setting to the open windows.
pub fn apply_code_colors(cx: &mut App) {
    set_code_colors(cx);
    // Re-apply the current mode so code blocks, editors and the text view defaults pick up
    // the new colors; that resets the text size, so restore it.
    let mode = cx.theme().mode;
    Theme::change(mode, None, cx);
    crate::settings::apply_text_size(cx);
}

/// Light or dark as chosen in Settings, following the OS for `window` by default.
pub fn sync_with_window(window: &mut Window, cx: &mut App) {
    use crate::settings::ThemeChoice;
    let mode = match cx.default_global::<AppSettings>().theme {
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::System => match window.appearance() {
            gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark => {
                ThemeMode::Dark
            }
            gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
                ThemeMode::Light
            }
        },
    };
    if cx.theme().mode != mode {
        Theme::change(mode, Some(window), cx);
        crate::settings::apply_text_size(cx);
    }
}

/// App-specific colours not covered by gpui-kit's semantic tokens.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub accent: Hsla,
    /// The accent for small glyphs, such as a button's icon.
    pub accent_icon: Hsla,
    pub subtle: Hsla,
    pub code_bg: Hsla,
    pub added: Hsla,
    pub added_bg: Hsla,
    pub removed: Hsla,
    pub removed_bg: Hsla,
    pub success: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    /// A switch that is on: its track and outline.
    pub switch_on: Hsla,
    pub switch_on_edge: Hsla,
    /// The outline of a switch that is off (its track is the theme's).
    pub switch_edge: Hsla,
    pub switch_thumb: Hsla,
}

pub fn palette(cx: &App) -> Palette {
    if cx.theme().mode.is_dark() {
        Palette {
            sidebar: rgb(0x001F_1E1D).into(),
            surface: rgb(0x0030_302E).into(),
            accent: rgb(0x0024_A926).into(),
            accent_icon: rgb(0x005C_C45E).into(),
            subtle: rgb(0x0077_746C).into(),
            code_bg: rgb(0x001F_1E1D).into(),
            added: rgb(0x007F_B069).into(),
            added_bg: rgba(0x7FB0_6926).into(),
            removed: rgb(0x00E5_675B).into(),
            removed_bg: rgba(0xE567_5B26).into(),
            success: rgb(0x007F_B069).into(),
            error: rgb(0x00E5_675B).into(),
            warning: rgb(0x00D9_A63C).into(),
            switch_on: rgb(0x001F_7A3C).into(),
            switch_on_edge: rgb(0x005C_C45E).into(),
            switch_edge: rgb(0x008B_877E).into(),
            switch_thumb: rgb(0x00F5_F3EC).into(),
        }
    } else {
        Palette {
            sidebar: rgb(0x00F3_F1EA).into(),
            surface: rgb(0x00FF_FFFF).into(),
            accent: rgb(0x0000_573F).into(),
            accent_icon: rgb(0x0000_573F).into(),
            subtle: rgb(0x00A3_9F95).into(),
            code_bg: rgb(0x00F3_F1EA).into(),
            added: rgb(0x004E_8A3A).into(),
            added_bg: rgba(0x4E8A_3A1F).into(),
            removed: rgb(0x00C2_463A).into(),
            removed_bg: rgba(0xC246_3A1F).into(),
            success: rgb(0x004E_8A3A).into(),
            error: rgb(0x00C2_463A).into(),
            warning: rgb(0x00A8_7A16).into(),
            switch_on: rgb(0x001B_7A3A).into(),
            switch_on_edge: rgb(0x000F_5225).into(),
            switch_edge: rgb(0x008F_8B80).into(),
            switch_thumb: rgb(0x00FF_FFFF).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::input::{Input, InputState};
    use gpui_kit::component::{ActiveTheme as _, Root, Theme, ThemeMode};
    use gpui_kit::{
        AppContext as _, Context, Entity, Hsla, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, div, px,
    };

    struct Field(Entity<InputState>);

    impl Render for Field {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(300.)).p_4().child(Input::new(&self.0))
        }
    }

    /// A focused text field has one thin outline, which is not green, in both themes.
    #[gpui_kit::test]
    fn a_focused_field_has_a_plain_outline(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            super::init(cx);
        });
        let mut input = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| InputState::new(window, cx));
            input = Some(state.clone());
            let field = cx.new(|_| Field(state));
            Root::new(field, window, cx)
        });
        let input = input.unwrap();
        cx.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|window, cx| Theme::change(mode, Some(window), cx));
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                let ring = cx.theme().ring;
                assert!(ring.s < 0.3, "{mode:?}: {ring:?}");
                let scale = window.scale_factor();
                let none = px(0.).scale(scale);
                // An outline is painted in pieces, all with its bounds. A glow is around it, in
                // the same color but fainter.
                let outlines: Vec<_> = (window.painted_quads().into_iter())
                    .filter(|quad| {
                        let color = Hsla {
                            a: 1.,
                            ..quad.border_color
                        };
                        color == ring && quad.border_color.a > 0. && quad.border_widths.top > none
                    })
                    .collect();
                assert!(!outlines.is_empty(), "{mode:?}: no outline");
                for quad in &outlines {
                    assert_eq!(quad.bounds, outlines[0].bounds, "{mode:?}: a glow");
                    assert_eq!(quad.border_color, ring, "{mode:?}: a glow");
                    assert!(quad.border_widths.top <= px(1.).scale(scale));
                }
            });
        }
    }
}
