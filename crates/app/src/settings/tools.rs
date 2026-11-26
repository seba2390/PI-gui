//! Settings → Tools: the tools the model gets in chats and in Code sessions on this computer,
//! as pi describes them to it. pi is asked when the page is first shown after Settings opens
//! (see [`pi_config::tools`]): a pi that keeps no session, launched like a conversation of
//! each mode, reports them and quits.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::tools::{CHARS_PER_TOKEN, Tool, parse};
use pi_rpc::ClientEvent;
use pi_rpc::protocol::ExtensionUiMethod;
use pi_sessions::Mode;

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

/// The tools of both modes, asked for once per opening of Settings.
#[derive(Default)]
pub struct ToolCatalog {
    chat: Listing,
    code: Listing,
    /// pi was asked since Settings opened.
    asked: bool,
    /// The tools whose text for the model is shown.
    open: HashSet<(Mode, String)>,
    _tasks: Vec<Task<()>>,
}

struct ToolCatalogHandle(Entity<ToolCatalog>);

impl Global for ToolCatalogHandle {}

pub fn tool_catalog(cx: &mut App) -> Entity<ToolCatalog> {
    if let Some(handle) = cx.try_global::<ToolCatalogHandle>() {
        return handle.0.clone();
    }
    let catalog = cx.new(|_| ToolCatalog::default());
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
        self._tasks = [Mode::Chat, Mode::Code]
            .into_iter()
            .map(|mode| {
                let config = cx.global::<AppConfig>().clone();
                cx.spawn(async move |this, cx| {
                    config.child_env.wait().await;
                    let listing = match probe(&config, mode, cx).await {
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
                })
            })
            .collect();
    }
}

/// Starts a pi like a conversation in `mode` with the probe extension and reads its report.
async fn probe(config: &AppConfig, mode: Mode, cx: &mut AsyncApp) -> Result<Vec<Tool>, String> {
    let dir = pi_sessions::app_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tools");
    let probe = pi_config::tools::install_probe(&dir)
        .map_err(|error| format!("Could not write {}: {error}", dir.display()))?;
    let mut options = crate::chat::session::probe_options(config, mode);
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
             tool's description goes along with each message.",
        )
        .group(
            SettingGroup::new()
                .title("Chat")
                .description("No file or shell tools: chats cannot touch your computer.")
                .item(
                    SettingItem::render(|_, _, cx| section(Mode::Chat, cx)).keywords([
                        "python",
                        "web_search",
                        "web_fetch",
                        "chat",
                        "tokens",
                        "context",
                    ]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Code")
                .description("pi's tools and your extensions', in sessions on this computer.")
                .item(
                    SettingItem::render(|_, _, cx| section(Mode::Code, cx)).keywords([
                        "read",
                        "bash",
                        "edit",
                        "write",
                        "extension",
                        "code",
                        "tokens",
                        "context",
                    ]),
                ),
        )
}

/// A mode's tools: the context meter, a row per tool, then the ones pi has but leaves out.
fn section(mode: Mode, cx: &mut App) -> AnyElement {
    let catalog = tool_catalog(cx);
    catalog.update(cx, |catalog, cx| catalog.ask(cx));
    let catalog = catalog.read(cx);
    let listing = match mode {
        Mode::Chat => &catalog.chat,
        Mode::Code => &catalog.code,
    };
    let theme = cx.theme();
    let note = |text: String, color: Hsla| {
        div()
            .py_2()
            .text_xs()
            .text_color(color)
            .child(text)
            .into_any_element()
    };
    let tools = match listing {
        Listing::Asking => return note("Asking pi…".into(), theme.muted_foreground),
        Listing::Failed(error) => return note(error.clone(), palette(cx).warning),
        Listing::Listed(tools) => tools,
    };
    let (mut shown, left_out): (Vec<&Tool>, Vec<&Tool>) = tools
        .iter()
        .partition(|tool| tool.active || state(mode, tool).is_some());
    // Those always offered first; within each, the largest first.
    shown.sort_by_key(|tool| {
        (
            state(mode, tool).is_some(),
            std::cmp::Reverse(tool.context_chars()),
        )
    });
    let left_out = left_out
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let app_dir = pi_sessions::app_dir();
    let colors = shades(mode, &shown, cx);
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
                .children(
                    shown
                        .iter()
                        .zip(&colors)
                        .enumerate()
                        .map(|(index, (tool, color))| {
                            let expanded = catalog.open.contains(&(mode, tool.name.clone()));
                            row(mode, index, tool, *color, expanded, app_dir.as_deref(), cx)
                        }),
                ),
        )
        .when(!left_out.is_empty() && mode == Mode::Code, |this| {
            this.child(note(
                format!("pi also has {}, which it does not offer.", list(&left_out)),
                theme.muted_foreground,
            ))
        })
        .into_any_element()
}

/// Each tool's color in the meter: shades of the accent, the largest the strongest; tools
/// offered only sometimes (chats' web search) faint.
fn shades(mode: Mode, tools: &[&Tool], cx: &App) -> Vec<Hsla> {
    const STRENGTH: [f32; 6] = [1.0, 0.64, 0.42, 0.28, 0.2, 0.15];
    let accent = palette(cx).accent;
    let mut steady = 0;
    tools
        .iter()
        .map(|tool| {
            if state(mode, tool).is_some() {
                return accent.opacity(0.12);
            }
            let color = accent.opacity(STRENGTH[steady.min(STRENGTH.len() - 1)]);
            steady += 1;
            color
        })
        .collect()
}

