//! Fitting a page into the model's budget: split it into passages, score them against what
//! the model is looking for (BM25), and keep the best ones in page order.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::extract::{Block, BlockKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    /// The nearest heading above the passage.
    pub heading: Option<String>,
    pub text: String,
}

/// Splits blocks into passages of about `target` characters, never across a heading.
pub fn passages(blocks: &[Block], target: usize) -> Vec<Passage> {
    let target = target.max(200);
    let mut out = Vec::new();
    let mut heading: Option<String> = None;
    let mut current = String::new();
    let flush = |current: &mut String, heading: &Option<String>, out: &mut Vec<Passage>| {
        let text = std::mem::take(current).trim().to_owned();
        if !text.is_empty() {
            out.push(Passage {
                heading: heading.clone(),
                text,
            });
        }
    };
    for block in blocks {
        if let BlockKind::Heading(_) = block.kind {
            flush(&mut current, &heading, &mut out);
            heading = Some(block.text.clone());
            continue;
        }
        let rendered = render_block(block);
        for piece in split_long(&rendered, target) {
            if !current.is_empty() && current.len() + piece.len() > target {
                flush(&mut current, &heading, &mut out);
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&piece);
        }
    }
    flush(&mut current, &heading, &mut out);
    out
}

fn render_block(block: &Block) -> String {
    match block.kind {
        BlockKind::Heading(level) => {
            format!("{} {}", "#".repeat(usize::from(level)), block.text)
        }
        BlockKind::Paragraph => block.text.clone(),
        BlockKind::ListItem => format!("- {}", block.text),
        BlockKind::Quote => format!("> {}", block.text),
        BlockKind::Code => format!("```\n{}\n```", block.text),
    }
}

/// Splits text longer than `target` at sentence (or line) ends.
fn split_long(text: &str, target: usize) -> Vec<String> {
    if text.len() <= target * 3 / 2 {
        return vec![text.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        let sentence_end = matches!(byte, b'.' | b'!' | b'?')
            && bytes
                .get(index + 1)
                .is_none_or(|next| next.is_ascii_whitespace());
        if sentence_end || byte == b'\n' {
            let sentence = &text[start..=index];
            start = index + 1;
            if !current.is_empty() && current.len() + sentence.len() > target {
                pieces.push(std::mem::take(&mut current).trim().to_owned());
            }
            current.push_str(sentence);
        }
    }
    current.push_str(&text[start..]);
    if !current.trim().is_empty() {
        pieces.push(current.trim().to_owned());
    }
    // A "sentence" can still be huge (minified text, tables): cut it by characters.
    pieces
        .into_iter()
        .flat_map(|piece| {
            if piece.len() <= target * 2 {
                return vec![piece];
            }
            let chars: Vec<char> = piece.chars().collect();
            chars
                .chunks(target)
                .map(|chunk| chunk.iter().collect())
                .collect()
        })
        .collect()
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "in", "is", "it", "of", "on", "or", "that", "the", "this", "to", "was", "what", "when",
    "where", "which", "who", "why", "with", "you", "about", "into", "than", "then", "there",
];

pub fn terms(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| {
            !term.is_empty()
                && (term.len() > 1 || term.chars().all(|c| c.is_ascii_digit()))
                && !STOPWORDS.contains(&term.as_str())
        })
        .collect()
}

/// BM25 score of every passage (heading included) for `query`.
pub fn scores(passages: &[Passage], query: &str) -> Vec<f64> {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let query: HashSet<String> = terms(query).into_iter().collect();
    if query.is_empty() || passages.is_empty() {
        return vec![0.0; passages.len()];
    }
    let documents: Vec<Vec<String>> = passages
        .iter()
        .map(|passage| {
            let mut words = terms(&passage.text);
            if let Some(heading) = &passage.heading {
                words.extend(terms(heading));
            }
            words
        })
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let average = documents.iter().map(Vec::len).sum::<usize>() as f64 / documents.len() as f64;
    let mut frequency: HashMap<&str, usize> = HashMap::new();
    for document in &documents {
        let unique: HashSet<&str> = document.iter().map(String::as_str).collect();
        for term in unique {
            *frequency.entry(term).or_default() += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let total = documents.len() as f64;
    documents
        .iter()
        .map(|document| {
            #[allow(clippy::cast_precision_loss)]
            let length = document.len() as f64;
            query
                .iter()
                .map(|term| {
                    #[allow(clippy::cast_precision_loss)]
                    let tf = document.iter().filter(|word| *word == term).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    let df = frequency.get(term.as_str()).copied().unwrap_or(0) as f64;
                    let idf = ((total - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * length / average.max(1.0)))
                })
                .sum()
        })
        .collect()
}

/// The passages to show, in page order: with a `focus`, the opening passage plus the best
/// matches; without one, the start of the page. Stays within `max_chars`.
pub fn select(passages: &[Passage], focus: Option<&str>, max_chars: usize) -> Vec<usize> {
    let cost = |index: usize| passages[index].text.len() + 2;
    let mut chosen = Vec::new();
    let mut used = 0;
    let scores = focus.map_or_else(Vec::new, |focus| scores(passages, focus));
    let ranked = scores.iter().any(|score| *score > 0.0);

    if ranked {
        // The opening usually summarises the page; keep it when it is not too large.
        if !passages.is_empty() && cost(0) <= max_chars / 4 {
            chosen.push(0);
            used += cost(0);
        }
        let mut order: Vec<usize> = (0..passages.len())
            .filter(|index| scores[*index] > 0.0)
            .collect();
        order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b)));
        for index in order {
            if !chosen.contains(&index) && used + cost(index) <= max_chars {
                chosen.push(index);
                used += cost(index);
            }
        }
    }
    // Fill what is left with the page from the top.
    for index in 0..passages.len() {
        if used + cost(index) > max_chars {
            if chosen.is_empty() {
                // Even the first passage is too big: show what fits of it.
                chosen.push(index);
            }
            break;
        }
        if !chosen.contains(&index) {
            chosen.push(index);
            used += cost(index);
        }
    }
    chosen.sort_unstable();
    chosen
}

