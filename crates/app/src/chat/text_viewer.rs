//! The full-window viewer of text files: with line numbers and the colors of the Code setting.
//! Text can be selected, copied and searched, and not changed.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState, Escape};
use gpui_kit::component::{ActiveTheme as _, IconName, Selectable as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::document::AttachedDocument;
use super::viewer_dialog;
use crate::code_highlight::language_for_path;
use crate::ui::{AppIcon, QuietButton as _};

/// Of a longer file, only the start is shown.
const MAX_SHOWN: usize = 2 * 1024 * 1024;
/// Longer text has no colors: parsing it takes a while.
const MAX_COLORED: u64 = 512 * 1024;

/// Opens the viewer on the text file at `path`. Esc or a click outside closes it.
pub fn open(path: PathBuf, window: &mut Window, cx: &mut App) {
    let viewer = cx.new(|cx| TextViewer::new(path, window, cx));
    let focus = viewer.read(cx).editor.focus_handle(cx);
    viewer_dialog::open(viewer, focus, window, cx);
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    Opening,
    Failed(SharedString),
    Open { lines: usize, cut: bool },
}

struct TextViewer {
    path: PathBuf,
    editor: Entity<EditorState>,
    wrap: bool,
    state: State,
    _opening: Task<()>,
}

impl TextViewer {
    fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wrap = wraps(&path);
        let is_short = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_COLORED);
        let language = language_for_path(&path).filter(|_| is_short);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language.unwrap_or("text"))
                .line_number(true)
                .searchable(true)
                .soft_wrap(wrap)
        });
        let read = cx.background_executor().spawn({
            let path = path.clone();
            async move { AttachedDocument::read_plain(&path).map(|file| shown(file.text())) }
        });
        let opening = cx.spawn_in(window, async move |this, cx| {
            let read = read.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.state = match read {
                    Ok((text, cut)) => {
                        let lines = text.lines().count();
                        this.editor.update(cx, |editor, cx| {
                            editor.set_value(text, window, cx);
                        });
                        State::Open { lines, cut }
                    }
                    Err(reason) => State::Failed(format!("It cannot be shown: {reason}.").into()),
                };
                cx.notify();
            });
        });
        Self {
            path,
            editor,
            wrap,
            state: State::Opening,
            _opening: opening,
        }
    }

    fn toggle_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.editor.update(cx, |editor, cx| {
            editor.set_soft_wrap(self.wrap, window, cx);
        });
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let open = matches!(self.state, State::Open { .. });
        viewer_dialog::toolbar(cx)
            .child(viewer_dialog::name(&self.path))
            .when_some(self.lines(), |this, lines| {
                this.child(div().px_2().text_xs().text_color(muted).child(lines))
            })
            .when(open, |this| {
                this.child(
                    viewer_dialog::button("text-wrap", AppIcon::TextWrap, "Wrap lines", cx)
                        .selected(self.wrap)
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_wrap(window, cx))),
                )
            })
            .child(
                viewer_dialog::button("text-elsewhere", IconName::FolderOpen, "Show in folder", cx)
                    .on_click(cx.listener(|this, _, _, cx| cx.reveal_path(&this.path))),
            )
            .child(viewer_dialog::close(cx))
    }

    fn lines(&self) -> Option<SharedString> {
        match self.state {
            State::Open { lines, cut: true } => Some(format!("First {lines} lines").into()),
            State::Open { lines: 1, .. } => Some("1 line".into()),
            State::Open { lines, .. } => Some(format!("{lines} lines").into()),
            _ => None,
        }
    }
}

