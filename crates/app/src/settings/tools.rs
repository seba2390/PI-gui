//! Settings → Tools: the tools the model gets in chats and in Code sessions on this computer,
//! as pi describes them to it, how much of the model's context they take, switches for the
//! optional ones, and the text of Pistachio's own tools to edit. pi is asked when the page is
//! first shown after Settings opens (see [`pi_config::tools`]): a pi that keeps no session,
//! launched like a conversation of each mode, reports them and quits.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::tools::{CHARS_PER_TOKEN, OPTIONAL_CODE_TOOLS, Text, Texts, Tool, parse};
use pi_rpc::ClientEvent;
use pi_rpc::protocol::ExtensionUiMethod;
use pi_sessions::Mode;

use super::AppSettings;
use crate::config::AppConfig;
use crate::theme::palette;
use crate::ui::AppIcon;

/// How long pi may take to start and report.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What pi said about one mode's tools.
#[derive(Debug, Clone, Default)]
enum Listing {
    #[default]
    Asking,
    Listed(Vec<Tool>),
    Failed(String),
}

/// The fields of a tool's text being edited.
struct Editor {
    description: Entity<TextareaState>,
    guidelines: Entity<TextareaState>,
    _subscriptions: [Subscription; 2],
}

/// The tools of both modes, asked for once per opening of Settings (and again after a switch
/// that changes them).
#[derive(Default)]
pub struct ToolCatalog {
    chat: Listing,
    code: Listing,
    /// pi was asked since Settings opened.
    asked: bool,
    /// The tools whose text for the model is shown.
    open: HashSet<(Mode, String)>,
    /// Pistachio's tools whose text is being edited.
    editing: HashSet<String>,
    /// The text written for the model, by tool (see [`pi_config::tools::Texts`]).
    texts: Texts,
    texts_path: PathBuf,
    editors: HashMap<String, Editor>,
    _chat_task: Option<Task<()>>,
    _code_task: Option<Task<()>>,
}

struct ToolCatalogHandle(Entity<ToolCatalog>);

impl Global for ToolCatalogHandle {}

pub fn tool_catalog(cx: &mut App) -> Entity<ToolCatalog> {
    if let Some(handle) = cx.try_global::<ToolCatalogHandle>() {
        return handle.0.clone();
    }
    let texts_path = cx
        .global::<AppConfig>()
        .tools_dir
        .join(pi_config::tools::TEXTS_FILE);
    let catalog = cx.new(|_| ToolCatalog {
        texts: pi_config::tools::load_texts(&texts_path),
        texts_path,
        ..ToolCatalog::default()
    });
    cx.set_global(ToolCatalogHandle(catalog.clone()));
    catalog
}

impl ToolCatalog {
    /// Settings opened: ask pi again when the page is shown (what is known stays shown).
    pub fn mark_stale(&mut self) {
        self.asked = false;
    }

    /// Asks pi, unless it was asked since Settings opened.
    fn ask(&mut self, cx: &mut Context<Self>) {
        if self.asked {
            return;
        }
        self.asked = true;
        self.refresh(Mode::Chat, cx);
        self.refresh(Mode::Code, cx);
    }

    /// Asks pi about `mode`'s tools; what is shown stays until the answer arrives.
    fn refresh(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let config = cx.global::<AppConfig>().clone();
        let code_tools = crate::chat::session::code_tools(cx);
        let task = cx.spawn(async move |this, cx| {
            config.child_env.wait().await;
            let listing = match probe(&config, mode, &code_tools, cx).await {
                Ok(tools) => Listing::Listed(tools),
                Err(error) => {
                    log::warn!("could not list pi's tools: {error}");
                    Listing::Failed(error)
                }
            };
            let _ = this.update(cx, |this, cx| {
                match mode {
                    Mode::Chat => this.chat = listing,
                    Mode::Code => this.code = listing,
                }
                cx.notify();
            });
        });
        match mode {
            Mode::Chat => self._chat_task = Some(task),
            Mode::Code => self._code_task = Some(task),
        }
    }

    /// Turns an optional Code tool on or off for sessions started from now on.
    fn set_code_tool(&mut self, name: &str, on: bool, cx: &mut Context<Self>) {
        AppSettings::update(cx, |settings| {
            settings.code_tools.retain(|tool| tool != name);
            if on {
                settings.code_tools.push(name.to_owned());
            }
        });
        // With codemode on, the other tools' descriptions say how to call them from scripts.
        self.refresh(Mode::Code, cx);
        cx.notify();
    }

