//! The transcript's virtualized, tail-following message list.
//!
//! Vendored from gpui-kit 0.10's `gpui_component::message_scroller`
//! (<https://github.com/longbridge/gpui-kit>), Copyright gpui-kit contributors, licensed
//! under the Apache License 2.0 (its text ships in THIRD-PARTY-LICENSES.txt with every
//! release, gpui-kit being a dependency), and trimmed to what the chat uses. The one behavioural change: the list measures every row, not
//! only the rows it has laid out (over a few frames, see [`super::list`]). gpui-kit keeps its `ListState` private and builds it
//! measuring visible rows only, so the rows above the viewport of a freshly opened long
//! session count as zero height: the scrollbar thumb starts far too big and shrinks and
//! jumps as scrolling up measures them. Measuring all rows makes the scroll extent exact
//! from the first frame.

use std::{ops::Range, time::Duration};

use gpui_kit::base::ScrollBounce;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, linear_color_stop, linear_gradient, px, rems,
};

use super::list::{FollowMode, ListAlignment, ListOffset, ListState, list};

const LIST_OVERDRAW: gpui_kit::Pixels = px(400.);
const JUMP_BUTTON_TRANSITION: Duration = Duration::from_millis(200);
const BOTTOM_FADE_TRANSITION: Duration = Duration::from_millis(200);

/// Renders the row at an index.
type RowRenderer = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static>;

/// The entity-owned scrolling state for a [`MessageScroller`].
///
/// The state owns only GPUI's virtual-list bookkeeping. Message data remains
/// with the caller and is read by the row renderer passed to
/// [`MessageScroller::new`].
pub struct MessageScrollerState {
    list_state: ListState,
}

impl MessageScrollerState {
    /// Create a state for `item_count` rows and enable tail following.
    ///
    /// The constructor receives the entity context so the list's scroll
    /// handler can safely defer its entity update until GPUI has released the
    /// list's internal borrow.
    pub fn new(item_count: usize, cx: &mut Context<Self>) -> Self {
        // `measure_all`: the scrollbar's extent must cover the rows above the viewport
        // from the first frame (see the module docs). After the first pass the list lays
        // out only rows a splice or remeasure invalidated: a streamed delta costs one row,
        // not all. Laying out every row (a reset, Ctrl+T/Ctrl+O, a change of width) costs
        // about 30 ms for a 100-row session in a release build, so the vendored list
        // (`super::list`) spreads it over frames, estimating rows it has not measured yet.
        let list_state =
            ListState::new(item_count, ListAlignment::Top, LIST_OVERDRAW).measure_all();
        list_state.set_follow_mode(FollowMode::Tail);

        let weak_state = cx.weak_entity();
        list_state.set_scroll_handler(move |_, _, cx| {
            let weak_state = weak_state.clone();

            cx.defer(move |cx| {
                let _ = weak_state.update(cx, |_, cx| cx.notify());
            });
        });

        Self { list_state }
    }

    /// Return whether the user has scrolled away from the latest content.
    pub fn is_scrolled_up(&self) -> bool {
        self.list_state.max_offset_for_scrollbar().y > px(0.)
            && !self.list_state.is_following_tail()
            && !self.list_state.is_scrolled_to_end().unwrap_or(false)
    }

    /// Return the row at the top of the viewport and how far into it the list
    /// is scrolled.
    ///
    /// Wheel, trackpad, and scrollbar scrolls, [`Self::scroll_to_item`] and
    /// [`Self::scroll_to_offset`] update it at once. [`Self::scroll_to_end`]
    /// and tail following settle it when the list next lays out.
    pub fn logical_scroll_top(&self) -> ListOffset {
        self.list_state.logical_scroll_top()
    }

    /// Reset the list to `item_count` rows.
    pub fn reset(&mut self, item_count: usize, cx: &mut Context<Self>) {
        self.list_state.reset(item_count);
        self.list_state.set_follow_mode(FollowMode::Tail);
        cx.notify();
    }

