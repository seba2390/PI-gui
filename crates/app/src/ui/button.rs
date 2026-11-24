//! The app's standard button styles.

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::{App, Styled as _, px};

/// Transparent at rest with a clearly visible hover fill, matching the hover of sidebar rows.
///
/// gpui-kit's `ghost` hovers with the accent color at half opacity in dark mode, which is
/// nearly invisible on the composer surface.
pub trait QuietButton {
    fn quiet(self, cx: &App) -> Self;

    /// The one button of a page that is otherwise empty (a new chat, session or skill): an
    /// outline with the accent on `icon` only. Filled like the window, so the welcome screen's
    /// drifting signs do not show through.
    fn quiet_outline(self, icon: impl Into<Icon>, cx: &App) -> Self;
}

impl QuietButton for Button {
    fn quiet(self, cx: &App) -> Self {
        let theme = cx.theme();
        self.custom(
            ButtonCustomVariant::new(cx)
                .foreground(theme.secondary_foreground)
                .hover(theme.secondary_hover)
                .active(theme.secondary_active),
        )
    }

    fn quiet_outline(self, icon: impl Into<Icon>, cx: &App) -> Self {
        let theme = cx.theme();
        self.custom(
            ButtonCustomVariant::new(cx)
                // The border, also under the pointer.
                .color(theme.input)
                .foreground(theme.foreground)
                .hover(theme.secondary_hover)
                .active(theme.secondary_active),
        )
        .h(px(36.))
        .px_4()
        .border_1()
        .border_color(theme.input)
        .bg(theme.background)
        .icon(
            icon.into()
                .text_color(crate::theme::palette(cx).accent_icon),
        )
    }
}
