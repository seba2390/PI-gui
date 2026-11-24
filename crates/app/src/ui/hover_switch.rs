//! A switch with the app's hover fill around it.

use std::rc::Rc;

use gpui_kit::base::{Switch, SwitchThumb, SwitchTrack, spring};
use gpui_kit::component::{ActiveTheme as _, ThemeStyled as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::palette;

const WIDTH: Pixels = px(36.);
const HEIGHT: Pixels = px(20.);
const THUMB: Pixels = px(14.);
/// The outline's width, and the room between it and the thumb.
const EDGE: Pixels = px(1.5);

type OnClick = Rc<dyn Fn(bool, &mut Window, &mut App)>;

pub fn hover_switch(
    id: impl Into<ElementId>,
    checked: bool,
    on_click: impl Fn(bool, &mut Window, &mut App) + 'static,
    accessibility_label: Option<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let id = id.into();
    let hover = cx.theme().secondary_hover;
    div()
        .id(id.clone())
        .p(px(3.))
        .rounded_full()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(OutlinedSwitch {
            id,
            checked,
            on_click: Rc::new(on_click),
            accessibility_label,
        })
}

/// gpui-component's switch with an outline and a light thumb in both states: its own has a
/// thumb in the window's color, so in the dark theme one that is off looked disabled.
#[derive(IntoElement)]
struct OutlinedSwitch {
    id: ElementId,
    checked: bool,
    on_click: OnClick,
    accessibility_label: Option<SharedString>,
}

impl RenderOnce for OutlinedSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        // For the keyboard only: a click focuses the switch too, and the ring would stay after
        // it. (The window redraws when the kind of input changes.)
        let ring = focus_handle.is_focused(window) && window.last_input_was_keyboard();
        let colors = palette(cx);
        let (track, edge) = if self.checked {
            (colors.switch_on, colors.switch_on_edge)
        } else {
            (cx.theme().switch, colors.switch_edge)
        };
        let travel = WIDTH - THUMB - EDGE * 4.;
        let thumb_x = spring(
            (self.id.clone(), "thumb"),
            if self.checked { travel } else { px(0.) },
            cx.theme().motion_tokens().spring_move,
            window,
            cx,
        );
        let on_click = self.on_click;
        Switch::new(self.id.clone())
            .checked(self.checked)
            .when_some(self.accessibility_label, |this, label| {
                this.accessibility_label(label)
            })
            .on_change(move |next, _, window, cx| on_click(next, window, cx))
            .track_focus(&focus_handle)
            .child(
                SwitchTrack::new((self.id, "track"))
                    .checked(self.checked)
                    .w(WIDTH)
                    .h(HEIGHT)
                    .flex_none()
                    .flex()
                    .items_center()
                    .p(EDGE)
                    .rounded_full()
                    .border(EDGE)
                    .border_color(edge)
                    .bg(track)
                    .when(ring, |this| this.focus_ring_style(window, cx))
                    .child(
                        SwitchThumb::new(self.checked)
                            .relative()
                            .left(thumb_x)
                            .size(THUMB)
                            .flex_none()
                            .rounded_full()
                            .bg(colors.switch_thumb),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
    use gpui_kit::{
        Context, Hsla, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div,
    };

    use crate::theme::palette;

    struct One(Rc<Cell<bool>>);

    impl Render for One {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let on = self.0.clone();
            let switch =
                super::hover_switch("one", on.get(), move |next, _, _| on.set(next), None, cx);
            div()
                .flex()
                .items_start()
                .child(switch.debug_selector(|| "one".into()))
        }
    }

    /// The colors of the outlines and of the fills painted.
    fn painted(cx: &mut VisualTestContext) -> (Vec<Hsla>, Vec<Hsla>) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window
                .painted_quads()
                .iter()
                .map(|quad| {
                    (
                        quad.border_color,
                        quad.background.as_solid().unwrap_or_default(),
                    )
                })
                .unzip()
        })
    }

    /// Off and on are both outlined, with a thumb that stands out; only on is green.
    #[gpui_kit::test]
    fn off_and_on_are_outlined(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
        });
        let on = Rc::new(Cell::new(false));
        let shown = on.clone();
        let (_, cx) = cx.add_window_view(|_, _| One(shown));
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            cx.update(|window, cx| Theme::change(mode, Some(window), cx));
            let (colors, off_track) = cx.update(|_, cx| (palette(cx), cx.theme().switch));
            assert!(
                (colors.switch_thumb.l - off_track.l).abs() > 0.1,
                "{mode:?}"
            );
            let (edges, fills) = painted(cx);
            assert!(edges.contains(&colors.switch_edge), "off, {mode:?}");
            assert!(!fills.contains(&colors.switch_on));
            assert!(fills.contains(&colors.switch_thumb));

            let switch = cx.debug_bounds("one").unwrap().center();
            cx.simulate_click(switch, Modifiers::default());
            assert!(on.get());
            let (edges, fills) = painted(cx);
            assert!(edges.contains(&colors.switch_on_edge), "on, {mode:?}");
            assert!(fills.contains(&colors.switch_on));
            assert!(fills.contains(&colors.switch_thumb));
            cx.simulate_click(switch, Modifiers::default());
            assert!(!on.get());
        }
    }
}
