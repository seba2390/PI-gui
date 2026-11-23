//! Links to the session's files in the model's answers: a file name or path the answer
//! mentions (`fib.rs` in backticks, or src/main.rs:12 in the text) opens in the Files tab when
//! it is a file in the session's folder.
//!
//! Display only: the links are added to the markdown the chat draws, never to the transcript,
//! so copying, the session file and pi itself keep the model's exact text. A name is linked
//! only when it points at one file: an exact path, or a name or path ending that exactly one
//! file in the folder has.
//!
//! Each finished text block is parsed once ([`candidates`]); resolving its candidates against
//! a new file list ([`FileIndex::resolve`]) is a few hash lookups.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use ::markdown::mdast::Node as MdNode;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The scheme of the links this module makes.
const SCHEME: &str = "pistachio-file:";

/// Something in an answer that may name a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Where it is in the block's markdown source (backticks included).
    pub range: Range<usize>,
    pub path: String,
    pub line: Option<u32>,
}

/// The session folder's files, by name, for resolving candidates.
#[derive(Default)]
pub struct FileIndex {
    files: Arc<Vec<String>>,
    /// File name → indices into `files` of the files with that name.
    by_name: HashMap<String, Vec<usize>>,
}

impl FileIndex {
    pub fn new(files: Arc<Vec<String>>) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            let name = file.rsplit('/').next().unwrap_or(file);
            by_name.entry(name.to_owned()).or_default().push(index);
        }
        Self { files, by_name }
    }

    /// Whether this index was built from `files`.
    pub fn is_for(&self, files: &Arc<Vec<String>>) -> bool {
        Arc::ptr_eq(&self.files, files)
    }

    /// The file (relative to the folder `root`) that `path` names, if exactly one does.
    pub fn resolve(&self, path: &str, root: &Path) -> Option<&str> {
        let mut path = path.trim_start_matches("./");
        let root = root.to_string_lossy();
        let root = root.trim_end_matches('/');
        if path.starts_with('/') {
            path = path.strip_prefix(root)?.strip_prefix('/')?;
        }
        if path.is_empty() || path.ends_with('/') || path.split('/').any(|part| part == "..") {
            return None;
        }
        let name = path.rsplit('/').next().unwrap_or(path);
        let mut matches = self.by_name.get(name)?.iter().filter_map(|&index| {
            let file = self.files[index].as_str();
            (file == path
                || file
                    .strip_suffix(path)
                    .is_some_and(|rest| rest.ends_with('/')))
            .then_some(file)
        });
        let first = matches.next()?;
        // An exact path wins over files that merely end the same way.
        if first == path {
            return Some(first);
        }
        match matches.next() {
            Some(other) if other == path => Some(other),
            Some(_) => None,
            None => Some(first),
        }
    }
}

/// The places in `markdown` that may name a file: inline code, and path-like words in the
/// text (with a `/` or an extension). Code blocks, links and HTML are left alone.
pub fn candidates(markdown: &str) -> Vec<Candidate> {
    let Ok(tree) = ::markdown::to_mdast(markdown, &::markdown::ParseOptions::gfm()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    collect(&tree, markdown, &mut found);
    found.sort_by_key(|candidate| candidate.range.start);
    found
}

fn collect(node: &::markdown::mdast::Node, source: &str, found: &mut Vec<Candidate>) {
    use ::markdown::mdast::Node;
    let range = node
        .position()
        .map(|position| position.start.offset..position.end.offset)
        .filter(|range| source.get(range.clone()).is_some());
    match node {
        // Already links, or not prose.
        Node::Link(_)
        | Node::LinkReference(_)
        | Node::Image(_)
        | Node::ImageReference(_)
        | Node::Code(_)
        | Node::Html(_)
        | Node::Definition(_)
        | Node::InlineMath(_)
        | Node::Math(_) => {}
        Node::InlineCode(code) => {
            if let Some(range) = range
                && let Some((path, line)) = split_line(code.value.trim())
                && !path.chars().any(char::is_whitespace)
            {
                found.push(Candidate {
                    range,
                    path: path.to_owned(),
                    line,
                });
            }
        }
        Node::Text(text) => {
            // Only text whose source is the text itself (no escapes or entities), so the
            // offsets of its words are right.
            if let Some(range) = range
                && source[range.clone()] == text.value
            {
                words(&text.value, range.start, found);
            }
        }
        _ => {
            for child in node.children().into_iter().flatten() {
                collect(child, source, found);
            }
        }
    }
}

/// Path-like words of a text that starts at `offset` in the source.
fn words(text: &str, offset: usize, found: &mut Vec<Candidate>) {
    let mut start = None;
    for (index, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if !c.is_whitespace() {
            start.get_or_insert(index);
            continue;
        }
        let Some(begin) = start.take() else { continue };
        let word = &text[begin..index];
        // Not the punctuation around it: (src/main.rs), "fib.rs", fib.rs.
        let trimmed_start = word.trim_start_matches(['(', '[', '{', '<', '"', '\'', '*', '_']);
        let trimmed = trimmed_start.trim_end_matches([
            '(', ')', ']', '}', '>', '"', '\'', ',', ';', '!', '?', '.', ':', '*', '_',
        ]);
        if trimmed.is_empty() || trimmed.contains("://") || trimmed.starts_with('@') {
            continue;
        }
        let Some((path, line)) = split_line(trimmed) else {
            continue;
        };
        if !looks_like_path(path) {
            continue;
        }
        let at = offset + begin + (word.len() - trimmed_start.len());
        found.push(Candidate {
            range: at..at + trimmed.len(),
            path: path.to_owned(),
            line,
        });
    }
}

/// A word in prose is taken for a path only with a folder or an extension, so a file named
/// like an ordinary word (`test`, `Makefile`) is linked only in backticks.
fn looks_like_path(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    let extension = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=10).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext.chars().any(|c| c.is_ascii_alphabetic())
    });
    let folder = word.contains('/') && !name.is_empty();
    (extension || folder)
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@'))
}

