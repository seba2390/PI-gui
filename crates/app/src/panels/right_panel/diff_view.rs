//! The "Diff" tab: changed files of the working tree, each expandable into its patch.
//!
//! Git runs on the background executor. Status loads are coalesced: at most one is in flight,
//! requests meanwhile queue exactly one more, which starts after a short cooldown so a stream
//! of filesystem events (say, a build writing ignored files) can't keep git busy. Every patch
//! load carries a token, so late results never overwrite newer ones. Rows are rendered by one
//! virtualized list.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::diff_lines::{self, LineKind, ParsedDiff};
use super::diff_model::{DiffModel, DiffRow, FileEntry, LoadedStatus, Note, Phase, Summary, Tone};
use crate::theme::palette;

const FILE_ROW_HEIGHT: Pixels = px(30.);
const LINE_HEIGHT: Pixels = px(20.);
/// Minimum pause between a status load and a queued one.
const STATUS_COOLDOWN: Duration = Duration::from_millis(500);
/// Approximate advance of one monospace digit at `text_xs`, to size the line-number gutter.
const DIGIT_WIDTH: f32 = 7.5;

/// Colours resolved once per batch of rows.
struct Colors {
    foreground: Hsla,
    muted: Hsla,
    subtle: Hsla,
    border: Hsla,
    hover: Hsla,
    hunk_bg: Hsla,
    added: Hsla,
    added_bg: Hsla,
    removed: Hsla,
    removed_bg: Hsla,
    modified: Hsla,
    renamed: Hsla,
    error: Hsla,
    mono: SharedString,
}

impl Colors {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        let palette = palette(cx);
        Self {
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            subtle: palette.subtle,
            border: theme.border,
            hover: theme.secondary_hover,
            hunk_bg: palette.code_bg,
            added: palette.added,
            added_bg: palette.added_bg,
            removed: palette.removed,
            removed_bg: palette.removed_bg,
            modified: palette.warning,
            renamed: palette.accent,
            error: palette.error,
            mono: theme.mono_font_family.clone(),
        }
    }

    fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Added => self.added,
            Tone::Modified => self.modified,
            Tone::Removed => self.removed,
            Tone::Renamed => self.renamed,
            Tone::Conflict => self.error,
            Tone::Other => self.muted,
        }
    }
}

pub struct DiffView {
    cwd: PathBuf,
    model: DiffModel,
    /// Row heights for the virtual list, rebuilt whenever the rows change.
    sizes: Rc<Vec<Size<Pixels>>>,
    scroll: VirtualListScrollHandle,
    /// The status load (or the cooldown before a queued one) in flight, if any.
    status_task: Option<Task<()>>,
    /// A refresh was requested while `status_task` was running.
    status_queued: bool,
    patch_tasks: HashMap<PathBuf, Task<()>>,
}

impl DiffView {
    /// Creates the view without loading anything; call [`Self::refresh`].
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            model: DiffModel::default(),
            sizes: Rc::default(),
            scroll: VirtualListScrollHandle::new(),
            status_task: None,
            status_queued: false,
            patch_tasks: HashMap::new(),
        }
    }

    pub fn summary(&self) -> Option<&Summary> {
        self.model.summary()
    }

    /// Switches to another folder, dropping all state and loads in flight.
    pub fn set_cwd(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        *self = Self::new(cwd);
        cx.notify();
    }

    /// Re-reads the git status (and then every expanded patch).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.status_task.is_some() {
            self.status_queued = true;
            return;
        }
        let cwd = self.cwd.clone();
        self.status_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    workspace::git::status(&cwd)
                        .map(|repo| repo.map(LoadedStatus::new))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_task = None;
                let reload = this.model.set_status(result);
                this.patch_tasks
                    .retain(|path, _| this.model.is_expanded(path));
                for path in reload {
                    this.load_patch(path, cx);
                }
                this.sync(cx);
                if std::mem::take(&mut this.status_queued) {
                    this.status_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(STATUS_COOLDOWN).await;
                        let _ = this.update(cx, |this, cx| {
                            // Requests during the cooldown are covered by this load.
                            this.status_queued = false;
                            this.status_task = None;
                            this.refresh(cx);
                        });
                    }));
                }
            });
        }));
    }

    /// Number of rows (files, patch lines and notes).
    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.model.rows().len()
    }

    /// Expands or collapses a file, loading its patch when expanded.
    pub(super) fn toggle(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.model.toggle(path) {
            self.load_patch(path.to_path_buf(), cx);
        } else {
            self.patch_tasks.remove(path);
        }
        self.sync(cx);
    }

    /// Loads (or reloads) the patch of an expanded file. A load already in flight for the
    /// same file is cancelled.
    fn load_patch(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(request) = self.model.begin_load(&path) else {
            return;
        };
        let key = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    workspace::git::change_diff(&request.repo, &request.change)
                        .map(|patch| ParsedDiff::parse(&patch, diff_lines::MAX_LINES))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.patch_tasks.remove(&path);
                if this.model.finish_load(&path, request.token, result) {
                    this.sync(cx);
                }
            });
        });
        self.patch_tasks.insert(key, task);
    }

    /// Recomputes row heights after the rows changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.sizes = Rc::new(
            self.model
                .rows()
                .iter()
                .map(|row| {
                    let height = match row {
                        DiffRow::File { .. } => FILE_ROW_HEIGHT,
                        DiffRow::Line { .. } | DiffRow::Note(_) => LINE_HEIGHT,
                    };
                    size(px(0.), height)
                })
                .collect(),
        );
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = Colors::new(cx);
        let rows = self.model.rows();
        range
            .filter_map(|ix| {
                Some(match rows.get(ix)? {
                    DiffRow::File {
                        entry,
                        expanded,
                        separated,
                    } => render_file(ix, entry, *expanded, *separated, &colors, cx),
                    DiffRow::Line { diff, ix: line } => render_line(diff, *line, &colors),
                    DiffRow::Note(note) => render_note(note, &colors),
                })
            })
            .collect()
    }
}

