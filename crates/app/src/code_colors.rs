//! Color palettes for code (Settings → Appearance → Code colors).
//!
//! The Pistachio palette is the `syntax` block of `assets/themes/pi.json`. The others are
//! well-known editor palettes, each in a light and a dark variant that follows the app's
//! light/dark mode. Only the token colors change: backgrounds stay the app's own.

use gpui_kit::component::highlighter::SyntaxColors;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeColors {
    #[default]
    Pistachio,
    One,
    Github,
    Solarized,
    Gruvbox,
    Catppuccin,
}

impl CodeColors {
    pub const ALL: [Self; 6] = [
        Self::Pistachio,
        Self::One,
        Self::Github,
        Self::Solarized,
        Self::Gruvbox,
        Self::Catppuccin,
    ];

    /// The value stored in settings.json (and used by the Settings dropdown).
    pub fn id(self) -> &'static str {
        match self {
            Self::Pistachio => "pistachio",
            Self::One => "one",
            Self::Github => "github",
            Self::Solarized => "solarized",
            Self::Gruvbox => "gruvbox",
            Self::Catppuccin => "catppuccin",
        }
    }

    #[cfg(test)]
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|colors| colors.id() == id)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pistachio => "Pistachio",
            Self::One => "One",
            Self::Github => "GitHub",
            Self::Solarized => "Solarized",
            Self::Gruvbox => "Gruvbox",
            Self::Catppuccin => "Catppuccin",
        }
    }

    /// The syntax colors for this palette, or `None` for Pistachio (the theme file's own).
    pub fn syntax(self, dark: bool) -> Option<SyntaxColors> {
        let palette = self.palette(dark)?;
        // Every key is a `SyntaxColors` field, so this cannot fail; the unit test checks it.
        serde_json::from_value(palette.to_json()).ok()
    }

    fn palette(self, dark: bool) -> Option<Palette> {
        Some(match (self, dark) {
            (Self::Pistachio, _) => return None,
            (Self::One, true) => Palette {
                keyword: "#C678DD",
                string: "#98C379",
                comment: "#7F848E",
                function: "#61AFEF",
                type_: "#E5C07B",
                number: "#D19A66",
                property: "#E06C75",
                text: "#ABB2BF",
                punctuation: "#ABB2BF",
                operator: "#56B6C2",
                tag: "#E06C75",
                attribute: "#D19A66",
                heading: "#E06C75",
                link: "#61AFEF",
            },
            (Self::One, false) => Palette {
                keyword: "#A626A4",
                string: "#50A14F",
                comment: "#A0A1A7",
                function: "#4078F2",
                type_: "#C18401",
                number: "#986801",
                property: "#E45649",
                text: "#383A42",
                punctuation: "#383A42",
                operator: "#0184BC",
                tag: "#E45649",
                attribute: "#986801",
                heading: "#E45649",
                link: "#4078F2",
            },
            (Self::Github, true) => Palette {
                keyword: "#FF7B72",
                string: "#A5D6FF",
                comment: "#8B949E",
                function: "#D2A8FF",
                type_: "#FFA657",
                number: "#79C0FF",
                property: "#79C0FF",
                text: "#E6EDF3",
                punctuation: "#C9D1D9",
                operator: "#FF7B72",
                tag: "#7EE787",
                attribute: "#79C0FF",
                heading: "#79C0FF",
                link: "#A5D6FF",
            },
            (Self::Github, false) => Palette {
                keyword: "#CF222E",
                string: "#0A3069",
                comment: "#6E7781",
                function: "#8250DF",
                type_: "#953800",
                number: "#0550AE",
                property: "#0550AE",
                text: "#24292F",
                punctuation: "#24292F",
                operator: "#CF222E",
                tag: "#116329",
                attribute: "#0550AE",
                heading: "#0550AE",
                link: "#0A3069",
            },
            (Self::Solarized, dark) => Palette {
                keyword: "#859900",
                string: "#2AA198",
                comment: if dark { "#586E75" } else { "#93A1A1" },
                function: "#268BD2",
                type_: "#B58900",
                number: "#D33682",
                property: "#268BD2",
                text: if dark { "#93A1A1" } else { "#586E75" },
                punctuation: if dark { "#839496" } else { "#657B83" },
                operator: "#859900",
                tag: "#268BD2",
                attribute: "#B58900",
                heading: "#CB4B16",
                link: "#6C71C4",
            },
            (Self::Gruvbox, true) => Palette {
                keyword: "#FB4934",
                string: "#B8BB26",
                comment: "#928374",
                function: "#8EC07C",
                type_: "#FABD2F",
                number: "#D3869B",
                property: "#83A598",
                text: "#EBDBB2",
                punctuation: "#A89984",
                operator: "#FE8019",
                tag: "#83A598",
                attribute: "#FABD2F",
                heading: "#FE8019",
                link: "#83A598",
            },
            (Self::Gruvbox, false) => Palette {
                keyword: "#9D0006",
                string: "#79740E",
                comment: "#928374",
                function: "#427B58",
                type_: "#B57614",
                number: "#8F3F71",
                property: "#076678",
                text: "#3C3836",
                punctuation: "#7C6F64",
                operator: "#AF3A03",
                tag: "#076678",
                attribute: "#B57614",
                heading: "#AF3A03",
                link: "#076678",
            },
            (Self::Catppuccin, true) => Palette {
                keyword: "#CBA6F7",
                string: "#A6E3A1",
                comment: "#9399B2",
                function: "#89B4FA",
                type_: "#F9E2AF",
                number: "#FAB387",
                property: "#B4BEFE",
                text: "#CDD6F4",
                punctuation: "#9399B2",
                operator: "#89DCEB",
                tag: "#89B4FA",
                attribute: "#F9E2AF",
                heading: "#F38BA8",
                link: "#89B4FA",
            },
            (Self::Catppuccin, false) => Palette {
                keyword: "#8839EF",
                string: "#40A02B",
                comment: "#7C7F93",
                function: "#1E66F5",
                type_: "#DF8E1D",
                number: "#FE640B",
                property: "#7287FD",
                text: "#4C4F69",
                punctuation: "#7C7F93",
                operator: "#04A5E5",
                tag: "#1E66F5",
                attribute: "#DF8E1D",
                heading: "#D20F39",
                link: "#1E66F5",
            },
        })
    }
}

