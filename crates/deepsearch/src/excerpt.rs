//! Fitting an attached document into a message: all of it, or its outline, its start and the
//! passages that match what the message asks.

use std::fmt::Write as _;

use crate::extract::BlockKind;
use crate::rank::{self, Passage};

const PASSAGE_CHARS: usize = 900;
/// Shorter documents are always whole, so they are not split.
pub const ALWAYS_WHOLE: usize = 32 * 1024;
/// The outline takes at most this much of an excerpt, and the start of the document as well.
const OUTLINE_BYTES: usize = 3000;
const START_BYTES: usize = 3000;

/// A document's text, prepared so that fitting it is quick.
#[derive(Debug, Clone)]
pub struct Document {
    text: String,
    passages: Vec<Passage>,
    /// The passages are lines that follow each other, and not paragraphs.
    lines: bool,
    /// The headings.
    outline: Vec<(u8, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fitted<'a> {
    Whole(&'a str),
    /// Its outline and some of its passages.
    Excerpt {
        text: String,
        shown: usize,
        total: usize,
    },
}

impl Document {
    /// A document that is Markdown.
    pub fn new(markdown: String) -> Self {
        if markdown.len() <= ALWAYS_WHOLE {
            return Self {
                text: markdown,
                passages: Vec::new(),
                lines: false,
                outline: Vec::new(),
            };
        }
        let blocks = crate::markdown::blocks(&markdown);
        let outline = blocks
            .iter()
            .filter_map(|block| match block.kind {
                BlockKind::Heading(level) => Some((level, block.text.clone())),
                _ => None,
            })
            .collect();
        Self {
            passages: rank::passages(&blocks, PASSAGE_CHARS),
            lines: false,
            outline,
            text: markdown,
        }
    }

    /// A document that is plain lines: a log, a table, source code.
    pub fn lines(text: String) -> Self {
        let passages = if text.len() <= ALWAYS_WHOLE {
            Vec::new()
        } else {
            line_passages(&text)
        };
        Self {
            text,
            passages,
            lines: true,
            outline: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether [`Self::fit`] leaves something out.
    pub fn is_cut(&self, max_whole: usize) -> bool {
        self.text.len() > max_whole && !self.passages.is_empty()
    }

    /// The document, or of one that is longer than `max_whole` about `max_bytes` that answer
    /// `question` best.
    pub fn fit(&self, question: &str, max_whole: usize, max_bytes: usize) -> Fitted<'_> {
        if !self.is_cut(max_whole) {
            return Fitted::Whole(&self.text);
        }
        let passages = &self.passages;
        let mut start = Vec::new();
        let mut used = 0;
        for (index, passage) in passages.iter().enumerate() {
            used += passage.text.len() + 2;
            if used > START_BYTES && index > 0 {
                break;
            }
            start.push(index);
        }
        let outline = self.outline();
        let left = max_bytes.saturating_sub(outline.len() + START_BYTES);
        let rest = &passages[start.len()..];
        let mut chosen = start.clone();
        chosen.extend(
            rank::select(rest, Some(question), left)
                .into_iter()
                .map(|index| index + start.len()),
        );
        let body = if self.lines {
            render_lines(passages, &chosen)
        } else {
            rank::render(passages, &chosen, max_bytes)
        };
        Fitted::Excerpt {
            text: format!("{outline}{body}"),
            shown: chosen.len(),
            total: passages.len(),
        }
    }

    fn outline(&self) -> String {
        if self.outline.is_empty() {
            return String::new();
        }
        let top = self.outline.iter().map(|(level, _)| *level).min();
        let mut out = String::from("Outline:\n\n");
        for (level, heading) in &self.outline {
            if out.len() + heading.len() > OUTLINE_BYTES {
                out.push_str("- […]\n");
                break;
            }
            let indent = usize::from(level - top.unwrap_or(1));
            let _ = writeln!(out, "{}- {heading}", "  ".repeat(indent));
        }
        out.push_str("\n---\n\n");
        out
    }
}

/// Passages of whole lines, where one is not too long itself.
fn line_passages(text: &str) -> Vec<Passage> {
    let mut passages = Vec::new();
    let mut current = String::new();
    let mut add = |text: String| {
        if !text.trim().is_empty() {
            passages.push(Passage {
                heading: None,
                text,
            });
        }
    };
    for line in text.lines() {
        // Best where there is an empty line.
        let full = current.len() + line.len() > PASSAGE_CHARS
            || (line.trim().is_empty() && current.len() > PASSAGE_CHARS / 2);
        if full && !current.is_empty() {
            add(std::mem::take(&mut current));
        }
        let mut rest = line;
        while rest.len() > 2 * PASSAGE_CHARS {
            let mut at = PASSAGE_CHARS;
            while !rest.is_char_boundary(at) {
                at -= 1;
            }
            add(rest[..at].to_owned());
            rest = &rest[at..];
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(rest);
    }
    add(current);
    passages
}

/// The chosen passages, with `[…]` where some were left out.
fn render_lines(passages: &[Passage], chosen: &[usize]) -> String {
    let mut out = String::new();
    let mut next = 0;
    for &index in chosen {
        if index > next {
            out.push_str("[…]\n");
        }
        out.push_str(&passages[index].text);
        out.push('\n');
        next = index + 1;
    }
    if next < passages.len() {
        out.push_str("[…]\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const MAX: usize = 10_000;
    const ALL: usize = ALWAYS_WHOLE;

    /// A book of 120 chapters, each about one thing.
    fn book() -> String {
        let mut text = String::from("# The Book\n\nThis book is about many things.\n\n");
        for n in 0..120 {
            let filler = format!("Chapter {n} says a lot about topic{n}. ").repeat(12);
            let _ = write!(text, "## Chapter {n}\n\n{filler}\n\n");
        }
        text
    }

    #[test]
    fn a_short_document_stays_whole() {
        let document = Document::new("# Hi\n\nText.\n".into());
        assert!(!document.is_cut(1));
        assert_eq!(document.fit("x", 1, 10), Fitted::Whole("# Hi\n\nText.\n"));
    }

    #[test]
    fn a_long_one_is_cut_to_what_was_asked() {
        let document = Document::new(book());
        let Fitted::Excerpt { text, shown, total } = document.fit("What about topic42?", ALL, MAX)
        else {
            panic!("not cut");
        };
        assert!(text.len() <= MAX + 500, "{}", text.len());
        assert!(shown < total && shown > 3);
        assert!(text.starts_with("Outline:\n\n- The Book\n  - Chapter 0\n"));
        assert!(text.contains("This book is about many things."));
        assert!(text.contains("a lot about topic42."), "{text}");
        assert!(text.contains("[…]"));
        // Another question gets other passages.
        let Fitted::Excerpt { text, .. } = document.fit("topic7", ALL, MAX) else {
            panic!("not cut");
        };
        assert!(text.contains("a lot about topic7."));
        assert!(!text.contains("a lot about topic42."));
    }

    #[test]
    fn without_a_question_it_is_the_start() {
        let document = Document::new(book());
        let Fitted::Excerpt { text, .. } = document.fit("", ALL, MAX) else {
            panic!("not cut");
        };
        assert!(text.contains("a lot about topic0.") && text.contains("a lot about topic5."));
        assert!(!text.contains("a lot about topic119."));
    }

    #[test]
    fn one_huge_paragraph_is_cut_too() {
        let document = Document::new("word ".repeat(20_000));
        let Fitted::Excerpt { text, shown, total } = document.fit("word", ALL, MAX) else {
            panic!("not cut");
        };
        assert!(text.len() <= MAX + 500 && shown < total);
        assert!(!text.contains("Outline"));
    }

    #[test]
    fn a_long_one_is_whole_where_there_is_room() {
        let text = book();
        let document = Document::new(text.clone());
        assert!(document.is_cut(text.len() - 1) && !document.is_cut(text.len()));
        assert_eq!(document.fit("x", text.len(), 10), Fitted::Whole(&text));
        assert_eq!(document.text(), text);
    }

    /// Lines stay lines: a table keeps its columns' names and whole rows.
    #[test]
    fn lines_stay_whole() {
        let mut csv = String::from("id,name,price\n");
        for n in 0..3000 {
            let _ = writeln!(csv, "{n},boat{n},{}", n * 7);
        }
        let document = Document::lines(csv.clone());
        let Fitted::Excerpt { text, shown, total } = document.fit("boat2345", ALL, MAX) else {
            panic!("not cut");
        };
        assert!(text.len() <= MAX + 500 && shown < total, "{}", text.len());
        assert!(text.starts_with("id,name,price\n0,boat0,0\n"));
        assert!(text.contains("\n2345,boat2345,16415\n"));
        let rows = text.lines().filter(|line| *line != "[…]");
        assert!(rows.clone().all(|row| csv.contains(&format!("{row}\n"))));
        // Parts that follow each other are together, and nothing is there twice.
        let mut seen = std::collections::HashSet::new();
        assert!(rows.clone().all(|row| seen.insert(row)));
        assert!(text.ends_with("[…]\n"));
        // One line that is too long, of characters that are more than a byte.
        let document = Document::lines("ø".repeat(40_000));
        let Fitted::Excerpt { text, .. } = document.fit("x", ALL, MAX) else {
            panic!("not cut");
        };
        assert!(text.len() <= MAX + 500);
    }
}
