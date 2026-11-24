//! Checking a report: whether its links work and were read, and what its sources say about each
//! claim. Judging that is the model's (the extension asks it); here the claims are found, the
//! passages that could back them are picked, and the quotes the model gives are looked up.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::pages::{self, StoredPage};
use crate::rank::{self, Passage};
use crate::tools;

/// How much of each source is shown for a claim.
const EVIDENCE_CHARS: usize = 2400;
/// Statements without a source that are shown at most.
const MAX_UNSOURCED: usize = 60;
/// Pages loaded at once.
const THREADS: usize = 6;
/// A quote counts as found when this share of its word triples is on the page.
const QUOTE_MATCH: f64 = 0.7;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CheckInput {
    pub report: String,
    /// Where pages that were read are kept.
    pub pages: PathBuf,
    /// Pages the model read before this pi process started.
    pub read: Vec<String>,
    pub max_claims: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkState {
    /// The model read it.
    Read,
    /// It exists, or may (some sites refuse automated readers), but was not read.
    Unread,
    /// There is no such page.
    Broken,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub url: String,
    pub state: LinkState,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Claim {
    pub id: usize,
    /// The sentence, without its links.
    pub text: String,
    pub urls: Vec<String>,
    /// What its sources say that is closest to it.
    pub evidence: String,
}

/// A statement that no page the model read stands behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsourced {
    pub text: String,
    /// What it cites instead: pages that were not read.
    pub urls: Vec<String>,
    /// It says something checkable (a number that no sourced claim has, or it cites a page that
    /// was not read). Otherwise it may be advice or a remark: the model is asked.
    pub sure: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checked {
    pub links: Vec<Link>,
    /// The claims to judge, in the order of the report.
    pub claims: Vec<Claim>,
    pub unsourced: Vec<Unsourced>,
    /// How many claims of the report there are, with and without a source.
    pub total_claims: usize,
}

/// A sentence, bullet or table row of the report and the sources it names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Statement {
    text: String,
    urls: Vec<String>,
    /// It is in a part about what is uncertain, where nothing has to be proven.
    caveat: bool,
}

pub fn check(config: &Config, input: &CheckInput) -> Checked {
    let statements = statements(&input.report);
    let mut urls: Vec<String> = Vec::new();
    for url in statements
        .iter()
        .flat_map(|statement| &statement.urls)
        .cloned()
        .chain(all_links(&input.report))
    {
        if !urls
            .iter()
            .any(|known| pages::key(known) == pages::key(&url))
        {
            urls.push(url);
        }
    }
    let read: HashSet<String> = input.read.iter().map(|url| pages::key(url)).collect();
    let loaded = load_all(config, input, &urls, &read);
    let links: Vec<Link> = urls
        .iter()
        .zip(&loaded)
        .map(|(url, page)| link(url, page, &read))
        .collect();
    // A page that was not read is no source, whatever it says.
    let sources: HashMap<String, Option<&StoredPage>> = links
        .iter()
        .zip(&loaded)
        .filter(|(link, _)| link.state == LinkState::Read)
        .map(|(link, page)| (pages::key(&link.url), page.as_ref().ok()))
        .collect();

    let (cited, bare): (Vec<&Statement>, Vec<&Statement>) =
        statements.iter().partition(|statement| {
            statement
                .urls
                .iter()
                .any(|url| sources.contains_key(&pages::key(url)))
        });
    let unsourced = unsourced(&cited, &bare);
    let total_claims = cited.len() + unsourced.iter().filter(|claim| claim.sure).count();
    let claims = pick(&cited, input.max_claims)
        .into_iter()
        .filter_map(|index| {
            let claim = cited[index];
            let pages: Vec<(&String, &StoredPage)> = claim
                .urls
                .iter()
                .filter_map(|url| Some((url, (*sources.get(&pages::key(url))?)?)))
                .collect();
            let mut evidence = String::new();
            for (url, page) in &pages {
                let passages = passages_for(page, &claim.text, EVIDENCE_CHARS / pages.len());
                let _ = writeln!(evidence, "From {url} ({}):\n{passages}", page.title);
            }
            // Read before, but not there any more: nothing to judge it by.
            (!evidence.is_empty()).then(|| Claim {
                id: index + 1,
                text: claim.text.clone(),
                urls: pages.iter().map(|(url, _)| (*url).clone()).collect(),
                evidence,
            })
        })
        .collect();
    Checked {
        links,
        claims,
        unsourced,
        total_claims,
    }
}

/// The statements among `bare` that need a source.
fn unsourced(cited: &[&Statement], bare: &[&Statement]) -> Vec<Unsourced> {
    let sourced: HashSet<String> = cited
        .iter()
        .flat_map(|claim| numbers(&claim.text))
        .collect();
    let mut out: Vec<Unsourced> = Vec::new();
    for statement in bare {
        let sure = if statement.urls.is_empty() {
            let numbers = numbers(&statement.text);
            // A summary may repeat what is backed elsewhere.
            if statement.caveat || (!numbers.is_empty() && numbers.is_subset(&sourced)) {
                continue;
            }
            !numbers.is_empty()
        } else {
            true
        };
        if !out.iter().any(|known| known.text == statement.text) {
            out.push(Unsourced {
                text: statement.text.clone(),
                urls: statement.urls.clone(),
                sure,
            });
        }
    }
    // Where there are too many, the sure ones.
    out.sort_by_key(|claim| !claim.sure);
    out.truncate(MAX_UNSOURCED);
    out
}

/// The numbers in `text`, without what groups their digits.
fn numbers(text: &str) -> HashSet<String> {
    text.split(|c: char| c.is_whitespace() || "–—-/()[]:;|~≈".contains(c))
        .map(|word| {
            word.chars()
                .filter(char::is_ascii_digit)
                .collect::<String>()
        })
        .filter(|digits| !digits.is_empty())
        .collect()
}

/// What `page` says that is closest to `claim`.
fn passages_for(page: &StoredPage, claim: &str, max_chars: usize) -> String {
    let chosen = rank::select(&page.passages, Some(claim), max_chars);
    rank::render(&page.passages, &chosen, max_chars)
}

/// The page at each address: kept from reading it, or loaded now.
fn load_all(
    config: &Config,
    input: &CheckInput,
    urls: &[String],
    read: &HashSet<String>,
) -> Vec<Result<StoredPage, String>> {
    let load = |url: &String| {
        if let Some(page) = pages::load(&input.pages, url) {
            return Ok(page);
        }
        let parsed = url::Url::parse(url).map_err(|error| error.to_string())?;
        // Not PDFs: one that breaks its reader would end the whole check.
        let mut page = tools::load_page(config, &parsed, false)?;
        page.read = read.contains(&pages::key(url));
        pages::store(&input.pages, url, &page);
        Ok(page)
    };
    let mut loaded = Vec::with_capacity(urls.len());
    for batch in urls.chunks(THREADS) {
        std::thread::scope(|scope| {
            let threads: Vec<_> = batch.iter().map(|url| scope.spawn(|| load(url))).collect();
            for thread in threads {
                loaded.push(
                    thread
                        .join()
                        .unwrap_or_else(|_| Err("it could not be read".into())),
                );
            }
        });
    }
    loaded
}

fn link(url: &str, page: &Result<StoredPage, String>, read: &HashSet<String>) -> Link {
    let was_read = read.contains(&pages::key(url));
    let (state, note) = match page {
        Ok(page) if page.read || was_read => (LinkState::Read, String::new()),
        Ok(_) => (LinkState::Unread, "you did not read this page".into()),
        Err(_) if was_read => (LinkState::Read, String::new()),
        Err(error) => {
            let gone = [
                "HTTP 404",
                "HTTP 410",
                "host not found",
                "failed to lookup",
                "invalid",
            ]
            .iter()
            .any(|sign| error.to_lowercase().contains(&sign.to_lowercase()));
            let state = if gone {
                LinkState::Broken
            } else {
                LinkState::Unread
            };
            (state, error.clone())
        }
    };
    Link {
        url: url.to_owned(),
        state,
        note,
    }
}

/// The claims to judge, at most `max`: those with numbers first, as they are the easiest to get
/// wrong. In the order of the report.
fn pick(cited: &[&Statement], max: usize) -> Vec<usize> {
    let has_number = |index: &usize| cited[*index].text.chars().any(|c| c.is_ascii_digit());
    let (mut picked, rest): (Vec<usize>, Vec<usize>) = (0..cited.len()).partition(has_number);
    picked.extend(rest);
    picked.truncate(max);
    picked.sort_unstable();
    picked
}

/// A part of the report as it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Heading(String),
    Paragraph(String),
    /// How far it is indented, and its text.
    Bullet(usize, String),
    /// Its cells, as `column: value`.
    Row(String),
}

