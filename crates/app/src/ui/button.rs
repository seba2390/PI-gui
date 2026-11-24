//! The app's standard button styles.

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::{App, Styled as _, black, px, white};

/// Transparent at rest with a clearly visible hover fill, matching the hover of sidebar rows.
///
/// gpui-kit's `ghost` hovers with the accent color at half opacity in dark mode, which is
/// nearly invisible on the composer surface.
pub trait QuietButton {
    fn quiet(self, cx: &App) -> Self;

    /// What a form is for (Create, Save): filled green.
    fn form_action(self, cx: &App) -> Self;

    /// Another button beside [`Self::form_action`]: an outline.
    fn form_other(self, cx: &App) -> Self;

    /// [`Self::form_other`] for something that removes.
    fn form_danger(self, cx: &App) -> Self;

    /// One of a few things to choose from, as a card that fills its place.
    fn choice_card(self, chosen: bool, cx: &App) -> Self;

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

    fn form_action(self, cx: &App) -> Self {
        let colors = crate::theme::palette(cx);
        let fill = colors.switch_on;
        self.custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(white())
                .hover(fill.blend(white().opacity(0.12)))
                .active(fill.blend(black().opacity(0.15))),
        )
        .form_size()
        .border_color(fill)
        .bg(fill)
    }

    fn form_other(self, cx: &App) -> Self {
        let theme = cx.theme();
        self.custom(
            ButtonCustomVariant::new(cx)
                .color(theme.input)
                .foreground(theme.foreground)
                .hover(theme.secondary_hover)
                .active(theme.secondary_active),
        )
        .form_size()
        .border_color(theme.input)
        .bg(theme.background)
    }

    fn form_danger(self, cx: &App) -> Self {
        let color = crate::theme::palette(cx).error;
        self.custom(
            ButtonCustomVariant::new(cx)
                .color(color.opacity(0.5))
                .foreground(color)
                .hover(color.opacity(0.14))
                .active(color.opacity(0.22)),
        )
        .form_size()
        .border_color(color.opacity(0.5))
        .bg(cx.theme().background)
    }

    fn choice_card(self, chosen: bool, cx: &App) -> Self {
        let theme = cx.theme();
        let green = crate::theme::palette(cx).accent_icon;
        let (edge, fill) = if chosen {
            (green, theme.background.blend(green.opacity(0.1)))
        } else {
            (theme.input, theme.background)
        };
        self.custom(
            ButtonCustomVariant::new(cx)
                .color(edge)
                .foreground(theme.foreground)
                .hover(theme.background.blend(theme.secondary_hover.opacity(0.6)))
                .active(theme.secondary_active),
        )
        .w_full()
        .h(px(54.))
        .px_3()
        .border_1()
        .border_color(edge)
        .bg(fill)
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

trait FormSize {
    fn form_size(self) -> Self;
}

impl FormSize for Button {
    fn form_size(self) -> Self {
        self.h(px(30.)).px_3().border_1()
    }
}
