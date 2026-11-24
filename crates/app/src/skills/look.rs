//! How a skill looks in the list: an icon for what its name is about, and a color that stays
//! the same for that name.

use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::*;

/// The icon for each word a name may contain.
const ICONS: &[(&str, &[&str])] = &[
    (
        "icons/rocket.svg",
        &["release", "releases", "deploy", "ship", "publish", "launch"],
    ),
    (
        "icons/database.svg",
        &[
            "sql",
            "db",
            "database",
            "databases",
            "postgres",
            "sqlite",
            "query",
            "queries",
            "migration",
            "migrations",
        ],
    ),
    (
        "icons/git-branch.svg",
        &[
            "git",
            "branch",
            "commit",
            "commits",
            "pr",
            "merge",
            "rebase",
            "changelog",
        ],
    ),
    (
        "icons/flask-conical.svg",
        &[
            "test",
            "tests",
            "testing",
            "qa",
            "experiment",
            "experiments",
        ],
    ),
    (
        "icons/bug.svg",
        &["debug", "debugging", "bug", "bugs", "fix", "triage"],
    ),
    (
        "icons/shield-check.svg",
        &[
            "security", "audit", "review", "reviews", "lint", "check", "checks",
        ],
    ),
    (
        "icons/book-open.svg",
        &[
            "docs",
            "doc",
            "documentation",
            "guide",
            "guides",
            "readme",
            "manual",
            "onboarding",
        ],
    ),
    (
        "icons/chart-column.svg",
        &[
            "chart",
            "charts",
            "plot",
            "plots",
            "data",
            "analysis",
            "metrics",
            "benchmark",
            "benchmarks",
            "perf",
        ],
    ),
    (
        "icons/palette.svg",
        &["design", "ui", "css", "theme", "frontend", "style", "brand"],
    ),
    (
        "icons/server.svg",
        &[
            "server", "servers", "ssh", "docker", "infra", "ops", "cluster", "slurm", "gpu",
            "cloud",
        ],
    ),
    (
        "icons/globe.svg",
        &["web", "http", "api", "browser", "site", "search"],
    ),
    (
        "icons/image.svg",
        &[
            "image",
            "images",
            "picture",
            "pictures",
            "screenshot",
            "video",
        ],
    ),
    (
        "icons/message-square.svg",
        &[
            "chat", "slack", "email", "emails", "mail", "message", "messages",
        ],
    ),
    (
        "icons/file-text.svg",
        &[
            "pdf", "docx", "report", "reports", "notes", "write", "writing", "paper", "letter",
        ],
    ),
    (
        "icons/code-xml.svg",
        &[
            "code",
            "refactor",
            "rust",
            "python",
            "typescript",
            "javascript",
            "go",
            "cpp",
        ],
    ),
    (
        "icons/wrench.svg",
        &["build", "tool", "tools", "setup", "install", "config", "ci"],
    ),
];

const OTHER: &str = "icons/puzzle.svg";
const BUILT_IN: &str = "icons/pi-logo.svg";

/// A skill has one of these, by its name.
const COLORS: [u32; 8] = [
    0x006F_B7DD,
    0x00C9_9AE0,
    0x00E0_B56F,
    0x00E5_8F86,
    0x007F_CFC0,
    0x00A9_B8F0,
    0x008F_BF6A,
    0x00D9_8AB5,
];

/// The first word of the name that has an icon decides.
fn icon(name: &str) -> &'static str {
    name.split('-')
        .find_map(|word| {
            ICONS
                .iter()
                .find_map(|(icon, words)| words.contains(&word).then_some(*icon))
        })
        .unwrap_or(OTHER)
}

fn own_color(name: &str) -> Hsla {
    // FNV-1a: the same in every build, so a skill keeps its color.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    rgb(COLORS[hash as usize % COLORS.len()]).into()
}

/// `color` made light or dark enough to be seen on a dark or a light background.
fn visible(color: Hsla, dark: bool) -> Hsla {
    let l = if dark {
        color.l.max(0.6)
    } else {
        color.l.min(0.4)
    };
    Hsla { l, ..color }
}

/// The square icon of a skill. The ones that come with Pistachio have its π.
pub fn tile(name: &str, builtin: bool, cx: &App) -> Div {
    let (path, color) = if builtin {
        (BUILT_IN, crate::theme::palette(cx).accent_icon)
    } else {
        let dark = cx.theme().mode.is_dark();
        (icon(name), visible(own_color(name), dark))
    };
    div()
        .flex()
        .flex_none()
        .size(px(30.))
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(color.opacity(0.16))
        .child(Icon::default().path(path).small().text_color(color))
}

#[cfg(test)]
mod tests {
    use super::{BUILT_IN, COLORS, ICONS, OTHER, icon, own_color, visible};
    use gpui_kit::{Hsla, black, rgb, white};

    #[test]
    fn names_decide_the_icon() {
        assert_eq!(icon("release-notes"), "icons/rocket.svg");
        assert_eq!(icon("sql-review"), "icons/database.svg");
        assert_eq!(icon("repo-guide"), "icons/book-open.svg");
        assert_eq!(icon("my-thing"), OTHER);
        assert_eq!(icon("latest"), OTHER, "whole words only");
    }

    #[test]
    fn every_icon_is_bundled_and_every_word_has_one() {
        let mut words: Vec<&str> = Vec::new();
        for (icon, its_words) in ICONS {
            assert!(crate::assets::AppAssets::get(icon).is_some(), "{icon}");
            words.extend(*its_words);
        }
        for icon in [OTHER, BUILT_IN] {
            assert!(crate::assets::AppAssets::get(icon).is_some(), "{icon}");
        }
        let count = words.len();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), count, "a word is listed twice");
    }

    #[test]
    fn a_name_keeps_its_color() {
        assert_eq!(own_color("release-notes"), own_color("release-notes"));
        let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
        let colors: std::collections::HashSet<String> = names
            .map(|name| format!("{:?}", own_color(name)))
            .into_iter()
            .collect();
        assert!(colors.len() > 3, "names share too few colors");
    }

    #[test]
    fn any_color_can_be_seen() {
        let colors = COLORS.map(|color| Hsla::from(rgb(color)));
        for color in colors.into_iter().chain([black(), white()]) {
            assert!(visible(color, true).l >= 0.6);
            assert!(visible(color, false).l <= 0.4);
            assert!((visible(color, true).h - color.h).abs() < f32::EPSILON);
        }
    }
}
