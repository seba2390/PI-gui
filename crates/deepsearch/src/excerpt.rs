//! Fitting an attached document into a message: all of it, or its outline, its start and the
//! passages that match what the message asks.

use std::fmt::Write as _;

use crate::extract::BlockKind;
use crate::rank::{self, Passage};

const PASSAGE_CHARS: usize = 900;
/// The outline takes at most this much of an excerpt, and the start of the document as well.
const OUTLINE_BYTES: usize = 3000;
const START_BYTES: usize = 3000;

/// A document's Markdown, prepared so that fitting it is quick.
#[derive(Debug, Clone)]
pub struct Document {
    markdown: String,
    passages: Vec<Passage>,
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
    /// Splits `markdown` into passages if it has more than `max_whole` bytes.
    pub fn new(markdown: String, max_whole: usize) -> Self {
        if markdown.len() <= max_whole {
            return Self {
                markdown,
                passages: Vec::new(),
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
            outline,
            // Only the passages are shown.
            markdown: String::new(),
        }
    }

    /// The document, or of a long one about `max_bytes` that answer `question` best.
    pub fn fit(&self, question: &str, max_bytes: usize) -> Fitted<'_> {
        if self.passages.is_empty() {
            return Fitted::Whole(&self.markdown);
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
        let body = rank::render(passages, &chosen, max_bytes);
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const MAX: usize = 10_000;

    /// A book of 60 chapters, each about one thing.
    fn book() -> String {
        let mut text = String::from("# The Book\n\nThis book is about many things.\n\n");
        for n in 0..60 {
            let filler = format!("Chapter {n} says a lot about topic{n}. ").repeat(12);
            let _ = write!(text, "## Chapter {n}\n\n{filler}\n\n");
        }
        text
    }

    #[test]
    fn a_short_document_stays_whole() {
        let document = Document::new("# Hi\n\nText.\n".into(), 100);
        assert_eq!(document.fit("x", 10), Fitted::Whole("# Hi\n\nText.\n"));
    }

    #[test]
    fn a_long_one_is_cut_to_what_was_asked() {
        let document = Document::new(book(), MAX);
        let Fitted::Excerpt { text, shown, total } = document.fit("What about topic42?", MAX)
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
        let Fitted::Excerpt { text, .. } = document.fit("topic7", MAX) else {
            panic!("not cut");
        };
        assert!(text.contains("a lot about topic7."));
        assert!(!text.contains("a lot about topic42."));
    }

    #[test]
    fn without_a_question_it_is_the_start() {
        let document = Document::new(book(), MAX);
        let Fitted::Excerpt { text, .. } = document.fit("", MAX) else {
            panic!("not cut");
        };
        assert!(text.contains("a lot about topic0.") && text.contains("a lot about topic5."));
        assert!(!text.contains("a lot about topic59."));
    }

    #[test]
    fn one_huge_paragraph_is_cut_too() {
        let document = Document::new("word ".repeat(20_000), MAX);
        let Fitted::Excerpt { text, shown, total } = document.fit("word", MAX) else {
            panic!("not cut");
        };
        assert!(text.len() <= MAX + 500 && shown < total);
        assert!(!text.contains("Outline"));
    }
}