    fn toggle_open(&mut self, mode: Mode, name: &str, cx: &mut Context<Self>) {
        let key = (mode, name.to_owned());
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        cx.notify();
    }

    /// The fields to edit `tool`'s text in, made the first time they are shown.
    fn editor(&mut self, tool: &Tool, window: &mut Window, cx: &mut Context<Self>) -> &Editor {
        let name = tool.name.clone();
        if !self.editors.contains_key(&name) {
            let field = |text: String, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 18)
                        .default_value(text)
                })
            };
            let description = field(tool.description.clone(), window, cx);
            let guidelines = field(tool.guidelines.join("\n"), window, cx);
            let changed = |name: String| {
                move |this: &mut Self,
                      _: Entity<TextareaState>,
                      event: &InputEvent,
                      cx: &mut Context<Self>| {
                    if let InputEvent::Change = event {
                        this.edited(&name, cx);
                    }
                }
            };
            let subscriptions = [
                cx.subscribe(&description, changed(name.clone())),
                cx.subscribe(&guidelines, changed(name.clone())),
            ];
            self.editors.insert(
                name.clone(),
                Editor {
                    description,
                    guidelines,
                    _subscriptions: subscriptions,
                },
            );
        }
        &self.editors[&name]
    }

    /// Saves what the fields of `name` say, for conversations started from now on, and counts
    /// it at once.
    fn edited(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.get(name) else {
            return;
        };
        let description = editor.description.read(cx).value().to_string();
        let guidelines = guideline_lines(&editor.guidelines.read(cx).value());
        for listing in [&mut self.chat, &mut self.code] {
            if let Listing::Listed(tools) = listing
                && let Some(tool) = tools.iter_mut().find(|tool| tool.name == name)
            {
                tool.description.clone_from(&description);
                tool.guidelines.clone_from(&guidelines);
            }
        }
        self.texts.insert(
            name.to_owned(),
            Text {
                description: Some(description),
                guidelines: Some(guidelines),
            },
        );
        self.save_texts(cx);
        cx.notify();
    }

    /// Opens the fields to edit `tool`'s text, the description focused.
    fn start_editing(&mut self, tool: &Tool, window: &mut Window, cx: &mut Context<Self>) {
        self.editors.remove(&tool.name);
        let field = self.editor(tool, window, cx).description.clone();
        field.update(cx, |field, cx| field.focus(window, cx));
        self.editing.insert(tool.name.clone());
        cx.notify();
    }

    /// Back to the text as the model reads it (what was typed is saved already).
    fn stop_editing(&mut self, name: &str, cx: &mut Context<Self>) {
        self.editing.remove(name);
        self.editors.remove(name);
        cx.notify();
    }

    /// Back to the tool's own text: pi is asked again for it.
    fn reset(&mut self, mode: Mode, name: &str, cx: &mut Context<Self>) {
        self.texts.remove(name);
        self.editors.remove(name);
        self.editing.remove(name);
        // Written before pi is asked, so it reads the text without the change.
        let _ = pi_config::tools::save_texts(&self.texts_path, &self.texts);
        self.refresh(mode, cx);
        cx.notify();
    }

    fn save_texts(&self, cx: &App) {
        let texts = self.texts.clone();
        crate::persist::save_in_background(cx, self.texts_path.clone(), move |path| {
            pi_config::tools::save_texts(path, &texts)
        });
    }
}

/// The guidelines in an edited field: one per line, empty lines left out.
fn guideline_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Starts a pi like a conversation in `mode` with the probe extension and reads its report.
/// A Code one gets the optional tools that are on, as sessions do.
async fn probe(
    config: &AppConfig,
    mode: Mode,
    code_tools: &[String],
    cx: &mut AsyncApp,
) -> Result<Vec<Tool>, String> {
    let dir = &config.tools_dir;
    let probe = pi_config::tools::install_probe(dir)
        .map_err(|error| format!("Could not write {}: {error}", dir.display()))?;
    let mut options = crate::chat::session::probe_options(config, mode);
    if mode == Mode::Code {
        crate::chat::session::optional_tool_args(code_tools, &mut options);
    }
    options
        .extra_args
        .extend(["--extension".into(), probe.into()]);
    let (process, events) = pi_rpc::PiProcess::spawn(&options)
        .map_err(|error| format!("Could not start pi: {error}"))?;
    // Dropping the process stops pi: when it answered, or when it took too long.
    let process = Arc::new(Mutex::new(Some(process)));
    let timed_out = Arc::new(AtomicBool::new(false));
    let timer = cx.background_executor().timer(TIMEOUT);
    let _watchdog = cx.background_spawn({
        let (process, timed_out) = (process.clone(), timed_out.clone());
        async move {
            timer.await;
            timed_out.store(true, Ordering::Relaxed);
            drop(
                process
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take(),
            );
        }
    });
    let found = loop {
        match events.recv().await {
            Ok(ClientEvent::ExtensionUi(request)) => {
                if let ExtensionUiMethod::SetStatus {
                    status_key,
                    status_text: Some(text),
                } = request.method
                    && status_key == pi_config::tools::STATUS_KEY
                {
                    break parse(&text).ok_or_else(|| "pi's answer could not be read.".to_owned());
                }
            }
            Ok(ClientEvent::Disconnected) | Err(_) => {
                if timed_out.load(Ordering::Relaxed) {
                    break Err("pi did not answer in time.".to_owned());
                }
                let tail = process
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .map_or_else(Vec::new, pi_rpc::PiProcess::stderr_tail);
                let detail = tail
                    .last()
                    .map_or_else(String::new, |line| format!(": {line}"));
                break Err(format!("pi stopped before it answered{detail}"));
            }
            Ok(_) => {}
        }
    };
    drop(
        process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(),
    );
    found
}

