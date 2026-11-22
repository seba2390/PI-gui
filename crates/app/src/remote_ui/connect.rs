//! The connect dialog: connects to a saved host, handles the first-connection trust prompt, a
//! changed host key and the automatic pi install, offers to copy model settings, then lets
//! the user pick (type, browse or create) a folder on the remote machine.

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{DirListing, HostId};

use crate::hosts::{self, HostStatus, Hosts, HostsEvent, INSTALL_PI};
use crate::ui::{CopyButton, QuietButton as _};

#[derive(Debug, Clone)]
pub enum ConnectFlowEvent {
    /// Start a session on `host` in `folder` (an absolute path there).
    Open { host: HostId, folder: String },
    /// Open a terminal on `host` (e.g. to install pi by hand).
    OpenTerminal { host: HostId },
    /// Connected and pi is ready (only with `auto_open`: the caller opens its session now).
    Ready { host: HostId },
}

enum Browse {
    Loading,
    Ready(DirListing),
    Failed(SharedString),
}

/// Copying model settings to a host that has none.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelsCopy {
    Offer,
    Copying,
    Done,
    Failed(SharedString),
    Dismissed,
}

#[allow(clippy::struct_excessive_bools)] // independent UI flags
pub struct ConnectFlow {
    host: HostId,
    /// Emit [`ConnectFlowEvent::Ready`] once connected instead of browsing folders.
    auto_open: bool,
    ready_sent: bool,
    hosts: Entity<Hosts>,
    /// Set once connected: the folder browser.
    browse: Option<Browse>,
    /// The folder being browsed (also what "Open this folder" opens).
    path_input: Entity<InputState>,
    show_hidden: bool,
    /// The "New folder" name field while it is open.
    new_folder: Option<Entity<InputState>>,
    folder_error: Option<SharedString>,
    models: ModelsCopy,
    include_credentials: bool,
    _subscriptions: Vec<Subscription>,
    _load: Task<()>,
    _copy: Task<()>,
}

impl EventEmitter<ConnectFlowEvent> for ConnectFlow {}

