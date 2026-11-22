//! Find in chat: matching a query against the text the transcript shows.
//!
//! The app searches rendered markdown itself (it owns the views); this module matches text
//! and lists the plain-text fields of an item, in the order they appear on screen: thinking,
//! tool output or an edit's diff lines, and a shell command's output.

use std::borrow::Cow;
use std::ops::Range;

use crate::model::{BlockKind, ItemKind, TranscriptItem};
use crate::tool::{self, DiffKind};

/// Expanded tool output shows at most this many lines, so only those are searched.
pub const MAX_OUTPUT_LINES: usize = 400;

/// The byte ranges of `query` in `haystack`, ignoring case, in order and not overlapping.
/// An empty (or all-space) query matches nothing.
pub fn find_matches(haystack: &str, query: &str) -> Vec<Range<usize>> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut matches = Vec::new();
    let mut search_from = 0;
    for (start, _) in haystack.char_indices() {
        if start < search_from {
            continue;
        }
        let mut chars = haystack[start..].char_indices();
        let mut end = None;
        for (i, want) in needle.iter().enumerate() {
            match chars.next() {
                Some((offset, c)) if fold(c) == *want => {
                    if i + 1 == needle.len() {
                        end = Some(start + offset + c.len_utf8());
                    }
                }
                _ => break,
            }
        }
        if let Some(end) = end {
            matches.push(start..end);
            search_from = end;
        }
    }
    matches
}

/// Case folding for matching one character to one character (so byte ranges stay those of
/// the original text). `İ` and the like lowercase to two characters; their first is enough.
fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// The text an expanded output box shows: the first [`MAX_OUTPUT_LINES`] lines of `output`.
pub fn shown_output(output: &str) -> Cow<'_, str> {
    let trimmed = output.trim_end();
    match trimmed
        .match_indices('\n')
        .nth(MAX_OUTPUT_LINES.saturating_sub(1))
    {
        // More lines than shown: cut after the last shown one.
        Some((ix, _)) => Cow::Owned(trimmed[..ix].lines().collect::<Vec<_>>().join("\n")),
        None if trimmed.contains('\r') => {
            Cow::Owned(trimmed.lines().collect::<Vec<_>>().join("\n"))
        }
        None => Cow::Borrowed(trimmed),
    }
}

/// Which plain-text part of a block a match is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// A thinking block's text (trimmed, as shown).
    Thinking,
    /// A tool call's (or shell command's) output, as [`shown_output`] gives it.
    Output,
    /// One line of an edit's diff.
    DiffLine(usize),
}

/// The plain-text fields of `item` worth searching, as `(block index, field, text)`, in
/// screen order. Markdown blocks (user prompts and assistant text) are not included: they
/// are searched in their rendered form.
pub fn plain_fields(item: &TranscriptItem) -> Vec<(usize, Field, Cow<'_, str>)> {
    let mut fields = Vec::new();
    match &item.kind {
        ItemKind::Assistant(assistant) => {
            for (index, block) in assistant.blocks.iter().enumerate() {
                match &block.kind {
                    BlockKind::Thinking(thinking) if !thinking.redacted => {
                        fields.push((index, Field::Thinking, Cow::Borrowed(thinking.text.trim())));
                    }
                    BlockKind::Tool(tool) => match tool::diff_lines(tool) {
                        Some(lines) => {
                            for (line_ix, line) in lines.into_iter().enumerate() {
                                if line.kind != DiffKind::Gap {
                                    fields.push((
                                        index,
                                        Field::DiffLine(line_ix),
                                        Cow::Owned(line.text),
                                    ));
                                }
                            }
                        }
                        None => fields.push((index, Field::Output, shown_output(&tool.output))),
                    },
                    _ => {}
                }
            }
        }
        ItemKind::Bash { output, .. } => fields.push((0, Field::Output, shown_output(output))),
        _ => {}
    }
    fields
}

