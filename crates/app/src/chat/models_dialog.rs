//! "Add model": adds a custom model to pi's `models.json`, with a form (provider presets,
//! fetching the server's model list) or by editing the file itself.
//!
//! The file is the one the session's pi reads: in pi's agent folder on this computer, or on
//! the SSH host for a remote session. pi only reads it when it starts, so after saving the
//! window restarts pi on the same session and selects the new model.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::{ModelSpec, ModelsFile, NewProvider, ProviderInfo};
use remote::{HostId, Ssh};

use crate::ui::QuietButton as _;

#[derive(Debug, Clone)]
pub enum ModelsDialogEvent {
    /// `models.json` was saved; restart pi and select `select` (`(provider, model id)`).
    Saved { select: Option<(String, String)> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsTab {
    Form,
    Json,
}

/// Where the file lives.
#[derive(Clone)]
enum Place {
    Local(PathBuf),
    Remote {
        label: String,
        ssh: Arc<Ssh>,
        path: String,
    },
}

impl Place {
    fn describe(&self, home: Option<&std::path::Path>) -> String {
        match self {
            Self::Local(path) => crate::ui::short_path(path, home),
            Self::Remote { label, path, .. } => format!("{label}:{path}"),
        }
    }

    fn read(&self) -> Result<String, String> {
        let bytes = match self {
            Self::Local(path) => match std::fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
            },
            Self::Remote { ssh, path, .. } => ssh.read_file(path).map_err(|e| e.to_string())?,
        };
        Ok(bytes
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default())
    }

    /// Writes atomically, keeping the previous file as `models.json.bak-pi-gui`.
    fn write(&self, text: &str) -> Result<(), String> {
        match self {
            Self::Local(path) => {
                let fail =
                    |error: std::io::Error| format!("Could not save {}: {error}", path.display());
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(fail)?;
                }
                if path.is_file() {
                    std::fs::copy(path, path.with_extension("json.bak-pi-gui")).map_err(fail)?;
                }
                let tmp = path.with_extension("json.pi-gui-tmp");
                std::fs::write(&tmp, text).map_err(fail)?;
                std::fs::rename(&tmp, path).map_err(fail)
            }
            Self::Remote { ssh, path, .. } => ssh
                .upload(path, text.as_bytes().to_vec())
                .map_err(|error| error.to_string()),
        }
    }

    /// GETs `url` with curl where pi runs (so `localhost` means the same machine pi uses);
    /// the API key goes through stdin, never on a command line.
    fn fetch(&self, url: &str, key: Option<&str>) -> Result<String, String> {
        let header = key
            .map(|key| format!("Authorization: Bearer {key}\n"))
            .unwrap_or_default();
        let stdout = match self {
            Self::Local(_) => {
                use std::io::Write as _;
                let mut child = std::process::Command::new("curl")
                    .args(["-sS", "-m", "8", "-H", "@-", url])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|error| format!("Could not run curl: {error}"))?;
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(header.as_bytes());
                }
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(stderr.trim().trim_start_matches("curl: ").to_owned());
                }
                output.stdout
            }
            Self::Remote { ssh, .. } => {
                let script = format!(
                    "command -v curl >/dev/null 2>&1 || {{ echo 'curl is not installed on the host' >&2; exit 1; }}\n\
                     curl -sS -m 8 -H @- {}",
                    remote::sh_quote(url)
                );
                ssh.exec_with_input(&script, header.into_bytes())
                    .map_err(|error| error.to_string().trim_start_matches("curl: ").to_owned())?
            }
        };
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderChoice {
    Existing(String),
    New,
}

enum Fetched {
    None,
    Loading,
    Ready(Vec<String>),
    Failed(SharedString),
}

