//! Readable text from web pages: the main content of an HTML page as Markdown-like blocks
//! (headings, paragraphs, list items, quotes, code), without navigation, scripts, cookie
//! banners and similar boilerplate.

use html5ever::tendril::TendrilSink as _;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading(u8),
    Paragraph,
    ListItem,
    Quote,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub description: Option<String>,
    pub published: Option<String>,
    pub blocks: Vec<Block>,
}

/// Elements whose content is never page text.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "nav", "footer", "aside",
    "form", "button", "select", "input", "textarea", "dialog", "menu", "object", "embed", "video",
    "audio", "map", "head",
];

/// `class`/`id` words that mark boilerplate containers.
const BOILERPLATE: &[&str] = &[
    "nav",
    "navbar",
    "navigation",
    "menu",
    "footer",
    "sidebar",
    "cookie",
    "cookies",
    "consent",
    "banner",
    "advert",
    "advertisement",
    "ads",
    "promo",
    "share",
    "sharing",
    "social",
    "related",
    "recommended",
    "comments",
    "newsletter",
    "subscribe",
    "breadcrumb",
    "breadcrumbs",
    "popup",
    "modal",
    "toolbar",
    "masthead",
    "skip",
];

const BOILERPLATE_ROLES: &[&str] = &[
    "navigation",
    "banner",
    "contentinfo",
    "complementary",
    "search",
    "dialog",
    "alertdialog",
    "menu",
    "menubar",
];

/// Elements that end the current block.
const BLOCK_ELEMENTS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "figure",
    "figcaption",
    "table",
    "thead",
    "tbody",
    "tr",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "details",
    "summary",
    "address",
    "hr",
    "center",
];

/// Main content shorter than this is not trusted; the whole body is used instead.
const MIN_MAIN_CHARS: usize = 400;
const MAX_DEPTH: usize = 400;

pub fn html(source: &str) -> Page {
    let dom =
        html5ever::parse_document(RcDom::default(), html5ever::ParseOpts::default()).one(source);
    let mut meta = Meta::default();
    collect_meta(&dom.document, &mut meta, 0);

    let body = find(&dom.document, &|node| element_name(node) == Some("body"), 0)
        .unwrap_or_else(|| dom.document.clone());
    let root = main_content(&body).unwrap_or(body);
    let root_is_main = element_name(&root) != Some("body");

    let mut extractor = Extractor {
        blocks: Vec::new(),
        text: String::new(),
        link_chars: 0,
        kind: BlockKind::Paragraph,
        in_pre: false,
        in_link: false,
        quote_depth: 0,
        list_depth: 0,
        cell_depth: 0,
        keep_header: root_is_main,
    };
    extractor.walk(&root, 0);
    extractor.flush();

    let title = meta
        .og_title
        .or(meta.title)
        .or_else(|| {
            extractor.blocks.iter().find_map(|block| match block.kind {
                BlockKind::Heading(1) => Some(block.text.clone()),
                _ => None,
            })
        })
        .unwrap_or_default();
    Page {
        title,
        description: meta.description,
        published: meta.published,
        blocks: extractor.blocks,
    }
}

/// Plain text (or Markdown): one block per paragraph.
pub fn plain(text: &str) -> Page {
    let blocks = text
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| match heading_level(paragraph) {
            Some(level) => Block {
                kind: BlockKind::Heading(level),
                text: paragraph.trim_start_matches('#').trim().to_owned(),
            },
            None => Block {
                kind: BlockKind::Paragraph,
                text: paragraph.to_owned(),
            },
        })
        .collect();
    Page {
        blocks,
        ..Page::default()
    }
}

fn heading_level(line: &str) -> Option<u8> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ') && !line.contains('\n'))
        .then(|| u8::try_from(hashes).unwrap_or(6))
}

