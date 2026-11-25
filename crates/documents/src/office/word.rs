//! Word files.

use std::collections::{BTreeMap, HashMap};

use roxmltree::Node;

use super::markdown::{Markdown, link};
use super::package::{Package, attribute, child, elements, parse};

const MAIN: &str = "word/document.xml";
/// How deep styles are based on others.
const STYLE_DEPTH: usize = 10;

pub fn read(bytes: Vec<u8>) -> Result<String, String> {
    let mut package = Package::open(bytes)?;
    let document = package.needed(MAIN)?;
    let links = package.relations(MAIN)?;
    let styles = match package.text("word/styles.xml")? {
        Some(text) => styles(&text)?,
        None => HashMap::new(),
    };
    let lists = match package.text("word/numbering.xml")? {
        Some(text) => ordered_lists(&text)?,
        None => HashMap::new(),
    };
    let notes = package.text("word/footnotes.xml")?;
    let ends = package.text("word/endnotes.xml")?;
    let xml = parse(&document)?;
    let body = child(xml.root_element(), "body").ok_or("there is no text in it")?;
    let mut reader = Reader {
        out: Markdown::default(),
        links,
        styles,
        lists,
        counts: BTreeMap::new(),
    };
    reader.blocks(body);
    for (part, kind) in [(notes, "footnote"), (ends, "endnote")] {
        if let Some(part) = part {
            reader.notes(&part, kind)?;
        }
    }
    Ok(reader.out.finish())
}

#[derive(Debug, Clone, Default)]
struct Style {
    /// 1 for the highest heading.
    heading: Option<usize>,
    list: Option<List>,
}

/// The id of a list, and the level in it.
#[derive(Debug, Clone, PartialEq)]
struct List {
    id: String,
    level: usize,
}

struct Reader {
    out: Markdown,
    links: HashMap<String, String>,
    styles: HashMap<String, Style>,
    /// Levels of lists with numbers; the others have bullets.
    lists: HashMap<(String, usize), bool>,
    counts: BTreeMap<(String, usize), usize>,
}