impl Render for TextViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let area = viewer_dialog::area(window, cx);
        let body = match &self.state {
            // Hardly seen.
            State::Opening => div().flex_1().into_any_element(),
            State::Failed(error) => viewer_dialog::message(
                error.clone(),
                Button::new("text-folder")
                    .form_other(cx)
                    .label("Show in folder")
                    .on_click(cx.listener(|this, _, _, cx| cx.reveal_path(&this.path))),
                cx,
            )
            .into_any_element(),
            State::Open { .. } => div()
                .debug_selector(|| "text-viewer".into())
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(&self.editor)
                        .readonly(true)
                        .appearance(false)
                        .aria_label("Text")
                        .size_full(),
                )
                .into_any_element(),
        };
        v_flex()
            // What the editor does not need Esc for closes the viewer.
            .on_action(|_: &Escape, window, cx| window.close_dialog(cx))
            .w(area.width)
            .h(area.height)
            .child(self.render_toolbar(cx))
            .child(body)
    }
}

/// `text`, or the whole lines at its start, and whether there is more.
fn shown(text: &str) -> (String, bool) {
    if text.len() <= MAX_SHOWN {
        return (text.to_owned(), false);
    }
    let mut end = MAX_SHOWN;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |at| at + 1);
    (text[..end].to_owned(), true)
}

/// Prose reads better wrapped; code and tables do not.
fn wraps(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "" | "txt" | "md" | "markdown" | "tex" | "rst" | "org" | "adoc" | "rtf" | "eml" | "srt"
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::{Root, WindowExt as _};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
        Window, div,
    };
    use pretty_assertions::assert_eq;

    use super::{MAX_SHOWN, Path, State, TextViewer, shown, wraps};

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn window(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| Empty);
            Root::new(empty, window, cx)
        });
        cx
    }

    fn viewer(path: &Path, cx: &mut VisualTestContext) -> Entity<TextViewer> {
        let path = path.to_path_buf();
        let viewer = cx.update(|window, cx| cx.new(|cx| TextViewer::new(path, window, cx)));
        cx.run_until_parked();
        viewer
    }

    #[gpui_kit::test]
    fn text_is_shown(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brev.tex");
        std::fs::write(&path, b"Bl\xe5b\xe6r og gr\xf8d\nkoster 5 kr. om \xe5ret\n").unwrap();
        let cx = window(cx);
        let text = viewer(&path, cx);
        text.update_in(cx, |viewer, window, cx| {
            let state = State::Open {
                lines: 2,
                cut: false,
            };
            assert_eq!(viewer.state, state);
            let shown = viewer.editor.read(cx).value();
            assert_eq!(shown, "Blåbær og grød\nkoster 5 kr. om året\n");
            assert!(viewer.wrap);
            viewer.toggle_wrap(window, cx);
            assert!(!viewer.wrap);
        });
        let other = dir.path().join("data.bin");
        std::fs::write(&other, b"\0\x01\x02\xff").unwrap();
        for (path, reason) in [
            (other, "not a text file"),
            (dir.path().join("missing"), "could not be read"),
        ] {
            let failed = viewer(&path, cx);
            let state = State::Failed(format!("It cannot be shown: {reason}.").into());
            assert_eq!(
                failed.read_with(cx, |viewer, _| viewer.state.clone()),
                state
            );
        }
    }

    /// Esc closes it, and typing changes nothing.
    #[gpui_kit::test]
    fn it_opens_and_closes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "text").unwrap();
        let cx = window(cx);
        cx.update(|window, cx| super::open(path.clone(), window, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("text-viewer").is_some());
        cx.simulate_keystrokes("x enter backspace");
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "text");
    }

    #[test]
    fn long_text_is_cut_between_lines() {
        assert_eq!(shown("a\nb"), ("a\nb".into(), false));
        let text = "a line of it\n".repeat(MAX_SHOWN / 10);
        let (start, cut) = shown(&text);
        assert!(cut && start.len() <= MAX_SHOWN && start.ends_with("it\n"));
        let (start, cut) = shown(&"ø".repeat(MAX_SHOWN));
        assert!(cut && start.len() <= MAX_SHOWN);
    }

    #[test]
    fn prose_is_wrapped() {
        for (name, wrapped) in [
            ("a.TXT", true),
            ("README", true),
            ("a.tex", true),
            ("a.csv", false),
            ("a.rs", false),
            ("server.log", false),
        ] {
            assert_eq!(wraps(Path::new(name)), wrapped, "{name}");
        }
    }
}
