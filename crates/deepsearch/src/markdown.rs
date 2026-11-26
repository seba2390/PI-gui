//! Blocks from Markdown, as document parsers write it: headings, paragraphs, lists, quotes, code
//! and tables. Tables are paragraphs of rows; long ones are split, each part with the column
//! names, so that a passage of it can be understood alone.

use crate::extract::{Block, BlockKind, Page, squash};

/// Long tables are split into parts of about this many bytes.
const TABLE_PART: usize = 700;
const TITLE_CHARS: usize = 150;

pub fn page(text: &str) -> Page {
    let blocks = blocks(text);
    // The first heading of the highest level that is not just a section, or the first text.
    let top = blocks
        .iter()
        .filter_map(|block| match block.kind {
            BlockKind::Heading(level) => Some(level),
            _ => None,
        })
        .min();
    let title = blocks
        .iter()
        .find(|block| top.is_some_and(|top| block.kind == BlockKind::Heading(top)))
        .or_else(|| blocks.iter().find(|b| b.kind == BlockKind::Paragraph))
        .map_or_default(|block| block.text.chars().take(TITLE_CHARS).collect());
    Page {
        title,
        blocks,
        ..Page::default()
    }
}

pub fn blocks(text: &str) -> Vec<Block> {
    let mut blocks = Blocks::default();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if let Some(fence) = fence(trimmed) {
            blocks.flush();
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim().starts_with(fence) && line.trim().chars().all(|c| fence.contains(c))
                {
                    break;
                }
                code.push(line);
            }
            blocks.push(BlockKind::Code, code.join("\n"));
        } else if trimmed.starts_with("$$") {
            // A formula keeps its lines.
            blocks.flush();
            let mut formula = vec![trimmed];
            let closed = trimmed.len() > 4 && trimmed.ends_with("$$");
            while !closed && let Some(line) = lines.next() {
                formula.push(line.trim());
                if line.trim().ends_with("$$") {
                    break;
                }
            }
            blocks.push(BlockKind::Paragraph, formula.join("\n"));
        } else if trimmed.starts_with("<table") {
            blocks.flush();
            let mut html = line.to_owned();
            while !html.contains("</table>")
                && let Some(line) = lines.next()
            {
                html.push('\n');
                html.push_str(line);
            }
            blocks.table(html_rows(&html));
        } else if is_row(trimmed) {
            blocks.flush();
            let mut rows = vec![cells(trimmed)];
            while let Some(line) = lines.next_if(|line| is_row(line.trim())) {
                rows.push(cells(line.trim()));
            }
            blocks.table(rows);
        } else if trimmed.is_empty() {
            blocks.flush();
        } else if let Some((level, text)) = heading(trimmed) {
            blocks.flush();
            blocks.push(BlockKind::Heading(level), plain(text));
        } else if is_rule(trimmed) {
            blocks.flush();
        } else if let Some(text) = item(trimmed) {
            blocks.flush();
            blocks.start(BlockKind::ListItem, text);
        } else if let Some(text) = trimmed.strip_prefix('>') {
            if blocks.kind != Some(BlockKind::Quote) {
                blocks.flush();
            }
            blocks.start(BlockKind::Quote, text.trim());
        } else {
            let kind = blocks.kind.clone().unwrap_or(BlockKind::Paragraph);
            blocks.start(kind, trimmed);
        }
    }
    blocks.flush();
    blocks.done
}

#[derive(Default)]
struct Blocks {
    done: Vec<Block>,
    /// The block being read, whose lines are joined.
    kind: Option<BlockKind>,
    text: String,
}

impl Blocks {
    /// Starts a block of `kind`, or adds a line to the one being read.
    fn start(&mut self, kind: BlockKind, line: &str) {
        self.kind = Some(kind);
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        self.text.push_str(line);
    }

    fn flush(&mut self) {
        let text = squash(&std::mem::take(&mut self.text));
        if let Some(kind) = self.kind.take() {
            self.push(kind, text);
        }
    }

