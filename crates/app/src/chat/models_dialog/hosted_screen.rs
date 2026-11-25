//! Locally hosted models: on this device or on another one. Nothing is assumed to run anywhere:
//! once a device is chosen, Ollama, LM Studio, llama.cpp and vLLM are looked for on their usual
//! ports there, and any server can also be connected by its address or port.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::{IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::server::{self, KNOWN_SERVERS, Kind, KnownServer, Lookup, THIS_DEVICE};

use super::start_screen::{Back, available, grid};
use super::{ModelsDialog, card, label, probe};
use crate::pi_place::PiPlace;
use crate::ui::AppIcon;

/// Where the server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Device {
    This,
    Other,
}

/// Whether a known server answers on its usual port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Found {
    Checking,
    Missing,
    /// `models` is `None` when it wants a key first.
    Running {
        kind: Kind,
        models: Option<usize>,
    },
}

/// The known servers on a device, as far as they answer.
pub(super) struct Scan {
    pub host: String,
    pub servers: Vec<(KnownServer, Found)>,
    /// How to start the server last clicked that does not answer.
    pub hint: Option<SharedString>,
}

impl Scan {
    fn nothing(&self) -> bool {
        self.servers
            .iter()
            .all(|(_, found)| *found == Found::Missing)
    }
}

impl ModelsDialog {
    pub(super) fn choose_device(&mut self, device: Device, cx: &mut Context<Self>) {
        self.device = Some(device);
        self.address_error = None;
        if device == Device::This && self.this_device.is_none() {
            self.scan(device, THIS_DEVICE.to_owned(), cx);
        }
        cx.notify();
    }

    fn scan_of(&mut self, device: Device) -> &mut Option<Scan> {
        match device {
            Device::This => &mut self.this_device,
            Device::Other => &mut self.other_device,
        }
    }

