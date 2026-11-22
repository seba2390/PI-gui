//! The "working" indicator: a little π that hops, Luxo style. It crouches, springs up
//! (stretching), lands with a squash and rests a moment; every third hop it flips over in
//! the air.
//!
//! The animation is a fixed table of poses stepped by the caller's existing ticker (the chat's
//! and the sidebar's, while an agent works), so it adds no timers or per-frame work of its own.

use gpui_kit::component::Icon;
use gpui_kit::*;

use super::AppIcon;

/// One pose: how far the π is off the ground (as a fraction of its size), its squash and
/// stretch, and how far it has turned (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub lift: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub turn: f32,
}

/// `(lift, scale_x, scale_y, lean)` per frame of one hop. Frames come every 80 ms, so a hop
/// lasts about 1.3 s: anticipation, flight, landing, rest.
const HOP: [(f32, f32, f32, f32); 16] = [
    (0.0, 1.00, 1.00, 0.0),
    (0.0, 1.10, 0.86, 0.0),
    (0.0, 1.18, 0.74, 0.0),    // crouch
    (0.15, 0.86, 1.18, -0.12), // take off, stretched
    (0.38, 0.92, 1.10, -0.06),
    (0.52, 0.98, 1.02, 0.0),
    (0.56, 1.00, 1.00, 0.0), // top
    (0.50, 1.00, 1.00, 0.04),
    (0.34, 0.96, 1.05, 0.08),
    (0.12, 0.90, 1.12, 0.10), // falling, stretched
    (0.0, 1.22, 0.72, 0.0),   // land: squash
    (0.0, 0.94, 1.07, 0.0),
    (0.0, 1.03, 0.97, 0.0),
    (0.0, 1.00, 1.00, 0.0),
    (0.0, 1.00, 1.00, 0.0),
    (0.0, 1.00, 1.00, 0.0),
];

/// Frames in the air (take off to touch down), over which a flip turns once.
const AIRBORNE: std::ops::Range<usize> = 3..10;

/// The pose at animation frame `frame`.
pub fn pose(frame: usize) -> Pose {
    let step = frame % HOP.len();
    let hop = frame / HOP.len();
    let (lift, scale_x, scale_y, lean) = HOP[step];
    // Every third hop is a full forward flip instead of a lean.
    let turn = if hop % 3 == 2 && AIRBORNE.contains(&step) {
        #[allow(clippy::cast_precision_loss)]
        let progress = (step - AIRBORNE.start + 1) as f32 / AIRBORNE.len() as f32;
        progress * std::f32::consts::TAU
    } else {
        lean
    };
    Pose {
        lift,
        scale_x,
        scale_y,
        turn,
    }
}

/// The π at `frame` of its hop, `size` square, in `color`. The box keeps its size; the π
/// moves inside (and above) it, so nothing around it shifts.
pub fn hopping_pi(frame: usize, size: Pixels, color: Hsla) -> Div {
    let pose = pose(frame);
    // Scaling is about the centre: move a squashed π down so it stays on the ground.
    let planted = size * (1. - pose.scale_y) / 2.;
    let transform = Transformation::scale(gpui_kit::size(pose.scale_x, pose.scale_y))
        .with_translation(point(px(0.), planted - size * pose.lift))
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
    use super::{AIRBORNE, HOP, pose};

    #[test]
    fn hops_on_the_ground_and_lands_squashed() {
        for frame in 0..HOP.len() * 3 {
            let pose = pose(frame);
            assert!(pose.lift >= 0., "never below the ground");
            if pose.lift == 0. {
                // On the ground the π keeps roughly its area (squash, not shrink).
                let area = pose.scale_x * pose.scale_y;
                assert!((0.8..=1.1).contains(&area), "frame {frame}: area {area}");
            }
        }
        let landing = pose(AIRBORNE.end);
        assert!(
            landing.scale_y < 0.8 && landing.scale_x > 1.1,
            "squash on landing"
        );
        assert!(pose(6).lift > 0.5, "jumps about half its height");
    }

    #[test]
    fn every_third_hop_flips() {
        let flip_hop = 2 * HOP.len();
        let turned = pose(flip_hop + AIRBORNE.end - 1).turn;
        assert!(
            (turned - std::f32::consts::TAU).abs() < 1e-5,
            "a full turn by touchdown"
        );
        // Other hops only lean.
        for frame in 0..2 * HOP.len() {
            assert!(pose(frame).turn.abs() < 0.2);
        }
        assert_eq!(pose(flip_hop + AIRBORNE.end).turn, 0.);
    }
}
