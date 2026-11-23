//! Short names for conversations, made from their first message.
//!
//! pi names a session only when asked (`/name`, `set_session_name`); the app can do it
//! automatically. This crate holds the parts that do not need the UI:
//!
//! - [`worth_naming`], [`as_is`], [`excerpt`]: what a namer gets to read.
//! - [`clean`]: turns whatever a model answered into a short title.
//! - [`print`]: one call to the conversation's own model through `pi --print`, without a
//!   session, so the conversation's file and history are untouched.
//! - [`local`]: a small model on this computer (quantized GGUF, run with candle).

pub mod local;
pub mod print;

/// The most words a title keeps: models asked for 3–6 words sometimes answer a sentence.
pub const MAX_WORDS: usize = 8;
/// The most characters a title keeps (the sidebar shows about 30 of them).
pub const MAX_CHARS: usize = 60;
/// How much of the first message a namer reads. The start says what a conversation is
/// about; a long paste would only make the call slower (and dearer).
pub const EXCERPT_CHARS: usize = 1500;

/// What a namer is told (the system prompt).
pub const INSTRUCTIONS: &str = "You name conversations. Given the first message of a \
conversation, reply with a short title of 3 to 6 words that says what it is about. Do not \
answer the message or do what it asks (no code, no answers): only name it. Reply with the \
title only: no quotes, no trailing punctuation, no explanation. Write the title in the \
language of the message.";

/// Whether a first message gets a name at all: not an empty one (only pictures) and not a
/// slash command, which pi expands into a prompt the user never wrote.
pub fn worth_naming(message: &str) -> bool {
    let text = message.trim();
    !text.is_empty() && !text.starts_with('/')
}

/// A first message short enough to be its own title ("Fix the login bug"): one line of at
/// most four words. Asking a model to shorten it would only reword it.
pub fn as_is(message: &str) -> Option<String> {
    let text = message.trim();
    if !worth_naming(text)
        || text.contains('\n')
        || text.split_whitespace().count() > 4
        || text.chars().count() > 32
    {
        return None;
    }
    clean(text)
}

/// The start of `message` a namer reads: at most `max_chars` characters, cut between words
/// when possible, with "…" when something was left out.
pub fn excerpt(message: &str, max_chars: usize) -> String {
    let text = message.trim();
    let Some((cut, _)) = text.char_indices().nth(max_chars) else {
        return text.to_owned();
    };
    let head = &text[..cut];
    let head = head
        .rfind(char::is_whitespace)
        .filter(|space| *space > cut / 2)
        .map_or(head, |space| &head[..space]);
    format!("{}…", head.trim_end())
}

/// Quotes and Markdown a model wraps a title in.
fn is_wrapping(c: char) -> bool {
    matches!(
        c,
        '"' | '\'' | '`' | '*' | '_' | '#' | '“' | '”' | '‘' | '’' | '«' | '»' | '„'
    ) || c.is_whitespace()
}

/// What may end a title before clean-up: wrapping plus punctuation.
fn is_trailing(c: char) -> bool {
    is_wrapping(c) || matches!(c, '.' | '!' | '?' | ',' | ';' | ':' | '…' | '-' | '–' | '—')
}

/// Words a title cut short should not end on ("Debugging the login flow of the").
const DANGLING: &[&str] = &[
    "a", "an", "and", "as", "at", "by", "for", "from", "in", "into", "of", "on", "or", "the", "to",
    "with", "who", "which", "that", "than", "more", "most", "is", "are", "my", "your", "when",
    "how", "what", "about", "og", "i", "til", "på", "en", "et", "der", "som", "af",
];

/// Quote marks that come in pairs; a title cut short can be left with one of a pair.
const QUOTES: &[char] = &['\'', '"', '`', '‘', '’', '“', '”'];

