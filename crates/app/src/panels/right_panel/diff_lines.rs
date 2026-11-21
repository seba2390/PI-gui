//! Parsing a unified (git) patch into display lines for the "Changes" tab.
//!
//! Pure and GPUI-free apart from [`SharedString`], so it runs on the background executor and
//! is unit-tested directly. Output is capped so a huge diff can't bloat memory or the UI.

use gpui_kit::SharedString;

/// At most this many lines (hunk headers included) are kept per file.
pub const MAX_LINES: usize = 2000;
/// Longer lines are cut (and marked with `…`): they are clipped on screen anyway, and shaping
/// megabyte-long minified lines every frame would stall rendering.
const MAX_LINE_CHARS: usize = 400;
const TAB: &str = "    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// `@@ -a,b +c,d @@ context` header.
    Hunk,
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` and similar annotations.
    Note,
}

impl LineKind {
    /// The sign shown in the marker column.
    pub fn sign(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Removed => "-",
            Self::Hunk | Self::Context | Self::Note => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old file (context and removed lines).
    pub old: Option<u32>,
    /// Line number in the new file (context and added lines).
    pub new: Option<u32>,
    /// The line without its `+`/`-`/` ` marker, tabs expanded.
    pub text: SharedString,
}

/// A parsed, capped patch for one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDiff {
    pub lines: Vec<DiffLine>,
    /// Patch lines dropped because of [`MAX_LINES`].
    pub hidden: usize,
    /// Git reported a binary file ("Binary files … differ").
    pub binary: bool,
    /// Digits of the largest line number, to size the gutter.
    pub number_digits: u32,
}

impl ParsedDiff {
    /// Parses the output of `git diff` for a single file. Headers before the first hunk
    /// (`diff --git`, `index`, `---`/`+++`, modes, renames) are skipped.
    pub fn parse(patch: &str, max_lines: usize) -> Self {
        let mut out = Self::default();
        let (mut old, mut new) = (0u32, 0u32);
        let mut in_hunk = false;
        let mut max_number = 0u32;

        for raw in patch.lines() {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if line.starts_with("@@") {
                in_hunk = true;
                if let Some((o, n)) = parse_hunk_header(line) {
                    (old, new) = (o, n);
                }
                if out.lines.len() < max_lines {
                    out.lines.push(DiffLine {
                        kind: LineKind::Hunk,
                        old: None,
                        new: None,
                        text: display_text(line),
                    });
                } else {
                    out.hidden += 1;
                }
                continue;
            }
            if !in_hunk {
                if line.starts_with("Binary files ") || line == "GIT binary patch" {
                    out.binary = true;
                }
                continue;
            }
            if line.starts_with("diff --git ") {
                // A second file in the same patch: back to header mode.
                in_hunk = false;
                continue;
            }
            let (kind, body, numbers) = match line.as_bytes().first() {
                Some(b'+') => (LineKind::Added, &line[1..], (None, Some(new))),
                Some(b'-') => (LineKind::Removed, &line[1..], (Some(old), None)),
                Some(b'\\') => (LineKind::Note, line, (None, None)),
                Some(b' ') => (LineKind::Context, &line[1..], (Some(old), Some(new))),
                // An empty line inside a hunk is an (unprefixed) empty context line.
                None => (LineKind::Context, "", (Some(old), Some(new))),
                Some(_) => continue,
            };
            match kind {
                LineKind::Added => new += 1,
                LineKind::Removed => old += 1,
                LineKind::Context => {
                    old += 1;
                    new += 1;
                }
                LineKind::Hunk | LineKind::Note => {}
            }
            max_number = max_number.max(old).max(new);
            if out.lines.len() < max_lines {
                out.lines.push(DiffLine {
                    kind,
                    old: numbers.0,
                    new: numbers.1,
                    text: display_text(body),
                });
            } else {
                out.hidden += 1;
            }
        }
        out.number_digits = digits(max_number);
        out
    }
}

/// `@@ -12,5 +12,7 @@ fn x` → the first old and new line numbers `(12, 12)`.
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let mut parts = line.split_whitespace().skip(1);
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

/// Expands tabs and cuts overly long lines, borrowing nothing from the patch.
fn display_text(text: &str) -> SharedString {
    let cut = text.char_indices().nth(MAX_LINE_CHARS).map(|(ix, _)| ix);
    let body = cut.map_or(text, |ix| &text[..ix]);
    let mut owned = if body.contains('\t') {
        body.replace('\t', TAB)
    } else {
        body.to_owned()
    };
    if cut.is_some() {
        owned.push('…');
    }
    SharedString::from(owned)
}

fn digits(mut n: u32) -> u32 {
    let mut count = 1;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::{LineKind, MAX_LINE_CHARS, ParsedDiff, digits, parse_hunk_header};

    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs
index 83db48f..bf269f4 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,4 +1,5 @@ fn main() {
 one
-two
+deux
+trois
\tfour

\\ No newline at end of file
";

    #[test]
    fn parses_lines_and_numbers() {
        let diff = ParsedDiff::parse(PATCH, 100);
        let kinds: Vec<_> = diff.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Hunk,
                LineKind::Context,
                LineKind::Removed,
                LineKind::Added,
                LineKind::Added,
                // "\tfour" starts with a tab, not a space: not a valid diff line, skipped.
                LineKind::Context,
                LineKind::Note,
            ]
        );
        assert_eq!(diff.lines[0].text.as_ref(), "@@ -1,4 +1,5 @@ fn main() {");
        assert_eq!((diff.lines[1].old, diff.lines[1].new), (Some(1), Some(1)));
        assert_eq!((diff.lines[2].old, diff.lines[2].new), (Some(2), None));
        assert_eq!((diff.lines[3].old, diff.lines[3].new), (None, Some(2)));
        assert_eq!((diff.lines[4].old, diff.lines[4].new), (None, Some(3)));
        assert_eq!((diff.lines[5].old, diff.lines[5].new), (Some(3), Some(4)));
        assert_eq!(diff.lines[2].text.as_ref(), "two");
        assert_eq!(diff.hidden, 0);
        assert!(!diff.binary);
        assert_eq!(diff.number_digits, 1);
    }

    #[test]
    fn caps_lines_and_counts_the_rest() {
        let body = (0..50)
            .map(|i| format!("+line {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let patch = format!("@@ -0,0 +1,50 @@\n{body}");
        let diff = ParsedDiff::parse(&patch, 10);
        assert_eq!(diff.lines.len(), 10);
        assert_eq!(diff.hidden, 41);
        assert_eq!(diff.lines[9].new, Some(9));
        assert_eq!(diff.number_digits, 2);
    }

    #[test]
    fn detects_binary_and_expands_tabs() {
        let diff = ParsedDiff::parse(
            "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n",
            10,
        );
        assert!(diff.binary);
        assert_eq!(diff.lines.len(), 0);

        let diff = ParsedDiff::parse("@@ -1 +1 @@\n+\tx\r\n", 10);
        assert_eq!(diff.lines[1].text.as_ref(), "    x");
    }

    #[test]
    fn cuts_long_lines() {
        let long = "x".repeat(MAX_LINE_CHARS + 50);
        let diff = ParsedDiff::parse(&format!("@@ -1 +1 @@\n+{long}\n"), 10);
        assert_eq!(diff.lines[1].text.chars().count(), MAX_LINE_CHARS + 1);
        assert!(diff.lines[1].text.ends_with('…'));
    }

    #[test]
    fn hunk_headers() {
        assert_eq!(parse_hunk_header("@@ -12,5 +14,7 @@ fn x"), Some((12, 14)));
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,3 @@"), Some((0, 1)));
        assert_eq!(parse_hunk_header("@@ garbage"), None);
        assert_eq!(digits(0), 1);
        assert_eq!(digits(9), 1);
        assert_eq!(digits(10), 2);
        assert_eq!(digits(12_345), 5);
    }
}
