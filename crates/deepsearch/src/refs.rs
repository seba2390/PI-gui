//! References that are no Markdown links yet. Models cite in many ways: `[example.com/page]`,
//! `[1]`, `(Short title)` or a bare address, with the links in a list at the end.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::check::links;

/// A short name that fits more pages than this names none of them.
const MAX_PAGES: usize = 4;

/// `report` with every reference to a page it links to as a Markdown link.
pub fn as_links(report: &str) -> String {
    let report = bare_addresses(report);
    let pages = Pages::of(&report);
    let mut code = false;
    let mut out = String::with_capacity(report.len());
    for line in report.split_inclusive('\n') {
        let fence = line.trim_start().starts_with("```");
        code ^= fence;
        if code || fence {
            out.push_str(line);
        } else {
            out.push_str(&pages.link_line(line));
        }
    }
    out
}

/// Bare addresses, and `<https://…>`, as links.
fn bare_addresses(report: &str) -> String {
    let mut out = String::with_capacity(report.len());
    let mut from = 0;
    while let Some(at) = report[from..].find("http").map(|at| from + at) {
        let rest = &report[at..];
        if !rest.starts_with("http://") && !rest.starts_with("https://") {
            out.push_str(&report[from..at + 4]);
            from = at + 4;
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || "<>\"|`".contains(c))
            .unwrap_or(rest.len());
        let before = &report[..at];
        // The address of a link, or a link's label.
        if before.ends_with("](") || before.ends_with("](<") || rest[..end].contains("](") {
            out.push_str(&report[from..at + end]);
            from = at + end;
            continue;
        }
        let mut url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '*', ']']);
        while url.ends_with(')') && url.matches(')').count() > url.matches('(').count() {
            url = url[..url.len() - 1].trim_end_matches(['.', ',', ';']);
        }
        out.push_str(&report[from..at]);
        // A definition, `[1]: https://…`.
        let _ = write!(out, "[{url}]({url})");
        from = at + url.len();
    }
    out.push_str(&report[from..]);
    out
}

/// What the pages a report links to are called in it.
struct Pages {
    /// Address, and the same without what its name leaves out.
    urls: Vec<(String, String)>,
    labels: HashMap<String, String>,
    numbers: HashMap<String, String>,
}

fn name(text: &str) -> String {
    let text = text
        .trim()
        .trim_matches(['*', '_', '`', '"'])
        .to_lowercase();
    let text = text
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    text.trim_end_matches('/').to_owned()
}

impl Pages {
    fn of(report: &str) -> Self {
        let mut pages = Self {
            urls: Vec::new(),
            labels: HashMap::new(),
            numbers: HashMap::new(),
        };
        for line in report.lines() {
            let found = links(line);
            for (_, label, url) in &found {
                let key = name(url);
                if !pages.urls.iter().any(|(_, known)| *known == key) {
                    pages.urls.push(((*url).to_owned(), key));
                }
                let label = name(label);
                if label.chars().count() > 2 {
                    pages
                        .labels
                        .entry(label)
                        .or_insert_with(|| (*url).to_owned());
                }
            }
            // "1. [Title](url)", "[1] [Title](url)" and "[1]: [url](url)".
            if let Some((range, _, url)) = found.first() {
                let lead =
                    line[..range.start].trim_matches([' ', '-', '*', '[', ']', ':', '.', ')']);
                if !lead.is_empty() && lead.chars().all(|c| c.is_ascii_digit()) {
                    pages
                        .numbers
                        .entry(lead.to_owned())
                        .or_insert_with(|| (*url).to_owned());
                }
            }
        }
        pages
    }

    /// The pages `text` names, if it names any.
    fn find(&self, text: &str) -> Vec<String> {
        if let Some(url) = self.numbers.get(text.trim()) {
            return vec![url.clone()];
        }
        let text = name(text);
        if let Some(url) = self.labels.get(&text) {
            return vec![url.clone()];
        }
        if !text.contains('.') || text.contains(' ') {
            return Vec::new();
        }
        if let Some((url, _)) = self.urls.iter().find(|(_, key)| *key == text) {
            return vec![url.clone()];
        }
        let found: Vec<String> = self
            .urls
            .iter()
            .filter(|(_, key)| {
                let host = key.split('/').next().unwrap_or(key);
                key.starts_with(&format!("{text}/")) || host.ends_with(&format!(".{text}"))
            })
            .map(|(url, _)| url.clone())
            .collect();
        if found.len() > MAX_PAGES {
            Vec::new()
        } else {
            found
        }
    }

