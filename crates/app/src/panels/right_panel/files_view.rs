//! The "Files" tab: a lazily loaded, gitignore-aware tree of the working folder.
//!
//! Directories are listed on the background executor when they become visible, and only the
//! listings affected by a filesystem change are reloaded. Rows are rendered by one
//! virtualized list.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::diff_view::placeholder;
use super::file_tree::{FileTree, Node, RootState, RowKind};
use crate::theme::palette;

const ROW_HEIGHT: Pixels = px(26.);
const INDENT: f32 = 14.;
/// Width of the chevron slot, so files line up with sibling folders' icons.
const CHEVRON_WIDTH: Pixels = px(12.);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesEvent {
    /// Mention this (absolute) path in the chat.
    Insert(PathBuf),
}

pub struct FilesView {
    tree: FileTree,
    selected: Option<PathBuf>,
    /// Listings in flight by directory; a newer load for the same directory replaces (and so
    /// cancels) the older one.
    loads: HashMap<PathBuf, Task<()>>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<FilesEvent> for FilesView {}

impl FilesView {
    /// Creates the view without loading anything; call [`Self::refresh`].
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            tree: FileTree::new(cwd),
            selected: None,
            loads: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Switches to another folder, dropping all state and loads in flight.
    pub fn set_cwd(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        *self = Self::new(cwd);
        cx.notify();
    }

    /// Reloads every visible directory (and loads the root the first time).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        for dir in self.tree.visible_dirs() {
            self.load(dir, cx);
        }
    }

    /// Reloads only the listings affected by the changed paths.
    pub fn on_changes(&mut self, changed: &[PathBuf], cx: &mut Context<Self>) {
        self.tree.prune_hidden();
        for dir in self.tree.affected_dirs(changed) {
            self.load(dir, cx);
        }
    }

    /// Number of visible rows.
    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.tree.rows().len()
    }

    fn load_missing(&mut self, cx: &mut Context<Self>) {
        for dir in self.tree.missing() {
            self.load(dir, cx);
        }
    }

    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let token = self.tree.begin_load(&dir);
        let root = self.tree.root().to_path_buf();
        let key = dir.clone();
        let task = cx.spawn(async move |this, cx| {
            let target = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    workspace::files::list_dir(&root, &target, false)
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loads.remove(&dir);
                if this.tree.finish_load(&dir, token, result) {
                    // Expanded subdirectories of a fresh listing may still need theirs.
                    this.load_missing(cx);
                    cx.notify();
                }
            });
        });
        self.loads.insert(key, task);
    }

    fn toggle(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if !self.tree.toggle(dir) {
            // Collapsed listings are reloaded fresh when shown again; don't keep them.
            self.tree.prune_hidden();
        }
        self.load_missing(cx);
        cx.notify();
    }

    fn click(&mut self, node: &Node, event: &ClickEvent, cx: &mut Context<Self>) {
        self.selected = Some(node.path.clone());
        if node.is_dir {
            self.toggle(&node.path, cx);
        } else {
            if event.click_count() >= 2 {
                cx.emit(FilesEvent::Insert(node.path.clone()));
            }
            cx.notify();
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (foreground, muted, hover, selected_bg) = (
            theme.foreground,
            theme.muted_foreground,
            theme.secondary_hover,
            theme.accent,
        );
        let error = palette(cx).error;
        let rows = self.tree.rows();
        range
            .filter_map(|ix| {
                let row = rows.get(ix)?;
                let indent = px(8. + INDENT * f32::from(row.depth));
                let base = h_flex().h(ROW_HEIGHT).w_full().pl(indent).pr_1().gap_1p5();
                Some(match &row.kind {
                    RowKind::Node { node, expanded, .. } => {
                        let selected = self.selected.as_deref() == Some(node.path.as_path());
                        let (clicked, inserted) = (node.clone(), node.clone());
                        let (chevron, icon) = if node.is_dir {
                            let chevron = if *expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            };
                            let folder = if *expanded {
                                IconName::FolderOpen
                            } else {
                                IconName::Folder
                            };
                            (Some(chevron), folder)
                        } else {
                            (None, IconName::File)
                        };
                        base.id(("file-row", ix))
                            .group("file-row")
                            .cursor_pointer()
                            .when(selected, |this| this.bg(selected_bg))
                            .hover(|style| style.bg(hover))
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.click(&clicked, event, cx);
                            }))
                            .child(
                                div().flex_none().w(CHEVRON_WIDTH).children(
                                    chevron.map(|chevron| {
                                        Icon::new(chevron).xsmall().text_color(muted)
                                    }),
                                ),
                            )
                            .child(Icon::new(icon).small().text_color(muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(node.name.clone()),
                            )
                            .child(
                                div()
                                    .invisible()
                                    .group_hover("file-row", |style| style.visible())
                                    .child(
                                        Button::new(("file-insert", ix))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Plus)
                                            .tooltip("Mention in chat")
                                            .on_click(cx.listener(move |_, _, window, cx| {
                                                window.prevent_default();
                                                cx.stop_propagation();
                                                cx.emit(FilesEvent::Insert(inserted.path.clone()));
                                            })),
                                    ),
                            )
                            .into_any_element()
                    }
                    RowKind::Empty => base
                        .pl(indent + CHEVRON_WIDTH + px(6.))
                        .text_xs()
                        .italic()
                        .text_color(muted)
                        .child("Empty")
                        .into_any_element(),
                    RowKind::Error(message) => base
                        .pl(indent + CHEVRON_WIDTH + px(6.))
                        .text_xs()
                        .text_color(error)
                        .child(div().truncate().child(message.clone()))
                        .into_any_element(),
                })
            })
            .collect()
    }
}

impl Render for FilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.tree.root_state() {
            RootState::Loading => placeholder("Loading files…".into(), None, cx),
            RootState::Failed(error) => placeholder(
                "Could not list this folder".into(),
                Some(SharedString::from(error.to_owned())),
                cx,
            ),
            RootState::Ready if self.tree.rows().is_empty() => {
                placeholder("This folder is empty".into(), None, cx)
            }
            RootState::Ready => div()
                .relative()
                .size_full()
                .child(
                    uniform_list(
                        "file-rows",
                        self.tree.rows().len(),
                        cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
                    )
                    .size_full()
                    .py_1()
                    .track_scroll(&self.scroll),
                )
                .child(Scrollbar::vertical(&self.scroll))
                .into_any_element(),
        }
    }
}

/// `path` relative to `root`, for chat mentions (unchanged when outside it).
pub fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::relative_to;

    #[test]
    fn mentions_are_relative_to_the_root() {
        assert_eq!(
            relative_to(Path::new("/r"), Path::new("/r/src/a.rs")),
            PathBuf::from("src/a.rs")
        );
        assert_eq!(
            relative_to(Path::new("/r"), Path::new("/elsewhere/a")),
            PathBuf::from("/elsewhere/a")
        );
    }
}
