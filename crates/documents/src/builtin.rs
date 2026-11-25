//! The built-in reader: the text of PDF files, page by page, and that of other documents
//! ([`office`]).

use std::fmt::Write as _;
use std::sync::mpsc;
use std::time::Duration;

/// How long reading one PDF may take.
const TIMEOUT: Duration = Duration::from_secs(40);
/// Deeply nested PDFs need more stack than a thread usually has.
const STACK: usize = 64 * 1024 * 1024;

use crate::{Kind, office};

/// The paragraphs of each page.
pub fn read(bytes: Vec<u8>) -> Result<Vec<Vec<String>>, String> {
    let pages = in_time(move || {
        pdf_extract::extract_text_from_mem_by_pages(&bytes).map_err(|error| error.to_string())
    })?;
    Ok(pages.iter().map(|page| paragraphs(page)).collect())
}

/// The text of a document of `kind`, which is no PDF, as Markdown.
pub fn read_other(kind: Kind, bytes: Vec<u8>) -> Result<String, String> {
    let text = in_time(move || office::read(kind, bytes))?;
    if text.trim().is_empty() {
        return Err("there is no text in it".into());
    }
    Ok(text)
}

fn in_time<T: Send + 'static>(
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = mpsc::channel();
    // Not joined: a PDF that takes too long is left to the end of the helper process.
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let _ = sender.send(read());
        })
        .map_err(|error| error.to_string())?;
    match receiver.recv_timeout(TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err("it takes too long to read".into()),
    }
}

/// The pages as Markdown, with a heading per page.
pub fn markdown(pages: &[Vec<String>]) -> String {
    let mut out = String::new();
    for (index, paragraphs) in pages.iter().enumerate() {
        if paragraphs.is_empty() {
            continue;
        }
        let _ = write!(out, "## Page {}\n\n", index + 1);
        for paragraph in paragraphs {
            out.push_str(paragraph);
            out.push_str("\n\n");
        }
    }
    out
}

/// Joins the lines of each paragraph, and words that were split over two lines.
fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
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
pub(crate) mod tests {
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
    fn markdown_says_which_page_a_paragraph_is_from() {
        let pages = [
            vec!["The Title".to_owned(), "First.".to_owned()],
            vec![],
            vec!["Third.".to_owned()],
        ];
        assert_eq!(
            markdown(&pages),
            "## Page 1\n\nThe Title\n\nFirst.\n\n## Page 3\n\nThird.\n\n"
        );
    }

    #[test]
    fn reads_a_pdf_and_refuses_other_files() {
        let mut pdf = hello_pdf();
        assert!(read(pdf.clone()).unwrap()[0][0].contains("Hello PDF"));
        pdf.truncate(20);
        assert!(read(pdf).is_err());
        assert!(read(b"<html>not a pdf</html>".to_vec()).is_err());
    }

    /// A one-page PDF saying "Hello PDF".
    pub(crate) fn hello_pdf() -> Vec<u8> {
        pdf("BT /F1 24 Tf 100 700 Td (Hello PDF) Tj ET")
    }

    /// A one-page PDF that draws `stream`.
    pub(crate) fn pdf(stream: &str) -> Vec<u8> {
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