    /// The pages named in what stands between brackets, if all of it is names.
    fn find_all(&self, text: &str) -> Vec<String> {
        let whole = self.find(text);
        if !whole.is_empty() {
            return whole;
        }
        let parts: Vec<&str> = text.split([',', ';']).collect();
        if parts.len() < 2 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for part in parts {
            let found = self.find(part);
            if found.is_empty() {
                return Vec::new();
            }
            out.extend(found);
        }
        out
    }

    fn link_line(&self, line: &str) -> String {
        let linked: Vec<_> = links(line).into_iter().map(|(range, _, _)| range).collect();
        let mut out = String::with_capacity(line.len());
        let mut from = 0;
        while let Some(open) = line[from..].find(['[', '(']).map(|at| from + at) {
            out.push_str(&line[from..open]);
            if let Some(link) = linked.iter().find(|link| link.contains(&open)) {
                out.push_str(&line[open..link.end]);
                from = link.end;
                continue;
            }
            let close = if line[open..].starts_with('[') {
                ']'
            } else {
                ')'
            };
            let end = line[open + 1..]
                .find(['[', '(', ']', ')'])
                .map(|at| open + 1 + at)
                .filter(|end| line[*end..].starts_with(close));
            let found = end.map_or_else(Vec::new, |end| self.find_all(&line[open + 1..end]));
            match end {
                Some(end) if !found.is_empty() => {
                    let label = &line[open + 1..end];
                    for (index, url) in found.iter().enumerate() {
                        let label = if index == 0 { label } else { "" };
                        let _ = write!(out, "[{label}]({url})");
                    }
                    from = end + 1;
                }
                _ => {
                    out.push_str(&line[open..=open]);
                    from = open + 1;
                }
            }
        }
        out.push_str(&line[from..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const SOURCES: &str = "\n## Sources
- [PSGAMER – Køb](https://psgamer.dk/ps5/koeb/): prices
- [PSGAMER – Tilbud](https://www.psgamer.dk/ps5/tilbud)
- [webtech360.com](https://blog.webtech360.com/da/x%C3%A6/99)
";

    fn linked(line: &str) -> String {
        let report = format!("{line}{SOURCES}");
        let out = as_links(&report);
        out[..out.len() - SOURCES.len()].to_owned()
    }

    #[test]
    fn names_of_pages_become_links() {
        let koeb = "https://psgamer.dk/ps5/koeb/";
        let tilbud = "https://www.psgamer.dk/ps5/tilbud";
        assert_eq!(
            linked("It costs 5 kr. [psgamer.dk/ps5/koeb], [psgamer.dk/ps5/tilbud]"),
            format!(
                "It costs 5 kr. [psgamer.dk/ps5/koeb]({koeb}), [psgamer.dk/ps5/tilbud]({tilbud})"
            )
        );
        assert_eq!(
            linked("| a | [psgamer.dk/ps5/koeb; psgamer.dk/ps5/tilbud] |"),
            format!("| a | [psgamer.dk/ps5/koeb; psgamer.dk/ps5/tilbud]({koeb})[]({tilbud}) |")
        );
        assert_eq!(
            linked("As said (PSGAMER – Køb) and (per webtech360)."),
            format!("As said [PSGAMER – Køb]({koeb}) and (per webtech360).")
        );
        assert_eq!(
            linked("[webtech360.com] and [psgamer.dk]"),
            format!(
                "[webtech360.com](https://blog.webtech360.com/da/x%C3%A6/99) and \
                 [psgamer.dk]({koeb})[]({tilbud})"
            )
        );
        // Links stay, and what names no page.
        for line in ["[A](https://a.org/x_(y)) (in stock) [x] [2026]", "- [ ] 1"] {
            assert_eq!(linked(line), line);
        }
    }

    #[test]
    fn numbers_and_bare_addresses_are_references_too() {
        let report = "Sold [1], [2]. See https://c.org/x, or <https://d.org>.\n```\n[1]\n```\n\
                      1. [A](https://a.org)\n[2]: https://b.org/p\n";
        assert_eq!(
            as_links(report),
            "Sold [1](https://a.org), [2](https://b.org/p). See \
             [https://c.org/x](https://c.org/x), or <[https://d.org](https://d.org)>.\n```\n[1]\n\
             ```\n1. [A](https://a.org)\n[2](https://b.org/p): \
             [https://b.org/p](https://b.org/p)\n"
        );
    }
}
