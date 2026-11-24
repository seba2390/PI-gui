//! Find in chat (⌘F): a search bar over the conversation.
//!
//! Every match is highlighted, the current one more strongly; Enter / Shift+Enter (or ⌘G /
//! ⌘⇧G) move between them, Esc closes the bar. Prompts and replies are searched as rendered
//! (the markdown views paint the highlights themselves); thinking, tool output and edit
//! diffs as the plain text their rows show ([`transcript::search::plain_fields`]), so a
//! match in a collapsed row is found too, and the row opens when the match becomes current.
//!
//! The search runs when the query changes and when the transcript does while the bar is
//! open; matching itself lives in the `transcript` crate.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::text::RangeHighlight;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use transcript::explore::RowRole;
use transcript::search::{self, Field};
use transcript::{BlockKind, ItemKind};

use super::{BlockKey, ChatView};
use crate::theme::palette;
use crate::ui::list::ListOffset;

actions!(chat, [FindNext, FindPrevious]);

/// Key context of the find bar.
const FIND_CONTEXT: &str = "ChatFind";

/// Line height of tool output and diffs (`text_xs`, monospace), to scroll near a match
/// inside a tall row.
const CODE_LINE_HEIGHT: f32 = 16.;
/// Room left above a match scrolled to inside a row.
const SCROLL_CONTEXT: f32 = 96.;

pub fn init(cx: &mut App) {
    let (next, previous) = if cfg!(target_os = "macos") {
        ("cmd-g", "cmd-shift-g")
    } else {
        ("ctrl-g", "ctrl-shift-g")
    };
    cx.bind_keys([
        KeyBinding::new(next, FindNext, Some(super::CHAT_CONTEXT)),
        KeyBinding::new(previous, FindPrevious, Some(super::CHAT_CONTEXT)),
    ]);
}

/// Plain-text matches by block and field, each with whether it is the current one.
type PlainMatches = HashMap<(BlockKey, Field), Vec<(Range<usize>, bool)>>;

/// Where in the transcript a match is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FindMatch {
    /// The transcript item (row index).
    row: usize,
    key: BlockKey,
    /// `None` for a markdown block (ranges index its rendered text).
    field: Option<Field>,
    range: Range<usize>,
}

/// The open find bar.
pub(super) struct Find {
    input: Entity<InputState>,
    query: String,
    matches: Vec<FindMatch>,
    current: Option<usize>,
    /// Plain-text matches by block and field, with whether each is the current one, for the
    /// rows to paint.
    plain: PlainMatches,
    /// Markdown views given highlights, so they can be cleared.
    highlighted: HashSet<BlockKey>,
    _subscription: Subscription,
}

