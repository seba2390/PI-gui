//! Documents (PDF and Office files) attached to a chat: their text goes along with the message, as
//! a helper process reads it.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use deepsearch::excerpt::{self, Fitted};
use documents::client::{self, Stop, Stopped};

use super::composer::MAX_INLINE_BYTES;

/// How much of a document that is longer than [`MAX_INLINE_BYTES`] goes along with a message.
const EXCERPT_BYTES: usize = 64 * 1024;

/// A document that is being read; dropping it stops that.
pub struct Reading {
    _stop: Stop,
}

impl Reading {
    pub fn new() -> (Self, Stopped) {
        let (stop, stopped) = Stop::new();
        (Self { _stop: stop }, stopped)
    }
}

/// A document that was read.
pub struct AttachedDocument {
    text: excerpt::Document,
    reply: documents::cli::Reply,
}

impl AttachedDocument {
    /// Whether `path` is read like this.
    pub fn is_one(path: &Path) -> bool {
        documents::Kind::of(path).is_some()
    }

    /// Reads the document at `path`. Blocks until that is done or stopped.
    pub fn read(
        exe: Option<&PathBuf>,
        dir: &Path,
        path: &Path,
        stopped: &Stopped,
    ) -> Result<Self, String> {
        let exe = exe.ok_or("the app's own executable could not be found")?;
        let reply = client::read(exe, dir, path, stopped)?;
        Ok(Self::new(reply))
    }

    fn new(mut reply: documents::cli::Reply) -> Self {
        let markdown = std::mem::take(&mut reply.markdown);
        Self {
            text: excerpt::Document::new(markdown, MAX_INLINE_BYTES),
            reply,
        }
    }

    /// Why it was not read by the chosen parser.
    pub fn note(&self) -> Option<&str> {
        self.reply.note.as_deref()
    }

    /// What the model gets with a message that says `message`: the document, or what matches it.
    pub fn contents(&self, message: &str) -> Cow<'_, str> {
        match self.text.fit(message, EXCERPT_BYTES) {
            Fitted::Whole(text) => Cow::Borrowed(text),
            Fitted::Excerpt { text, shown, total } => Cow::Owned(format!(
                "(It is too long to include all of it. This is its outline, its start and what \
                 matches this message: {shown} of {total} parts, with […] where some are left \
                 out.)\n\n{text}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use documents::Parser;
    use documents::cli::Reply;

    use super::*;

    fn document(markdown: String, note: Option<&str>) -> AttachedDocument {
        AttachedDocument::new(Reply {
            markdown,
            parser: Parser::Docling,
            note: note.map(str::to_owned),
        })
    }

    #[test]
    fn the_model_gets_the_text_and_nothing_about_how_it_was_read() {
        let text = "# Hi\n\n```\ncode\n```\n";
        let document = document(text.into(), Some("MinerU failed."));
        assert_eq!(document.contents("x"), text);
        assert_eq!(document.note(), Some("MinerU failed."));
    }

    #[test]
    fn a_long_one_is_shortened_to_what_the_message_asks() {
        let mut text = String::from("# The Book\n\n");
        for n in 0..400 {
            let filler = format!("Chapter {n} is about topic{n}. ").repeat(30);
            let _ = write!(text, "## Chapter {n}\n\n{filler}\n\n");
        }
        assert!(text.len() > MAX_INLINE_BYTES);
        let book = document(text, None);
        let block = book.contents("What is topic321?");
        assert!(block.contains("too long to include all of it"));
        assert!(block.contains("about topic321."));
        assert!(block.contains("Outline:\n\n- The Book"));
        assert!(block.len() < EXCERPT_BYTES + 2000, "{}", block.len());
    }

    #[test]
    fn knows_documents_by_their_name() {
        assert!(AttachedDocument::is_one(Path::new("/a/B.PDF")));
        assert!(AttachedDocument::is_one(Path::new("a.docx")));
        assert!(!AttachedDocument::is_one(Path::new("a.txt")));
    }
}
