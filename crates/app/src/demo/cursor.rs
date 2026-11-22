//! The demo cursor: an arrow drawn above everything (menus included) with a click ripple.
//! It only shows what the runner does; the clicks themselves are separate synthetic events.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const ASSET: &str = "demo/cursor.svg";
const SIZE: Size<f32> = Size {
    width: 25.,
    height: 32.,
};
/// The arrow's tip within [`SIZE`].
const HOTSPOT: Point<f32> = Point { x: 3.6, y: 3.1 };
const RIPPLE: Duration = Duration::from_millis(520);

pub struct DemoCursor {
    position: Point<Pixels>,
    visible: bool,
    pressed: bool,
    ripples: Vec<(Point<Pixels>, Instant)>,
}

impl DemoCursor {
    pub fn new(position: Point<Pixels>) -> Self {
        Self {
            position,
            visible: false,
            pressed: false,
            ripples: Vec::new(),
        }
    }

    pub fn position(&self) -> Point<Pixels> {
        self.position
    }

    pub fn set_position(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.position = position;
        self.visible = true;
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        cx.notify();
    }

    pub fn set_pressed(&mut self, pressed: bool, cx: &mut Context<Self>) {
        self.pressed = pressed;
        if pressed {
            self.ripples.push((self.position, Instant::now()));
        }
        cx.notify();
    }
}

impl Render for DemoCursor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.ripples
            .retain(|(_, start)| now.duration_since(*start) < RIPPLE);
        if !self.ripples.is_empty() {
            window.request_animation_frame();
        }
        let accent = crate::theme::palette(cx).accent;
        let ripples = self.ripples.iter().map(|(at, start)| {
            let t = now.duration_since(*start).as_secs_f32() / RIPPLE.as_secs_f32();
            let eased = 1. - (1. - t).powi(3);
            let radius = px(5. + 21. * eased);
            div()
                .absolute()
                .left(at.x - radius)
                .top(at.y - radius)
                .size(radius * 2.)
                .rounded_full()
                .border_2()
                .border_color(accent.opacity(0.95 * (1. - t)))
                .bg(accent.opacity(0.28 * (1. - t)))
        });
        // Pressing shrinks the arrow towards its tip.
        let scale = if self.pressed { 0.86 } else { 1. };
        let (width, height) = (px(SIZE.width * scale), px(SIZE.height * scale));
        let left = self.position.x - px(HOTSPOT.x * scale);
        let top = self.position.y - px(HOTSPOT.y * scale);
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(ripples)
            .child(
                img(ASSET)
                    .absolute()
                    .left(left)
                    .top(top)
                    .w(width)
                    .h(height)
                    // Always laid out, so the image is loaded before the cursor first shows.
                    .when(!self.visible, |this| this.opacity(0.)),
            )
    }
}