fn render_file(
    ix: usize,
    entry: &Rc<FileEntry>,
    expanded: bool,
    separated: bool,
    colors: &Colors,
    cx: &mut Context<DiffView>,
) -> AnyElement {
    let clicked = entry.clone();
    h_flex()
        .id(("diff-file", ix))
        .h(FILE_ROW_HEIGHT)
        .w_full()
        .px_2()
        .gap_1p5()
        .cursor_pointer()
        .when(separated, |this| {
            this.border_t_1().border_color(colors.border)
        })
        .hover(|style| style.bg(colors.hover))
        .on_click(cx.listener(move |this, _, _, cx| this.toggle(clicked.path(), cx)))
        .child(
            Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(colors.muted),
        )
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .text_xs()
                .font_family(colors.mono.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.tone(entry.tone))
                .child(entry.letter.clone()),
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
                        .max_w(relative(0.75))
                        .truncate()
                        .text_sm()
                        .text_color(colors.foreground)
                        .child(entry.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(entry.detail.clone()),
                ),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .text_xs()
                .font_family(colors.mono.clone())
                .children(
                    entry
                        .additions
                        .clone()
                        .map(|text| div().text_color(colors.added).child(text)),
                )
                .children(
                    entry
                        .deletions
                        .clone()
                        .map(|text| div().text_color(colors.removed).child(text)),
                ),
        )
        .into_any_element()
}

fn render_line(diff: &ParsedDiff, ix: usize, colors: &Colors) -> AnyElement {
    let Some(line) = diff.lines.get(ix) else {
        return div().into_any_element();
    };
    let digits = u8::try_from(diff.number_digits).unwrap_or(u8::MAX);
    let gutter = px(DIGIT_WIDTH * f32::from(digits) + 16.);
    let (background, marker, text) = match line.kind {
        LineKind::Added => (Some(colors.added_bg), colors.added, colors.foreground),
        LineKind::Removed => (Some(colors.removed_bg), colors.removed, colors.foreground),
        LineKind::Hunk => (Some(colors.hunk_bg), colors.muted, colors.muted),
        LineKind::Context => (None, colors.muted, colors.foreground),
        LineKind::Note => (None, colors.muted, colors.muted),
    };
    // Removed lines only exist in the old file; everything else is numbered as in the new one.
    let number = if line.kind == LineKind::Removed {
        line.old
    } else {
        line.new
    };
    h_flex()
        .h(LINE_HEIGHT)
        .w_full()
        .overflow_hidden()
        .font_family(colors.mono.clone())
        .text_xs()
        .when_some(background, |this, background| this.bg(background))
        .child(
            div()
                .flex_none()
                .w(gutter)
                .pr_2()
                .text_right()
                .text_color(colors.subtle)
                .children(number.map(|number| number.to_string())),
        )
        .child(
            div()
                .flex_none()
                .w(px(14.))
                .text_color(marker)
                .child(line.kind.sign()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(text)
                .when(line.kind == LineKind::Note, |this| this.italic())
                .child(line.text.clone()),
        )
        .into_any_element()
}

fn render_note(note: &Note, colors: &Colors) -> AnyElement {
    let (text, color): (SharedString, Hsla) = match note {
        Note::Loading => ("Loading diff…".into(), colors.muted),
        Note::Binary => ("Binary file not shown".into(), colors.muted),
        Note::NoText => ("No textual changes".into(), colors.muted),
        Note::Hidden(count) => {
            let noun = if *count == 1 { "line" } else { "lines" };
            (format!("{count} more {noun} hidden").into(), colors.muted)
        }
        Note::Error(error) => (format!("Could not load diff: {error}").into(), colors.error),
    };
    h_flex()
        .h(LINE_HEIGHT)
        .w_full()
        .pl(px(30.))
        .overflow_hidden()
        .text_xs()
        .italic()
        .text_color(color)
        .child(div().truncate().child(text))
        .into_any_element()
}

/// A centred message filling the tab, for empty, loading and error states.
pub fn placeholder(title: SharedString, detail: Option<SharedString>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .px_6()
        .text_center()
        .child(div().text_sm().text_color(theme.foreground).child(title))
        .children(detail.map(|detail| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(detail)
        }))
        .into_any_element()
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.model.phase() {
            Phase::Loading => placeholder("Loading changes…".into(), None, cx),
            Phase::NotRepo => placeholder(
                "Not a git repository".into(),
                Some("Changes appear here when the folder is tracked by git.".into()),
                cx,
            ),
            Phase::Failed(error) => {
                placeholder("Could not read git status".into(), Some(error.clone()), cx)
            }
            Phase::Ready if self.model.rows().is_empty() => placeholder(
                "No changes".into(),
                Some("The working tree is clean.".into()),
                cx,
            ),
            Phase::Ready => div()
                .relative()
                .size_full()
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "diff-rows",
                        self.sizes.clone(),
                        |this, range, _, cx| this.render_rows(range, cx),
                    )
                    .track_scroll(&self.scroll),
                )
                .child(Scrollbar::vertical(&self.scroll))
                .into_any_element(),
        }
    }
}