pub(super) fn page() -> SettingPage {
    SettingPage::new("Tools")
        .icon(AppIcon::Wrench)
        .description(
            "What the model can use in each mode, and how much of its context that takes: every \
             tool's description goes along with each message. Changes apply to conversations \
             started afterwards.",
        )
        .group(
            SettingGroup::new()
                .title("Chat")
                .description("No file or shell tools: chats cannot touch your computer.")
                .item(
                    SettingItem::render(|_, window, cx| section(Mode::Chat, window, cx)).keywords(
                        [
                            "python",
                            "web_search",
                            "web_fetch",
                            "chat",
                            "tokens",
                            "context",
                        ],
                    ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Code")
                .description(
                    "pi's tools and your extensions', in sessions on this computer. The optional \
                     ones are off until you turn them on; the sandbox checks them like read.",
                )
                .item(
                    SettingItem::render(|_, window, cx| section(Mode::Code, window, cx)).keywords(
                        [
                            "read",
                            "bash",
                            "edit",
                            "write",
                            "grep",
                            "find",
                            "ls",
                            "codemode",
                            "extension",
                            "code",
                            "tokens",
                            "context",
                        ],
                    ),
                ),
        )
}

/// How a tool is offered in a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Offer {
    /// Always, while its mode is used.
    Always,
    /// Only while web search is on in a chat.
    WithWebSearch,
    /// When its switch is on (Python in chats, the optional Code tools).
    Switch(bool),
}

impl Offer {
    /// Whether it goes along with every message (and counts in the meter).
    fn counted(self) -> bool {
        matches!(self, Self::Always | Self::Switch(true))
    }
}

/// How `tool` is offered in `mode`; `None` for one pi has but never offers there.
fn offer(mode: Mode, tool: &Tool, python_on: bool, code_tools: &[String]) -> Option<Offer> {
    let name = tool.name.as_str();
    match mode {
        Mode::Chat if deepsearch::TOOL_NAMES.contains(&name) => Some(Offer::WithWebSearch),
        Mode::Chat if name == python_tool::TOOL_NAME => Some(Offer::Switch(python_on)),
        Mode::Code if OPTIONAL_CODE_TOOLS.contains(&name) => {
            Some(Offer::Switch(code_tools.iter().any(|tool| tool == name)))
        }
        _ => tool.active.then_some(Offer::Always),
    }
}

/// Whether the tool comes with Pistachio, so its text can be edited.
fn ours(tool: &Tool, app_dir: Option<&Path>) -> bool {
    tool.extension
        .as_ref()
        .is_some_and(|path| app_dir.is_some_and(|dir| path.starts_with(dir)))
}

/// A mode's tools: the context meter, a row per tool, then the ones pi has but leaves out.
fn section(mode: Mode, window: &mut Window, cx: &mut App) -> AnyElement {
    let catalog = tool_catalog(cx);
    catalog.update(cx, |catalog, cx| catalog.ask(cx));
    let python_on = python_tool::enabled(&cx.global::<AppConfig>().python_dir);
    let code_tools = crate::chat::session::code_tools(cx);
    let app_dir = pi_sessions::app_dir();
    let listing = match mode {
        Mode::Chat => catalog.read(cx).chat.clone(),
        Mode::Code => catalog.read(cx).code.clone(),
    };
    let note = |text: String, color: Hsla| {
        div()
            .py_2()
            .text_xs()
            .text_color(color)
            .child(text)
            .into_any_element()
    };
    let tools = match listing {
        Listing::Asking => return note("Asking pi…".into(), cx.theme().muted_foreground),
        Listing::Failed(error) => return note(error, palette(cx).warning),
        Listing::Listed(tools) => tools,
    };
    let mut shown: Vec<(&Tool, Offer)> = Vec::new();
    let mut left_out = Vec::new();
    for tool in &tools {
        match offer(mode, tool, python_on, &code_tools) {
            Some(offer) => shown.push((tool, offer)),
            None => left_out.push(tool.name.as_str()),
        }
    }
    // Offered with every message first, then the others; within each, the largest first.
    shown.sort_by_key(|(tool, offer)| {
        (
            !offer.counted(),
            matches!(offer, Offer::Switch(false)),
            std::cmp::Reverse(tool.context_chars()),
        )
    });
    let colors = shades(&shown, cx);
    // The fields of the tools being edited.
    let editing = catalog.read(cx).editing.clone();
    let editors: HashMap<String, (Entity<TextareaState>, Entity<TextareaState>)> = shown
        .iter()
        .filter(|(tool, _)| editing.contains(&tool.name))
        .map(|(tool, _)| {
            catalog.update(cx, |catalog, cx| {
                let editor = catalog.editor(tool, window, cx);
                (
                    tool.name.clone(),
                    (editor.description.clone(), editor.guidelines.clone()),
                )
            })
        })
        .collect();
    let catalog = catalog.read(cx);
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap_2()
        .child(meter(mode, &shown, &colors, cx))
        .child(
            v_flex()
                .w_full()
                .when(shown.is_empty(), |this| {
                    this.child(note(
                        "No tools: the model can only answer.".into(),
                        theme.muted_foreground,
                    ))
                })
                .children(shown.iter().zip(&colors).enumerate().map(
                    |(index, ((tool, offer), color))| {
                        let expanded = catalog.open.contains(&(mode, tool.name.clone()));
                        let edit = editors
                            .get(&tool.name)
                            .map(|(description, guidelines)| Edit {
                                description: description.clone(),
                                guidelines: guidelines.clone(),
                            });
                        row(
                            &RowInfo {
                                mode,
                                index,
                                offer: *offer,
                                color: *color,
                                expanded,
                                origin: origin(tool, app_dir.as_deref()),
                                editable: ours(tool, app_dir.as_deref()),
                                changed: catalog.texts.contains_key(&tool.name),
                            },
                            tool,
                            edit,
                            cx,
                        )
                    },
                )),
        )
        .when(!left_out.is_empty() && mode == Mode::Code, |this| {
            this.child(note(
                format!(
                    "pi also has {}, which Code sessions don't offer.",
                    list(&left_out)
                ),
                theme.muted_foreground,
            ))
        })
        .into_any_element()
}

/// Each tool's color in the meter: shades of the accent, the largest the strongest; tools
/// offered only sometimes faint, and those switched off without color.
fn shades(tools: &[(&Tool, Offer)], cx: &App) -> Vec<Hsla> {
    const STRENGTH: [f32; 6] = [1.0, 0.64, 0.42, 0.28, 0.2, 0.15];
    let accent = palette(cx).accent;
    let mut steady = 0;
    tools
        .iter()
        .map(|(_, offer)| match offer {
            Offer::WithWebSearch => accent.opacity(0.12),
            Offer::Switch(false) => cx.theme().muted_foreground.opacity(0.35),
            Offer::Always | Offer::Switch(true) => {
                let color = accent.opacity(STRENGTH[steady.min(STRENGTH.len() - 1)]);
                steady += 1;
                color
            }
        })
        .collect()
}

/// How much of the model's context the tools take: a total, a bar split by tool and a legend.
/// Tools switched off are left out; chats' web search tools are counted apart.
fn meter(mode: Mode, tools: &[(&Tool, Offer)], colors: &[Hsla], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let in_bar: Vec<(&Tool, Offer, Hsla)> = tools
        .iter()
        .zip(colors)
        .filter(|((_, offer), _)| !matches!(offer, Offer::Switch(false)))
        .map(|((tool, offer), color)| (*tool, *offer, *color))
        .collect();
    let sum = |sometimes: bool| -> usize {
        in_bar
            .iter()
            .filter(|(_, offer, _)| (*offer == Offer::WithWebSearch) == sometimes)
            .map(|(tool, ..)| tool.context_chars())
            .sum()
    };
    let (always, sometimes) = (sum(false), sum(true));
    let all = (always + sometimes).max(1);
    #[allow(clippy::cast_precision_loss)]
    let share = |chars: usize| chars as f32 / all as f32;
    let bar = h_flex()
        .w_full()
        .h(px(10.))
        .gap(px(2.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.border)
        .children(in_bar.iter().map(|(tool, _, color)| {
            div()
                .h_full()
                .w(relative(share(tool.context_chars())))
                .bg(*color)
        }));
    let legend = h_flex()
        .w_full()
        .flex_wrap()
        .gap_x_4()
        .gap_y_1()
        .children(in_bar.iter().map(|(tool, offer, color)| {
            h_flex()
                .gap_1p5()
                .text_xs()
                .child(dot(*color, *offer, cx))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child(tool.name.clone()),
                )
                .child(div().text_color(muted).child(tokens(tool.context_chars())))
        }));
    let headline = h_flex()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(format!("≈ {}", tokens(always))),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted)
                .child("tokens with every message"),
        );
    let detail = if sometimes > 0 {
        format!("≈ {} with web search on", tokens(always + sometimes))
    } else {
        format!("{} characters", thousands(always))
    };
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.5))
        .debug_selector(move || format!("tools-meter-{}", mode_id(mode)))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_end()
                .gap_3()
                .child(headline)
                .child(div().text_xs().text_color(muted).child(detail)),
        )
        .child(bar)
        .child(legend)
        .child(div().text_xs().text_color(muted).child(format!(
            "Counted from the text pi sends for each tool (description, parameters and \
             guidelines), at about {CHARS_PER_TOKEN} characters a token."
        )))
        .into_any_element()
}