/// The match after (or before) `current` among `total`, wrapping around; the first (or
/// last) when there is no current one.
pub fn step(current: Option<usize>, total: usize, forward: bool) -> Option<usize> {
    if total == 0 {
        return None;
    }
    Some(match (current, forward) {
        (None, true) => 0,
        (None, false) => total - 1,
        (Some(ix), true) => (ix + 1) % total,
        (Some(ix), false) => (ix + total - 1) % total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantItem, Block, ItemId, ThinkingBlock, ToolBlock, ToolStatus};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn matches_ignore_case_and_do_not_overlap() {
        assert_eq!(
            find_matches("Needle, needle, NEEDLE", "needle"),
            [0..6, 8..14, 16..22]
        );
        assert_eq!(find_matches("aaaa", "aa"), [0..2, 2..4]);
        assert_eq!(find_matches("abc", ""), Vec::<Range<usize>>::new());
        assert_eq!(find_matches("abc", "  "), Vec::<Range<usize>>::new());
        assert_eq!(find_matches("abc", "abcd"), Vec::<Range<usize>>::new());
        // Byte ranges of the original text, with multi-byte characters around.
        let text = "Ærø — ÆRØ";
        let found = find_matches(text, "ærø");
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[1].clone()], "ÆRØ");
    }

    #[test]
    fn shown_output_is_capped_like_the_output_box() {
        assert_eq!(shown_output("a\nb\n\n"), "a\nb");
        assert_eq!(shown_output("a\r\nb\r\n"), "a\nb");
        let long = (0..MAX_OUTPUT_LINES + 5).fold(String::new(), |mut text, i| {
            use std::fmt::Write as _;
            let _ = writeln!(text, "line {i}");
            text
        });
        let shown = shown_output(&long);
        assert_eq!(shown.lines().count(), MAX_OUTPUT_LINES);
        assert!(shown.ends_with(&format!("line {}", MAX_OUTPUT_LINES - 1)));
    }

    #[test]
    fn steps_wrap_around() {
        assert_eq!(step(None, 0, true), None);
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(None, 3, false), Some(2));
        assert_eq!(step(Some(2), 3, true), Some(0));
        assert_eq!(step(Some(0), 3, false), Some(2));
        assert_eq!(step(Some(1), 3, true), Some(2));
    }

    fn tool(name: &str, output: &str, details: Option<serde_json::Value>) -> ToolBlock {
        ToolBlock {
            call_id: "c".into(),
            name: name.into(),
            args: json!({}),
            args_partial: String::new(),
            status: ToolStatus::Done,
            output: output.into(),
            details,
            duration_ms: None,
            nested: Vec::new(),
        }
    }

    #[test]
    fn lists_plain_fields_in_screen_order() {
        let thinking = ThinkingBlock {
            text: "  hmm  ".into(),
            redacted: false,
            done: true,
            duration: None,
            started_at: None,
        };
        let block = |content_index, kind| Block {
            content_index,
            kind,
        };
        let item = TranscriptItem {
            id: ItemId(1),
            version: 0,
            kind: ItemKind::Assistant(AssistantItem {
                blocks: vec![
                    block(0, BlockKind::Thinking(thinking)),
                    block(1, BlockKind::Tool(tool("bash", "out\n", None))),
                    block(
                        2,
                        BlockKind::Tool(tool(
                            "edit",
                            "Successfully replaced",
                            Some(json!({"diff": " 1 a\n-2 b\n+2 c\n     ..."})),
                        )),
                    ),
                ],
                model: String::new(),
                stop_reason: pi_rpc::protocol::StopReason::Stop,
                error: None,
            }),
        };
        let fields: Vec<(usize, Field, String)> = plain_fields(&item)
            .into_iter()
            .map(|(block, field, text)| (block, field, text.into_owned()))
            .collect();
        assert_eq!(
            fields,
            vec![
                (0, Field::Thinking, "hmm".to_owned()),
                (1, Field::Output, "out".to_owned()),
                (2, Field::DiffLine(0), "a".to_owned()),
                (2, Field::DiffLine(1), "b".to_owned()),
                (2, Field::DiffLine(2), "c".to_owned()),
            ]
        );
    }
}
