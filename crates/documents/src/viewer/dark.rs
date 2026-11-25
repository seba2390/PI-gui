//! Pages with light text on a dark ground. Colors keep their hue, and photos stay as they are.

/// What white and black become, as red, green and blue.
const GROUND: [i32; 3] = [0x26, 0x26, 0x24];
const INK: [i32; 3] = [0xEC, 0xE9, 0xE0];
/// Of the pixels of a picture that is not a photo (a diagram, a scan), this part is white.
const WHITE_PART: f64 = 0.4;
/// Pages that are darker than this on average are left alone.
const DARK_LUMA: u64 = 80;

/// A part of a page, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

/// Darkens a page of opaque RGBA pixels, whose pictures are at `pictures`.
pub fn darken(pixels: &mut [u8], width: usize, pictures: &[Area]) {
    if width == 0 || is_dark(pixels) {
        return;
    }
    let photos: Vec<Area> = pictures
        .iter()
        .copied()
        .filter(|area| is_photo(pixels, width, *area))
        .collect();
    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        let spans: Vec<(usize, usize)> = photos
            .iter()
            .filter(|area| (area.top..area.bottom).contains(&y))
            .map(|area| (area.left, area.right))
            .collect();
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if !spans
                .iter()
                .any(|(left, right)| (*left..*right).contains(&x))
            {
                let dark = color([pixel[0], pixel[1], pixel[2]]);
                pixel[..3].copy_from_slice(&dark);
            }
        }
    }
}

fn luma([red, green, blue]: [i32; 3]) -> i32 {
    (54 * red + 183 * green + 19 * blue) / 256
}

/// `rgb` with its lightness turned round and its hue kept.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255
fn color(rgb: [u8; 3]) -> [u8; 3] {
    let inverted = rgb.map(|channel| 255 - i32::from(channel));
    let twice = 2 * luma(inverted);
    let mut out = [0; 3];
    for channel in 0..3 {
        let turned = (twice - inverted[channel]).clamp(0, 255);
        let (ground, ink) = (GROUND[channel], INK[channel]);
        out[channel] = (ground + turned * (ink - ground) / 255) as u8;
    }
    out
}

fn is_dark(pixels: &[u8]) -> bool {
    // Every pixel is more than it takes.
    let (mut sum, mut count) = (0_u64, 0_u64);
    for pixel in pixels.as_chunks::<4>().0.iter().step_by(7) {
        sum += u64::from(luma([pixel[0], pixel[1], pixel[2]].map(i32::from)).unsigned_abs());
        count += 1;
    }
    sum < DARK_LUMA * count
}

#[allow(clippy::cast_precision_loss)] // counts of pixels
fn is_photo(pixels: &[u8], width: usize, area: Area) -> bool {
    let (mut white, mut count) = (0_u64, 0_u64);
    for y in area.top..area.bottom {
        for x in area.left..area.right.min(width) {
            let at = (y * width + x) * 4;
            let Some(pixel) = pixels.get(at..at + 3) else {
                continue;
            };
            white += u64::from(pixel.iter().all(|channel| *channel >= 235));
            count += 1;
        }
    }
    count > 0 && (white as f64) < count as f64 * WHITE_PART
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn colors_keep_their_hue() {
        assert_eq!(color([255; 3]), [0x26, 0x26, 0x24]);
        assert_eq!(color([0; 3]), [0xEC, 0xE9, 0xE0]);
        let [red, green, blue] = color([0, 0, 255]);
        assert!(blue > red && blue > green && red > 150, "a light blue");
        let [red, green, blue] = color([200, 0, 0]);
        assert!(red > green && red > blue && green > 100, "a light red");
    }

    /// A page 10 pixels wide and high: white, but the top left quarter.
    fn page(quarter: u8) -> Vec<u8> {
        let mut pixels = vec![255; 400];
        for y in 0..5 {
            for x in 0..5 {
                pixels[(y * 10 + x) * 4..][..3].copy_from_slice(&[quarter; 3]);
            }
        }
        pixels
    }

    const QUARTER: Area = Area {
        left: 0,
        top: 0,
        right: 5,
        bottom: 5,
    };

    #[test]
    fn photos_stay_as_they_are() {
        let mut pixels = page(120);
        darken(&mut pixels, 10, &[QUARTER]);
        assert_eq!(pixels[..4], [120, 120, 120, 255]);
        assert_eq!(pixels[5 * 4..][..4], [0x26, 0x26, 0x24, 255]);
        // But not pictures that are white, like scans.
        let mut pixels = page(250);
        darken(&mut pixels, 10, &[QUARTER]);
        assert!(pixels[0] < 0x30);
        // Nor what is no picture.
        let mut pixels = page(0);
        darken(&mut pixels, 10, &[]);
        assert_eq!(pixels[..4], [0xEC, 0xE9, 0xE0, 255]);
    }

    #[test]
    fn dark_pages_stay_dark() {
        let dark = [20, 30, 40, 255].repeat(100);
        let mut pixels = dark.clone();
        darken(&mut pixels, 10, &[]);
        assert_eq!(pixels, dark);
        darken(&mut [], 0, &[]);
    }
}
