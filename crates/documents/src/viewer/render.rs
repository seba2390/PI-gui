//! Drawing pages.

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterSettings, SoftMask, TransformExt as _, interpret_page,
};
use hayro::hayro_syntax::page::{Page, Pages};
use hayro::kurbo::{Affine, BezPath, Rect, Shape as _};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

use super::dark::{self, Area};
use super::links::Links;
use super::{PageRequest, Picture, Size};

/// Of a page as it is drawn, in pixels.
const MAX_SIDE: f32 = 12_000.;
const MAX_AREA: f32 = 40_000_000.;

/// What is kept of a file while its pages are drawn.
pub struct Drawing<'a> {
    pages: &'a Pages<'a>,
    cache: RenderCache<'a>,
    links: Links<'a>,
    settings: InterpreterSettings,
}

impl<'a> Drawing<'a> {
    pub fn new(pages: &'a Pages<'a>) -> Self {
        Self {
            pages,
            cache: RenderCache::new(),
            links: Links::new(pages),
            settings: InterpreterSettings::default(),
        }
    }

    pub fn sizes(&self) -> Vec<Size> {
        self.pages
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                Size { width, height }
            })
            .collect()
    }

    pub fn draw(&self, request: &PageRequest) -> Result<Picture, String> {
        let page = self
            .pages
            .get(request.page)
            .ok_or("there is no such page")?;
        let (width, height) = page.render_dimensions();
        let scale = limit(width, height, request.scale);
        let side = |points: f32| (points * scale).round().max(1.);
        // It cuts off what is left of a pixel.
        let pixmap = hayro::render(
            page,
            &self.cache,
            &self.settings,
            &RenderSettings {
                hinting: false,
                ..RenderSettings::default()
            },
            &PixmapSettings {
                x_scale: (side(width) + 0.01) / width,
                y_scale: (side(height) + 0.01) / height,
                bg_color: WHITE,
            },
        );
        let mut pixels = pixmap.data_as_u8_slice().to_vec();
        if request.dark {
            let width = usize::from(pixmap.width());
            let height = usize::from(pixmap.height());
            let pictures = self.pictures(page, scale, width, height);
            dark::darken(&mut pixels, width, &pictures);
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Ok(Picture {
            width: pixmap.width().into(),
            height: pixmap.height().into(),
            links: self.links.on(request.page),
            pixels,
        })
    }

    /// Where the pictures on a page are.
    fn pictures(&self, page: &'a Page<'a>, scale: f32, width: usize, height: usize) -> Vec<Area> {
        // Cached apart from what was drawn: fonts and the like are the same.
        let cache = hayro::hayro_interpret::InterpreterCache::new();
        #[allow(clippy::cast_precision_loss)] // pixels
        let target = Rect::new(0., 0., width as f64, height as f64);
        let mut context = Context::new(
            Affine::scale(scale.into()) * page.initial_transform(true).to_kurbo(),
            target,
            &cache,
            page.xref(),
            self.settings.clone(),
        );
        let mut finder = PictureFinder {
            clips: vec![target],
            found: Vec::new(),
        };
        interpret_page(page, &mut context, &mut finder);
        finder.found
    }
}

/// The scale to draw a page at, at most `scale`.
fn limit(width: f32, height: f32, scale: f32) -> f32 {
    let scale = if scale.is_finite() { scale } else { 1. };
    scale
        .max(0.01)
        .min(MAX_SIDE / width.max(height))
        .min((MAX_AREA / (width * height)).sqrt())
}

/// Draws nothing, and remembers where pictures would be.
struct PictureFinder {
    /// What can be seen, within each other.
    clips: Vec<Rect>,
    found: Vec<Area>,
}

impl PictureFinder {
    fn clip(&mut self, rect: Rect) {
        let within = self.clips.last().map_or(rect, |clip| clip.intersect(rect));
        self.clips.push(within);
    }
}