/// How much of the model's context the tools take: a total, a bar split by tool and a legend.
fn meter(mode: Mode, tools: &[&Tool], colors: &[Hsla], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let sum = |sometimes: bool| -> usize {
        tools
            .iter()
            .filter(|tool| state(mode, tool).is_some() == sometimes)
            .map(|tool| tool.context_chars())
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
        .children(tools.iter().zip(colors).map(|(tool, color)| {
            div()
                .h_full()
                .w(relative(share(tool.context_chars())))
                .bg(*color)
        }));
    let legend =
        h_flex()
            .w_full()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .children(tools.iter().zip(colors).map(|(tool, color)| {
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .child(dot(*color, state(mode, tool).is_some(), cx))
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

/// A tool's color as a dot; outlined for one offered only sometimes, whose color is faint.
fn dot(color: Hsla, sometimes: bool, cx: &App) -> Div {
    div()
        .flex_none()
        .size(px(8.))
        .rounded_full()
        .bg(color)
        .when(sometimes, |dot| {
            dot.border_1().border_color(palette(cx).accent.opacity(0.6))
        })
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

/// Why a tool is shown although pi does not offer it right away.
fn state(mode: Mode, tool: &Tool) -> Option<&'static str> {
    let web = deepsearch::TOOL_NAMES.contains(&tool.name.as_str());
    match mode {
        Mode::Chat if web => Some("While web search is on"),
        Mode::Chat if tool.name == python_tool::TOOL_NAME && !tool.active => {
            Some("Turned off under General")
        }
        _ => None,
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
        "grep" => "Searches the contents of files.",
        "find" => "Finds files by name.",
        "ls" => "Lists a folder.",
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
    if app_dir.is_some_and(|dir| path.starts_with(dir)) {
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

/// A tool: its name, where it comes from and its share of the context; our summary; and,
/// opened, exactly what the model is told.
fn row(
    mode: Mode,
    index: usize,
    tool: &Tool,
    color: Hsla,
    expanded: bool,
    app_dir: Option<&Path>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
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
    let key = (mode, tool.name.clone());
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
            Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall(),
        )
        .child("What the model sees")
        .on_click(move |_, _, cx| {
            let key = key.clone();
            tool_catalog(cx).update(cx, |catalog, cx| {
                if !catalog.open.remove(&key) {
                    catalog.open.insert(key);
                }
                cx.notify();
            });
        });
    v_flex()
        .w_full()
        .gap_1()
        .py_3()
        .when(index > 0, |this| {
            this.border_t_1().border_color(theme.border)
        })
        .debug_selector({
            let name = tool.name.clone();
            move || format!("tool-{name}")
        })
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(dot(color, state(mode, tool).is_some(), cx))
                .child(
                    div()
                        .text_sm()
                        .font_family(mono)
                        .font_weight(FontWeight::MEDIUM)
                        .child(tool.name.clone()),
                )
                .child(tag(origin(tool, app_dir), muted))
                .when_some(state(mode, tool), |this, state| {
                    this.child(tag(state.into(), palette(cx).accent))
                })
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("≈ {} tokens", tokens(tool.context_chars()))),
                ),
        )
        .child(
            div()
                .pl(px(16.))
                .text_sm()
                .text_color(theme.foreground)
                .child(summary(tool)),
        )
        .child(div().pl(px(16.)).pt_0p5().child(disclosure))
        .when(expanded, |this| {
            this.child(div().pl(px(16.)).child(model_text(tool, cx)))
        })
        .into_any_element()
}

/// Exactly what the model is told about `tool`, word for word: its description, the guidelines
/// added to the system prompt, and its parameters.
fn model_text(tool: &Tool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    let label = |text: &'static str| {
        div()
            .pt_2()
            .pb_0p5()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted)
            .child(text)
    };
    let text = |content: String| {
        div()
            .w_full()
            .font_family(mono.clone())
            .text_xs()
            .line_height(relative(1.55))
            .text_color(theme.foreground.opacity(0.88))
            .whitespace_normal()
            .child(content)
    };
    let parameter_line = |parameter: &pi_config::tools::Parameter| {
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
    };
    v_flex()
        .mt_1p5()
        .w_full()
        .px_3()
        .py_2p5()
        .rounded_md()
        .border_l_2()
        .border_color(palette(cx).accent.opacity(0.7))
        .bg(theme.muted)
        .child(
            div()
                .pb_0p5()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child("Description"),
        )
        .child(text(tool.description.clone()))
        .when(!tool.guidelines.is_empty(), |this| {
            this.child(label("Guidelines in the system prompt"))
                .children(tool.guidelines.iter().map(|line| text(format!("- {line}"))))
        })
        .when(!tool.parameters.is_empty(), |this| {
            this.child(label("Parameters")).children(
                tool.parameters
                    .iter()
                    .map(|parameter| text(parameter_line(parameter))),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pi_config::tools::Tool;
    use pi_sessions::Mode;

    use super::{list, origin, state, summary, thousands, tokens};

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
    fn chats_show_web_search_and_python_even_when_off() {
        assert_eq!(
            state(Mode::Chat, &tool("web_search", None, false)),
            Some("While web search is on")
        );
        assert_eq!(
            state(Mode::Chat, &tool("python", None, false)),
            Some("Turned off under General")
        );
        assert_eq!(state(Mode::Chat, &tool("python", None, true)), None);
        assert_eq!(state(Mode::Code, &tool("grep", None, false)), None);
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
