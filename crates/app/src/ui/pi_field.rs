//! The background of the welcome screen and of new, empty chats: a field of small π glyphs
//! that drift out of the way of the mouse and spring back, and fade away when the conversation
//! starts.
//!
//! Built to cost nothing while idle: each glyph is one GPU sprite of the cached `pi-logo` SVG
//! (three sizes, so three atlas tiles), the physics is a few float operations per glyph, and
//! frames are only produced (at most [`FRAME`]) while something moves. Once every glyph has
//! settled the timer stops; no frames, no wake-ups.

use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

const GLYPH: &str = "icons/pi-logo.svg";
/// Frame interval while animating (~60 fps, also on 120 Hz displays).
const FRAME: Duration = Duration::from_millis(16);
/// Glyph sizes in pixels; few distinct sizes keep the sprite atlas at three tiles.
const SIZES: [f32; 3] = [9., 12., 16.];
/// Average distance between glyphs.
const SPACING: f32 = 74.;
const MAX_GLYPHS: usize = 160;
/// How far the mouse reaches, and how hard it pushes (px/s²).
const REACH: f32 = 130.;
const PUSH: f32 = 2600.;
/// How much of the mouse's own motion is passed on to nearby glyphs.
const DRAG: f32 = 0.35;
/// The spring back home (1/s²) and its damping (1/s): slightly underdamped, so glyphs wobble.
const SPRING: f32 = 38.;
const DAMPING: f32 = 7.5;
/// The opening drift into place.
const INTRO: Duration = Duration::from_millis(900);
/// The fade when the conversation starts.
const FADE: Duration = Duration::from_millis(700);

struct Glyph {
    /// Resting place, relative to the field's top-left corner.
    home: Point<f32>,
    offset: Point<f32>,
    velocity: Point<f32>,
    size: f32,
    angle: f32,
    /// Base opacity, before the glow near the mouse and the fade behind the headline.
    alpha: f32,
    green: bool,
}

pub struct PiField {
    glyphs: Vec<Glyph>,
    /// The size the glyphs were laid out for.
    laid_out: Size<f32>,
    /// The mouse, relative to the field, while it is over it.
    mouse: Option<Point<f32>>,
    /// Mouse movement since the last frame, handed on to nearby glyphs.
    mouse_moved: Point<f32>,
    intro_started: Option<Instant>,
    /// When the fade-out started ([`Self::fade_out`]).
    fade_started: Option<Instant>,
    ticker: Option<Task<()>>,
    last_tick: Instant,
    /// Painted since the last tick: when the field is not on screen, animation stops.
    painted: bool,
    /// A headline in the middle, which the glyphs are kept faint behind.
    headline: bool,
}

impl PiField {
    pub fn new() -> Self {
        Self {
            glyphs: Vec::new(),
            laid_out: Size::default(),
            mouse: None,
            mouse_moved: Point::default(),
            intro_started: None,
            fade_started: None,
            ticker: None,
            last_tick: Instant::now(),
            painted: false,
            headline: true,
        }
    }

    /// A field without a headline in its middle.
    pub fn plain() -> Self {
        Self {
            headline: false,
            ..Self::new()
        }
    }

    /// Plays the opening drift again (when the welcome screen is shown anew).
    pub fn replay(&mut self, cx: &mut Context<Self>) {
        self.laid_out = Size::default();
        self.fade_started = None;
        cx.notify();
    }

    /// Fades the glyphs away (the first message was sent).
    pub fn fade_out(&mut self, cx: &mut Context<Self>) {
        if self.fade_started.is_none() {
            self.fade_started = Some(Instant::now());
            self.wake(cx);
        }
    }

    /// Whether the fade-out has finished (nothing is drawn any more).
    pub fn is_faded(&self) -> bool {
        self.fade_started
            .is_some_and(|start| start.elapsed() >= FADE)
    }

    /// 1 before the fade-out, easing to 0 as it runs.
    fn visibility(&self) -> f32 {
        self.fade_started.map_or(1., |start| {
            let t = (start.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.);
            1. - t * t * (3. - 2. * t)
        })
    }

