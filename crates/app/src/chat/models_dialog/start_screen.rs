//! The first screen: what is connected, and where more models can come from.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use pi_config::ProviderInfo;
use pi_config::providers::{BEST_KNOWN, BUILT_IN, BuiltIn};
use pi_config::server::{self, Kind, LocalServer};

use super::{ModelsDialog, Screen, card, count, label, probe};
use crate::pi_place::PiPlace;
use crate::ui::{AppIcon, QuietButton as _, light};

/// Whether a server that usually runs on one's own computer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Local {
    Checking,
    Missing,
    Found { kind: Kind, models: usize },
}

impl ModelsDialog {
    /// Looks once for model servers where pi runs.
    pub(super) fn find_local(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        for (ix, (server, _)) in self.local.iter().enumerate() {
            let (server, pi) = (*server, place.pi.clone());
            let task = cx.spawn(async move |this, cx| {
                let found = cx
                    .background_executor()
                    .spawn(async move { probe::find(&pi, &server) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.local[ix].1 = match found {
                        Some((kind, models)) => Local::Found { kind, models },
                        None => Local::Missing,
                    };
                    cx.notify();
                });
            });
            self._tasks.push(task);
        }
    }

    pub(super) fn connect_to_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value().trim().to_owned();
        match server::base_urls(&address) {
            Ok(urls) => self.open_new_server(urls, window, cx),
            Err(error) => {
                self.address_error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    /// The provider in the file for the server at `base_url`.
    pub(super) fn provider_at(&self, base_url: &str) -> Option<&ProviderInfo> {
        let same = |url: &str| url.trim_end_matches('/').replace("127.0.0.1", "localhost");
        self.providers.iter().find(|provider| {
            provider
                .base_url
                .as_deref()
                .is_some_and(|url| same(url) == same(base_url))
        })
    }

    pub(super) fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let connected = self.render_connected(cx);
        let local = self.render_local(cx);
        let keys = self.render_providers(cx);
        let error_color = crate::theme::palette(cx).error;
        let server = v_flex()
            .gap_1p5()
            .child(label("On another server", cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.address).aria_label("Server address"))
                    .child(
                        div().flex_none().debug_selector(|| "connect".into()).child(
                            Button::new("connect")
                                .label("Connect")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.connect_to_address(window, cx);
                                })),
                        ),
                    ),
            )
            .child(match &self.address_error {
                Some(error) => div().text_xs().text_color(error_color).child(error.clone()),
                None => label(
                    "Any server that speaks OpenAI's, Anthropic's or Gemini's API. The rest is \
                     found out from its address.",
                    cx,
                ),
            });
        v_flex()
            .gap_3()
            .child(
                div()
                    .id("models-start")
                    .max_h(px(500.))
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap_4()
                            .pr_1()
                            .children(connected)
                            .child(local)
                            .child(keys)
                            .child(server),
                    ),
            )
            .child(
                h_flex().child(div().flex_1()).child(
                    div().debug_selector(|| "edit-models-json".into()).child(
                        Button::new("edit-models-json")
                            .quiet(cx)
                            .small()
                            .icon(Icon::from(AppIcon::SquarePen).text_color(muted))
                            .label("Edit models.json")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.screen = Screen::Json;
                                cx.notify();
                            })),
                    ),
                ),
            )
            .into_any_element()
    }

    /// The providers in the file.
    fn render_connected(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.providers.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        Some(
            v_flex()
                .gap_1p5()
                .child(label("Connected", cx))
                .children(self.providers.iter().map(|provider| {
                    let id = provider.id.clone();
                    let (icon, detail) = describe(provider, &self.available);
                    let end = Icon::new(IconName::ChevronRight)
                        .small()
                        .text_color(muted)
                        .into_any_element();
                    let selector = format!("provider:{id}");
                    div().debug_selector(|| selector.clone()).child(
                        card(
                            selector.clone(),
                            icon,
                            provider.label().to_owned(),
                            detail.into(),
                            Some(end),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.open_provider(&id, window, cx),
                        )),
                    )
                })),
        )
    }

    /// The servers on this computer.
    fn render_local(&self, cx: &mut Context<Self>) -> Div {
        let place = match self.place.as_ref().map(|place| &place.pi) {
            Some(PiPlace::Remote { label, .. }) => label.clone(),
            _ => "this computer".to_owned(),
        };
        let cards: Vec<Div> = self
            .local
            .iter()
            .filter(|(server, state)| {
                // These have ports of their own; the others only show when there is something.
                matches!(server.kind, Kind::Ollama | Kind::LmStudio)
                    || matches!(state, Local::Found { .. })
            })
            .map(|(server, state)| {
                let server = *server;
                let (name, end) = match *state {
                    Local::Found { kind, models } => (
                        kind.label().unwrap_or("Model server"),
                        available(models, cx),
                    ),
                    Local::Checking => (
                        server.label,
                        label("Looking…", cx).flex_none().into_any_element(),
                    ),
                    Local::Missing => (
                        server.label,
                        label("Not running", cx).flex_none().into_any_element(),
                    ),
                };
                let selector = format!("local:{}", server.label);
                div()
                    .debug_selector(|| selector.clone())
                    .w(relative(0.49))
                    .child(
                        card(
                            selector.clone(),
                            IconName::Cpu,
                            name,
                            SharedString::default(),
                            Some(end),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.open_local(server, window, cx),
                        )),
                    )
            })
            .collect();
        v_flex()
            .gap_1p5()
            .child(label(format!("On {place}"), cx))
            .child(grid(cards))
            .children(self.local_hint.map(|hint| label(hint, cx)))
    }

    fn open_local(&mut self, server: LocalServer, window: &mut Window, cx: &mut Context<Self>) {
        let state = self
            .local
            .iter()
            .find(|(local, _)| *local == server)
            .map(|(_, state)| *state);
        if let Some(Local::Found { .. }) = state {
            self.local_hint = None;
            self.open_new_server(vec![server.address.to_owned()], window, cx);
        } else {
            // It may run by now.
            self.local_hint = Some(server.start);
            self.find_local(cx);
            cx.notify();
        }
    }

    /// pi's own providers.
    fn render_providers(&self, cx: &mut Context<Self>) -> Div {
        let cards: Vec<Div> = BUILT_IN[..BEST_KNOWN]
            .iter()
            .map(|provider| {
                let models = self.available.get(provider.id).copied().unwrap_or_default();
                let selector = format!("built-in:{}", provider.id);
                div()
                    .debug_selector(|| selector.clone())
                    .w(relative(0.49))
                    .child(
                        card(
                            selector.clone(),
                            AppIcon::Key,
                            provider.label,
                            SharedString::default(),
                            (models > 0).then(|| available(models, cx)),
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_provider(provider.id, window, cx);
                            },
                        )),
                    )
            })
            .collect();
        let weak = cx.weak_entity();
        let more = Button::new("more-providers")
            .quiet(cx)
            .small()
            .label("More providers")
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| more_providers(menu, &weak));
        v_flex()
            .gap_1p5()
            .child(label("From a provider, with an API key or a sign-in", cx))
            .child(grid(cards))
            .child(h_flex().child(more))
    }
}

