//! Extension dialogs (`ctx.ui.select/confirm/input/editor`) as modal dialogs.
//!
//! Every dialog is answered exactly once: by a button (or Enter in a field), or `cancelled`
//! when it is dismissed. pi's own timeouts still apply on its side.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};

use super::session::PiSession;
use crate::ui::dialog::{Submit, leave_enter, on_submit, open_form_dialog};

type SendAnswer = Rc<dyn Fn(&ExtensionUiAnswer, &mut App)>;

/// Sends one answer, ignoring any later attempts.
#[derive(Clone)]
struct Responder {
    answer: SendAnswer,
    answered: Rc<Cell<bool>>,
}

impl Responder {
    fn new(answer: impl Fn(&ExtensionUiAnswer, &mut App) + 'static) -> Self {
        Self {
            answer: Rc::new(answer),
            answered: Rc::new(Cell::new(false)),
        }
    }

    fn send(&self, answer: &ExtensionUiAnswer, cx: &mut App) {
        if !self.answered.replace(true) {
            (self.answer)(answer, cx);
        }
    }
}

pub fn open(
    request: ExtensionUiRequest,
    session: Entity<PiSession>,
    window: &mut Window,
    cx: &mut App,
) {
    let ExtensionUiRequest { id, method } = request;
    let responder = Responder::new(move |answer, cx| {
        session.update(cx, |session, _| session.answer_dialog(&id, answer));
    });
    open_method(method, responder, window, cx);
}

fn open_method(method: ExtensionUiMethod, responder: Responder, window: &mut Window, cx: &mut App) {
    match method {
        ExtensionUiMethod::Confirm { title, message, .. } => {
            open_confirm(title, message, responder, window, cx);
        }
        ExtensionUiMethod::Select { title, options, .. } => {
            open_select(title, options, responder, window, cx);
        }
        ExtensionUiMethod::Input {
            title, placeholder, ..
        } => {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx);
                match &placeholder {
                    Some(placeholder) => state.placeholder(placeholder.clone()),
                    None => state,
                }
            });
            let submit = {
                let responder = responder.clone();
                on_submit(&input, window, cx, move |text, window, cx| {
                    responder.send(&ExtensionUiAnswer::Value(text), cx);
                    window.close_dialog(cx);
                })
            };
            let focus = input.focus_handle(cx);
            let field = move || Input::new(&input).into_any_element();
            open_text_dialog(title, focus, field, submit, responder, window, cx);
        }
        ExtensionUiMethod::Editor { title, prefill, .. } => {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(6, 20)
                    .default_value(prefill.clone().unwrap_or_default())
            });
            let submit = {
                let (responder, input) = (responder.clone(), input.clone());
                Rc::new(move |window: &mut Window, cx: &mut App| {
                    let text = input.read(cx).value().to_string();
                    responder.send(&ExtensionUiAnswer::Value(text), cx);
                    window.close_dialog(cx);
                })
            };
            let focus = input.focus_handle(cx);
            let field = move || Textarea::new(&input).into_any_element();
            open_text_dialog(title, focus, field, submit, responder, window, cx);
        }
        _ => {}
    }
}

/// A yes/no question. It takes the keyboard from whatever the user was typing in, so "yes"
/// takes a click, or Enter or Space on the focused button: never an Enter meant for that text.
fn open_confirm(
    title: String,
    message: String,
    responder: Responder,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let button = |id: &'static str, label: &'static str, yes: bool| {
            let responder = responder.clone();
            Button::new(id).label(label).on_click(move |_, window, cx| {
                responder.send(&ExtensionUiAnswer::Confirmed(yes), cx);
                window.close_dialog(cx);
            })
        };
        let dismissed = responder.clone();
        alert
            .title(title.clone())
            .description(message.clone())
            .footer(
                DialogFooter::new()
                    .child(button("ext-no", "Cancel", false))
                    .child(button("ext-yes", "OK", true).primary()),
            )
            .on_ok(leave_enter)
            .on_cancel(move |_, _, cx| {
                dismissed.send(&ExtensionUiAnswer::Confirmed(false), cx);
                true
            })
    });
}

