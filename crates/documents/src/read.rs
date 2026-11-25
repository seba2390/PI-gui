//! Reading one document with the chosen parser, or else with the built-in reader.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::cache::{self, Entry};
use crate::config::Config;
use crate::install::Layout;
use crate::{Installed, Parser, builtin, helper, installed, text};

/// What a document is, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Xlsb,
    Xls,
    Odt,
    Odp,
    Ods,
    Epub,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "pdf" => Some(Self::Pdf),
            // With macros or as a template, the text is the same.
            "docx" | "docm" | "dotx" => Some(Self::Docx),
            "pptx" | "pptm" | "ppsx" | "potx" => Some(Self::Pptx),
            "xlsx" | "xlsm" | "xltx" => Some(Self::Xlsx),
            "xlsb" => Some(Self::Xlsb),
            "xls" => Some(Self::Xls),
            "odt" | "ott" => Some(Self::Odt),
            "odp" | "otp" => Some(Self::Odp),
            "ods" | "ots" => Some(Self::Ods),
            "epub" => Some(Self::Epub),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx => "Word document",
            Self::Odt => "Text document",
            Self::Pptx | Self::Odp => "Presentation",
            Self::Xlsx | Self::Xlsb | Self::Xls | Self::Ods => "Spreadsheet",
            Self::Epub => "Book",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
            Self::Xlsx => "xlsx",
            Self::Xlsb => "xlsb",
            Self::Xls => "xls",
            Self::Odt => "odt",
            Self::Odp => "odp",
            Self::Ods => "ods",
            Self::Epub => "epub",
        }
    }
}

