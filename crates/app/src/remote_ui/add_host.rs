//! The "Add SSH connection" dialog: hosts found in the user's OpenSSH config (indexed when the
//! dialog opens), or a custom connection entered by hand.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{HostId, Target};

use super::host_form::{HostFields, HostForm};
use crate::config::AppConfig;
use crate::hosts::{self, Hosts};
use crate::ui::QuietButton as _;

#[derive(Debug, Clone)]
pub enum AddHostEvent {
    /// A host was saved; connect to it next.
    Added(HostId),
}

/// A host from `~/.ssh/config`, with what it resolves to once `ssh -G` answered.
struct Discovered {
    alias: String,
    resolved: Option<String>,
}

pub struct AddHost {
    hosts: Entity<Hosts>,
    discovered: Option<Vec<Discovered>>,
    custom: Option<HostForm>,
    _scan: Task<()>,
    _enter: Vec<Subscription>,
}

impl EventEmitter<AddHostEvent> for AddHost {}

impl AddHost {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let home = cx.global::<AppConfig>().home.clone();
        // Index the SSH config in the background: reading files and `ssh -G` per host.
        let scan = cx.spawn(async move |this, cx| {
            let discovered = cx
                .background_executor()
                .spawn(async move {
                    let Some(home) = home else { return Vec::new() };
                    remote::discover_hosts(&home)
                        .into_iter()
                        .map(|host| {
                            let target = Target::Alias {
                                alias: host.alias.clone(),
                            };
                            let resolved = remote::resolve(&target).ok().map(|resolved| {
                                let user = resolved
                                    .user
                                    .map(|user| format!("{user}@"))
                                    .unwrap_or_default();
                                let port = if resolved.port == 22 {
                                    String::new()
                                } else {
                                    format!(":{}", resolved.port)
                                };
                                format!("{user}{}{port}", resolved.hostname)
                            });
                            Discovered {
                                alias: host.alias,
                                resolved,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.discovered = Some(discovered);
                cx.notify();
            });
        });
        Self {
            hosts: hosts::hosts(cx),
            discovered: None,
            custom: None,
            _scan: scan,
            _enter: Vec::new(),
        }
    }

    fn add_alias(&mut self, alias: String, cx: &mut Context<Self>) {
        let id = self.hosts.update(cx, |hosts, cx| {
            hosts.add(alias.clone(), Target::Alias { alias }, cx)
        });
        cx.emit(AddHostEvent::Added(id));
    }

    fn remove_alias(&mut self, alias: &str, cx: &mut Context<Self>) {
        let id = self
            .hosts
            .read(cx)
            .hosts()
            .iter()
            .find_map(|host| match &host.target {
                Target::Alias { alias: saved } if saved == alias => Some(host.id.clone()),
                _ => None,
            });
        if let Some(id) = id {
            self.hosts.update(cx, |hosts, cx| hosts.remove(&id, cx));
        }
        cx.notify();
    }

    fn show_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = HostForm::new(&HostFields::default(), None, window, cx);
        self._enter = form
            .inputs()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_custom(cx);
                    }
                })
            })
            .into();
        form.focus(window, cx);
        self.custom = Some(form);
        cx.notify();
    }

    fn add_custom(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.custom else { return };
        match form.target(None, cx) {
            Err(error) => {
                form.error = Some(error);
                cx.notify();
            }
            Ok((label, target)) => {
                let id = self
                    .hosts
                    .update(cx, |hosts, cx| hosts.add(label, target, cx));
                cx.emit(AddHostEvent::Added(id));
            }
        }
    }

    fn render_discovered(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (foreground, muted, hover) = (
            theme.foreground,
            theme.muted_foreground,
            theme.secondary_hover,
        );
        let saved: Vec<String> = self
            .hosts
            .read(cx)
            .hosts()
            .iter()
            .filter_map(|host| match &host.target {
                Target::Alias { alias } => Some(alias.clone()),
                Target::Custom { .. } => None,
            })
            .collect();
        let Some(discovered) = &self.discovered else {
            return div()
                .text_sm()
                .text_color(muted)
                .child("Reading your SSH config…")
                .into_any_element();
        };
        if discovered.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child("No hosts found in ~/.ssh/config. Add a custom connection instead.")
                .into_any_element();
        }
        let rows = discovered.iter().map(|host| {
            let added = saved.contains(&host.alias);
            let alias = host.alias.clone();
            h_flex()
                .id(ElementId::Name(format!("ssh-host:{}", host.alias).into()))
                .h(px(34.))
                .px_2()
                .gap_2()
                .rounded_md()
                .when(!added, |this| {
                    this.cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.add_alias(alias.clone(), cx)),
                        )
                })
                .child(Icon::new(IconName::Globe).small().text_color(muted))
                .child(
                    div()
                        .text_sm()
                        .text_color(foreground)
                        .child(host.alias.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .children(host.resolved.clone()),
                )
                .when(added, |this| {
                    let alias = host.alias.clone();
                    this.child(div().text_xs().text_color(muted).child("Added"))
                        .child(
                            Button::new(ElementId::Name(
                                format!("remove-host:{}", host.alias).into(),
                            ))
                            .quiet(cx)
                            .xsmall()
                            .icon(IconName::Delete)
                            .accessibility_label("Remove this connection")
                            .tooltip("Remove this connection")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove_alias(&alias, cx)),
                            ),
                        )
                })
        });
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child("From ~/.ssh/config"),
            )
            .child(
                div()
                    .id("ssh-hosts")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .child(v_flex().gap_0p5().children(rows)),
            )
            .into_any_element()
    }

    fn render_custom(form: &HostForm, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(form.render(cx))
            .child(
                h_flex().justify_end().child(
                    Button::new("add-custom-host")
                        .primary()
                        .label("Add connection")
                        .on_click(cx.listener(|this, _, _, cx| this.add_custom(cx))),
                ),
            )
            .into_any_element()
    }
}

