//! The Settings page, shown in the chat column: General, pi & models, Web search and SSH
//! connections. Every change is saved right away.

use std::path::{Path, PathBuf};

use deepsearch::config::{BackendKind, Config as WebConfig, Mode as WebMode};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::setting::{
    NumberFieldOptions, RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem,
    SettingPage, Settings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::settings::{StartupDefaults, THINKING_LEVELS};
use remote::HostId;

use super::{AppSettings, StartMode, TextSize, ThemeChoice};
use crate::code_colors::CodeColors;
use crate::config::AppConfig;
use crate::ui::AppIcon;

/// What the window does for the page (dialogs and connections live there).
#[derive(Debug, Clone)]
pub enum SettingsEvent {
    AddModel,
    EditModels,
    AddHost,
    ManageHosts,
    ConnectHost(HostId),
    CheckForUpdates,
}

/// Web search's `config.json`, loaded when the page opens.
struct WebSearchState {
    dir: Option<PathBuf>,
    config: WebConfig,
    error: Option<String>,
}

impl Global for WebSearchState {}

/// The startup model and thinking level in pi's `settings.json` on this computer.
struct PiStartupState {
    path: Option<PathBuf>,
    defaults: StartupDefaults,
    error: Option<String>,
}

impl Global for PiStartupState {}

pub struct SettingsView {
    brave_key: Entity<InputState>,
    tavily_key: Entity<InputState>,
    /// `(provider/id, label)` of the models the visible session knows, for the startup model.
    models: Vec<(SharedString, SharedString)>,
    /// `pi --version` of the configured `pi`, once known.
    pi_version: Option<String>,
    /// The page shown first (scripted runs open Speech to text directly).
    start_page: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    /// `models` are the visible session's models (`provider`, `id`, `name`).
    pub fn new(
        models: Vec<(String, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        load_web_search(cx);
        load_pi_startup(cx);
        let (brave, tavily) = {
            let config = &cx.global::<WebSearchState>().config;
            (
                config.brave.api_key.clone().unwrap_or_default(),
                config.tavily.api_key.clone().unwrap_or_default(),
            )
        };
        let key_input = |value: String, placeholder: &str, window: &mut Window, cx: &mut App| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let brave_key = key_input(brave, "From BRAVE_API_KEY if empty", window, cx);
        let tavily_key = key_input(tavily, "From TAVILY_API_KEY if empty", window, cx);
        let subscriptions = vec![
            // Speech to text: download progress and installed models.
            cx.observe(&crate::stt::models::models(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&brave_key, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let key = input.read(cx).value().trim().to_owned();
                    update_web(cx, |config| {
                        config.brave.api_key = Some(key).filter(|k| !k.is_empty());
                    });
                }
            }),
            cx.subscribe(&tavily_key, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let key = input.read(cx).value().trim().to_owned();
                    update_web(cx, |config| {
                        config.tavily.api_key = Some(key).filter(|k| !k.is_empty());
                    });
                }
            }),
            cx.observe(&crate::hosts::hosts(cx), |_, _, cx| cx.notify()),
        ];
        let models = models
            .into_iter()
            .map(|(provider, id, name)| {
                let value: SharedString = format!("{provider}/{id}").into();
                let label: SharedString = if name.is_empty() || name == id {
                    value.clone()
                } else {
                    format!("{name} ({provider})").into()
                };
                (value, label)
            })
            .collect();
        let mut this = Self {
            brave_key,
            tavily_key,
            models,
            pi_version: None,
            start_page: 0,
            _subscriptions: subscriptions,
        };
        this.probe_pi_version(cx);
        this
    }

    /// Asks the configured `pi` for its version in the background.
    fn probe_pi_version(&mut self, cx: &mut Context<Self>) {
        let (program, env) = {
            let config = cx.global::<AppConfig>();
            (config.pi_program.clone(), config.child_env.clone())
        };
        cx.spawn(async move |this, cx| {
            let version = cx
                .background_executor()
                .spawn(async move {
                    std::process::Command::new(&program)
                        .arg("--version")
                        .envs(env)
                        .stdin(std::process::Stdio::null())
                        .output()
                        .ok()
                        .filter(|output| output.status.success())
                        .and_then(|output| {
                            String::from_utf8_lossy(&output.stdout)
                                .lines()
                                .last()
                                .map(|line| line.trim().to_owned())
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pi_version = Some(version.unwrap_or_else(|| "not found".to_owned()));
                cx.notify();
            });
        })
        .detach();
    }

    fn general_page(&self, weak: &WeakEntity<Self>) -> SettingPage {
        let weak = weak.clone();
        SettingPage::new("General")
            .icon(IconName::Settings)
            .default_open(true)
            .group(
                SettingGroup::new()
                    .title("Appearance")
                    .item(SettingItem::new(
                        "Theme",
                        if crate::demo::enabled() {
                            demo_theme_field()
                        } else {
                            SettingField::dropdown(
                                options(&[
                                    ("system", "Follow system"),
                                    ("light", "Light"),
                                    ("dark", "Dark"),
                                ]),
                                |cx| {
                                    match cx.global::<AppSettings>().theme {
                                        ThemeChoice::System => "system",
                                        ThemeChoice::Light => "light",
                                        ThemeChoice::Dark => "dark",
                                    }
                                    .into()
                                },
                                |value, cx| {
                                    let theme = match value.as_ref() {
                                        "light" => ThemeChoice::Light,
                                        "dark" => ThemeChoice::Dark,
                                        _ => ThemeChoice::System,
                                    };
                                    AppSettings::update(cx, |s| s.theme = theme);
                                },
                            )
                        },
                    ))
                    .item(
                        SettingItem::new(
                            "Code colors",
                            SettingField::dropdown(
                                CodeColors::ALL
                                    .into_iter()
                                    .map(|colors| (colors.id().into(), colors.label().into()))
                                    .collect(),
                                |cx| cx.global::<AppSettings>().code_colors.id().into(),
                                |value, cx| {
                                    let colors = CodeColors::from_id(value.as_ref());
                                    AppSettings::update(cx, |s| s.code_colors = colors);
                                },
                            ),
                        )
                        .description(
                            "Syntax colors for code in chat, skills and models.json. \
                             Each has a light and a dark variant.",
                        ),
                    )
                    .item(SettingItem::render(|_, _, cx| code_colors_preview(cx)))
                    .item(
                        SettingItem::new(
                            "Text size",
                            SettingField::dropdown(
                                options(&[
                                    ("small", "Small"),
                                    ("default", "Default"),
                                    ("large", "Large"),
                                    ("larger", "Larger"),
                                ]),
                                |cx| {
                                    match cx.global::<AppSettings>().text_size {
                                        TextSize::Small => "small",
                                        TextSize::Default => "default",
                                        TextSize::Large => "large",
                                        TextSize::Larger => "larger",
                                    }
                                    .into()
                                },
                                |value, cx| {
                                    let size = match value.as_ref() {
                                        "small" => TextSize::Small,
                                        "large" => TextSize::Large,
                                        "larger" => TextSize::Larger,
                                        _ => TextSize::Default,
                                    };
                                    AppSettings::update(cx, |s| s.text_size = size);
                                },
                            ),
                        )
                        .description("Scales the whole interface."),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Start")
                    .item(SettingItem::new(
                        "Open in",
                        SettingField::dropdown(
                            options(&[
                                ("last", "The mode used last"),
                                ("chat", "Chat"),
                                ("code", "Code"),
                            ]),
                            |cx| {
                                match cx.global::<AppSettings>().start_mode {
                                    StartMode::Last => "last",
                                    StartMode::Chat => "chat",
                                    StartMode::Code => "code",
                                }
                                .into()
                            },
                            |value, cx| {
                                let mode = match value.as_ref() {
                                    "chat" => StartMode::Chat,
                                    "code" => StartMode::Code,
                                    _ => StartMode::Last,
                                };
                                AppSettings::update(cx, |s| s.start_mode = mode);
                            },
                        ),
                    ))
                    .item(
                        SettingItem::new(
                            "Reopen the last session",
                            hover_switch(
                                "settings-switch-1",
                                |cx| cx.global::<AppSettings>().reopen_last_session,
                                |on, cx| AppSettings::update(cx, |s| s.reopen_last_session = on),
                            ),
                        )
                        .description(
                            "Instead of the welcome screen, also for sessions on SSH hosts.",
                        ),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Background chats")
                    .item(
                        SettingItem::new(
                            "Notifications",
                            hover_switch(
                                "settings-switch-2",
                                |cx| cx.global::<AppSettings>().background_notifications,
                                |on, cx| {
                                    AppSettings::update(cx, |s| s.background_notifications = on);
                                },
                            ),
                        )
                        .description(
                            "When a chat working in the background finishes or asks something.",
                        ),
                    )
                    .item(
                        SettingItem::new(
                            if cfg!(target_os = "macos") {
                                "Bounce the Dock icon"
                            } else {
                                "Flash the window"
                            },
                            hover_switch(
                                "settings-switch-3",
                                |cx| cx.global::<AppSettings>().request_attention,
                                |on, cx| AppSettings::update(cx, |s| s.request_attention = on),
                            ),
                        )
                        .description("When that happens while Pistachio is not in front."),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Updates")
                    .item(
                        SettingItem::new(
                            "Check for updates when Pistachio opens",
                            hover_switch(
                                "settings-switch-4",
                                |cx| cx.global::<AppSettings>().check_for_updates,
                                |on, cx| AppSettings::update(cx, |s| s.check_for_updates = on),
                            ),
                        )
                        .description(
                            "Asks GitHub for the latest release (no personal data is sent). \
                             You are asked before anything is downloaded.",
                        ),
                    )
                    .item(SettingItem::render(move |_, _, cx| {
                        let weak = weak.clone();
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "Pistachio {}",
                                        crate::updates::CURRENT_VERSION
                                    )),
                            )
                            .child(
                                Button::new("check-updates-now")
                                    .small()
                                    .label("Check now")
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |_, cx| {
                                            cx.emit(SettingsEvent::CheckForUpdates);
                                        });
                                    }),
                            )
                    })),
            )
    }

    fn pi_page(&self, weak: &WeakEntity<Self>, cx: &App) -> SettingPage {
        let now = match &self.pi_version {
            Some(version) => version.clone(),
            None => "checking…".to_owned(),
        };
        let pi_description = if std::env::var_os("PISTACHIO_PI_BIN").is_some() {
            "Set by PISTACHIO_PI_BIN, which takes precedence.".to_owned()
        } else {
            format!("Leave empty to find pi on PATH. Version: {now}. Used by new sessions.")
        };
        let mut model_options: Vec<(SharedString, SharedString)> =
            vec![("".into(), "Let pi choose".into())];
        model_options.extend(self.models.iter().cloned());
        // A model set earlier that the visible session does not list still shows.
        let current = startup_model(cx);
        if !current.is_empty() && !model_options.iter().any(|(value, _)| *value == current) {
            model_options.push((current.clone().into(), current.into()));
        }
        let thinking_options: Vec<(SharedString, SharedString)> =
            std::iter::once((SharedString::from(""), SharedString::from("Let pi choose")))
                .chain(
                    THINKING_LEVELS
                        .iter()
                        .map(|level| (SharedString::from(*level), SharedString::from(*level))),
                )
                .collect();
        let models_hint = if self.models.is_empty() {
            "Open a session to list its models here."
        } else {
            "The model new sessions start with."
        };
        let (weak_add, weak_edit) = (weak.clone(), weak.clone());
        SettingPage::new("pi & models")
            .icon(AppIcon::PiLogo)
            .group(
                SettingGroup::new().title("pi").item(
                    SettingItem::new(
                        "pi program",
                        SettingField::input(
                            |cx| {
                                cx.global::<AppSettings>()
                                    .pi_program
                                    .clone()
                                    .unwrap_or_default()
                                    .into()
                            },
                            |value, cx| {
                                let path = value.trim().to_owned();
                                AppSettings::update(cx, |s| {
                                    s.pi_program = Some(path).filter(|p| !p.is_empty());
                                });
                            },
                        ),
                    )
                    .description(pi_description),
                ),
            )
            .group(
                SettingGroup::new()
                    .title("Startup model (this computer)")
                    .description(
                        "Saved in pi's settings.json, like pi's /settings in the terminal.",
                    )
                    .item(
                        SettingItem::new(
                            "Model",
                            SettingField::scrollable_dropdown(
                                model_options,
                                |cx| startup_model(cx).into(),
                                |value, cx| {
                                    let (provider, model) = match value.split_once('/') {
                                        Some((provider, model)) => {
                                            (Some(provider.to_owned()), Some(model.to_owned()))
                                        }
                                        None => (None, None),
                                    };
                                    update_pi_startup(cx, |defaults| {
                                        defaults.provider = provider;
                                        defaults.model = model;
                                    });
                                },
                            ),
                        )
                        .description(models_hint),
                    )
                    .item(SettingItem::new(
                        "Thinking level",
                        SettingField::dropdown(
                            thinking_options,
                            |cx| {
                                cx.global::<PiStartupState>()
                                    .defaults
                                    .thinking_level
                                    .clone()
                                    .unwrap_or_default()
                                    .into()
                            },
                            |value, cx| {
                                let level = Some(value.to_string()).filter(|v| !v.is_empty());
                                update_pi_startup(cx, |defaults| defaults.thinking_level = level);
                            },
                        ),
                    ))
                    .item(SettingItem::render(move |_, _, cx| {
                        div().when_some(
                            cx.global::<PiStartupState>().error.clone(),
                            |this, error| this.text_sm().text_color(cx.theme().danger).child(error),
                        )
                    })),
            )
            .group(
                SettingGroup::new()
                    .title("Models")
                    .description("Custom models and providers in pi's models.json.")
                    .item(SettingItem::render(move |_, _, _| {
                        let (weak_add, weak_edit) = (weak_add.clone(), weak_edit.clone());
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-add-model")
                                    .small()
                                    .label("Add model…")
                                    .on_click(move |_, _, cx| {
                                        let _ = weak_add
                                            .update(cx, |_, cx| cx.emit(SettingsEvent::AddModel));
                                    }),
                            )
                            .child(
                                Button::new("settings-edit-models")
                                    .small()
                                    .label("Edit models.json…")
                                    .on_click(move |_, _, cx| {
                                        let _ = weak_edit
                                            .update(cx, |_, cx| cx.emit(SettingsEvent::EditModels));
                                    }),
                            )
                    })),
            )
    }

    fn web_search_page(&self) -> SettingPage {
        let (brave, tavily) = (self.brave_key.clone(), self.tavily_key.clone());
        let number = |min: f64, max: f64| NumberFieldOptions {
            min,
            max,
            step: 1.0,
        };
        SettingPage::new("Web search")
            .icon(AppIcon::Globe)
            .description("For chats with web search on (the globe next to the paperclip).")
            .group(
                SettingGroup::new()
                    .title("Search service")
                    .item(SettingItem::new(
                        "Service",
                        SettingField::dropdown(
                            options(&[
                                ("duckduckgo", "DuckDuckGo (no key)"),
                                ("searxng", "SearXNG (your instance)"),
                                ("brave", "Brave Search (API key)"),
                                ("tavily", "Tavily (API key)"),
                            ]),
                            |cx| backend_value(cx.global::<WebSearchState>().config.backend).into(),
                            |value, cx| {
                                let backend = match value.as_ref() {
                                    "searxng" => BackendKind::Searxng,
                                    "brave" => BackendKind::Brave,
                                    "tavily" => BackendKind::Tavily,
                                    _ => BackendKind::Duckduckgo,
                                };
                                update_web(cx, |config| config.backend = backend);
                            },
                        ),
                    ))
                    .item(
                        SettingItem::new(
                            "SearXNG address",
                            SettingField::input(
                                |cx| cx.global::<WebSearchState>().config.searxng.url.clone().into(),
                                |value, cx| {
                                    let url = value.trim().to_owned();
                                    update_web(cx, |config| config.searxng.url = url);
                                },
                            ),
                        )
                        .description("Its settings.yml must allow the json format."),
                    )
                    .item(
                        SettingItem::new(
                            "Brave API key",
                            SettingField::render(move |_, _, _| {
                                Input::new(&brave).small().mask_toggle().w(px(260.))
                            }),
                        )
                        .description("Stored only on this computer, readable by you alone."),
                    )
                    .item(SettingItem::new(
                        "Tavily API key",
                        SettingField::render(move |_, _, _| {
                            Input::new(&tavily).small().mask_toggle().w(px(260.))
                        }),
                    )),
            )
            .group(
                SettingGroup::new().title("New chats").item(SettingItem::new(
                    "Web search",
                    SettingField::dropdown(
                        options(&[("off", "Off"), ("search", "Search"), ("deep", "Deep search")]),
                        |cx| cx.global::<WebSearchState>().config.default_mode.as_str().into(),
                        |value, cx| {
                            if let Some(mode) = WebMode::parse(&value) {
                                update_web(cx, |config| config.default_mode = mode);
                            }
                        },
                    ),
                )),
            )
            .group(
                SettingGroup::new()
                    .title("Limits per message")
                    .description("The model is told to answer with what it found once a limit is reached.")
                    .item(SettingItem::new(
                        "Search: searches",
                        SettingField::number_input(
                            number(1., 50.),
                            |cx| f64::from(cx.global::<WebSearchState>().config.modes.search.max_searches),
                            |value, cx| update_web(cx, |c| c.modes.search.max_searches = to_u32(value)),
                        ),
                    ))
                    .item(SettingItem::new(
                        "Search: pages read",
                        SettingField::number_input(
                            number(0., 50.),
                            |cx| f64::from(cx.global::<WebSearchState>().config.modes.search.max_fetches),
                            |value, cx| update_web(cx, |c| c.modes.search.max_fetches = to_u32(value)),
                        ),
                    ))
                    .item(SettingItem::new(
                        "Deep search: searches",
                        SettingField::number_input(
                            number(1., 200.),
                            |cx| f64::from(cx.global::<WebSearchState>().config.modes.deep.max_searches),
                            |value, cx| update_web(cx, |c| c.modes.deep.max_searches = to_u32(value)),
                        ),
                    ))
                    .item(SettingItem::new(
                        "Deep search: pages read",
                        SettingField::number_input(
                            number(0., 200.),
                            |cx| f64::from(cx.global::<WebSearchState>().config.modes.deep.max_fetches),
                            |value, cx| update_web(cx, |c| c.modes.deep.max_fetches = to_u32(value)),
                        ),
                    ))
                    .item(
                        SettingItem::new(
                            "Text per page (characters)",
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 1000.,
                                    max: 100_000.,
                                    step: 1000.,
                                },
                                |cx| {
                                    #[allow(clippy::cast_precision_loss)]
                                    let chars = cx.global::<WebSearchState>().config.fetch.max_chars as f64;
                                    chars
                                },
                                |value, cx| {
                                    update_web(cx, |c| c.fetch.max_chars = to_u32(value) as usize);
                                },
                            ),
                        )
                        .description("Lower it for models with a small context window."),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Network")
                    .item(
                        SettingItem::new(
                            "Allow local network addresses",
                            hover_switch("settings-switch-5",
                                |cx| cx.global::<WebSearchState>().config.fetch.allow_private_networks,
                                |on, cx| update_web(cx, |c| c.fetch.allow_private_networks = on),
                            ),
                        )
                        .description(
                            "Off by default: pages the model asks for can then reach devices on your network.",
                        ),
                    )
                    .item(SettingItem::render(move |_, _, cx| {
                        div().when_some(cx.global::<WebSearchState>().error.clone(), |this, error| {
                            this.text_sm().text_color(cx.theme().danger).child(error)
                        })
                    })),
            )
    }

    fn ssh_page(&self, weak: &WeakEntity<Self>) -> SettingPage {
        let weak = weak.clone();
        SettingPage::new("SSH connections")
            .icon(IconName::Network)
            .description("Run pi on other machines.")
            .group(
                SettingGroup::new().item(
                    SettingItem::new(
                        "Connect saved connections at start",
                        hover_switch(
                            "settings-switch-hosts",
                            |cx| cx.global::<AppSettings>().connect_hosts_at_start,
                            |on, cx| AppSettings::update(cx, |s| s.connect_hosts_at_start = on),
                        ),
                    )
                    .description(
                        "Connects quietly in the background (no password prompts), so status and \
                         sessions are ready. Takes effect the next time Pistachio opens.",
                    ),
                ),
            )
            .group(
                SettingGroup::new()
                    .title("Saved connections")
                    .item(SettingItem::render(move |_, _, cx| {
                        let hosts = crate::hosts::hosts(cx);
                        let hosts = hosts.read(cx);
                        let muted = cx.theme().muted_foreground;
                        let rows: Vec<AnyElement> = hosts
                            .hosts()
                            .iter()
                            .enumerate()
                            .map(|(index, host)| {
                                let status = hosts.status(&host.id);
                                let connected = status.is_connected();
                                let (weak, id) = (weak.clone(), host.id.clone());
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .py_1()
                                    .child(crate::remote_ui::status_dot(Some(&status), cx))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .truncate()
                                                    .child(host.label.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .truncate()
                                                    .child(crate::remote_ui::status_text(&status)),
                                            ),
                                    )
                                    .when(!connected, |this| {
                                        this.child(
                                            Button::new(("settings-connect", index))
                                                .small()
                                                .label("Connect")
                                                .on_click(move |_, _, cx| {
                                                    let id = id.clone();
                                                    let _ = weak.update(cx, |_, cx| {
                                                        cx.emit(SettingsEvent::ConnectHost(id));
                                                    });
                                                }),
                                        )
                                    })
                                    .into_any_element()
                            })
                            .collect();
                        let empty = rows.is_empty();
                        let (weak_add, weak_manage) = (weak.clone(), weak.clone());
                        v_flex()
                            .w_full()
                            .gap_1()
                            .children(rows)
                            .when(empty, |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(muted)
                                        .child("No saved connections yet."),
                                )
                            })
                            .child(
                                h_flex()
                                    .pt_2()
                                    .gap_2()
                                    .child(
                                        Button::new("settings-add-host")
                                            .small()
                                            .primary()
                                            .label("Add SSH connection…")
                                            .on_click(move |_, _, cx| {
                                                let _ = weak_add.update(cx, |_, cx| {
                                                    cx.emit(SettingsEvent::AddHost);
                                                });
                                            }),
                                    )
                                    .when(!empty, |this| {
                                        this.child(
                                            Button::new("settings-manage-hosts")
                                                .small()
                                                .label("Rename, remove or change…")
                                                .on_click(move |_, _, cx| {
                                                    let _ = weak_manage.update(cx, |_, cx| {
                                                        cx.emit(SettingsEvent::ManageHosts);
                                                    });
                                                }),
                                        )
                                    }),
                            )
                    })),
            )
    }
}

