//! A line of text that fades out at the end where it does not fit.

use std::sync::Arc;

use gpui_kit::*;

/// How far before the end the text starts to fade.
const FADE: Pixels = px(48.);

/// How narrow the text may become (shorter text stays whole).
const MIN_WIDTH: Pixels = px(56.);

/// One line of text in the surrounding style. In a narrow row it gives way and fades out at the
/// end. The letters themselves fade (nothing is drawn over them), so it works on any
/// background, also under the pointer.
pub struct FadingText(SharedString);

impl FadingText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        // A line break (folder names may have one) can't be shaped.
        Self(text.into().replace(['\n', '\r'], " ").into())
    }

    fn layout(&self, window: &Window) -> (Arc<LineLayout>, Pixels) {
        let style = window.text_style();
        let rem = window.rem_size();
        let run = style.to_run(self.0.len());
        let line =
            window
                .text_system()
                .layout_line(&self.0, style.font_size.to_pixels(rem), &[run], None);
        (line, style.line_height_in_pixels(rem))
    }
}

impl Element for FadingText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let (line, line_height) = self.layout(window);
        let natural = line.width.ceil();
        let layout = window.request_measured_layout(
            Style::default(),
            move |known, available: Size<AvailableSpace>, _, _| {
                let width = known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(width) => natural.min(width),
                    AvailableSpace::MinContent => natural.min(MIN_WIDTH),
                    AvailableSpace::MaxContent => natural,
                });
                size(width, known.height.unwrap_or(line_height))
            },
        );
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let (line, line_height) = self.layout(window);
        let color = window.text_style().color;
        let ends = self
            .0
            .char_indices()
            .map(|(start, c)| (c.len_utf8(), line.x_for_index(start + c.len_utf8())));
        let runs: Vec<DecorationRun> = opacities(ends, line.width, bounds.size.width)
            .into_iter()
            .map(|(len, opacity)| DecorationRun {
                #[allow(clippy::cast_possible_truncation)]
                len: len as u32,
                color: color.opacity(opacity),
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        let _ = line.paint(
            bounds.origin,
            line_height,
            TextAlign::Left,
            None,
            &runs,
            window,
            cx,
        );
    }
}

/// The opacity of each character, given as its length in bytes and where it ends, of a text of
/// width `natural` shown in `width`. A character that does not fit is not seen at all, so none
/// is cut.
fn opacities(
    ends: impl Iterator<Item = (usize, Pixels)>,
    natural: Pixels,
    width: Pixels,
) -> Vec<(usize, f32)> {
    // Layout rounds to device pixels, which must not fade a text that fits.
    let fits = natural <= width + px(1.);
    ends.map(|(len, end)| {
        let opacity = if fits {
            1.
        } else {
            ((width - end) / FADE).clamp(0., 1.)
        };
        (len, opacity)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{FADE, opacities};
    use gpui_kit::{Pixels, px};

    /// `count` characters of 10px, one byte each.
    fn at(count: u16, width: Pixels) -> Vec<f32> {
        let ends = (1..=count).map(|n| (1, px(10. * f32::from(n))));
        opacities(ends, px(10. * f32::from(count)), width)
            .into_iter()
            .map(|(_, opacity)| opacity)
            .collect()
    }

    #[test]
    fn text_that_fits_is_whole() {
        assert_eq!(at(8, px(200.)), [1.; 8]);
        assert_eq!(at(8, px(80.)), [1.; 8]);
        assert_eq!(at(8, px(79.5)), [1.; 8], "rounded");
    }

    #[test]
    fn text_that_does_not_fit_fades_out() {
        let faded = at(40, px(205.));
        let full = faded.iter().take_while(|opacity| **opacity >= 1.).count();
        assert_eq!(full, 15, "letters that end before the fade are whole");
        assert!(faded.windows(2).all(|pair| pair[0] >= pair[1]));
        // It begins within the fade and is gone where the room ends.
        assert!(faded[full] > 0.8 && px(10. * 16.) > px(205.) - FADE);
        assert!(faded[19] > 0. && faded[19] < 0.2);
        assert!(faded[20..].iter().all(|opacity| *opacity <= 0.));
    }
}
