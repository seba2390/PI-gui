//! Tidying the Markdown a parser writes.

use std::fmt::Write as _;

/// Longer lines are broken: readers of it (pi's `read`) show whole lines, up to a limit.
const MAX_LINE: usize = 8000;

/// `markdown` without what only takes space: embedded pictures, comments and runs of empty lines.
pub fn tidy(markdown: &str) -> String {
    let text = without_pictures(&without_comments(&markdown.replace("\r\n", "\n")));
    let mut out = String::with_capacity(text.len());
    let mut empty = true;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if !empty {
                out.push('\n');
            }
            empty = true;
            continue;
        }
        empty = false;
        for piece in pieces(line) {
            out.push_str(piece);
            out.push('\n');
        }
    }
    out.truncate(out.trim_end().len());
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Drops `<!-- … -->`, such as the marks for pictures and page breaks.
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("-->") else {
            rest = &rest[start..];
            break;
        };
        rest = &rest[start + end + 3..];
    }
    out + rest
}

/// Replaces pictures given as data, or as files that are not kept, by their description.
fn without_pictures(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("![") {
        let Some((alt, after)) = rest[start + 2..].split_once("](") else {
            break;
        };
        let Some(end) = after.find(')').filter(|_| !alt.contains('\n')) else {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            continue;
        };
        out.push_str(&rest[..start]);
        if !alt.trim().is_empty() {
            let _ = write!(out, "[Picture: {}]", alt.trim());
        }
        rest = &after[end + 1..];
    }
    out + rest
}

/// `line` in pieces of at most [`MAX_LINE`] bytes, broken at spaces.
fn pieces(mut line: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    while line.len() > MAX_LINE {
        let mut limit = MAX_LINE;
        while !line.is_char_boundary(limit) {
            limit -= 1;
        }
        let at = line[..limit]
            .rfind(' ')
            .filter(|at| *at > 0)
            .unwrap_or(limit);
        pieces.push(line[..at].trim_end());
        line = line[at..].trim_start();
    }
    pieces.push(line);
    pieces
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn drops_what_only_takes_space() {
        let markdown = "# Title  \r\n\r\n\r\n\r\n<!-- image -->\n\nText ![](data:image/png;base64,AAAA) \
                        more ![A chart](images/x.png).\n\n\n<!-- page\nbreak -->\nEnd\n\n";
        assert_eq!(
            tidy(markdown),
            "# Title\n\nText  more [Picture: A chart].\n\nEnd\n"
        );
        assert_eq!(tidy(" \n\n"), "");
        // Not a comment, and not pictures.
        assert_eq!(tidy("a <!-- b"), "a <!-- b\n");
        assert_eq!(tidy("wow![ and ](\nx ![y]"), "wow![ and ](\nx ![y]\n");
    }

    #[test]
    fn breaks_lines_no_reader_would_show() {
        let line = "wörd ".repeat(4000);
        let tidied = tidy(&line);
        assert!(tidied.lines().count() > 2);
        assert!(tidied.lines().all(|line| line.len() <= MAX_LINE));
        assert_eq!(tidied.split_whitespace().count(), 4000);
        let unbroken = "ø".repeat(MAX_LINE);
        assert_eq!(tidy(&unbroken).replace('\n', ""), unbroken);
    }
}
