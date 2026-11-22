//! The "working" indicator: a little π that pulses, gently shrinking and growing.
//!
//! The animation is stepped by the caller's existing ticker (the chat's and the sidebar's,
//! every 80 ms while an agent works), so it adds no timers or per-frame work of its own.

use gpui_kit::component::Icon;
use gpui_kit::*;

use super::AppIcon;

/// Frames per pulse (small → big → small): about 1.3 s at the 80 ms tick, a calm breath.
const PULSE_FRAMES: usize = 16;
/// The pulse's smallest and largest scale.
const PULSE: (f32, f32) = (0.72, 1.08);

/// The π's scale at animation frame `frame`.
#[allow(clippy::cast_precision_loss)]
pub fn scale(frame: usize) -> f32 {
    let phase = (frame % PULSE_FRAMES) as f32 / PULSE_FRAMES as f32 * std::f32::consts::TAU;
    // 0 at the smallest, 1 at the largest; starts small and swells.
    let swell = (1. - phase.cos()) / 2.;
    PULSE.0 + (PULSE.1 - PULSE.0) * swell
}

/// The π at `frame` of its pulse, `size` square, in `color`. The box keeps its size; only the
/// π grows and shrinks inside it, so nothing around it moves.
pub fn pulsing_pi(frame: usize, size: Pixels, color: Hsla) -> Div {
    let scale = scale(frame);
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
    use super::{PULSE, PULSE_FRAMES, scale};

    #[test]
    fn pulses_between_small_and_big() {
        assert!((scale(0) - PULSE.0).abs() < 1e-5, "starts small");
        assert!(
            (scale(PULSE_FRAMES / 2) - PULSE.1).abs() < 1e-5,
            "swells to full"
        );
        assert!(
            (scale(PULSE_FRAMES) - scale(0)).abs() < 1e-6,
            "loops seamlessly"
        );
        for frame in 0..PULSE_FRAMES * 2 {
            assert!((PULSE.0 - 1e-5..=PULSE.1 + 1e-5).contains(&scale(frame)));
        }
        // Grows, then shrinks.
        assert!(scale(3) > scale(2) && scale(12) < scale(11));
    }
}