fn open_select(
    title: String,
    options: Vec<String>,
    responder: Responder,
    window: &mut Window,
    cx: &mut App,
) {
    let count = options.len();
    let choose = responder.clone();
    let list = cx.new(|cx| {
        ChoiceList::new(count, cx, move |index, focus, _| {
            let (responder, option) = (choose.clone(), options[index].clone());
            let button = Button::new(("ext-option", index))
                .outline()
                .w_full()
                .track_focus(focus)
                .label(option.clone())
                .on_click(move |_, window, cx| {
                    responder.send(&ExtensionUiAnswer::Value(option.clone()), cx);
                    window.close_dialog(cx);
                });
            // Room for the focus ring, which the scrolling list would cut off.
            div()
                .flex_none()
                .px_0p5()
                .py_1()
                .child(button)
                .into_any_element()
        })
    });
    let focus = list.focus_handle(cx);
    // A question over several lines: the first is the title, the rest what it is about (the
    // sandbox's gate lists the commits and what needs care that way).
    let (heading, about) = match title.split_once('\n') {
        Some((heading, about)) => (heading.to_owned(), Some(about.to_owned())),
        None => (title, None),
    };
    open_form_dialog(Some(focus), window, cx, move |dialog, _, cx| {
        let on_close = responder.clone();
        let muted = cx.theme().muted_foreground;
        dialog
            .title(heading.clone())
            .when_some(about.clone(), |this, about| {
                this.child(
                    v_flex()
                        .gap_0p5()
                        .pb_2()
                        .text_sm()
                        .text_color(muted)
                        .children(about.lines().map(|line| div().child(line.to_owned()))),
                )
            })
            .child(list.clone())
            .on_close(move |_, _, cx| on_close.send(&ExtensionUiAnswer::Cancelled, cx))
    });
}

fn open_text_dialog(
    title: String,
    focus: FocusHandle,
    field: impl Fn() -> AnyElement + 'static,
    submit: Submit,
    responder: Responder,
    window: &mut Window,
    cx: &mut App,
) {
    open_form_dialog(Some(focus), window, cx, move |dialog, _, _| {
        let (submit, cancel, on_close) = (submit.clone(), responder.clone(), responder.clone());
        dialog
            .title(title.clone())
            .w(px(520.))
            .child(field())
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("ext-cancel").label("Cancel").on_click(
                        move |_, window, cx| {
                            cancel.send(&ExtensionUiAnswer::Cancelled, cx);
                            window.close_dialog(cx);
                        },
                    ))
                    .child(
                        Button::new("ext-submit")
                            .primary()
                            .label("Submit")
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
            )
            .on_close(move |_, _, cx| on_close.send(&ExtensionUiAnswer::Cancelled, cx))
    });
}

/// One choice in a [`open_picker`] list.
pub struct PickerItem {
    pub label: String,
    pub detail: Option<String>,
}

/// A dialog with a scrollable list to pick one entry from; `on_pick` gets its index.
pub fn open_picker(
    title: &str,
    items: Vec<PickerItem>,
    on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.to_owned();
    let on_pick = Rc::new(on_pick);
    let count = items.len();
    let list = cx.new(|cx| {
        ChoiceList::new(count, cx, move |index, focus, cx| {
            let (muted, hover) = (cx.theme().muted_foreground, cx.theme().secondary_hover);
            let (item, on_pick) = (&items[index], on_pick.clone());
            v_flex()
                .id(("picker-row", index))
                .track_focus(focus)
                .w_full()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .focus(move |style| style.bg(hover))
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    on_pick(index, window, cx);
                })
                .child(div().text_sm().truncate().child(item.label.clone()))
                .children(
                    item.detail
                        .clone()
                        .map(|detail| div().text_xs().truncate().text_color(muted).child(detail)),
                )
                .into_any_element()
        })
    });
    let focus = list.focus_handle(cx);
    open_form_dialog(Some(focus), window, cx, move |dialog, _, _| {
        dialog.title(title.clone()).w(px(560.)).child(list.clone())
    });
}

