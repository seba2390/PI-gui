//! Settings → Document parsing: the parsers, each with its state and what can be done with it,
//! and the dialogs to download and remove one.

use documents::{Installed, Parser, Progress};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::document_parsers::{DocumentParsers, document_parsers, existing};
use crate::model_store::size_label;
use crate::theme::palette;
use crate::ui::dialog::open_form_dialog;
use crate::ui::{AppIcon, QuietButton as _};

pub(super) fn page() -> SettingPage {
    SettingPage::new("Document parsing")
        .icon(AppIcon::Files)
        .description(
            "How PDF files are read: the ones you attach to a chat, the ones chats find on the \
             web, and the ones pi reads in Code sessions on this computer.",
        )
        .group(SettingGroup::new().title("Parser").item(
            SettingItem::render(|_, _, cx| rows(cx)).keywords([
                "PDF",
                "Word",
                "Excel",
                "PowerPoint",
                "scan",
                "Docling",
                "MinerU",
                "PyMuPDF",
            ]),
        ))
}

/// What a row shows below the name, and what can be done with it.
#[derive(Debug, Clone, PartialEq)]
struct State {
    /// Version, size and license.
    facts: String,
    problem: Option<String>,
    /// How far the download is, and what it does.
    progress: Option<(f32, &'static str)>,
    in_use: bool,
    actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Use,
    /// Asks first.
    Download,
    Update,
    Cancel,
    /// Asks first.
    Remove,
}

impl Action {
    fn run(self, parser: Parser, window: &mut Window, cx: &mut App) {
        match self {
            Self::Use => {
                document_parsers(cx).update(cx, |parsers, cx| parsers.choose(parser, cx));
            }
            Self::Update => {
                document_parsers(cx).update(cx, |parsers, cx| parsers.install(parser, cx));
            }
            Self::Cancel => {
                document_parsers(cx).update(cx, |parsers, _| parsers.cancel_download());
            }
            Self::Download => ask_to_download(parser, window, cx),
            Self::Remove => ask_to_remove(parser, window, cx),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Download => "download",
            Self::Update => "update",
            Self::Cancel => "cancel",
            Self::Remove => "remove",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Use => "Use",
            Self::Download => "Download…",
            Self::Update => "Update",
            Self::Cancel => "Cancel",
            Self::Remove => "Remove…",
        }
    }
}

fn state(parsers: &DocumentParsers, parser: Parser) -> State {
    let chosen = parsers.chosen() == parser;
    let mut state = State {
        facts: format!("Comes with Pistachio · License: {}", parser.license()),
        problem: parsers.error(parser).map(str::to_owned),
        progress: None,
        in_use: chosen,
        actions: Vec::new(),
    };
    if parser == Parser::Builtin {
        if !chosen {
            state.actions.push(Action::Use);
        }
        return state;
    }
    let installed = parsers.installed(parser);
    state.in_use = chosen && installed != Installed::No;
    let download = parser.download();
    state.facts = match &download {
        Ok(download) => format!(
            "{} · {} download, {} on disk · License: {}",
            download.version(),
            size_label(download.size()),
            size_label(download.disk_size()),
            parser.license()
        ),
        Err(_) => format!("License: {}", parser.license()),
    };
    if let Some((downloading, progress)) = parsers.downloading()
        && downloading == parser
    {
        state.progress = Some((
            progress.map_or(0., Progress::fraction),
            progress.map_or("Downloading", |progress| progress.stage.label()),
        ));
        state.actions.push(Action::Cancel);
        return state;
    }
    if installed == Installed::No {
        match download {
            Ok(_) => state.actions.push(Action::Download),
            Err(reason) => state.problem = Some(reason.to_owned()),
        }
        if chosen && state.problem.is_none() {
            state.problem = Some("Not downloaded: the built-in reader is used instead".into());
        }
        return state;
    }
    if !chosen {
        state.actions.push(Action::Use);
    }
    if installed == Installed::Outdated && download.is_ok() {
        state.actions.push(Action::Update);
    }
    state.actions.push(Action::Remove);
    state
}

fn rows(cx: &App) -> AnyElement {
    let Some(parsers) = existing(cx) else {
        return div().into_any_element();
    };
    let busy = parsers.downloading().is_some();
    let rows = Parser::ALL
        .into_iter()
        .map(|parser| row(parser, state(parsers, parser), busy, cx));
    v_flex().w_full().children(rows).into_any_element()
}

fn row(parser: Parser, state: State, busy: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = palette(cx).accent;
    let buttons = state.actions.into_iter().map(|action| {
        let button = Button::new(SharedString::from(format!(
            "parser-{}-{}",
            action.id(),
            parser.id()
        )))
        .label(action.label())
        .accessibility_label(format!("{} {}", action.label(), parser.label()))
        .xsmall()
        // One download at a time.
        .disabled(busy && matches!(action, Action::Download | Action::Update))
        .on_click(move |_, window, cx| action.run(parser, window, cx));
        match action {
            Action::Download => button.icon(AppIcon::Download).outline(),
            Action::Use | Action::Update => button.outline(),
            Action::Cancel | Action::Remove => button.ghost(),
        }
    });
    h_flex()
        .w_full()
        .gap_3()
        .py_2()
        .debug_selector(move || format!("parser-{}", parser.id()))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(parser.label()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(parser.strengths()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(state.facts),
                )
                .when_some(state.problem, |this, problem| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(palette(cx).warning)
                            .debug_selector(move || format!("parser-problem-{}", parser.id()))
                            .child(problem),
                    )
                })
                .when_some(state.progress, |this, (fraction, what)| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{what}… {:.0}%", fraction * 100.)),
                    )
                    .child(
                        div()
                            .mt_1()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(accent)
                                    .w(relative(fraction)),
                            ),
                    )
                }),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .when(state.in_use, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(accent)
                            .debug_selector(move || format!("parser-in-use-{}", parser.id()))
                            .child("In use"),
                    )
                })
                .children(buttons),
        )
}

