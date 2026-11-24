//! Dialogs that leave Enter to the control with the keyboard focus.
//!
//! gpui-kit's dialogs take Enter as "OK": whatever has focus in them, they close. A field's
//! own Enter then comes too late, a focused button is never pressed, and a stray Enter throws
//! away what was typed.

use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{App, ClickEvent, Entity, FocusHandle, Window};

/// Opens a dialog that does not close on Enter: a focused button is pressed, a field reports
/// it (see [`on_submit`]), and anywhere else it does nothing. `focus` is what the dialog starts
/// on, if anything.
pub fn open_form_dialog(
    focus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
) {
    window.open_dialog(cx, move |dialog, window, cx| {
        build(dialog, window, cx).on_ok(leave_enter)
    });
    // Opening a dialog focuses the dialog itself.
    if let Some(focus) = focus {
        focus.focus(window, cx);
    }
}

/// The `on_ok` of a dialog that leaves Enter to the focused control.
pub fn leave_enter(_: &ClickEvent, _: &mut Window, cx: &mut App) -> bool {
    cx.propagate();
    false
}

/// What a button does to submit a dialog.
pub type Submit = Rc<dyn Fn(&mut Window, &mut App)>;

/// Runs `submit` with the text of `input` when Enter is pressed in it. Returns the same for a
/// button to do. `submit` lives as long as `input`, so it must not hold it.
pub fn on_submit(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
    submit: impl Fn(String, &mut Window, &mut App) + 'static,
) -> Submit {
    let submit = Rc::new(submit);
    let on_enter = submit.clone();
    window
        .subscribe(input, cx, move |input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                on_enter(input.read(cx).value().to_string(), window, cx);
            }
        })
        .detach();
    let input = input.clone();
    Rc::new(move |window, cx| submit(input.read(cx).value().to_string(), window, cx))
}

/// Driving dialogs in tests.
#[cfg(test)]
pub mod testing {
    use gpui_kit::component::{Root, WindowExt as _};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Context, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Render,
        TestAppContext, VisualTestContext, Window, div,
    };

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// An empty window that dialogs can open in.
    pub fn window(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(gpui_kit::component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| Empty);
            Root::new(empty, window, cx)
        });
        draw(cx);
        cx
    }

    pub fn draw(cx: &mut VisualTestContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    /// Presses and releases each key (buttons act on the release).
    pub fn press(keys: &'static str, cx: &mut VisualTestContext) {
        for key in keys.split(' ') {
            let keystroke = Keystroke::parse(key).unwrap().with_simulated_ime();
            cx.simulate_event(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            draw(cx);
            cx.simulate_event(KeyUpEvent { keystroke });
            draw(cx);
        }
    }

    pub fn click(id: &'static str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.click(id, cx));
        draw(cx);
    }

    pub fn is_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::input::{Input, InputState};
    use gpui_kit::{
        AppContext as _, Focusable as _, ParentElement as _, TestAppContext, VisualTestContext,
    };
    use pretty_assertions::assert_eq;

    use super::testing::{click, draw, is_open, press, window};
    use super::{on_submit, open_form_dialog};

    /// A dialog with one field, all of it selected, that takes any text but "bad".
    fn open(cx: &mut VisualTestContext) -> Rc<RefCell<Vec<String>>> {
        let taken = Rc::new(RefCell::new(Vec::new()));
        let saved = taken.clone();
        cx.update(|window, cx| {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx).default_value("old");
                input.select_all(window, cx);
                input
            });
            let submit = on_submit(&input, window, cx, move |text, window, cx| {
                saved.borrow_mut().push(text.clone());
                if text != "bad" {
                    window.close_dialog(cx);
                }
            });
            let focus = input.focus_handle(cx);
            open_form_dialog(Some(focus), window, cx, move |dialog, _, _| {
                let submit = submit.clone();
                dialog
                    .title("Name")
                    .child(Input::new(&input))
                    .child(Button::new("save").on_click(move |_, window, cx| submit(window, cx)))
            });
        });
        draw(cx);
        taken
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn enter_submits_the_focused_field(cx: &mut TestAppContext) {
        let cx = window(cx);
        let taken = open(cx);
        press("enter", cx);
        assert_eq!(*taken.borrow(), ["old"]);
        assert!(!is_open(cx));

        let taken = open(cx);
        cx.simulate_input("new");
        press("enter", cx);
        assert_eq!(*taken.borrow(), ["new"]);
        assert!(!is_open(cx));

        let taken = open(cx);
        cx.simulate_input("new");
        click("save", cx);
        assert_eq!(*taken.borrow(), ["new"]);
        assert!(!is_open(cx));

        let taken = open(cx);
        press("escape", cx);
        assert!(taken.borrow().is_empty());
        assert!(!is_open(cx));
    }

    /// What was typed is kept when it is not taken.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn a_dialog_stays_until_the_text_is_taken(cx: &mut TestAppContext) {
        let cx = window(cx);
        let taken = open(cx);
        cx.simulate_input("bad");
        press("enter", cx);
        assert!(is_open(cx));
        cx.simulate_input("ly");
        press("enter", cx);
        assert_eq!(*taken.borrow(), ["bad", "badly"]);
        assert!(!is_open(cx));
    }

    /// A dialog without a focused control does nothing on Enter, and Enter in the one on top
    /// closes only that.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn enter_leaves_other_dialogs_open(cx: &mut TestAppContext) {
        let cx = window(cx);
        cx.update(|window, cx| {
            open_form_dialog(None, window, cx, |dialog, _, _| dialog.title("Below"));
        });
        draw(cx);
        press("enter", cx);
        assert!(is_open(cx));
        open(cx);
        press("enter enter", cx);
        assert!(is_open(cx));
        press("escape", cx);
        assert!(!is_open(cx));
    }
}
