//! Color types and resolution of terminal colors (named / indexed / truecolor) to plain RGB.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{self, Color, NamedColor};

/// Brightness factor applied to dim (SGR 2) colors, same as alacritty.
const DIM_FACTOR: f32 = 0.66;

/// A plain 24-bit color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Build a color from `0xRRGGBB`.
    pub const fn from_hex(hex: u32) -> Self {
        let [_, r, g, b] = hex.to_be_bytes();
        Self { r, g, b }
    }

    /// The color as `0xRRGGBB`.
    pub const fn to_hex(self) -> u32 {
        u32::from_be_bytes([0, self.r, self.g, self.b])
    }

    #[must_use]
    fn scale(self, factor: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let f = |c: u8| (f32::from(c) * factor).round().clamp(0.0, 255.0) as u8;
        Self::new(f(self.r), f(self.g), f(self.b))
    }
}

impl From<ansi::Rgb> for Rgb {
    fn from(c: ansi::Rgb) -> Self {
        Self::new(c.r, c.g, c.b)
    }
}

impl From<Rgb> for ansi::Rgb {
    fn from(c: Rgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

/// Theme colors used to resolve every terminal color.
///
/// The 256-color cube (16..232) and grayscale ramp (232..256) use the standard xterm values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their bright variants.
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
}

impl Default for Palette {
    /// A muted dark palette (One Dark inspired).
    fn default() -> Self {
        let h = Rgb::from_hex;
        Self {
            ansi: [
                h(0x003f_4451), // black
                h(0x00e0_6c75), // red
                h(0x0098_c379), // green
                h(0x00e5_c07b), // yellow
                h(0x0061_afef), // blue
                h(0x00c6_78dd), // magenta
                h(0x0056_b6c2), // cyan
                h(0x00ab_b2bf), // white
                h(0x005c_6370), // bright black
                h(0x00f0_8a93), // bright red
                h(0x00b5_dc93), // bright green
                h(0x00f2_d39a), // bright yellow
                h(0x008c_c8ff), // bright blue
                h(0x00dd_a5ee), // bright magenta
                h(0x007f_d3de), // bright cyan
                h(0x00e6_e9ef), // bright white
            ],
            foreground: h(0x00ab_b2bf),
            background: h(0x001e_2127),
            cursor: h(0x0052_8bff),
        }
    }
}

impl Palette {
    /// Default color for an index into alacritty's 269 entry color table
    /// (0..256 indexed colors, then foreground, background, cursor, dim colors,
    /// bright foreground, dim foreground).
    pub fn indexed(&self, index: usize) -> Rgb {
        const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let i = index - 16;
                Rgb::new(CUBE[i / 36], CUBE[(i / 6) % 6], CUBE[i % 6])
            }
            232..=255 => {
                // index < 256, so the value is at most 8 + 10 * 23 = 238.
                let v = u8::try_from(8 + (index - 232) * 10).unwrap_or(u8::MAX);
                Rgb::new(v, v, v)
            }
            257 => self.background,
            258 => self.cursor,
            259..=266 => self.ansi[index - 259].scale(DIM_FACTOR),
            268 => self.foreground.scale(DIM_FACTOR),
            // 256 (foreground), 267 (bright foreground) and anything out of range.
            _ => self.foreground,
        }
    }
}

/// Resolves cell colors against the palette plus any colors the program changed at runtime
/// (OSC 4/10/11/12).
pub(crate) struct ColorResolver<'a> {
    pub palette: &'a Palette,
    pub overrides: &'a Colors,
    pub bold_is_bright: bool,
}

