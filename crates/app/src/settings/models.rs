//! Settings → pi & models: the providers in models.json, and whether their servers answer.

use std::collections::HashMap;

use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::*;
use pi_config::server::{self, Location};
use pi_config::{ModelsFile, ProviderInfo};

use super::view::{SettingsEvent, SettingsView};
use crate::chat::models_dialog::{self, Open};
use crate::pi_place::PiPlace;
use gpui_kit::prelude::FluentBuilder as _;

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
    /// The provider whose trash button was pressed: removing it waits for a yes.
    removing: Option<String>,
    /// A provider is being removed.
    busy: bool,
    _tasks: Vec<Task<()>>,
    _remove: Option<Task<()>>,
}

/// Where a provider's models run, which the list is grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    ThisDevice,
    OtherDevices,
    Api,
}

impl Group {
    const ALL: [Self; 3] = [Self::ThisDevice, Self::OtherDevices, Self::Api];

    fn of(provider: &ProviderInfo) -> Self {
        if provider.built_in().is_some() {
            return Self::Api;
        }
        match addresses(provider).first().map(|url| server::location(url)) {
            Some(Location::ThisDevice) => Self::ThisDevice,
            Some(Location::OtherDevice) => Self::OtherDevices,
            Some(Location::Online) | None => Self::Api,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::ThisDevice => "This device",
            Self::OtherDevices => "Other devices",
            Self::Api => "API connections",
        }
    }
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
        self.removing = None;
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
    let groups: Vec<AnyElement> = Group::ALL
        .into_iter()
        .filter_map(|group| {
            let rows: Vec<AnyElement> = connected
                .providers
                .iter()
                .filter(|provider| Group::of(provider) == group)
                .map(|provider| row(provider, connected, weak, cx))
                .collect();
            (!rows.is_empty()).then(|| {
                let title = group.title();
                v_flex()
                    .debug_selector(move || format!("connected-group:{title}"))
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(title),
                    )
                    .children(rows)
                    .into_any_element()
            })
        })
        .collect();
    let open = |open: Open| {
        let weak = weak.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |_, cx| cx.emit(SettingsEvent::Models(open.clone())));
        }
    };
    v_flex()
        .w_full()
        .gap_4()
        .children(groups)
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
    let open = {
        let (weak, id) = (weak.clone(), id.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |_, cx| {
                cx.emit(SettingsEvent::Models(Open::Provider(id.clone())));
            });
        }
    };
    let card = Button::new(SharedString::from(format!("connected:{id}")))
        .choice_card(false, cx)
        .accessibility_label(format!("{}, {detail} {state}", provider.label()))
        .child(
            h_flex()
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
                    h_flex()
                        .flex_none()
                        .gap_1p5()
                        .text_xs()
                        .text_color(muted)
                        .child(light(color))
                        .child(state)
                }))
                .child(Icon::new(IconName::ChevronRight).text_color(muted)),
        )
        .on_click(open);
    let removing = connected.removing.as_deref() == Some(id.as_str());
    let trash = {
        let (weak, id) = (weak.clone(), id.clone());
        div()
            .flex_none()
            .debug_selector(|| format!("remove:{}", provider.id))
            .child(
                Button::new(SharedString::from(format!("remove:{id}")))
                    .quiet(cx)
                    .small()
                    .icon(Icon::from(AppIcon::Trash).text_color(muted))
                    .accessibility_label(format!("Remove {}", provider.label()))
                    .disabled(connected.busy)
                    .on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.connected.removing = Some(id.clone());
                            cx.notify();
                        });
                    }),
            )
    };
    v_flex()
        .w_full()
        .gap_1p5()
        .debug_selector(|| format!("connected:{id}"))
        .child(
            h_flex()
                .w_full()
                .gap_1()
                .child(div().flex_1().min_w_0().child(card))
                .child(trash),
        )
        .when(removing, |this| {
            this.child(confirm(provider, connected, weak, cx))
        })
        .into_any_element()
}

