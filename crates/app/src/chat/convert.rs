//! Pictures that models do not take, as PNG.

use std::io::Cursor;
use std::path::Path;

use gpui_kit::SvgRenderer;
use image::{DynamicImage, ImageFormat, ImageReader, RgbaImage};

/// The longer side of a drawing, which has no size of its own, in pixels.
const DRAWING: f32 = 1536.;
/// Larger files are not read.
const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The picture at `path`, which ends with `extension`, if it is one that is converted. Blocks.
pub fn to_png(path: &Path, extension: &str, svg: &SvgRenderer) -> Option<Vec<u8>> {
    let format = match extension {
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "svg" => None,
        _ => return None,
    };
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let picture = match format {
        // With limits on what it takes in memory.
        Some(format) => ImageReader::with_format(Cursor::new(bytes), format)
            .decode()
            .ok()?,
        None => drawing(&bytes, svg)?,
    };
    let mut png = Cursor::new(Vec::new());
    let picture = DynamicImage::ImageRgba8(picture.into_rgba8());
    picture.write_to(&mut png, ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

/// A drawing on white: what is not drawn on could be taken for black, like what is drawn.
fn drawing(bytes: &[u8], svg: &SvgRenderer) -> Option<DynamicImage> {
    let parsed = svg.parse_svg(bytes).ok()?;
    let small = svg.render_parsed(&parsed, 0.1).ok()?;
    let size = small.size(0);
    #[allow(clippy::cast_precision_loss)] // pixels
    let longer = i32::from(size.width).max(i32::from(size.height)).max(1) as f32;
    let large = svg.render_parsed(&parsed, 0.1 * DRAWING / longer).ok()?;
    let size = large.size(0);
    let mut pixels = large.as_bytes(0)?.to_vec();
    for [blue, green, red, alpha] in pixels.as_chunks_mut::<4>().0 {
        // Blue comes first.
        std::mem::swap(blue, red);
        for channel in [blue, green, red] {
            let (value, part) = (u32::from(*channel), u32::from(*alpha));
            let on_white = (value * part + 255 * (255 - part)) / 255;
            *channel = u8::try_from(on_white).unwrap_or(u8::MAX);
        }
        *alpha = 255;
    }
    let width = u32::try_from(i32::from(size.width)).ok()?;
    let height = u32::try_from(i32::from(size.height)).ok()?;
    RgbaImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgba8)
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use image::{ImageFormat, Rgba, RgbaImage};

    #[gpui_kit::test]
    fn pictures_become_png(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let svg = cx.update(|cx| cx.svg_renderer());
        let read = |name: &str, extension| {
            let png = super::to_png(&dir.path().join(name), extension, &svg)?;
            Some(
                image::load_from_memory_with_format(&png, ImageFormat::Png)
                    .unwrap()
                    .into_rgba8(),
            )
        };
        let picture = RgbaImage::from_pixel(3, 2, Rgba([10, 20, 30, 255]));
        for (name, extension, format) in [
            ("a.bmp", "bmp", ImageFormat::Bmp),
            ("a.tif", "tif", ImageFormat::Tiff),
        ] {
            picture
                .save_with_format(dir.path().join(name), format)
                .unwrap();
            assert_eq!(read(name, extension).as_ref(), Some(&picture));
        }
        // Drawn large enough to see, on white.
        let drawing = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"12\">\
                       <rect width=\"12\" height=\"12\" fill=\"#f00\"/></svg>";
        std::fs::write(dir.path().join("a.svg"), drawing).unwrap();
        let png = read("a.svg", "svg").unwrap();
        assert!(
            png.width() > 1000 && png.width().abs_diff(2 * png.height()) <= 2,
            "{:?}",
            png.dimensions()
        );
        assert_eq!(png.get_pixel(50, 50), &Rgba([255, 0, 0, 255]));
        assert_eq!(png.get_pixel(png.width() - 50, 50), &Rgba([255; 4]));

        std::fs::write(dir.path().join("b.svg"), "no").unwrap();
        assert_eq!(read("b.svg", "svg"), None);
        std::fs::write(dir.path().join("b.bmp"), "no").unwrap();
        assert_eq!(read("b.bmp", "bmp"), None);
        assert_eq!(read("b.bmp", "png"), None);
    }
}
