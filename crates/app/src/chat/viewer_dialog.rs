//! What the full-window viewers of files have in common: the dialog they are in, and the bar at
//! its top.

use std::path::Path;

use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;

use crate::ui::QuietButton as _;
use crate::ui::dialog::leave_enter;

const MAX_WIDTH: f32 = 1180.;
pub const TOOLBAR: f32 = 44.;

/// Opens `viewer` with the keyboard on `focus`. Esc or a click outside closes it.
pub fn open<V: Render>(viewer: Entity<V>, focus: FocusHandle, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, window, cx| {
        dialog
            .w(dialog_size(window, cx).width)
            .centered()
            .p_0()
            .close_button(false)
            .on_ok(leave_enter)
            .child(viewer.clone())
    });
    // After the dialog has taken focus, for the keys.
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

fn dialog_size(window: &Window, cx: &App) -> Size<Pixels> {
    let margin = cx.theme().spacing_tokens().lg * 2.;
    let window = window.viewport_size();
    size(
        (window.width - margin).min(px(MAX_WIDTH)),
        window.height - margin,
    )
}

/// The size of what is in the dialog, without its border.
pub fn area(window: &Window, cx: &App) -> Size<Pixels> {
    dialog_size(window, cx) - size(px(2.), px(2.))
}

/// The bar, for `children` to be added to: first what is before the name, then the name
/// ([`name`]), then buttons, and last [`close`].
pub fn toolbar(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(TOOLBAR))
        .px_3()
        .gap_1()
        .border_b_1()
        .border_color(cx.theme().border)
}

pub fn name(path: &Path) -> Div {
    let name = path
        .file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned());
    div().flex_1().min_w_0().text_sm().truncate().child(name)
}

pub fn button(
    id: &'static str,
    icon: impl Into<component::Icon>,
    label: &'static str,
    cx: &App,
) -> Button {
    Button::new(id)
        .quiet(cx)
        .small()
        .icon(icon)
        .accessibility_label(label)
        .tooltip(label)
}

pub fn close(cx: &App) -> Button {
    button("viewer-close", IconName::Close, "Close", cx)
        .on_click(|_, window, cx| window.close_dialog(cx))
}

/// Instead of what cannot be shown: why, and `instead`.
pub fn message(text: SharedString, instead: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .px_4()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
        .child(instead)
}
