//! "Manage SSH connections": rename saved hosts, change the command that starts pi, and remove
//! hosts.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::HostId;

use crate::hosts::{self, Hosts, HostsEvent};
use crate::ui::QuietButton as _;

#[derive(Debug, Clone)]
pub enum ManageHostsEvent {
    /// Open the "Add SSH connection" dialog.
    Add,
}

struct HostForm {
    name: Entity<InputState>,
    pi_command: Entity<InputState>,
    confirm_remove: bool,
    note: Option<(bool, SharedString)>,
}

pub struct ManageHosts {
    hosts: Entity<Hosts>,
    forms: HashMap<HostId, HostForm>,
    _subscription: Subscription,
}

impl EventEmitter<ManageHostsEvent> for ManageHosts {}

impl ManageHosts {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hosts = hosts::hosts(cx);
        let subscription = cx.subscribe_in(&hosts, window, |this, _, event, window, cx| {
            if *event == HostsEvent::Changed {
                this.sync_forms(window, cx);
                cx.notify();
            }
        });
        let mut this = Self {
            hosts,
            forms: HashMap::new(),
            _subscription: subscription,
        };
        this.sync_forms(window, cx);
        this
    }

    /// One form per saved host (new hosts get one, removed ones lose theirs).
    fn sync_forms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved: Vec<(HostId, String, String)> = self
            .hosts
            .read(cx)
            .hosts()
            .iter()
            .map(|host| {
                (
                    host.id.clone(),
                    host.label.clone(),
                    host.pi_command.clone().unwrap_or_default(),
                )
            })
            .collect();
        self.forms
            .retain(|id, _| saved.iter().any(|(saved, _, _)| saved == id));
        for (id, label, pi_command) in saved {
            self.forms.entry(id).or_insert_with(|| HostForm {
                name: cx.new(|cx| InputState::new(window, cx).default_value(label)),
                pi_command: cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("pi (installed automatically when missing)")
                        .default_value(pi_command)
                }),
                confirm_remove: false,
                note: None,
            });
        }
    }

    fn save(&mut self, id: &HostId, cx: &mut Context<Self>) {
        let Some(form) = self.forms.get_mut(id) else {
            return;
        };
        let name = form.name.read(cx).value().to_string();
        let command = form.pi_command.read(cx).value().to_string();
        form.note = Some((true, "Saved.".into()));
        self.hosts.update(cx, |hosts, cx| {
            hosts.rename(id, name, cx);
            hosts.set_pi_command(id, command, cx);
        });
        cx.notify();
    }

    fn remove(&mut self, id: &HostId, cx: &mut Context<Self>) {
        self.hosts.update(cx, |hosts, cx| hosts.remove(id, cx));
        self.forms.remove(id);
        cx.notify();
    }

    fn render_host(&self, id: &HostId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.forms.get(id)?;
        let hosts = self.hosts.read(cx);
        let host = hosts.get(id)?;
        let status = hosts.status(id);
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = crate::theme::palette(cx);
        let key = id.0.clone();
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Input::new(input).small())
        };
        let (save_id, remove_id, confirm_id) = (id.clone(), id.clone(), id.clone());
        Some(
            v_flex()
                .gap_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .gap_2()
                        .child(super::status_dot(Some(&status), cx))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(host.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(format!(
                                    "{} · {}",
                                    host.target.display(),
                                    super::status_text(&status)
                                )),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(field("Name", &form.name))
                        .child(field("Command that starts pi", &form.pi_command)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .when_some(form.note.clone(), |this, (ok, note)| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(if ok { muted } else { colors.error })
                                    .child(note),
                            )
                        })
                        .child(div().flex_1())
                        .map(|this| {
                            if form.confirm_remove {
                                this.child(div().text_xs().text_color(muted).child(
                                    "Remove this connection? Sessions on the host are kept.",
                                ))
                                .child(
                                    Button::new(ElementId::Name(
                                        format!("remove-yes:{key}").into(),
                                    ))
                                    .small()
                                    .with_variant(ButtonVariant::Danger)
                                    .label("Remove")
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.remove(&remove_id, cx);
                                        },
                                    )),
                                )
                            } else {
                                this.child(
                                    Button::new(ElementId::Name(format!("remove:{key}").into()))
                                        .quiet(cx)
                                        .small()
                                        .icon(IconName::Delete)
                                        .label("Remove")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(form) = this.forms.get_mut(&confirm_id) {
                                                form.confirm_remove = true;
                                            }
                                            cx.notify();
                                        })),
                                )
                            }
                        })
                        .child(
                            Button::new(ElementId::Name(format!("save:{key}").into()))
                                .small()
                                .primary()
                                .label("Save")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.save(&save_id, cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for ManageHosts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ids: Vec<HostId> = self
            .hosts
            .read(cx)
            .hosts()
            .iter()
            .map(|host| host.id.clone())
            .collect();
        let muted = cx.theme().muted_foreground;
        let cards: Vec<AnyElement> = ids
            .iter()
            .filter_map(|id| self.render_host(id, cx))
            .collect();
        v_flex()
            .w_full()
            .gap_3()
            .when(cards.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("No SSH connections yet."),
                )
            })
            .child(
                div()
                    .id("manage-hosts")
                    .max_h(px(460.))
                    .overflow_y_scroll()
                    .child(v_flex().gap_2().children(cards)),
            )
            .child(
                h_flex().child(
                    Button::new("manage-add-host")
                        .quiet(cx)
                        .icon(IconName::Plus)
                        .label("Add SSH connection…")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ManageHostsEvent::Add))),
                ),
            )
    }
}