/// Collapses whitespace runs to single spaces.
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text of an HTML fragment (search snippets with `<b>` highlights), entities decoded.
pub fn fragment_text(source: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in source.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    squash(&decode_entities(&text))
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Meta {
    title: Option<String>,
    og_title: Option<String>,
    description: Option<String>,
    published: Option<String>,
}

fn collect_meta(node: &Handle, meta: &mut Meta, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match element_name(node) {
        Some("title") if meta.title.is_none() => {
            meta.title = Some(squash(&text_of(node))).filter(|title| !title.is_empty());
        }
        Some("meta") => {
            let key = attr(node, "property")
                .or_else(|| attr(node, "name"))
                .or_else(|| attr(node, "itemprop"))
                .map(|key| key.to_ascii_lowercase());
            let content = attr(node, "content").map(|content| squash(&content));
            if let (Some(key), Some(content)) = (key, content.filter(|c| !c.is_empty())) {
                let slot = match key.as_str() {
                    "og:title" => Some(&mut meta.og_title),
                    "description" | "og:description" => Some(&mut meta.description),
                    "article:published_time"
                    | "datepublished"
                    | "date"
                    | "dc.date"
                    | "article:modified_time"
                    | "datemodified" => Some(&mut meta.published),
                    _ => None,
                };
                if let Some(slot) = slot {
                    slot.get_or_insert(content);
                }
            }
        }
        Some("time") if meta.published.is_none() => {
            meta.published = attr(node, "datetime");
        }
        _ => {}
    }
    for child in node.children.borrow().iter() {
        collect_meta(child, meta, depth + 1);
    }
}

/// The largest `<article>`/`<main>`/`role=main` element, if it holds enough text.
fn main_content(body: &Handle) -> Option<Handle> {
    let mut candidates = Vec::new();
    collect_candidates(body, &mut candidates, 0);
    candidates
        .into_iter()
        .map(|node| (visible_len(&node, 0), node))
        .filter(|(len, _)| *len >= MIN_MAIN_CHARS)
        .max_by_key(|(len, _)| *len)
        .map(|(_, node)| node)
}

fn collect_candidates(node: &Handle, out: &mut Vec<Handle>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let is_candidate = matches!(element_name(node), Some("article" | "main"))
        || attr(node, "role").as_deref() == Some("main");
    if is_candidate {
        out.push(node.clone());
    }
    for child in node.children.borrow().iter() {
        collect_candidates(child, out, depth + 1);
    }
}

fn visible_len(node: &Handle, depth: usize) -> usize {
    if depth > MAX_DEPTH {
        return 0;
    }
    match &node.data {
        NodeData::Text { contents } => contents.borrow().trim().len(),
        NodeData::Element { .. } if is_skipped(node, true) => 0,
        _ => node
            .children
            .borrow()
            .iter()
            .map(|child| visible_len(child, depth + 1))
            .sum(),
    }
}

fn find(node: &Handle, predicate: &dyn Fn(&Handle) -> bool, depth: usize) -> Option<Handle> {
    if predicate(node) {
        return Some(node.clone());
    }
    if depth > MAX_DEPTH {
        return None;
    }
    node.children
        .borrow()
        .iter()
        .find_map(|child| find(child, predicate, depth + 1))
}

fn element_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(&name.local),
        _ => None,
    }
}

fn attr(node: &Handle, key: &str) -> Option<String> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|attribute| &*attribute.name.local == key)
            .map(|attribute| attribute.value.to_string()),
        _ => None,
    }
}

fn text_of(node: &Handle) -> String {
    let mut text = String::new();
    append_text(node, &mut text, 0);
    text
}

fn append_text(node: &Handle, out: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if let NodeData::Text { contents } = &node.data {
        out.push_str(&contents.borrow());
    }
    for child in node.children.borrow().iter() {
        append_text(child, out, depth + 1);
    }
}

/// Whether an element (and everything in it) is boilerplate.
fn is_skipped(node: &Handle, keep_header: bool) -> bool {
    let Some(name) = element_name(node) else {
        return false;
    };
    if SKIPPED.contains(&name) || (name == "header" && !keep_header) {
        return true;
    }
    if matches!(name, "html" | "body" | "main" | "article") {
        return false;
    }
    if attr(node, "hidden").is_some() || attr(node, "aria-hidden").as_deref() == Some("true") {
        return true;
    }
    if let Some(style) = attr(node, "style") {
        let style = style.replace(' ', "").to_ascii_lowercase();
        if style.contains("display:none") || style.contains("visibility:hidden") {
            return true;
        }
    }
    if attr(node, "role").is_some_and(|role| BOILERPLATE_ROLES.contains(&role.as_str())) {
        return true;
    }
    ["class", "id"].iter().any(|key| {
        attr(node, key).is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| BOILERPLATE.contains(&word))
        })
    })
}

