//! A blurred picture of the window, drawn behind a card that sits over everything (the update
//! prompt). GPUI cannot blur what is behind an element, so the window's last frame is
//! captured (macOS: see `vendor/gpui-pre-macos`, `request_frame_capture`), shrunk, blurred
//! and drawn scaled up: the GPU's linear filtering smooths it further.

use std::sync::Arc;

use gpui_kit::RenderImage;

/// About this many pixels across after shrinking: plenty for something this blurred.
const TARGET_WIDTH: u32 = 320;
/// Box blur radius in shrunk pixels; three passes approximate a Gaussian.
const RADIUS: usize = 5;
const PASSES: usize = 3;

/// Blurs `frame` (RGBA) and blends it `amount` (0–1) toward `tint` (RGB), so a card on top
/// stays easy to read.
pub fn blur(frame: &image::RgbaImage, tint: [f32; 3], amount: f32) -> Arc<RenderImage> {
    let (width, height) = frame.dimensions();
    let factor = (width / TARGET_WIDTH).max(1);
    let (w, h) = ((width / factor).max(1), (height / factor).max(1));
    let (w_us, h_us) = (w as usize, h as usize);

    // Shrink by averaging each `factor`×`factor` block.
    let mut pixels = vec![[0_f32; 3]; w_us * h_us];
    let raw = frame.as_raw();
    let stride = width as usize * 4;
    #[allow(clippy::cast_precision_loss)] // small counts
    let block = (factor * factor) as f32;
    for y in 0..h_us {
        for x in 0..w_us {
            let mut sum = [0_f32; 3];
            for dy in 0..factor as usize {
                let row = (y * factor as usize + dy) * stride;
                for dx in 0..factor as usize {
                    let at = row + (x * factor as usize + dx) * 4;
                    for (channel, value) in sum.iter_mut().zip(&raw[at..at + 3]) {
                        *channel += f32::from(*value);
                    }
                }
            }
            pixels[y * w_us + x] = sum.map(|channel| channel / block);
        }
    }

    for _ in 0..PASSES {
        box_blur(&mut pixels, w_us, h_us, RADIUS, true);
        box_blur(&mut pixels, w_us, h_us, RADIUS, false);
    }

    // GPUI's images are BGRA.
    let amount = amount.clamp(0., 1.);
    let mut out = Vec::with_capacity(w_us * h_us * 4);
    for pixel in &pixels {
        let mixed: [f32; 3] =
            std::array::from_fn(|i| pixel[i] * (1. - amount) + tint[i] * 255. * amount);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255
        let byte = |value: f32| value.round().clamp(0., 255.) as u8;
        out.extend_from_slice(&[byte(mixed[2]), byte(mixed[1]), byte(mixed[0]), 255]);
    }
    let buffer = image::RgbaImage::from_raw(w, h, out).expect("sized to w × h");
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
        buffer
    )]))
}

/// One pass of a box blur along rows (`horizontal`) or columns, with the edges extended.
#[allow(clippy::cast_possible_wrap)] // a few hundred pixels, far below isize::MAX
fn box_blur(pixels: &mut [[f32; 3]], w: usize, h: usize, radius: usize, horizontal: bool) {
    let (lines, length) = if horizontal { (h, w) } else { (w, h) };
    let index = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    #[allow(clippy::cast_precision_loss)] // small counts
    let span = (2 * radius + 1) as f32;
    let mut line_buffer = vec![[0_f32; 3]; length];
    for line in 0..lines {
        for (i, slot) in line_buffer.iter_mut().enumerate() {
            *slot = pixels[index(line, i)];
        }
        let at = |i: isize| {
            let clamped = i.clamp(0, length as isize - 1);
            #[allow(clippy::cast_sign_loss)] // clamped to 0..length
            line_buffer[clamped as usize]
        };
        let r = radius as isize;
        let mut sum = [0_f32; 3];
        for i in -r..=r {
            let value = at(i);
            for c in 0..3 {
                sum[c] += value[c];
            }
        }
        for i in 0..length {
            pixels[index(line, i)] = sum.map(|channel| channel / span);
            let (add, remove) = (at(i as isize + r + 1), at(i as isize - r));
            for c in 0..3 {
                sum[c] += add[c] - remove[c];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blurs_a_sharp_edge_and_keeps_flat_color() {
        // Left half black, right half white, 640×40: shrinks by 2 to 320×20.
        let frame = image::RgbaImage::from_fn(640, 40, |x, _| {
            if x < 320 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let mut pixels = vec![[0_f32; 3]; 320 * 20];
        for (i, pixel) in pixels.iter_mut().enumerate() {
            *pixel = if i % 320 < 160 { [0.; 3] } else { [255.; 3] };
        }
        box_blur(&mut pixels, 320, 20, RADIUS, true);
        // Far from the edge the color is unchanged; at the edge it is in between.
        assert_eq!(pixels[10 * 320 + 20], [0.; 3]);
        assert_eq!(pixels[10 * 320 + 300], [255.; 3]);
        let edge = pixels[10 * 320 + 160][0];
        assert!(edge > 50. && edge < 205., "{edge}");

        let image = blur(&frame, [0., 0., 0.], 0.);
        let data = image.as_bytes(0).unwrap();
        assert_eq!(data.len(), 320 * 20 * 4);
        // BGRA, opaque; black on the left, white on the right.
        assert_eq!(&data[..4], &[0, 0, 0, 255]);
        assert_eq!(&data[data.len() - 4..], &[255, 255, 255, 255]);
    }
}
