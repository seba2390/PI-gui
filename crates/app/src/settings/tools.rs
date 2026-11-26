//! Settings → Tools: the tools the model gets in chats and in Code sessions on this computer,
//! as pi describes them to it. pi is asked when the page is first shown after Settings opens
//! (see [`pi_config::tools`]): a pi that keeps no session, launched like a conversation of
//! each mode, reports them and quits.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::tools::{Tool, parse};
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
            "What the model can use, as pi describes each tool to it. This is the text the \
             model reads to decide when to call a tool and how.",
        )
        .group(
            SettingGroup::new()
                .title("Chat")
                .description(
                    "Chats have no file or shell tools: the model cannot read, change or run \
                     anything on your computer. Web search's tools are offered while web search \
                     is on in a chat; Python can be turned off under General.",
                )
                .item(
                    SettingItem::render(|_, _, cx| rows(Mode::Chat, cx)).keywords([
                        "python",
                        "web_search",
                        "web_fetch",
                        "chat",
                    ]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Code")
                .description(
                    "pi's tools and those of your extensions, in Code sessions on this \
                     computer. A project can add its own; commands run in the sandbox when it is \
                     on.",
                )
                .item(
                    SettingItem::render(|_, _, cx| rows(Mode::Code, cx)).keywords([
                        "read",
                        "bash",
                        "edit",
                        "write",
                        "extension",
                        "code",
                    ]),
                ),
        )
}

/// The tools of `mode`: those offered first, then a line naming the ones pi has but leaves out.
fn rows(mode: Mode, cx: &mut App) -> AnyElement {
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
        Listing::Asking => {
            return note("Asking pi…".into(), theme.muted_foreground);
        }
        Listing::Failed(error) => return note(error.clone(), palette(cx).warning),
        Listing::Listed(tools) => tools,
    };
    let (shown, left_out): (Vec<&Tool>, Vec<&Tool>) = tools
        .iter()
        .partition(|tool| tool.active || state(mode, tool).is_some());
    let left_out = left_out
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let app_dir = pi_sessions::app_dir();
    v_flex()
        .w_full()
        .gap_1()
        .when(shown.is_empty(), |this| {
            this.child(note(
                "No tools: the model can only answer.".into(),
                theme.muted_foreground,
            ))
        })
        .children(
            shown
                .into_iter()
                .map(|tool| row(mode, tool, app_dir.as_deref(), cx)),
        )
        .when(!left_out.is_empty() && mode == Mode::Code, |this| {
            this.child(note(
                format!("pi also has {}, which it does not offer.", list(&left_out)),
                theme.muted_foreground,
            ))
        })
        .into_any_element()
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

fn row(mode: Mode, tool: &Tool, app_dir: Option<&Path>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    let accent = palette(cx).accent;
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
    let parameters = tool
        .parameters
        .iter()
        .map(|parameter| {
            if parameter.required {
                parameter.name.clone()
            } else {
                format!("{}?", parameter.name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let state = state(mode, tool);
    v_flex()
        .w_full()
        .gap_1()
        .py_2()
        .debug_selector({
            let name = tool.name.clone();
            move || format!("tool-{name}")
        })
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_family(mono.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .child(tool.name.clone()),
                )
                .child(tag(origin(tool, app_dir), muted))
                .when_some(state, |this, state| this.child(tag(state.into(), accent))),
        )
        .when(!tool.description.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.foreground)
                    .whitespace_normal()
                    .child(tool.description.clone()),
            )
        })
        .children(tool.guidelines.iter().map(|line| {
            h_flex()
                .items_start()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(div().flex_none().child("·"))
                .child(div().flex_1().min_w_0().child(line.clone()))
        }))
        .when(!parameters.is_empty(), |this| {
            this.child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(muted)
                    .child("Parameters")
                    .child(div().font_family(mono).child(parameters)),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pi_config::tools::Tool;
    use pi_sessions::Mode;

    use super::{list, origin, state};

    fn tool(name: &str, extension: Option<&str>, active: bool) -> Tool {
        Tool {
            name: name.into(),
            description: String::new(),
            guidelines: Vec::new(),
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
    fn names_are_listed_in_words() {
        assert_eq!(list(&["grep"]), "grep");
        assert_eq!(list(&["grep", "find", "ls"]), "grep, find and ls");
    }
}
