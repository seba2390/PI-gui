//! The connect dialog: connects to a saved host, handles the first-connection trust prompt and
//! a missing pi, then lets the user pick a folder on the remote machine.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{DirListing, HostId};

use crate::hosts::{self, HostStatus, Hosts, HostsEvent, INSTALL_PI};
use crate::ui::{CopyButton, QuietButton as _};

#[derive(Debug, Clone)]
pub enum ConnectFlowEvent {
    /// Start a session on `host` in `folder` (an absolute path there).
    Open { host: HostId, folder: String },
}

enum Browse {
    Loading,
    Ready(DirListing),
    Failed(SharedString),
}

pub struct ConnectFlow {
    host: HostId,
    hosts: Entity<Hosts>,
    /// Set once connected: the folder browser.
    browse: Option<Browse>,
    _subscription: Subscription,
    _load: Task<()>,
}

impl EventEmitter<ConnectFlowEvent> for ConnectFlow {}

impl ConnectFlow {
    /// Connects to `host` (if needed) and continues to the folder browser.
    pub fn new(host: HostId, cx: &mut Context<Self>) -> Self {
        let hosts = hosts::hosts(cx);
        let subscription = cx.subscribe(&hosts, |this, _, event: &HostsEvent, cx| match event {
            HostsEvent::Changed => this.status_changed(cx),
        });
        let mut this = Self {
            host,
            hosts,
            browse: None,
            _subscription: subscription,
            _load: Task::ready(()),
        };
        if !this.status(cx).is_ready() {
            let id = this.host.clone();
            this.hosts
                .update(cx, |hosts, cx| hosts.connect(&id, false, cx));
        }
        this.status_changed(cx);
        this
    }

    fn status(&self, cx: &App) -> HostStatus {
        self.hosts.read(cx).status(&self.host)
    }

    fn status_changed(&mut self, cx: &mut Context<Self>) {
        if self.status(cx).is_ready() && self.browse.is_none() {
            let start = {
                let hosts = self.hosts.read(cx);
                hosts
                    .get(&self.host)
                    .and_then(|host| host.recent_folders.first().cloned())
                    .or_else(|| hosts.probe(&self.host).map(|probe| probe.home.clone()))
                    .unwrap_or_else(|| "~".to_owned())
            };
            self.load(start, cx);
        }
        cx.notify();
    }