impl ChatView {
    /// Opens the find bar (or focuses it again) with its text selected.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in chat"));
            let subscription =
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.on_find_query_changed(cx),
                        InputEvent::PressEnter { shift, .. } => {
                            this.find_step(!*shift, window, cx);
                        }
                        _ => {}
                    }
                });
            self.find = Some(Find {
                input,
                query: String::new(),
                matches: Vec::new(),
                current: None,
                plain: HashMap::new(),
                highlighted: HashSet::new(),
                _subscription: subscription,
            });
        }
        if let Some(find) = &self.find {
            let input = find.input.clone();
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        cx.notify();
    }

    /// Opens the bar searching for `text` (`PISTACHIO_OPEN=find:<text>`, for layout checks).
    pub fn find_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
        if let Some(find) = &self.find {
            let input = find.input.clone();
            input.update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
        }
        self.on_find_query_changed(cx);
    }

    /// Closes the bar, removing every highlight, and returns to the message box.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.take() else {
            return;
        };
        self.clear_markdown_highlights(&find.highlighted, cx);
        let current_row = find
            .current
            .and_then(|ix| find.matches.get(ix))
            .map(|found| self.display_row(found.row));
        if let Some(row) = current_row {
            // Its output box may have grown to show the match.
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(row..row + 1, cx)
            });
        }
        self.composer_focus(cx).focus(window, cx);
        cx.notify();
    }

    /// Searches the query as it is typed, starting from the first match on screen.
    fn on_find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.query = find.input.read(cx).value().to_string();
        self.run_find(cx);
        let top = self.scroller.read(cx).logical_scroll_top().item_ix;
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let current = find
            .matches
            .iter()
            .position(|found| found.row >= top)
            .or((!find.matches.is_empty()).then_some(0));
        self.set_current(current, true, cx);
    }

    /// Searches again after the transcript changed, keeping the current match if it is
    /// still there (without scrolling: the user may be reading).
    pub(super) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        if find.query.trim().is_empty() {
            return;
        }
        let previous = find.current.and_then(|ix| find.matches.get(ix)).cloned();
        let previous_ix = find.current;
        self.run_find(cx);
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let current = previous
            .and_then(|previous| find.matches.iter().position(|found| *found == previous))
            .or_else(|| previous_ix.map(|ix| ix.min(find.matches.len().saturating_sub(1))))
            .filter(|_| !find.matches.is_empty());
        // The first matches (text that streamed in, or a session that finished loading)
        // are revealed like a typed query's; later updates leave the scroll position alone.
        let first = previous_ix.is_none() && current.is_none() && !find.matches.is_empty();
        if first {
            self.on_find_query_changed(cx);
            return;
        }
        self.set_current(current, false, cx);
    }

    /// Next (or previous) match, wrapping around.
    fn find_step(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let current = search::step(find.current, find.matches.len(), forward);
        self.set_current(current, true, cx);
    }

    fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(true, window, cx);
    }

    fn find_previous(&mut self, _: &FindPrevious, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(false, window, cx);
    }

    /// Finds every match of the query, in screen order.
    fn run_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let query = find.query.clone();
        let mut matches = Vec::new();
        let session = self.session.read(cx);
        for (row, item) in session.transcript().items().iter().enumerate() {
            let mut plain = search::plain_fields(item);
            let markdown_blocks: Vec<usize> = match &item.kind {
                ItemKind::User(_) => vec![0],
                ItemKind::Assistant(assistant) => assistant
                    .blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, block)| matches!(block.kind, BlockKind::Text(_)))
                    .map(|(index, _)| index)
                    .collect(),
                _ => Vec::new(),
            };
            // Merge markdown and plain fields by block index (screen order).
            let mut fields: Vec<(usize, Option<Field>, String)> = plain
                .drain(..)
                .map(|(block, field, text)| (block, Some(field), text.into_owned()))
                .collect();
            for block in markdown_blocks {
                let key = BlockKey {
                    item: item.id,
                    block,
                };
                if let Some(markdown) = self.markdown.get(&key) {
                    let text = markdown.state.read(cx).rendered_text();
                    fields.push((block, None, text.as_str().to_owned()));
                }
            }
            fields.sort_by_key(|(block, _, _)| *block);
            for (block, field, text) in fields {
                let key = BlockKey {
                    item: item.id,
                    block,
                };
                matches.extend(
                    search::find_matches(&text, &query)
                        .into_iter()
                        .map(|range| FindMatch {
                            row,
                            key,
                            field,
                            range,
                        }),
                );
            }
        }
        if let Some(find) = self.find.as_mut() {
            find.matches = matches;
            find.current = None;
        }
    }

    /// Makes `current` the current match: repaints the highlights and, with `reveal`, opens
    /// its row and scrolls to it.
    fn set_current(&mut self, current: Option<usize>, reveal: bool, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let old_row = find
            .current
            .and_then(|ix| find.matches.get(ix))
            .map(|found| found.row);
        find.current = current;
        find.plain.clear();
        for (ix, found) in find.matches.iter().enumerate() {
            if let Some(field) = found.field {
                find.plain
                    .entry((found.key, field))
                    .or_default()
                    .push((found.range.clone(), Some(ix) == current));
            }
        }
        self.paint_markdown_highlights(cx);
        let current_match = self
            .find
            .as_ref()
            .and_then(|find| find.current.and_then(|ix| find.matches.get(ix)).cloned());
        // Rows whose output box grows or shrinks with the current match are measured again.
        let rows: Vec<usize> = old_row
            .into_iter()
            .chain(current_match.as_ref().map(|found| found.row))
            .map(|row| self.display_row(row))
            .collect();
        self.scroller.update(cx, |scroller, cx| {
            for row in rows {
                scroller.remeasure_items(row..row + 1, cx);
            }
        });
        if reveal && let Some(found) = current_match {
            self.reveal_match(&found, cx);
        }
        cx.notify();
    }

    /// Gives each markdown view its matches (the current one stronger) and clears views
    /// that no longer have any.
    fn paint_markdown_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_ref() else {
            return;
        };
        let (all, current) = match_colors(cx);
        let mut by_key: HashMap<BlockKey, Vec<RangeHighlight>> = HashMap::new();
        for (ix, found) in find.matches.iter().enumerate() {
            if found.field.is_none() {
                let color = if Some(ix) == find.current {
                    current
                } else {
                    all
                };
                by_key
                    .entry(found.key)
                    .or_default()
                    .push(RangeHighlight::new(found.range.clone(), color));
            }
        }
        let stale: HashSet<BlockKey> = find
            .highlighted
            .iter()
            .filter(|key| !by_key.contains_key(key))
            .copied()
            .collect();
        self.clear_markdown_highlights(&stale, cx);
        let mut painted = HashSet::new();
        for (key, highlights) in by_key {
            if let Some(markdown) = self.markdown.get(&key) {
                let ok = markdown
                    .state
                    .update(cx, |state, cx| state.set_range_highlights(highlights, cx));
                if ok.is_ok() {
                    painted.insert(key);
                }
            }
        }
        if let Some(find) = self.find.as_mut() {
            find.highlighted = painted;
        }
    }

    fn clear_markdown_highlights(&self, keys: &HashSet<BlockKey>, cx: &mut Context<Self>) {
        for key in keys {
            if let Some(markdown) = self.markdown.get(key) {
                markdown
                    .state
                    .update(cx, |state, cx| state.clear_range_highlights(cx));
            }
        }
    }

    /// The row that shows item `row`: the start of its "Explored" group if it is folded
    /// into one.
    fn display_row(&self, row: usize) -> usize {
        if !matches!(self.roles.get(row), Some(RowRole::Hidden)) {
            return row;
        }
        (0..row)
            .rev()
            .find(|start| {
                matches!(self.roles.get(*start), Some(RowRole::Group(group)) if group.last >= row)
            })
            .unwrap_or(row)
    }

    /// Opens whatever hides the match (its group, a collapsed thinking or tool block) and
    /// scrolls it into view.
    fn reveal_match(&mut self, found: &FindMatch, cx: &mut Context<Self>) {
        let display = self.display_row(found.row);
        let in_group = matches!(self.roles.get(display), Some(RowRole::Group(_)));
        if in_group {
            let items = self.session.read(cx).transcript().items();
            if let Some(first) = items.get(display) {
                let group = BlockKey {
                    item: first.id,
                    block: usize::MAX,
                };
                self.expand(group, false);
            }
        }
        match found.field {
            Some(Field::Thinking) => self.expand(found.key, true),
            // A user's shell command always shows its output; a tool block may be collapsed.
            Some(Field::Output | Field::DiffLine(_)) => {
                let is_tool = matches!(
                    self.session
                        .read(cx)
                        .transcript()
                        .items()
                        .get(found.row)
                        .map(|item| &item.kind),
                    Some(ItemKind::Assistant(_))
                );
                if is_tool {
                    self.expand(found.key, false);
                }
            }
            None => {}
        }
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(display..display + 1, cx)
        });
        match found.field {
            None => {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_item(display, cx));
                // The list lays the row out first; then the view scrolls to the line.
                if let Some(markdown) = self.markdown.get(&found.key) {
                    let range = found.range.clone();
                    markdown
                        .state
                        .update(cx, |state, cx| state.reveal_range(range, cx).ok());
                }
            }
            Some(field) => {
                // Near the match's line within a tall row; the exact layout is the row's,
                // so this is an estimate that keeps the match on screen.
                let line = self.match_line(found, field, cx);
                #[allow(clippy::cast_precision_loss)]
                let offset = (line as f32 * CODE_LINE_HEIGHT - SCROLL_CONTEXT).max(0.);
                self.scroller.update(cx, |scroller, cx| {
                    scroller.scroll_to_offset(
                        ListOffset {
                            item_ix: display,
                            offset_in_item: px(offset),
                        },
                        cx,
                    )
                });
            }
        }
    }

    /// The line of a plain-text match within its block.
    fn match_line(&self, found: &FindMatch, field: Field, cx: &App) -> usize {
        if let Field::DiffLine(line) = field {
            return line;
        }
        let items = self.session.read(cx).transcript().items();
        let Some(item) = items.get(found.row) else {
            return 0;
        };
        search::plain_fields(item)
            .into_iter()
            .find(|(block, f, _)| *block == found.key.block && *f == field)
            .map_or(0, |(_, _, text)| {
                text[..found.range.start.min(text.len())]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
            })
    }

    /// Expands a block (thinking, a tool call or a group) if it is collapsed.
    fn expand(&mut self, key: BlockKey, is_thinking: bool) {
        if !self.is_expanded(key, is_thinking) && !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
    }

    /// Find highlights for a plain-text field of a row, as styles over the text it shows.
    pub(crate) fn find_highlights(
        &self,
        key: BlockKey,
        field: Field,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(found) = self
            .find
            .as_ref()
            .and_then(|find| find.plain.get(&(key, field)))
        else {
            return Vec::new();
        };
        let (all, current) = match_colors(cx);
        found
            .iter()
            .map(|(range, is_current)| {
                (
                    range.clone(),
                    HighlightStyle {
                        background_color: Some(if *is_current { current } else { all }),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    /// The current match is in this block's output, which then shows in full (not in a
    /// small scrolling box) so the match is visible.
    pub(crate) fn find_current_in(&self, key: BlockKey) -> bool {
        self.find.as_ref().is_some_and(|find| {
            find.current
                .and_then(|ix| find.matches.get(ix))
                .is_some_and(|found| found.key == key && found.field.is_some())
        })
    }

    /// The find bar, floating over the top right of the transcript.
    pub(super) fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = cx.theme();
        let label = match (find.current, find.matches.len()) {
            _ if find.query.trim().is_empty() => String::new(),
            (_, 0) => "No results".to_owned(),
            (Some(ix), total) => format!("{} of {total}", ix + 1),
            (None, total) => format!("{total} found"),
        };
        let has_matches = !find.matches.is_empty();
        Some(
            h_flex()
                .key_context(FIND_CONTEXT)
                // The input binds Esc itself; here it closes the bar.
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    this.close_find(window, cx);
                    cx.stop_propagation();
                }))
                .absolute()
                .top_2()
                .right_4()
                .gap_1()
                .p_1()
                .pl_2()
                .items_center()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(palette(cx).surface)
                .shadow_md()
                .child(
                    div()
                        .w(px(220.))
                        .child(Input::new(&find.input).appearance(false)),
                )
                .child(
                    div()
                        .min_w(px(64.))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    Button::new("find-previous")
                        .icon(IconName::ChevronUp)
                        .ghost()
                        .xsmall()
                        .disabled(!has_matches)
                        .tooltip(if cfg!(target_os = "macos") {
                            "Previous match (⇧Enter, ⌘⇧G)"
                        } else {
                            "Previous match (Shift+Enter, Ctrl+Shift+G)"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.find_step(false, window, cx);
                        })),
                )
                .child(
                    Button::new("find-next")
                        .icon(IconName::ChevronDown)
                        .ghost()
                        .xsmall()
                        .disabled(!has_matches)
                        .tooltip(if cfg!(target_os = "macos") {
                            "Next match (Enter, ⌘G)"
                        } else {
                            "Next match (Enter, Ctrl+G)"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.find_step(true, window, cx);
                        })),
                )
                .child(
                    Button::new("find-close")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close (Esc)")
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// Registers the find actions on the chat's root element.
    pub(super) fn find_actions(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
    }
}

/// Background colors of matches and of the current match.
fn match_colors(cx: &App) -> (Hsla, Hsla) {
    let warning = palette(cx).warning;
    (warning.opacity(0.28), warning.opacity(0.7))
}