/// `words` with quote marks at word edges removed when they don't pair up (a title cut
/// mid-quote). Apostrophes inside words ("don't") are untouched.
fn balance_quotes(words: &[&str]) -> Vec<String> {
    let edge_quotes = words
        .iter()
        .map(|word| {
            usize::from(word.starts_with(QUOTES))
                + usize::from(word.len() > 1 && word.ends_with(QUOTES))
        })
        .sum::<usize>();
    words
        .iter()
        .map(|word| {
            if edge_quotes % 2 == 1 {
                word.trim_matches(QUOTES).to_owned()
            } else {
                (*word).to_owned()
            }
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// A model's answer as a title: the first line that is not a lead-in ("Here is a title:"),
/// without a "Title:" label, quotes, Markdown or trailing punctuation, on one line, at most
/// [`MAX_WORDS`] words and [`MAX_CHARS`] characters. `None` when nothing is left.
pub fn clean(raw: &str) -> Option<String> {
    let text = strip_thinking(raw);
    let line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.trim_matches(is_trailing).is_empty())
        .find(|line| !line.ends_with(':'))?;
    let line = strip_label(line);
    let line = line
        .trim_start_matches(is_wrapping)
        .trim_end_matches(is_trailing);
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0;
    for word in words.iter().take(MAX_WORDS) {
        let len = word.chars().count() + usize::from(!kept.is_empty());
        if chars + len > MAX_CHARS {
            break;
        }
        chars += len;
        kept.push(word);
    }
    if kept.is_empty() {
        // One very long "word" (a URL, say): keep its start.
        let word = words.first()?;
        return Some(word.chars().take(MAX_CHARS).collect());
    }
    if kept.len() < words.len() {
        while kept.len() > 1
            && kept
                .last()
                .is_some_and(|word| DANGLING.contains(&word.to_lowercase().as_str()))
        {
            kept.pop();
        }
    }
    let title = balance_quotes(&kept).join(" ");
    let title = title.trim_end_matches(is_trailing);
    (!title.is_empty()).then(|| title.to_owned())
}

/// `text` without `<think>…</think>` blocks that reasoning models put in their answer; an
/// unclosed block (cut off) leaves nothing after it.
fn strip_thinking(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `line` without a leading label such as "Title:" or "**Conversation name:**".
fn strip_label(line: &str) -> &str {
    let Some((head, rest)) = line.split_once(':') else {
        return line;
    };
    let head = head.trim_matches(is_wrapping).to_lowercase();
    let labelled = head.split_whitespace().count() <= 3
        && ["title", "name", "subject", "topic"]
            .iter()
            .any(|label| head.contains(label));
    if labelled && !rest.trim_matches(is_trailing).is_empty() {
        rest
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn titles_cut_short_end_cleanly() {
        // Seen from Qwen2.5 0.5B: too long, cut mid-phrase and mid-quote.
        assert_eq!(
            clean("Fixing 'cannot borrow `x` as mutable more than once' in Rust").as_deref(),
            Some("Fixing cannot borrow x as mutable")
        );
        assert_eq!(
            clean("SQL query to find the ten customers who spent the most").as_deref(),
            Some("SQL query to find the ten customers")
        );
        // Short enough: left alone, apostrophes and paired quotes included.
        assert_eq!(
            clean("Don't panic: the `x` borrow").as_deref(),
            Some("Don't panic: the `x` borrow")
        );
    }

    #[test]
    fn cleans_model_answers() {
        let cases = [
            ("Fixing the login bug", "Fixing the login bug"),
            ("\"Fixing the Login Bug.\"", "Fixing the Login Bug"),
            (
                "Title: Rust lifetimes explained",
                "Rust lifetimes explained",
            ),
            ("**Title:** *Rust lifetimes*", "Rust lifetimes"),
            (
                "## Planning a trip to Japan\n\nThis title…",
                "Planning a trip to Japan",
            ),
            (
                "Here is a title:\n\n“Sourdough starter tips”",
                "Sourdough starter tips",
            ),
            ("  Debug   CI\tfailures!  ", "Debug CI failures"),
            (
                "<think>The user wants…</think>\nCSV parsing in Python",
                "CSV parsing in Python",
            ),
            ("Why does my test fail?", "Why does my test fail"),
            ("Don't panic: a guide", "Don't panic: a guide"),
        ];
        for (raw, title) in cases {
            assert_eq!(clean(raw).as_deref(), Some(title), "{raw:?}");
        }
    }

    #[test]
    fn long_answers_are_cut_between_words() {
        assert_eq!(
            clean("Setting up a reproducible development environment for the new team").as_deref(),
            Some("Setting up a reproducible development environment")
        );
        let url = format!("https://{}", "x".repeat(100));
        assert_eq!(clean(&url).map(|t| t.chars().count()), Some(MAX_CHARS));
        assert!(clean(&"word ".repeat(30)).unwrap().chars().count() <= MAX_CHARS);
    }

    #[test]
    fn nothing_usable_is_none() {
        for raw in ["", "   ", "\"\"", "...", "Title:", "<think>still thinking"] {
            assert_eq!(clean(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn short_messages_are_their_own_title() {
        assert_eq!(
            as_is("Fix the login bug.").as_deref(),
            Some("Fix the login bug")
        );
        assert_eq!(as_is("  hi  ").as_deref(), Some("hi"));
        assert_eq!(as_is("Please look at why the build fails"), None);
        assert_eq!(as_is("two\nlines"), None);
        assert_eq!(as_is("/review"), None);
    }

    #[test]
    fn worth_naming_skips_commands_and_empty_messages() {
        assert!(worth_naming("Explain this"));
        assert!(!worth_naming("  "));
        assert!(!worth_naming("/skill:review the diff"));
    }

    #[test]
    fn excerpts_cut_long_messages() {
        assert_eq!(excerpt("  short  ", 10), "short");
        assert_eq!(excerpt("one two three four", 12), "one two…");
        assert_eq!(excerpt("ææææææææ", 4), "ææææ…");
    }
}