    /// Replace `old_range` with `count` new rows.
    ///
    /// Returns `false` when the range is outside the current list and leaves
    /// the state unchanged.
    pub fn splice(
        &mut self,
        old_range: Range<usize>,
        count: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.valid_range(&old_range) {
            return false;
        }

        let neighbor = old_range.start.checked_sub(1);
        let touches_start = old_range.start == 0;
        self.list_state.splice(old_range, count);

        // The default row wrapper pads every row except the last, so a row
        // whose "last" status may have flipped carries a stale measured
        // height. Remeasure the new last row and the survivor next to the
        // splice.
        if let Some(last) = self.list_state.item_count().checked_sub(1) {
            self.list_state.remeasure_items(last..last + 1);
            if let Some(neighbor) = neighbor.filter(|neighbor| *neighbor != last) {
                self.list_state.remeasure_items(neighbor..neighbor + 1);
            }
        }

        // A surviving former first row may have lost its list inset, or a
        // surviving row may have become the first after a removal.
        if touches_start {
            for index in [0, count] {
                if index < self.list_state.item_count() {
                    self.list_state.remeasure_items(index..index + 1);
                }
            }
        }

        cx.notify();
        true
    }

    /// Mark all rows for remeasurement while preserving a proportional anchor.
    pub fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.list_state.remeasure();
        cx.notify();
    }

    /// Mark rows in `range` for remeasurement while preserving an item anchor.
    ///
    /// Returns `false` when the range is outside the current list.
    pub fn remeasure_items(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> bool {
        if !self.valid_range(&range) {
            return false;
        }

        self.list_state.remeasure_items(range);
        cx.notify();
        true
    }

    /// Scroll to the row at `index`, if it exists.
    pub fn scroll_to_item(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        self.scroll_to_offset(
            ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            },
            cx,
        )
    }

    /// Scroll to a position [`Self::logical_scroll_top`] returned, if its row exists:
    /// the row at the top of the viewport and how far into it the list is scrolled.
    ///
    /// Restores a saved reading position to the pixel, where [`Self::scroll_to_item`]
    /// puts the top of the row at the top of the viewport.
    pub fn scroll_to_offset(&mut self, offset: ListOffset, cx: &mut Context<Self>) -> bool {
        if offset.item_ix >= self.list_state.item_count() {
            return false;
        }

        self.list_state.scroll_to(offset);
        cx.notify();
        true
    }

    /// Resume tail following and scroll to the latest row.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list_state.set_follow_mode(FollowMode::Tail);
        self.list_state.scroll_to_end();
        cx.notify();
    }

    fn valid_range(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.list_state.item_count()
    }
}

/// A virtualized message list with a scrollbar, a bottom fade and a jump-to-latest button.
#[derive(IntoElement)]
pub struct MessageScroller {
    id: ElementId,
    state: Entity<MessageScrollerState>,
    renderer: RowRenderer,
    style: StyleRefinement,
    row_style: StyleRefinement,
    bottom_fade: Option<Hsla>,
}

impl MessageScroller {
    /// Create a message scroller with a renderer for each row.
    pub fn new<E>(
        id: impl Into<ElementId>,
        state: Entity<MessageScrollerState>,
        renderer: impl FnMut(usize, &mut Window, &mut App) -> E + 'static,
    ) -> Self
    where
        E: IntoElement,
    {
        let mut renderer = renderer;
        Self {
            id: id.into(),
            state,
            renderer: Box::new(move |index, window, cx| {
                renderer(index, window, cx).into_any_element()
            }),
            style: StyleRefinement::default(),
            row_style: StyleRefinement::default(),
            bottom_fade: None,
        }
    }

    /// Refine the full-width wrapper around every rendered row.
    pub fn with_row_style(mut self, style: StyleRefinement) -> Self {
        self.row_style = style;
        self
    }