type RenderChoice = Box<dyn Fn(usize, &FocusHandle, &App) -> AnyElement>;

/// A scrollable list to choose from with the mouse or the keyboard: ↑, ↓ and Tab move to an
/// entry, Enter and Space choose it. Nothing is chosen until the user moves to it.
struct ChoiceList {
    focus: FocusHandle,
    entries: Vec<FocusHandle>,
    /// The entry that had focus when the list was last drawn.
    focused: Option<usize>,
    scroll: ScrollHandle,
    render_entry: RenderChoice,
}

impl ChoiceList {
    /// `render_entry` draws an entry that tracks the given focus and acts on a click.
    fn new(
        count: usize,
        cx: &mut Context<Self>,
        render_entry: impl Fn(usize, &FocusHandle, &App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            entries: (0..count)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            focused: None,
            scroll: ScrollHandle::new(),
            render_entry: Box::new(render_entry),
        }
    }

    fn focused(&self, window: &Window) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.is_focused(window))
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.entries.len().saturating_sub(1);
        let next = match (event.keystroke.key.as_str(), self.focused(window)) {
            ("down", None) => 0,
            ("down", Some(index)) => last.min(index + 1),
            ("up", None) => last,
            ("up", Some(index)) => index.saturating_sub(1),
            _ => return,
        };
        if let Some(entry) = self.entries.get(next) {
            entry.focus(window, cx);
            cx.stop_propagation();
        }
    }
}

