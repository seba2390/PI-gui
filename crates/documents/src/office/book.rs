//! EPUB books: their chapters, in order.

use std::collections::HashMap;

use super::html;
use super::markdown::Markdown;
use super::package::{Package, attribute, child, elements, join, parse};

pub fn read(bytes: Vec<u8>) -> Result<String, String> {
    let mut package = Package::open(bytes)?;
    let container = package.needed("META-INF/container.xml")?;
    let main = parse(&container)?
        .descendants()
        .find(|node| node.tag_name().name() == "rootfile")
        .and_then(|root| attribute(root, "full-path"))
        .ok_or("it is damaged")?
        .to_owned();
    let folder = main.rsplit_once('/').map_or("", |(folder, _)| folder);
    let text = package.needed(&main)?;
    let xml = parse(&text)?;
    let part = |name| child(xml.root_element(), name);
    let chapters: HashMap<&str, &str> = part("manifest")
        .into_iter()
        .flat_map(elements)
        .filter_map(|item| Some((attribute(item, "id")?, attribute(item, "href")?)))
        .collect();
    let mut out = Markdown::default();
    if let Some(title) = part("metadata").and_then(|about| child(about, "title")) {
        out.heading(1, title.text().unwrap_or_default());
    }
    let mut read = 0;
    for item in part("spine").into_iter().flat_map(elements) {
        let Some(href) = attribute(item, "idref").and_then(|id| chapters.get(id)) else {
            continue;
        };
        // Of a protected book, chapters are no text.
        if let Some(chapter) = package.text(&join(folder, &decoded(href)))? {
            html::write(&chapter, &mut out);
            read += 1;
        }
    }
    if read > 0 {
        Ok(out.finish())
    } else if package.has("META-INF/encryption.xml") {
        Err("it is protected against copying".into())
    } else {
        Err("there is no text in it".into())
    }
}

/// `href` with what is written as `%20` and the like.
fn decoded(href: &str) -> String {
    let mut out = Vec::new();
    let bytes = href.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let code = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match code {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    const CONTAINER: (&str, &str) = (
        "META-INF/container.xml",
        "<container><rootfiles><rootfile full-path=\"OEBPS/content.opf\"/></rootfiles></container>",
    );

    #[test]
    fn chapters_in_their_order() {
        let main = "<package xmlns:dc=\"dc\"><metadata><dc:title>Bogen</dc:title></metadata><manifest>\
            <item id=\"a\" href=\"Text/chapter%201.xhtml\"/><item id=\"b\" href=\"two.xhtml\"/><item id=\"c\" href=\"c.css\"/></manifest>\
            <spine><itemref idref=\"b\"/><itemref idref=\"a\"/><itemref idref=\"none\"/></spine></package>";
        let bytes = zip(&[
            CONTAINER,
            ("OEBPS/content.opf", main),
            (
                "OEBPS/Text/chapter 1.xhtml",
                "<html><body><p>sidst</p></body></html>",
            ),
            (
                "OEBPS/two.xhtml",
                "<html><body><h1>Først</h1><p>Det &amp; hint&eacute;.</p></body></html>",
            ),
        ]);
        assert_eq!(
            read(bytes).unwrap(),
            "# Bogen\n\n## Først\n\nDet & hinté.\n\nsidst\n"
        );
    }

    #[test]
    fn protected_and_damaged_books() {
        let main = "<package><manifest><item id=\"a\" href=\"a.xhtml\"/></manifest><spine><itemref idref=\"a\"/></spine></package>";
        let main = ("OEBPS/content.opf", main);
        assert!(
            read(zip(&[CONTAINER, main]))
                .unwrap_err()
                .contains("no text")
        );
        let protected = zip(&[CONTAINER, main, ("META-INF/encryption.xml", "<e/>")]);
        assert!(read(protected).unwrap_err().contains("protected"));
        assert!(read(zip(&[CONTAINER])).is_err());
        assert!(read(b"no".to_vec()).is_err());
        assert_eq!(decoded("a%20b%zz%"), "a b%zz%");
    }
}