fn parts(report: &str) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    let mut code = false;
    let mut open = false;
    let mut columns: Vec<String> = Vec::new();
    for raw in report.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            code = !code;
            open = false;
            continue;
        }
        if code {
            continue;
        }
        if line.starts_with('|') {
            open = false;
            let cells = cells(line);
            if columns.is_empty() {
                columns = cells;
            } else if !cells
                .iter()
                .all(|cell| cell.chars().all(|c| "-: ".contains(c)))
            {
                let named = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| !cell.is_empty());
                let text = named
                    .map(|(index, cell)| match columns.get(index) {
                        Some(column) if !column.is_empty() && index > 0 => {
                            format!("{column}: {cell}")
                        }
                        _ => cell.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                out.push(Part::Row(text));
            }
            continue;
        }
        columns.clear();
        if line.is_empty() || line.chars().all(|c| "-*_= ".contains(c)) {
            open = false;
        } else if line.starts_with('#') {
            out.push(Part::Heading(line.to_owned()));
            open = false;
        } else if let Some(text) = bullet(line) {
            out.push(Part::Bullet(raw.len() - raw.trim_start().len(), text));
            open = true;
        } else if let (true, Some(Part::Paragraph(text) | Part::Bullet(_, text))) =
            (open, out.last_mut())
        {
            text.push(' ');
            text.push_str(line);
        } else {
            // A line in bold alone is a heading too.
            let bold = line.starts_with("**") && line.trim_end_matches(':').ends_with("**");
            if bold && line.matches("**").count() == 2 && line.len() < 90 {
                out.push(Part::Heading(line.to_owned()));
                open = false;
            } else {
                out.push(Part::Paragraph(
                    line.trim_start_matches(['>', ' ']).to_owned(),
                ));
                open = true;
            }
        }
    }
    out
}