impl Parser {
    pub fn reads(self, kind: Kind) -> bool {
        match self {
            Self::Builtin => true,
            Self::Docling => matches!(kind, Kind::Pdf | Kind::Docx | Kind::Pptx | Kind::Xlsx),
            Self::Pymupdf4llm | Self::Mineru => kind == Kind::Pdf,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The paragraphs of each page.
    Pages(Vec<Vec<String>>),
    Markdown(String),
}

/// A document that was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub content: Content,
    /// What read it.
    pub parser: Parser,
    /// Why that is not the chosen parser.
    pub note: Option<String>,
}

impl Document {
    pub fn markdown(self) -> String {
        match self.content {
            Content::Pages(pages) => text::tidy(&builtin::markdown(&pages)),
            Content::Markdown(markdown) => markdown,
        }
    }
}

/// Reads the file at `path` with the settings in `dir`. Blocks; a downloaded parser gets
/// `timeout`.
pub fn read(
    dir: &Path,
    config: &Config,
    path: &Path,
    timeout: Duration,
) -> Result<Document, String> {
    let kind = Kind::of(path).ok_or("it is not a document that can be read")?;
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > config.max_file_bytes {
        return Err(format!(
            "it is larger than {} MB",
            config.max_file_bytes / 1_000_000
        ));
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    read_bytes(dir, config, kind, bytes, timeout)
}

/// [`read`] for a document that is not a file here.
pub fn read_bytes(
    dir: &Path,
    config: &Config,
    kind: Kind,
    bytes: Vec<u8>,
    timeout: Duration,
) -> Result<Document, String> {
    let parser = config.parser;
    if kind == Kind::Pdf && !is_pdf(&bytes) {
        return Err("it is not a PDF file".into());
    }
    // Those that read only some kinds leave the others to the built-in reader.
    let problem = if parser == Parser::Builtin || !parser.reads(kind) {
        None
    } else {
        match parse(dir, parser, kind, &bytes, timeout) {
            Ok(markdown) => {
                return Ok(Document {
                    content: Content::Markdown(markdown),
                    parser,
                    note: None,
                });
            }
            Err(error) => Some(error),
        }
    };
    let note = problem.map(|problem| {
        format!(
            "{} could not be used ({problem}), so the built-in reader was.",
            parser.label()
        )
    });
    let failed = |error: &str| match &note {
        Some(note) => format!("{error}. {}", note.trim_end_matches('.')),
        None => error.to_owned(),
    };
    if kind != Kind::Pdf {
        let markdown = builtin::read_other(kind, bytes).map_err(|error| failed(&error))?;
        return Ok(Document {
            content: Content::Markdown(markdown),
            parser: Parser::Builtin,
            note,
        });
    }
    let pages = builtin::read(bytes).map_err(|error| failed(&error))?;
    if pages.iter().all(Vec::is_empty) {
        return Err(failed(if parser.reads_scans() {
            "there is no text in it"
        } else {
            "there is no text in it. If it is a scan, Docling and MinerU can read it (Settings → \
             Document parsing)"
        }));
    }
    Ok(Document {
        content: Content::Pages(pages),
        parser: Parser::Builtin,
        note,
    })
}

/// The document as Markdown by `parser`, or why not.
fn parse(
    dir: &Path,
    parser: Parser,
    kind: Kind,
    bytes: &[u8],
    timeout: Duration,
) -> Result<String, String> {
    if installed(dir, parser) == Installed::No {
        return Err("it is not downloaded".into());
    }
    let layout = Layout::installed(dir, parser);
    let marker = fs::read_to_string(layout.marker()).unwrap_or_default();
    let revision = format!("{} {marker}", kind.extension());
    match cache::load(dir, bytes, &revision) {
        Some(Entry::Markdown(markdown)) => return Ok(markdown),
        Some(Entry::Failed(error)) => return Err(error),
        None => {}
    }
    // A copy: the parser needs nothing else of the document's folder, and no odd name breaks it.
    let run = helper::Run::new(dir)?;
    let file = run.dir.join(format!("document.{}", kind.extension()));
    fs::write(&file, bytes).map_err(|e| format!("writing {}: {e}", file.display()))?;
    let go = AtomicBool::new(false);
    let result = helper::parse(dir, &layout, parser, &file, timeout, &go).and_then(|markdown| {
        let markdown = text::tidy(&markdown);
        if markdown.is_empty() {
            Err("it found no text".into())
        } else {
            Ok(markdown)
        }
    });
    let entry = match &result {
        Ok(markdown) => Entry::Markdown(markdown.clone()),
        Err(error) => Entry::Failed(error.clone()),
    };
    // Not having had a turn says nothing about the document.
    if entry != Entry::Failed(helper::BUSY.into()) {
        cache::store(dir, bytes, &revision, &entry);
    }
    result
}

/// Whether `bytes` start like a PDF: readers accept some other bytes before its mark.
fn is_pdf(bytes: &[u8]) -> bool {
    let start = &bytes[..bytes.len().min(1024)];
    start.windows(5).any(|window| window == b"%PDF-")
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::builtin::tests::{hello_pdf, pdf};
    use crate::install::fake;

    const LONG: Duration = Duration::from_secs(20);

    fn config(parser: Parser) -> Config {
        Config {
            parser,
            ..Config::default()
        }
    }

    fn try_it(dir: &Path, parser: Parser, kind: Kind, bytes: Vec<u8>) -> Result<Document, String> {
        read_bytes(dir, &config(parser), kind, bytes, LONG)
    }

    #[test]
    fn the_built_in_reader_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let document = try_it(dir.path(), Parser::Builtin, Kind::Pdf, hello_pdf()).unwrap();
        assert_eq!(document.parser, Parser::Builtin);
        assert_eq!(document.note, None);
        assert!(document.markdown().starts_with("## Page 1\n\nHello PDF"));
        // It leaves nothing behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_chosen_parser_reads() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        // $3 is the document, $4 where the Markdown goes.
        fake(
            dir,
            Parser::Docling,
            "echo run >> \"$0.runs\"; printf '# Title\\n\\n\\n\\n<!-- image -->\\n' > \"$4\"; \
             echo \"${3##*/}\" >> \"$4\"",
        );
        for (kind, name) in [(Kind::Pdf, "document.pdf"), (Kind::Xlsx, "document.xlsx")] {
            for _ in 0..2 {
                let document = try_it(dir, Parser::Docling, kind, hello_pdf()).unwrap();
                assert_eq!(document.parser, Parser::Docling);
                assert_eq!(document.note, None);
                assert_eq!(document.markdown(), format!("# Title\n\n{name}\n"));
            }
        }
        // Once each.
        let python = Layout::installed(dir, Parser::Docling).python();
        let runs = format!("{}.runs", python.display());
        assert_eq!(fs::read_to_string(runs).unwrap(), "run\nrun\n");
        // Its files are gone.
        let left = fs::read_dir(dir.join("tmp")).unwrap().flatten();
        assert_eq!(left.map(|e| e.file_name()).collect::<Vec<_>>(), ["lock"]);
    }

    #[test]
    fn falls_back_to_the_built_in_reader_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let document = try_it(dir, Parser::Mineru, Kind::Pdf, hello_pdf()).unwrap();
        assert_eq!(document.parser, Parser::Builtin);
        assert_eq!(
            document.note.as_deref(),
            Some("MinerU could not be used (it is not downloaded), so the built-in reader was.")
        );
        fake(dir, Parser::Mineru, "echo 'MemoryError' >&2; exit 1");
        let document = try_it(dir, Parser::Mineru, Kind::Pdf, hello_pdf()).unwrap();
        assert!(document.note.unwrap().contains("(MemoryError)"));
        fake(dir, Parser::Pymupdf4llm, "echo ' ' > \"$4\"");
        let document = try_it(dir, Parser::Pymupdf4llm, Kind::Pdf, hello_pdf()).unwrap();
        assert!(document.note.unwrap().contains("found no text"));
        assert!(matches!(document.content, Content::Pages(_)));
    }