    /// Fade the transcript's bottom edge into `color`.
    ///
    /// A partially visible row melts into the surface behind the scroller
    /// instead of clipping mid-line. The fade shows only while the reader is
    /// away from the live edge — at the bottom nothing is clipped. Pass the
    /// color of that surface; it sits under the jump button.
    pub fn with_bottom_fade(mut self, color: impl Into<Hsla>) -> Self {
        self.bottom_fade = Some(color.into());
        self
    }
}

impl Styled for MessageScroller {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for MessageScroller {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let root_id = self.id.clone();
        let (list_state, scrolled_up) = {
            let state = self.state.read(cx);
            (state.list_state.clone(), state.is_scrolled_up())
        };
        let jump_button_visibility = transition(
            (root_id.clone(), "jump-button-visibility"),
            if scrolled_up { 1. } else { 0. },
            Transition::new(JUMP_BUTTON_TRANSITION),
            window,
            cx,
        );
        // At the live edge nothing is clipped below, so a visible fade would
        // suggest more content than there is.
        let bottom_fade_visibility = if self.bottom_fade.is_some() {
            transition(
                (root_id.clone(), "bottom-fade-visibility"),
                if scrolled_up { 1. } else { 0. },
                Transition::new(BOTTOM_FADE_TRANSITION),
                window,
                cx,
            )
        } else {
            0.
        };
        let tokens = cx.theme().semantic_tokens();
        let row_style = self.row_style;
        let mut renderer = self.renderer;

        // Read the count outside the row closure: the list holds a mutable
        // borrow of its state while rendering rows, so the closure must not
        // borrow it again. The count is stable within one render pass.
        //
        // The list's vertical inset rides on the first and last rows rather than
        // on the list: a list measures its scroll extent from its items alone, so
        // padding of its own would make the extent stop short of the end.
        let item_count = list_state.item_count();
        let list = list(list_state.clone(), move |index, window, cx| {
            div()
                .w_full()
                .when(index == 0, |this| this.pt_2())
                .when(index + 1 == item_count, |this| this.pb_2())
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px_3()
                        .when(index + 1 < item_count, |this| this.pb_8())
                        .refine_style(&row_style)
                        .child(renderer(index, window, cx)),
                )
                .into_any_element()
        })
        .size_full()
        .min_h_0();