impl Focusable for ChoiceList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChoiceList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focused(window);
        if self.focused != focused {
            self.focused = focused;
            if let Some(index) = focused {
                self.scroll.scroll_to_item(index);
            }
        }
        let entries: Vec<AnyElement> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, focus)| (self.render_entry)(index, focus, cx))
            .collect();
        v_flex()
            .id("choices")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .max_h(px(420.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap_0p5()
            .children(entries)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui_kit::{TestAppContext, VisualTestContext};
    use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod};
    use pretty_assertions::assert_eq;

    use super::{PickerItem, Responder};
    use crate::ui::dialog::testing::{click, draw, is_open, press, window};

    type Answers = Rc<RefCell<Vec<ExtensionUiAnswer>>>;

    fn ask(method: ExtensionUiMethod, cx: &mut VisualTestContext) -> Answers {
        let answers = Answers::default();
        let sent = answers.clone();
        let responder = Responder::new(move |answer, _| sent.borrow_mut().push(answer.clone()));
        cx.update(|window, cx| super::open_method(method, responder, window, cx));
        draw(cx);
        answers
    }

    fn confirm() -> ExtensionUiMethod {
        ExtensionUiMethod::Confirm {
            title: "Allow rm -rf?".into(),
            message: "rm -rf /tmp/x".into(),
            timeout: None,
        }
    }

    fn select(count: usize) -> ExtensionUiMethod {
        ExtensionUiMethod::Select {
            title: "Pick one".into(),
            options: (0..count).map(|index| format!("option {index}")).collect(),
            timeout: None,
        }
    }

    fn input() -> ExtensionUiMethod {
        ExtensionUiMethod::Input {
            title: "Branch name".into(),
            placeholder: None,
            timeout: None,
        }
    }

    fn value(text: &str) -> ExtensionUiAnswer {
        ExtensionUiAnswer::Value(text.into())
    }

    /// Keys meant for whatever was typed in before the question came do not answer it.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn a_confirm_ignores_stray_keys(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answers = ask(confirm(), cx);
        press("enter space a enter cmd-enter", cx);
        assert_eq!(*answers.borrow(), []);
        assert!(is_open(cx));
        press("escape", cx);
        assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Confirmed(false)]);
        assert!(!is_open(cx));
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn a_confirm_takes_its_buttons(cx: &mut TestAppContext) {
        let cx = window(cx);
        for (keys, yes) in [
            ("tab enter", false),
            ("tab space", false),
            ("tab tab enter", true),
            ("tab tab space", true),
        ] {
            let answers = ask(confirm(), cx);
            press(keys, cx);
            assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Confirmed(yes)]);
            assert!(!is_open(cx));
        }
        for (id, yes) in [("ext-no", false), ("ext-yes", true)] {
            let answers = ask(confirm(), cx);
            click(id, cx);
            assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Confirmed(yes)]);
            assert!(!is_open(cx));
        }
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn enter_submits_an_input(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answers = ask(input(), cx);
        cx.simulate_input("fix");
        press("enter", cx);
        assert_eq!(*answers.borrow(), [value("fix")]);
        assert!(!is_open(cx));

        let answers = ask(input(), cx);
        cx.simulate_input("fix");
        press("escape", cx);
        assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Cancelled]);

        let answers = ask(input(), cx);
        cx.simulate_input("fix");
        press("tab enter", cx);
        assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Cancelled]);
        assert!(!is_open(cx));
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn enter_is_a_newline_in_an_editor(cx: &mut TestAppContext) {
        let cx = window(cx);
        let method = ExtensionUiMethod::Editor {
            title: "Edit".into(),
            prefill: None,
        };
        let answers = ask(method, cx);
        cx.simulate_input("one");
        press("enter", cx);
        cx.simulate_input("two");
        assert!(is_open(cx));
        assert_eq!(*answers.borrow(), []);
        click("ext-submit", cx);
        assert_eq!(*answers.borrow(), [value("one\ntwo")]);
        assert!(!is_open(cx));
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn the_arrow_keys_choose_in_a_select(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answers = ask(select(3), cx);
        press("enter space", cx);
        assert_eq!(*answers.borrow(), []);
        assert!(is_open(cx));
        press("down down enter", cx);
        assert_eq!(*answers.borrow(), [value("option 1")]);
        assert!(!is_open(cx));

        for (keys, picked) in [
            ("up enter", 2),
            ("down up down down down down enter", 2),
            ("tab enter", 0),
            ("tab tab space", 1),
        ] {
            let answers = ask(select(3), cx);
            press(keys, cx);
            assert_eq!(*answers.borrow(), [value(&format!("option {picked}"))]);
        }

        let answers = ask(select(3), cx);
        press("down escape", cx);
        assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Cancelled]);
    }

    /// The entry the keyboard is on scrolls into view.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn a_long_select_scrolls(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answers = ask(select(80), cx);
        press("up", cx);
        click_option(79, cx);
        assert_eq!(*answers.borrow(), [value("option 79")]);
    }

    fn click_option(index: usize, cx: &mut VisualTestContext) {
        use gpui_kit::test::TestWindowExt as _;
        cx.update(|window, cx| window.click(("ext-option", index), cx));
        draw(cx);
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn a_picker_picks_with_the_keyboard(cx: &mut TestAppContext) {
        let cx = window(cx);
        let picked = Rc::new(Cell::new(None));
        let pick = picked.clone();
        cx.update(|window, cx| {
            let items = (0..3)
                .map(|index| PickerItem {
                    label: format!("item {index}"),
                    detail: None,
                })
                .collect();
            super::open_picker(
                "Pick",
                items,
                move |index, _, _| pick.set(Some(index)),
                window,
                cx,
            );
        });
        draw(cx);
        press("enter", cx);
        assert_eq!(picked.get(), None);
        assert!(is_open(cx));
        press("down down enter", cx);
        assert_eq!(picked.get(), Some(1));
        assert!(!is_open(cx));
    }
}