    fn push(&mut self, kind: BlockKind, text: String) {
        if !text.trim().is_empty() {
            self.done.push(Block { kind, text });
        }
    }

    /// A table's rows, the first with the column names.
    fn table(&mut self, rows: Vec<Vec<String>>) {
        let mut rows = rows
            .into_iter()
            .filter(|row| row.iter().any(|cell| !cell.is_empty()))
            .filter(|row| !row.iter().all(|cell| is_delimiter(cell)));
        let Some(head) = rows.next() else {
            return;
        };
        let line = |cells: &[String]| format!("| {} |", cells.join(" | "));
        let head = format!("{}\n|{}", line(&head), " --- |".repeat(head.len().max(1)));
        let mut part = head.clone();
        let mut filled = false;
        for row in rows {
            let row = line(&row);
            if filled && part.len() + row.len() > TABLE_PART {
                self.push(
                    BlockKind::Paragraph,
                    std::mem::replace(&mut part, head.clone()),
                );
            }
            part.push('\n');
            part.push_str(&row);
            filled = true;
        }
        self.push(BlockKind::Paragraph, part);
    }
}

fn fence(line: &str) -> Option<&str> {
    ["```", "~~~"]
        .into_iter()
        .find(|fence| line.starts_with(fence))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    let text = line[level..].strip_prefix(' ')?;
    let level = u8::try_from(level).ok().filter(|l| (1..=6).contains(l))?;
    Some((level, text.trim_end_matches('#').trim()))
}

/// A heading's text without emphasis.
fn plain(text: &str) -> String {
    squash(&text.replace("**", "").replace("__", ""))
}

fn is_rule(line: &str) -> bool {
    line.len() >= 3
        && ['-', '*', '_']
            .into_iter()
            .any(|mark| line.chars().all(|c| c == mark || c == ' '))
}

fn item(line: &str) -> Option<&str> {
    let bullet = ["- ", "* ", "+ ", "• "]
        .into_iter()
        .find_map(|bullet| line.strip_prefix(bullet));
    let numbered = || {
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        let rest = &line[digits..];
        // The number says which one it is.
        ((1..=3).contains(&digits) && (rest.starts_with(". ") || rest.starts_with(") ")))
            .then_some(line)
    };
    bullet.map(str::trim).or_else(numbered)
}

fn is_row(line: &str) -> bool {
    line.len() > 1 && line.starts_with('|') && line.ends_with('|')
}

fn cells(row: &str) -> Vec<String> {
    let inner = &row[1..row.len() - 1];
    let mut cells = vec![String::new()];
    let mut escaped = false;
    for c in inner.chars() {
        if c == '|' && !escaped {
            cells.push(String::new());
        } else if let Some(cell) = cells.last_mut() {
            cell.push(c);
        }
        escaped = c == '\\' && !escaped;
    }
    cells
        .iter()
        .map(|cell| squash(&cell.replace("<br>", " ")))
        .collect()
}

/// `---`, `:-:`: the line below the column names.
fn is_delimiter(cell: &str) -> bool {
    !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':'))
}