impl Reader {
    fn blocks(&mut self, node: Node<'_, '_>) {
        for block in elements(node) {
            match block.tag_name().name() {
                "p" => self.paragraph(block),
                "tbl" => {
                    let rows = self.rows(block);
                    self.out.table(&rows);
                }
                "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" => self.blocks(block),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, node: Node<'_, '_>) {
        let mut text = String::new();
        let mut boxes = Vec::new();
        self.inline(node, &mut text, &mut boxes);
        let own = child(node, "pPr");
        let style = own
            .and_then(|own| value(own, "pStyle"))
            .and_then(|id| self.styles.get(id))
            .cloned()
            .unwrap_or_default();
        let own = own.map(properties).unwrap_or_default();
        let heading = own.heading.or(style.heading);
        let list = own.list.or(style.list).filter(|list| list.id != "0");
        match (heading, list) {
            (Some(level), _) => self.out.heading(level, &text),
            (None, Some(list)) => {
                let number = self.number(&list, text.trim().is_empty());
                self.out.item(list.level, number, &text);
            }
            (None, None) => self.out.paragraph(&text),
        }
        for text_box in boxes {
            self.out.paragraph(&text_box);
        }
    }

    fn number(&mut self, list: &List, empty: bool) -> Option<usize> {
        let key = (list.id.clone(), list.level);
        // The levels below start again.
        self.counts
            .retain(|(id, level), _| *id != list.id || *level <= list.level);
        if empty || !self.lists.get(&key).copied().unwrap_or_default() {
            return None;
        }
        let count = self.counts.entry(key).or_default();
        *count += 1;
        Some(*count)
    }

    /// The text in `node`, and what is in text boxes there.
    fn inline(&self, node: Node<'_, '_>, text: &mut String, boxes: &mut Vec<String>) {
        for part in elements(node) {
            match part.tag_name().name() {
                "t" => text.push_str(part.text().unwrap_or_default()),
                "tab" | "ptab" => text.push(' '),
                "br" | "cr" => text.push('\n'),
                "noBreakHyphen" => text.push('-'),
                "footnoteReference" | "endnoteReference" => {
                    if let Some(id) = attribute(part, "id") {
                        let mark = part.tag_name().name().chars().next().unwrap_or('f');
                        text.push_str(&note(mark, id));
                    }
                }
                "hyperlink" => {
                    let mut inner = String::new();
                    self.inline(part, &mut inner, boxes);
                    let target = attribute(part, "id").and_then(|id| self.links.get(id));
                    text.push_str(&target.map_or(inner.clone(), |target| link(&inner, target)));
                }
                // What it says is there twice, for old programs as well.
                "AlternateContent" => {
                    let newest = child(part, "Choice").or_else(|| child(part, "Fallback"));
                    if let Some(newest) = newest {
                        self.inline(newest, text, boxes);
                    }
                }
                "txbxContent" => {
                    let mut lines = Vec::new();
                    for paragraph in elements(part) {
                        let mut line = String::new();
                        self.inline(paragraph, &mut line, boxes);
                        lines.push(line);
                    }
                    boxes.push(lines.join("\n"));
                }
                // Deleted, hidden in fields, or how it looks.
                "del" | "moveFrom" | "delText" | "instrText" | "pPr" | "rPr" => {}
                _ => self.inline(part, text, boxes),
            }
        }
    }

    fn rows(&self, table: Node<'_, '_>) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for row in elements(table) {
            match row.tag_name().name() {
                "tr" => rows.push(self.cells(row)),
                "sdt" | "sdtContent" | "customXml" | "ins" => rows.extend(self.rows(row)),
                _ => {}
            }
        }
        rows
    }

    fn cells(&self, row: Node<'_, '_>) -> Vec<String> {
        let mut cells = Vec::new();
        for cell in elements(row) {
            match cell.tag_name().name() {
                "tc" => {
                    let mut text = String::new();
                    let mut boxes = Vec::new();
                    for block in elements(cell) {
                        self.inline(block, &mut text, &mut boxes);
                        text.push(' ');
                    }
                    cells.push(text);
                    let span = child(cell, "tcPr")
                        .and_then(|own| value(own, "gridSpan"))
                        .and_then(|span| span.parse::<usize>().ok())
                        .unwrap_or(1);
                    cells.extend((1..span.min(64)).map(|_| String::new()));
                }
                "sdt" | "sdtContent" | "customXml" => cells.extend(self.cells(cell)),
                _ => {}
            }
        }
        cells
    }

    fn notes(&mut self, part: &str, kind: &str) -> Result<(), String> {
        let xml = parse(part)?;
        for footnote in elements(xml.root_element()) {
            // Separators have a type.
            let is_note = footnote.tag_name().name() == kind
                && attribute(footnote, "type").is_none_or(|kind| kind == "normal");
            let Some(id) = attribute(footnote, "id").filter(|_| is_note) else {
                continue;
            };
            let mut text = String::new();
            self.inline(footnote, &mut text, &mut Vec::new());
            let text = super::markdown::squash(&text);
            if !text.is_empty() {
                let mark = kind.chars().next().unwrap_or('f');
                self.out.paragraph(&format!("{}: {text}", note(mark, id)));
            }
        }
        Ok(())
    }
}

fn note(mark: char, id: &str) -> String {
    match mark {
        'e' => format!("[^e{id}]"),
        _ => format!("[^{id}]"),
    }
}

/// The `val` of the child `name`.
fn value<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    attribute(child(node, name)?, "val")
}

/// What the properties of a paragraph or a style say themselves.
fn properties(node: Node<'_, '_>) -> Style {
    let outline = value(node, "outlineLvl").and_then(|level| level.parse::<usize>().ok());
    let list = child(node, "numPr").and_then(|list| {
        Some(List {
            id: value(list, "numId")?.to_owned(),
            level: value(list, "ilvl")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0),
        })
    });
    Style {
        // Level 9 is text.
        heading: outline.filter(|level| *level < 9).map(|level| level + 1),
        list,
    }
}

