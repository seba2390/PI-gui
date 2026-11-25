//! Really downloads the parsers and reads a PDF with two columns and a table (and a Word file):
//! `cargo test -p documents --test live -- --ignored --test-threads 1 --nocapture`, or with the
//! name of one of them, e.g. `… --ignored pymupdf4llm`.
//!
//! `PISTACHIO_DOCUMENTS_LIVE=<folder>` keeps the parsers there instead of in a temporary folder.

#![allow(clippy::unwrap_used)] // also in what the tests share

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use documents::config::Config;
use documents::{Document, Installed, Kind, Parser, install, installed, read, read_bytes, remove};
use pretty_assertions::assert_eq;

const REPORT: &[u8] = include_bytes!("report.pdf");
const LONG: Duration = Duration::from_secs(600);

fn with(parser: Parser, test: impl FnOnce(&Path)) {
    if let Err(reason) = parser.download() {
        println!("{}: {reason}", parser.label());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let kept = std::env::var_os("PISTACHIO_DOCUMENTS_LIVE");
    let dir = kept.as_ref().map_or_else(
        // Spaces and letters outside ASCII are fine in a user's folder name.
        || temp.path().join("Søren's files").join("documents"),
        |dir| Path::new(dir).to_path_buf(),
    );
    if installed(&dir, parser) != Installed::Current {
        let started = Instant::now();
        install(&dir, parser, &AtomicBool::new(false), &|progress| {
            println!("{:?} {:.0}%", progress.stage, progress.fraction() * 100.);
        })
        .unwrap();
        println!("installed in {:?}", started.elapsed());
    }
    assert_eq!(installed(&dir, parser), Installed::Current);
    let root = dir.join("parsers").join(parser.id());
    assert!(!root.join("partial").exists() && !root.join("app/tools").exists());
    test(&dir);
    if kept.is_none() {
        remove(&dir, parser).unwrap();
        assert_eq!(installed(&dir, parser), Installed::No);
    }
}

fn parse(dir: &Path, parser: Parser, kind: Kind, bytes: &[u8]) -> Document {
    let config = Config {
        parser,
        ..Config::default()
    };
    let started = Instant::now();
    let document = read_bytes(dir, &config, kind, bytes.to_vec(), LONG).unwrap();
    println!("read in {:?}", started.elapsed());
    document
}

fn reads_the_report(parser: Parser) {
    with(parser, |dir| {
        let document = parse(dir, parser, Kind::Pdf, REPORT);
        assert_eq!(document.note, None);
        assert_eq!(document.parser, parser);
        let markdown = document.markdown();
        println!("{markdown}");
        let at = |text: &str| markdown.find(text).unwrap_or_else(|| panic!("no {text}"));
        // Headings, and one column after the other.
        let heading = |text: &str| {
            let line = markdown.lines().find(|line| line.contains(text)).unwrap();
            assert!(line.starts_with('#'), "{line}");
        };
        heading("The Pistachio Report");
        heading("Harvest");
        assert!(at("left column ends") < at("Right column starts"));
        assert!(at("right column ends") < at("Harvest"));
        // The table, as Markdown or HTML.
        let row = markdown
            .lines()
            .find(|line| line.contains("523900"))
            .unwrap();
        assert!(row.contains("USA") && row.contains("48 %"), "{row}");
        assert!(row.contains('|') || row.contains("<td"), "{row}");
        at("go well with software");
        // Again: from the cache.
        let started = Instant::now();
        assert_eq!(parse(dir, parser, Kind::Pdf, REPORT).markdown(), markdown);
        assert!(started.elapsed() < Duration::from_secs(2));
    });
}

#[test]
#[ignore = "downloads PyMuPDF4LLM"]
fn pymupdf4llm() {
    reads_the_report(Parser::Pymupdf4llm);
}

#[test]
#[ignore = "downloads Docling"]
fn docling() {
    reads_the_report(Parser::Docling);
    with(Parser::Docling, |dir| {
        let document = parse(
            dir,
            Parser::Docling,
            Kind::Docx,
            include_bytes!("letter.docx"),
        );
        let markdown = document.markdown();
        println!("{markdown}");
        assert!(markdown.contains("# A Letter"));
        assert!(markdown.contains("Dear pistachio"));
    });
}

#[test]
#[ignore = "downloads MinerU"]
fn mineru() {
    reads_the_report(Parser::Mineru);
}

/// The built-in reader needs no download.
#[test]
fn builtin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.pdf");
    std::fs::write(&path, REPORT).unwrap();
    let document = read(dir.path(), &Config::default(), &path, LONG).unwrap();
    assert_eq!((document.parser, &document.note), (Parser::Builtin, &None));
    let markdown = document.markdown();
    assert!(markdown.starts_with("## Page 1\n"));
    assert!(markdown.contains("523900") && markdown.contains("## Page 2\n"));
}