    #[test]
    fn a_parser_that_takes_too_long_is_not_tried_again_right_away() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fake(dir, Parser::Docling, "echo run >> \"$0.runs\"; sleep 60");
        let config = config(Parser::Docling);
        let short = Duration::from_secs(2);
        for _ in 0..2 {
            let document = read_bytes(dir, &config, Kind::Pdf, hello_pdf(), short).unwrap();
            assert!(document.note.unwrap().contains("took longer"));
        }
        let python = Layout::installed(dir, Parser::Docling).python();
        let runs = format!("{}.runs", python.display());
        assert_eq!(fs::read_to_string(runs).unwrap(), "run\n");
    }

    #[test]
    fn says_why_a_document_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let error = |parser, kind, bytes| try_it(dir, parser, kind, bytes).unwrap_err();
        assert!(error(Parser::Builtin, Kind::Docx, vec![1]).contains("damaged"));
        let text = error(Parser::Docling, Kind::Docx, vec![1]);
        assert!(
            text.contains("damaged") && text.contains("not downloaded"),
            "{text}"
        );
        let protected = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1".to_vec();
        assert!(error(Parser::Builtin, Kind::Docx, protected).contains("password"));
        assert_eq!(
            error(Parser::Builtin, Kind::Pdf, b"<html>".to_vec()),
            "it is not a PDF file"
        );
        // A page without text.
        let scan = pdf("0 0 100 100 re f");
        let text = error(Parser::Builtin, Kind::Pdf, scan.clone());
        assert!(text.contains("Docling and MinerU can read it"), "{text}");
        let text = error(Parser::Docling, Kind::Pdf, scan);
        assert!(text.contains("not downloaded"), "{text}");

        let path = dir.join("big.PDF");
        fs::write(&path, hello_pdf()).unwrap();
        let mut small = config(Parser::Builtin);
        assert!(read(dir, &small, &path, LONG).is_ok());
        small.max_file_bytes = 10;
        assert!(
            read(dir, &small, &path, LONG)
                .unwrap_err()
                .contains("larger")
        );
        assert!(read(dir, &small, &dir.join("a.txt"), LONG).is_err());
        assert!(read(dir, &small, &dir.join("none.pdf"), LONG).is_err());
    }

    /// The ones Docling does not read are read by the built-in reader, as are all with the others.
    #[test]
    fn each_kind_has_a_reader() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        fake(dir, Parser::Docling, "printf 'by Docling' > \"$4\"");
        fake(dir, Parser::Mineru, "printf 'by MinerU' > \"$4\"");
        let word = crate::office::testing::zip(&[(
            "word/document.xml",
            "<w:document xmlns:w=\"w\"><w:body><w:p><w:r><w:t>Hi</w:t></w:r></w:p></w:body></w:document>",
        )]);
        let odt = crate::office::testing::zip(&[(
            "content.xml",
            "<o:d xmlns:o=\"o\" xmlns:t=\"t\"><o:body><o:text><t:p>Hi</t:p></o:text></o:body></o:d>",
        )]);
        for (parser, kind, text) in [
            (Parser::Builtin, Kind::Docx, "Hi\n"),
            (Parser::Mineru, Kind::Docx, "Hi\n"),
            (Parser::Docling, Kind::Docx, "by Docling\n"),
            (Parser::Docling, Kind::Odt, "Hi\n"),
        ] {
            let bytes = if kind == Kind::Odt { &odt } else { &word };
            let document = try_it(dir, parser, kind, bytes.clone()).unwrap();
            assert_eq!(document.note, None);
            assert_eq!(document.markdown(), text, "{parser:?}");
        }
        // One that fails.
        fake(dir, Parser::Docling, "echo 'MemoryError' >&2; exit 1");
        let other = [word.as_slice(), b"\n"].concat();
        let document = try_it(dir, Parser::Docling, Kind::Docx, other).unwrap();
        assert!(document.note.unwrap().contains("MemoryError"));
        assert_eq!(Kind::of(Path::new("/a/b.Pptx")), Some(Kind::Pptx));
        assert_eq!(Kind::of(Path::new("/a/b.xlsm")), Some(Kind::Xlsx));
        assert_eq!(Kind::of(Path::new("/a/b.doc")), None);
    }
}