/// A tool's color as a dot: outlined for one offered only sometimes (its color is faint), and
/// hollow for one switched off.
fn dot(color: Hsla, offer: Offer, cx: &App) -> Div {
    let dot = div().flex_none().size(px(8.)).rounded_full();
    match offer {
        Offer::WithWebSearch => dot
            .bg(color)
            .border_1()
            .border_color(palette(cx).accent.opacity(0.6)),
        Offer::Switch(false) => dot.border_1().border_color(color),
        Offer::Always | Offer::Switch(true) => dot.bg(color),
    }
}

/// Tokens for `chars`, for people: `640`, `1.2k`.
fn tokens(chars: usize) -> String {
    let tokens = chars.div_ceil(CHARS_PER_TOKEN);
    if tokens < 1000 {
        tokens.to_string()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let thousands = tokens as f64 / 1000.;
        format!("{thousands:.1}k")
    }
}

/// `4960` as `4,960`.
fn thousands(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn mode_id(mode: Mode) -> &'static str {
    match mode {
        Mode::Chat => "chat",
        Mode::Code => "code",
    }
}

/// Our own short account of a tool; for one we don't know, the first sentence it gives the
/// model.
fn summary(tool: &Tool) -> String {
    let known = match tool.name.as_str() {
        "read" => "Reads a file, or looks at an image, in the project.",
        "bash" => "Runs a shell command in the project folder.",
        "edit" => "Changes part of a file by replacing exact text.",
        "write" => "Creates a file, or replaces all of its content.",
        "grep" => "Searches the contents of files, faster and tidier than grep in bash.",
        "find" => "Finds files by name pattern.",
        "ls" => "Lists a folder's contents.",
        "codemode" => {
            "Writes a short script that calls several tools at once and keeps only what matters."
        }
        "powershell" => "Runs a PowerShell command.",
        "web_search" => "Searches the web: titles, links and short snippets.",
        "web_fetch" => "Reads a web page or PDF as text.",
        "python" => "Runs short Python programs in a sealed sandbox: maths, text, dates, checks.",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let description = tool.description.trim();
    let first = description
        .find(". ")
        .map_or(description, |end| &description[..=end]);
    let first = first.lines().next().unwrap_or_default();
    if first.chars().count() > 140 {
        first.chars().take(139).collect::<String>() + "…"
    } else {
        first.to_owned()
    }
}

/// "a, b and c".
fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Where a tool comes from: pi, Pistachio, or an extension (by its file or folder name).
fn origin(tool: &Tool, app_dir: Option<&Path>) -> String {
    let Some(path) = &tool.extension else {
        return "pi".into();
    };
    if ours(tool, app_dir) {
        return "Pistachio".into();
    }
    // `…/name/index.ts` is named by its folder.
    let stem = path.file_stem().map(|stem| stem.to_string_lossy());
    let name = match stem.as_deref() {
        Some("index") | None => path
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()),
        Some(stem) => Some(stem.to_owned()),
    };
    name.unwrap_or_else(|| "extension".into())
}