impl ColorResolver<'_> {
    fn lookup(&self, index: usize) -> Rgb {
        self.overrides[index].map_or_else(|| self.palette.indexed(index), Rgb::from)
    }

    pub fn named(&self, color: NamedColor) -> Rgb {
        self.lookup(color as usize)
    }

    /// Foreground color, taking bold-as-bright and dim into account.
    pub fn fg(&self, color: Color, flags: Flags) -> Rgb {
        let bold = flags.contains(Flags::BOLD);
        let bright = self.bold_is_bright && bold;
        let dim = flags.contains(Flags::DIM) && !bright;
        match color {
            Color::Spec(rgb) if dim => Rgb::from(rgb).scale(DIM_FACTOR),
            Color::Spec(rgb) => rgb.into(),
            Color::Named(named) if bright => self.named(named.to_bright()),
            Color::Named(named) if dim => self.named(named.to_dim()),
            Color::Named(named) => self.named(named),
            Color::Indexed(idx) => {
                let idx = usize::from(idx);
                match idx {
                    0..8 if bright => self.lookup(idx + 8),
                    0..8 if dim => self.lookup(NamedColor::DimBlack as usize + idx),
                    8..16 if dim => self.lookup(NamedColor::DimBlack as usize + idx - 8),
                    _ if dim => self.lookup(idx).scale(DIM_FACTOR),
                    _ => self.lookup(idx),
                }
            }
        }
    }

    /// Background color (never dimmed or brightened).
    pub fn bg(&self, color: Color) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb.into(),
            Color::Named(named) => self.named(named),
            Color::Indexed(idx) => self.lookup(usize::from(idx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn resolver<'a>(palette: &'a Palette, overrides: &'a Colors) -> ColorResolver<'a> {
        ColorResolver {
            palette,
            overrides,
            bold_is_bright: false,
        }
    }

    #[test]
    fn hex_round_trip() {
        let c = Rgb::from_hex(0x0012_3456);
        assert_eq!(c, Rgb::new(0x12, 0x34, 0x56));
        assert_eq!(c.to_hex(), 0x0012_3456);
    }

    #[test]
    fn xterm_cube_and_grayscale() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb::new(0, 0, 0));
        assert_eq!(p.indexed(196), Rgb::new(255, 0, 0));
        assert_eq!(p.indexed(21), Rgb::new(0, 0, 255));
        assert_eq!(p.indexed(231), Rgb::new(255, 255, 255));
        assert_eq!(p.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(p.indexed(255), Rgb::new(238, 238, 238));
        assert_eq!(p.indexed(1), p.ansi[1]);
        assert_eq!(p.indexed(256), p.foreground);
        assert_eq!(p.indexed(257), p.background);
    }

    #[test]
    fn bold_as_bright_is_optional() {
        let p = Palette::default();
        let o = Colors::default();
        let red = Color::Named(NamedColor::Red);
        assert_eq!(resolver(&p, &o).fg(red, Flags::BOLD), p.ansi[1]);
        let bright = ColorResolver {
            bold_is_bright: true,
            ..resolver(&p, &o)
        };
        assert_eq!(bright.fg(red, Flags::BOLD), p.ansi[9]);
        assert_eq!(bright.fg(Color::Indexed(2), Flags::BOLD), p.ansi[10]);
    }

    #[test]
    fn dim_scales_color() {
        let p = Palette::default();
        let o = Colors::default();
        let r = resolver(&p, &o);
        let dimmed = r.fg(
            Color::Spec(ansi::Rgb {
                r: 200,
                g: 100,
                b: 0,
            }),
            Flags::DIM,
        );
        assert_eq!(dimmed, Rgb::new(132, 66, 0));
        assert_eq!(
            r.fg(Color::Indexed(1), Flags::DIM),
            p.ansi[1].scale(DIM_FACTOR)
        );
        assert_eq!(
            r.fg(Color::Indexed(9), Flags::DIM),
            p.ansi[1].scale(DIM_FACTOR)
        );
    }

    #[test]
    fn runtime_overrides_win() {
        let p = Palette::default();
        let mut o = Colors::default();
        o[NamedColor::Background] = Some(ansi::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            resolver(&p, &o).bg(Color::Named(NamedColor::Background)),
            Rgb::new(1, 2, 3)
        );
    }
}
