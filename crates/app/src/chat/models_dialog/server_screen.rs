//! A model server: what it offers, which of its models pi uses, and how pi connects to it.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::server::{self, Kind, Offer, OfferedModel};
use pi_config::{ModelSpec, ProviderInfo, ProviderSpec, SavedModel};

use super::probe::{self, Connection};
use super::{ModelsDialog, Screen, bottom, count, field, grouped, label};
use crate::ui::{FadingText, QuietButton as _, described_item, light};

/// The most rows drawn at once; searching finds the others.
const MOST_ROWS: usize = 150;

/// The key of a server that has none: pi only offers models of providers with one.
const NO_KEY: &str = "none";

pub(super) struct ServerScreen {
    /// The provider in the file, when it is not a new one.
    existing: Option<ProviderInfo>,
    /// The addresses to try, the likeliest first.
    urls: Vec<String>,
    base_url: Entity<InputState>,
    pub(super) key: Entity<InputState>,
    name: Entity<InputState>,
    id: Entity<InputState>,
    search: Entity<InputState>,
    pub(super) by_id: Entity<InputState>,
    api: String,
    kind: Kind,
    state: State,
    pub(super) rows: Vec<Row>,
    /// The connection options show.
    options: bool,
    /// "Remove" was pressed once.
    removing: bool,
    pub(super) error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

enum State {
    /// There is no address to connect to: one of pi's own providers.
    NoAddress,
    Connecting,
    Online,
    /// Only models with an address of their own are there.
    Elsewhere,
    /// Whether it got a key, which it did not take.
    NeedsKey {
        refused: bool,
    },
    NoList(SharedString),
    Offline(SharedString),
}

#[allow(clippy::struct_excessive_bools)] // independent flags
pub(super) struct Row {
    spec: ModelSpec,
    /// It is in the file.
    saved: bool,
    ticked: bool,
    /// The server lists it.
    offered: bool,
    not_chat: bool,
    /// Its own address, when it is not the provider's.
    base_url: Option<String>,
    open: bool,
    pub(super) fields: Option<Fields>,
    trial: Trial,
}

/// The fields of a row that was opened.
pub(super) struct Fields {
    name: Entity<InputState>,
    pub(super) context_window: Entity<InputState>,
    max_tokens: Entity<InputState>,
    _enter: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trial {
    None,
    Running,
    Worked(Duration),
    Failed(SharedString),
}

impl Row {
    fn new(spec: ModelSpec) -> Self {
        Self {
            spec,
            saved: false,
            ticked: false,
            offered: false,
            not_chat: false,
            base_url: None,
            open: false,
            fields: None,
            trial: Trial::None,
        }
    }

    fn saved(model: &SavedModel) -> Self {
        Self {
            saved: true,
            ticked: true,
            base_url: model.base_url.clone(),
            ..Self::new(model.spec.clone())
        }
    }

    fn offered(model: OfferedModel) -> Self {
        let max_tokens = model
            .max_tokens
            .or_else(|| server::fitting_max_tokens(model.context_window?));
        Self {
            offered: true,
            not_chat: model.not_chat,
            ..Self::new(ModelSpec {
                id: model.id,
                name: model.name.unwrap_or_default(),
                context_window: model.context_window,
                max_tokens,
                reasoning: model.reasoning.unwrap_or(false),
                images: model.images.unwrap_or(false),
            })
        }
    }

    /// What to call it.
    fn label(&self) -> &str {
        if self.spec.name.is_empty() {
            &self.spec.id
        } else {
            &self.spec.name
        }
    }

    fn matches(&self, search: &str) -> bool {
        search.split_whitespace().all(|word| {
            let word = word.to_lowercase();
            self.spec.id.to_lowercase().contains(&word)
                || self.spec.name.to_lowercase().contains(&word)
        })
    }