/// Asks whether to download `parser`, showing what that is.
fn ask_to_download(parser: Parser, window: &mut Window, cx: &mut App) {
    open_form_dialog(None, window, cx, move |dialog, _, cx| {
        dialog
            .title(format!("Download {}?", parser.label()))
            .w(px(540.))
            .child(download_body(parser, cx))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(cancel_button("parser-download-cancel", cx))
                    .child(
                        Button::new("parser-download-start")
                            .form_action(cx)
                            .icon(AppIcon::Download)
                            .label("Download")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                document_parsers(cx)
                                    .update(cx, |parsers, cx| parsers.install(parser, cx));
                            }),
                    ),
            )
    });
}

fn cancel_button(id: &'static str, cx: &App) -> Button {
    Button::new(id)
        .form_other(cx)
        .icon(IconName::Close)
        .label("Cancel")
        .on_click(|_, window, cx| window.close_dialog(cx))
}

fn download_body(parser: Parser, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let Ok(download) = parser.download() else {
        return div();
    };
    let part = |icon: Icon, name: String, note: &'static str, size: u64| {
        h_flex()
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .child(icon.small().text_color(palette(cx).accent_icon))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().child(name))
                    .child(div().text_xs().text_color(muted).child(note)),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child(size_label(size)),
            )
    };
    let models = download.models_size();
    v_flex()
        .gap_3()
        .child(div().text_sm().text_color(muted).child(parser.strengths()))
        .child(
            v_flex()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .child(part(
                    AppIcon::Package.into(),
                    format!("{} {}", parser.label(), download.version()),
                    "The parser and the Python packages it is built on",
                    download.packages_size(),
                ))
                .when(models > 0, |this| {
                    this.child(part(
                        AppIcon::Brain.into(),
                        "Models".into(),
                        "From Hugging Face; they run on this computer",
                        models,
                    ))
                })
                .child(part(
                    AppIcon::CodeXml.into(),
                    format!("Python {}", python_env::catalog::PYTHON_VERSION),
                    "A private copy; your own Python is not touched",
                    download.platform.python_size,
                ))
                .child(part(
                    AppIcon::Wrench.into(),
                    format!("uv {}", python_env::catalog::UV_VERSION),
                    "Installs the rest, and is deleted afterwards",
                    download.platform.uv_size,
                ))
                .child(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .child(div().flex_1().child("Download"))
                        .child(format!(
                            "{} · {} on disk",
                            size_label(download.size()),
                            size_label(download.disk_size())
                        )),
                ),
        )
        .child(div().text_xs().text_color(muted).child(format!(
            "It goes into Pistachio's own folder, reads documents without using the network, and \
             you can remove it at any time. License: {}",
            parser.license_note()
        )))
}

/// Asks before deleting `parser`.
fn ask_to_remove(parser: Parser, window: &mut Window, cx: &mut App) {
    open_form_dialog(None, window, cx, move |dialog, _, cx| {
        let size = parser
            .download()
            .map(|download| format!(" (about {})", size_label(download.disk_size())))
            .unwrap_or_default();
        let instead = if existing(cx).is_some_and(|parsers| parsers.chosen() == parser) {
            " Documents are read with the built-in reader instead."
        } else {
            ""
        };
        dialog
            .title(format!("Remove {}?", parser.label()))
            .w(px(420.))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Everything that was downloaded for it is deleted{size}.{instead}"
                    )),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(cancel_button("parser-remove-cancel", cx))
                    .child(
                        Button::new("parser-remove")
                            .form_danger(cx)
                            .icon(AppIcon::Trash)
                            .label("Remove")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                document_parsers(cx)
                                    .update(cx, |parsers, cx| parsers.remove(parser, cx));
                            }),
                    ),
            )
    });
}

