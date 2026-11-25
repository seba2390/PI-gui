//! OpenDocument texts and presentations (LibreOffice and others).

use std::fmt::Write as _;

use roxmltree::Node;

use super::markdown::{Markdown, link};
use super::package::{Package, attribute, child, elements, parse};
use super::slides;

const MAIN: &str = "content.xml";
/// Empty rows and cells can be repeated very often.
const MAX_REPEATED: usize = 64;

pub fn text(bytes: Vec<u8>) -> Result<String, String> {
    with_body(bytes, "text", |body, out| {
        let mut reader = Reader::new(out);
        reader.blocks(body);
        reader.write_notes();
    })
}

pub fn slides(bytes: Vec<u8>) -> Result<String, String> {
    with_body(bytes, "presentation", |body, out| {
        let pages = elements(body).filter(|node| node.tag_name().name() == "page");
        for (index, page) in pages.enumerate() {
            let mut title = String::new();
            for frame in elements(page).filter(|frame| is_title(*frame)) {
                let mut inline = Reader::new(out);
                inline.inline(frame, &mut title);
            }
            out.heading(2, &slides::heading(index + 1, &title));
            let mut reader = Reader::new(out);
            for frame in elements(page) {
                match frame.tag_name().name() {
                    "notes" => {
                        let mut said = String::new();
                        reader.inline(frame, &mut said);
                        let said = super::markdown::squash(&said);
                        if !said.is_empty() {
                            reader.out.paragraph(&format!("Notes: {said}"));
                        }
                    }
                    _ if is_title(frame) => {}
                    _ => reader.blocks(frame),
                }
            }
            reader.write_notes();
        }
    })
}

fn with_body(
    bytes: Vec<u8>,
    kind: &str,
    read: impl FnOnce(Node<'_, '_>, &mut Markdown),
) -> Result<String, String> {
    let mut package = Package::open(bytes)?;
    let encrypted = package
        .text("META-INF/manifest.xml")?
        .is_some_and(|manifest| manifest.contains("encryption-data"));
    if encrypted {
        return Err("it is protected with a password".into());
    }
    let content = package.needed(MAIN)?;
    let xml = parse(&content)?;
    let body = child(xml.root_element(), "body")
        .and_then(|body| child(body, kind))
        .ok_or("it is damaged, or not what its name says")?;
    let mut out = Markdown::default();
    read(body, &mut out);
    Ok(out.finish())
}

fn is_title(frame: Node<'_, '_>) -> bool {
    attribute(frame, "class") == Some("title")
}

struct Reader<'a> {
    out: &'a mut Markdown,
    notes: Vec<String>,
}

impl<'a> Reader<'a> {
    fn new(out: &'a mut Markdown) -> Self {
        Self {
            out,
            notes: Vec::new(),
        }
    }

    fn blocks(&mut self, node: Node<'_, '_>) {
        for block in elements(node) {
            match block.tag_name().name() {
                "h" => {
                    let level = attribute(block, "outline-level").and_then(|l| l.parse().ok());
                    let text = self.said(block);
                    self.out.heading(level.unwrap_or(1), &text);
                }
                "p" => {
                    let text = self.said(block);
                    self.out.paragraph(&text);
                }
                "list" => self.list(block, 0),
                "table" => {
                    let mut rows = Vec::new();
                    self.rows(block, &mut rows);
                    self.out.table(&rows);
                }
                // What was deleted, and what is there again as headings.
                "tracked-changes" | "table-of-content" | "annotation" | "notes" => {}
                _ => self.blocks(block),
            }
        }
    }

    fn list(&mut self, list: Node<'_, '_>, depth: usize) {
        for item in elements(list) {
            for block in elements(item) {
                match block.tag_name().name() {
                    "list" => self.list(block, depth + 1),
                    _ => {
                        let text = self.said(block);
                        self.out.item(depth, None, &text);
                    }
                }
            }
        }
    }

    fn rows(&mut self, table: Node<'_, '_>, rows: &mut Vec<Vec<String>>) {
        for row in elements(table) {
            match row.tag_name().name() {
                "table-row" => {
                    let mut cells = Vec::new();
                    for cell in elements(row) {
                        let mut text = String::new();
                        for block in elements(cell) {
                            text.push_str(&self.said(block));
                            text.push(' ');
                        }
                        let repeated = attribute(cell, "number-columns-repeated")
                            .and_then(|times| times.parse().ok())
                            .unwrap_or(1_usize);
                        for _ in 0..repeated.min(MAX_REPEATED) {
                            cells.push(text.clone());
                        }
                    }
                    rows.push(cells);
                }
                "table-header-rows" | "table-rows" | "table-row-group" => self.rows(row, rows),
                _ => {}
            }
        }
    }

