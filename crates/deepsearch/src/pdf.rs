//! PDF files from the web: their text by the parser in Settings → Document parsing.

use std::path::Path;

use documents::config::{self, Config};
use documents::{Content, Kind};

use crate::extract::{Block, BlockKind, Page};

/// The text of a PDF, and why it was not read by the chosen parser.
pub fn read(bytes: Vec<u8>) -> Result<(Page, Option<String>), String> {
    let dir = config::default_dir().unwrap_or_else(|| std::env::temp_dir().join("pistachio"));
    read_in(&dir, &config::load(&dir), bytes)
}

fn read_in(dir: &Path, config: &Config, bytes: Vec<u8>) -> Result<(Page, Option<String>), String> {
    let timeout = config.web_timeout();
    let document = documents::read_bytes(dir, config, Kind::Pdf, bytes, timeout)?;
    let page = match document.content {
        Content::Pages(pages) => from_pages(pages),
        Content::Markdown(markdown) => crate::markdown::page(&markdown),
    };
    Ok((page, document.note))
}

/// A heading per page, so a passage says where it is from.
fn from_pages(pages: Vec<Vec<String>>) -> Page {
    let mut blocks = Vec::new();
    for (index, paragraphs) in pages.into_iter().enumerate() {
        if paragraphs.is_empty() {
            continue;
        }
        blocks.push(Block {
            kind: BlockKind::Heading(2),
            text: format!("Page {}", index + 1),
        });
        blocks.extend(paragraphs.into_iter().map(|text| Block {
            kind: BlockKind::Paragraph,
            text,
        }));
    }
    // A paper starts with its title.
    let title = blocks
        .iter()
        .find(|block| block.kind == BlockKind::Paragraph)
        .map(|block| block.text.chars().take(150).collect())
        .unwrap_or_default();
    Page {
        title,
        blocks,
        ..Page::default()
    }
}

#[cfg(test)]
mod tests {
    use documents::Parser;
    use pretty_assertions::assert_eq;

    use super::*;

    const REPORT: &[u8] = include_bytes!("../../documents/tests/report.pdf");

    #[test]
    fn says_which_page_a_passage_is_from() {
        let pages = vec![
            vec!["The Title".into(), "First.".into()],
            vec![],
            vec!["Third.".into()],
        ];
        let page = from_pages(pages);
        assert_eq!(page.title, "The Title");
        let texts: Vec<_> = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect();
        assert_eq!(texts, ["Page 1", "The Title", "First.", "Page 3", "Third."]);
    }

    #[test]
    fn a_parser_that_is_missing_is_replaced_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        let (page, note) = read_in(dir.path(), &config, REPORT.to_vec()).unwrap();
        assert_eq!(page.blocks[0].text, "Page 1");
        assert_eq!(note, None);
        config.parser = Parser::Docling;
        let (_, note) = read_in(dir.path(), &config, REPORT.to_vec()).unwrap();
        assert!(note.unwrap().starts_with("Docling could not be used"));
        assert!(read_in(dir.path(), &config, b"<html>".to_vec()).is_err());
    }
}
