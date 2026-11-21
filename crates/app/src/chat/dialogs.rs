//! Extension dialogs (`ctx.ui.select/confirm/input/editor`) as modal dialogs.
//!
//! Every dialog is answered exactly once: by a button, or `cancelled` when it is
//! dismissed. pi's own timeouts still apply on its side.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};

use super::session::PiSession;

/// Sends one answer, ignoring any later attempts.
#[derive(Clone)]
struct Responder {
    id: Rc<str>,
    session: Entity<PiSession>,
    answered: Rc<Cell<bool>>,
}

impl Responder {
    fn send(&self, answer: &ExtensionUiAnswer, cx: &mut App) {
        if !self.answered.replace(true) {
            let id = self.id.clone();
            self.session
                .update(cx, |session, _| session.answer_dialog(&id, answer));
        }
    }
}

pub fn open(
    request: ExtensionUiRequest,
    session: Entity<PiSession>,
    window: &mut Window,
    cx: &mut App,
) {
    let responder = Responder {
        id: request.id.into(),
        session,
        answered: Rc::new(Cell::new(false)),
    };
    match request.method {
        ExtensionUiMethod::Confirm { title, message, .. } => {
            let (ok, cancel) = (responder.clone(), responder.clone());
            window.open_alert_dialog(cx, move |alert, _, _| {
                let (ok, cancel) = (ok.clone(), cancel.clone());
                alert
                    .title(title.clone())
                    .description(message.clone())
                    .confirm()
                    .on_ok(move |_, _, cx| {
                        ok.send(&ExtensionUiAnswer::Confirmed(true), cx);
                        true
                    })
                    .on_cancel(move |_, _, cx| {
                        cancel.send(&ExtensionUiAnswer::Confirmed(false), cx);
                        true
                    })
            });
        }
        ExtensionUiMethod::Select { title, options, .. } => {
            window.open_dialog(cx, move |dialog, _, _| {
                let buttons = options.iter().enumerate().map(|(index, option)| {
                    let (responder, option) = (responder.clone(), option.clone());
                    Button::new(("ext-option", index))
                        .outline()
                        .w_full()
                        .label(option.clone())
                        .on_click(move |_, window, cx| {
                            responder.send(&ExtensionUiAnswer::Value(option.clone()), cx);
                            window.close_dialog(cx);
                        })
                });
                let on_close = responder.clone();
                dialog
                    .title(title.clone())
                    .child(v_flex().gap_2().children(buttons))
                    .on_close(move |_, _, cx| on_close.send(&ExtensionUiAnswer::Cancelled, cx))
            });
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
            open_text_dialog(title, TextField::Line(input), responder, window, cx);
        }
        ExtensionUiMethod::Editor { title, prefill, .. } => {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(6, 20)
                    .default_value(prefill.clone().unwrap_or_default())
            });
            open_text_dialog(title, TextField::Multi(input), responder, window, cx);
        }
        _ => {}
    }
}

#[derive(Clone)]
enum TextField {
    Line(Entity<InputState>),
    Multi(Entity<TextareaState>),
}

impl TextField {
    fn value(&self, cx: &App) -> String {
        match self {
            Self::Line(input) => input.read(cx).value().to_string(),
            Self::Multi(input) => input.read(cx).value().to_string(),
        }
    }
}

fn open_text_dialog(
    title: String,
    field: TextField,
    responder: Responder,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, _| {
        let (submit, cancel, on_close) = (responder.clone(), responder.clone(), responder.clone());
        let field_for_submit = field.clone();
        let body: AnyElement = match &field {
            TextField::Line(input) => Input::new(input).into_any_element(),
            TextField::Multi(input) => Textarea::new(input).into_any_element(),
        };
        dialog
            .title(title.clone())
            .w(px(520.))
            .child(body)
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
                            .on_click(move |_, window, cx| {
                                let value = field_for_submit.value(cx);
                                submit.send(&ExtensionUiAnswer::Value(value), cx);
                                window.close_dialog(cx);
                            }),
                    ),
            )
            .on_close(move |_, _, cx| on_close.send(&ExtensionUiAnswer::Cancelled, cx))
    });
}