    /// What the form says about the model.
    fn spec(&self, cx: &App) -> Result<ModelSpec, String> {
        let mut spec = self.spec.clone();
        if let Some(fields) = &self.fields {
            spec.name = text(&fields.name, cx);
            spec.context_window = tokens(&fields.context_window, "The context window", cx)
                .map_err(|error| format!("{}: {error}", spec.id))?;
            spec.max_tokens = tokens(&fields.max_tokens, "The longest answer", cx)
                .map_err(|error| format!("{}: {error}", spec.id))?;
        }
        Ok(spec)
    }
}

/// The models in the file, then the others the server offers: those for conversations first.
fn merge(rows: &mut Vec<Row>, offer: Offer) {
    for row in rows.iter_mut() {
        row.offered = false;
    }
    let mut new = Vec::new();
    for model in offer.models {
        match rows.iter_mut().find(|row| row.spec.id == model.id) {
            Some(row) => row.offered = true,
            None => new.push(Row::offered(model)),
        }
    }
    // What another address offered and nobody wants goes.
    rows.retain(|row| row.saved || row.offered || row.ticked);
    new.sort_by_key(|row| row.not_chat);
    rows.extend(new);
}

fn text(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().trim().to_owned()
}

fn tokens(input: &Entity<InputState>, what: &str, cx: &App) -> Result<Option<u64>, String> {
    let text = text(input, cx).replace(['_', ',', ' ', '.'], "");
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .map(Some)
        .ok_or_else(|| format!("{what} must be a number of tokens."))
}

impl ServerScreen {
    fn new(window: &mut Window, cx: &mut Context<ModelsDialog>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let base_url = input("http://192.168.1.20:8000/v1", window, cx);
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the key, or $NAME or !command")
        });
        let search = input("Search models", window, cx);
        let by_id = input("Model ID, as the server names it", window, cx);
        let mut subscriptions = Vec::new();
        for input in [&base_url, &key] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.connect_again(cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe_in(
            &by_id,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_by_id(window, cx);
                }
            },
        ));
        Self {
            existing: None,
            urls: Vec::new(),
            base_url,
            key,
            name: input("How it is called (optional)", window, cx),
            id: input("ollama", window, cx),
            search,
            by_id,
            api: "openai-completions".to_owned(),
            kind: Kind::Other,
            state: State::NoAddress,
            rows: Vec::new(),
            options: false,
            removing: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn title(&self, cx: &App) -> SharedString {
        if let Some(existing) = &self.existing {
            return existing.label().to_owned().into();
        }
        let url = text(&self.base_url, cx);
        server::short_address(&url).to_owned().into()
    }

    /// How many models would be added and removed.
    fn changes(&self) -> (usize, usize) {
        let count = |f: fn(&Row) -> bool| self.rows.iter().filter(|row| f(row)).count();
        (
            count(|row| row.ticked && !row.saved),
            count(|row| !row.ticked && row.saved),
        )
    }
}

impl ModelsDialog {
    pub(super) fn open_new_server(
        &mut self,
        urls: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = urls
            .iter()
            .find_map(|url| {
                self.provider_at(url)
                    .or_else(|| self.provider_at(&format!("{url}/v1")))
            })
            .cloned();
        if let Some(saved) = saved {
            self.open_saved_server(saved, window, cx);
            return;
        }
        let screen = ServerScreen::new(window, cx);
        set(&screen.base_url, &urls[0], window, cx);
        self.screen = Screen::Server(Box::new(screen));
        self.connect_to(urls, cx);
    }

    pub(super) fn open_saved_server(
        &mut self,
        saved: ProviderInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let screen = Self::saved_server(saved, window, cx);
        let url = text(&screen.base_url, cx);
        self.screen = Screen::Server(Box::new(screen));
        self.stop_asking();
        if !url.is_empty() {
            self.connect_to(vec![url], cx);
        }
        cx.notify();
    }

