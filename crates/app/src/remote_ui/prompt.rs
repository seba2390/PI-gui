//! ssh's questions (passwords, key passphrases, one-time codes) as an in-app dialog. The
//! answer goes straight back to ssh through the askpass socket; it is never stored.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use remote::Prompt;

/// Shows `prompt` and answers it once: with the typed text on "Continue" (or Enter), with
/// "cancelled" when the dialog is dismissed any other way.
pub fn open_prompt(prompt: Prompt, window: &mut Window, cx: &mut App) {
    let secret = prompt.is_secret();
    let text: SharedString = prompt.text.clone().into();
    let pending = Rc::new(RefCell::new(Some(prompt)));
    let input = cx.new(|cx| InputState::new(window, cx).masked(secret));
    let answer = {
        let (pending, input) = (pending.clone(), input.clone());
        move |ok: bool, cx: &App| {
            if let Some(prompt) = pending.borrow_mut().take() {
                let value = input.read(cx).value().to_string();
                prompt.answer(ok.then_some(value));
            }
        }
    };
    let answer = Rc::new(answer);
    {
        let answer = answer.clone();
        let handle = window.window_handle();
        cx.subscribe(&input, move |_, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                answer(true, cx);
                let _ = handle.update(cx, |_, window, cx| window.close_dialog(cx));
            }
        })
        .detach();
    }
    input.update(cx, |input, cx| input.focus(window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (ok, cancel, closed) = (answer.clone(), answer.clone(), answer.clone());
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
            .on_close(move |_, _, cx| closed(false, cx))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("prompt-cancel").label("Cancel").on_click(
                        move |_, window, cx| {
                            cancel(false, cx);
                            window.close_dialog(cx);
                        },
                    ))
                    .child(
                        Button::new("prompt-ok")
                            .primary()
                            .label("Continue")
                            .on_click(move |_, window, cx| {
                                ok(true, cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}
