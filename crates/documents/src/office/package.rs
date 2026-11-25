//! Files that are a zip archive of XML.

use std::collections::HashMap;
use std::io::{Cursor, Read as _};

use roxmltree::{Document, Node, ParsingOptions};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::plain;

/// Of one part, unpacked: more is a trick, or no text.
const MAX_PART: u64 = 256 * 1024 * 1024;
/// Of all parts that are read.
const MAX_ALL: u64 = 1024 * 1024 * 1024;

pub struct Package {
    zip: ZipArchive<Cursor<Vec<u8>>>,
    left: u64,
}

impl Package {
    pub fn open(bytes: Vec<u8>) -> Result<Self, String> {
        let zip = ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| "it is damaged, or not what its name says")?;
        Ok(Self { zip, left: MAX_ALL })
    }

    pub fn has(&self, name: &str) -> bool {
        self.zip.index_for_name(name).is_some()
    }

    /// The text of the part `name`, if there is one.
    pub fn text(&mut self, name: &str) -> Result<Option<String>, String> {
        let part = match self.zip.by_name(name) {
            Ok(part) => part,
            Err(ZipError::FileNotFound) => return Ok(None),
            Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED)) => {
                return Err("it is protected with a password".into());
            }
            Err(_) => return Err("it is damaged".into()),
        };
        let max = MAX_PART.min(self.left);
        let mut bytes = Vec::new();
        part.take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "it is damaged")?;
        let len = bytes.len() as u64;
        if len > max {
            return Err("there is too much in it".into());
        }
        self.left -= len;
        Ok(plain::decode(bytes))
    }

    /// The text of the part `name`, which has to be there.
    pub fn needed(&mut self, name: &str) -> Result<String, String> {
        self.text(name)?
            .ok_or_else(|| "it is damaged, or not what its name says".into())
    }

    /// Where the parts and addresses that the part `name` refers to are, by their ids.
    pub fn relations(&mut self, name: &str) -> Result<HashMap<String, String>, String> {
        let (folder, file) = name.rsplit_once('/').unwrap_or(("", name));
        let rels = if folder.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{folder}/_rels/{file}.rels")
        };
        let Some(text) = self.text(&rels)? else {
            return Ok(HashMap::new());
        };
        let xml = parse(&text)?;
        Ok(elements(xml.root_element())
            .filter_map(|relation| {
                let target = attribute(relation, "Target")?;
                let target = if attribute(relation, "TargetMode") == Some("External") {
                    target.to_owned()
                } else {
                    join(folder, target)
                };
                Some((attribute(relation, "Id")?.to_owned(), target))
            })
            .collect())
    }
}

/// `text` as XML. Definitions of entities, which could grow without end, are refused.
pub fn parse(text: &str) -> Result<Document<'_>, String> {
    let options = ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options).map_err(|_| "it is damaged".into())
}

pub fn elements<'a, 'i>(node: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(Node::is_element)
}

/// The child of `node` called `name`, whatever its namespace.
pub fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    elements(node).find(|child| child.tag_name().name() == name)
}

/// The value of the attribute called `name`, whatever its namespace.
pub fn attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

/// The part that `target` is, as the parts in `folder` say it.
pub fn join(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = match target.strip_prefix('/') {
        Some(_) => Vec::new(),
        None => folder.split('/').filter(|part| !part.is_empty()).collect(),
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    #[test]
    fn parts_and_what_they_refer_to() {
        let rels = "<Relationships><Relationship Id=\"a\" Target=\"../x.xml\"/>\
                    <Relationship Id=\"b\" Target=\"https://pi.dev\" TargetMode=\"External\"/>\
                    <Relationship Id=\"c\" Target=\"/word/y.xml\"/></Relationships>";
        let bytes = zip(&[
            ("word/document.xml", "<a/>"),
            ("word/_rels/document.xml.rels", rels),
        ]);
        let mut package = Package::open(bytes).unwrap();
        assert!(package.has("word/document.xml") && !package.has("x"));
        assert_eq!(package.text("word/document.xml").unwrap().unwrap(), "<a/>");
        assert_eq!(package.text("none").unwrap(), None);
        assert!(package.needed("none").is_err());
        let relations = package.relations("word/document.xml").unwrap();
        assert_eq!(relations["a"], "x.xml");
        assert_eq!(relations["b"], "https://pi.dev");
        assert_eq!(relations["c"], "word/y.xml");
        assert!(package.relations("other.xml").unwrap().is_empty());
        assert!(Package::open(b"PK no".to_vec()).is_err());
    }

    #[test]
    fn entities_do_not_grow() {
        let bomb = "<!DOCTYPE a [<!ENTITY b \"bbbb\"><!ENTITY c \"&b;&b;\">]><a>&c;</a>";
        assert!(parse(bomb).is_err());
    }
}