/// `path:12`, `path:12:3` or `path#L12` → the path and line.
fn split_line(text: &str) -> Option<(&str, Option<u32>)> {
    if let Some((path, line)) = text.rsplit_once("#L")
        && let Ok(line) = line.parse()
    {
        return Some((path, Some(line)));
    }
    let mut path = text;
    let mut line = None;
    for _ in 0..2 {
        if let Some((rest, number)) = path.rsplit_once(':')
            && let Ok(number) = number.parse::<u32>()
        {
            line = Some(number);
            path = rest;
        } else {
            break;
        }
    }
    (!path.is_empty()).then_some((path, line))
}

/// `markdown` with each resolved candidate made a link to its file (`links` in source order:
/// the candidate and its file, relative to the folder).
pub fn link(markdown: &str, links: &[(&Candidate, &str)]) -> String {
    let mut out = String::with_capacity(markdown.len() + links.len() * 48);
    let mut at = 0;
    for (candidate, file) in links {
        if candidate.range.start < at {
            continue;
        }
        out.push_str(&markdown[at..candidate.range.start]);
        out.push('[');
        out.push_str(&markdown[candidate.range.clone()]);
        out.push_str("](");
        out.push_str(&url(file, candidate.line));
        out.push(')');
        at = candidate.range.end;
    }
    out.push_str(&markdown[at..]);
    out
}

/// The link to `file` (relative to the folder), at `line`.
pub fn url(file: &str, line: Option<u32>) -> String {
    let mut url = String::from(SCHEME);
    for byte in file.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-') {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    if let Some(line) = line {
        let _ = write!(url, ":{line}");
    }
    url
}

/// The file and line a link made by [`url`] points at.
pub fn parse_url(url: &str) -> Option<(String, Option<u32>)> {
    let rest = url.strip_prefix(SCHEME)?;
    let (encoded, line) = match rest.rsplit_once(':') {
        Some((path, line)) => (path, Some(line.parse().ok()?)),
        None => (rest, None),
    };
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut iter = encoded.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some((String::from_utf8(bytes).ok()?, line))
}

/// A file link as the chat draws it: a parsed `[label](pistachio-file:…)`.
struct FileLink {
    path: String,
    line: Option<u32>,
    /// The model wrote it in backticks: drawn like inline code.
    code: bool,
}

/// Draws file links as their own elements, so they look like the text around them (inline
/// code stays a code chip) with a quiet underline, and light up under the pointer.
struct FileLinkPlugin;

const PLUGIN: &str = "pistachio-file-link";

impl MarkdownPlugin for FileLinkPlugin {
    fn name(&self) -> &str {
        PLUGIN
    }