    fn said(&mut self, node: Node<'_, '_>) -> String {
        let mut text = String::new();
        self.inline(node, &mut text);
        text
    }

    fn inline(&mut self, node: Node<'_, '_>, text: &mut String) {
        for part in node.children() {
            if part.is_text() {
                text.push_str(&part.text().unwrap_or_default().replace('\n', " "));
                continue;
            }
            match part.tag_name().name() {
                "s" | "tab" => text.push(' '),
                "line-break" => text.push('\n'),
                "a" => {
                    let inner = self.said(part);
                    match attribute(part, "href") {
                        Some(target) => text.push_str(&link(&inner, target)),
                        None => text.push_str(&inner),
                    }
                }
                "note" => {
                    if let Some(body) = child(part, "note-body") {
                        let said = self.said(body);
                        self.notes.push(said);
                        let number = self.notes.len();
                        let _ = write!(text, "[^{number}]");
                    }
                }
                "annotation" | "tracked-changes" | "desc" | "title" => {}
                // Paragraphs in a frame or a cell.
                "p" | "h" => {
                    self.inline(part, text);
                    text.push(' ');
                }
                _ => self.inline(part, text),
            }
        }
    }

    fn write_notes(&mut self) {
        for (index, note) in std::mem::take(&mut self.notes).into_iter().enumerate() {
            let note = super::markdown::squash(&note);
            self.out.paragraph(&format!("[^{}]: {note}", index + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    fn file(kind: &str, body: &str) -> Vec<u8> {
        let content = format!(
            "<office:document-content xmlns:office=\"o\" xmlns:text=\"t\" xmlns:table=\"b\" xmlns:draw=\"d\" \
             xmlns:presentation=\"p\" xmlns:xlink=\"x\"><office:body><office:{kind}>{body}</office:{kind}></office:body></office:document-content>"
        );
        zip(&[(MAIN, &content)])
    }

    #[test]
    fn texts() {
        let body = "<text:tracked-changes><text:p>gone</text:p></text:tracked-changes>\
            <text:h text:outline-level=\"2\">Både og</text:h>\
            <text:p>Se <text:a xlink:href=\"https://pi.dev\">pi</text:a><text:s text:c=\"3\"/><text:span>nu</text:span>\
            <text:note><text:note-citation>1</text:note-citation><text:note-body><text:p>En note.</text:p></text:note-body></text:note></text:p>\
            <text:list><text:list-item><text:p>en</text:p><text:list><text:list-item><text:p>to</text:p></text:list-item></text:list></text:list-item></text:list>\
            <text:section><table:table><table:table-header-rows><table:table-row><table:table-cell><text:p>A</text:p></table:table-cell>\
            <table:table-cell><text:p>B</text:p></table:table-cell></table:table-row></table:table-header-rows>\
            <table:table-row><table:table-cell><text:p>1</text:p><text:p>2</text:p></table:table-cell>\
            <table:table-cell table:number-columns-repeated=\"16384\"/></table:table-row></table:table></text:section>";
        assert_eq!(
            text(file("text", body)).unwrap(),
            "## Både og\n\nSe [pi](https://pi.dev) nu[^1]\n\n- en\n  - to\n\n\
             | A | B |\n| --- | --- |\n| 1 2 |  |\n\n[^1]: En note.\n"
        );
    }

    #[test]
    fn presentations() {
        let body = "<draw:page draw:name=\"page1\"><draw:frame presentation:class=\"title\"><draw:text-box><text:p>Både</text:p></draw:text-box></draw:frame>\
            <draw:frame><draw:text-box><text:list><text:list-item><text:p>punkt</text:p></text:list-item></text:list></draw:text-box></draw:frame>\
            <presentation:notes><draw:frame><draw:text-box><text:p>Sig hej</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>\
            <draw:page><draw:custom-shape><text:p>form</text:p></draw:custom-shape></draw:page>";
        assert_eq!(
            slides(file("presentation", body)).unwrap(),
            "## Slide 1: Både\n\n- punkt\n\nNotes: Sig hej\n\n## Slide 2\n\nform\n"
        );
    }

    #[test]
    fn files_that_are_something_else() {
        assert!(text(file("presentation", "")).is_err());
        assert!(text(zip(&[("a", "b")])).is_err());
        let protected = zip(&[
            (MAIN, "<a/>"),
            (
                "META-INF/manifest.xml",
                "<m><manifest:encryption-data/></m>",
            ),
        ]);
        assert!(text(protected).unwrap_err().contains("password"));
    }
}
