//! Where the pages of a file are: one below the other, centred.

use std::ops::Range;

use super::Size;

/// The space around and between the pages.
pub const GAP: f32 = 12.;
/// Pages are not made wider than this to fill the view.
const MAX_FIT_WIDTH: f32 = 900.;
pub const MIN_SCALE: f32 = 0.2;
pub const MAX_SCALE: f32 = 6.;
/// The steps of the zoom buttons.
const STEPS: [f32; 13] = [
    0.25, 0.5, 0.67, 0.75, 0.9, 1., 1.25, 1.5, 2., 2.5, 3., 4., 6.,
];

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    scale: f32,
    sizes: Vec<Size>,
    tops: Vec<f32>,
    /// The size of everything.
    pub width: f32,
    pub height: f32,
}

/// A place in a file that stays the same when the scale changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub page: usize,
    /// In points from the top of the page.
    pub y: f32,
}

/// A page's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The scale at which the widest page fills a view `width` wide.
pub fn fit(sizes: &[Size], width: f32) -> f32 {
    let widest = sizes.iter().map(|size| size.width).fold(1., f32::max);
    let room = (width - 2. * GAP).clamp(1., MAX_FIT_WIDTH);
    (room / widest).clamp(MIN_SCALE, MAX_SCALE)
}

/// The next step of zooming in or out from `scale`.
pub fn step(scale: f32, zoom_in: bool) -> f32 {
    // Not a step that looks the same.
    let next = if zoom_in {
        STEPS.iter().find(|step| **step > scale * 1.01)
    } else {
        STEPS.iter().rev().find(|step| **step < scale * 0.99)
    };
    next.copied().unwrap_or(scale)
}

impl Layout {
    /// `sizes` at `scale`, in a view `view` wide.
    pub fn new(sizes: &[Size], scale: f32, view: f32) -> Self {
        let mut tops = Vec::with_capacity(sizes.len());
        let mut y = GAP;
        let mut widest: f32 = 0.;
        for size in sizes {
            tops.push(y);
            y += size.height * scale + GAP;
            widest = widest.max(size.width * scale);
        }
        Self {
            scale,
            sizes: sizes.to_vec(),
            tops,
            width: view.max(widest + 2. * GAP),
            height: y,
        }
    }

    /// How many pages there are.
    pub fn count(&self) -> usize {
        self.sizes.len()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn frame(&self, page: usize) -> Frame {
        let size = self.sizes[page];
        let width = size.width * self.scale;
        Frame {
            x: (self.width - width) / 2.,
            y: self.tops[page],
            width,
            height: size.height * self.scale,
        }
    }

    /// The pages between `top` and `top + height`.
    pub fn visible(&self, top: f32, height: f32) -> Range<usize> {
        let first = self.page_at(top);
        let end = self.tops.partition_point(|y| *y < top + height);
        first..end.max(first + 1).min(self.sizes.len())
    }

    /// The page at `y`, or the last one before it.
    pub fn page_at(&self, y: f32) -> usize {
        self.tops.partition_point(|top| *top <= y).saturating_sub(1)
    }

    pub fn place(&self, y: f32) -> Place {
        let page = self.page_at(y);
        Place {
            page,
            y: self.tops.get(page).map_or(0., |top| (y - top) / self.scale),
        }
    }

    pub fn y(&self, place: Place) -> f32 {
        self.tops
            .get(place.page)
            .map_or(0., |top| top + place.y * self.scale)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // computed the same way
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn sizes() -> Vec<Size> {
        [(100., 200.), (200., 100.), (100., 200.)]
            .map(|(width, height)| Size { width, height })
            .to_vec()
    }

    #[test]
    fn pages_are_below_each_other() {
        let layout = Layout::new(&sizes(), 2., 300.);
        assert_eq!((layout.width, layout.height), (424., 1048.));
        let frames: Vec<_> = (0..3).map(|page| layout.frame(page)).collect();
        assert_eq!(
            frames[1],
            Frame {
                x: 12.,
                y: 424.,
                width: 400.,
                height: 200.
            }
        );
        assert_eq!(frames[0].x, 112.);
        assert_eq!(layout.visible(0., 100.), 0..1);
        assert_eq!(layout.visible(420., 10.), 0..2);
        assert_eq!(layout.visible(500., 400.), 1..3);
        assert_eq!(layout.visible(5000., 10.), 2..3);
        assert_eq!(layout.visible(-50., 10.), 0..1);
        // Room to spare.
        assert_eq!(Layout::new(&sizes(), 1., 1000.).frame(0).x, 450.);
        assert_eq!(Layout::new(&[], 1., 10.).visible(0., 10.), 0..0);
    }

    #[test]
    fn places_survive_zooming() {
        let sizes = sizes();
        let (small, large) = (Layout::new(&sizes, 1., 9.), Layout::new(&sizes, 3., 9.));
        for y in [0., 12., 100., 230., 400.] {
            let place = small.place(y);
            assert!(
                (large.place(large.y(place)).y - place.y).abs() < 0.01,
                "{y}"
            );
            assert!((small.y(place) - y).abs() < 0.01, "{y}");
        }
        assert_eq!(small.place(324.).page, 1);
    }

    #[test]
    fn scales() {
        assert_eq!(fit(&sizes(), 424.), 2.);
        assert_eq!(fit(&sizes(), 5000.), 4.5);
        assert_eq!(fit(&sizes(), 0.), MIN_SCALE);
        assert_eq!(step(1., true), 1.25);
        assert_eq!(step(1.249, true), 1.5);
        assert_eq!(step(1.1, false), 1.);
        assert_eq!(step(6., true), 6.);
        assert_eq!(step(0.2, false), 0.2);
    }
}