#[cfg(test)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod tests {
    use std::path::Path;

    use documents::config;
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
    use pretty_assertions::assert_eq;

    use super::{Action, Parser, existing, size_label, state};
    use crate::config::AppConfig;
    use crate::settings::AppSettings;
    use crate::settings::view::SettingsView;
    use crate::ui::dialog::testing as dialog;

    /// The Document parsing page, with its settings in `dir`.
    fn open<'a>(dir: &Path, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = AppConfig::for_tests("/nonexistent/pi".into());
            config.deepsearch_dir = dir.join("deepsearch");
            config.documents_dir = dir.join("documents");
            cx.set_global(config);
            cx.set_global(AppSettings::default());
            crate::search_server::init(cx);
            crate::stt::init(cx);
            crate::naming::init(cx);
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = SettingsView::new(Vec::new(), window, cx);
                view.start_on_documents();
                view
            });
            Root::new(view, window, cx)
        });
        dialog::draw(cx);
        cx
    }

    /// Makes `parser` look installed, of an older version.
    fn install(dir: &Path, parser: Parser) {
        let app = dir.join("documents/parsers").join(parser.id()).join("app");
        std::fs::create_dir_all(app.join("python/bin")).unwrap();
        std::fs::write(app.join("python/bin/python3"), "").unwrap();
        std::fs::write(app.join(".installed"), "older").unwrap();
    }

    fn actions(parser: Parser, cx: &VisualTestContext) -> Vec<Action> {
        cx.read(|cx| state(existing(cx).unwrap(), parser).actions)
    }

    fn in_use(cx: &mut VisualTestContext) -> Vec<Parser> {
        let selector = |parser: Parser| match parser {
            Parser::Builtin => "parser-in-use-builtin",
            Parser::Pymupdf4llm => "parser-in-use-pymupdf4llm",
            Parser::Docling => "parser-in-use-docling",
            Parser::Mineru => "parser-in-use-mineru",
        };
        Parser::ALL
            .into_iter()
            .filter(|parser| cx.debug_bounds(selector(*parser)).is_some())
            .collect()
    }

    /// Waits for the settings to be saved.
    fn saved(dir: &Path, cx: &mut VisualTestContext) -> Parser {
        dialog::draw(cx);
        std::thread::sleep(std::time::Duration::from_millis(100));
        config::load(&dir.join("documents")).parser
    }

    #[gpui_kit::test]
    fn the_built_in_reader_is_in_use_until_another_is_chosen(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), Parser::Pymupdf4llm);
        let cx = open(dir.path(), cx);
        assert_eq!(in_use(cx), [Parser::Builtin]);
        assert_eq!(
            actions(Parser::Pymupdf4llm, cx),
            [Action::Use, Action::Update, Action::Remove]
        );
        dialog::click("parser-use-pymupdf4llm", cx);
        assert_eq!(in_use(cx), [Parser::Pymupdf4llm]);
        assert_eq!(saved(dir.path(), cx), Parser::Pymupdf4llm);

        // Removing it asks first, and goes back to the built-in reader.
        for cancel in [true, false] {
            dialog::click("parser-remove-pymupdf4llm", cx);
            assert!(dialog::is_open(cx));
            if cancel {
                dialog::click("parser-remove-cancel", cx);
                assert_eq!(in_use(cx), [Parser::Pymupdf4llm]);
            } else {
                dialog::click("parser-remove", cx);
            }
            assert!(!dialog::is_open(cx));
        }
        assert_eq!(in_use(cx), [Parser::Builtin]);
        assert_eq!(saved(dir.path(), cx), Parser::Builtin);
        assert!(!dir.path().join("documents/parsers/pymupdf4llm").exists());
    }

    #[gpui_kit::test]
    fn a_download_is_asked_for(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        assert_eq!(actions(Parser::Builtin, cx), []);
        for parser in [Parser::Pymupdf4llm, Parser::Docling, Parser::Mineru] {
            let state = cx.read(|cx| state(existing(cx).unwrap(), parser));
            match parser.download() {
                Ok(download) => {
                    assert_eq!(state.actions, [Action::Download]);
                    assert!(state.facts.contains(&size_label(download.size())));
                }
                Err(reason) => {
                    assert_eq!(state.actions, []);
                    assert_eq!(state.problem.as_deref(), Some(reason));
                }
            }
            assert!(state.facts.contains(parser.license()));
        }
        if Parser::Pymupdf4llm.download().is_ok() {
            dialog::click("parser-download-pymupdf4llm", cx);
            assert!(dialog::is_open(cx));
            dialog::click("parser-download-cancel", cx);
            assert!(!dialog::is_open(cx));
            cx.read(|cx| assert_eq!(existing(cx).unwrap().downloading(), None));
        }
    }

    /// One that was chosen but is gone says what is used instead.
    #[gpui_kit::test]
    fn a_missing_parser_says_so(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let chosen = config::Config {
            parser: Parser::Pymupdf4llm,
            ..config::Config::default()
        };
        config::save(&dir.path().join("documents"), &chosen).unwrap();
        let cx = open(dir.path(), cx);
        assert_eq!(in_use(cx), []);
        assert!(cx.debug_bounds("parser-problem-pymupdf4llm").is_some());
        dialog::click("parser-use-builtin", cx);
        assert_eq!(in_use(cx), [Parser::Builtin]);
    }
}
