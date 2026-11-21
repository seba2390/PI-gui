//! The app's standard toolbar button style.

use gpui_kit::App;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};

/// Transparent at rest with a clearly visible hover fill, matching the hover of sidebar rows.
///
/// gpui-kit's `ghost` hovers with the accent color at half opacity in dark mode, which is
/// nearly invisible on the composer surface.
pub trait QuietButton {
    fn quiet(self, cx: &App) -> Self;
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
}