/// Asks before removing a provider; one this app set up on this device is deleted too.
fn confirm(
    provider: &ProviderInfo,
    connected: &Connected,
    weak: &WeakEntity<SettingsView>,
    cx: &App,
) -> Div {
    let installed = installed(provider, cx);
    let label = provider.label();
    let models = match provider.models.len() {
        0 => String::new(),
        1 => " and its model".to_owned(),
        n => format!(" and its {n} models"),
    };
    let (question, yes) = match (&installed, Group::of(provider)) {
        (Some(installed), _) => {
            let what: Vec<&str> = installed
                .model
                .iter()
                .chain(&installed.engine)
                .map(String::as_str)
                .collect();
            let question = if what.is_empty() {
                format!("Remove {label} from pi? Its files stay: another local model uses them.")
            } else {
                let size = if installed.size > 0 {
                    format!(", about {}", crate::model_store::size_label(installed.size))
                } else {
                    String::new()
                };
                format!(
                    "Remove {label} from pi and delete {} from this device{size}?",
                    what.join(" and ")
                )
            };
            (question, "Delete")
        }
        (None, Group::ThisDevice) => (
            format!(
                "Remove {label}{models} from pi? The server and its models stay on this device."
            ),
            "Remove",
        ),
        (None, Group::OtherDevices) => (
            format!("Remove {label}{models} from pi? Nothing changes on that device."),
            "Remove",
        ),
        (None, Group::Api) => (
            format!("Remove {label}{models} and its API key from pi?"),
            "Remove",
        ),
    };
    let id = provider.id.clone();
    let remove = {
        let (weak, id) = (weak.clone(), id.clone());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |view, cx| remove(view, &id, cx));
        }
    };
    let keep = {
        let weak = weak.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |view, cx| {
                view.connected.removing = None;
                cx.notify();
            });
        }
    };
    h_flex()
        .w_full()
        .gap_2()
        .pl_2()
        .child(div().flex_1().min_w_0().text_xs().child(question))
        .child(
            div()
                .flex_none()
                .debug_selector(|| format!("confirm-remove:{id}"))
                .child(
                    Button::new(SharedString::from(format!("confirm-remove:{id}")))
                        .form_danger(cx)
                        .label(yes)
                        .disabled(connected.busy)
                        .on_click(remove),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("keep:{id}")))
                .form_other(cx)
                .label("Keep")
                .disabled(connected.busy)
                .on_click(keep),
        )
}

/// What this app set up on this device for `provider`, which removing it deletes.
fn installed(provider: &ProviderInfo, cx: &App) -> Option<crate::local_models::Installed> {
    let models = crate::local_models::local_models(cx)?;
    let models = models.read(cx);
    addresses(provider)
        .iter()
        .find_map(|url| models.installed_at(url))
}

/// Removes provider `id` from models.json and, when this app set it up on this device, stops it
/// and deletes its model and engine.
fn remove(view: &mut SettingsView, id: &str, cx: &mut Context<SettingsView>) {
    let Some(place) = PiPlace::for_host(None, cx) else {
        return;
    };
    let Some(provider) = view
        .connected
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .cloned()
    else {
        return;
    };
    view.connected.busy = true;
    view.connected.error = None;
    cx.notify();
    let id = id.to_owned();
    let task = cx.spawn(async move |this, cx| {
        let saved = cx
            .background_spawn(async move {
                let path = place.agent_path("models.json");
                let text = place.read(&path)?.unwrap_or_default();
                let mut file = ModelsFile::parse(&text)
                    .map_err(|error| format!("models.json has a problem. {error}"))?;
                file.remove_provider(&id);
                place.write(&path, &file.to_text(), true)
            })
            .await;
        let deleted = match saved {
            Err(error) => Err(error),
            Ok(()) => {
                let deleting = this
                    .update(cx, |_, cx| {
                        let models = crate::local_models::local_models(cx)?;
                        addresses(&provider)
                            .iter()
                            .find_map(|url| models.update(cx, |models, cx| models.remove(url, cx)))
                    })
                    .ok()
                    .flatten();
                match deleting {
                    Some(task) => task.await.map(|()| true),
                    None => Ok(false),
                }
            }
        };
        let _ = this.update(cx, |view, cx| {
            view.connected.busy = false;
            match deleted {
                Ok(deleted) => {
                    view.models_changed(cx);
                    let label = provider.label();
                    let note = if deleted {
                        format!("Removed {label} and deleted its files.")
                    } else {
                        format!("Removed {label}.")
                    };
                    cx.emit(SettingsEvent::ModelsRemoved(note.into()));
                }
                Err(error) => {
                    // models.json may have changed already.
                    view.models_changed(cx);
                    view.connected.error = Some(error.into());
                    cx.notify();
                }
            }
        });
    });
    view.connected._remove = Some(task);
}

#[cfg(test)]
mod tests {
    use pi_config::ModelsFile;

    use super::Group;

    #[test]
    fn groups_by_where_models_run() {
        let file = ModelsFile::parse(
            r#"{"providers": {
                "llama-cpp": {"baseUrl": "http://127.0.0.1:39280/v1", "api": "openai-completions", "apiKey": "none", "models": [{"id": "m"}]},
                "spark": {"baseUrl": "http://192.168.1.89:8888/v1", "api": "openai-completions", "apiKey": "none", "models": [{"id": "m"}]},
                "openrouter": {"baseUrl": "https://openrouter.ai/api/v1", "api": "openai-completions", "apiKey": "$KEY", "models": [{"id": "m"}]},
                "openai": {"apiKey": "sk-test"}
            }}"#,
        )
        .unwrap();
        let groups: Vec<_> = file
            .providers()
            .iter()
            .map(|provider| (provider.id.clone(), Group::of(provider)))
            .collect();
        assert_eq!(
            groups,
            [
                ("llama-cpp".to_owned(), Group::ThisDevice),
                ("spark".to_owned(), Group::OtherDevices),
                ("openrouter".to_owned(), Group::Api),
                ("openai".to_owned(), Group::Api),
            ]
        );
    }
}
