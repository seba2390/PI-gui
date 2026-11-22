//! The "working" indicator: a little π that spins while it pulses, shrinking and growing.
//!
//! The animation is stepped by the caller's existing ticker (the chat's and the sidebar's,
//! every 80 ms while an agent works), so it adds no timers or per-frame work of its own.

use gpui_kit::component::Icon;
use gpui_kit::*;

use super::AppIcon;

/// Frames per full turn: about 1.3 s at the 80 ms tick.
const TURN_FRAMES: usize = 16;
/// Frames per pulse (small → big → small): two pulses per turn.
const PULSE_FRAMES: usize = 8;
/// The pulse's smallest and largest scale.
const PULSE: (f32, f32) = (0.72, 1.08);

/// How the π is drawn at one frame: its turn (radians) and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub turn: f32,
    pub scale: f32,
}

/// The pose at animation frame `frame`.
#[allow(clippy::cast_precision_loss)]
pub fn pose(frame: usize) -> Pose {
    let turn = (frame % TURN_FRAMES) as f32 / TURN_FRAMES as f32 * std::f32::consts::TAU;
    let pulse = (frame % PULSE_FRAMES) as f32 / PULSE_FRAMES as f32 * std::f32::consts::TAU;
    // 0 at the smallest, 1 at the largest; starts small and swells.
    let swell = (1. - pulse.cos()) / 2.;
    Pose {
        turn,
        scale: PULSE.0 + (PULSE.1 - PULSE.0) * swell,
    }
}

/// The π at `frame` of its spin, `size` square, in `color`. The box keeps its size; only the
/// π turns and pulses inside it, so nothing around it moves.
pub fn spinning_pi(frame: usize, size: Pixels, color: Hsla) -> Div {
    let pose = pose(frame);
    let transform = Transformation::scale(gpui_kit::size(pose.scale, pose.scale))
        .with_rotation(radians(pose.turn));
    div().flex_none().size(size).child(
        Icon::from(AppIcon::PiLogo)
            .size(size)
            .text_color(color)
            .transform(transform),
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
    use super::{PULSE, PULSE_FRAMES, TURN_FRAMES, pose};

    #[test]
    fn spins_a_full_turn_and_comes_round() {
        assert_eq!(pose(0).turn, 0.);
        let half = pose(TURN_FRAMES / 2).turn;
        assert!((half - std::f32::consts::PI).abs() < 1e-5);
        assert_eq!(pose(TURN_FRAMES), pose(0), "loops seamlessly");
        // Always turning the same way.
        for frame in 1..TURN_FRAMES {
            assert!(pose(frame).turn > pose(frame - 1).turn);
        }
    }

    #[test]
    fn pulses_between_small_and_big() {
        assert!((pose(0).scale - PULSE.0).abs() < 1e-5, "starts small");
        assert!(
            (pose(PULSE_FRAMES / 2).scale - PULSE.1).abs() < 1e-5,
            "swells to full"
        );
        for frame in 0..TURN_FRAMES * 2 {
            let scale = pose(frame).scale;
            assert!((PULSE.0 - 1e-5..=PULSE.1 + 1e-5).contains(&scale));
        }
    }
}
