//! SearXNG in Settings → Web search: the address of one, or the one on this computer, with what
//! is missing for it and a button that gets and runs it.

use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingField, SettingItem};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use searxng::{Installed, Probe, catalog};

use super::view::{update_web, web_config};
use crate::model_store::size_label;
use crate::search_server::{Running, SearchServer, search_server};
use crate::ui::QuietButton as _;

/// What an installation takes on disk, about.
const DISK_SIZE: &str = "190 MB";

/// The settings below the service when SearXNG is chosen.
pub(super) fn items(cx: &App) -> Vec<SettingItem> {
    let supported = catalog::platform().is_some();
    if web_config(cx).searxng.local && supported {
        return vec![SettingItem::render(|_, _, cx| local_row(cx)).keywords(["SearXNG"])];
    }
    let address = SettingItem::new(
        "SearXNG address",
        SettingField::input(
            |cx| web_config(cx).searxng.url.clone().into(),
            |value, cx| {
                let url = value.trim().to_owned();
                update_web(cx, |config| config.searxng.url = url);
            },
        ),
    )
    .description("Its settings.yml must allow the json format.");
    let check = SettingItem::render(|_, _, cx| check_row(cx));
    let mut items = vec![address, check];
    // One that can be used makes another one unnecessary.
    if supported && probed(cx) != Some(Probe::Ready) {
        items.push(SettingItem::render(|_, _, cx| offer_row(cx)).keywords(["SearXNG"]));
    }
    items
}

fn probed(cx: &App) -> Option<Probe> {
    let url = &web_config(cx).searxng.url;
    search_server(cx).read(cx).probed(url)
}

/// What answers at the address.
fn check_row(cx: &App) -> AnyElement {
    let url = web_config(cx).searxng.url.clone();
    let (good, text) = match probed(cx) {
        None => (true, format!("Checking {url}…")),
        Some(Probe::Ready) => (
            true,
            format!("SearXNG answers at {url}. Nothing is missing."),
        ),
        Some(Probe::NoJson) => (
            false,
            format!(
                "SearXNG at {url} refuses the json format: add it to search.formats in its \
                 settings.yml."
            ),
        ),
        Some(Probe::Unexpected(status)) => (
            false,
            format!("What answers at {url} is not a SearXNG that can be used (HTTP {status})."),
        ),
        Some(Probe::Nothing) => (false, format!("Nothing answers at {url}.")),
    };
    let theme = cx.theme();
    row(
        "searxng-address-check",
        div()
            .text_sm()
            .text_color(if good {
                theme.muted_foreground
            } else {
                theme.warning
            })
            .child(text),
        Button::new("searxng-check")
            .label("Check again")
            .form_other(cx)
            .on_click(move |_, _, cx| {
                search_server(cx).update(cx, |server, cx| server.probe(&url, true, cx));
            }),
    )
}

/// What is missing on this computer, and a button that gets it.
fn offer_row(cx: &App) -> AnyElement {
    let installed = search_server(cx).read(cx).installed() != Installed::No;
    let text = if installed {
        "SearXNG is installed on this computer.".to_owned()
    } else {
        missing()
    };
    row(
        "searxng-offer",
        summary("Run SearXNG on this computer", text, cx),
        Button::new("searxng-get")
            .label(if installed {
                "Use it"
            } else {
                "Download and run"
            })
            .form_other(cx)
            .on_click(|_, _, cx| use_local(cx)),
    )
}

/// Makes chats search with the SearXNG on this computer, downloading it first if it is missing.
/// The Settings page must have been opened.
pub fn use_local(cx: &mut App) {
    update_web(cx, |config| config.searxng.local = true);
    search_server(cx).update(cx, |server, cx| {
        if server.installed() == Installed::No {
            server.install(cx);
        } else {
            server.start(cx);
        }
    });
}

