//! The turn rail: a column of dots at the left edge of the conversation, one per prompt. The
//! dot of the turn being read is drawn longer, in the accent color. Hovering a dot grows it and
//! shows a card with that prompt and the start of its answer; clicking it scrolls there.
//!
//! It is its own view: scrolling redraws the dots, not the conversation.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::{ItemKind, TranscriptItem};

use super::ChatView;
use super::session::PiSession;
use crate::theme::palette;
use crate::ui::MessageScrollerState;

/// The rail's width: the dots sit in its middle.
const RAIL_WIDTH: f32 = 24.;
/// The most height the dots take before they move closer together.
const RAIL_HEIGHT: f32 = 360.;
const ROW_MAX: f32 = 14.;
const ROW_MIN: f32 = 4.;
const CARD_WIDTH: f32 = 340.;
/// How much of a prompt and its answer the card shows.
const PROMPT_CHARS: usize = 280;
const REPLY_CHARS: usize = 160;

/// One prompt and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Its row in the conversation.
    pub row: usize,
    pub prompt: String,
    /// Files and pictures that came with it.
    pub attached: usize,
    /// The start of the answer, once there is one.
    pub reply: Option<String>,
}

/// The turns of `items`, leaving out the first `folded` rows (a side chat's taken-over history,
/// shown folded into one card).
pub fn turns(items: &[TranscriptItem], folded: usize) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (row, item) in items.iter().enumerate().skip(folded) {
        match &item.kind {
            ItemKind::User(user) => turns.push(Turn {
                row,
                // What was typed, or else what was attached.
                prompt: shorten(
                    Some(user.typed().trim())
                        .filter(|typed| !typed.is_empty())
                        .or_else(|| user.title())
                        .unwrap_or_default(),
                    PROMPT_CHARS,
                ),
                attached: user.files.len() + user.images.len(),
                reply: None,
            }),
            ItemKind::Assistant(assistant) => {
                if let Some(turn) = turns.last_mut().filter(|turn| turn.reply.is_none())
                    && let Some(answer) = assistant.answer().filter(|text| !text.trim().is_empty())
                {
                    turn.reply = Some(shorten(&answer, REPLY_CHARS));
                }
            }
            _ => {}
        }
    }
    turns
}

/// The turn being read: the last one that starts at or above the top of the view.
pub fn active(turns: &[Turn], top_row: usize, at_end: bool) -> Option<usize> {
    if at_end {
        return turns.len().checked_sub(1);
    }
    turns
        .iter()
        .rposition(|turn| turn.row <= top_row)
        .or((!turns.is_empty()).then_some(0))
}

/// `text` on one line, cut at about `limit` characters on a word.
fn shorten(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let cut: String = flat.chars().take(limit).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':']))
}

pub struct TurnRail {
    chat: WeakEntity<ChatView>,
    session: Entity<PiSession>,
    scroller: Entity<MessageScrollerState>,
    /// The turn under the pointer.
    hovered: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl TurnRail {
    pub fn new(
        chat: WeakEntity<ChatView>,
        session: Entity<PiSession>,
        scroller: Entity<MessageScrollerState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.observe(&scroller, |_, _, cx| cx.notify()),
        ];
        Self {
            chat,
            session,
            scroller,
            hovered: None,
            _subscriptions: subscriptions,
        }
    }

    /// A side chat's history was folded or opened.
    pub fn folding_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn jump(&mut self, row: usize, cx: &mut Context<Self>) {
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_item(row, cx));
        if let Some(chat) = self.chat.upgrade() {
            chat.update(cx, |_, cx| cx.notify());
        }
    }
}