impl ConnectFlow {
    /// Connects to `host` (if needed), installs pi when missing, and continues to the folder
    /// browser; with `auto_open`, (re)connects and reports [`ConnectFlowEvent::Ready`]
    /// instead, so the caller can open (or resume) a session there.
    pub fn new(host: HostId, auto_open: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hosts = hosts::hosts(cx);
        let path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("/path/on/the/host or ~/project"));
        let subscriptions = vec![
            cx.subscribe(&hosts, |this, _, event: &HostsEvent, cx| {
                if *event == HostsEvent::Changed {
                    this.status_changed(cx);
                }
            }),
            cx.subscribe(&path_input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let path = input.read(cx).value().trim().to_owned();
                    if !path.is_empty() {
                        this.load(path, cx);
                    }
                }
            }),
        ];
        let mut this = Self {
            host,
            auto_open,
            ready_sent: false,
            hosts,
            browse: None,
            path_input,
            show_hidden: false,
            new_folder: None,
            folder_error: None,
            models: ModelsCopy::Offer,
            include_credentials: false,
            _subscriptions: subscriptions,
            _load: Task::ready(()),
            _copy: Task::ready(()),
        };
        let id = this.host.clone();
        match this.status(cx) {
            HostStatus::Connecting | HostStatus::Installing { .. } => {}
            // Reconnecting checks the connection again even when it looked fine.
            HostStatus::Connected if auto_open => {
                this.hosts
                    .update(cx, |hosts, cx| hosts.connect(&id, false, cx));
            }
            HostStatus::Connected => {}
            // Connected quietly at startup but pi is not there yet: install it now.
            HostStatus::MissingPi { error: None } => {
                this.hosts.update(cx, |hosts, cx| hosts.install(&id, cx));
            }
            _ => this
                .hosts
                .update(cx, |hosts, cx| hosts.connect(&id, false, cx)),
        }
        this.status_changed(cx);
        this
    }

    fn status(&self, cx: &App) -> HostStatus {
        self.hosts.read(cx).status(&self.host)
    }

    fn label(&self, cx: &App) -> String {
        self.hosts.read(cx).label(&self.host)
    }

    fn status_changed(&mut self, cx: &mut Context<Self>) {
        if self.auto_open {
            if self.status(cx).is_ready() && !self.ready_sent {
                self.ready_sent = true;
                cx.emit(ConnectFlowEvent::Ready {
                    host: self.host.clone(),
                });
            }
            cx.notify();
            return;
        }
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
        self.folder_error = None;
        let show_hidden = self.show_hidden;
        cx.notify();
        self._load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh.list_dirs(&path, show_hidden) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(listing) => {
                        this.set_path_text(&listing.path, cx);
                        this.browse = Some(Browse::Ready(listing));
                    }
                    // A bad typed path keeps the current listing and says why.
                    Err(error) if matches!(this.browse, Some(Browse::Ready(_))) => {
                        this.folder_error = Some(error.to_string().into());
                    }
                    Err(error) => this.browse = Some(Browse::Failed(error.to_string().into())),
                }
                cx.notify();
            });
        });
    }

    fn set_path_text(&mut self, path: &str, cx: &mut Context<Self>) {
        let input = self.path_input.clone();
        let path = path.to_owned();
        // Setting an input's value needs a window; do it on the next frame.
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| {
                    input.update(cx, |input, cx| input.set_value(path, window, cx));
                });
            }
        });
    }

    fn current_path(&self) -> Option<String> {
        match &self.browse {
            Some(Browse::Ready(listing)) => Some(listing.path.clone()),
            _ => None,
        }
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        if let Some(path) = self.current_path() {
            self.load(path, cx);
        }
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        self._subscriptions
            .push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.create_folder(cx);
                }
            }));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.new_folder = Some(input);
        cx.notify();
    }

    fn create_folder(&mut self, cx: &mut Context<Self>) {
        let (Some(input), Some(parent)) = (self.new_folder.clone(), self.current_path()) else {
            return;
        };
        let name = input.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let Some(ssh) = self.hosts.read(cx).ssh(&self.host) else {
            return;
        };
        self._load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ssh.make_dir(&parent, &name) })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.new_folder = None;
                    this.load(path, cx);
                }
                Err(error) => {
                    this.folder_error = Some(error.to_string().into());
                    cx.notify();
                }
            });
        });
    }

    fn retry(&mut self, trust: bool, cx: &mut Context<Self>) {
        let id = self.host.clone();
        self.hosts
            .update(cx, |hosts, cx| hosts.connect(&id, trust, cx));
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let id = self.host.clone();
        self.hosts.update(cx, |hosts, cx| hosts.install(&id, cx));
    }

    fn replace_key(&mut self, cx: &mut Context<Self>) {
        let id = self.host.clone();
        self.hosts
            .update(cx, |hosts, cx| hosts.replace_host_key(&id, cx));
    }

    fn copy_models(&mut self, cx: &mut Context<Self>) {
        self.models = ModelsCopy::Copying;
        let id = self.host.clone();
        let include = self.include_credentials;
        let task = self
            .hosts
            .update(cx, |hosts, cx| hosts.copy_model_settings(&id, include, cx));
        self._copy = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.models = match result {
                    Ok(()) => ModelsCopy::Done,
                    Err(error) => ModelsCopy::Failed(error.into()),
                };
                cx.notify();
            });
        });
        cx.notify();
    }

    fn open(&mut self, folder: String, cx: &mut Context<Self>) {
        cx.emit(ConnectFlowEvent::Open {
            host: self.host.clone(),
            folder,
        });
    }

    fn render_status(&self, status: &HostStatus, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let (muted_bg, mono) = (theme.muted, theme.mono_font_family.clone());
        let error_color = crate::theme::palette(cx).error;
        let label = self.label(cx);
        let message = |text: String| div().text_sm().text_color(foreground).child(text);
        let progress = |text: String, cx: &App| {
            h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(super::status_dot(Some(&HostStatus::Connecting), cx))
                .child(text)
        };
        match status {
            HostStatus::Idle | HostStatus::Connecting | HostStatus::Connected => {
                progress(format!("Connecting to {label}…"), cx).into_any_element()
            }
            HostStatus::Installing { .. } | HostStatus::MissingPi { error: None } => {
                let step = match status {
                    HostStatus::Installing { step } => step.to_string(),
                    _ => "Preparing".to_owned(),
                };
                v_flex()
                    .gap_2()
                    .child(progress(format!("Setting up pi on {label}: {step}…"), cx))
                    .child(div().text_xs().text_color(muted).child(format!(
                        "pi isn't installed on {label} yet, so it is being installed into \
                         ~/.pi-gui/server there (no admin rights needed; Node.js is downloaded \
                         too if the host has no recent one). This happens only once."
                    )))
                    .into_any_element()
            }
            HostStatus::NeedsTrust { fingerprint } => v_flex()
                .gap_2()
                .child(message(format!(
                    "First connection to {label}. Check that this is the host's key:"
                )))
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(muted_bg)
                        .font_family(mono)
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
            HostStatus::KeyChanged => v_flex()
                .gap_2()
                .child(message(format!(
                    "{label} presented a different host key than the one recorded when you \
                     last connected."
                )))
                .child(div().text_xs().text_color(muted).child(
                    "This is expected when the server was reinstalled, its keys were \
                     regenerated, or its address now points to another machine. Otherwise \
                     someone may be intercepting the connection: if you are not sure, ask \
                     whoever runs the server before continuing.",
                ))
                .child(div().text_xs().text_color(muted).child(
                    "Continuing removes the old key from ~/.ssh/known_hosts (ssh-keygen -R) \
                     and shows you the new key's fingerprint to confirm.",
                ))
                .child(
                    h_flex().justify_end().gap_2().child(
                        Button::new("replace-key")
                            .with_variant(ButtonVariant::Danger)
                            .label("Remove the old key and continue")
                            .on_click(cx.listener(|this, _, _, cx| this.replace_key(cx))),
                    ),
                )
                .into_any_element(),
            HostStatus::MissingPi { error: Some(error) } => v_flex()
                .gap_2()
                .child(message(format!("pi could not be set up on {label}.")))
                .child(div().text_xs().text_color(error_color).child(error.clone()))
                .child(div().text_xs().text_color(muted).child(
                    "You can install it yourself in a terminal on the host, then check again:",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .bg(muted_bg)
                        .child(div().flex_1().font_family(mono).text_xs().child(INSTALL_PI))
                        .child(CopyButton::new("copy-install", INSTALL_PI).tooltip("Copy command")),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("open-terminal")
                                .quiet(cx)
                                .icon(IconName::SquareTerminal)
                                .label("Open a terminal there")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.emit(ConnectFlowEvent::OpenTerminal {
                                        host: this.host.clone(),
                                    });
                                })),
                        )
                        .child(
                            Button::new("install-again")
                                .quiet(cx)
                                .label("Try installing again")
                                .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                        )
                        .child(
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
                .child(div().text_xs().text_color(error_color).child(error.clone()))
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

    /// "This host has no model settings" with a button to copy this computer's.
    fn render_models(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let has_models = self
            .hosts
            .read(cx)
            .probe(&self.host)
            .is_none_or(|probe| probe.has_models);
        let local_models =
            pi_sessions::agent_dir().is_some_and(|agent| agent.join("models.json").is_file());
        if (has_models && self.models != ModelsCopy::Done)
            || !local_models
            || self.models == ModelsCopy::Dismissed
        {
            return None;
        }
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let colors = crate::theme::palette(cx);
        let label = self.label(cx);
        let body = match &self.models {
            ModelsCopy::Done => div()
                .text_xs()
                .text_color(colors.success)
                .child(format!("Copied your model settings to {label}."))
                .into_any_element(),
            state => {
                let include = self.include_credentials;
                v_flex()
                    .gap_1p5()
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{label} has no pi model settings (models.json). Copy yours? An \
                         existing file is never overwritten without a backup."
                    )))
                    .child(
                        Checkbox::new("copy-credentials")
                            .small()
                            .checked(include)
                            .label("Also copy my credentials (auth.json)")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.include_credentials = *checked;
                                cx.notify();
                            })),
                    )
                    .when_some(
                        match state {
                            ModelsCopy::Failed(error) => Some(error.clone()),
                            _ => None,
                        },
                        |this, error| {
                            this.child(div().text_xs().text_color(colors.error).child(error))
                        },
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("skip-models")
                                    .quiet(cx)
                                    .small()
                                    .label("Not now")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.models = ModelsCopy::Dismissed;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("copy-models")
                                    .small()
                                    .label(if *state == ModelsCopy::Copying {
                                        "Copying…"
                                    } else {
                                        "Copy model settings"
                                    })
                                    .disabled(*state == ModelsCopy::Copying)
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_models(cx))),
                            ),
                    )
                    .into_any_element()
            }
        };
        Some(
            div()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(body)
                .into_any_element(),
        )
    }

    fn render_browser(&self, browse: &Browse, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (foreground, muted, hover) = (
            theme.foreground,
            theme.muted_foreground,
            theme.secondary_hover,
        );
        let error_color = crate::theme::palette(cx).error;
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
                return v_flex()
                    .gap_2()
                    .child(div().text_sm().text_color(error_color).child(error.clone()))
                    .child(
                        h_flex().justify_end().child(
                            Button::new("browse-home")
                                .label("Go to the home folder")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.load("~".to_owned(), cx)),
                                ),
                        ),
                    )
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
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    // Double-click opens a folder right away.
                    if event.click_count() >= 2 {
                        this.open(target.clone(), cx);
                    } else {
                        this.load(target.clone(), cx);
                    }
                }))
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

        let show_hidden = self.show_hidden;
        v_flex()
            .gap_2()
            .children(self.render_models(cx))
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
                h_flex()
                    .gap_1()
                    .child(div().flex_1().child(Input::new(&self.path_input).small()))
                    .child(
                        Button::new("go-to-path")
                            .quiet(cx)
                            .small()
                            .icon(IconName::ArrowRight)
                            .accessibility_label("Go to this folder (Enter)")
                            .tooltip("Go to this folder (Enter)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let path = this.path_input.read(cx).value().trim().to_owned();
                                if !path.is_empty() {
                                    this.load(path, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("toggle-hidden")
                            .quiet(cx)
                            .small()
                            .icon(if show_hidden {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            })
                            .accessibility_label(if show_hidden {
                                "Hide hidden folders"
                            } else {
                                "Show hidden folders"
                            })
                            .tooltip(if show_hidden {
                                "Hide hidden folders"
                            } else {
                                "Show hidden folders"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx))),
                    )
                    .child(
                        Button::new("new-folder")
                            .quiet(cx)
                            .small()
                            .icon(IconName::Plus)
                            .accessibility_label("New folder")
                            .tooltip("New folder")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_new_folder(window, cx);
                            })),
                    ),
            )
            .when_some(self.new_folder.clone(), |this, input| {
                this.child(
                    h_flex()
                        .gap_1()
                        .child(div().flex_1().child(Input::new(&input).small()))
                        .child(
                            Button::new("create-folder")
                                .small()
                                .label("Create")
                                .on_click(cx.listener(|this, _, _, cx| this.create_folder(cx))),
                        )
                        .child(
                            Button::new("cancel-folder")
                                .quiet(cx)
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.new_folder = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.folder_error.clone(), |this, error| {
                this.child(div().text_xs().text_color(error_color).child(error))
            })
            .child(
                div()
                    .id("remote-dirs")
                    .h(px(280.))
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