impl Render for AddHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let body = match &self.custom {
            Some(form) => Self::render_custom(form, cx),
            None => v_flex()
                .gap_3()
                .child(self.render_discovered(cx))
                .child(
                    Button::new("custom-host")
                        .quiet(cx)
                        .icon(IconName::Plus)
                        .label("Custom connection…")
                        .on_click(cx.listener(|this, _, window, cx| this.show_custom(window, cx))),
                )
                .into_any_element(),
        };
        v_flex().w_full().gap_3().child(body)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::component::WindowExt as _;
    use gpui_kit::{AppContext as _, ParentElement as _, TestAppContext};

    use super::{AddHost, AddHostEvent};
    use crate::config::AppConfig;
    use crate::hosts::{Hosts, HostsHandle};
    use crate::ui::dialog::open_form_dialog;
    use crate::ui::dialog::testing::{click, draw, is_open, press, window};

    /// Enter in a field adds the connection; what is wrong shows in the dialog, which stays.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn enter_adds_a_custom_host(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests("pi".into()));
            let hosts = cx.new(|_| Hosts::for_tests(Vec::new()));
            cx.set_global(HostsHandle(hosts));
        });
        let cx = window(cx);
        let added = Rc::new(Cell::new(0));
        let add = cx.update(|window, cx| {
            let add = cx.new(AddHost::new);
            let added = added.clone();
            window
                .subscribe(&add, cx, move |_, _: &AddHostEvent, window, cx| {
                    added.set(added.get() + 1);
                    window.close_dialog(cx);
                })
                .detach();
            let view = add.clone();
            open_form_dialog(None, window, cx, move |dialog, _, _| {
                dialog.child(view.clone())
            });
            add
        });
        draw(cx);
        press("enter", cx);
        assert!(is_open(cx));
        click("custom-host", cx);
        cx.simulate_input("Build server");
        press("enter", cx);
        assert!(is_open(cx));
        assert_eq!(added.get(), 0);
        cx.update(|_, cx| {
            assert!(add.read(cx).custom.as_ref().unwrap().error.is_some());
        });
        press("tab", cx);
        cx.simulate_input("192.0.2.7");
        press("enter", cx);
        assert_eq!(added.get(), 1);
        assert!(!is_open(cx));
    }
}