/// What a row shows besides its tool.
struct RowInfo {
    mode: Mode,
    index: usize,
    offer: Offer,
    color: Hsla,
    expanded: bool,
    origin: String,
    /// One of Pistachio's tools: its text can be edited.
    editable: bool,
    /// Its text was edited: it differs from the tool's own.
    changed: bool,
}

/// The fields of a Pistachio tool's text, while it is edited.
struct Edit {
    description: Entity<TextareaState>,
    guidelines: Entity<TextareaState>,
}

/// A tool: its name, where it comes from, its share of the context and its switch; our
/// summary; and, opened, exactly what the model is told (to edit, for Pistachio's tools).
fn row(info: &RowInfo, tool: &Tool, edit: Option<Edit>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    let off = info.offer == Offer::Switch(false);
    let tag = |text: String, color: Hsla| {
        div()
            .flex_none()
            .px_1p5()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(color)
            .child(text)
    };
    let mode = info.mode;
    let name = tool.name.clone();
    let disclosure = h_flex()
        .id(SharedString::from(format!(
            "tool-text-{}-{}",
            mode_id(mode),
            tool.name
        )))
        .gap_1()
        .text_xs()
        .text_color(muted)
        .cursor_pointer()
        .hover(|style| style.text_color(theme.foreground))
        .child(
            Icon::new(if info.expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall(),
        )
        .child("What the model sees")
        .on_click({
            let name = name.clone();
            move |_, _, cx| {
                tool_catalog(cx).update(cx, |catalog, cx| catalog.toggle_open(mode, &name, cx));
            }
        });
    let switch = match info.offer {
        Offer::Switch(on) => Some(crate::ui::hover_switch(
            SharedString::from(format!("tool-switch-{}-{}", mode_id(mode), tool.name)),
            on,
            {
                let name = name.clone();
                move |on, _, cx| switch_tool(mode, &name, on, cx)
            },
            Some(format!("Offer {} to the model", tool.name).into()),
            cx,
        )),
        _ => None,
    };
    let size = if off {
        "off".to_owned()
    } else {
        format!("≈ {} tokens", tokens(tool.context_chars()))
    };
    let dimmed = |color: Hsla| if off { color.opacity(0.55) } else { color };
    v_flex()
        .w_full()
        .gap_1()
        .py_3()
        .when(info.index > 0, |this| {
            this.border_t_1().border_color(theme.border)
        })
        .debug_selector({
            let name = tool.name.clone();
            move || format!("tool-{name}")
        })
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(dot(info.color, info.offer, cx))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(mono)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(dimmed(theme.foreground))
                                        .child(tool.name.clone()),
                                )
                                .child(tag(info.origin.clone(), muted))
                                .when(info.offer == Offer::WithWebSearch, |this| {
                                    this.child(tag(
                                        "While web search is on".into(),
                                        palette(cx).accent,
                                    ))
                                })
                                .when(info.changed, |this| {
                                    this.child(tag("Edited".into(), palette(cx).warning))
                                }),
                        )
                        .child(
                            div()
                                .pl(px(16.))
                                .text_sm()
                                .text_color(dimmed(theme.foreground))
                                .child(summary(tool)),
                        )
                        .child(div().pl(px(16.)).pt_0p5().child(disclosure)),
                )
                // The size, and under it the switch.
                .child(
                    v_flex()
                        .flex_none()
                        .items_end()
                        .gap_1p5()
                        .child(div().text_xs().text_color(muted).child(size))
                        .children(switch),
                ),
        )
        .when(info.expanded, |this| {
            let text = match edit {
                Some(edit) => editable_text(mode, tool, edit, info.changed, cx),
                None => model_text(tool, info.editable.then_some(mode), cx),
            };
            this.child(div().pl(px(16.)).child(text))
        })
        .into_any_element()
}