struct Extractor {
    blocks: Vec<Block>,
    text: String,
    /// Characters of the current block that are link text (to drop link lists).
    link_chars: usize,
    kind: BlockKind,
    in_pre: bool,
    in_link: bool,
    quote_depth: usize,
    list_depth: usize,
    /// Inside a table cell, blocks run together so a row stays on one line.
    cell_depth: usize,
    keep_header: bool,
}

impl Extractor {
    fn walk(&mut self, node: &Handle, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match &node.data {
            NodeData::Text { contents } => {
                let contents = contents.borrow();
                if self.in_link {
                    self.link_chars += contents.trim().len();
                }
                self.text.push_str(&contents);
            }
            NodeData::Element { .. } => self.element(node, depth),
            NodeData::Document => self.children(node, depth),
            _ => {}
        }
    }

    fn children(&mut self, node: &Handle, depth: usize) {
        for child in node.children.borrow().iter() {
            self.walk(child, depth + 1);
        }
    }

    fn element(&mut self, node: &Handle, depth: usize) {
        if is_skipped(node, self.keep_header) {
            return;
        }
        let name = element_name(node).unwrap_or_default();
        // Documentation generators' permalink anchors (`#`, `¶`).
        if name == "a" && attr(node, "class").is_some_and(|class| class.contains("headerlink")) {
            return;
        }
        if self.cell_depth > 0 && name != "td" && name != "th" && name != "table" {
            self.text.push(' ');
            self.children(node, depth);
            self.text.push(' ');
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = name[1..].parse().unwrap_or(2);
                self.kind = BlockKind::Heading(level);
                self.children(node, depth);
                self.flush();
            }
            "li" => {
                self.flush();
                self.list_depth += 1;
                self.kind = BlockKind::ListItem;
                self.children(node, depth);
                self.flush();
                self.list_depth -= 1;
            }
            "pre" => {
                self.flush();
                self.in_pre = true;
                self.kind = BlockKind::Code;
                self.children(node, depth);
                self.flush();
                self.in_pre = false;
            }
            "blockquote" => {
                self.flush();
                self.quote_depth += 1;
                self.children(node, depth);
                self.flush();
                self.quote_depth -= 1;
            }
            "br" => self.text.push('\n'),
            "td" | "th" => {
                self.text.push_str(" | ");
                self.cell_depth += 1;
                self.children(node, depth);
                self.cell_depth -= 1;
            }
            "table" => {
                // A table nested in a cell starts afresh.
                let cells = std::mem::take(&mut self.cell_depth);
                self.flush();
                self.children(node, depth);
                self.flush();
                self.cell_depth = cells;
            }
            "a" => {
                let was_in_link = self.in_link;
                self.in_link = true;
                self.children(node, depth);
                self.in_link = was_in_link;
            }
            "img" => {}
            _ if BLOCK_ELEMENTS.contains(&name) => {
                self.flush();
                self.children(node, depth);
                self.flush();
            }
            _ => self.children(node, depth),
        }
    }

    /// Ends the current block, keeping it unless it is empty or mostly links.
    fn flush(&mut self) {
        let raw = std::mem::take(&mut self.text);
        let link_chars = std::mem::take(&mut self.link_chars);
        let next_kind = self.default_kind();
        let kind = std::mem::replace(&mut self.kind, next_kind);
        let text = match kind {
            BlockKind::Code => raw.trim_matches('\n').trim_end().to_owned(),
            // Documentation generators end headings with a permalink sign.
            BlockKind::Heading(_) => squash(&raw)
                .trim_end_matches(['#', '¶', '§', ' '])
                .to_owned(),
            _ => squash(raw.trim_start_matches([' ', '|']))
                .trim_end_matches([' ', '|'])
                .to_owned(),
        };
        if text.is_empty() {
            return;
        }
        let len = text.len();
        let mostly_links = link_chars * 10 > len * 6 && len < 400;
        if mostly_links && !matches!(kind, BlockKind::Heading(_)) {
            return;
        }
        self.blocks.push(Block { kind, text });
    }

    fn default_kind(&self) -> BlockKind {
        if self.in_pre {
            BlockKind::Code
        } else if self.list_depth > 0 {
            BlockKind::ListItem
        } else if self.quote_depth > 0 {
            BlockKind::Quote
        } else {
            BlockKind::Paragraph
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const PAGE: &str = r##"<!doctype html><html><head>
        <title>Site | Fallback</title>
        <meta property="og:title" content="Rust 2024 released">
        <meta name="description" content="What is new.">
        <meta property="article:published_time" content="2025-02-20">
        <script>var tracking = 1;</script></head>
      <body>
        <header class="site-header"><a href="/">Home</a> <a href="/blog">Blog</a></header>
        <nav><ul><li><a href="/a">A</a></li></ul></nav>
        <div class="cookie-banner">We use cookies</div>
        <main>
          <article>
            <header><h1>Rust 2024 released</h1></header>
            <p>The <b>Rust</b> team is happy to announce a new edition. It brings several
               changes to the language and the standard library that make code clearer.</p>
            <h2>What's in it<a class="headerlink" href="#w">#</a></h2>
            <ul><li>RPIT lifetime capture rules</li><li>Never type fallback</li></ul>
            <blockquote><p>Editions are opt-in.</p></blockquote>
            <pre><code>fn main() {
    println!("hi");
}</code></pre>
            <p><a href="/x">Related post one</a> <a href="/y">Related post two</a></p>
            <table><tr><th><p>Edition</p></th><th>Year</th></tr><tr><td><p>2021</p></td><td>2021</td></tr></table>
            <p style="display: none">hidden text</p>
            <p>More filler text so the article is long enough to be chosen as the main content of
               this page rather than the whole body, which also includes navigation and banners
               that should never reach the model when an article is available on the page.</p>
          </article>
        </main>
        <footer>Copyright</footer>
      </body></html>"##;

    #[test]
    fn extracts_article_without_boilerplate() {
        let page = html(PAGE);
        assert_eq!(page.title, "Rust 2024 released");
        assert_eq!(page.description.as_deref(), Some("What is new."));
        assert_eq!(page.published.as_deref(), Some("2025-02-20"));
        let blocks: Vec<(BlockKind, &str)> = page
            .blocks
            .iter()
            .map(|block| (block.kind.clone(), block.text.as_str()))
            .collect();
        assert_eq!(blocks[0], (BlockKind::Heading(1), "Rust 2024 released"));
        assert!(blocks[1].1.starts_with("The Rust team is happy"));
        assert_eq!(blocks[2], (BlockKind::Heading(2), "What's in it"));
        assert_eq!(
            blocks[3],
            (BlockKind::ListItem, "RPIT lifetime capture rules")
        );
        assert_eq!(blocks[5], (BlockKind::Quote, "Editions are opt-in."));
        assert_eq!(blocks[6].0, BlockKind::Code);
        assert!(blocks[6].1.contains("    println!(\"hi\");"));
        assert_eq!(blocks[7].1, "Edition | Year");
        assert_eq!(blocks[8].1, "2021 | 2021");
        let all = page
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for boilerplate in [
            "Home",
            "cookies",
            "tracking",
            "Copyright",
            "Related post",
            "hidden text",
        ] {
            assert!(!all.contains(boilerplate), "kept {boilerplate:?}");
        }
    }

    #[test]
    fn falls_back_to_body_and_decodes_snippets() {
        let page = html("<html><body><h1>Hi</h1><p>Short page.</p></body></html>");
        assert_eq!(page.title, "Hi");
        assert_eq!(page.blocks.len(), 2);
        assert_eq!(
            fragment_text("The <strong>Rust</strong> &amp; C&#39;s   &#x2014; book"),
            "The Rust & C's \u{2014} book"
        );
    }
}