    /// Lays the glyphs out on a jittered grid (even, never clumped), the same for the same
    /// size, and starts the opening drift.
    fn lay_out(&mut self, size: Size<f32>) {
        self.laid_out = size;
        // Spread out on very large screens rather than exceed the glyph budget.
        #[allow(clippy::cast_precision_loss)]
        let spacing = SPACING.max((size.width * size.height / MAX_GLYPHS as f32).sqrt() * 1.05);
        let columns = (size.width / spacing).ceil().max(1.);
        let rows = (size.height / spacing).ceil().max(1.);
        let (cell_w, cell_h) = (size.width / columns, size.height / rows);
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        self.glyphs.clear();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        for row in 0..rows as u32 {
            for column in 0..columns as u32 {
                if self.glyphs.len() == MAX_GLYPHS {
                    break;
                }
                #[allow(clippy::cast_precision_loss)]
                let home = Point::new(
                    (column as f32 + 0.15 + 0.7 * random.next()) * cell_w,
                    (row as f32 + 0.15 + 0.7 * random.next()) * cell_h,
                );
                let size_class = random.next();
                self.glyphs.push(Glyph {
                    home,
                    // Start a little off home; the springs bring them in.
                    offset: Point::new(random.signed() * 26., 18. + random.next() * 22.),
                    velocity: Point::default(),
                    size: if size_class < 0.5 {
                        SIZES[0]
                    } else if size_class < 0.85 {
                        SIZES[1]
                    } else {
                        SIZES[2]
                    },
                    angle: random.signed() * 0.45,
                    alpha: 0.10 + 0.14 * random.next(),
                    green: random.next() < 0.28,
                });
            }
        }
        self.intro_started = Some(Instant::now());
    }

    fn pointer(&mut self, position: Option<Point<f32>>, cx: &mut Context<Self>) {
        if let (Some(old), Some(new)) = (self.mouse, position) {
            self.mouse_moved += new - old;
        }
        self.mouse = position;
        self.wake(cx);
    }

    /// Animates until everything has settled.
    fn wake(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.last_tick = Instant::now();
        // The field is on screen: it was painted when the event that woke it arrived.
        self.painted = true;
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let now = Instant::now();
                        let dt = (now - this.last_tick).as_secs_f32();
                        this.last_tick = now;
                        let keep = this.painted && this.step(dt);
                        this.painted = false;
                        cx.notify();
                        if !keep {
                            this.ticker = None;
                        }
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    /// Advances the physics by `dt` seconds; `false` once nothing moves any more.
    fn step(&mut self, dt: f32) -> bool {
        // A late frame must not make the springs overshoot.
        let dt = dt.clamp(0.001, 1. / 30.);
        let drag = std::mem::take(&mut self.mouse_moved);
        let mut moving = self
            .intro_started
            .is_some_and(|start| start.elapsed() < INTRO)
            || self
                .fade_started
                .is_some_and(|start| start.elapsed() < FADE);
        if self.is_faded() {
            return false;
        }
        for glyph in &mut self.glyphs {
            let position = glyph.home + glyph.offset;
            let mut accel = Point::new(
                -SPRING * glyph.offset.x - DAMPING * glyph.velocity.x,
                -SPRING * glyph.offset.y - DAMPING * glyph.velocity.y,
            );
            if let Some(mouse) = self.mouse {
                let away = position - mouse;
                let distance = away.x.hypot(away.y);
                if distance < REACH && distance > 0.01 {
                    let falloff = (1. - distance / REACH).powi(2);
                    let push = PUSH * falloff / distance;
                    accel += Point::new(away.x * push, away.y * push);
                    // The mouse's wake: nearby glyphs are swept along with it.
                    glyph.velocity += drag * (DRAG * falloff);
                }
            }
            glyph.velocity += accel * dt;
            glyph.offset += glyph.velocity * dt;
            let speed = glyph.velocity.x.abs() + glyph.velocity.y.abs();
            let force = accel.x.abs() + accel.y.abs();
            if speed > 0.6 || force > 6. {
                moving = true;
            }
        }
        moving
    }

    fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        grey: Hsla,
        green: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = Size::new(f32::from(bounds.size.width), f32::from(bounds.size.height));
        if (area.width - self.laid_out.width).abs() > 0.5
            || (area.height - self.laid_out.height).abs() > 0.5
        {
            self.lay_out(area);
            self.wake(cx);
        }
        self.painted = true;
        let visibility = self.visibility();
        if visibility <= 0. {
            return;
        }
        let intro = self.intro_started.map_or(1., |start| {
            (start.elapsed().as_secs_f32() / INTRO.as_secs_f32()).min(1.)
        });
        let intro = intro * intro * (3. - 2. * intro);
        let center = Point::new(area.width / 2., area.height / 2.);
        let scale = window.scale_factor();
        for glyph in &self.glyphs {
            let position = glyph.home + glyph.offset;
            // Fade behind the headline and button so they stay readable.
            let dx = (position.x - center.x) / 300.;
            let dy = (position.y - center.y) / 160.;
            let clear = if self.headline {
                smoothstep(0.55, 1.15, dx.hypot(dy))
            } else {
                1.
            };
            // Glow near the mouse and while displaced.
            let near = self.mouse.map_or(0., |mouse| {
                let away = position - mouse;
                (1. - away.x.hypot(away.y) / (REACH * 1.3)).max(0.)
            });
            let stirred = (glyph.offset.x.abs() + glyph.offset.y.abs()) / 60.;
            let glow = (near * near + stirred * 0.5).min(1.);
            let color = if glyph.green || glow > 0.35 {
                green
            } else {
                grey
            };
            let alpha = (glyph.alpha + glow * 0.45) * (0.25 + 0.75 * clear) * intro * visibility;
            if alpha < 0.01 {
                continue;
            }
            let origin = point(
                bounds.origin.x + px(position.x - glyph.size / 2.),
                bounds.origin.y + px(position.y - glyph.size / 2.),
            );
            let glyph_bounds = Bounds::new(origin, size(px(glyph.size), px(glyph.size)));
            // Turned a little by how far it was pushed sideways, about its own center.
            let angle = glyph.angle + glyph.offset.x * 0.012;
            let transformation = TransformationMatrix::unit()
                .translate(glyph_bounds.center().scale(scale))
                .rotate(radians(angle))
                .translate(glyph_bounds.center().scale(-scale));
            let _ = window.paint_svg(
                glyph_bounds,
                GLYPH.into(),
                None,
                transformation,
                color.opacity(alpha),
                cx,
            );
        }
    }
}

