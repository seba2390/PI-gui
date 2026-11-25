//! Writing Markdown.

use std::fmt::Write as _;

#[derive(Debug, Default)]
pub struct Markdown {
    text: String,
    /// The last thing was an item, so the next one follows it directly.
    in_list: bool,
}

impl Markdown {
    /// `level` from 1.
    pub fn heading(&mut self, level: usize, text: &str) {
        let text = squash(text);
        if !text.is_empty() {
            self.block();
            let _ = writeln!(self.text, "{} {text}", "#".repeat(level.clamp(1, 6)));
        }
    }

    pub fn paragraph(&mut self, text: &str) {
        let lines: Vec<String> = text
            .lines()
            .map(squash)
            .filter(|line| !line.is_empty())
            .collect();
        if !lines.is_empty() {
            self.block();
            self.text.push_str(&lines.join("\n"));
            self.text.push('\n');
        }
    }

    /// An item, `number` in an ordered list.
    pub fn item(&mut self, depth: usize, number: Option<usize>, text: &str) {
        let text = squash(text);
        if text.is_empty() {
            return;
        }
        if !self.in_list {
            self.block();
        }
        let indent = "  ".repeat(depth.min(8));
        let _ = match number {
            Some(number) => writeln!(self.text, "{indent}{number}. {text}"),
            None => writeln!(self.text, "{indent}- {text}"),
        };
        self.in_list = true;
    }

    /// Lines that keep their spaces.
    pub fn code(&mut self, text: &str) {
        let text = text.trim_matches('\n');
        if !text.trim().is_empty() {
            self.block();
            let _ = writeln!(self.text, "```\n{text}\n```");
        }
    }

    /// Rows of cells, the first with the names of the columns.
    pub fn table(&mut self, rows: &[Vec<String>]) {
        let rows: Vec<Vec<String>> = rows
            .iter()
            .map(|row| row.iter().map(|cell| squash(cell)).collect::<Vec<_>>())
            .filter(|row: &Vec<String>| row.iter().any(|cell| !cell.is_empty()))
            .collect();
        let columns = rows
            .iter()
            .filter_map(|row| row.iter().rposition(|cell| !cell.is_empty()))
            .max()
            .map_or(0, |last| last + 1);
        // What has one column is a frame around text.
        if columns < 2 {
            for cell in rows.iter().flatten() {
                self.paragraph(cell);
            }
            return;
        }
        self.block();
        for (index, row) in rows.iter().enumerate() {
            self.text.push('|');
            for column in 0..columns {
                let cell = row.get(column).map_or("", String::as_str);
                let _ = write!(self.text, " {} |", cell.replace('|', "\\|"));
            }
            self.text.push('\n');
            if index == 0 {
                self.text.push('|');
                self.text.push_str(&" --- |".repeat(columns));
                self.text.push('\n');
            }
        }
    }

    fn block(&mut self) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.in_list = false;
    }

    pub fn finish(self) -> String {
        self.text
    }
}

pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A link, if it leads to the web.
pub fn link(text: &str, target: &str) -> String {
    let web = ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| target.starts_with(scheme));
    let shown = text.trim();
    if !web || shown.is_empty() || shown == target || target.contains(char::is_whitespace) {
        return text.to_owned();
    }
    format!("[{shown}]({target})")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn blocks() {
        let mut out = Markdown::default();
        out.heading(1, " A\n title ");
        out.paragraph("two  words\nand a line\n\n");
        out.item(0, None, "one");
        out.item(1, Some(2), "two");
        out.paragraph("");
        out.paragraph("after");
        out.code("\n  x\n");
        assert_eq!(
            out.finish(),
            "# A title\n\ntwo words\nand a line\n\n- one\n  2. two\n\nafter\n\n```\n  x\n```\n"
        );
    }

    #[test]
    fn tables() {
        let row = |cells: &[&str]| cells.iter().map(|cell| (*cell).to_owned()).collect();
        let mut out = Markdown::default();
        out.table(&[
            row(&["A", "B", ""]),
            row(&["", "", ""]),
            row(&["1|2"]),
            row(&["x", "two\nlines"]),
        ]);
        out.table(&[row(&["", ""])]);
        out.table(&[row(&["framed", ""])]);
        assert_eq!(
            out.finish(),
            "| A | B |\n| --- | --- |\n| 1\\|2 |  |\n| x | two lines |\n\nframed\n"
        );
    }

    #[test]
    fn links() {
        assert_eq!(link("pi", "https://pi.dev"), "[pi](https://pi.dev)");
        for (text, target) in [
            ("https://a.b", "https://a.b"),
            ("x", "#top"),
            ("x", "file:///a"),
            (" ", "https://a.b"),
        ] {
            assert_eq!(link(text, target), text);
        }
    }
}