impl<'a> Device<'a> for PictureFinder {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}

    fn push_clip_path(&mut self, clip: &ClipPath) {
        self.clip(clip.path.bounding_box());
    }

    fn push_clip_rect(&mut self, rect: &Rect) {
        self.clip(*rect);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        // Stencils are painted in a color, like text.
        if matches!(image, Image::Stencil(_)) {
            return;
        }
        let size = Rect::new(0., 0., image.width().into(), image.height().into());
        let area = props.transform.transform_rect_bbox(size);
        let seen = self.clips.last().map_or(area, |clip| clip.intersect(area));
        if seen.width() <= 0. || seen.height() <= 0. {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // within the page
        let pixel = |value: f64| value.max(0.).round() as usize;
        self.found.push(Area {
            left: pixel(seen.x0),
            top: pixel(seen.y0),
            right: pixel(seen.x1),
            bottom: pixel(seen.y1),
        });
    }

    fn pop_clip(&mut self) {
        if self.clips.len() > 1 {
            self.clips.pop();
        }
    }

    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // computed the same way
mod tests {
    use hayro::hayro_syntax::Pdf;
    use pretty_assertions::assert_eq;

    use super::super::testing;
    use super::*;

    fn draw(bytes: Vec<u8>, request: &PageRequest) -> Result<Picture, String> {
        let pdf = Pdf::new(bytes).unwrap();
        Drawing::new(pdf.pages()).draw(request)
    }

    fn pixel(picture: &Picture, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width + x) * 4) as usize;
        picture.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn pages_are_drawn() {
        let mut request = PageRequest {
            page: 1,
            scale: 1.5,
            dark: false,
        };
        let picture = draw(testing::pdf(2, ""), &request).unwrap();
        assert_eq!((picture.width, picture.height), (150, 300));
        assert_eq!(picture.pixels.len(), 150 * 300 * 4);
        assert_eq!(pixel(&picture, 20, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&picture, 140, 20), [255; 4]);
        request.dark = true;
        let picture = draw(testing::pdf(2, ""), &request).unwrap();
        assert_eq!(pixel(&picture, 20, 20), [0xE0, 0xE9, 0xEC, 255]);
        assert_eq!(pixel(&picture, 140, 20), [0x24, 0x26, 0x26, 255]);
        request.page = 2;
        assert!(draw(testing::pdf(2, ""), &request).is_err());
    }

    #[test]
    fn sizes() {
        let pdf = Pdf::new(testing::pdf(2, "")).unwrap();
        let size = Size {
            width: 100.,
            height: 200.,
        };
        assert_eq!(Drawing::new(pdf.pages()).sizes(), [size; 2]);
        assert_eq!(limit(100., 200., 3.), 3.);
        assert!(limit(600., 800., 1000.) < 15.);
        assert_eq!(limit(100., 200., f32::NAN), 1.);
        assert!(limit(50_000., 10., 1.) * 50_000. <= MAX_SIDE);
    }

    /// Pictures are found where they are seen.
    #[test]
    fn pictures_are_found() {
        let image = "q 20 0 0 40 10 100 cm BI /W 1 /H 1 /CS /G /BPC 8 ID x EI Q";
        let clipped =
            "q 0 0 15 200 re W n 20 0 0 40 10 100 cm BI /W 1 /H 1 /CS /G /BPC 8 ID x EI Q";
        let content = format!("{image} {clipped}");
        let bytes = String::from_utf8(testing::pdf(1, "")).unwrap().replace(
            "/Length 20 >>\nstream\n0 g 0 150 50 50 re f",
            &format!("/Length {} >>\nstream\n{content}", content.len()),
        );
        let pdf = Pdf::new(bytes.into_bytes()).unwrap();
        let pages = pdf.pages();
        let found = Drawing::new(pages).pictures(&pages[0], 1., 100, 200);
        let area = |right| Area {
            left: 10,
            top: 60,
            right,
            bottom: 100,
        };
        assert_eq!(found, [area(30), area(15)]);
    }
}