/// The colors a palette defines; [`Palette::to_json`] spreads them over every highlight name
/// the compiled-in grammars use (see `src/syntax.rs`).
struct Palette {
    keyword: &'static str,
    string: &'static str,
    comment: &'static str,
    function: &'static str,
    type_: &'static str,
    /// Numbers and constants (`true`, `null`, `None`…).
    number: &'static str,
    /// Object keys and fields (JSON, YAML, TOML keys; struct fields).
    property: &'static str,
    /// Plain identifiers.
    text: &'static str,
    punctuation: &'static str,
    operator: &'static str,
    /// HTML/JSX tags.
    tag: &'static str,
    attribute: &'static str,
    /// Markdown headings and list markers.
    heading: &'static str,
    link: &'static str,
}

impl Palette {
    fn to_json(&self) -> serde_json::Value {
        let color = |hex: &str| serde_json::json!({ "color": hex });
        serde_json::json!({
            "keyword": color(self.keyword),
            "string": color(self.string),
            "string.escape": color(self.operator),
            "escape": color(self.operator),
            "comment": { "color": self.comment, "font_style": "italic" },
            "function": color(self.function),
            "constructor": color(self.type_),
            "type": color(self.type_),
            "namespace": color(self.type_),
            "number": color(self.number),
            "constant": color(self.number),
            "boolean": color(self.number),
            "label": color(self.number),
            "property": color(self.property),
            "variable": color(self.text),
            "punctuation": color(self.punctuation),
            "delimiter": color(self.punctuation),
            "operator": color(self.operator),
            "tag": color(self.tag),
            "attribute": color(self.attribute),
            "title": { "color": self.heading, "font_weight": 700 },
            "punctuation.list_marker": color(self.heading),
            "emphasis": { "font_style": "italic" },
            "emphasis.strong": { "font_weight": 700 },
            "text.literal": color(self.string),
            "text.code.span": color(self.string),
            "link_text": color(self.link),
            "text.reference": color(self.link),
            "link_uri": color(self.comment),
            "text.uri": color(self.comment),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CodeColors;

    #[test]
    fn every_palette_parses_in_both_modes() {
        for colors in CodeColors::ALL {
            for dark in [false, true] {
                let syntax = colors.syntax(dark);
                assert_eq!(
                    syntax.is_some(),
                    colors != CodeColors::Pistachio,
                    "{colors:?} dark={dark}"
                );
                if let Some(syntax) = syntax {
                    // A palette color for each kind of token the grammars emit.
                    for name in ["keyword", "string", "comment", "property", "tag", "title"] {
                        assert!(syntax.style(name).is_some(), "{colors:?} {name}");
                    }
                }
            }
        }
    }

    #[test]
    fn ids_round_trip() {
        for colors in CodeColors::ALL {
            assert_eq!(CodeColors::from_id(colors.id()), colors);
        }
        assert_eq!(CodeColors::from_id("unknown"), CodeColors::Pistachio);
    }
}