fn cells(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .replace("\\|", "¦")
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// What a list item says, or `None` for another line.
fn bullet(line: &str) -> Option<String> {
    let text = line
        .strip_prefix(['-', '*', '+', '•'])
        .or_else(|| {
            let digits = line.trim_start_matches(|c: char| c.is_ascii_digit());
            (digits.len() < line.len() && line.len() - digits.len() < 4)
                .then(|| digits.strip_prefix(['.', ')']))
                .flatten()
        })
        .filter(|text| text.starts_with(char::is_whitespace))?;
    Some(text.trim().to_owned())
}

/// Everything the report states, with the sources it names for it: in the same sentence, else
/// in its paragraph or bullet, else in the line that leads into its list or table or right after
/// it. Without the list of sources.
fn statements(report: &str) -> Vec<Statement> {
    let mut out: Vec<Statement> = Vec::new();
    // The statements of the block of bullets or rows being read, or just read.
    let mut block: Option<usize> = None;
    let mut lead: Vec<String> = Vec::new();
    // The bullets above, by how far they are indented, and what they cite.
    let mut above: Vec<(usize, Vec<String>)> = Vec::new();
    let mut sources = false;
    let mut caveat = false;
    for part in parts(report) {
        match part {
            Part::Heading(text) => {
                sources = is_sources_heading(&text);
                caveat = is_caveat_heading(&text);
                block = None;
                lead.clear();
                above.clear();
            }
            _ if sources => {}
            Part::Paragraph(text) => {
                let (plain, urls) = without_links(&text);
                if is_sources_heading(&plain) && urls.is_empty() {
                    sources = true;
                    continue;
                }
                // "Sources: [a], [b]" below a list or table.
                if let Some(from) = block.take() {
                    let named: Vec<String> = sentences(&text)
                        .iter()
                        .filter(|sentence| is_source_line(sentence))
                        .flat_map(|sentence| without_links(sentence).1)
                        .collect();
                    for statement in &mut out[from..] {
                        if statement.urls.is_empty() {
                            statement.urls.clone_from(&named);
                        }
                    }
                }
                above.clear();
                lead = if plain.trim_end().ends_with(':') {
                    urls
                } else {
                    Vec::new()
                };
                state(&text, &[], caveat, &mut out);
            }
            Part::Bullet(indent, text) => {
                above.retain(|(other, _)| *other < indent);
                let inherited = above
                    .iter()
                    .rev()
                    .find(|(_, urls)| !urls.is_empty())
                    .map_or_else(|| lead.clone(), |(_, urls)| urls.clone());
                block.get_or_insert(out.len());
                state(&text, &inherited, caveat, &mut out);
                above.push((indent, without_links(&text).1));
            }
            Part::Row(text) => {
                block.get_or_insert(out.len());
                let (text, urls) = without_links(&text);
                out.push(Statement {
                    text,
                    urls: if urls.is_empty() { lead.clone() } else { urls },
                    caveat,
                });
            }
        }
    }
    out.retain(|statement| {
        let words = statement.text.split_whitespace().count();
        let number = statement.text.chars().any(|c| c.is_ascii_digit());
        words >= if number { 3 } else { 5 } && !statement.text.trim_end().ends_with('?')
    });
    out
}

/// Adds the sentences of a paragraph or bullet. Those without a link get the block's own links,
/// or `inherited`.
fn state(text: &str, inherited: &[String], caveat: bool, out: &mut Vec<Statement>) {
    let first = out.len();
    let own = without_links(text).1;
    for sentence in sentences(text) {
        let (text, urls) = without_links(&sentence);
        // A link after the full stop belongs to the sentence before it.
        if is_source_line(&sentence) {
            if let Some(last) = out[first..].last_mut() {
                last.urls.extend(urls);
                continue;
            }
            // "Details: [a]", or a short link; a long one may be the claim itself.
            let named = text
                .split_whitespace()
                .next()
                .is_some_and(|word| word.ends_with(':'));
            if named || link_words(&sentence) < 6 {
                continue;
            }
        }
        // A line that only introduces what follows.
        if urls.is_empty() && text.ends_with(':') {
            continue;
        }
        out.push(Statement { text, urls, caveat });
    }
    let fallback = if own.is_empty() { inherited } else { &own };
    for statement in &mut out[first..] {
        if statement.urls.is_empty() {
            statement.urls = fallback.to_vec();
        }
    }
}

/// Whether a sentence says nothing besides its links ("Source: [a] and [b]").
fn is_source_line(sentence: &str) -> bool {
    let (text, urls) = without_links(sentence);
    let words = text.split_whitespace().count();
    !urls.is_empty() && words.saturating_sub(link_words(sentence)) < 4
}

fn heading_title(line: &str) -> String {
    line.trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase()
}

fn is_sources_heading(line: &str) -> bool {
    matches!(
        heading_title(line).as_str(),
        "sources"
            | "sources used"
            | "references"
            | "bibliography"
            | "source list"
            | "kilder"
            | "quellen"
            | "fuentes"
            | "källor"
            | "bronnen"
    )
}

fn is_caveat_heading(line: &str) -> bool {
    let title = heading_title(line);
    [
        "caveat",
        "limitation",
        "verif",
        "uncertain",
        "open question",
        "gaps",
        "not confirmed",
        "unknown",
        "forbehold",
        "usikker",
    ]
    .iter()
    .any(|word| title.contains(word))
}

/// Words whose full stop does not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "kr", "ca", "approx", "vs", "no", "nr", "inkl", "incl", "ekskl", "excl", "mr", "ms", "dr",
    "st", "fig", "e.g", "i.e", "bl.a", "f.eks", "z.b", "u.s",
];

