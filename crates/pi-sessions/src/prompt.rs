//! What a prompt says besides what the user typed.
//!
//! The model of a chat has no tools to open files, so the app sends their text along: one block
//! per file ([`block`]) before the message. Views show the files and the message ([`split`]), and
//! not the blocks. pi adds notes on the pictures it made smaller, which are not shown either.

use std::ops::Range;

use serde::Serialize;

const OPEN: &str = "<attachment name=\"";
const ID: &str = "\" id=\"";
const OPEN_END: &str = "\">\n";
const CLOSE: &str = "\n</attachment>";

/// A file that was attached to a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachedFile {
    pub name: String,
    /// What the app knows it by.
    pub id: Option<String>,
}

impl AttachedFile {
    /// The end of its name, in lower case and with the dot.
    pub fn extension(&self) -> Option<String> {
        extension(&self.name)
    }
}

/// The end of the file name `name`, in lower case and with the dot.
pub fn extension(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    let known = !stem.is_empty() && (1..=5).contains(&extension.chars().count());
    known.then(|| format!(".{}", extension.to_lowercase()))
}

/// A prompt as it was sent, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt<'a> {
    /// The attached files, each with its block.
    pub files: Vec<(AttachedFile, &'a str)>,
    /// What the user typed.
    pub text: &'a str,
    /// Where that is.
    pub range: Range<usize>,
}

impl Prompt<'_> {
    /// The prompt without what pi added to it.
    pub fn as_typed(&self) -> String {
        let blocks = self.files.iter().map(|(_, block)| *block);
        join(blocks, self.text)
    }

    /// A line that says what it is about: what was typed, or else the first file.
    pub fn title(&self) -> Option<&str> {
        let typed = self.text.lines().map(str::trim).find(|l| !l.is_empty());
        typed.or_else(|| Some(self.files.first()?.0.name.as_str()))
    }
}

/// The block of the file `name`, which says `contents`.
pub fn block(name: &str, id: Option<&str>, contents: &str) -> String {
    let id = id.map_or_else(String::new, |id| format!("{ID}{id}"));
    let contents = contents.trim_matches('\n');
    // Or the block would end there.
    let contents = contents.replace("</attachment>", "<\\/attachment>");
    format!("{OPEN}{}{id}{OPEN_END}{contents}{CLOSE}", escape(name))
}

/// A prompt of `blocks` and `text`.
pub fn join<'a>(blocks: impl IntoIterator<Item = &'a str>, text: &'a str) -> String {
    let mut prompt = String::new();
    for part in blocks.into_iter().chain([text]) {
        if !prompt.is_empty() && !part.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(part);
    }
    prompt
}

/// Several prompts as one: their files, then their texts.
pub fn merge(prompts: &[String]) -> String {
    let prompts: Vec<_> = prompts.iter().map(|prompt| split(prompt)).collect();
    let texts: Vec<_> = prompts
        .iter()
        .map(|prompt| prompt.text)
        .filter(|text| !text.is_empty())
        .collect();
    let text = texts.join("\n\n");
    let blocks = prompts.iter().flat_map(|prompt| &prompt.files);
    join(blocks.map(|(_, block)| *block), &text)
}

/// Takes a prompt apart.
pub fn split(prompt: &str) -> Prompt<'_> {
    let mut files = Vec::new();
    let mut start = 0;
    while let Some((file, len)) = first_block(&prompt[start..]) {
        files.push((file, &prompt[start..start + len]));
        start += len;
        // The blank line after it.
        start += prompt[start..].len() - prompt[start..].trim_start_matches('\n').len();
    }
    let end = start + without_notes(&prompt[start..]).len();
    Prompt {
        files,
        text: &prompt[start..end],
        range: start..end,
    }
}

/// The file whose block `text` starts with, and how long that is.
fn first_block(text: &str) -> Option<(AttachedFile, usize)> {
    let attributes = text.strip_prefix(OPEN)?;
    let (attributes, _) = attributes.split_once('\n')?;
    let attributes = attributes.strip_suffix(OPEN_END.trim_end())?;
    let (name, id) = match attributes.split_once(ID) {
        Some((name, id)) => (name, Some(id.to_owned())),
        None => (attributes, None),
    };
    let len = text.find(CLOSE)? + CLOSE.len();
    let ends = text[len..].is_empty() || text[len..].starts_with('\n');
    let name = unescape(name);
    ends.then_some((AttachedFile { name, id }, len))
}