fn missing() -> String {
    let size = catalog::platform().map_or(0, catalog::Platform::download_size);
    format!(
        "Missing on this computer: SearXNG, its own Python {} and {} Python packages. One click \
         downloads them ({}, {DISK_SIZE} on disk) into Pistachio's folder; nothing else on this \
         computer changes, and Docker is not needed. License: AGPL-3.0.",
        catalog::PYTHON_VERSION,
        catalog::package_count(),
        size_label(size),
    )
}

/// The SearXNG on this computer: its state, and what can be done with it.
fn local_row(cx: &App) -> AnyElement {
    let server = search_server(cx);
    let server = server.read(cx);
    let installed = server.installed();
    let text = if server.is_downloading() {
        match server.progress() {
            Some(progress) => format!(
                "Downloading {}… {:.0}%",
                progress.stage.label(),
                progress.fraction() * 100.
            ),
            None => "Downloading…".into(),
        }
    } else if installed == Installed::No {
        missing()
    } else {
        match server.running() {
            Running::Here(server) => format!(
                "Running on port {}. Only this computer can reach it. It stops when Pistachio \
                 quits.",
                server.port()
            ),
            Running::Elsewhere => "Running, started by another Pistachio.".into(),
            Running::Starting => "Starting…".into(),
            Running::No => "Installed. It starts when a chat searches the web.".into(),
        }
    };
    let button =
        |id: &'static str, label: &'static str| Button::new(id).label(label).form_other(cx);
    let busy = server.is_downloading() || matches!(server.running(), Running::Starting);
    let mut actions = h_flex().gap_2();
    if server.is_downloading() {
        actions = actions.child(button("searxng-cancel", "Cancel").on_click(|_, _, cx| {
            search_server(cx).update(cx, |server, _| server.cancel_download());
        }));
    } else if !busy {
        let (installed, stopped) = (
            installed != Installed::No,
            matches!(server.running(), Running::No),
        );
        let here = matches!(server.running(), Running::Here(_));
        actions = actions
            .when(!installed, |this| {
                this.child(
                    button("searxng-download", "Download and run").on_click(|_, _, cx| {
                        search_server(cx).update(cx, SearchServer::install);
                    }),
                )
            })
            .when(server.installed() == Installed::Outdated, |this| {
                this.child(button("searxng-update", "Update").on_click(|_, _, cx| {
                    search_server(cx).update(cx, SearchServer::install);
                }))
            })
            .when(installed && stopped, |this| {
                this.child(button("searxng-start", "Start").on_click(|_, _, cx| {
                    search_server(cx).update(cx, SearchServer::start);
                }))
            })
            .when(here, |this| {
                this.child(button("searxng-stop", "Stop").on_click(|_, _, cx| {
                    search_server(cx).update(cx, SearchServer::stop);
                }))
            })
            // Another Pistachio's would lose its files.
            .when(installed && (here || stopped), |this| {
                this.child(button("searxng-remove", "Remove").on_click(|_, _, cx| {
                    search_server(cx).update(cx, SearchServer::remove);
                    update_web(cx, |config| config.searxng.local = false);
                }))
            })
            .child(
                button("searxng-address", "Use another address").on_click(|_, _, cx| {
                    update_web(cx, |config| config.searxng.local = false);
                }),
            );
    }
    let error = server.error().map(str::to_owned);
    row(
        "searxng-local",
        summary("SearXNG on this computer", text, cx).when_some(error, |this, error| {
            this.child(div().text_sm().text_color(cx.theme().danger).child(error))
        }),
        actions,
    )
}

fn summary(title: &'static str, text: String, cx: &App) -> Div {
    v_flex()
        .gap_0p5()
        .child(div().text_sm().child(title))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
}

fn row(name: &'static str, text: impl IntoElement, actions: impl IntoElement) -> AnyElement {
    h_flex()
        .debug_selector(|| name.into())
        .w_full()
        .gap_3()
        .child(div().flex_1().min_w_0().child(text))
        .child(div().flex_none().child(actions))
        .into_any_element()
}