/// The chosen passages as Markdown, with headings where the section changes and `[…]` where
/// passages were left out.
pub fn render(passages: &[Passage], chosen: &[usize], max_chars: usize) -> String {
    let mut out = String::new();
    let mut previous: Option<usize> = None;
    let mut heading: Option<&str> = None;
    for &index in chosen {
        let passage = &passages[index];
        if previous.is_some_and(|previous| index > previous + 1)
            || (previous.is_none() && index > 0)
        {
            out.push_str("[…]\n\n");
        }
        if passage.heading.as_deref() != heading {
            heading = passage.heading.as_deref();
            if let Some(heading) = heading {
                out.push_str("## ");
                out.push_str(heading);
                out.push_str("\n\n");
            }
        }
        out.push_str(&passage.text);
        out.push_str("\n\n");
        previous = Some(index);
    }
    if previous.is_some_and(|previous| previous + 1 < passages.len()) {
        out.push_str("[…]\n");
    }
    if out.len() > max_chars + 200 {
        let cut: String = out.chars().take(max_chars).collect();
        return format!("{cut}…\n");
    }
    out.trim_end().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn block(kind: BlockKind, text: &str) -> Block {
        Block {
            kind,
            text: text.to_owned(),
        }
    }

    fn sample() -> Vec<Passage> {
        let blocks = vec![
            block(BlockKind::Paragraph, "Overview of the GPU market in 2026."),
            block(BlockKind::Heading(2), "Pricing"),
            block(
                BlockKind::Paragraph,
                "The DGX Spark costs 3999 dollars at launch.",
            ),
            block(BlockKind::Heading(2), "History"),
            block(
                BlockKind::Paragraph,
                "Early accelerators were expensive and rare.",
            ),
            block(BlockKind::Heading(2), "Memory"),
            block(
                BlockKind::Paragraph,
                "The Spark has 128 GB of unified memory.",
            ),
        ];
        passages(&blocks, 200)
    }

    #[test]
    fn splits_passages_at_headings() {
        let passages = sample();
        assert_eq!(passages.len(), 4);
        assert_eq!(passages[1].heading.as_deref(), Some("Pricing"));
        assert_eq!(
            passages[1].text,
            "The DGX Spark costs 3999 dollars at launch."
        );
    }

    #[test]
    fn keeps_intro_and_best_matches_in_page_order() {
        let passages = sample();
        let chosen = select(&passages, Some("unified memory GB"), 80);
        assert_eq!(chosen, vec![0, 3]);
        let text = render(&passages, &chosen, 1000);
        assert_eq!(
            text,
            "Overview of the GPU market in 2026.\n\n[…]\n\n## Memory\n\nThe Spark has 128 GB \
             of unified memory.\n"
        );
    }

    #[test]
    fn without_focus_shows_the_start() {
        let passages = sample();
        assert_eq!(select(&passages, None, 90), vec![0, 1]);
        assert_eq!(select(&passages, Some("zzz"), 90), vec![0, 1]);
    }

    #[test]
    fn long_text_is_split_at_sentences() {
        let text = "One sentence here. ".repeat(40);
        let pieces = split_long(text.trim(), 100);
        assert!(pieces.len() > 5);
        assert!(pieces.iter().all(|piece| piece.len() <= 120));
        assert!(pieces[0].ends_with('.'));
    }
}
