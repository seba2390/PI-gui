//! The "working" indicator: a little π that pulses, gently shrinking and growing.
//!
//! The pulse follows the wall clock, not a frame count, so it stays smooth however the frames
//! fall, and every π on screen (chat and sidebar) breathes in step. Views showing one repaint
//! every [`FRAME`] while an agent works, and not at all otherwise.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui_kit::component::Icon;
use gpui_kit::*;

use super::AppIcon;

/// How often a view showing the π repaints while an agent works: 30 fps is smooth for a slow
/// pulse, and it only runs while something is working.
pub const FRAME: Duration = Duration::from_millis(33);
/// One breath (small → big → small).
const PERIOD: f32 = 1.4;
/// The pulse's smallest and largest scale.
const PULSE: (f32, f32) = (0.72, 1.08);

/// Seconds since the first π was drawn: the shared clock of every pulse.
fn clock() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// How far into a breath `seconds` is: 0 at the smallest, 1 at the largest, eased (a cosine),
/// so it slows at both ends like breathing.
pub fn breath_at(seconds: f32) -> f32 {
    let phase = (seconds / PERIOD).fract() * std::f32::consts::TAU;
    (1. - phase.cos()) / 2.
}

/// The current breath, for things that pulse along with the π (the "Working…" label).
pub fn breath() -> f32 {
    breath_at(clock())
}

/// The π's scale `seconds` into the pulse.
pub fn scale_at(seconds: f32) -> f32 {
    PULSE.0 + (PULSE.1 - PULSE.0) * breath_at(seconds)
}

/// The π as it is now, `size` square, in `color`. The box keeps its size; only the π grows
/// and shrinks inside it, so nothing around it moves.
pub fn pulsing_pi(size: Pixels, color: Hsla) -> Div {
    let scale = scale_at(clock());
    div().flex_none().size(size).child(
        Icon::from(AppIcon::PiLogo)
            .size(size)
            .text_color(color)
            .transform(Transformation::scale(gpui_kit::size(scale, scale))),
    )
}

/// A still π, for places that mark finished work ("Worked for 12s").
pub fn resting_pi(size: Pixels, color: Hsla) -> Div {
    div()
        .flex_none()
        .size(size)
        .child(Icon::from(AppIcon::PiLogo).size(size).text_color(color))
}

#[cfg(test)]
mod tests {
    use super::{FRAME, PERIOD, PULSE, scale_at};

    #[test]
    fn pulses_between_small_and_big() {
        assert!((scale_at(0.) - PULSE.0).abs() < 1e-5, "starts small");
        assert!(
            (scale_at(PERIOD / 2.) - PULSE.1).abs() < 1e-5,
            "swells to full"
        );
        assert!(
            (scale_at(PERIOD) - scale_at(0.)).abs() < 1e-4,
            "loops seamlessly"
        );
        for step in 0..200 {
            #[allow(clippy::cast_precision_loss)]
            let scale = scale_at(step as f32 * 0.037);
            assert!((PULSE.0 - 1e-5..=PULSE.1 + 1e-5).contains(&scale));
        }
    }

    #[test]
    fn frames_change_it_by_small_steps() {
        // Smooth: no frame moves the π by more than a few percent of its size.
        let frame = FRAME.as_secs_f32();
        for step in 0..100 {
            #[allow(clippy::cast_precision_loss)]
            let t = step as f32 * frame;
            assert!((scale_at(t + frame) - scale_at(t)).abs() < 0.05);
        }
    }
}
