//! A read-only view of one file in the "Files" tab: syntax colors (the user's Code colors)
//! and line numbers, in gpui-kit's code editor set to read-only, so text can still be
//! selected and copied.
//!
//! The file is read on the background executor through the same [`workspace::Location`] as
//! the tree, so it works for folders on an SSH host. Only the first
//! [`workspace::viewer::MAX_VIEW_BYTES`] are read: a longer file opens cut, with a notice,
//! and a binary file shows a notice instead of its bytes.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use workspace::viewer::{self, FileContent};

use super::diff_view::placeholder;
use crate::ui::QuietButton as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileViewerEvent {
    /// Back to the tree.
    Close,
    /// Mention this (absolute) path in the chat.
    Insert(PathBuf),
}

/// What the viewer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Loading,
    Text { truncated: bool, size: u64 },
    Binary { size: u64 },
    Failed(SharedString),
}

pub struct FileViewer {
    root: PathBuf,
    path: PathBuf,
    location: workspace::Location,
    /// `path` relative to the folder, as shown in the header.
    title: SharedString,
    phase: Phase,
    editor: Entity<EditorState>,
    /// The file changed on disk; it is read again when next rendered (so a viewer in a
    /// hidden tab does no work).
    stale: bool,
    _load: Task<()>,
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

impl FileViewer {
    pub fn new(
        root: PathBuf,
        path: PathBuf,
        location: workspace::Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = crate::code_highlight::language_for_path(&path).unwrap_or("text");
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                // Code reads better unwrapped; long lines scroll sideways.
                .soft_wrap(false)
        });
        let title = super::files_view::relative_to(&root, &path)
            .to_string_lossy()
            .into_owned()
            .into();
        let mut this = Self {
            root,
            path,
            location,
            title,
            phase: Phase::Loading,
            editor,
            stale: false,
            _load: Task::ready(()),
        };
        this.reload(window, cx);
        this
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file changed on disk.
    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        cx.notify();
    }

    /// Reads the file (again).
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (root, path, location) = (self.root.clone(), self.path.clone(), self.location.clone());
        self._load = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    viewer::read_head_at(&location, &root, &path, viewer::MAX_VIEW_BYTES)
                        .map(|head| (head.size, FileContent::classify(&head)))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.phase = match result {
                    Ok((size, FileContent::Text { text, truncated })) => {
                        this.editor
                            .update(cx, |editor, cx| editor.set_value(text, window, cx));
                        Phase::Text { truncated, size }
                    }
                    Ok((size, FileContent::Binary)) => Phase::Binary { size },
                    Err(error) => Phase::Failed(error.into()),
                };
                cx.notify();
            });
        });
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let name = self.path.file_name().map_or_else(
            || self.title.to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let parent = Path::new(self.title.as_ref())
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .h(px(32.))
            .flex_none()
            .w_full()
            .pl_1()
            .pr_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("file-viewer-back")
                    .quiet(cx)
                    .xsmall()
                    .icon(IconName::ArrowLeft)
                    .accessibility_label("Back to files")
                    .tooltip("Back to files")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FileViewerEvent::Close))),
            )
            .child(
                Icon::new(IconName::File)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.7))
                            .truncate()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(parent),
                    ),
            )
            .child(
                Button::new("file-viewer-insert")
                    .quiet(cx)
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("Mention")
                    .accessibility_label("Mention in chat")
                    .tooltip("Mention this file in the chat (⌥-click in the tree)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(FileViewerEvent::Insert(this.path.clone()));
                    })),
            )
    }
}

/// A file size for notices, e.g. `3.2 MB`.
fn format_size(size: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let bytes = size as f64;
    if size < 1024 {
        format!("{size} bytes")
    } else if bytes < 1024. * 1024. {
        format!("{:.0} KB", bytes / 1024.)
    } else if bytes < 1024. * 1024. * 1024. {
        format!("{:.1} MB", bytes / (1024. * 1024.))
    } else {
        format!("{:.1} GB", bytes / (1024. * 1024. * 1024.))
    }
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.stale) {
            self.reload(window, cx);
        }
        let theme = cx.theme();
        let (muted, warning_bg) = (theme.muted_foreground, theme.muted);
        let body = match &self.phase {
            Phase::Loading => placeholder("Opening…".into(), None, cx),
            Phase::Failed(error) => {
                placeholder("Could not open this file".into(), Some(error.clone()), cx)
            }
            Phase::Binary { size } => placeholder(
                "Binary file".into(),
                Some(format!("{} · not shown as text", format_size(*size)).into()),
                cx,
            ),
            Phase::Text { truncated, size } => v_flex()
                .size_full()
                .when(*truncated, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px_3()
                            .py_1()
                            .bg(warning_bg)
                            .text_xs()
                            .text_color(muted)
                            .child(format!(
                                "Large file ({}): showing the first {}.",
                                format_size(*size),
                                format_size(viewer::MAX_VIEW_BYTES as u64)
                            )),
                    )
                })
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&self.editor)
                            .readonly(true)
                            .appearance(false)
                            .aria_label(self.title.clone())
                            .size_full(),
                    ),
                )
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(format_size(12), "12 bytes");
        assert_eq!(format_size(512 * 1024), "512 KB");
        assert_eq!(format_size(3 * 1024 * 1024 + 300 * 1024), "3.3 MB");
    }
}
