//! Settings → pi & models: the providers in models.json, and whether their servers answer.

use std::collections::HashMap;

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, v_flex};
use gpui_kit::*;
use pi_config::{ModelsFile, ProviderInfo};

use super::view::{SettingsEvent, SettingsView};
use crate::chat::models_dialog::{self, Open};
use crate::pi_place::PiPlace;
use crate::ui::{AppIcon, FadingText, QuietButton as _, buttons, light};

/// The providers in models.json on this computer.
#[derive(Default)]
pub struct Connected {
    providers: Vec<ProviderInfo>,
    error: Option<SharedString>,
    /// How many models pi has of each provider, as far as that is known.
    available: HashMap<String, usize>,
    /// Whether the server of a provider answers, once that is known.
    online: HashMap<String, bool>,
    /// The servers were asked: once, when the list is first drawn.
    asked: bool,
    _tasks: Vec<Task<()>>,
}

impl Connected {
    /// `models` are the provider of each model the visible session has.
    pub fn load<'a>(models: impl Iterator<Item = &'a str>, cx: &App) -> Self {
        let mut this = Self::default();
        this.set_available(models);
        this.reload(cx);
        this
    }

    /// `models` are the provider of each model pi has.
    pub fn set_available<'a>(&mut self, models: impl Iterator<Item = &'a str>) {
        self.available.clear();
        for provider in models {
            *self.available.entry(provider.to_owned()).or_insert(0) += 1;
        }
    }

    pub fn providers(&self) -> &[ProviderInfo] {
        &self.providers
    }

    pub fn reload(&mut self, cx: &App) {
        (self.providers, self.error) = match read(cx) {
            Ok(file) => (file.providers(), None),
            Err(error) => (Vec::new(), Some(error.into())),
        };
        self.online.clear();
        self.asked = false;
    }

    /// Asks each server once whether it is there.
    fn ask(&mut self, cx: &mut Context<SettingsView>) {
        self.asked = true;
        let Some(place) = PiPlace::for_host(None, cx) else {
            return;
        };
        self._tasks.clear();
        for provider in &self.providers {
            let urls = addresses(provider);
            if urls.is_empty() {
                continue;
            }
            let (id, place) = (provider.id.clone(), place.clone());
            self._tasks.push(cx.spawn(async move |this, cx| {
                let online = cx
                    .background_executor()
                    .spawn(
                        async move { urls.iter().any(|url| models_dialog::answers(&place, url)) },
                    )
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.connected.online.insert(id, online);
                    cx.notify();
                });
            }));
        }
    }
}

fn read(cx: &App) -> Result<ModelsFile, String> {
    let Some(place) = PiPlace::for_host(None, cx) else {
        return Ok(ModelsFile::default());
    };
    let text = place.read(&place.agent_path("models.json"))?;
    ModelsFile::parse(&text.unwrap_or_default())
        .map_err(|error| format!("models.json has a problem. {error}"))
}

/// Where a provider's models are: its address, and those of models that have their own.
fn addresses(provider: &ProviderInfo) -> Vec<String> {
    let mut urls: Vec<String> = provider
        .base_url
        .iter()
        .chain(
            provider
                .models
                .iter()
                .filter_map(|model| model.base_url.as_ref()),
        )
        .cloned()
        .collect();
    urls.dedup();
    urls
}

/// The providers, and the buttons to add and edit.
pub(super) fn rows(weak: &WeakEntity<SettingsView>, cx: &mut App) -> AnyElement {
    let Some(view) = weak.upgrade() else {
        return div().into_any_element();
    };
    if !view.read(cx).connected.asked {
        // Only once it shows.
        let view = view.clone();
        cx.defer(move |cx| {
            view.update(cx, |view, cx| {
                if !view.connected.asked {
                    let mut connected = std::mem::take(&mut view.connected);
                    connected.ask(cx);
                    view.connected = connected;
                }
            });
        });
    }
    let connected = &view.read(cx).connected;
    let (muted, accent) = {
        let palette = crate::theme::palette(cx);
        (cx.theme().muted_foreground, palette.accent_icon)
    };
    let rows: Vec<AnyElement> = connected
        .providers
        .iter()
        .map(|provider| row(provider, connected, weak, cx))
        .collect();
    let open = |open: Open| {
        let weak = weak.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |_, cx| cx.emit(SettingsEvent::Models(open.clone())));
        }
    };
    v_flex()
        .w_full()
        .gap_2()
        .children(rows)
        .children(connected.error.clone().map(|error| {
            div()
                .text_sm()
                .text_color(crate::theme::palette(cx).error)
                .child(error)
        }))
        .children(
            (connected.providers.is_empty() && connected.error.is_none()).then(|| {
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Nothing is connected here yet.")
            }),
        )
        .child(
            buttons()
                .child(
                    Button::new("settings-add-models")
                        .form_other(cx)
                        .icon(Icon::from(IconName::Plus).text_color(accent))
                        .label("Add models…")
                        .on_click(open(Open::Start)),
                )
                .child(
                    Button::new("settings-edit-models")
                        .form_other(cx)
                        .icon(Icon::from(AppIcon::SquarePen).text_color(accent))
                        .label("Edit models.json…")
                        .on_click(open(Open::Json)),
                ),
        )
        .into_any_element()
}

fn row(
    provider: &ProviderInfo,
    connected: &Connected,
    weak: &WeakEntity<SettingsView>,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let palette = crate::theme::palette(cx);
    let (icon, detail) = models_dialog::describe(provider, &connected.available);
    let (color, state) = match connected.online.get(&provider.id) {
        Some(true) => (Some(palette.success), "Online"),
        Some(false) => (Some(palette.error), "Offline"),
        None => (None, ""),
    };
    let id = provider.id.clone();
    let weak = weak.clone();
    div()
        .w_full()
        .debug_selector(|| format!("connected:{id}"))
        .child(
            Button::new(SharedString::from(format!("connected:{id}")))
                .choice_card(false, cx)
                .accessibility_label(format!("{}, {detail} {state}", provider.label()))
                .child(
                    component::h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2p5()
                        .child(icon.text_color(muted))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .items_start()
                                .child(FadingText::new(provider.label().to_owned()))
                                .child(
                                    div()
                                        .w_full()
                                        .text_xs()
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(muted)
                                        .child(FadingText::new(detail)),
                                ),
                        )
                        .children(color.map(|color| {
                            component::h_flex()
                                .flex_none()
                                .gap_1p5()
                                .text_xs()
                                .text_color(muted)
                                .child(light(color))
                                .child(state)
                        }))
                        .child(Icon::new(IconName::ChevronRight).text_color(muted)),
                )
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |_, cx| {
                        cx.emit(SettingsEvent::Models(Open::Provider(id.clone())));
                    });
                }),
        )
        .into_any_element()
}
