//! Files whose text goes along with the message of a chat: documents (PDF, Office and others), as a
//! helper process reads them, and plain text.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use deepsearch::excerpt::{self, Fitted};
use documents::client::{self, Stop, Stopped};

/// Plain text files larger than this are not read.
pub const MAX_TEXT_FILE: u64 = 16 * 1024 * 1024;
/// What is left of a text that is shortened, at least: less says too little.
const MIN_EXCERPT: usize = 16 * 1024;
/// That, where nothing else limits it.
const USUAL_EXCERPT: usize = 64 * 1024;
/// How many bytes of text a token is, with some to spare: source code and tables take more tokens
/// than prose.
const BYTES_PER_TOKEN: u64 = 3;

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

/// How much text may go along with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// Of one file, as the settings say.
    limit: Option<usize>,
    /// What the model still has room for, if that is known.
    left: Option<usize>,
}

impl Room {
    /// With `limit` in the settings, for a model that has used `usage.0` tokens of `usage.1`.
    pub fn new(limit: Option<usize>, usage: Option<(u64, u64)>) -> Self {
        let left = usage
            .filter(|(_, window)| *window > 0)
            .map(|(used, window)| {
                // A fifth is for the message and the answer.
                let tokens = window.saturating_sub(used).saturating_sub(window / 5);
                usize::try_from(tokens * BYTES_PER_TOKEN).unwrap_or(usize::MAX)
            });
        Self { limit, left }
    }

    /// A file up to this size goes whole.
    fn whole(&self) -> usize {
        let limit = self.limit.unwrap_or(usize::MAX);
        limit.min(self.left.unwrap_or(usize::MAX))
    }

    /// A longer one becomes about so long.
    fn excerpt(&self) -> usize {
        let wanted = match (self.limit, self.left) {
            (Some(limit), _) => limit / 4,
            (None, Some(left)) => left / 2,
            (None, None) => USUAL_EXCERPT,
        };
        wanted.min(self.left.unwrap_or(usize::MAX)).max(MIN_EXCERPT)
    }

    /// After a text of `bytes`.
    pub fn take(&mut self, bytes: usize) {
        if let Some(left) = &mut self.left {
            *left = left.saturating_sub(bytes);
        }
    }
}

/// A file that was read.
pub struct AttachedDocument {
    text: excerpt::Document,
    note: Option<String>,
}