    /// Looks for the known servers on `host`, each on its usual port.
    fn scan(&mut self, device: Device, host: String, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        let slot = self.scan_of(device);
        let hint = slot
            .take()
            .filter(|scan| scan.host == host)
            .and_then(|scan| scan.hint);
        *slot = Some(Scan {
            host: host.clone(),
            servers: KNOWN_SERVERS
                .iter()
                .map(|server| (*server, Found::Checking))
                .collect(),
            hint,
        });
        for (ix, server) in KNOWN_SERVERS.iter().enumerate() {
            let (server, pi, host) = (*server, place.pi.clone(), host.clone());
            let task = cx.spawn(async move |this, cx| {
                let found = cx
                    .background_executor()
                    .spawn({
                        let host = host.clone();
                        async move { probe::find(&pi, &server, &host) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    // Another device may have been asked for since.
                    if let Some(scan) = this.scan_of(device).as_mut()
                        && scan.host == host
                    {
                        scan.servers[ix].1 = match found {
                            Some((kind, models)) => Found::Running { kind, models },
                            None => Found::Missing,
                        };
                        cx.notify();
                    }
                });
            });
            self._tasks.push(task);
        }
    }

    /// Connects to what is typed for another device: looks around on a device, or asks a server.
    pub(super) fn connect_to_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value().trim().to_owned();
        match server::lookup(&address) {
            Ok(Lookup::Device(host)) => {
                self.scan(Device::Other, host, cx);
                cx.notify();
            }
            Ok(Lookup::Address(urls)) => {
                self.back_to = Back::Hosted;
                self.open_new_server(urls, window, cx);
            }
            Err(error) => {
                self.address_error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    /// Connects to a server on another port of this device.
    pub(super) fn connect_to_port(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self
            .port
            .read(cx)
            .value()
            .trim()
            .trim_start_matches(':')
            .to_owned();
        match port.parse::<u16>() {
            Ok(port) if port > 0 => {
                if let Ok(urls) = server::base_urls(&format!("{THIS_DEVICE}:{port}")) {
                    self.back_to = Back::Hosted;
                    self.open_new_server(urls, window, cx);
                }
            }
            _ => {
                self.address_error =
                    Some("Enter the port the server listens on, like 8000.".into());
                cx.notify();
            }
        }
    }

    fn open_found(
        &mut self,
        device: Device,
        server: KnownServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scan) = self.scan_of(device).as_mut() else {
            return;
        };
        let found = scan
            .servers
            .iter()
            .find(|(known, _)| *known == server)
            .map(|(_, found)| *found);
        match found {
            Some(Found::Running { .. }) => {
                scan.hint = None;
                let url = server.base_url(&scan.host);
                self.back_to = Back::Hosted;
                self.open_new_server(vec![url], window, cx);
            }
            Some(Found::Checking) | None => {}
            Some(Found::Missing) => {
                // It may run by now.
                scan.hint = Some(match device {
                    Device::This => server.start.into(),
                    Device::Other => format!("{} {}", server.start, server.share).into(),
                });
                let host = scan.host.clone();
                self.scan(device, host, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn render_hosted(&self, cx: &mut Context<Self>) -> AnyElement {
        let this_detail: SharedString = match self.place.as_ref().map(|place| &place.pi) {
            Some(PiPlace::Remote { label, .. }) => format!("{label}, where pi runs").into(),
            _ => "Where Pistachio runs".into(),
        };
        let device_card = |device: Device, icon: AppIcon, title, detail: SharedString| {
            let selector = match device {
                Device::This => "device:this",
                Device::Other => "device:other",
            };
            div()
                .debug_selector(|| selector.into())
                .w(relative(0.49))
                .child(
                    card(
                        selector.to_owned(),
                        icon,
                        title,
                        detail,
                        None,
                        self.device == Some(device),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_device(device, cx))),
                )
        };
        let devices = h_flex()
            .justify_between()
            .child(device_card(
                Device::This,
                AppIcon::Server,
                "This device",
                this_detail,
            ))
            .child(device_card(
                Device::Other,
                AppIcon::Globe,
                "Another device",
                "By its address".into(),
            ));
        let below = match self.device {
            None => label(
                "Ollama, LM Studio, llama.cpp, vLLM, or any server that speaks OpenAI's, \
                 Anthropic's or Gemini's API.",
                cx,
            )
            .into_any_element(),
            Some(Device::This) => self.render_this_device(cx),
            Some(Device::Other) => self.render_other_device(cx),
        };
        v_flex()
            .gap_3()
            .child(Self::render_top("Locally hosted model", None, cx))
            .child(devices)
            .child(below)
            .into_any_element()
    }

    fn render_this_device(&self, cx: &mut Context<Self>) -> AnyElement {
        let port = v_flex()
            .gap_1p5()
            .child(label("Another server on this device", cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.port).aria_label("Port"))
                    .child(connect_button(
                        "connect-port",
                        cx.listener(|this, _, window, cx| this.connect_to_port(window, cx)),
                    )),
            )
            .child(self.address_help(
                "Any server that speaks OpenAI's, Anthropic's or Gemini's API, by its port.",
                cx,
            ));
        v_flex()
            .gap_4()
            .children(
                self.this_device
                    .as_ref()
                    .map(|scan| self.render_scan(Device::This, scan, cx)),
            )
            .child(port)
            .into_any_element()
    }

    fn render_other_device(&self, cx: &mut Context<Self>) -> AnyElement {
        let address = v_flex()
            .gap_1p5()
            .child(label("Address of the device, or of the server", cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.address).aria_label("Address"))
                    .child(connect_button(
                        "connect",
                        cx.listener(|this, _, window, cx| this.connect_to_address(window, cx)),
                    )),
            )
            .child(self.address_help(
                "Without a port, Ollama, LM Studio, llama.cpp and vLLM are looked for on their \
                 usual ports. With one, any server that speaks OpenAI's, Anthropic's or Gemini's \
                 API.",
                cx,
            ));
        v_flex()
            .gap_4()
            .child(address)
            .children(
                self.other_device
                    .as_ref()
                    .map(|scan| self.render_scan(Device::Other, scan, cx)),
            )
            .into_any_element()
    }

    /// What went wrong with what was typed, else how it works.
    fn address_help(&self, help: &'static str, cx: &App) -> Div {
        match &self.address_error {
            Some(error) => div()
                .text_xs()
                .text_color(crate::theme::palette(cx).error)
                .child(error.clone()),
            None => label(help, cx),
        }
    }

    /// The known servers on a device: Ollama and LM Studio always, the others when they answer.
    fn render_scan(&self, device: Device, scan: &Scan, cx: &mut Context<Self>) -> Div {
        let prefix = match device {
            Device::This => "local",
            Device::Other => "other",
        };
        let cards: Vec<Div> = scan
            .servers
            .iter()
            .filter(|(server, found)| {
                matches!(server.kind, Kind::Ollama | Kind::LmStudio)
                    || matches!(found, Found::Running { .. })
            })
            .map(|(server, found)| {
                let server = *server;
                let (name, end) = match *found {
                    Found::Running { kind, models } => {
                        (kind.label().unwrap_or(server.label), available(models, cx))
                    }
                    Found::Checking => (
                        server.label,
                        label("Looking…", cx).flex_none().into_any_element(),
                    ),
                    Found::Missing => (
                        server.label,
                        label("Not running", cx).flex_none().into_any_element(),
                    ),
                };
                let selector = format!("{prefix}:{}", server.label);
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
                            false,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_found(device, server, window, cx);
                            },
                        )),
                    )
            })
            .collect();
        let title = match device {
            Device::This => "On this device".to_owned(),
            Device::Other => format!("On {}", scan.host),
        };
        v_flex()
            .gap_1p5()
            .child(label(title, cx))
            .child(grid(cards))
            .children(scan.hint.clone().map(|hint| label(hint, cx)))
            .when(device == Device::Other && scan.nothing(), |this| {
                this.child(label(
                    format!(
                        "Nothing answers on the usual ports. If the server listens on another \
                         one, enter it too, like {}:8888.",
                        scan.host
                    ),
                    cx,
                ))
            })
    }
}

/// A Connect button next to a field.
fn connect_button(
    id: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .debug_selector(|| id.into())
        .child(Button::new(id).label("Connect").on_click(on_click))
}
