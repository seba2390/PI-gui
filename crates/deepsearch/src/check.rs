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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Checked {
    pub links: Vec<Link>,
    /// The claims to judge, in the order of the report.
    pub claims: Vec<Claim>,
    /// How many claims of the report there are.
    pub total_claims: usize,
}

/// A sentence of the report with links to its sources.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cited {
    text: String,
    urls: Vec<String>,
}

pub fn check(config: &Config, input: &CheckInput) -> Checked {
    let cited = citations(&input.report);
    let mut urls: Vec<String> = Vec::new();
    for url in cited
        .iter()
        .flat_map(|cited| &cited.urls)
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
    let links = urls
        .iter()
        .zip(&loaded)
        .map(|(url, page)| link(url, page, &read))
        .collect();
    let by_key: HashMap<String, &StoredPage> = urls
        .iter()
        .zip(&loaded)
        .filter_map(|(url, page)| Some((pages::key(url), page.as_ref().ok()?)))
        .collect();

    let total_claims = cited.len();
    let claims = pick(&cited, input.max_claims)
        .into_iter()
        .filter_map(|index| {
            let claim = &cited[index];
            let mut evidence = String::new();
            for url in &claim.urls {
                if let Some(page) = by_key.get(&pages::key(url)) {
                    let budget = EVIDENCE_CHARS / claim.urls.len();
                    let passages = passages_for(page, &claim.text, budget);
                    let _ = writeln!(evidence, "From {url} ({}):\n{passages}", page.title);
                }
            }
            // Nothing to judge it by: the link's own state says so.
            (!evidence.is_empty()).then(|| Claim {
                id: index + 1,
                text: claim.text.clone(),
                urls: claim.urls.clone(),
                evidence,
            })
        })
        .collect();
    Checked {
        links,
        claims,
        total_claims,
    }
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
fn pick(cited: &[Cited], max: usize) -> Vec<usize> {
    let has_number = |index: &usize| cited[*index].text.chars().any(|c| c.is_ascii_digit());
    let (mut picked, rest): (Vec<usize>, Vec<usize>) = (0..cited.len()).partition(has_number);
    picked.extend(rest);
    picked.truncate(max);
    picked.sort_unstable();
    picked
}

/// The sentences of `report` that cite a source, up to its list of sources.
fn citations(report: &str) -> Vec<Cited> {
    let mut out: Vec<Cited> = Vec::new();
    let mut code = false;
    for line in report.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            code = !code;
        }
        if code || line.starts_with("```") {
            continue;
        }
        if is_sources_heading(line) {
            break;
        }
        let mut in_line = 0;
        for sentence in sentences(line) {
            let (text, urls) = without_links(&sentence);
            if urls.is_empty() {
                // A link after the full stop belongs to the sentence before it.
                out.push(Cited { text, urls });
                in_line += 1;
                continue;
            }
            let words = text.split_whitespace().count();
            let labels = link_words(&sentence);
            if words.saturating_sub(labels) < 3 && in_line > 0 {
                if let Some(last) = out.last_mut() {
                    last.urls.extend(urls);
                }
                continue;
            }
            out.push(Cited { text, urls });
            in_line += 1;
        }
    }
    out.retain(|cited| !cited.urls.is_empty() && cited.text.split_whitespace().count() >= 4);
    out
}

fn is_sources_heading(line: &str) -> bool {
    let title = line
        .trim_matches(|c: char| !c.is_alphabetic())
        .to_lowercase();
    let heading = line.starts_with('#') || line.starts_with("**") || line == "Sources";
    heading
        && matches!(
            title.as_str(),
            "sources" | "references" | "bibliography" | "source list"
        )
}

/// Splits a line at the ends of sentences, but not inside links.
fn sentences(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let chars: Vec<char> = line.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        current.push(c);
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            '.' | '!' | '?' if depth == 0 => {
                let space = chars.get(index + 1).is_none_or(|next| next.is_whitespace());
                let starts = chars.get(index + 2).is_none_or(|next| !next.is_lowercase());
                if space && starts {
                    out.push(std::mem::take(&mut current).trim().to_owned());
                }
            }
            _ => {}
        }
    }
    out.push(current.trim().to_owned());
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

    #[test]
    fn finds_the_claims_and_their_sources() {
        let found: Vec<_> = citations(REPORT)
            .into_iter()
            .map(|cited| (cited.text, cited.urls))
            .collect();
        let claim = |text: &str, urls: &[&str]| {
            (
                text.to_owned(),
                urls.iter().map(|url| (*url).to_owned()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            found,
            [
                claim(
                    "Qwen scored 87.1% on it (CORE, up from 77.7%. Nothing cited here.",
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

    #[test]
    fn claims_with_numbers_come_first_when_there_are_too_many() {
        let cited: Vec<_> = ["no number", "has 1", "none", "has 22"]
            .map(|text| Cited {
                text: text.into(),
                urls: vec!["https://a.org".into()],
            })
            .into();
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
        // The one without a page has nothing to judge it by.
        assert_eq!(checked.total_claims, 3);
        assert_eq!(checked.claims.len(), 2);
        assert!(
            checked.claims[0]
                .evidence
                .contains("accuracy of 87.1 percent")
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