    fn parse(&self, node: &MdNode, _: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let MdNode::Link(link) = node else {
            return None;
        };
        let (path, line) = parse_url(&link.url)?;
        let (label, code) = match link.children.as_slice() {
            [MdNode::InlineCode(code)] => (code.value.clone(), true),
            children => (
                children
                    .iter()
                    .map(|child| match child {
                        MdNode::Text(text) => text.value.as_str(),
                        _ => "",
                    })
                    .collect::<String>(),
                false,
            ),
        };
        let markdown = if code {
            format!("`{label}`")
        } else {
            label.clone()
        };
        Some(
            MarkdownNode::new(PLUGIN, FileLink { path, line, code })
                .text(label)
                .markdown(markdown),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let link = node.data::<FileLink>()?;
        let theme = cx.theme();
        let (foreground, chip, chip_hover, underline) = (
            theme.foreground,
            theme.muted,
            theme.secondary_hover,
            theme.muted_foreground.opacity(0.5),
        );
        let mono = theme.mono_font_family.clone();
        let action = super::OpenFileLink {
            path: link.path.clone(),
            line: link.line,
        };
        let tip = match link.line {
            Some(line) => format!("Open {}:{line}", link.path),
            None => format!("Open {}", link.path),
        };
        let id = ElementId::Name(
            format!(
                "file-link:{}:{}",
                node.source_range().map_or(0, |range| range.start),
                link.path
            )
            .into(),
        );
        let element = div()
            .id(id)
            .text_size(context.font_size())
            .line_height(context.line_height())
            .text_color(foreground)
            .rounded(px(4.))
            .underline()
            .text_decoration_color(underline)
            .cursor_pointer()
            .when(link.code, |this| this.font_family(mono).px(px(3.)).bg(chip))
            .when(!link.code, |this| this.px(px(1.)))
            .hover(move |style| style.bg(chip_hover).text_decoration_color(foreground))
            .tooltip(move |window, cx| {
                component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(Box::new(action.clone()), cx);
            })
            .child(node.as_text().to_owned());
        Some(InlineElement::new(element))
    }
}

/// The markdown extensions every chat text view uses: drawing file links and formulas.
pub fn markdown_extensions() -> MarkdownExtensions {
    static EXTENSIONS: LazyLock<MarkdownExtensions> = LazyLock::new(|| {
        // A fixed revision: text views rebuilt every frame keep their parsed text.
        super::math::plugins(MarkdownExtensions::default().plugin(FileLinkPlugin))
            .parser_revision(1)
    });
    EXTENSIONS.clone()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use pretty_assertions::assert_eq;

    // Not `super::*`: gpui's `test` attribute would replace the standard one.
    use super::{Candidate, FileIndex, candidates, link, parse_url, split_line, url};

    fn index(files: &[&str]) -> FileIndex {
        FileIndex::new(Arc::new(files.iter().map(|&f| f.to_owned()).collect()))
    }

    /// The answer as the chat draws it.
    fn shown(markdown: &str, files: &FileIndex) -> String {
        let root = Path::new("/work/proj");
        let found = candidates(markdown);
        let links: Vec<(&Candidate, &str)> = found
            .iter()
            .filter_map(|candidate| Some((candidate, files.resolve(&candidate.path, root)?)))
            .collect();
        link(markdown, &links)
    }

    #[test]
    fn links_files_of_the_folder() {
        let files = index(&[
            "fib.rs",
            "src/main.rs",
            "crates/a/lib.rs",
            "crates/b/lib.rs",
            "my file.txt",
            "Makefile",
        ]);
        assert_eq!(
            shown(
                "Done. The script is at `fib.rs` and compiles to `./fib`:",
                &files
            ),
            "Done. The script is at [`fib.rs`](pistachio-file:fib.rs) and compiles to `./fib`:"
        );
        // Plain text, with a line number and punctuation around it.
        assert_eq!(
            shown("See src/main.rs:12, (fib.rs) and main.rs.", &files),
            "See [src/main.rs:12](pistachio-file:src/main.rs:12), ([fib.rs](pistachio-file:fib.rs)) \
             and [main.rs](pistachio-file:src/main.rs)."
        );
        // A name two files share, and a path ending only one has.
        assert_eq!(
            shown("`lib.rs` vs `b/lib.rs`", &files),
            "`lib.rs` vs [`b/lib.rs`](pistachio-file:crates/b/lib.rs)"
        );
        // Absolute paths inside the folder; outside it, and `..`, stay text.
        assert_eq!(
            shown("`/work/proj/fib.rs` `/etc/fib.rs` `../fib.rs`", &files),
            "[`/work/proj/fib.rs`](pistachio-file:fib.rs) `/etc/fib.rs` `../fib.rs`"
        );
        // A plain word is a path only with a folder or an extension; backticks link any name.
        assert_eq!(
            shown("Run Makefile or `Makefile`", &files),
            "Run Makefile or [`Makefile`](pistachio-file:Makefile)"
        );
    }

    #[test]
    fn leaves_code_links_and_urls_alone() {
        let files = index(&["fib.rs", "docs/a.md"]);
        let markdown = "```\nfib.rs\n```\n\n[docs](docs/a.md) https://x.io/fib.rs \\_fib.rs";
        assert_eq!(shown(markdown, &files), markdown);
        // In a list and a table cell.
        assert_eq!(
            shown("- edit `fib.rs`\n\n| f |\n|---|\n| fib.rs |", &files),
            "- edit [`fib.rs`](pistachio-file:fib.rs)\n\n| f |\n|---|\n| [fib.rs](pistachio-file:fib.rs) |"
        );
    }

    #[test]
    fn urls_round_trip() {
        let url = url("dir/my file (1).rs", Some(7));
        assert_eq!(url, "pistachio-file:dir/my%20file%20%281%29.rs:7");
        assert_eq!(
            parse_url(&url),
            Some(("dir/my file (1).rs".to_owned(), Some(7)))
        );
        assert_eq!(parse_url("https://x.io"), None);
        assert_eq!(split_line("a.rs#L5"), Some(("a.rs", Some(5))));
        assert_eq!(split_line("a.rs:5:2"), Some(("a.rs", Some(5))));
    }
}
