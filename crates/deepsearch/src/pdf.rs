//! Text from PDF files.

use std::sync::mpsc;
use std::time::Duration;

use crate::extract::{Block, BlockKind, Page};

/// How long reading one PDF may take.
const TIMEOUT: Duration = Duration::from_secs(40);
/// Deeply nested PDFs need more stack than a thread usually has.
const STACK: usize = 64 * 1024 * 1024;

/// The text of a PDF, with a heading per page (so a passage says where it is from).
pub fn read(bytes: Vec<u8>) -> Result<Page, String> {
    let (sender, receiver) = mpsc::channel();
    // Not joined: a PDF that takes too long is left to the end of the helper process.
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let _ = sender.send(pdf_extract::extract_text_from_mem_by_pages(&bytes));
        })
        .map_err(|error| error.to_string())?;
    match receiver.recv_timeout(TIMEOUT) {
        Ok(Ok(pages)) => Ok(page(&pages)),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("it takes too long to read".into()),
    }
}

fn page(pages: &[String]) -> Page {
    let mut blocks = Vec::new();
    for (index, text) in pages.iter().enumerate() {
        let paragraphs = paragraphs(text);
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

/// Joins the lines of each paragraph, and words that were split over two lines.
fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = crate::extract::squash(line);
        if line.is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let split_word = current.ends_with('-')
            && current
                .chars()
                .rev()
                .nth(1)
                .is_some_and(char::is_alphabetic)
            && line.starts_with(char::is_lowercase);
        if split_word {
            current.pop();
        } else if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&line);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn joins_lines_and_split_words() {
        let text = "A Title\n\nDeep research agents answer complex ques-\ntions through search.\n\
                    Qwen3-\n27B is a model, GPT-\n5 too.\n\n  \nNext.";
        assert_eq!(
            paragraphs(text),
            [
                "A Title",
                "Deep research agents answer complex questions through search. Qwen3- 27B is a \
                 model, GPT- 5 too.",
                "Next.",
            ]
        );
    }

    #[test]
    fn says_which_page_a_passage_is_from() {
        let page = page(&["The Title\n\nFirst.".into(), " \n".into(), "Third.".into()]);
        assert_eq!(page.title, "The Title");
        let texts: Vec<_> = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect();
        assert_eq!(texts, ["Page 1", "The Title", "First.", "Page 3", "Third."]);
    }

    #[test]
    fn reads_a_pdf_and_refuses_other_files() {
        let mut pdf = hello_pdf();
        assert!(
            read(pdf.clone()).unwrap().blocks[1]
                .text
                .contains("Hello PDF")
        );
        pdf.truncate(20);
        assert!(read(pdf).is_err());
        assert!(read(b"<html>not a pdf</html>".to_vec()).is_err());
    }

    /// A one-page PDF saying "Hello PDF".
    fn hello_pdf() -> Vec<u8> {
        let stream = "BT /F1 24 Tf 100 700 Td (Hello PDF) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            let _ = write!(out, "{} 0 obj\n{object}\nendobj\n", index + 1);
        }
        let xref = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in offsets {
            let _ = writeln!(out, "{offset:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        );
        out.into_bytes()
    }
}
