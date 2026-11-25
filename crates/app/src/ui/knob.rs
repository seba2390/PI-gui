//! Sliders with an outlined knob.
//!
//! gpui-kit draws a slider's knob as a plain disc in the theme's `slider_thumb` colour, which
//! is white in the light theme and so vanishes on a white page. Here the disc is made
//! transparent and a knob with a hairline edge and a soft shadow (like macOS's own) is drawn
//! in its place. gpui-kit keeps the dragging, the keys and its hover and focus ring.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::*;

/// gpui-kit's knob: 16 px, centred on the 24 px high track.
const KNOB: Pixels = px(16.);
const TRACK_HEIGHT: Pixels = px(24.);

/// `slider` (for `state`, linear) with the outlined knob. It takes the width it is given.
/// The view must re-render while the slider moves (they all do: their labels follow it).
pub fn outlined(slider: Slider, state: &Entity<SliderState>, cx: &App) -> Div {
    let state = state.read(cx);
    let (min, max) = (state.min_value(), state.max_value());
    let at = if max > min {
        ((state.value().end() - min) / (max - min)).clamp(0., 1.)
    } else {
        0.
    };
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    let (edge, shadow) = if dark { (0.45, 0.5) } else { (0.16, 0.22) };
    div()
        .relative()
        .w_full()
        .child(slider.text_color(transparent_black()))
        .child(
            div()
                .absolute()
                .top((TRACK_HEIGHT - KNOB) / 2.)
                .left(relative(at))
                .ml(-KNOB / 2.)
                .size(KNOB)
                .rounded_full()
                .bg(theme.slider_thumb)
                .border_1()
                .border_color(hsla(0., 0., 0., edge))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., shadow),
                    offset: point(px(0.), px(1.)),
                    blur_radius: px(3.),
                    spread_radius: px(0.),
                    inset: false,
                }]),
        )
}