    fn saved_server(
        saved: ProviderInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ServerScreen {
        let mut screen = ServerScreen::new(window, cx);
        for (input, value) in [
            (&screen.base_url, saved.base_url.as_deref()),
            (&screen.key, saved.api_key.as_deref()),
            (&screen.name, saved.name.as_deref()),
            (&screen.id, Some(saved.id.as_str())),
        ] {
            set(input, value.unwrap_or_default(), window, cx);
        }
        screen.api = saved.api.clone().unwrap_or_default();
        screen.rows = saved.models.iter().map(Row::saved).collect();
        screen.existing = Some(saved);
        screen
    }

    /// Asks the server at one of `urls` what it offers.
    fn connect_to(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        let (Some(place), Screen::Server(screen)) = (self.place.clone(), &mut self.screen) else {
            return;
        };
        screen.state = State::Connecting;
        screen.error = None;
        screen.urls.clone_from(&urls);
        let key = place.key(screen.key.read(cx).value().as_ref());
        let mut own: Vec<String> = screen
            .rows
            .iter()
            .filter_map(|row| row.base_url.clone())
            .collect();
        own.sort();
        own.dedup();
        cx.notify();
        self.asked += 1;
        let asked = self.asked;
        let window = self.window;
        self.task = Some(cx.spawn(async move |this, cx| {
            let has_key = key.is_some();
            let connection = cx
                .background_executor()
                .spawn(async move {
                    let connection = probe::connect(&place.pi, &urls, key.as_deref());
                    // Models can have addresses of their own, and the provider's none behind it.
                    if matches!(connection, Connection::Offline(_))
                        && own.iter().any(|url| probe::answers(&place.pi, url))
                    {
                        Connection::Elsewhere
                    } else {
                        connection
                    }
                })
                .await;
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.asked == asked {
                        this.connected(connection, has_key, window, cx);
                    }
                });
            });
        }));
    }

    /// After the address or the key changed.
    fn connect_again(&mut self, cx: &mut Context<Self>) {
        let Screen::Server(screen) = &mut self.screen else {
            return;
        };
        match server::base_urls(&text(&screen.base_url, cx)) {
            Ok(urls) => self.connect_to(urls, cx),
            Err(error) => {
                screen.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn connected(
        &mut self,
        connection: Connection,
        has_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The server may be in the file under the address that was found.
        if let Connection::Online { base_url, .. } = &connection
            && matches!(&self.screen, Screen::Server(screen) if screen.existing.is_none())
            && let Some(saved) = self.provider_at(base_url).cloned()
        {
            self.screen = Screen::Server(Box::new(Self::saved_server(saved, window, cx)));
        }
        let taken: Vec<&str> = self
            .providers
            .iter()
            .map(|provider| provider.id.as_str())
            .collect();
        let Screen::Server(screen) = &mut self.screen else {
            return;
        };
        let new = screen.existing.is_none();
        let set_up =
            |screen: &mut ServerScreen, base_url: &str, window: &mut Window, cx: &mut App| {
                if !new {
                    return;
                }
                set(&screen.base_url, base_url, window, cx);
                let id = server::provider_id(screen.kind, base_url, &taken);
                set(&screen.id, &id, window, cx);
            };
        match connection {
            Connection::Online { base_url, offer } => {
                screen.kind = offer.kind;
                screen.state = State::Online;
                if new {
                    offer.api.clone_into(&mut screen.api);
                }
                set_up(screen, &base_url, window, cx);
                merge(&mut screen.rows, offer);
                // There is nothing to choose.
                let mut chats = screen.rows.iter_mut().filter(|row| !row.not_chat);
                if let (Some(only), None, true) = (chats.next(), chats.next(), new) {
                    only.ticked = true;
                }
            }
            Connection::Elsewhere => screen.state = State::Elsewhere,
            Connection::NeedsKey { base_url } => {
                screen.state = State::NeedsKey { refused: has_key };
                set_up(screen, &base_url, window, cx);
                screen.key.update(cx, |key, cx| key.focus(window, cx));
            }
            Connection::NoList { base_url, message } => {
                screen.state = State::NoList(message.into());
                set_up(screen, &base_url, window, cx);
            }
            Connection::Offline(message) => {
                screen.state = State::Offline(message.into());
                let url = server::attempts(&screen.urls[0])
                    .first()
                    .map_or_else(|| screen.urls[0].clone(), |first| first.base_url.clone());
                set_up(screen, &url, window, cx);
            }
        }
        cx.notify();
    }

    /// Adds a model the server does not list.
    fn add_by_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Server(screen) = &mut self.screen else {
            return;
        };
        let id = text(&screen.by_id, cx);
        if id.is_empty() {
            return;
        }
        let ix = match screen.rows.iter().position(|row| row.spec.id == id) {
            Some(ix) => ix,
            None => {
                screen.rows.insert(
                    0,
                    Row::new(ModelSpec {
                        id,
                        ..ModelSpec::default()
                    }),
                );
                0
            }
        };
        screen.rows[ix].ticked = true;
        set(&screen.by_id, "", window, cx);
        set(&screen.search, "", window, cx);
        if !screen.rows[ix].open {
            self.toggle_open(ix, window, cx);
        }
        cx.notify();
    }

    /// Shows or hides the fields of a model.
    fn toggle_open(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Server(screen) = &mut self.screen else {
            return;
        };
        let Some(row) = screen.rows.get_mut(ix) else {
            return;
        };
        row.open = !row.open;
        if row.fields.is_none() {
            let number = |value: Option<u64>| value.map_or_default(grouped);
            let input = |placeholder: &str, value: String, window: &mut Window, cx: &mut App| {
                let placeholder = placeholder.to_owned();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
            let fields = [
                input(&row.spec.id, row.spec.name.clone(), window, cx),
                input("128,000", number(row.spec.context_window), window, cx),
                input("16,384", number(row.spec.max_tokens), window, cx),
            ];
            let enter = fields
                .iter()
                .map(|input| {
                    cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            this.save_server(cx);
                        }
                    })
                })
                .collect();
            let [name, context_window, max_tokens] = fields;
            row.fields = Some(Fields {
                name,
                context_window,
                max_tokens,
                _enter: enter,
            });
        }
        cx.notify();
    }

    fn set_row(&mut self, ix: usize, change: impl FnOnce(&mut Row), cx: &mut Context<Self>) {
        if let Screen::Server(screen) = &mut self.screen
            && let Some(row) = screen.rows.get_mut(ix)
        {
            change(row);
            screen.error = None;
            cx.notify();
        }
    }

    /// Has the ticked models say something, one after the other.
    fn try_models(&mut self, cx: &mut Context<Self>) {
        let (Some(place), Screen::Server(screen)) = (self.place.clone(), &mut self.screen) else {
            return;
        };
        let api = if screen.api.is_empty() {
            "openai-completions".to_owned()
        } else {
            screen.api.clone()
        };
        let base_url = text(&screen.base_url, cx);
        let key = place.key(screen.key.read(cx).value().as_ref());
        let models: Vec<(String, String)> = screen
            .rows
            .iter_mut()
            .filter(|row| row.ticked)
            .map(|row| {
                row.trial = Trial::Running;
                let url = row.base_url.clone().unwrap_or_else(|| base_url.clone());
                (row.spec.id.clone(), url)
            })
            .collect();
        cx.notify();
        self.asked += 1;
        let asked = self.asked;
        self.task = Some(cx.spawn(async move |this, cx| {
            for (id, url) in models {
                let (pi, api, key, model) =
                    (place.pi.clone(), api.clone(), key.clone(), id.clone());
                let result = cx
                    .background_executor()
                    .spawn(async move { probe::try_model(&pi, &api, &url, key.as_deref(), &model) })
                    .await;
                let going_on = this.update(cx, |this, cx| {
                    let Screen::Server(screen) = &mut this.screen else {
                        return false;
                    };
                    if this.asked != asked {
                        return false;
                    }
                    if let Some(row) = screen.rows.iter_mut().find(|row| row.spec.id == id) {
                        row.trial = match result {
                            Ok(time) => Trial::Worked(time),
                            Err(error) => Trial::Failed(error.into()),
                        };
                    }
                    cx.notify();
                    true
                });
                if !matches!(going_on, Ok(true)) {
                    return;
                }
            }
        }));
    }

    pub(super) fn save_server(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let (Some(Ok(file)), Screen::Server(screen)) = (&self.loaded, &self.screen) else {
            return;
        };
        let mut file = file.clone();
        let result = (|| {
            let mut spec = ProviderSpec {
                name: text(&screen.name, cx),
                base_url: text(&screen.base_url, cx),
                api: screen.api.clone(),
                api_key: text(&screen.key, cx),
            };
            let had_key = screen
                .existing
                .as_ref()
                .is_none_or(|existing| existing.api_key.is_some());
            if spec.api_key.is_empty() && had_key && !spec.base_url.is_empty() {
                NO_KEY.clone_into(&mut spec.api_key);
            }
            let id = if let Some(existing) = &screen.existing {
                file.set_provider(&existing.id, &spec)
                    .map_err(|error| error.to_string())?;
                existing.id.clone()
            } else {
                let id = text(&screen.id, cx);
                if !screen.rows.iter().any(|row| row.ticked) {
                    return Err("Tick the models to add.".to_owned());
                }
                file.add_provider(&id, &spec)
                    .map_err(|error| error.to_string())?;
                id
            };
            let mut added = None;
            for row in &screen.rows {
                if row.ticked {
                    file.set_model(&id, &row.spec(cx)?)
                        .map_err(|error| error.to_string())?;
                    if !row.saved && added.is_none() {
                        added = Some((id.clone(), row.spec.id.clone()));
                    }
                } else if row.saved {
                    file.remove_model(&id, &row.spec.id);
                }
            }
            Ok((file.to_text(), added))
        })();
        match result {
            Ok((text, select)) => self.save(text, select, None, cx),
            Err(error) => {
                self.set_error(Some(error.into()));
                cx.notify();
            }
        }
    }

    /// Removes the provider and its models; asks first.
    fn remove_server(&mut self, cx: &mut Context<Self>) {
        let (Some(Ok(file)), Screen::Server(screen)) = (&self.loaded, &mut self.screen) else {
            return;
        };
        let Some(existing) = &screen.existing else {
            return;
        };
        if !screen.removing {
            screen.removing = true;
            cx.notify();
            return;
        }
        let mut file = file.clone();
        file.remove_provider(&existing.id);
        let note = format!("Removed {}.", existing.label());
        self.save(file.to_text(), None, Some(note.into()), cx);
    }

    pub(super) fn render_server(
        &self,
        screen: &ServerScreen,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = render_state(screen, cx);
        let needs_key = matches!(screen.state, State::NeedsKey { .. });
        let problem = match &screen.state {
            State::NeedsKey { refused: false } => Some("This server wants an API key.".into()),
            State::NeedsKey { refused: true } => Some("The server did not accept this key.".into()),
            State::NoList(message) => Some(SharedString::from(format!(
                "It does not say which models it has. {message} If they are there all the same, \
                 add them by their ID."
            ))),
            State::Offline(message) => Some(message.clone()),
            State::NoAddress | State::Connecting | State::Online | State::Elsewhere => None,
        };
        let warning = crate::theme::palette(cx).warning;
        let muted = cx.theme().muted_foreground;
        let can_retry = matches!(screen.state, State::Offline(_) | State::NoList(_));
        let problem = problem.map(|problem| {
            h_flex()
                .gap_2()
                .items_start()
                .text_xs()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(warning),
                )
                .child(div().flex_1().min_w_0().child(problem))
                .when(can_retry, |this| {
                    this.child(
                        div().debug_selector(|| "retry".into()).child(
                            Button::new("retry")
                                .quiet(cx)
                                .xsmall()
                                .icon(IconName::RefreshCw)
                                .label("Try again")
                                .on_click(cx.listener(|this, _, _, cx| this.connect_again(cx))),
                        ),
                    )
                })
        });
        let (added, removed) = screen.changes();
        let any = screen.rows.iter().any(|row| row.ticked);
        let action = if self.saving {
            "Saving…".to_owned()
        } else if screen.existing.is_some() && (added == 0 || removed > 0) {
            "Save".to_owned()
        } else if added == 0 {
            "Add models".to_owned()
        } else {
            format!("Add {}", count(added, "model"))
        };
        let running = screen.rows.iter().any(|row| row.trial == Trial::Running);
        let can_try = any && !matches!(screen.state, State::NoAddress | State::Connecting);
        v_flex()
            .gap_3()
            .child(Self::render_top(screen.title(cx), Some(status), cx))
            .children(problem)
            .when(needs_key, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(Input::new(&screen.key).mask_toggle().aria_label("API key"))
                        .child(
                            div().flex_none().debug_selector(|| "use-key".into()).child(
                                Button::new("use-key")
                                    .label("Connect")
                                    .on_click(cx.listener(|this, _, _, cx| this.connect_again(cx))),
                            ),
                        ),
                )
            })
            .child(self.render_models(screen, cx))
            .when(screen.options, |this| {
                this.child(self.render_options(screen, !needs_key, cx))
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div().debug_selector(|| "connection-options".into()).child(
                            Button::new("connection-options")
                                .quiet(cx)
                                .small()
                                .icon(
                                    Icon::new(if screen.options {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .text_color(muted),
                                )
                                .label("Connection")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Screen::Server(screen) = &mut this.screen {
                                        screen.options = !screen.options;
                                        screen.removing = false;
                                        cx.notify();
                                    }
                                })),
                        ),
                    )
                    .child(
                        bottom(screen.error.as_ref(), cx)
                            .when(can_try, |this| {
                                this.child(
                                    div().debug_selector(|| "try-models".into()).child(
                                        Button::new("try-models")
                                            .label(if running { "Testing…" } else { "Test" })
                                            .disabled(running)
                                            .tooltip("Has the ticked models answer a short message")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.try_models(cx)),
                                            ),
                                    ),
                                )
                            })
                            .child(
                                div().debug_selector(|| "save-server".into()).child(
                                    Button::new("save-server")
                                        .primary()
                                        .label(action)
                                        .disabled(
                                            self.saving || (screen.existing.is_none() && !any),
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.save_server(cx)),
                                        ),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_models(&self, screen: &ServerScreen, cx: &mut Context<Self>) -> Div {
        let search = text(&screen.search, cx);
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let matching: Vec<usize> = (0..screen.rows.len())
            .filter(|ix| screen.rows[*ix].matches(&search))
            .collect();
        let more = matching.len().saturating_sub(MOST_ROWS);
        let none = match screen.state {
            State::Connecting => "Asking the server for its models…",
            _ if !search.is_empty() => "No model has that in its name.",
            State::Online => "The server has no models yet.",
            _ => "No models yet.",
        };
        let note = match screen.kind {
            Kind::Ollama => {
                Some("Ollama may use less: the context length in its own settings decides.")
            }
            _ => None,
        };
        v_flex()
            .gap_2()
            .when(screen.rows.len() > 8, |this| {
                this.child(
                    Input::new(&screen.search)
                        .small()
                        .prefix(Icon::new(IconName::Search).small().text_color(muted))
                        .cleanable(true)
                        .aria_label("Search models"),
                )
            })
            .child(
                div()
                    .id("server-models")
                    .max_h(px(if screen.options { 200. } else { 290. }))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .when(matching.is_empty(), |this| {
                        this.child(div().p_3().child(label(none, cx)))
                    })
                    .children(
                        matching
                            .iter()
                            .take(MOST_ROWS)
                            .enumerate()
                            .map(|(nth, ix)| {
                                div()
                                    .when(nth > 0, |this| this.border_t_1().border_color(border))
                                    .child(render_row(*ix, screen, cx))
                            }),
                    )
                    .when(more > 0, |this| {
                        this.child(
                            div()
                                .p_3()
                                .child(label(format!("{more} more. Search to find them."), cx)),
                        )
                    }),
            )
            .children(note.map(|note| label(note, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Input::new(&screen.by_id)
                            .small()
                            .aria_label("Add a model by its ID"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .debug_selector(|| "add-by-id".into())
                            .child(
                                Button::new("add-by-id")
                                    .small()
                                    .label("Add by ID")
                                    .tooltip("For a model that is not in the list")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_by_id(window, cx);
                                    })),
                            ),
                    ),
            )
    }

    /// How pi connects to the server.
    fn render_options(&self, screen: &ServerScreen, with_key: bool, cx: &mut Context<Self>) -> Div {
        let weak = cx.weak_entity();
        let current = screen.api.clone();
        let api_label = pi_config::APIS
            .iter()
            .find(|(api, _)| *api == screen.api)
            .map_or_else(
                || {
                    if screen.api.is_empty() {
                        "pi's own".to_owned()
                    } else {
                        screen.api.clone()
                    }
                },
                |(_, name)| (*name).to_owned(),
            );
        let api = Button::new("api-choice")
            .label(api_label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                pi_config::APIS.iter().fold(menu, |menu, (api, name)| {
                    let weak = weak.clone();
                    menu.item(
                        described_item(format!("api:{api}"), *name, *api)
                            .checked(*api == current)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if let Screen::Server(screen) = &mut this.screen {
                                        (*api).clone_into(&mut screen.api);
                                        cx.notify();
                                    }
                                });
                            }),
                    )
                })
            });
        let id = match &screen.existing {
            // pi's sessions know it by its ID.
            Some(existing) => v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(label("ID", cx))
                .child(
                    h_flex()
                        .h(px(32.))
                        .text_sm()
                        .child(FadingText::new(existing.id.clone())),
                ),
            None => field("ID", &screen.id, cx),
        };
        let remove = screen.existing.as_ref().map(|existing| {
            let models = existing.models.len();
            h_flex()
                .gap_2()
                .when(screen.removing, |this| {
                    this.child(div().text_xs().child(format!(
                        "Remove {} and {} from pi?",
                        existing.label(),
                        count(models, "model")
                    )))
                })
                .child(
                    div().debug_selector(|| "remove-server".into()).child(
                        Button::new("remove-server")
                            .form_danger(cx)
                            .label(if screen.removing {
                                "Remove"
                            } else {
                                "Remove connection…"
                            })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.remove_server(cx))),
                    ),
                )
                .when(screen.removing, |this| {
                    this.child(
                        Button::new("keep-server")
                            .form_other(cx)
                            .label("Keep")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Screen::Server(screen) = &mut this.screen {
                                    screen.removing = false;
                                    cx.notify();
                                }
                            })),
                    )
                })
        });
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(field("Address", &screen.base_url, cx))
                    .child(
                        v_flex()
                            .flex_none()
                            .gap_1()
                            .child(label("API", cx))
                            .child(api),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(field("Name", &screen.name, cx))
                    .child(id),
            )
            .when(with_key, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(label("API key", cx))
                        .child(Input::new(&screen.key).mask_toggle().aria_label("API key")),
                )
            })
            .children(remove)
    }
}