        let viewport = div()
            .id((root_id.clone(), "viewport"))
            // Announce appended rows as a log region.
            .role(Role::Log)
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                // The bounce scrolls the list itself, past the list's own
                // scroll handler; tell the state, as that handler would.
                ScrollBounce::new((root_id.clone(), "bounce"), &list_state, list).on_scroll({
                    let state = self.state.downgrade();
                    move |_, cx| {
                        let _ = state.update(cx, |_, cx| cx.notify());
                    }
                }),
            )
            // The fade sits above the rows but below the scrollbar and the
            // jump button, so neither control is washed out by it.
            .when_some(
                self.bottom_fade.filter(|_| bottom_fade_visibility > 0.),
                |this, color| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(rems(3.))
                            .opacity(bottom_fade_visibility)
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(color.opacity(0.), 0.),
                                linear_color_stop(color, 1.),
                            )),
                    )
                },
            )
            .vertical_scrollbar(&list_state);

        div()
            .id(root_id.clone())
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(viewport)
            .when(jump_button_visibility > 0., |this| {
                let state = self.state.clone();

                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(rems(0.5 + jump_button_visibility * 0.5))
                        .flex()
                        .justify_center()
                        .opacity(jump_button_visibility)
                        .child(
                            Button::new((root_id, "jump-to-latest"))
                                .secondary()
                                .icon(IconName::ArrowDown)
                                .tooltip("Jump to latest")
                                .rounded(cx.theme().radius_full())
                                .border_1()
                                .border_color(tokens.colors.border)
                                .bg(tokens.colors.background)
                                .text_color(tokens.colors.foreground)
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |state, cx| state.scroll_to_end(cx));
                                })
                                .when(!scrolled_up, |button| button.disabled(true)),
                        ),
                )
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of `gpui_kit` would bring GPUI's `test` attribute into scope
    // and break `#[test]`.
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
        ScrollDelta, ScrollEvent, Styled as _, TestAppContext, VisualTestContext, Window, div,
        point, px,
    };

    use super::{MessageScroller, MessageScrollerState};
    use crate::ui::list::ListOffset;

    /// The scroller in a 400px square, with rows of `height(index)` that count their renders.
    struct Harness {
        state: Entity<MessageScrollerState>,
        height: fn(usize) -> Pixels,
        renders: Rc<Cell<usize>>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (height, renders) = (self.height, self.renders.clone());
            div().size(px(400.)).child(MessageScroller::new(
                "messages",
                self.state.clone(),
                move |index, _, _| {
                    renders.set(renders.get() + 1);
                    div().h(height(index))
                },
            ))
        }
    }

    fn uniform(_: usize) -> Pixels {
        px(40.)
    }

    /// Opens a window on `rows` rows; returns the state, the render count and the context.
    fn open(
        cx: &mut TestAppContext,
        rows: usize,
        height: fn(usize) -> Pixels,
    ) -> (
        Entity<MessageScrollerState>,
        Rc<Cell<usize>>,
        &mut VisualTestContext,
    ) {
        cx.update(gpui_kit::component::init);
        let state = cx.new(|cx| MessageScrollerState::new(rows, cx));
        let renders = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let (state, renders) = (state.clone(), renders.clone());
            move |_, _| Harness {
                state,
                height,
                renders,
            }
        });
        draw(cx);
        (state, renders, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn extent(state: &Entity<MessageScrollerState>, cx: &mut VisualTestContext) -> Pixels {
        cx.update(|_, cx| state.read(cx).list_state.max_offset_for_scrollbar().y)
    }

    #[gpui_kit::test]
    fn state_operations_check_their_ranges(cx: &mut TestAppContext) {
        let state = cx.new(|cx| MessageScrollerState::new(3, cx));
        cx.update(|cx| {
            assert!(!state.read(cx).is_scrolled_up());
            state.update(cx, |state, cx| {
                assert!(!state.scroll_to_item(3, cx));
                assert!(state.splice(3..3, 2, cx));
                assert!(state.splice(0..0, 1, cx));
                assert!(!state.splice(5..7, 0, cx));
                assert!(state.remeasure_items(0..6, cx));
                assert!(!state.remeasure_items(6..7, cx));
                assert!(state.scroll_to_item(2, cx));
                assert!(!state.list_state.is_following_tail());
                state.scroll_to_end(cx);
                assert!(state.list_state.is_following_tail());
                state.reset(2, cx);
                assert_eq!(state.list_state.item_count(), 2);
                assert!(state.list_state.is_following_tail());
            });
        });
    }

    /// The bug this vendored copy fixes: opening a long conversation at its end, the
    /// scrollbar must already span the rows above the viewport, not grow (and its thumb
    /// shrink and jump) as scrolling up measures them.
    #[gpui_kit::test]
    fn the_scroll_extent_covers_every_row_from_the_first_frame(cx: &mut TestAppContext) {
        let varied = |index: usize| px([20., 50., 80., 110., 140.][index % 5]);
        let (state, _, cx) = open(cx, 60, varied);
        let first = extent(&state, cx);

        cx.update(|_, cx| state.update(cx, |state, cx| state.scroll_to_item(0, cx)));
        draw(cx);
        assert_eq!(
            extent(&state, cx),
            first,
            "scrolling to the top measured rows the first frame had not"
        );
        // Every row with the 32px gap after all but the last, the list's 8px insets at
        // both ends, less the 400px viewport.
        let rows: f32 = (0..60).map(|index| f32::from(varied(index))).sum();
        assert_eq!(first, px(rows + 32. * 59. + 16. - 400.));
    }

    /// Streaming invalidates one row per delta: the list must lay out that row again,
    /// not every row of the conversation.
    #[gpui_kit::test]
    fn remeasuring_one_row_does_not_lay_out_every_row(cx: &mut TestAppContext) {
        let (state, renders, cx) = open(cx, 200, uniform);
        assert!(renders.get() >= 200, "the first frame measures every row");

        // A frame with nothing invalidated renders the visible rows only.
        renders.set(0);
        cx.update(|window, _| window.refresh());
        draw(cx);
        let visible = renders.get();
        assert!(visible < 20, "an idle frame rendered {visible} rows");

        renders.set(0);
        cx.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.splice(200..200, 1, cx);
                state.remeasure_items(199..200, cx);
            });
        });
        draw(cx);
        assert!(
            renders.get() <= visible + 3,
            "a streamed row rendered {} rows",
            renders.get()
        );
    }

    #[gpui_kit::test]
    fn logical_scroll_top_reports_the_row_at_the_top(cx: &mut TestAppContext) {
        let (state, _, cx) = open(cx, 30, uniform);
        let top =
            |cx: &mut VisualTestContext| cx.update(|_, cx| state.read(cx).logical_scroll_top());

        // Following the tail, layout settles on the row the tail starts on.
        let tail = top(cx);
        assert!(tail.item_ix > 0 && tail.item_ix < 30, "{tail:?}");

        cx.update(|_, cx| state.update(cx, |state, cx| state.scroll_to_item(10, cx)));
        draw(cx);
        assert_eq!(top(cx).item_ix, 10);

        // A wheel scroll by one row: its 40px and the 32px gap after it.
        cx.simulate_event(ScrollEvent {
            position: point(px(200.), px(200.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-72.))),
            ..Default::default()
        });
        let after_wheel = top(cx);
        assert_eq!(
            (after_wheel.item_ix, after_wheel.offset_in_item),
            (11, px(0.))
        );
    }

    #[gpui_kit::test]
    fn scroll_to_offset_restores_a_saved_position(cx: &mut TestAppContext) {
        let (state, _, cx) = open(cx, 30, uniform);
        let saved = ListOffset {
            item_ix: 10,
            offset_in_item: px(17.),
        };
        cx.update(|_, cx| {
            state.update(cx, |state, cx| {
                let past_the_end = ListOffset {
                    item_ix: 30,
                    offset_in_item: px(0.),
                };
                assert!(!state.scroll_to_offset(past_the_end, cx));
                state.reset(30, cx);
                assert!(state.scroll_to_offset(saved, cx));
            });
        });
        draw(cx);
        let top = cx.update(|_, cx| state.read(cx).logical_scroll_top());
        assert_eq!((top.item_ix, top.offset_in_item), (10, px(17.)));
    }

    /// A scrollbar drag to the bottom of the track must reach the end of the content and
    /// resume following the tail.
    #[gpui_kit::test]
    fn the_scrollbar_drag_range_reaches_the_end_of_the_content(cx: &mut TestAppContext) {
        let (state, _, cx) = open(cx, 10, uniform);
        cx.update(|_, cx| {
            let list_state = state.read(cx).list_state.clone();
            list_state.scrollbar_drag_started();
            let max = list_state.max_offset_for_scrollbar().y;
            list_state.set_offset_from_scrollbar(point(px(0.), -max));
            list_state.scrollbar_drag_ended();
        });
        cx.update(|_, cx| {
            let list_state = state.read(cx).list_state.clone();
            assert_eq!(list_state.is_scrolled_to_end(), Some(true));
            assert!(list_state.is_following_tail());
            assert!(!state.read(cx).is_scrolled_up());
        });
    }
}