fn styles(text: &str) -> Result<HashMap<String, Style>, String> {
    let xml = parse(text)?;
    let mut own = HashMap::new();
    let mut based_on = HashMap::new();
    for node in elements(xml.root_element()) {
        let Some(id) = attribute(node, "styleId").filter(|_| node.tag_name().name() == "style")
        else {
            continue;
        };
        let mut style = child(node, "pPr").map(properties).unwrap_or_default();
        // Its id is in the language of the program, its name is not.
        let name = value(node, "name").unwrap_or_default().to_ascii_lowercase();
        if let Some(level) = name.strip_prefix("heading ") {
            style.heading = level.parse().ok().or(style.heading);
        } else if name == "title" {
            style.heading = Some(1);
        }
        own.insert(id.to_owned(), style);
        if let Some(base) = value(node, "basedOn") {
            based_on.insert(id.to_owned(), base.to_owned());
        }
    }
    let mut styles = HashMap::new();
    for id in own.keys() {
        let mut style = own[id].clone();
        let mut base = based_on.get(id);
        for _ in 0..STYLE_DEPTH {
            let Some(parent) = base.and_then(|base| own.get(base)) else {
                break;
            };
            style.heading = style.heading.or(parent.heading);
            style.list = style.list.or_else(|| parent.list.clone());
            base = base.and_then(|base| based_on.get(base));
        }
        styles.insert(id.clone(), style);
    }
    Ok(styles)
}