/// The rows of an HTML table.
fn html_rows(html: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for row in html.split("<tr").skip(1) {
        let row = row.split("</tr").next().unwrap_or_default();
        let mut cells = Vec::new();
        for cell in row.replace("<th", "<td").split("<td").skip(1) {
            let text = cell.split_once('>').map_or("", |(_, text)| text);
            let text = text.split("</t").next().unwrap_or_default();
            cells.push(crate::extract::fragment_text(text).replace('|', "\\|"));
        }
        rows.push(cells);
    }
    rows
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use BlockKind::{Code, Heading, ListItem, Paragraph, Quote};

    fn read(text: &str) -> Vec<(BlockKind, String)> {
        blocks(text)
            .into_iter()
            .map(|block| (block.kind, block.text))
            .collect()
    }

    #[test]
    fn reads_what_parsers_write() {
        let text = "# **The _Title_**\n\nA paragraph\nover two lines.\n\n## Methods ##\n- One\n  more\n\
                    * Two\n1. First\n\n> A quote\n> goes on\n\n---\n\n```python\n# not a heading\nx = 1\n```\n\
                    $$\nE = mc^2\n$$\n$$ a $$\nEnd.";
        assert_eq!(
            read(text),
            [
                (Heading(1), "The _Title_".into()),
                (Paragraph, "A paragraph over two lines.".into()),
                (Heading(2), "Methods".into()),
                (ListItem, "One more".into()),
                (ListItem, "Two".into()),
                (ListItem, "1. First".into()),
                (Quote, "A quote goes on".into()),
                (Code, "# not a heading\nx = 1".into()),
                (Paragraph, "$$\nE = mc^2\n$$".into()),
                (Paragraph, "$$ a $$".into()),
                (Paragraph, "End.".into()),
            ]
        );
        assert_eq!(read("#hashtag\n\n####### seven"), {
            [
                (Paragraph, "#hashtag".to_owned()),
                (Paragraph, "####### seven".to_owned()),
            ]
        });
        assert_eq!(read(" \n\n"), []);
        // Not closed.
        assert_eq!(read("```\ncode"), [(Code, "code".to_owned())]);
        assert_eq!(read("$$\na"), [(Paragraph, "$$\na".to_owned())]);
    }

    #[test]
    fn tables_become_rows() {
        let text = "Before\n|**Country**|Tonnes|\n|:---|---:|\n| USA |  523900 |\n| a\\|b | x<br>y |\nAfter";
        assert_eq!(
            read(text),
            [
                (Paragraph, "Before".into()),
                (
                    Paragraph,
                    "| **Country** | Tonnes |\n| --- | --- |\n| USA | 523900 |\n| a\\|b | x y |"
                        .into()
                ),
                (Paragraph, "After".into()),
            ]
        );
        let html = "<table><tr><th>Country</th><th colspan=\"2\">Tonnes &amp; share</th></tr>\n\
                    <tr><td>USA</td><td>523900</td><td><b>48</b> %</td></tr></table>\n\nAfter";
        assert_eq!(
            read(html),
            [
                (
                    Paragraph,
                    "| Country | Tonnes & share |\n| --- | --- |\n| USA | 523900 | 48 % |".into()
                ),
                (Paragraph, "After".into()),
            ]
        );
        assert_eq!(read("<table>never closed"), []);
        assert_eq!(read("|---|---|"), []);
    }

    #[test]
    fn every_part_of_a_long_table_has_the_column_names() {
        use std::fmt::Write as _;
        let mut text = String::from("| Name | Value |\n|---|---|\n");
        for n in 0..100 {
            let _ = writeln!(text, "| row {n} | {} |", n * n);
        }
        let parts = read(&text);
        assert!(parts.len() >= 3);
        for (_, part) in &parts {
            assert!(part.starts_with("| Name | Value |\n| --- | --- |\n| row"));
            assert!(part.len() < TABLE_PART + 100);
        }
        let rows: usize = parts.iter().map(|(_, part)| part.lines().count() - 2).sum();
        assert_eq!(rows, 100);
    }

    #[test]
    fn the_title_is_the_highest_heading() {
        assert_eq!(
            page("## Abstract\n\nText\n\n# A Paper\n\nx").title,
            "A Paper"
        );
        assert_eq!(page("## A Paper\n\n## Abstract").title, "A Paper");
        assert_eq!(page("Just text.\n\nMore.").title, "Just text.");
        assert_eq!(page("").title, "");
    }

    #[test]
    fn passages_of_it_stay_whole() {
        let text = "# Title\n\nIntro.\n\n## Costs\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let passages = crate::rank::passages(&blocks(text), 900);
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[1].heading.as_deref(), Some("Costs"));
        assert_eq!(passages[1].text, "| A | B |\n| --- | --- |\n| 1 | 2 |");
    }
}