impl SettingsView {
    /// Opens on the Speech to text page (before the first render).
    pub fn start_on_speech(&mut self) {
        self.start_page = 4;
    }

    fn speech_page(&self) -> SettingPage {
        use crate::stt::{AutoStop, catalog};
        SettingPage::new("Speech to text")
            .icon(IconName::Mic)
            .description(
                "Dictate messages with the microphone button. Speech is transcribed by a model \
                 that runs on this computer; nothing is sent anywhere.",
            )
            .group(
                SettingGroup::new()
                    .title("Models")
                    .item(SettingItem::render(|_, _, cx| {
                        crate::stt::ui::settings_model_rows(cx)
                    })),
            )
            .group(
                SettingGroup::new()
                    .title("Dictation")
                    .item(
                        SettingItem::new(
                            "Stop listening",
                            SettingField::dropdown(
                                AutoStop::ALL
                                    .into_iter()
                                    .map(|stop| (stop.id().into(), stop.label().into()))
                                    .collect(),
                                |cx| cx.global::<AppSettings>().speech_auto_stop.id().into(),
                                |value, cx| {
                                    let stop = AutoStop::from_id(value.as_ref());
                                    AppSettings::update(cx, |s| s.speech_auto_stop = stop);
                                },
                            ),
                        )
                        .description(
                            "Dictation ends by itself after this much silence. Enter or the stop \
                             button end it at any time; Esc discards it.",
                        ),
                    )
                    .item(
                        SettingItem::new(
                            "Language (Whisper)",
                            SettingField::dropdown(
                                catalog::LANGUAGES
                                    .iter()
                                    .map(|(code, label)| ((*code).into(), (*label).into()))
                                    .collect(),
                                |cx| cx.global::<AppSettings>().speech_language.clone().into(),
                                |value, cx| {
                                    let language = value.to_string();
                                    AppSettings::update(cx, |s| s.speech_language = language);
                                },
                            ),
                        )
                        .description(
                            "Whisper is more accurate when told the language. Parakeet detects \
                             it by itself; Moonshine understands English only.",
                        ),
                    ),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        div().size_full().bg(cx.theme().background).child(
            Settings::new("app-settings")
                .default_selected_index(SelectIndex {
                    page_ix: self.start_page,
                    group_ix: None,
                })
                .pages([
                    self.general_page(&weak),
                    self.pi_page(&weak, cx),
                    self.web_search_page(),
                    self.ssh_page(&weak),
                    self.speech_page(),
                ]),
        )
    }
}

/// An on/off setting: gpui-kit's switch with a hover ring, so it reads as clickable.
fn hover_switch(
    id: &'static str,
    value: impl Fn(&App) -> bool + 'static,
    set_value: impl Fn(bool, &mut App) + 'static,
) -> SettingField<SharedString> {
    let set_value = std::rc::Rc::new(set_value);
    SettingField::render(move |_, _, cx| {
        let set_value = set_value.clone();
        let hover = cx.theme().secondary_hover;
        div()
            .id(id)
            .p(px(3.))
            .rounded_full()
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .child(
                Switch::new(id)
                    .checked(value(cx))
                    .on_click(move |checked: &bool, _, cx| set_value(*checked, cx)),
            )
    })
}

/// The Theme menu in demo mode: the stock dropdown's look, with items the demo cursor can
/// point at (see [`crate::demo::menu_item`]).
fn demo_theme_field() -> SettingField<SharedString> {
    SettingField::render(|options: &RenderOptions, _: &mut Window, cx: &mut App| {
        const CHOICES: [(ThemeChoice, &str, &str); 3] = [
            (ThemeChoice::System, "system", "Follow system"),
            (ThemeChoice::Light, "light", "Light"),
            (ThemeChoice::Dark, "dark", "Dark"),
        ];
        let current = cx.global::<AppSettings>().theme;
        let index = CHOICES
            .iter()
            .position(|(choice, ..)| *choice == current)
            .unwrap_or_default();
        let button = Button::new(("demo-theme", index))
            .label(CHOICES[index].2)
            .dropdown_caret(true)
            .outline()
            .with_size(options.size())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                for (choice, value, label) in CHOICES {
                    menu = menu.item(
                        crate::demo::menu_item(label, format!("theme:{value}"))
                            .checked(choice == current)
                            .on_click(move |_, _, cx| {
                                AppSettings::update(cx, |s| s.theme = choice);
                            }),
                    );
                }
                menu
            });
        crate::demo::mark("theme-picker", button)
    })
}