/// Which levels of which lists have numbers.
fn ordered_lists(text: &str) -> Result<HashMap<(String, usize), bool>, String> {
    let xml = parse(text)?;
    let mut formats: HashMap<&str, Vec<(usize, bool)>> = HashMap::new();
    for node in elements(xml.root_element()) {
        if node.tag_name().name() != "abstractNum" {
            continue;
        }
        let levels = elements(node)
            .filter(|level| level.tag_name().name() == "lvl")
            .filter_map(|level| {
                let format = value(level, "numFmt").unwrap_or("decimal");
                let number = attribute(level, "ilvl")?.parse().ok()?;
                Some((number, !matches!(format, "bullet" | "none")))
            })
            .collect();
        if let Some(id) = attribute(node, "abstractNumId") {
            formats.insert(id, levels);
        }
    }
    let mut lists = HashMap::new();
    for node in elements(xml.root_element()) {
        if node.tag_name().name() != "num" {
            continue;
        }
        let Some(id) = attribute(node, "numId") else {
            continue;
        };
        for (level, ordered) in value(node, "abstractNumId")
            .and_then(|id| formats.get(id))
            .into_iter()
            .flatten()
        {
            lists.insert((id.to_owned(), *level), *ordered);
        }
    }
    Ok(lists)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    const W: &str = "xmlns:w=\"w\" xmlns:r=\"r\" xmlns:mc=\"mc\"";

    fn document(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document {W}><w:body>{body}</w:body></w:document>"
        )
    }

    fn p(properties: &str, text: &str) -> String {
        format!("<w:p><w:pPr>{properties}</w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    #[test]
    fn headings_in_any_language() {
        let styles = format!(
            "<w:styles {W}>\
             <w:style w:styleId=\"Overskrift1\"><w:name w:val=\"heading 1\"/></w:style>\
             <w:style w:styleId=\"Mine\"><w:name w:val=\"Mine\"/><w:basedOn w:val=\"Overskrift1\"/></w:style>\
             <w:style w:styleId=\"Titel\"><w:name w:val=\"Title\"/></w:style>\
             <w:style w:styleId=\"Loop\"><w:name w:val=\"a\"/><w:basedOn w:val=\"Loop\"/></w:style></w:styles>"
        );
        let body = [
            p("<w:pStyle w:val=\"Titel\"/>", "Rapport"),
            p("<w:pStyle w:val=\"Overskrift1\"/>", "Først"),
            p("<w:pStyle w:val=\"Mine\"/>", "Så"),
            p("<w:outlineLvl w:val=\"2\"/>", "Dybt"),
            p("<w:pStyle w:val=\"Loop\"/>", "Tekst &amp; mere"),
        ]
        .concat();
        let bytes = zip(&[(MAIN, &document(&body)), ("word/styles.xml", &styles)]);
        assert_eq!(
            read(bytes).unwrap(),
            "# Rapport\n\n# Først\n\n# Så\n\n### Dybt\n\nTekst & mere\n"
        );
    }

    #[test]
    fn lists_count() {
        let numbering = format!(
            "<w:numbering {W}><w:abstractNum w:abstractNumId=\"7\">\
             <w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"decimal\"/></w:lvl>\
             <w:lvl w:ilvl=\"1\"><w:numFmt w:val=\"lowerLetter\"/></w:lvl></w:abstractNum>\
             <w:abstractNum w:abstractNumId=\"8\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"bullet\"/></w:lvl></w:abstractNum>\
             <w:num w:numId=\"1\"><w:abstractNumId w:val=\"7\"/></w:num>\
             <w:num w:numId=\"2\"><w:abstractNumId w:val=\"8\"/></w:num></w:numbering>"
        );
        let item = |id: usize, level: usize, text: &str| {
            let list =
                format!("<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{id}\"/></w:numPr>");
            p(&list, text)
        };
        let body = [
            item(1, 0, "one"),
            item(1, 1, "a"),
            item(1, 1, "b"),
            item(1, 0, "two"),
            item(1, 1, "a again"),
            p("", "Between"),
            item(2, 0, "dot"),
            item(0, 0, "no list"),
        ]
        .concat();
        let bytes = zip(&[(MAIN, &document(&body)), ("word/numbering.xml", &numbering)]);
        assert_eq!(
            read(bytes).unwrap(),
            "1. one\n  1. a\n  2. b\n2. two\n  1. a again\n\nBetween\n\n- dot\n\nno list\n"
        );
    }

    #[test]
    fn what_is_in_paragraphs() {
        let rels = "<Relationships><Relationship Id=\"rId1\" Target=\"https://pi.dev\" TargetMode=\"External\"/></Relationships>";
        let body = "<w:p><w:r><w:t>See </w:t></w:r><w:hyperlink r:id=\"rId1\"><w:r><w:t>pi</w:t></w:r></w:hyperlink>\
             <w:r><w:t xml:space=\"preserve\"> now</w:t><w:footnoteReference w:id=\"2\"/><w:br/><w:t>line</w:t></w:r>\
             <w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:ins><w:r><w:t> kept</w:t></w:r></w:ins>\
             <w:r><w:instrText>PAGE</w:instrText></w:r></w:p>\
             <w:p><w:r><mc:AlternateContent><mc:Choice><w:txbxContent><w:p><w:r><w:t>boxed</w:t></w:r></w:p></w:txbxContent></mc:Choice>\
             <mc:Fallback><w:txbxContent><w:p><w:r><w:t>boxed</w:t></w:r></w:p></w:txbxContent></mc:Fallback></mc:AlternateContent></w:r></w:p>\
             <w:sdt><w:sdtContent><w:p><w:r><w:t>controlled</w:t></w:r></w:p></w:sdtContent></w:sdt>";
        let notes = format!(
            "<w:footnotes {W}><w:footnote w:id=\"0\" w:type=\"separator\"><w:p><w:r><w:t>-</w:t></w:r></w:p></w:footnote>\
             <w:footnote w:id=\"2\"><w:p><w:r><w:t>A note.</w:t></w:r></w:p></w:footnote></w:footnotes>"
        );
        let bytes = zip(&[
            (MAIN, &document(body)),
            ("word/_rels/document.xml.rels", rels),
            ("word/footnotes.xml", &notes),
        ]);
        assert_eq!(
            read(bytes).unwrap(),
            "See [pi](https://pi.dev) now[^2]\nline kept\n\nboxed\n\ncontrolled\n\n[^2]: A note.\n"
        );
    }

    #[test]
    fn tables() {
        let cell = |properties: &str, text: &str| {
            format!("<w:tc><w:tcPr>{properties}</w:tcPr>{}</w:tc>", p("", text))
        };
        let body = format!(
            "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
            cell("", "Name"),
            cell("", "Price"),
            cell("<w:gridSpan w:val=\"2\"/>", "Boats"),
            cell("", "Dinghy"),
            cell("", "400"),
        );
        assert_eq!(
            read(zip(&[(MAIN, &document(&body))])).unwrap(),
            "| Name | Price |\n| --- | --- |\n| Boats |  |\n| Dinghy | 400 |\n"
        );
    }

    #[test]
    fn files_that_are_something_else() {
        for bytes in [
            b"no".to_vec(),
            zip(&[("a.txt", "x")]),
            zip(&[(MAIN, "<w:broken")]),
            zip(&[(MAIN, "<a/>")]),
        ] {
            assert!(read(bytes).is_err());
        }
    }
}
