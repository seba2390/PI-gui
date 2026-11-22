//! The "Add SSH connection" dialog: hosts found in the user's OpenSSH config (indexed when the
//! dialog opens), or a custom connection entered by hand.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{HostId, Target};

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

struct CustomForm {
    name: Entity<InputState>,
    host: Entity<InputState>,
    user: Entity<InputState>,
    port: Entity<InputState>,
    identity: Entity<InputState>,
    jump: Entity<InputState>,
    error: Option<SharedString>,
}

pub struct AddHost {
    hosts: Entity<Hosts>,
    discovered: Option<Vec<Discovered>>,
    custom: Option<CustomForm>,
    _scan: Task<()>,
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
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        self.custom = Some(CustomForm {
            name: input("Name (e.g. Build server)", window, cx),
            host: input("Host name or IP address", window, cx),
            user: input("User (optional)", window, cx),
            port: input("Port (22)", window, cx),
            identity: input("Key file, e.g. ~/.ssh/id_ed25519 (optional)", window, cx),
            jump: input("Jump host, e.g. user@bastion (optional)", window, cx),
            error: None,
        });
        cx.notify();
    }

    fn add_custom(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.custom else { return };
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let optional = |text: String| (!text.is_empty()).then_some(text);
        let host = value(&form.host);
        if host.is_empty() {
            form.error = Some("Enter a host name or IP address.".into());
            cx.notify();
            return;
        }
        // ssh would read a leading `-` as one of its own options.
        let starts_with_dash = [&form.host, &form.user, &form.jump]
            .into_iter()
            .any(|input| value(input).starts_with('-'));
        if starts_with_dash || host.chars().any(char::is_whitespace) {
            form.error = Some(
                "Host, user and jump host cannot start with “-” (and the host has no spaces)."
                    .into(),
            );
            cx.notify();
            return;
        }
        let port = match optional(value(&form.port)).map(|port| port.parse::<u16>()) {
            None => None,
            Some(Ok(port)) => Some(port),
            Some(Err(_)) => {
                form.error = Some("The port must be a number between 1 and 65535.".into());
                cx.notify();
                return;
            }
        };
        let home = cx.global::<AppConfig>().home.clone();
        let identity_file =
            optional(value(&form.identity)).map(|path| expand_home(&path, home.as_deref()));
        let target = Target::Custom {
            host: host.clone(),
            user: optional(value(&form.user)),
            port,
            identity_file,
            proxy_jump: optional(value(&form.jump)),
        };
        let label = optional(value(&form.name)).unwrap_or(host);
        let id = self
            .hosts
            .update(cx, |hosts, cx| hosts.add(label, target, cx));
        cx.emit(AddHostEvent::Added(id));
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

    fn render_custom(&self, form: &CustomForm, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Input::new(input).small())
        };
        v_flex()
            .gap_2()
            .child(field("Name", &form.name))
            .child(field("Host", &form.host))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(field("User", &form.user)))
                    .child(div().w(px(90.)).child(field("Port", &form.port))),
            )
            .child(field("Key file", &form.identity))
            .child(field("Jump host", &form.jump))
            .when_some(form.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(crate::theme::palette(cx).error)
                        .child(error),
                )
            })
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

/// `~/x` → `<home>/x`, so key paths work regardless of how ssh is invoked.
fn expand_home(path: &str, home: Option<&std::path::Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

impl Render for AddHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let body = match &self.custom {
            Some(form) => {
                let form = CustomForm {
                    name: form.name.clone(),
                    host: form.host.clone(),
                    user: form.user.clone(),
                    port: form.port.clone(),
                    identity: form.identity.clone(),
                    jump: form.jump.clone(),
                    error: form.error.clone(),
                };
                self.render_custom(&form, cx)
            }
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
    use std::path::{Path, PathBuf};

    use super::expand_home;

    #[test]
    fn expands_home_in_key_paths() {
        assert_eq!(
            expand_home("~/.ssh/id", Some(Path::new("/home/u"))),
            PathBuf::from("/home/u/.ssh/id")
        );
        assert_eq!(
            expand_home("/abs/key", Some(Path::new("/home/u"))),
            PathBuf::from("/abs/key")
        );
        assert_eq!(expand_home("~/.ssh/id", None), PathBuf::from("~/.ssh/id"));
    }
}