#[allow(clippy::struct_excessive_bools)] // independent form flags
pub struct ModelsDialog {
    place: Option<Place>,
    tab: ModelsTab,
    loaded: Option<Result<ModelsFile, SharedString>>,
    providers: Vec<ProviderInfo>,
    choice: ProviderChoice,
    /// Keep "New provider" selected when the file loads.
    prefer_new: bool,
    provider_id: Entity<InputState>,
    provider_name: Entity<InputState>,
    base_url: Entity<InputState>,
    api_key: Entity<InputState>,
    api: &'static str,
    model_id: Entity<InputState>,
    model_name: Entity<InputState>,
    context_window: Entity<InputState>,
    max_tokens: Entity<InputState>,
    reasoning: bool,
    images: bool,
    fetched: Fetched,
    form_error: Option<SharedString>,
    editor: Entity<EditorState>,
    json_error: Option<SharedString>,
    saving: bool,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<ModelsDialogEvent> for ModelsDialog {}

impl ModelsDialog {
    /// The dialog for the `models.json` that pi on `host` (or this computer) reads.
    pub fn new(
        host: Option<HostId>,
        tab: ModelsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let place = match host {
            None => pi_sessions::agent_dir().map(|agent| Place::Local(agent.join("models.json"))),
            Some(host) => {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                match (hosts.ssh(&host), hosts.probe(&host)) {
                    (Some(ssh), Some(probe)) => Some(Place::Remote {
                        label: hosts.label(&host),
                        ssh,
                        path: format!("{}/models.json", probe.agent_dir),
                    }),
                    _ => None,
                }
            }
        };
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let mut this = Self {
            tab,
            loaded: None,
            providers: Vec::new(),
            choice: ProviderChoice::New,
            prefer_new: false,
            provider_id: input("e.g. ollama", window, cx),
            provider_name: input("Display name (optional)", window, cx),
            base_url: input("http://localhost:11434/v1", window, cx),
            api_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Key, $ENV_VAR or !command (optional)")
                    .masked(true)
            }),
            api: "openai-completions",
            model_id: input("e.g. qwen2.5-coder:7b", window, cx),
            model_name: input("Display name (optional)", window, cx),
            context_window: input("128000", window, cx),
            max_tokens: input("16384", window, cx),
            reasoning: false,
            images: false,
            fetched: Fetched::None,
            form_error: None,
            editor: cx.new(|cx| EditorState::new(window, cx).language("json")),
            json_error: None,
            saving: false,
            _tasks: Vec::new(),
            place,
        };
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            self.loaded = Some(Err(
                "Connect to the host first: pi's settings there are not known yet.".into(),
            ));
            return;
        };
        self.loaded = None;
        let task = cx.spawn(async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move { place.read() })
                .await;
            let _ = this.update(cx, |this, cx| {
                match text {
                    Ok(text) => {
                        this.set_editor_text(text.clone(), cx);
                        match ModelsFile::parse(&text) {
                            Ok(file) => this.file_loaded(file),
                            Err(error) => {
                                // Still editable as text.
                                this.loaded = Some(Err(format!(
                                    "models.json has a problem; fix it in the models.json tab. {error}"
                                )
                                .into()));
                                this.tab = ModelsTab::Json;
                                this.json_error = Some(error.to_string().into());
                            }
                        }
                    }
                    Err(error) => this.loaded = Some(Err(error.into())),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Starts on "New provider" instead of the file's first provider.
    pub fn start_with_new_provider(&mut self, cx: &mut Context<Self>) {
        self.prefer_new = true;
        self.choice = ProviderChoice::New;
        cx.notify();
    }

    fn file_loaded(&mut self, file: ModelsFile) {
        self.providers = file.providers();
        // Default to the first provider the file has, else a new one.
        if let ProviderChoice::Existing(id) = &self.choice
            && !self.providers.iter().any(|provider| &provider.id == id)
        {
            self.choice = ProviderChoice::New;
        }
        if self.choice == ProviderChoice::New
            && !self.prefer_new
            && let Some(first) = self.providers.first()
        {
            self.choice = ProviderChoice::Existing(first.id.clone());
        }
        self.loaded = Some(Ok(file));
    }

    fn set_editor_text(&mut self, text: String, cx: &mut Context<Self>) {
        let editor = self.editor.clone();
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| {
                    editor.update(cx, |editor, cx| editor.set_value(text, window, cx));
                });
            }
        });
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn set_input(input: &Entity<InputState>, text: &str, window: &mut Window, cx: &mut App) {
        let text = text.to_owned();
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }

    fn apply_preset(
        &mut self,
        preset: pi_config::Preset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choice = ProviderChoice::New;
        Self::set_input(&self.provider_id, preset.provider_id, window, cx);
        Self::set_input(&self.provider_name, preset.provider_name, window, cx);
        Self::set_input(&self.base_url, preset.base_url, window, cx);
        Self::set_input(&self.api_key, preset.api_key, window, cx);
        self.api = preset.api;
        self.fetched = Fetched::None;
        self.form_error = None;
        cx.notify();
    }

    /// Base URL, API and key of the chosen provider (for fetching its model list).
    fn endpoint(&self, cx: &App) -> Option<(String, String, Option<String>)> {
        match &self.choice {
            ProviderChoice::New => {
                let url = Self::value(&self.base_url, cx);
                let key = Self::value(&self.api_key, cx);
                (!url.is_empty()).then(|| (url, self.api.to_owned(), Some(key)))
            }
            ProviderChoice::Existing(id) => {
                let provider = self.providers.iter().find(|provider| &provider.id == id)?;
                Some((
                    provider.base_url.clone()?,
                    provider
                        .api
                        .clone()
                        .unwrap_or_else(|| "openai-completions".to_owned()),
                    provider.api_key.clone(),
                ))
            }
        }
    }

    fn fetch_models(&mut self, cx: &mut Context<Self>) {
        let (Some(place), Some((url, api, key))) = (self.place.clone(), self.endpoint(cx)) else {
            self.fetched = Fetched::Failed("Enter the server's base URL first.".into());
            cx.notify();
            return;
        };
        if !api.starts_with("openai") {
            self.fetched =
                Fetched::Failed("Listing models works for OpenAI-compatible servers only.".into());
            cx.notify();
            return;
        }
        self.fetched = Fetched::Loading;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let key = key.as_deref().and_then(pi_config::literal_key);
                    let body = place.fetch(&pi_config::models_url(&url), key)?;
                    pi_config::parse_model_list(&body).map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.fetched = match result {
                    Ok(ids) if ids.is_empty() => {
                        Fetched::Failed("The server lists no models.".into())
                    }
                    Ok(ids) => Fetched::Ready(ids),
                    Err(error) => Fetched::Failed(error.into()),
                };
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn number(input: &Entity<InputState>, label: &str, cx: &App) -> Result<Option<u64>, String> {
        let text = Self::value(input, cx).replace(['_', ',', ' '], "");
        if text.is_empty() {
            return Ok(None);
        }
        text.parse::<u64>()
            .map(Some)
            .map_err(|_| format!("{label} must be a whole number of tokens."))
    }

    fn add_model(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(file)) = &self.loaded else { return };
        let mut file = file.clone();
        let result = (|| {
            let model = ModelSpec {
                id: Self::value(&self.model_id, cx),
                name: Self::value(&self.model_name, cx),
                context_window: Self::number(&self.context_window, "Context window", cx)?,
                max_tokens: Self::number(&self.max_tokens, "Max output tokens", cx)?,
                reasoning: self.reasoning,
                images: self.images,
            };
            let (provider_id, new) = match &self.choice {
                ProviderChoice::Existing(id) => (id.clone(), None),
                ProviderChoice::New => {
                    let new = NewProvider {
                        id: Self::value(&self.provider_id, cx),
                        name: Self::value(&self.provider_name, cx),
                        base_url: Self::value(&self.base_url, cx),
                        api: self.api.to_owned(),
                        api_key: Self::value(&self.api_key, cx),
                    };
                    (new.id.clone(), Some(new))
                }
            };
            file.add_model(&provider_id, new.as_ref(), &model)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((file.to_text(), (provider_id, model.id)))
        })();
        match result {
            Ok((text, select)) => self.save(text, Some(select), cx),
            Err(error) => {
                self.form_error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn save_json(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match ModelsFile::parse(&text) {
            Ok(_) => {
                let text = if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                };
                self.save(text, None, cx);
            }
            Err(error) => {
                self.json_error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn save(&mut self, text: String, select: Option<(String, String)>, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        self.saving = true;
        self.form_error = None;
        self.json_error = None;
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { place.write(&text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => cx.emit(ModelsDialogEvent::Saved { select }),
                    Err(error) => {
                        match this.tab {
                            ModelsTab::Form => this.form_error = Some(error.into()),
                            ModelsTab::Json => this.json_error = Some(error.into()),
                        }
                        cx.notify();
                    }
                }
            });
        });
        self._tasks.push(task);
    }

    fn render_provider_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let label = match &self.choice {
            ProviderChoice::New => "New provider".to_owned(),
            ProviderChoice::Existing(id) => self
                .providers
                .iter()
                .find(|provider| &provider.id == id)
                .map_or_else(|| id.clone(), |provider| provider.label().to_owned()),
        };
        let providers = self.providers.clone();
        let weak = cx.weak_entity();
        let current = self.choice.clone();
        Button::new("provider-choice")
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for provider in &providers {
                    let weak = weak.clone();
                    let id = provider.id.clone();
                    let detail = provider
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "built-in".into());
                    menu = menu.item(
                        PopupMenuItem::new(format!("{}  ·  {detail}", provider.label()))
                            .checked(current == ProviderChoice::Existing(id.clone()))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.choice = ProviderChoice::Existing(id.clone());
                                    this.fetched = Fetched::None;
                                    cx.notify();
                                });
                            }),
                    );
                }
                if !providers.is_empty() {
                    menu = menu.separator();
                }
                let weak = weak.clone();
                menu.item(
                    PopupMenuItem::new("New provider…")
                        .icon(IconName::Plus)
                        .checked(current == ProviderChoice::New)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.choice = ProviderChoice::New;
                                this.fetched = Fetched::None;
                                cx.notify();
                            });
                        }),
                )
            })
    }

    fn render_api_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let weak = cx.weak_entity();
        let current = self.api;
        Button::new("api-choice")
            .small()
            .label(current)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (api, description) in pi_config::APIS {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{api}  ·  {description}"))
                            .checked(*api == current)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.api = api;
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let error_color = crate::theme::palette(cx).error;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Input::new(input).small())
        };
        let section = |title: &'static str| {
            div()
                .pt_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(title)
        };
        let new_provider = self.choice == ProviderChoice::New;
        let existing = match &self.choice {
            ProviderChoice::Existing(id) => self.providers.iter().find(|p| &p.id == id).cloned(),
            ProviderChoice::New => None,
        };

        let mut provider = v_flex().gap_2().child(
            h_flex()
                .gap_2()
                .child(section("Provider"))
                .child(div().flex_1())
                .child(self.render_provider_menu(cx)),
        );
        if new_provider {
            provider = provider
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .child(div().text_xs().text_color(muted).child("Start from:"))
                        .children(pi_config::PRESETS.iter().enumerate().map(|(ix, preset)| {
                            let preset = *preset;
                            Button::new(("preset", ix))
                                .quiet(cx)
                                .xsmall()
                                .label(preset.label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_preset(preset, window, cx);
                                }))
                        })),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(field("Provider ID", &self.provider_id))
                        .child(field("Name", &self.provider_name)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_xs().text_color(muted).child("API"))
                        .child(self.render_api_menu(cx)),
                )
                .child(field("Base URL", &self.base_url))
                .child(field("API key", &self.api_key))
                .child(div().text_xs().text_color(muted).child(
                    "Local servers usually accept any key. $NAME reads an environment variable \
                     and !command runs a command when pi needs the key.",
                ));
        } else if let Some(existing) = existing {
            provider = provider.child(div().text_xs().text_color(muted).child(format!(
                "{} · {} · {} model{} already",
                existing.base_url.as_deref().unwrap_or("built-in endpoint"),
                existing.api.as_deref().unwrap_or("default API"),
                existing.model_ids.len(),
                if existing.model_ids.len() == 1 {
                    ""
                } else {
                    "s"
                },
            )));
        }

        let fetched: AnyElement = match &self.fetched {
            Fetched::None => div().into_any_element(),
            Fetched::Loading => div()
                .text_xs()
                .text_color(muted)
                .child("Asking the server…")
                .into_any_element(),
            Fetched::Failed(error) => div()
                .text_xs()
                .text_color(error_color)
                .child(error.clone())
                .into_any_element(),
            Fetched::Ready(ids) => div()
                .id("fetched-models")
                .max_h(px(96.))
                .overflow_y_scroll()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(ids.iter().enumerate().map(|(ix, id)| {
                            let id = id.clone();
                            Button::new(("fetched", ix))
                                .quiet(cx)
                                .xsmall()
                                .label(id.clone())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    Self::set_input(&this.model_id, &id, window, cx);
                                    cx.notify();
                                }))
                        })),
                )
                .into_any_element(),
        };
        let (reasoning, images) = (self.reasoning, self.images);
        let model = v_flex()
            .gap_2()
            .child(section("Model"))
            .child(
                h_flex()
                    .items_end()
                    .gap_2()
                    .child(field("Model ID (as the server names it)", &self.model_id))
                    .child(
                        Button::new("fetch-models")
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("List server models")
                            .tooltip(
                                "Ask the server which models it offers (OpenAI-compatible servers)",
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.fetch_models(cx))),
                    ),
            )
            .child(fetched)
            .child(field("Name", &self.model_name))
            .child(
                h_flex()
                    .gap_2()
                    .child(field("Context window (tokens)", &self.context_window))
                    .child(field("Max output tokens", &self.max_tokens)),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(
                        Checkbox::new("model-reasoning")
                            .small()
                            .checked(reasoning)
                            .label("Supports thinking")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.reasoning = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("model-images")
                            .small()
                            .checked(images)
                            .label("Accepts images")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.images = *checked;
                                cx.notify();
                            })),
                    ),
            );

        v_flex()
            .gap_3()
            .child(
                div()
                    .id("model-form")
                    .max_h(px(470.))
                    .overflow_y_scroll()
                    .child(v_flex().gap_3().pr_1().child(provider).child(model)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.form_error.clone(), |this, error| {
                        this.child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(error_color)
                                .child(error),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("add-model")
                            .primary()
                            .label(if self.saving {
                                "Saving…"
                            } else {
                                "Add model"
                            })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.add_model(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_json(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let error_color = crate::theme::palette(cx).error;
        v_flex()
            .gap_2()
            .child(
                div()
                    .h(px(400.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(Editor::new(&self.editor).size_full()),
            )
            .child(div().text_xs().text_color(muted).child(
                "Format: {\"providers\": {\"<id>\": {\"baseUrl\", \"api\", \"apiKey\", \"models\": [{\"id\"}]}}}. \
                 See pi's docs/models.md. The previous file is kept as models.json.bak-pi-gui.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.json_error.clone(), |this, error| {
                        this.child(div().flex_1().text_xs().text_color(error_color).child(error))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("revert-json")
                            .quiet(cx)
                            .label("Revert")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.json_error = None;
                                this.load(cx);
                            })),
                    )
                    .child(
                        Button::new("save-json")
                            .primary()
                            .label(if self.saving { "Saving…" } else { "Save" })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_json(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ModelsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let error_color = crate::theme::palette(cx).error;
        let home = cx.global::<crate::config::AppConfig>().home.clone();
        let place = self
            .place
            .as_ref()
            .map(|place| place.describe(home.as_deref()))
            .unwrap_or_default();
        let weak = cx.weak_entity();
        let tabs = TabBar::new("models-tabs")
            .segmented()
            .small()
            .selected_index(match self.tab {
                ModelsTab::Form => 0,
                ModelsTab::Json => 1,
            })
            .on_click(move |ix: &usize, _, cx| {
                let tab = if *ix == 0 {
                    ModelsTab::Form
                } else {
                    ModelsTab::Json
                };
                let _ = weak.update(cx, |this, cx| {
                    this.tab = tab;
                    cx.notify();
                });
            })
            .child(Tab::new().label("Add a model"))
            .child(Tab::new().label("Edit models.json"));
        let body = match (&self.loaded, self.tab) {
            (None, _) => div()
                .text_sm()
                .text_color(muted)
                .child("Reading models.json…")
                .into_any_element(),
            (Some(Err(error)), ModelsTab::Form) => div()
                .text_sm()
                .text_color(error_color)
                .child(error.clone())
                .into_any_element(),
            (Some(_), ModelsTab::Json) if self.place.is_some() => self.render_json(cx),
            (Some(Err(error)), ModelsTab::Json) => div()
                .text_sm()
                .text_color(error_color)
                .child(error.clone())
                .into_any_element(),
            (Some(Ok(_)), ModelsTab::Form) => self.render_form(cx),
            (Some(Ok(_)), ModelsTab::Json) => self.render_json(cx),
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex().gap_2().child(tabs).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(place),
                ),
            )
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::Place;

    /// "List server models" against a real OpenAI-compatible server from this computer:
    /// `PI_GUI_TEST_MODELS_BASE_URL=http://host:port/v1 cargo test -p pi-gui -- --ignored lists_server_models`
    #[test]
    #[ignore = "needs PI_GUI_TEST_MODELS_BASE_URL"]
    fn lists_server_models() {
        let Ok(base) = std::env::var("PI_GUI_TEST_MODELS_BASE_URL") else {
            return;
        };
        let place = Place::Local(std::env::temp_dir().join("models.json"));
        let body = place
            .fetch(&pi_config::models_url(&base), Some("none"))
            .expect("fetch");
        let ids = pi_config::parse_model_list(&body).expect("model list");
        eprintln!("models: {ids:?}");
        assert_ne!(ids, Vec::<String>::new());
        // A server that is not there gives curl's message.
        let error = place
            .fetch("http://127.0.0.1:9/v1/models", None)
            .unwrap_err();
        eprintln!("unreachable: {error}");
        assert_ne!(error, "");
    }

    #[test]
    fn local_file_round_trip_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::Local(dir.path().join("agent/models.json"));
        assert_eq!(place.read().unwrap(), "");
        place.write("{\"a\": 1}\n").unwrap();
        place.write("{\"b\": 2}\n").unwrap();
        assert_eq!(place.read().unwrap(), "{\"b\": 2}\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("agent/models.json.bak-pi-gui")).unwrap(),
            "{\"a\": 1}\n"
        );
    }
}