    /// Lists `path` on the remote (in the background).
    fn load(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(ssh) = self.hosts.read(cx).ssh(&self.host) else {
            return;
        };
        if !matches!(self.browse, Some(Browse::Ready(_))) {
            self.browse = Some(Browse::Loading);
        }
        cx.notify();
        self._load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh.list_dirs(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.browse = Some(match result {
                    Ok(listing) => Browse::Ready(listing),
                    Err(error) => Browse::Failed(error.to_string().into()),
                });
                cx.notify();
            });
        });
    }

    fn retry(&mut self, trust: bool, cx: &mut Context<Self>) {
        let id = self.host.clone();
        self.hosts
            .update(cx, |hosts, cx| hosts.connect(&id, trust, cx));
    }

    fn open(&mut self, folder: String, cx: &mut Context<Self>) {
        cx.emit(ConnectFlowEvent::Open {
            host: self.host.clone(),
            folder,
        });
    }

    fn render_status(&self, status: &HostStatus, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let label = self
            .hosts
            .read(cx)
            .get(&self.host)
            .map_or_else(String::new, |host| host.label.clone());
        let message = |text: String| div().text_sm().text_color(theme.foreground).child(text);
        match status {
            HostStatus::Idle | HostStatus::Connecting | HostStatus::Connected => h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(super::status_dot(Some(&HostStatus::Connecting), cx))
                .child(format!("Connecting to {label}…"))
                .into_any_element(),
            HostStatus::NeedsTrust { fingerprint } => v_flex()
                .gap_2()
                .child(message(format!("First connection to {label}. Check that this is the host's key:")))
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(fingerprint.clone()),
                )
                .child(
                    h_flex().justify_end().gap_2().child(
                        Button::new("trust-host")
                            .primary()
                            .label("Trust and connect")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(true, cx))),
                    ),
                )
                .into_any_element(),
            HostStatus::MissingPi => v_flex()
                .gap_2()
                .child(message(format!(
                    "Connected to {label}, but pi is not installed there (or not on the login shell's PATH). Install it on the remote machine with:"
                )))
                .child(
                    h_flex()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(div().flex_1().font_family(theme.mono_font_family.clone()).text_xs().child(INSTALL_PI))
                        .child(CopyButton::new("copy-install", INSTALL_PI).tooltip("Copy command")),
                )
                .child(
                    h_flex().justify_end().gap_2().child(
                        Button::new("check-again")
                            .primary()
                            .label("Check again")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(false, cx))),
                    ),
                )
                .into_any_element(),
            HostStatus::Failed(error) => v_flex()
                .gap_2()
                .child(message(format!("Could not connect to {label}.")))
                .child(div().text_xs().text_color(crate::theme::palette(cx).error).child(error.clone()))
                .child(
                    h_flex().justify_end().child(
                        Button::new("retry-connect")
                            .primary()
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(false, cx))),
                    ),
                )
                .into_any_element(),
        }
    }

    fn render_browser(&self, browse: &Browse, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (foreground, muted, hover) = (
            theme.foreground,
            theme.muted_foreground,
            theme.secondary_hover,
        );
        let (muted_bg, mono) = (theme.muted, theme.mono_font_family.clone());
        let recent = self
            .hosts
            .read(cx)
            .get(&self.host)
            .map(|host| host.recent_folders.clone())
            .unwrap_or_default();
        let listing = match browse {
            Browse::Loading => {
                return div()
                    .text_sm()
                    .text_color(muted)
                    .child("Loading folders…")
                    .into_any_element();
            }
            Browse::Failed(error) => {
                return div()
                    .text_sm()
                    .text_color(crate::theme::palette(cx).error)
                    .child(error.clone())
                    .into_any_element();
            }
            Browse::Ready(listing) => listing,
        };
        let current = listing.path.clone();
        let row = |id: SharedString,
                   icon: IconName,
                   label: String,
                   target: String,
                   cx: &mut Context<Self>| {
            h_flex()
                .id(ElementId::Name(id))
                .h(px(28.))
                .px_2()
                .gap_2()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .text_color(foreground)
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.load(target.clone(), cx)))
                .child(Icon::new(icon).small().text_color(muted))
                .child(div().truncate().child(label))
        };

        let mut list = v_flex().gap_0p5();
        if let Some(parent) = listing.parent.clone() {
            list = list.child(row(
                "dir-up".into(),
                IconName::ArrowUp,
                "..".to_owned(),
                parent,
                cx,
            ));
        }
        for dir in &listing.dirs {
            let target = if current.ends_with('/') {
                format!("{current}{dir}")
            } else {
                format!("{current}/{dir}")
            };
            list = list.child(row(
                format!("dir:{dir}").into(),
                IconName::Folder,
                dir.clone(),
                target,
                cx,
            ));
        }
        if listing.dirs.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child("No subfolders"),
            );
        }

        v_flex()
            .gap_2()
            .when(!recent.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted).child("Recent"))
                    .child(h_flex().flex_wrap().gap_1().children(
                        recent.into_iter().enumerate().map(|(ix, folder)| {
                            let open = folder.clone();
                            Button::new(("recent-folder", ix))
                                .quiet(cx)
                                .small()
                                .icon(IconName::Folder)
                                .label(
                                    folder
                                        .rsplit('/')
                                        .find(|part| !part.is_empty())
                                        .unwrap_or("/")
                                        .to_owned(),
                                )
                                .tooltip(folder)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.open(open.clone(), cx)),
                                )
                        }),
                    ))
            })
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(muted_bg)
                    .font_family(mono)
                    .text_xs()
                    .truncate()
                    .child(current.clone()),
            )
            .child(
                div()
                    .id("remote-dirs")
                    .h(px(300.))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("open-remote-folder")
                        .primary()
                        .label("Open this folder")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open(current.clone(), cx)),
                        ),
                ),
            )
            .into_any_element()
    }
}

impl Render for ConnectFlow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status(cx);
        let body = match (&self.browse, status.is_ready()) {
            (Some(browse), true) => {
                // Re-borrow: render_browser needs `cx` mutably for listeners.
                let browse = match browse {
                    Browse::Loading => Browse::Loading,
                    Browse::Failed(error) => Browse::Failed(error.clone()),
                    Browse::Ready(listing) => Browse::Ready(listing.clone()),
                };
                self.render_browser(&browse, cx)
            }
            _ => self.render_status(&status, cx),
        };
        v_flex().w_full().gap_3().child(body)
    }
}