fn set(input: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut App) {
    let text = text.to_owned();
    input.update(cx, |input, cx| input.set_value(text, window, cx));
}

/// How the connection is, for the top of the screen.
fn render_state(screen: &ServerScreen, cx: &App) -> AnyElement {
    let palette = crate::theme::palette(cx);
    let muted = cx.theme().muted_foreground;
    let (color, text): (Hsla, SharedString) = match &screen.state {
        State::NoAddress => return div().into_any_element(),
        State::Connecting => (muted, "Connecting…".into()),
        State::Online => (
            palette.success,
            match screen.kind.label() {
                Some(kind) => format!("Connected · {kind}").into(),
                None => "Connected".into(),
            },
        ),
        State::Elsewhere => (palette.success, "Connected".into()),
        State::NeedsKey { .. } => (palette.warning, "Needs a key".into()),
        State::NoList(_) => (palette.warning, "Answers".into()),
        State::Offline(_) => (palette.error, "Offline".into()),
    };
    h_flex()
        .flex_none()
        .gap_1p5()
        .text_xs()
        .text_color(muted)
        .child(light(color))
        .child(text)
        .into_any_element()
}

fn render_row(ix: usize, screen: &ServerScreen, cx: &mut Context<ModelsDialog>) -> Div {
    let row = &screen.rows[ix];
    let palette = crate::theme::palette(cx);
    let muted = cx.theme().muted_foreground;
    let mut facts: Vec<String> = Vec::new();
    if !row.spec.name.is_empty() {
        facts.push(row.spec.id.clone());
    }
    if let Some(context) = row.spec.context_window {
        facts.push(format!("{} context", grouped(context)));
    }
    if row.not_chat {
        facts.push("not for conversations".to_owned());
    }
    if let Some(url) = &row.base_url {
        facts.push(server::short_address(url).to_owned());
    } else if row.saved && !row.offered && matches!(screen.state, State::Online) {
        facts.push("the server does not list it".to_owned());
    }
    let badge = |text: &'static str| {
        div()
            .flex_none()
            .px_1p5()
            .rounded_sm()
            .bg(cx.theme().muted)
            .text_xs()
            .text_color(muted)
            .child(text)
    };
    let change = if row.ticked == row.saved {
        None
    } else if row.ticked {
        Some(("New", palette.success))
    } else {
        Some(("Will be removed", palette.error))
    };
    let trial = match &row.trial {
        Trial::None => None,
        Trial::Running => Some((muted, SharedString::from("Waiting for an answer…"))),
        Trial::Worked(time) => Some((
            palette.success,
            format!("Works: it answered in {:.1} s", time.as_secs_f32()).into(),
        )),
        Trial::Failed(error) => Some((palette.error, error.clone())),
    };
    let id = row.spec.id.clone();
    let ticked = row.ticked;
    let title = h_flex()
        .id(("model-row", ix))
        .flex_1()
        .min_w_0()
        .gap_2()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.set_row(ix, |row| row.ticked = !ticked, cx);
        }))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .when(row.not_chat, |this| this.text_color(muted))
                        .child(FadingText::new(row.label().to_owned())),
                )
                .when(!facts.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(FadingText::new(facts.join(" · "))),
                    )
                })
                .children(trial.map(|(color, text)| div().text_xs().text_color(color).child(text))),
        )
        .children(
            change.map(|(text, color)| div().flex_none().text_xs().text_color(color).child(text)),
        )
        .when(row.spec.reasoning, |this| this.child(badge("Thinking")))
        .when(row.spec.images, |this| this.child(badge("Pictures")));
    v_flex()
        .child(
            h_flex()
                .px_2p5()
                .py_1p5()
                .gap_2p5()
                .child(
                    div().debug_selector(|| format!("tick:{id}")).child(
                        Checkbox::new(SharedString::from(format!("tick:{id}")))
                            .checked(ticked)
                            .accessibility_label(id.clone())
                            .on_click(cx.listener(move |this, ticked: &bool, _, cx| {
                                let ticked = *ticked;
                                this.set_row(ix, |row| row.ticked = ticked, cx);
                            })),
                    ),
                )
                .child(title)
                .child(
                    div()
                        .flex_none()
                        .debug_selector(|| format!("details:{id}"))
                        .child(
                            Button::new(SharedString::from(format!("details:{id}")))
                                .quiet(cx)
                                .xsmall()
                                .icon(
                                    Icon::new(if row.open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .text_color(muted),
                                )
                                .accessibility_label(format!("Details of {id}"))
                                .tooltip("Name, context and what it can do")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_open(ix, window, cx);
                                })),
                        ),
                ),
        )
        .children(
            row.fields
                .as_ref()
                .filter(|_| row.open)
                .map(|fields| render_fields(ix, row, fields, cx)),
        )
}

fn render_fields(ix: usize, row: &Row, fields: &Fields, cx: &mut Context<ModelsDialog>) -> Div {
    v_flex()
        .pl_9()
        .pr_2p5()
        .pb_2p5()
        .gap_2()
        .child(field("Name in the model menu", &fields.name, cx))
        .child(
            h_flex()
                .gap_2()
                .child(field("Context, in tokens", &fields.context_window, cx))
                .child(field("Longest answer, in tokens", &fields.max_tokens, cx)),
        )
        .child(
            h_flex()
                .gap_4()
                .child(
                    Checkbox::new(("thinks", ix))
                        .small()
                        .checked(row.spec.reasoning)
                        .label("Thinks before it answers")
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let checked = *checked;
                            this.set_row(ix, |row| row.spec.reasoning = checked, cx);
                        })),
                )
                .child(
                    Checkbox::new(("pictures", ix))
                        .small()
                        .checked(row.spec.images)
                        .label("Sees pictures")
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let checked = *checked;
                            this.set_row(ix, |row| row.spec.images = checked, cx);
                        })),
                ),
        )
}
