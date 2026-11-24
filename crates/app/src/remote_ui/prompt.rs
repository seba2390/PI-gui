//! ssh's questions (passwords, key passphrases, one-time codes) as an in-app dialog. The
//! answer goes straight back to ssh through the askpass socket; it is never stored.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use remote::Prompt;

use crate::ui::dialog::{on_submit, open_form_dialog};

/// Shows `prompt` and answers it once: with the typed text on "Continue" (or Enter), with
/// "cancelled" when the dialog is dismissed any other way.
pub fn open_prompt(prompt: Prompt, window: &mut Window, cx: &mut App) {
    let secret = prompt.is_secret();
    let text: SharedString = prompt.text.clone().into();
    let pending = RefCell::new(Some(prompt));
    let input = cx.new(|cx| InputState::new(window, cx).masked(secret));
    let answer = Rc::new(move |text: Option<String>| {
        if let Some(prompt) = pending.borrow_mut().take() {
            prompt.answer(text);
        }
    });
    let submit = {
        let answer = answer.clone();
        on_submit(&input, window, cx, move |text, window, cx| {
            answer(Some(text));
            window.close_dialog(cx);
        })
    };
    let focus = input.focus_handle(cx);
    open_form_dialog(Some(focus), window, cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (submit, cancel, closed) = (submit.clone(), answer.clone(), answer.clone());
        dialog
            .title("SSH")
            .w(px(440.))
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(text.clone()))
                    .child(Input::new(&input))
                    .child(div().text_xs().text_color(muted).child(
                        "Sent only to ssh for this connection; Pistachio does not store it.",
                    )),
            )
            .on_close(move |_, _, _| closed(None))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("prompt-cancel").label("Cancel").on_click(
                        move |_, window, cx| {
                            cancel(None);
                            window.close_dialog(cx);
                        },
                    ))
                    .child(
                        Button::new("prompt-ok")
                            .primary()
                            .label("Continue")
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, TryRecvError};

    use gpui_kit::{TestAppContext, VisualTestContext};
    use remote::Prompt;

    use crate::ui::dialog::testing::{click, draw, is_open, press, window};

    fn ask(cx: &mut VisualTestContext) -> Receiver<Option<String>> {
        let (prompt, answer) = Prompt::for_tests("me@host's password:");
        cx.update(|window, cx| super::open_prompt(prompt, window, cx));
        draw(cx);
        answer
    }

    /// The field has the keyboard, and Enter sends what was typed (not "cancelled").
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn enter_answers_with_the_text(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answer = ask(cx);
        cx.simulate_input("secret");
        assert_eq!(answer.try_recv(), Err(TryRecvError::Empty));
        press("enter", cx);
        assert_eq!(answer.try_recv(), Ok(Some("secret".into())));
        assert_eq!(answer.try_recv(), Err(TryRecvError::Disconnected));
        assert!(!is_open(cx));
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn escape_and_cancel_answer_none(cx: &mut TestAppContext) {
        let cx = window(cx);
        let answer = ask(cx);
        cx.simulate_input("secret");
        press("escape", cx);
        assert_eq!(answer.try_recv(), Ok(None));
        assert!(!is_open(cx));

        let answer = ask(cx);
        cx.simulate_input("secret");
        // Enter on the focused button is that button's, not the dialog's.
        press("tab enter", cx);
        assert_eq!(answer.try_recv(), Ok(None));
        assert!(!is_open(cx));

        let answer = ask(cx);
        cx.simulate_input("secret");
        click("prompt-ok", cx);
        assert_eq!(answer.try_recv(), Ok(Some("secret".into())));
    }

    /// A prompt asked while another is open leaves the first one open.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn enter_closes_only_the_prompt_it_answers(cx: &mut TestAppContext) {
        let cx = window(cx);
        let first = ask(cx);
        let second = ask(cx);
        cx.simulate_input("two");
        press("enter", cx);
        assert_eq!(second.try_recv(), Ok(Some("two".into())));
        assert_eq!(first.try_recv(), Err(TryRecvError::Empty));
        assert!(is_open(cx));
        // Back in the first one's field.
        cx.simulate_input("one");
        press("enter", cx);
        assert_eq!(first.try_recv(), Ok(Some("one".into())));
        assert!(!is_open(cx));
    }
}