/// Turns Python (chats) or an optional Code tool on or off.
fn switch_tool(mode: Mode, name: &str, on: bool, cx: &mut App) {
    match mode {
        Mode::Chat => {
            let dir = cx.global::<AppConfig>().python_dir.clone();
            if let Err(error) = python_tool::set_enabled(&dir, on) {
                log::warn!(
                    "could not save the Python setting in {}: {error}",
                    dir.display()
                );
            }
            tool_catalog(cx).update(cx, |_, cx| cx.notify());
        }
        Mode::Code => {
            tool_catalog(cx).update(cx, |catalog, cx| catalog.set_code_tool(name, on, cx));
        }
    }
}

/// A button inside the quote of what the model is told: transparent, with a hover tint of the
/// text color, which shows on the quote's fill (the theme's own hover nearly matches it).
trait OnQuote {
    fn on_quote(self, cx: &App) -> Self;
}

impl OnQuote for Button {
    fn on_quote(self, cx: &App) -> Self {
        let theme = cx.theme();
        self.custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .foreground(theme.foreground)
                .hover(theme.foreground.opacity(0.1))
                .active(theme.foreground.opacity(0.16)),
        )
    }
}

/// The frame of what the model is told: a quote, set off by the accent at its left.
fn quote(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .mt_1p5()
        .w_full()
        .px_3()
        .py_2p5()
        .rounded_md()
        .border_l_2()
        .border_color(palette(cx).accent.opacity(0.7))
        .bg(theme.muted)
}

