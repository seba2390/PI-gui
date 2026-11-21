//! Terminal color palettes matching the app's light and dark themes.
//!
//! The default background equals the theme background, so the terminal blends into the
//! window and only cells with an explicit background need painting.

use term::{Palette, Rgb};

#[allow(clippy::cast_possible_truncation)]
const fn h(hex: u32) -> Rgb {
    Rgb::new((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Warm dark palette for the "Pi Dark" theme.
pub const DARK: Palette = Palette {
    ansi: [
        h(0x003A_3935), // black
        h(0x00E5_675B), // red
        h(0x007F_B069), // green
        h(0x00D9_A63C), // yellow
        h(0x006A_9FD8), // blue
        h(0x00C4_6BB0), // magenta
        h(0x004F_A6A6), // cyan
        h(0x00CF_CBC2), // white
        h(0x0077_746C), // bright black
        h(0x00F0_857A), // bright red
        h(0x009C_C986), // bright green
        h(0x00EB_C064), // bright yellow
        h(0x008D_B8E8), // bright blue
        h(0x00D9_8CC8), // bright magenta
        h(0x006F_C2C2), // bright cyan
        h(0x00EC_E9E1), // bright white
    ],
    foreground: h(0x00EC_E9E1),
    background: h(0x0026_2624),
    cursor: h(0x00D9_7757),
};

/// Warm light palette for the "Pi Light" theme.
pub const LIGHT: Palette = Palette {
    ansi: [
        h(0x001F_1E1D), // black
        h(0x00C2_463A), // red
        h(0x004E_8A3A), // green
        h(0x00A8_7A16), // yellow
        h(0x003A_73B8), // blue
        h(0x00A8_4F95), // magenta
        h(0x0033_8585), // cyan
        h(0x00D6_D2C6), // white
        h(0x0075_726A), // bright black
        h(0x00D4_5A4D), // bright red
        h(0x005E_9E48), // bright green
        h(0x00BF_8D1F), // bright yellow
        h(0x004A_86CC), // bright blue
        h(0x00BD_62AA), // bright magenta
        h(0x0040_9A9A), // bright cyan
        h(0x00F0_EEE6), // bright white
    ],
    foreground: h(0x001F_1E1D),
    background: h(0x00FA_F9F5),
    cursor: h(0x00C9_6442),
};

pub fn for_mode(dark: bool) -> Palette {
    if dark { DARK } else { LIGHT }
}

#[cfg(test)]
mod tests {
    use super::{DARK, LIGHT, h};

    #[test]
    fn hex_unpacks_and_backgrounds_match_the_theme() {
        let c = h(0x0012_3456);
        assert_eq!((c.r, c.g, c.b), (0x12, 0x34, 0x56));
        assert_eq!(DARK.background, h(0x0026_2624));
        assert_eq!(LIGHT.background, h(0x00FA_F9F5));
    }
}
