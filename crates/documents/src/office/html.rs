//! Web pages as Markdown, as they are in books.

use html5ever::tendril::TendrilSink as _;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use super::markdown::{Markdown, link};

const MAX_DEPTH: usize = 400;

pub fn write(source: &str, out: &mut Markdown) {
    let dom =
        html5ever::parse_document(RcDom::default(), html5ever::ParseOpts::default()).one(source);
    let mut writer = Writer {
        out,
        text: String::new(),
        lists: Vec::new(),
        item: false,
        pre: false,
    };
    writer.walk(&dom.document, 0);
    writer.flush();
}

struct Writer<'a> {
    out: &'a mut Markdown,
    /// Of the block that is being read.
    text: String,
    /// The lists it is in: how many items an ordered one had.
    lists: Vec<Option<usize>>,
    /// The text is the start of an item.
    item: bool,
    pre: bool,
}

impl Writer<'_> {
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.text);
        if text.trim().is_empty() {
            return;
        }
        if self.pre {
            self.out.code(&text);
        } else if std::mem::take(&mut self.item) {
            let number = match self.lists.last_mut() {
                Some(Some(count)) => {
                    *count += 1;
                    Some(*count)
                }
                _ => None,
            };
            self.out
                .item(self.lists.len().saturating_sub(1), number, &text);
        } else {
            self.out.paragraph(&text);
        }
    }

    fn walk(&mut self, node: &Handle, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match &node.data {
            NodeData::Text { contents } => {
                let contents = contents.borrow();
                if self.pre {
                    self.text.push_str(&contents);
                } else {
                    self.text.push_str(&contents.replace(['\n', '\r'], " "));
                }
            }
            NodeData::Element { name, .. } => self.element(&name.local, node, depth),
            _ => self.children(node, depth),
        }
    }

    fn children(&mut self, node: &Handle, depth: usize) {
        for child in node.children.borrow().iter() {
            self.walk(child, depth + 1);
        }
    }

    fn element(&mut self, name: &str, node: &Handle, depth: usize) {
        match name {
            "head" | "script" | "style" | "svg" | "math" | "template" | "noscript" => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.children(node, depth);
                let text = std::mem::take(&mut self.text);
                // Below the name of the book.
                let level = usize::from(name.as_bytes()[1] - b'0') + 1;
                self.out.heading(level, &text);
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.push((name == "ol").then_some(0));
                self.children(node, depth);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                self.flush();
                self.item = true;
                self.children(node, depth);
                self.flush();
                self.item = false;
            }
            "pre" => {
                self.flush();
                self.pre = true;
                self.children(node, depth);
                self.flush();
                self.pre = false;
            }
            "table" => {
                self.flush();
                let mut rows = Vec::new();
                rows_of(node, &mut rows, depth);
                self.out.table(&rows);
            }
            "a" => {
                let before = std::mem::take(&mut self.text);
                self.children(node, depth);
                let inner = std::mem::replace(&mut self.text, before);
                let target = attribute(node, "href").unwrap_or_default();
                self.text.push_str(&link(&inner, &target));
            }
            "br" => self.text.push('\n'),
            "p" | "div" | "section" | "article" | "blockquote" | "figure" | "figcaption"
            | "header" | "footer" | "aside" | "dl" | "dt" | "dd" | "body" | "hr" | "main" => {
                // The paragraph of an item is the item.
                if !self.item || !self.text.trim().is_empty() {
                    self.flush();
                }
                self.children(node, depth);
                self.flush();
            }
            _ => self.children(node, depth),
        }
    }
}

fn rows_of(node: &Handle, rows: &mut Vec<Vec<String>>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for child in node.children.borrow().iter() {
        let NodeData::Element { name, .. } = &child.data else {
            continue;
        };
        match &*name.local {
            "tr" => {
                let cells = child.children.borrow();
                let cells = cells.iter().filter(|cell| {
                    matches!(&cell.data, NodeData::Element { name, .. } if matches!(&*name.local, "td" | "th"))
                });
                rows.push(cells.map(|cell| text_of(cell, depth)).collect());
            }
            _ => rows_of(child, rows, depth + 1),
        }
    }
}

fn text_of(node: &Handle, depth: usize) -> String {
    let mut text = String::new();
    if depth > MAX_DEPTH {
        return text;
    }
    if let NodeData::Text { contents } = &node.data {
        text.push_str(&contents.borrow());
    }
    for child in node.children.borrow().iter() {
        text.push_str(&text_of(child, depth + 1));
        if matches!(&child.data, NodeData::Element { name, .. } if matches!(&*name.local, "p" | "br" | "div"))
        {
            text.push(' ');
        }
    }
    text
}

fn attribute(node: &Handle, name: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    let attrs = attrs.borrow();
    let found = attrs.iter().find(|attr| &*attr.name.local == name)?;
    Some(found.value.to_string())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn pages() {
        let mut out = Markdown::default();
        write(
            "<html><head><title>no</title></head><body><h1>One</h1><p>It&nbsp;says <a href=\"https://pi.dev\">pi</a>\n and more.</p>\
             <ol><li><p>first</p><ul><li>inner</li></ul></li><li>second</li></ol><pre>a\n  b</pre>\
             <table><thead><tr><th>A</th><th>B</th></tr></thead><tr><td><p>1</p><p>2</p></td><td>3</td></tr></table>\
             <script>x()</script>loose</body></html>",
            &mut out,
        );
        assert_eq!(
            out.finish(),
            "## One\n\nIt says [pi](https://pi.dev) and more.\n\n1. first\n  - inner\n2. second\n\n\
             ```\na\n  b\n```\n\n| A | B |\n| --- | --- |\n| 1 2 | 3 |\n\nloose\n"
        );
    }
}