fn quote_label(text: &'static str, first: bool, cx: &App) -> Div {
    div()
        .when(!first, |this| this.pt_2())
        .pb_0p5()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A line of what the model is told: monospace, small, word for word.
fn quoted(content: String, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .w_full()
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .line_height(relative(1.55))
        .text_color(theme.foreground.opacity(0.88))
        .whitespace_normal()
        .child(content)
}

/// A parameter as the model reads it: name, type, whether required, and its description.
fn parameter_line(parameter: &pi_config::tools::Parameter) -> String {
    let mut head = parameter.name.clone();
    if !parameter.kind.is_empty() {
        head = format!("{head}: {}", parameter.kind);
    }
    if parameter.required {
        head.push_str(" (required)");
    }
    if parameter.description.is_empty() {
        head
    } else {
        format!("{head} — {}", parameter.description)
    }
}

/// Exactly what the model is told about `tool`, word for word: its description, the guidelines
/// added to the system prompt, and its parameters. With `edit_in` (one of Pistachio's tools,
/// in that mode) an Edit button opens it in fields.
fn model_text(tool: &Tool, edit_in: Option<Mode>, cx: &App) -> AnyElement {
    let edit = edit_in.map(|mode| {
        let name = tool.name.clone();
        Button::new(SharedString::from(format!(
            "tool-edit-{}-{}",
            mode_id(mode),
            tool.name
        )))
        .icon(Icon::from(AppIcon::Pencil))
        .label("Edit")
        .xsmall()
        .on_quote(cx)
        .accessibility_label(format!("Edit what the model reads about {}", tool.name))
        .on_click(move |_, window, cx| {
            let name = name.clone();
            tool_catalog(cx).update(cx, |catalog, cx| {
                let tool = [&catalog.chat, &catalog.code]
                    .into_iter()
                    .find_map(|listing| match listing {
                        Listing::Listed(tools) => tools.iter().find(|tool| tool.name == name),
                        _ => None,
                    })
                    .cloned();
                if let Some(tool) = tool {
                    catalog.start_editing(&tool, window, cx);
                }
            });
        })
    });
    quote(cx)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_start()
                .child(quote_label("Description", true, cx))
                .children(edit),
        )
        .child(quoted(tool.description.clone(), cx))
        .when(!tool.guidelines.is_empty(), |this| {
            this.child(quote_label("Guidelines in the system prompt", false, cx))
                .children(
                    tool.guidelines
                        .iter()
                        .map(|line| quoted(format!("- {line}"), cx)),
                )
        })
        .when(!tool.parameters.is_empty(), |this| {
            this.child(quote_label("Parameters", false, cx)).children(
                tool.parameters
                    .iter()
                    .map(|parameter| quoted(parameter_line(parameter), cx)),
            )
        })
        .into_any_element()
}

