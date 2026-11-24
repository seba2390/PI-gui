//! A switch with the app's hover fill around it.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, FocusableExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// It shows a focus ring for the keyboard only: a click focuses the switch too, and the ring
/// would stay after it. (The window redraws when the kind of input changes.)
pub fn hover_switch(
    id: impl Into<ElementId>,
    checked: bool,
    on_click: impl Fn(bool, &mut Window, &mut App) + 'static,
    accessibility_label: Option<SharedString>,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let id = id.into();
    let hover = cx.theme().secondary_hover;
    div()
        .id(id.clone())
        .p(px(3.))
        .rounded_full()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(
            Switch::new(id)
                .checked(checked)
                .focus_ring(window.last_input_was_keyboard())
                .when_some(accessibility_label, |this, label| {
                    this.accessibility_label(label)
                })
                .on_click(move |checked: &bool, window, cx| on_click(*checked, window, cx)),
        )
}