impl Render for PiField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grey = cx.theme().muted_foreground;
        let green = crate::theme::palette(cx).accent;
        let this = cx.entity();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                // Every mouse move in the window (also over the headline and button on top).
                let listener = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let inside = bounds.contains(&event.position).then(|| {
                        let local = event.position - bounds.origin;
                        Point::new(f32::from(local.x), f32::from(local.y))
                    });
                    listener.update(cx, |field, cx| {
                        if inside.is_some() || field.mouse.is_some() {
                            field.pointer(inside, cx);
                        }
                    });
                });
                this.update(cx, |field, cx| field.paint(bounds, grey, green, window, cx));
            },
        )
        .size_full()
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// A tiny deterministic generator (xorshift64*): the same layout on every launch.
struct Random(u64);

impl Random {
    /// Uniform in `0..1`.
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        #[allow(clippy::cast_precision_loss)]
        let bits = (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32;
        bits / 16_777_216.
    }

    /// Uniform in `-1..1`.
    fn signed(&mut self) -> f32 {
        self.next() * 2. - 1.
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Point, Size};

    use std::time::Instant;

    use super::{FADE, FRAME, MAX_GLYPHS, PiField};

    #[test]
    fn settles_after_a_push_and_stays_idle() {
        let mut field = PiField::new();
        field.lay_out(Size::new(800., 600.));
        field.intro_started = None;
        field.mouse = Some(Point::new(400., 300.));
        let mut frames = 0;
        // Simulated 60 fps: the physics must come to rest well within a few seconds.
        while frames < 600 {
            if !field.step(FRAME.as_secs_f32()) {
                break;
            }
            frames += 1;
        }
        assert!(frames < 400, "still moving after {frames} frames");
        let pushed = field
            .glyphs
            .iter()
            .map(|glyph| glyph.offset.x.hypot(glyph.offset.y))
            .fold(0., f32::max);
        assert!(pushed > 10., "the mouse should push nearby glyphs away");

        field.mouse = None;
        for _ in 0..600 {
            if !field.step(FRAME.as_secs_f32()) {
                break;
            }
        }
        assert!(
            field
                .glyphs
                .iter()
                .all(|glyph| glyph.offset.x.hypot(glyph.offset.y) < 1.),
            "glyphs should return home"
        );
    }

    #[test]
    fn layout_is_even_and_bounded() {
        let mut field = PiField::new();
        field.lay_out(Size::new(1000., 700.));
        assert!((80..=MAX_GLYPHS).contains(&field.glyphs.len()));
        field.lay_out(Size::new(5000., 4000.));
        assert!(field.glyphs.len() <= MAX_GLYPHS);
        let lowest = field
            .glyphs
            .iter()
            .map(|glyph| glyph.home.y)
            .fold(0., f32::max);
        assert!(lowest > 3500., "glyphs must cover the whole field");
    }

    #[test]
    fn a_finished_fade_stops_everything() {
        let mut field = PiField::new();
        field.lay_out(Size::new(800., 600.));
        assert!((field.visibility() - 1.).abs() < f32::EPSILON);
        // Mid-fade it keeps animating, even with every glyph at rest.
        field.intro_started = None;
        field.fade_started = Some(Instant::now());
        assert!(field.step(FRAME.as_secs_f32()));
        assert!(!field.is_faded());
        // Once it has run, nothing moves or is drawn, even with the mouse over it.
        field.fade_started = Instant::now().checked_sub(FADE * 2);
        field.mouse = Some(Point::new(400., 300.));
        assert!(field.is_faded());
        assert!(field.visibility() <= 0.);
        assert!(!field.step(FRAME.as_secs_f32()));
    }
}
