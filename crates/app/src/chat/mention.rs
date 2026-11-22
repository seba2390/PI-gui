//! Composer helpers that work on plain text: the `@file` mention being typed at the cursor,
//! how pi writes a file reference, and walking back through earlier prompts with ↑ / ↓.

/// The `@` mention the cursor is in: `start` is the byte offset of the `@` (or of `@"` for a
/// quoted path) and `query` what has been typed after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionQuery<'a> {
    pub start: usize,
    pub query: &'a str,
}

/// Finds the mention being typed just before byte offset `cursor` in `text`, the way pi's
/// terminal does: an `@` at the start of a word, followed by a path without spaces, or by `"`
/// and a path that may contain spaces.
pub fn mention_at(text: &str, cursor: usize) -> Option<MentionQuery<'_>> {
    let before = text.get(..cursor)?;
    // Quoted form first: `@"some path` with no closing quote yet.
    if let Some(quote) = before.rfind("@\"") {
        let query = &before[quote + 2..];
        if !query.contains('"') && !query.contains('\n') && is_word_start(before, quote) {
            return Some(MentionQuery {
                start: quote,
                query,
            });
        }
    }
    let word_start = before.rfind(char::is_whitespace).map_or(0, |index| {
        index + before[index..].chars().next().map_or(1, char::len_utf8)
    });
    let word = &before[word_start..];
    let query = word.strip_prefix('@')?;
    (!query.contains('"')).then_some(MentionQuery {
        start: word_start,
        query,
    })
}

fn is_word_start(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

/// How pi writes a reference to `path`: `@path`, or `@"path"` when it contains whitespace.
pub fn file_reference(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// Earlier prompts for ↑ / ↓ in the composer, like a shell's history.
#[derive(Debug, Default)]
pub struct History {
    /// The entry shown, as an index into the list it was taken from (oldest first).
    index: Option<usize>,
    /// The text the user had typed before browsing, given back after the newest entry.
    draft: String,
}

impl History {
    /// Whether the composer shows a history entry (so ↑ / ↓ keep browsing).
    pub fn is_browsing(&self) -> bool {
        self.index.is_some()
    }

    /// Stops browsing: the user edited the entry, sent it, or cleared the composer.
    pub fn reset(&mut self) {
        self.index = None;
        self.draft.clear();
    }

    /// ↑: the entry before the one shown (the newest when not browsing yet), or `None` when
    /// there is nothing older. `current` is the composer's text.
    pub fn previous<'a>(&mut self, entries: &'a [String], current: &str) -> Option<&'a str> {
        let next = match self.index {
            None => {
                current.clone_into(&mut self.draft);
                entries.len().checked_sub(1)?
            }
            Some(0) => return None,
            Some(index) => index.min(entries.len()).checked_sub(1)?,
        };
        self.index = Some(next);
        entries.get(next).map(String::as_str)
    }

    /// ↓: the entry after the one shown, or the saved draft after the newest. `None` when not
    /// browsing.
    pub fn next(&mut self, entries: &[String]) -> Option<String> {
        let index = self.index?;
        if index + 1 < entries.len() {
            self.index = Some(index + 1);
            entries.get(index + 1).cloned()
        } else {
            self.index = None;
            Some(std::mem::take(&mut self.draft))
        }
    }
}

/// Earlier prompts, oldest first, without empty ones or repeats of the one just before.
pub fn history_entries<'a>(prompts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for prompt in prompts {
        let prompt = prompt.trim();
        if !prompt.is_empty() && entries.last().is_none_or(|last| last != prompt) {
            entries.push(prompt.to_owned());
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_plain_mention_at_the_cursor() {
        let text = "look at @src/ma please";
        let cursor = "look at @src/ma".len();
        assert_eq!(
            mention_at(text, cursor),
            Some(MentionQuery {
                start: 8,
                query: "src/ma"
            })
        );
        assert_eq!(mention_at("@", 1).map(|m| m.query), Some(""));
    }

    #[test]
    fn ignores_an_at_inside_a_word_or_after_a_space() {
        assert_eq!(mention_at("mail me@example.com", 19), None);
        assert_eq!(mention_at("@src done", 9), None);
        assert_eq!(mention_at("plain text", 5), None);
    }

    #[test]
    fn finds_a_quoted_mention_with_spaces() {
        let text = "see @\"My Docs/no";
        assert_eq!(
            mention_at(text, text.len()),
            Some(MentionQuery {
                start: 4,
                query: "My Docs/no"
            })
        );
        // Closed quotes end the mention.
        assert_eq!(mention_at("see @\"a b\" ", 11), None);
    }

    #[test]
    fn handles_multibyte_text_before_the_cursor() {
        let text = "æøå @fil";
        assert_eq!(mention_at(text, text.len()).map(|m| m.query), Some("fil"));
        // A cursor inside a multi-byte character is not a valid position.
        assert_eq!(mention_at("æ", 1), None);
    }

    #[test]
    fn quotes_paths_with_spaces_like_pi() {
        assert_eq!(file_reference("src/main.rs"), "@src/main.rs");
        assert_eq!(file_reference("My Docs/a.md"), "@\"My Docs/a.md\"");
    }

    #[test]
    fn walks_back_and_forth_through_history() {
        let entries = history_entries(["first", "second", "second", "  ", "third"]);
        assert_eq!(entries, ["first", "second", "third"]);
        let mut history = History::default();
        assert_eq!(history.previous(&entries, "draft"), Some("third"));
        assert_eq!(history.previous(&entries, "third"), Some("second"));
        assert_eq!(history.previous(&entries, "second"), Some("first"));
        assert_eq!(history.previous(&entries, "first"), None);
        assert_eq!(history.next(&entries).as_deref(), Some("second"));
        assert_eq!(history.next(&entries).as_deref(), Some("third"));
        assert_eq!(history.next(&entries).as_deref(), Some("draft"));
        assert!(!history.is_browsing());
        assert_eq!(history.next(&entries), None);
    }

    #[test]
    fn empty_history_does_nothing() {
        let mut history = History::default();
        assert_eq!(history.previous(&[], ""), None);
        assert!(!history.is_browsing());
    }
}
