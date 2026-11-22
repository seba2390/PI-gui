//! Syntax colors for code shown line by line: the agent's edits and the files it read in the
//! chat, and the Changes panel.
//!
//! Tree-sitter needs whole, valid-ish source to parse, so code is never highlighted one line
//! at a time. A file's text is highlighted once and the colors are split back onto its lines
//! ([`highlight_lines`]). A diff is not code either: its old and new sides are rebuilt from
//! the context, removed and added lines ([`DiffSides`]), each side is highlighted once, and
//! every diff line takes its colors from the side it belongs to ([`highlight_diff`]).
//!
//! Only foreground styles are kept (no backgrounds), so the colors draw on top of the red and
//! green added/removed backgrounds. Colors come from the theme's highlight theme, which holds
//! the user's Code colors choice. Results are cached by content ([`HighlightCache`]) so
//! scrolling and streaming never parse the same text twice; a theme change clears the cache.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::HighlightStyle;
use gpui_kit::base::input::HighlightStyleResolver;
use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_kit::component::input::Rope;

/// Styles of one line, as byte ranges within that line.
pub type LineStyles = Vec<(Range<usize>, HighlightStyle)>;

/// Files longer than this are shown without colors: parsing them is not worth the time for
/// text that is capped on screen anyway.
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;

/// The highlighter language for a file, from its extension (or name), if it is one of the
/// grammars compiled in (see the workspace `Cargo.toml`).
pub fn language_for_path(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    // Extensionless names that are still code.
    match name.as_str() {
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return Some("bash"),
        "cargo.lock" | "pipfile" => return Some("toml"),
        _ => {}
    }
    let extension = name.rsplit_once('.')?.1;
    Some(match extension {
        "json" | "jsonc" | "json5" => "json",
        "md" | "markdown" | "mdx" => "markdown",
        "py" | "pyi" | "pyw" => "python",
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "ipp" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "java" => "java",
        "html" | "htm" | "xhtml" => "html",
        "css" | "scss" => "css",
        "sh" | "bash" | "zsh" => "bash",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "diff" | "patch" => "diff",
        _ => return None,
    })
}

/// Highlights `text` as `language` once and returns the styles of each line (split on `\n`;
/// a trailing newline adds no line).
pub fn highlight_lines(
    language: &str,
    text: &str,
    resolver: &dyn HighlightStyleResolver,
) -> Vec<LineStyles> {
    let styles = highlight(language, text, resolver);
    split_lines(text, &styles)
}

/// Highlights a whole text, keeping only foreground styles.
fn highlight(
    language: &str,
    text: &str,
    resolver: &dyn HighlightStyleResolver,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if text.is_empty() || text.len() > MAX_HIGHLIGHT_BYTES {
        return Vec::new();
    }
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from(text), None);
    highlighter
        .styles(&(0..text.len()), resolver)
        .into_iter()
        .filter_map(|(range, style)| {
            let style = HighlightStyle {
                background_color: None,
                ..style
            };
            (style != HighlightStyle::default()).then_some((range, style))
        })
        .collect()
}

/// Splits whole-text `styles` (sorted, non-overlapping byte ranges) into per-line styles
/// with ranges relative to each line. A style spanning lines (a block comment) is cut at
/// each newline.
pub fn split_lines<T: Clone>(
    text: &str,
    styles: &[(Range<usize>, T)],
) -> Vec<Vec<(Range<usize>, T)>> {
    let mut lines = Vec::new();
    let mut next = 0;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content_end = start + line.trim_end_matches('\n').len();
        let mut out = Vec::new();
        // Styles are sorted, so skip the ones that ended before this line.
        while next < styles.len() && styles[next].0.end <= start {
            next += 1;
        }
        let mut ix = next;
        while ix < styles.len() && styles[ix].0.start < content_end {
            let (range, style) = &styles[ix];
            let from = range.start.max(start);
            let to = range.end.min(content_end);
            if from < to {
                out.push((from - start..to - start, style.clone()));
            }
            ix += 1;
        }
        lines.push(out);
        start += line.len();
    }
    lines
}

/// Which side of a diff a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Only in the old file (removed).
    Old,
    /// Only in the new file (added).
    New,
    /// In both (context).
    Both,
    /// Not code: a hunk header, a gap or a note.
    Neither,
}

/// The old and new text rebuilt from a diff's lines, and where each diff line landed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DiffSides {
    pub old: String,
    pub new: String,
    /// Per diff line: the side whose colors it takes (`true` for new) and its line index
    /// there; `None` for lines that are not code.
    pub map: Vec<Option<(bool, usize)>>,
}