/// [`model_text`] for one of Pistachio's tools: the description and guidelines in fields to
/// edit (saved as typed), the parameters as they are, and a way back to the tool's own text.
fn editable_text(mode: Mode, tool: &Tool, edit: Edit, changed: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let field = |state: &Entity<TextareaState>| {
        div()
            .w_full()
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .child(Textarea::new(state).small().text_xs())
    };
    let name = tool.name.clone();
    let reset = Button::new(SharedString::from(format!("tool-reset-{}", tool.name)))
        .label("Use the original text")
        .xsmall()
        .on_quote(cx)
        .disabled(!changed)
        .on_click(move |_, _, cx| {
            let name = name.clone();
            tool_catalog(cx).update(cx, |catalog, cx| catalog.reset(mode, &name, cx));
        });
    let done = {
        let name = tool.name.clone();
        Button::new(SharedString::from(format!("tool-done-{}", tool.name)))
            .label("Done")
            .xsmall()
            .on_quote(cx)
            .border_1()
            .border_color(cx.theme().border)
            .on_click(move |_, _, cx| {
                let name = name.clone();
                tool_catalog(cx).update(cx, |catalog, cx| catalog.stop_editing(&name, cx));
            })
    };
    quote(cx)
        .gap_0p5()
        .child(quote_label("Description", true, cx))
        .child(field(&edit.description))
        .child(quote_label(
            "Guidelines in the system prompt, one per line",
            false,
            cx,
        ))
        .child(field(&edit.guidelines))
        .when(!tool.parameters.is_empty(), |this| {
            this.child(quote_label("Parameters", false, cx)).children(
                tool.parameters
                    .iter()
                    .map(|parameter| quoted(parameter_line(parameter), cx)),
            )
        })
        .child(
            h_flex()
                .w_full()
                .pt_2()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Saved as you type; used by conversations started afterwards."),
                )
                .child(h_flex().gap_1().child(reset).child(done)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pi_config::tools::Tool;
    use pi_sessions::Mode;

    use super::{Offer, guideline_lines, list, offer, origin, summary, thousands, tokens};

    fn tool(name: &str, extension: Option<&str>, active: bool) -> Tool {
        Tool {
            name: name.into(),
            description: String::new(),
            guidelines: Vec::new(),
            schema: String::new(),
            parameters: Vec::new(),
            extension: extension.map(Into::into),
            active,
        }
    }

    #[test]
    fn origins_are_named_plainly() {
        let app = Path::new("/home/me/.pi/agent/pistachio");
        let name = |extension| origin(&tool("t", extension, true), Some(app));
        assert_eq!(name(None), "pi");
        assert_eq!(
            name(Some("/home/me/.pi/agent/pistachio/python/extension.ts")),
            "Pistachio"
        );
        assert_eq!(name(Some("/home/me/.pi/agent/extensions/todo.ts")), "todo");
        assert_eq!(
            name(Some("/home/me/.pi/agent/extensions/mcp/index.ts")),
            "mcp"
        );
    }

    #[test]
    fn how_tools_are_offered() {
        let none: [String; 0] = [];
        let on = ["grep".to_owned()];
        assert_eq!(
            offer(Mode::Chat, &tool("web_search", None, false), true, &none),
            Some(Offer::WithWebSearch)
        );
        assert_eq!(
            offer(Mode::Chat, &tool("python", None, true), false, &none),
            Some(Offer::Switch(false))
        );
        assert_eq!(
            offer(Mode::Chat, &tool("python", None, false), true, &none),
            Some(Offer::Switch(true))
        );
        // Chats never offer pi's file tools, though pi lists them.
        assert_eq!(
            offer(Mode::Chat, &tool("read", None, false), true, &none),
            None
        );
        // The optional Code tools follow their switches, whatever pi reported.
        assert_eq!(
            offer(Mode::Code, &tool("grep", None, false), true, &on),
            Some(Offer::Switch(true))
        );
        assert_eq!(
            offer(Mode::Code, &tool("codemode", None, true), true, &on),
            Some(Offer::Switch(false))
        );
        assert_eq!(
            offer(Mode::Code, &tool("read", None, true), true, &none),
            Some(Offer::Always)
        );
        assert_eq!(
            offer(Mode::Code, &tool("powershell", None, false), true, &none),
            None
        );
        assert!(Offer::Switch(true).counted() && !Offer::Switch(false).counted());
        assert!(Offer::Always.counted() && !Offer::WithWebSearch.counted());
    }

    #[test]
    fn edited_guidelines_are_one_per_line() {
        assert_eq!(
            guideline_lines("  Use it.\n\n- Not this way \n"),
            ["Use it.", "- Not this way"]
        );
        assert_eq!(guideline_lines("\n \n"), Vec::<String>::new());
    }

    #[test]
    fn summaries_are_ours_or_the_first_sentence() {
        assert_eq!(
            summary(&tool("edit", None, true)),
            "Changes part of a file by replacing exact text."
        );
        let mut other = tool("todo", Some("/x/todo.ts"), true);
        other.description = "Keeps a list. Use it to plan.\nMore.".into();
        assert_eq!(summary(&other), "Keeps a list.");
    }

    #[test]
    fn sizes_read_easily() {
        assert_eq!(tokens(10), "3");
        assert_eq!(tokens(4_800), "1.2k");
        assert_eq!(thousands(4_960), "4,960");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }

    #[test]
    fn names_are_listed_in_words() {
        assert_eq!(list(&["grep"]), "grep");
        assert_eq!(list(&["grep", "find", "ls"]), "grep, find and ls");
    }
}