/// Cards, two in a row.
fn grid(cards: Vec<Div>) -> Div {
    h_flex()
        .flex_wrap()
        .justify_between()
        .gap_y_2()
        .children(cards)
}

fn more_providers(
    menu: component::menu::PopupMenu,
    weak: &WeakEntity<ModelsDialog>,
) -> component::menu::PopupMenu {
    BUILT_IN[BEST_KNOWN..].iter().fold(
        menu.scrollable(true).max_h(px(300.)),
        |menu, provider: &'static BuiltIn| {
            let weak = weak.clone();
            menu.item(
                PopupMenuItem::new(provider.label).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.open_provider(provider.id, window, cx);
                    });
                }),
            )
        },
    )
}

/// A green light and how many models there are.
fn available(models: usize, cx: &App) -> AnyElement {
    h_flex()
        .flex_none()
        .gap_1p5()
        .child(light(crate::theme::palette(cx).success))
        .child(label(count(models, "model"), cx))
        .into_any_element()
}

/// What to show of a provider in the file: its icon, and where its models come from and how many
/// there are.
pub fn describe(
    provider: &ProviderInfo,
    available: &std::collections::HashMap<String, usize>,
) -> (Icon, String) {
    if provider.built_in().is_some() {
        let detail = match available.get(&provider.id) {
            Some(models) => format!("API key · {}", count(*models, "model")),
            None => "API key".to_owned(),
        };
        return (AppIcon::Key.into(), detail);
    }
    let models = count(provider.models.len(), "model");
    match &provider.base_url {
        Some(url) => (
            AppIcon::Server.into(),
            format!("{} · {models}", server::short_address(url)),
        ),
        None => (AppIcon::Key.into(), models),
    }
}
