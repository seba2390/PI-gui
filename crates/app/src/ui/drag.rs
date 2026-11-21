//! Makes an element behave like a title bar: drag to move, double-click to zoom.
//!
//! The window uses a transparent, app-owned title bar (as gpui-kit's `TitleBar`
//! does), so each column's top bar is a drag region of its own.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::InteractiveElementExt as _;
use gpui_kit::{
    Div, InteractiveElement as _, MouseButton, MouseMoveEvent, Stateful, WindowControlArea,
};

/// Per-region drag state; clone it into each render.
#[derive(Clone, Default)]
pub struct WindowDrag {
    should_move: Rc<Cell<bool>>,
}

impl WindowDrag {
    pub fn apply(&self, element: Stateful<Div>) -> Stateful<Div> {
        let (down, up, moved) = (
            self.should_move.clone(),
            self.should_move.clone(),
            self.should_move.clone(),
        );
        element
            .window_control_area(WindowControlArea::Drag)
            .on_double_click(|_, window, _| {
                if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
                }
            })
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                // A button inside the bar that handled the press prevents default.
                down.set(!window.default_prevented());
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(false))
            .on_mouse_move(move |event: &MouseMoveEvent, window, _| {
                if event.pressed_button != Some(MouseButton::Left) {
                    moved.set(false);
                } else if moved.replace(false) {
                    window.start_window_move();
                }
            })
    }
}