impl Render for TurnRail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let folded = self
            .chat
            .upgrade()
            .map_or(0, |chat| chat.read(cx).folded_rows());
        let turns = turns(self.session.read(cx).transcript().items(), folded);
        if turns.len() < 2 {
            return div().into_any_element();
        }
        let scroller = self.scroller.read(cx);
        let at_end = !scroller.is_scrolled_up();
        let active = active(&turns, scroller.logical_scroll_top().item_ix, at_end);
        #[allow(clippy::cast_precision_loss)] // a conversation's turns
        let row_height = (RAIL_HEIGHT / turns.len() as f32).clamp(ROW_MIN, ROW_MAX);
        let theme = cx.theme();
        let (accent, subtle) = (palette(cx).accent, palette(cx).subtle);
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let (popover, border) = (theme.popover, theme.border);
        let count = turns.len();
        let dot_size = (row_height - 2.).clamp(3., 6.);
        let dots = turns.into_iter().enumerate().map(|(index, turn)| {
            let is_active = active == Some(index);
            let is_hovered = self.hovered == Some(index);
            let (width, height, color) = if is_hovered {
                (dot_size + 3., dot_size + 3., foreground)
            } else if is_active {
                (dot_size, (row_height * 2.).clamp(dot_size, 14.), accent)
            } else {
                (dot_size, dot_size, subtle.opacity(0.7))
            };
            let row = turn.row;
            div()
                .id(("turn-dot", index))
                .relative()
                .w(px(RAIL_WIDTH))
                .h(px(row_height))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(div().w(px(width)).h(px(height)).rounded_full().bg(color))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.hovered = Some(index);
                    } else if this.hovered == Some(index) {
                        this.hovered = None;
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.hovered = None;
                    this.jump(row, cx);
                }))
                .when(is_hovered, |this| {
                    this.child(card(
                        &turn,
                        index,
                        count,
                        (popover, border, foreground, muted),
                    ))
                })
        });
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(RAIL_WIDTH))
            .flex()
            .flex_col()
            .justify_center()
            .debug_selector(|| "turn-rail".into())
            .children(dots)
            .into_any_element()
    }
}

/// The card beside a hovered dot: which turn, the prompt and the start of its answer.
fn card(turn: &Turn, index: usize, count: usize, colors: (Hsla, Hsla, Hsla, Hsla)) -> Div {
    let (popover, border, foreground, muted) = colors;
    let heading = match turn.attached {
        0 => format!("Turn {} of {count}", index + 1),
        1 => format!("Turn {} of {count} · 1 file attached", index + 1),
        files => format!("Turn {} of {count} · {files} files attached", index + 1),
    };
    div()
        .absolute()
        .left(px(RAIL_WIDTH + 4.))
        .top(px(-14.))
        .w(px(CARD_WIDTH))
        .p_3()
        .flex()
        .flex_col()
        .gap_1p5()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(popover)
        .shadow_lg()
        .debug_selector(|| "turn-card".into())
        .child(div().text_xs().text_color(muted).child(heading))
        .child(div().text_sm().text_color(foreground).line_clamp(4).child(
            if turn.prompt.is_empty() {
                "(no text)".to_owned()
            } else {
                turn.prompt.clone()
            },
        ))
        .when_some(turn.reply.clone(), |this, reply| {
            this.child(
                div()
                    .pt_1p5()
                    .border_t_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .line_clamp(2)
                    .child(reply),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{Turn, active, shorten};

    fn turn(row: usize) -> Turn {
        Turn {
            row,
            prompt: String::new(),
            attached: 0,
            reply: None,
        }
    }

    #[test]
    fn the_active_turn_is_the_last_one_started_above_the_top() {
        let turns = [turn(0), turn(5), turn(9)];
        assert_eq!(active(&turns, 0, false), Some(0));
        assert_eq!(active(&turns, 4, false), Some(0));
        assert_eq!(active(&turns, 5, false), Some(1));
        assert_eq!(active(&turns, 30, false), Some(2));
        assert_eq!(active(&turns, 0, true), Some(2));
        assert_eq!(active(&[], 0, true), None);
    }

    #[test]
    fn long_text_is_cut_on_a_word_and_flattened() {
        assert_eq!(shorten("short\n  text", 20), "short text");
        assert_eq!(shorten("one two three four", 12), "one two…");
    }
}