/// Splits a paragraph at the ends of sentences, but not inside links.
fn sentences(text: &str) -> Vec<String> {
    let links: Vec<_> = links(text).into_iter().map(|(range, _, _)| range).collect();
    let mut out = Vec::new();
    let mut from = 0;
    for (at, _) in text.match_indices(['.', '!', '?']) {
        if links.iter().any(|link| link.contains(&at)) {
            continue;
        }
        let mut after = text[at + 1..].chars().peekable();
        // "…**." and "…)." still end one.
        while after.next_if(|c| "*)\"”".contains(*c)).is_some() {}
        let space = after.next().is_none_or(char::is_whitespace);
        let starts = after.next().is_none_or(|next| !next.is_lowercase());
        let word = text[from..at].split_whitespace().next_back().unwrap_or("");
        let word = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        // After "kr." a new sentence may start, but a number or sign goes on.
        let short = text[at..].starts_with('.')
            && ABBREVIATIONS.contains(&word.as_str())
            && !(word == "kr" && text[at + 1..].trim_start().starts_with(char::is_alphabetic));
        if space && starts && !short {
            let end = text[at + 1..]
                .find(char::is_whitespace)
                .map_or(text.len(), |space| at + 1 + space);
            out.push(text[from..end].trim().to_owned());
            from = end;
        }
    }
    out.push(text[from..].trim().to_owned());
    out.retain(|sentence| !sentence.is_empty());
    out
}

