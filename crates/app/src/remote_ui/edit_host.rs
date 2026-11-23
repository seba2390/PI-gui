//! "Change" of a saved connection: the fields of "Add SSH connection", filled in, and the
//! command that starts pi. A host from `~/.ssh/config` shows the config's values; the config
//! itself is never written.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{HostId, SavedHost, Target};

use super::host_form::{ConfigEntry, HostFields, HostForm, field};
use crate::config::AppConfig;
use crate::hosts::hosts;

#[derive(Debug, Clone)]
pub enum EditHostEvent {
    Save(HostEdit),
}

/// New settings for a saved host.
#[derive(Debug, Clone)]
pub struct HostEdit {
    pub host: HostId,
    label: String,
    target: Target,
    pi_command: String,
}

impl HostEdit {
    /// Whether saving ends the host's connection (it connects again with the new settings).
    pub fn reconnects(&self, cx: &App) -> bool {
        hosts(cx)
            .read(cx)
            .get(&self.host)
            .is_some_and(|host| host.target != self.target)
    }

    pub fn apply(&self, cx: &mut App) {
        hosts(cx).update(cx, |hosts, cx| {
            hosts.rename(&self.host, self.label.clone(), cx);
            // First the target: a connected host checks a new command right away.
            hosts.set_target(&self.host, self.target.clone(), cx);
            hosts.set_pi_command(&self.host, self.pi_command.clone(), cx);
        });
    }
}

struct Form {
    fields: HostForm,
    pi_command: Entity<InputState>,
    _enter: Vec<Subscription>,
}

pub struct EditHost {
    host: SavedHost,
    /// The ssh config's entry this host uses, if any.
    config: Option<ConfigEntry>,
    /// What the unchanged fields say; saving them keeps the saved target as it is.
    initial: Option<Target>,
    /// Shown once the ssh config was read.
    form: Option<Form>,
    _load: Task<()>,
}

impl EventEmitter<EditHostEvent> for EditHost {}

impl EditHost {
    pub fn new(host: &HostId, window: &mut Window, cx: &mut App) -> Option<Entity<Self>> {
        let host = hosts(cx).read(cx).get(host)?.clone();
        let home = cx.global::<AppConfig>().home.clone();
        let target = host.target.clone();
        Some(cx.new(|cx: &mut Context<Self>| {
            let load = cx.spawn_in(window, async move |this, cx| {
                let config = cx
                    .background_executor()
                    .spawn(async move {
                        let alias = match target {
                            Target::Alias { alias } => alias,
                            // Changed here before: still the config's host with overrides.
                            Target::Custom { host, .. } => {
                                let known = remote::discover_hosts(&home?);
                                known.into_iter().find(|known| known.alias == host)?.alias
                            }
                        };
                        let values = remote::config_values(&alias)
                            .map_err(|error| log::warn!("reading ssh config of {alias}: {error}"))
                            .ok()?;
                        Some(ConfigEntry { alias, values })
                    })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| this.show(config, window, cx));
            });
            Self {
                host,
                config: None,
                initial: None,
                form: None,
                _load: load,
            }
        }))
    }

    fn show(&mut self, config: Option<ConfigEntry>, window: &mut Window, cx: &mut Context<Self>) {
        let fields = HostFields::of(&self.host.label, &self.host.target, config.as_ref());
        let home = cx.global::<AppConfig>().home.clone();
        self.initial = fields
            .target(config.as_ref(), home.as_deref())
            .ok()
            .map(|(_, target)| target);
        let fields = HostForm::new(&fields, config.as_ref(), window, cx);
        let command = self.host.pi_command.clone().unwrap_or_default();
        let pi_command = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("pi (installed automatically when missing)")
                .default_value(command)
        });
        let enter = fields
            .inputs()
            .into_iter()
            .chain([&pi_command])
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(cx);
                    }
                })
            })
            .collect();
        self.config = config;
        self.form = Some(Form {
            fields,
            pi_command,
            _enter: enter,
        });
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.form else { return };
        let checked = form
            .fields
            .target(self.config.as_ref(), cx)
            .and_then(|(label, target)| {
                let target = if self.initial.as_ref() == Some(&target) {
                    self.host.target.clone()
                } else {
                    target
                };
                let hosts = hosts(cx).read(cx);
                match hosts.other_with_target(&self.host.id, &target) {
                    Some(other) => Err(format!("“{}” already connects there.", other.label).into()),
                    None => Ok((label, target)),
                }
            });
        match checked {
            Err(error) => {
                form.fields.error = Some(error);
                cx.notify();
            }
            Ok((label, target)) => {
                let edit = HostEdit {
                    host: self.host.id.clone(),
                    label,
                    target,
                    pi_command: form.pi_command.read(cx).value().to_string(),
                };
                cx.emit(EditHostEvent::Save(edit));
            }
        }
    }
}

impl Render for EditHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(form) = &self.form else {
            return div()
                .text_sm()
                .text_color(muted)
                .child("Reading your SSH config…");
        };
        v_flex()
            .w_full()
            .gap_2()
            .when_some(self.config.as_ref(), |this, config| {
                this.child(div().text_xs().text_color(muted).child(format!(
                    "From “{}” in ~/.ssh/config. Changes are saved in Pistachio; the file is \
                     left as it is.",
                    config.alias
                )))
            })
            .child(form.fields.render(cx))
            .child(field("Command that starts pi", &form.pi_command, cx))
            .child(
                h_flex().justify_end().pt_1().child(
                    Button::new("save-host")
                        .primary()
                        .label("Save")
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                ),
            )
    }
}