/// The Code colors preview: a Python snippet drawn as chat draws code blocks, in the chosen
/// palette. It redraws as soon as another palette is picked.
fn code_colors_preview(cx: &App) -> AnyElement {
    const SNIPPET: &str = r#"```python
from dataclasses import dataclass

@dataclass
class Model:
    """A model entry from models.json."""
    id: str
    context_window: int = 131_072
    reasoning: bool = True

    def label(self, short: bool = False) -> str | None:
        name = self.id.split("/")[-1]  # drop the provider
        return f"{name} ({self.context_window // 1024}k)" if not short else None
```"#;
    // Keyed by palette and mode, so a change never shows a stale highlight.
    let id = format!(
        "code-colors-preview-{}-{}",
        cx.global::<AppSettings>().code_colors.id(),
        if cx.theme().mode.is_dark() {
            "dark"
        } else {
            "light"
        },
    );
    div()
        .w_full()
        .text_sm()
        .child(component::text::TextView::markdown(
            SharedString::from(id),
            SNIPPET,
        ))
        .into_any_element()
}

fn options(pairs: &[(&'static str, &'static str)]) -> Vec<(SharedString, SharedString)> {
    pairs
        .iter()
        .map(|(value, label)| (SharedString::from(*value), SharedString::from(*label)))
        .collect()
}

fn backend_value(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Duckduckgo => "duckduckgo",
        BackendKind::Searxng => "searxng",
        BackendKind::Brave => "brave",
        BackendKind::Tavily => "tavily",
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_u32(value: f64) -> u32 {
    value.round().clamp(0., f64::from(u32::MAX)) as u32
}

fn startup_model(cx: &App) -> String {
    let defaults = &cx.global::<PiStartupState>().defaults;
    match (&defaults.provider, &defaults.model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        _ => String::new(),
    }
}

fn load_web_search(cx: &mut App) {
    let dir = cx.global::<AppConfig>().deepsearch_dir.clone();
    let (config, error) = match deepsearch::config::load(&dir) {
        Ok((config, _)) => (config, None),
        Err(error) => (
            WebConfig::default(),
            Some(format!("{error}; changes are not saved.")),
        ),
    };
    let dir = error.is_none().then_some(dir);
    cx.set_global(WebSearchState { dir, config, error });
}

/// Changes web search's settings and saves them (unless the file could not be read: then
/// saving would replace what the user wrote).
fn update_web(cx: &mut App, f: impl FnOnce(&mut WebConfig)) {
    let state = cx.global_mut::<WebSearchState>();
    let before = state.config.clone();
    f(&mut state.config);
    if state.config == before {
        return;
    }
    let Some(dir) = state.dir.clone() else { return };
    let config = state.config.clone();
    let path = dir.join(deepsearch::config::CONFIG_FILE);
    crate::persist::save_in_background(cx, path, move |_| {
        deepsearch::config::save(&dir, &config).map_err(std::io::Error::other)
    });
}

fn pi_settings_path() -> Option<PathBuf> {
    pi_sessions::agent_dir().map(|agent| agent.join("settings.json"))
}

fn load_pi_startup(cx: &mut App) {
    let path = pi_settings_path();
    let text = path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let state = match pi_config::settings::startup_defaults(&text) {
        Ok(defaults) => PiStartupState {
            path,
            defaults,
            error: None,
        },
        Err(error) => PiStartupState {
            path: None,
            defaults: StartupDefaults::default(),
            error: Some(format!("pi's {error}; fix the file to change these here.")),
        },
    };
    cx.set_global(state);
}

/// Changes the startup defaults and writes them into pi's `settings.json`, keeping everything
/// else in the file (read again right before writing, in case pi changed it meanwhile).
fn update_pi_startup(cx: &mut App, f: impl FnOnce(&mut StartupDefaults)) {
    let state = cx.global_mut::<PiStartupState>();
    let before = state.defaults.clone();
    f(&mut state.defaults);
    if state.defaults == before {
        return;
    }
    let Some(path) = state.path.clone() else {
        return;
    };
    let defaults = state.defaults.clone();
    crate::persist::save_in_background(cx, path, move |path| write_pi_settings(path, &defaults));
}

fn write_pi_settings(path: &Path, defaults: &StartupDefaults) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = pi_config::settings::with_startup_defaults(&text, defaults)
        .map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.pistachio-tmp");
    std::fs::write(&tmp, updated)?;
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(tmp, path)
}
