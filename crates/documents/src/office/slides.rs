//! PowerPoint files.

use roxmltree::Node;

use super::markdown::{Markdown, link, squash};
use super::package::{Package, attribute, child, elements, parse};
use std::collections::HashMap;

const MAIN: &str = "ppt/presentation.xml";

pub fn read(bytes: Vec<u8>) -> Result<String, String> {
    let mut package = Package::open(bytes)?;
    let presentation = package.needed(MAIN)?;
    let relations = package.relations(MAIN)?;
    let xml = parse(&presentation)?;
    let list = child(xml.root_element(), "sldIdLst").ok_or("there are no slides in it")?;
    let mut out = Markdown::default();
    for (index, slide) in elements(list).enumerate() {
        // It has an id of its own as well.
        let part = slide
            .attributes()
            .filter(|a| a.name() == "id")
            .find_map(|a| relations.get(a.value()));
        if let Some(part) = part {
            slide_text(&mut package, part, index + 1, &mut out)?;
        }
    }
    Ok(out.finish())
}

fn slide_text(
    package: &mut Package,
    part: &str,
    number: usize,
    out: &mut Markdown,
) -> Result<(), String> {
    let Some(text) = package.text(part)? else {
        return Ok(());
    };
    let relations = package.relations(part)?;
    let xml = parse(&text)?;
    // Hidden slides are not shown.
    if attribute(xml.root_element(), "show") == Some("0") {
        return Ok(());
    }
    let shapes = tree(xml.root_element());
    let mut title = String::new();
    if let Some(shapes) = shapes {
        find_title(shapes, &relations, &mut title);
    }
    out.heading(2, &heading(number, &title));
    if let Some(shapes) = shapes {
        write_shapes(shapes, &relations, out);
    }
    let notes = relations.values().find(|part| part.contains("notesSlide"));
    let Some(notes) = notes.map(|part| package.text(part)).transpose()?.flatten() else {
        return Ok(());
    };
    let xml = parse(&notes)?;
    let mut said = Vec::new();
    for shape in tree(xml.root_element()).into_iter().flat_map(elements) {
        if placeholder(shape) == Some("body") {
            said.extend(paragraphs(shape, &HashMap::new()).into_iter().map(|p| p.1));
        }
    }
    let said = squash(&said.join(" "));
    if !said.is_empty() {
        out.paragraph(&format!("Notes: {said}"));
    }
    Ok(())
}

pub fn heading(number: usize, title: &str) -> String {
    match squash(title) {
        title if title.is_empty() => format!("Slide {number}"),
        title => format!("Slide {number}: {title}"),
    }
}

fn tree<'a, 'i>(slide: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    child(child(slide, "cSld")?, "spTree")
}

/// What kind of placeholder of the layout `shape` fills.
fn placeholder<'a>(shape: Node<'a, '_>) -> Option<&'a str> {
    let place = child(child(child(shape, "nvSpPr")?, "nvPr")?, "ph")?;
    Some(attribute(place, "type").unwrap_or("body"))
}

fn is_title(shape: Node<'_, '_>) -> bool {
    matches!(placeholder(shape), Some("title" | "ctrTitle"))
}

fn find_title(shapes: Node<'_, '_>, links: &HashMap<String, String>, title: &mut String) {
    for shape in elements(shapes).filter(|shape| is_title(*shape)) {
        for (_, text) in paragraphs(shape, links) {
            title.push_str(&text);
            title.push(' ');
        }
    }
}

fn write_shapes(shapes: Node<'_, '_>, links: &HashMap<String, String>, out: &mut Markdown) {
    for shape in elements(shapes) {
        match shape.tag_name().name() {
            "sp" if !is_title(shape) => {
                // The number of the slide and the like are on every one.
                if matches!(placeholder(shape), Some("sldNum" | "dt" | "ftr")) {
                    continue;
                }
                let paragraphs = paragraphs(shape, links);
                let alone = paragraphs.len() < 2 && placeholder(shape).is_none();
                for (level, text) in paragraphs {
                    if alone {
                        out.paragraph(&text);
                    } else {
                        out.item(level, None, &text);
                    }
                }
            }
            "grpSp" => write_shapes(shape, links, out),
            "graphicFrame" => {
                for table in shape.descendants().filter(|n| n.tag_name().name() == "tbl") {
                    out.table(&rows(table, links));
                }
            }
            "AlternateContent" => {
                if let Some(newest) = child(shape, "Choice").or_else(|| child(shape, "Fallback")) {
                    write_shapes(newest, links, out);
                }
            }
            _ => {}
        }
    }
}