impl AttachedDocument {
    /// Whether `path` is read by the helper.
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
        Ok(Self {
            text: excerpt::Document::new(reply.markdown),
            note: reply.note,
        })
    }

    /// The plain text file at `path`. Blocks.
    pub fn read_plain(path: &Path) -> Result<Self, &'static str> {
        const UNREADABLE: &str = "could not be read";
        let meta = std::fs::metadata(path).map_err(|_| UNREADABLE)?;
        // Not a folder, or a pipe that would never end.
        if !meta.is_file() {
            return Err(UNREADABLE);
        }
        if meta.len() > MAX_TEXT_FILE {
            return Err("larger than 16 MB");
        }
        let bytes = std::fs::read(path).map_err(|_| UNREADABLE)?;
        let text = documents::plain::decode(bytes).ok_or("not a text file")?;
        Ok(Self::plain(path, text))
    }

    /// `text`, which is what `path` says.
    pub fn plain(path: &Path, text: String) -> Self {
        let markdown = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        Self {
            text: if markdown {
                excerpt::Document::new(text)
            } else {
                excerpt::Document::lines(text)
            },
            note: None,
        }
    }

    /// Why it was not read by the chosen parser.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn text(&self) -> &str {
        self.text.text()
    }

    /// Whether the model gets only a part of it.
    pub fn is_cut(&self, room: &Room) -> bool {
        self.text.is_cut(room.whole())
    }

    /// What the model gets with a message that says `message`: the document, or what matches it.
    pub fn contents(&self, message: &str, room: &Room) -> Cow<'_, str> {
        match self.text.fit(message, room.whole(), room.excerpt()) {
            Fitted::Whole(text) => Cow::Borrowed(text),
            Fitted::Excerpt { text, shown, total } => Cow::Owned(format!(
                "(It is too long to include all of it. This is its start and what matches this \
                 message, after its outline if it has one: {shown} of {total} parts, with […] \
                 where some are left out.)\n\n{text}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use pretty_assertions::assert_eq;

    use super::*;

    const KB: usize = 1024;

    fn room(limit: usize) -> Room {
        Room::new(Some(limit), None)
    }

    fn document(markdown: String) -> AttachedDocument {
        AttachedDocument {
            text: excerpt::Document::new(markdown),
            note: Some("MinerU failed.".into()),
        }
    }

    #[test]
    fn the_model_gets_the_text_and_nothing_about_how_it_was_read() {
        let text = "# Hi\n\n```\ncode\n```\n";
        let document = document(text.into());
        assert_eq!(document.contents("x", &room(256 * KB)), text);
        assert_eq!(document.note(), Some("MinerU failed."));
    }

    #[test]
    fn a_long_one_is_shortened_to_what_the_message_asks() {
        let mut text = String::from("# The Book\n\n");
        for n in 0..400 {
            let filler = format!("Chapter {n} is about topic{n}. ").repeat(30);
            let _ = write!(text, "## Chapter {n}\n\n{filler}\n\n");
        }
        assert!(text.len() > 256 * KB);
        let book = document(text.clone());
        let block = book.contents("What is topic321?", &room(256 * KB));
        assert!(block.contains("too long to include all of it"));
        assert!(block.contains("about topic321."));
        assert!(block.contains("Outline:\n\n- The Book"));
        assert!(block.len() < 64 * KB + 2000, "{}", block.len());
        assert!(book.is_cut(&room(256 * KB)));
        // Whole, with another limit or without.
        for limit in [Some(1024 * KB), None] {
            let room = Room::new(limit, Some((0, 1_000_000)));
            assert_eq!(book.contents("x", &room), text);
            assert!(!book.is_cut(&room));
        }
        // Unless the model has no room for it.
        let room = Room::new(None, Some((20_000, 100_000)));
        assert!(book.contents("topic321", &room).len() < 100 * KB);
    }

    #[test]
    fn room_is_what_the_settings_and_the_model_leave() {
        let room = Room::new(Some(256 * KB), None);
        assert_eq!((room.whole(), room.excerpt()), (256 * KB, 64 * KB));
        // 200,000 tokens, of which 160,000 are left for text.
        let mut room = Room::new(Some(1024 * KB), Some((0, 200_000)));
        assert_eq!((room.whole(), room.excerpt()), (480_000, 256 * KB));
        room.take(400_000);
        assert_eq!((room.whole(), room.excerpt()), (80_000, 80_000));
        room.take(80_001);
        assert_eq!((room.whole(), room.excerpt()), (0, MIN_EXCERPT));
        assert_eq!(Room::new(None, Some((500, 100))).whole(), 0);
        assert_eq!(Room::new(None, Some((0, 0))).whole(), usize::MAX);
        assert_eq!(Room::new(None, None).excerpt(), USUAL_EXCERPT);
    }

    #[test]
    fn plain_text_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, contents: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, contents).unwrap();
            AttachedDocument::read_plain(&path)
        };
        let text = |name, contents: &[u8]| write(name, contents).unwrap().text().to_owned();
        assert_eq!(text("a.txt", "grød\n".as_bytes()), "grød\n");
        assert_eq!(text("b.txt", b"\xff\xfeh\0i\0"), "hi");
        assert_eq!(
            write("c.zip", b"PK\x03\x04\0\0\xff").err(),
            Some("not a text file")
        );
        let large = vec![b'a'; usize::try_from(MAX_TEXT_FILE).unwrap() + 1];
        assert_eq!(write("d.log", &large).err(), Some("larger than 16 MB"));
        let folder = AttachedDocument::read_plain(dir.path());
        assert_eq!(folder.err(), Some("could not be read"));
        // A log that is too long is cut between lines.
        let log: String = (0..20_000).fold(String::new(), |mut log, n| {
            let _ = writeln!(log, "{n} something happened");
            log
        });
        let log = write("e.log", log.as_bytes()).unwrap();
        let contents = log.contents("12345", &room(256 * KB));
        assert!(contents.contains("\n12345 something happened\n"));
        assert!(contents.contains("\n0 something happened\n1 something"));
    }

    #[test]
    fn knows_documents_by_their_name() {
        assert!(AttachedDocument::is_one(Path::new("/a/B.PDF")));
        assert!(AttachedDocument::is_one(Path::new("a.docx")));
        assert!(AttachedDocument::is_one(Path::new("a.epub")));
        assert!(!AttachedDocument::is_one(Path::new("a.txt")));
    }
}