/// `text` without the notes on pictures that pi adds at its end. That a picture was left out stays.
fn without_notes(text: &str) -> &str {
    let (before, last) = match text.rfind("\n\n") {
        Some(at) => (&text[..at], &text[at + 2..]),
        None => ("", text),
    };
    let is_note = |line: &str| {
        (line.starts_with("[Image: original ") || line.starts_with("[Image converted from "))
            && line.ends_with(']')
    };
    if !last.is_empty() && last.lines().all(is_note) {
        before
    } else {
        text
    }
}

fn escape(name: &str) -> String {
    name.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace(['\n', '\r'], " ")
}

fn unescape(name: &str) -> String {
    name.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn file(name: &str, id: Option<&str>) -> AttachedFile {
        AttachedFile {
            name: name.into(),
            id: id.map(str::to_owned),
        }
    }

    #[test]
    fn files_are_told_from_the_message() {
        let pdf = block(
            "a \"b\" <&>.pdf",
            Some("0f"),
            "# Hi\n\n</attachment>\n\nend\n",
        );
        assert_eq!(
            pdf,
            "<attachment name=\"a &quot;b&quot; &lt;&amp;&gt;.pdf\" id=\"0f\">\n\
             # Hi\n\n<\\/attachment>\n\nend\n</attachment>"
        );
        let txt = block("b.txt", None, "two");
        let sent = join([pdf.as_str(), &txt], "What is this?\n\nAnd that?");
        let prompt = split(&sent);
        assert_eq!(
            prompt.files,
            [
                (file("a \"b\" <&>.pdf", Some("0f")), pdf.as_str()),
                (file("b.txt", None), txt.as_str())
            ]
        );
        assert_eq!(prompt.text, "What is this?\n\nAnd that?");
        assert_eq!(&sent[prompt.range.clone()], prompt.text);
        assert_eq!(prompt.as_typed(), sent);
        assert_eq!(prompt.title(), Some("What is this?"));

        // Only a file, an empty one too.
        let sent = block("b.txt", None, "");
        let prompt = split(&sent);
        assert_eq!((prompt.files.len(), prompt.text), (1, ""));
        assert_eq!(prompt.title(), Some("b.txt"));
    }

    #[test]
    fn other_messages_stay_as_they_are() {
        for text in [
            "",
            "  hi\n",
            "<attachment name=\"a.pdf\">",
            "<attachment name=\"a.pdf\">\nno end",
            "See <attachment name=\"a.pdf\">\nx\n</attachment>",
            "<attachment name=\"a.pdf\">\nx\n</attachment> more",
        ] {
            let prompt = split(text);
            assert_eq!((prompt.files.len(), prompt.text), (0, text));
        }
    }

    #[test]
    fn notes_on_pictures_are_left_out() {
        let note = "[Image: original 2088x1606, displayed at 2000x1538. Multiply coordinates by \
                    1.04 to map to original image.]";
        let converted = "[Image converted from image/bmp to image/png.]";
        let text = |sent: &str| split(sent).text.to_owned();
        assert_eq!(text(&format!("what?\n\n{note}")), "what?");
        assert_eq!(text(&format!("a\n\nb\n\n{converted}\n{note}")), "a\n\nb");
        assert_eq!(text(&format!("\n\n{note}")), "");
        let omitted = "what?\n\n[Image omitted: could not be resized below the inline image size \
                       limit.]";
        assert_eq!(text(omitted), omitted);
        let sent = join(
            [block("a.txt", None, "x").as_str()],
            &format!("y\n\n{note}"),
        );
        assert_eq!(split(&sent).text, "y");
        assert!(!split(&sent).as_typed().contains("[Image"));
    }

    #[test]
    fn several_prompts_become_one() {
        let (a, b) = (block("a.txt", None, "x"), block("b.txt", None, "y"));
        let prompts = [
            join([a.as_str()], "one"),
            join([b.as_str()], "two"),
            a.clone(),
        ];
        assert_eq!(merge(&prompts), format!("{a}\n\n{b}\n\n{a}\n\none\n\ntwo"));
        assert_eq!(merge(&["one".into(), "two".into()]), "one\n\ntwo");
    }

    #[test]
    fn extensions() {
        for (name, extension) in [
            ("a.PDF", Some(".pdf")),
            ("a.tar.gz", Some(".gz")),
            ("Makefile", None),
            (".env", None),
            ("what. is this", None),
        ] {
            assert_eq!(file(name, None).extension().as_deref(), extension);
        }
    }
}