/// The paragraphs in a shape or cell, each with its level.
fn paragraphs(shape: Node<'_, '_>, links: &HashMap<String, String>) -> Vec<(usize, String)> {
    let Some(body) = child(shape, "txBody") else {
        return Vec::new();
    };
    elements(body)
        .filter(|node| node.tag_name().name() == "p")
        .map(|paragraph| {
            let level = child(paragraph, "pPr")
                .and_then(|own| attribute(own, "lvl")?.parse().ok())
                .unwrap_or(0);
            let mut text = String::new();
            for run in elements(paragraph) {
                match run.tag_name().name() {
                    "r" | "fld" => {
                        let said = child(run, "t").and_then(|t| t.text()).unwrap_or_default();
                        let target = child(run, "rPr")
                            .and_then(|own| child(own, "hlinkClick"))
                            .and_then(|click| links.get(attribute(click, "id")?));
                        match target {
                            Some(target) => text.push_str(&link(said, target)),
                            None => text.push_str(said),
                        }
                    }
                    "br" => text.push(' '),
                    _ => {}
                }
            }
            (level, text)
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .collect()
}

fn rows(table: Node<'_, '_>, links: &HashMap<String, String>) -> Vec<Vec<String>> {
    elements(table)
        .filter(|row| row.tag_name().name() == "tr")
        .map(|row| {
            elements(row)
                .filter(|cell| cell.tag_name().name() == "tc")
                .map(|cell| {
                    let said: Vec<_> = paragraphs(cell, links).into_iter().map(|p| p.1).collect();
                    said.join(" ")
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    fn shape(placeholder: &str, paragraphs: &str) -> String {
        format!(
            "<p:sp><p:nvSpPr><p:nvPr>{placeholder}</p:nvPr></p:nvSpPr><p:txBody>{paragraphs}</p:txBody></p:sp>"
        )
    }

    fn p(text: &str) -> String {
        format!("<a:p><a:r><a:t>{text}</a:t></a:r></a:p>")
    }

    fn slide(attributes: &str, shapes: &str) -> String {
        format!(
            "<p:sld xmlns:p=\"p\" xmlns:a=\"a\" xmlns:r=\"r\" {attributes}><p:cSld><p:spTree>{shapes}</p:spTree></p:cSld></p:sld>"
        )
    }

    #[test]
    fn slides_in_their_order() {
        let main = "<p:presentation xmlns:p=\"p\" xmlns:r=\"r\"><p:sldIdLst>\
            <p:sldId id=\"256\" r:id=\"rId2\"/><p:sldId id=\"257\" r:id=\"rId1\"/><p:sldId id=\"258\" r:id=\"rId3\"/>\
            </p:sldIdLst></p:presentation>";
        let rels = "<Relationships><Relationship Id=\"rId1\" Target=\"slides/slide1.xml\"/>\
            <Relationship Id=\"rId2\" Target=\"slides/slide2.xml\"/>\
            <Relationship Id=\"rId3\" Target=\"slides/slide3.xml\"/></Relationships>";
        let first = slide(
            "",
            &[
                shape("<p:ph type=\"sldNum\"/>", &p("1")),
                shape(
                    "<p:ph idx=\"1\"/>",
                    &format!(
                        "{}<a:p><a:pPr lvl=\"1\"/><a:r><a:t>below</a:t></a:r></a:p>",
                        p("point")
                    ),
                ),
                shape("<p:ph type=\"ctrTitle\"/>", &p("The boats")),
                shape("", &p("A remark")),
            ]
            .concat(),
        );
        let cell = |text: &str| format!("<a:tc><a:txBody>{}</a:txBody></a:tc>", p(text));
        let table = format!(
            "<p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr>{}{}</a:tr><a:tr>{}{}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
            cell("A"),
            cell("B"),
            cell("1"),
            cell("2")
        );
        let second = slide(
            "",
            &format!("<p:grpSp>{}</p:grpSp>{table}", shape("", &p("grouped"))),
        );
        let notes = slide("", &shape("<p:ph type=\"body\"/>", &p("Say hello")));
        let slide_rels = "<Relationships><Relationship Id=\"rId1\" Target=\"../notesSlides/notesSlide1.xml\"/></Relationships>";
        let hidden = slide("show=\"0\"", &shape("", &p("hidden")));
        let bytes = zip(&[
            (MAIN, main),
            ("ppt/_rels/presentation.xml.rels", rels),
            ("ppt/slides/slide1.xml", &second),
            ("ppt/slides/slide2.xml", &first),
            ("ppt/slides/slide3.xml", &hidden),
            ("ppt/slides/_rels/slide2.xml.rels", slide_rels),
            ("ppt/notesSlides/notesSlide1.xml", &notes),
        ]);
        assert_eq!(
            read(bytes).unwrap(),
            "## Slide 1: The boats\n\n- point\n  - below\n\nA remark\n\nNotes: Say hello\n\n\
             ## Slide 2\n\ngrouped\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n"
        );
    }

    #[test]
    fn files_that_are_something_else() {
        assert!(read(zip(&[("a", "b")])).is_err());
        assert!(read(zip(&[(MAIN, "<a/>")])).is_err());
    }
}
