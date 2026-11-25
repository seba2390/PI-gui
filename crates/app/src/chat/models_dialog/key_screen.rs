//! A provider pi knows: its API key.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::providers::BuiltIn;

use super::probe::{self, KeyCheck};
use super::{ModelsDialog, ModelsDialogEvent, Screen, bottom, count, label};
use crate::ui::QuietButton as _;

pub(super) struct KeyScreen {
    provider: &'static BuiltIn,
    key: Entity<InputState>,
    /// There is a key in the file.
    saved: bool,
    checking: bool,
    /// A key that could not be checked, and is saved the next time.
    unchecked: Option<String>,
    pub(super) error: Option<SharedString>,
    _enter: Subscription,
}

impl KeyScreen {
    pub(super) fn new(
        provider: &'static BuiltIn,
        key: Option<String>,
        window: &mut Window,
        cx: &mut Context<ModelsDialog>,
    ) -> Self {
        let saved = key.is_some();
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste your API key")
                .default_value(key.unwrap_or_default())
        });
        key.update(cx, |key, cx| key.focus(window, cx));
        let enter = cx.subscribe(&key, |this, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => this.save_key(cx),
            InputEvent::Change => {
                if let Screen::Key(screen) = &mut this.screen {
                    screen.error = None;
                    screen.unchecked = None;
                    cx.notify();
                }
            }
            _ => {}
        });
        Self {
            provider,
            key,
            saved,
            checking: false,
            unchecked: None,
            error: None,
            _enter: enter,
        }
    }
}

impl ModelsDialog {
    /// Asks the provider whether it takes the key, and saves it.
    fn save_key(&mut self, cx: &mut Context<Self>) {
        let (saving, Some(place)) = (self.saving, self.place.clone()) else {
            return;
        };
        let Screen::Key(screen) = &mut self.screen else {
            return;
        };
        let key = screen.key.read(cx).value().trim().to_owned();
        if saving || screen.checking {
            return;
        }
        if key.is_empty() {
            screen.error = Some("Enter the key.".into());
            cx.notify();
            return;
        }
        if screen.unchecked.as_ref() == Some(&key) {
            self.write_key(&key, cx);
            return;
        }
        screen.checking = true;
        screen.error = None;
        cx.notify();
        let provider = screen.provider;
        self.asked += 1;
        let asked = self.asked;
        self.task = Some(cx.spawn(async move |this, cx| {
            let (checked, key) = cx
                .background_executor()
                .spawn(async move { (probe::check_key(&place.pi, provider, &key), key) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.asked != asked {
                    return;
                }
                let Screen::Key(screen) = &mut this.screen else {
                    return;
                };
                screen.checking = false;
                match checked {
                    KeyCheck::Accepted => this.write_key(&key, cx),
                    KeyCheck::Refused(error) => screen.error = Some(error.into()),
                    KeyCheck::Unknown(error) => {
                        screen.error = Some(error.into());
                        screen.unchecked = Some(key);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Saves the key, or removes it when it is empty.
    fn write_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let (Some(Ok(file)), Screen::Key(screen)) = (&self.loaded, &self.screen) else {
            return;
        };
        let provider = screen.provider;
        let mut file = file.clone();
        match file.set_key(provider.id, key) {
            Ok(()) => {
                let note = if key.is_empty() {
                    format!("Removed the key of {}.", provider.label)
                } else {
                    format!(
                        "{} is connected. Its models are in the model menu.",
                        provider.label
                    )
                };
                self.save(file.to_text(), None, Some(note.into()), cx);
            }
            Err(error) => {
                self.set_error(Some(error.to_string().into()));
                cx.notify();
            }
        }
    }

    pub(super) fn render_key(&self, screen: &KeyScreen, cx: &mut Context<Self>) -> AnyElement {
        let provider = screen.provider;
        let models = self.available.get(provider.id).copied().unwrap_or_default();
        let about = match (models, screen.saved) {
            (0, _) => format!(
                "With a key, all models of {} that pi knows are in the model menu.",
                provider.label
            ),
            (models, true) => format!(
                "pi offers {} of {}.",
                count(models, "model"),
                provider.label
            ),
            (models, false) => format!(
                "pi already offers {} of {}, with a sign-in or a key it found elsewhere. You don't \
                 need to add one.",
                count(models, "model"),
                provider.label
            ),
        };
        let links = h_flex()
            .gap_2()
            .when_some(provider.keys_url, |this, url| {
                this.child(
                    Button::new("get-key")
                        .quiet(cx)
                        .small()
                        .icon(IconName::ExternalLink)
                        .label("Get a key")
                        .tooltip(url)
                        .on_click(move |_, _, cx| cx.open_url(url)),
                )
            })
            .when_some(
                provider.sign_in.filter(|_| self.can_sign_in),
                |this, with| {
                    this.child(
                        Button::new("sign-in")
                            .quiet(cx)
                            .small()
                            .icon(IconName::CircleUser)
                            .label(format!("Sign in with {with}"))
                            .tooltip("Opens pi's /login in the terminal")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ModelsDialogEvent::SignIn(provider.id.to_owned()));
                            })),
                    )
                },
            );
        let action = if screen.checking {
            "Checking…"
        } else if self.saving {
            "Saving…"
        } else if screen.unchecked.is_some() {
            "Save anyway"
        } else if screen.saved {
            "Save"
        } else {
            "Connect"
        };
        let busy = screen.checking || self.saving;
        v_flex()
            .gap_3()
            .child(Self::render_top(provider.label, None, cx))
            .child(label(about, cx))
            .child(
                v_flex()
                    .gap_1()
                    .child(label("API key", cx))
                    .child(Input::new(&screen.key).mask_toggle().aria_label("API key")),
            )
            .child(links)
            .child(label(
                format!(
                    "Saved in models.json. “${}” reads the key from that environment variable \
                     instead, and “!command” runs a command that prints it.",
                    provider.env
                ),
                cx,
            ))
            .when(provider.sign_in.is_some() && !self.can_sign_in, |this| {
                this.child(label(
                    format!(
                        "To sign in with {} instead, use /login in a Code session.",
                        provider.sign_in.unwrap_or_default()
                    ),
                    cx,
                ))
            })
            .children(screen.error.clone().map(|error| {
                div()
                    .text_xs()
                    .text_color(crate::theme::palette(cx).error)
                    .child(error)
            }))
            .child(
                bottom(None, cx)
                    .when(screen.saved, |this| {
                        this.child(
                            div().debug_selector(|| "remove-key".into()).child(
                                Button::new("remove-key")
                                    .form_danger(cx)
                                    .label("Remove key")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.write_key("", cx))),
                            ),
                        )
                    })
                    .child(
                        div().debug_selector(|| "save-key".into()).child(
                            Button::new("save-key")
                                .primary()
                                .label(action)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.save_key(cx))),
                        ),
                    ),
            )
            .into_any_element()
    }
}