impl DiffSides {
    /// Rebuilds both sides. Hunks are joined back to back: the lines between them are
    /// missing, but tree-sitter recovers, and that is far closer to the real file than
    /// highlighting lines on their own.
    pub fn new<'a>(lines: impl IntoIterator<Item = (Side, &'a str)>) -> Self {
        let mut sides = Self::default();
        let (mut old_lines, mut new_lines) = (0, 0);
        for (side, text) in lines {
            let push = |buffer: &mut String, count: &mut usize| {
                buffer.push_str(text);
                buffer.push('\n');
                *count += 1;
                *count - 1
            };
            let mapped = match side {
                Side::Old => Some((false, push(&mut sides.old, &mut old_lines))),
                Side::New => Some((true, push(&mut sides.new, &mut new_lines))),
                Side::Both => {
                    push(&mut sides.old, &mut old_lines);
                    Some((true, push(&mut sides.new, &mut new_lines)))
                }
                Side::Neither => None,
            };
            sides.map.push(mapped);
        }
        sides
    }
}

/// The styles of each diff line, highlighting the old and new sides once each.
pub fn highlight_diff<'a>(
    language: &str,
    lines: impl IntoIterator<Item = (Side, &'a str)>,
    resolver: &dyn HighlightStyleResolver,
) -> Vec<LineStyles> {
    let sides = DiffSides::new(lines);
    let old = highlight_lines(language, &sides.old, resolver);
    let new = highlight_lines(language, &sides.new, resolver);
    sides
        .map
        .iter()
        .map(|mapped| match mapped {
            Some((true, ix)) => new.get(*ix).cloned().unwrap_or_default(),
            Some((false, ix)) => old.get(*ix).cloned().unwrap_or_default(),
            None => Vec::new(),
        })
        .collect()
}

/// The styles of the first `count` lines of `text`, joined with `\n` the way the caller
/// shows them, as ranges into that joined text.
pub fn join_lines(
    lines: &[LineStyles],
    text: &str,
    count: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for (line, styles) in text.lines().zip(lines).take(count) {
        // `lines()` drops a `\r` before the newline; a style may still reach it.
        out.extend(styles.iter().filter_map(|(range, style)| {
            let end = range.end.min(line.len());
            (range.start < end).then(|| (range.start + offset..end + offset, *style))
        }));
        offset += line.len() + 1;
    }
    out
}

/// Per-line styles keyed by content, valid for one highlight theme.
#[derive(Default)]
pub struct HighlightCache {
    theme: Option<Arc<HighlightTheme>>,
    entries: HashMap<u64, Rc<Vec<LineStyles>>>,
}

/// Cached results kept at most; past this the cache starts over. Each entry is small (only
/// what one tool call or file showed), and a long session revisits few of them.
const MAX_CACHED: usize = 512;