/// The links of `text`: each label and address, and where they are.
fn links(text: &str) -> Vec<(std::ops::Range<usize>, &str, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(middle) = text[from..].find("](").map(|at| from + at) {
        from = middle + 2;
        let Some(open) = text[..middle].rfind('[') else {
            continue;
        };
        let mut depth = 1;
        let end = text[from..].char_indices().find_map(|(at, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(from + at)
        });
        let Some(end) = end else {
            break;
        };
        // Without a title: `(https://… "Title")`.
        let url = text[from..end]
            .split_whitespace()
            .next()
            .unwrap_or_default();
        let url = url.trim_matches(|c| c == '<' || c == '>');
        if url.starts_with("http://") || url.starts_with("https://") {
            out.push((open..end + 1, &text[open + 1..middle], url));
        }
        from = end;
    }
    out
}

fn all_links(text: &str) -> Vec<String> {
    links(text)
        .into_iter()
        .map(|(_, _, url)| url.to_owned())
        .collect()
}

fn link_words(text: &str) -> usize {
    links(text)
        .iter()
        .map(|(_, label, _)| label.split_whitespace().count())
        .sum()
}

/// `text` with its links' labels instead of them, and their addresses.
fn without_links(text: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut urls = Vec::new();
    let mut from = 0;
    for (range, label, url) in links(text) {
        if range.start < from {
            continue;
        }
        out.push_str(&text[from..range.start]);
        out.push_str(label);
        if !urls.contains(&url.to_owned()) {
            urls.push(url.to_owned());
        }
        from = range.end;
    }
    out.push_str(&text[from..]);
    let out = out.replace("**", "");
    let out = out.trim_start_matches(['#', '-', '*', '>', ' ']);
    (crate::extract::squash(out), urls)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuotesInput {
    pub pages: PathBuf,
    pub quotes: Vec<Quote>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Quote {
    pub text: String,
    pub urls: Vec<String>,
}

/// Whether each quote is on one of its pages.
pub fn quotes(input: &QuotesInput) -> Vec<bool> {
    input
        .quotes
        .iter()
        .map(|quote| {
            quote.urls.iter().any(|url| {
                pages::load(&input.pages, url)
                    .is_some_and(|page| contains_quote(&page.passages, &quote.text))
            })
        })
        .collect()
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Models leave out or change a word now and then, so not every word must match.
#[allow(clippy::cast_precision_loss)] // A few words.
fn contains_quote(passages: &[Passage], quote: &str) -> bool {
    let quote = words(quote);
    if quote.len() < 3 {
        return false;
    }
    let page: Vec<String> = passages
        .iter()
        .flat_map(|passage| words(&passage.text))
        .collect();
    let triples: HashSet<&[String]> = page.windows(3).collect();
    let found = quote
        .windows(3)
        .filter(|triple| triples.contains(triple))
        .count();
    found as f64 / (quote.len() - 2) as f64 >= QUOTE_MATCH
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const REPORT: &str = "\
# Answer

Qwen scored **87.1%** on it ([CORE](https://a.org/p(1)), up from 77.7%. Nothing cited here.
A second claim is that context was cleared early. [Paper](https://b.org/x)

- It costs $1.24 per question, e.g. for o3 [Slim](https://c.org/ \"T\") and [other](https://d.org).
```
[code](https://code.org) has no claims in it at all
```

## Sources
- [CORE](https://a.org/p(1)): what it contributed was this and that.
";

    fn found(report: &str) -> Vec<(String, Vec<String>)> {
        statements(report)
            .into_iter()
            .map(|statement| (statement.text, statement.urls))
            .collect()
    }

    fn claim(text: &str, urls: &[&str]) -> (String, Vec<String>) {
        (
            text.to_owned(),
            urls.iter().map(|url| (*url).to_owned()).collect(),
        )
    }

    #[test]
    fn finds_the_claims_and_their_sources() {
        assert_eq!(
            found(REPORT),
            [
                claim(
                    "Qwen scored 87.1% on it (CORE, up from 77.7%.",
                    &["https://a.org/p(1)"]
                ),
                claim(
                    "A second claim is that context was cleared early.",
                    &["https://b.org/x"]
                ),
                claim(
                    "It costs $1.24 per question, e.g. for o3 Slim and other.",
                    &["https://c.org/", "https://d.org"]
                ),
            ]
        );
    }

    /// The report that had 2 checked claims out of dozens: most of it was in a table and bullets.
    #[test]
    fn sentences_end_where_they_end() {
        assert_eq!(
            sentences(
                "It costs ca. 5.149 kr. 100 were sold. That is 5.699 kr. Proshop has it [A. B \
                 C](https://a.org/x.y). **Sold out.** Done"
            ),
            [
                "It costs ca. 5.149 kr. 100 were sold.",
                "That is 5.699 kr.",
                "Proshop has it [A. B C](https://a.org/x.y).",
                "**Sold out.**",
                "Done",
            ]
        );
    }

    #[test]
    fn tables_and_bullets_are_claims_too() {
        let report = "\
Here is what I found.

## Prices

| Model | Price | Store |
|---|---|---|
| PS5 Digital | 5.149 kr. | Komplett |
| **PS5 Pro** | 7.499 kr. | [Elgiganten](https://e.dk) |

Prices include VAT. Details: [PSGAMER – what it costs](https://p.dk/a) and [offers](https://p.dk/b).

## Stock
According to [POWER](https://power.dk):
- The Pro is in 12 stores
  - most of them in Copenhagen, 9 to be exact
- The Slim is sold out everywhere

1. Sony raised the price by 400 kr. in April.
2. It fell again in May [Sony](https://s.dk/x).

## Caveats
- No prices for 27 November 2026 are known yet.

**Sources**
- [PSGAMER](https://p.dk/a): 100 prices of everything

Shall I check 3 more stores for you?
";
        let psgamer = ["https://p.dk/a", "https://p.dk/b"];
        let power = ["https://power.dk"];
        assert_eq!(
            found(report),
            [
                claim("Here is what I found.", &[]),
                claim("PS5 Digital; Price: 5.149 kr.; Store: Komplett", &psgamer),
                claim(
                    "PS5 Pro; Price: 7.499 kr.; Store: Elgiganten",
                    &["https://e.dk"]
                ),
                claim("The Pro is in 12 stores", &power),
                claim("most of them in Copenhagen, 9 to be exact", &power),
                claim("The Slim is sold out everywhere", &power),
                claim("Sony raised the price by 400 kr. in April.", &power),
                claim("It fell again in May Sony.", &["https://s.dk/x"]),
                claim("No prices for 27 November 2026 are known yet.", &[]),
            ]
        );
    }

    #[test]
    fn statements_with_numbers_need_a_source() {
        let statement = |text: &str, caveat| Statement {
            text: text.into(),
            urls: Vec::new(),
            caveat,
        };
        let cited = Statement {
            urls: vec!["https://a.org".into()],
            ..statement("It costs 5.149 kr. at Komplett and 5.699 elsewhere.", false)
        };
        let bare = [
            // Says again what is backed above.
            statement("In short, 5149–5.699 kr. is what it costs.", false),
            statement("Nobody knows what 2027 brings.", true),
            statement("It is the better console for most.", false),
            statement("It costs 13.580 kr. at Føniks.", false),
            Statement {
                urls: vec!["https://unread.org".into()],
                ..statement("Another page says so as well.", false)
            },
        ];
        let found: Vec<_> = unsourced(&[&cited], &bare.iter().collect::<Vec<_>>())
            .into_iter()
            .map(|claim| (claim.text, claim.sure))
            .collect();
        assert_eq!(
            found,
            [
                ("It costs 13.580 kr. at Føniks.".to_owned(), true),
                ("Another page says so as well.".to_owned(), true),
                ("It is the better console for most.".to_owned(), false),
            ]
        );
    }

    #[test]
    fn claims_with_numbers_come_first_when_there_are_too_many() {
        let cited: Vec<_> = ["no number", "has 1", "none", "has 22"]
            .map(|text| Statement {
                text: text.into(),
                urls: vec!["https://a.org".into()],
                caveat: false,
            })
            .into();
        let cited: Vec<_> = cited.iter().collect();
        assert_eq!(pick(&cited, 3), [0, 1, 3]);
        assert_eq!(pick(&cited, 2), [1, 3]);
        assert_eq!(pick(&cited, 0), [0usize; 0]);
        assert_eq!(pick(&cited, 9), [0, 1, 2, 3]);
    }

    fn stored(text: &str, read: bool) -> StoredPage {
        StoredPage {
            final_url: "https://a.org/p".into(),
            title: "A page".into(),
            published: None,
            description: None,
            passages: vec![
                Passage {
                    heading: None,
                    text: "Something unrelated about apples and pears.".into(),
                },
                Passage {
                    heading: None,
                    text: text.into(),
                },
            ],
            cut_off: false,
            shown: Vec::new(),
            read,
        }
    }

    #[test]
    fn links_and_claims_are_checked_against_the_stored_pages() {
        let dir = tempfile::tempdir().unwrap();
        let page = stored("The harness reached an accuracy of 87.1 percent.", true);
        pages::store(dir.path(), "https://a.org/p", &page);
        let mut unread = stored("Nobody looked at this page.", false);
        unread.final_url = "https://b.org".into();
        pages::store(dir.path(), "https://b.org", &unread);
        let input = CheckInput {
            report: "It reached an accuracy of 87.1% [A](https://a.org/p#s). Another claim stands \
                     here [B](https://b.org). An invalid one is [C](https://nowhere.invalid/x)."
                .into(),
            pages: dir.path().into(),
            read: Vec::new(),
            max_claims: 10,
        };
        let checked = check(&Config::default(), &input);
        let states: Vec<_> = checked.links.iter().map(|link| link.state).collect();
        assert_eq!(
            states,
            [LinkState::Read, LinkState::Unread, LinkState::Broken],
            "{:?}",
            checked.links
        );
        // Pages that were not read are no sources, also if they exist.
        assert_eq!(checked.total_claims, 3);
        assert_eq!(checked.claims.len(), 1);
        assert!(
            checked.claims[0]
                .evidence
                .contains("accuracy of 87.1 percent")
        );
        assert_eq!(
            checked
                .unsourced
                .iter()
                .map(|claim| (claim.urls[0].as_str(), claim.sure))
                .collect::<Vec<_>>(),
            [("https://b.org", true), ("https://nowhere.invalid/x", true)]
        );
        let light = CheckInput {
            max_claims: 0,
            ..input
        };
        assert_eq!(check(&Config::default(), &light).claims, []);
    }

    #[test]
    fn quotes_are_looked_up_on_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let page = stored(
            "The harness reached an accuracy of 87.1 percent on it.",
            true,
        );
        pages::store(dir.path(), "https://a.org/p", &page);
        let found = |text: &str| {
            quotes(&QuotesInput {
                pages: dir.path().into(),
                quotes: vec![Quote {
                    text: text.into(),
                    urls: vec!["https://nowhere.org".into(), "https://a.org/p".into()],
                }],
            })[0]
        };
        assert!(found("the harness reached an Accuracy of 87.1 percent"));
        assert!(found("harness reached an accuracy of 87.1 percent overall"));
        assert!(!found("the harness reached an accuracy of 92.4 percent"));
        assert!(!found("87.1"));
        assert!(!found("something else entirely is on that page"));
    }
}