impl HighlightCache {
    /// The cached styles for `key`'s content under `theme`, computing them with `compute`
    /// on a miss.
    pub fn get_or_insert(
        &mut self,
        theme: &Arc<HighlightTheme>,
        key: impl Hash,
        compute: impl FnOnce(&HighlightTheme) -> Vec<LineStyles>,
    ) -> Rc<Vec<LineStyles>> {
        let same_theme = self
            .theme
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, theme) || **cached == **theme);
        if !same_theme {
            self.entries.clear();
            self.theme = Some(theme.clone());
        }
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(found) = self.entries.get(&key) {
            return found.clone();
        }
        if self.entries.len() >= MAX_CACHED {
            self.entries.clear();
        }
        let styles = Rc::new(compute(theme));
        self.entries.insert(key, styles.clone());
        styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Hsla, hsla};
    use pretty_assertions::assert_eq;

    #[test]
    fn languages_by_extension() {
        let cases = [
            ("src/main.rs", Some("rust")),
            ("a/b.PY", Some("python")),
            ("x.tsx", Some("tsx")),
            ("x.d.ts", Some("typescript")),
            ("x.hpp", Some("cpp")),
            ("x.h", Some("c")),
            ("Cargo.toml", Some("toml")),
            ("Cargo.lock", Some("toml")),
            ("ci.yml", Some("yaml")),
            ("run.sh", Some("bash")),
            ("README", None),
            ("x.unknown", None),
        ];
        for (path, expected) in cases {
            assert_eq!(language_for_path(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn splits_styles_onto_lines() {
        let text = "ab cd\n/* x\ny */ z\n";
        // `cd`, a comment over two lines, and `z`.
        let styles = [(3..5, 'k'), (6..15, 'c'), (16..17, 'v')];
        let lines = split_lines(text, &styles);
        assert_eq!(
            lines,
            vec![
                vec![(3..5, 'k')],
                vec![(0..4, 'c')],
                vec![(0..4, 'c'), (5..6, 'v')],
            ]
        );
        // No trailing newline: the last line still counts; empty lines stay empty.
        assert_eq!(split_lines("a\n\nb", &[(3..4, 'x')]).len(), 3);
        assert_eq!(split_lines("a\n\nb", &[(3..4, 'x')])[2], vec![(0..1, 'x')]);
        assert_eq!(split_lines("", &[(0..0, 'x')]), Vec::<Vec<_>>::new());
    }

    #[test]
    fn rebuilds_both_sides_of_a_diff() {
        let lines = [
            (Side::Neither, "@@ -1,3 +1,3 @@"),
            (Side::Both, "fn main() {"),
            (Side::Old, "    old();"),
            (Side::New, "    new();"),
            (Side::New, "    newer();"),
            (Side::Both, "}"),
        ];
        let sides = DiffSides::new(lines);
        assert_eq!(sides.old, "fn main() {\n    old();\n}\n");
        assert_eq!(sides.new, "fn main() {\n    new();\n    newer();\n}\n");
        assert_eq!(
            sides.map,
            vec![
                None,
                Some((true, 0)),
                Some((false, 1)),
                Some((true, 1)),
                Some((true, 2)),
                Some((true, 3)),
            ]
        );
    }

    /// Colors every highlight name the same, so tests can see which bytes got a color.
    struct Paint;

    const PAINT: Hsla = hsla(0.5, 0.5, 0.5, 1.);

    impl HighlightStyleResolver for Paint {
        fn style(&self, name: &str) -> Option<HighlightStyle> {
            matches!(name, "string" | "comment").then_some(HighlightStyle {
                color: Some(PAINT),
                background_color: Some(PAINT),
                ..Default::default()
            })
        }
    }

    fn colored<'a>(line: &'a str, styles: &LineStyles) -> Vec<&'a str> {
        styles
            .iter()
            .map(|(range, _)| &line[range.clone()])
            .collect()
    }

    #[test]
    fn diff_lines_take_colors_from_their_whole_side() {
        crate::syntax::init();
        // The string opens on a context line and closes on an added one: highlighted line
        // by line, neither part would be a string.
        let lines = [
            (Side::Both, "s = \"\"\"start"),
            (Side::Old, "old\"\"\""),
            (Side::New, "new\"\"\"  # done"),
        ];
        let styles = highlight_diff("python", lines, &Paint);
        assert_eq!(styles.len(), 3);
        assert_eq!(colored(lines[0].1, &styles[0]), ["\"\"\"start"]);
        assert_eq!(colored(lines[1].1, &styles[1]), ["old\"\"\""]);
        assert_eq!(colored(lines[2].1, &styles[2]), ["new\"\"\"", "# done"]);
        // Backgrounds are dropped so the added/removed backgrounds show through.
        assert!(
            styles
                .iter()
                .flatten()
                .all(|(_, s)| s.background_color.is_none())
        );
    }

    #[test]
    fn joins_line_styles_for_shown_text() {
        let text = "a 'x'\nb\nc 'y'\n";
        let lines = vec![
            vec![(2..5, HighlightStyle::default())],
            vec![],
            vec![(2..5, HighlightStyle::default())],
        ];
        let joined = join_lines(&lines, text, 2);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].0, 2..5);
        let joined = join_lines(&lines, text, 3);
        assert_eq!(
            joined.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            [2..5, 10..13]
        );
    }

    #[test]
    fn cache_computes_once_per_content_and_theme() {
        let mut cache = HighlightCache::default();
        let theme = HighlightTheme::default_dark();
        let mut runs = 0;
        for _ in 0..3 {
            cache.get_or_insert(&theme, ("rust", "fn a() {}"), |_| {
                runs += 1;
                vec![vec![]]
            });
        }
        assert_eq!(runs, 1);
        cache.get_or_insert(&theme, ("rust", "fn b() {}"), |_| {
            runs += 1;
            vec![]
        });
        assert_eq!(runs, 2);
        // Another theme recomputes.
        let light = HighlightTheme::default_light();
        cache.get_or_insert(&light, ("rust", "fn a() {}"), |_| {
            runs += 1;
            vec![]
        });
        assert_eq!(runs, 3);
    }
}
