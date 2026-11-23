//! What changed since the version that ran last: shown once after an update, from the
//! changelog built into the app (so it needs no network and reads like the changelog).

use std::fmt::Write as _;

/// The changelog, as it was when this version was built.
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

const ISSUES_URL: &str = "https://github.com/seba2390/pistachio/issues/";

/// Markdown for every changelog section newer than `after` up to `upto`, newest first, or
/// `None` when there is none.
pub fn since(after: &str, upto: &str) -> Option<String> {
    notes_between(CHANGELOG, after, upto)
}

fn notes_between(changelog: &str, after: &str, upto: &str) -> Option<String> {
    let mut out = Vec::new();
    for (version, date, body) in sections(changelog) {
        let newer = crate::updates::is_newer(version, after);
        let not_too_new = !crate::updates::is_newer(version, upto);
        if newer && not_too_new && !body.trim().is_empty() {
            let mut text = format!("## Pistachio {version}");
            if !date.is_empty() {
                let _ = write!(text, "\n\n*{date}*");
            }
            text.push_str("\n\n");
            text.push_str(&link_issues(body.trim()));
            out.push(text);
        }
    }
    (!out.is_empty()).then(|| out.join("\n\n"))
}

/// `(version, date, body)` of each released section (`## [0.4.4] - 2026-11-23`), in file
/// order. Link definitions (`[0.4.4]: https://…`) are not part of any body.
fn sections(changelog: &str) -> Vec<(&str, &str, String)> {
    let mut sections = Vec::new();
    let mut current: Option<(&str, &str, Vec<&str>)> = None;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some((version, date, body)) = current.take() {
                sections.push((version, date, body.join("\n")));
            }
            let Some((version, rest)) = heading
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
            else {
                continue;
            };
            if version.eq_ignore_ascii_case("unreleased") {
                continue;
            }
            let date = rest.trim().trim_start_matches(['-', '–']).trim();
            current = Some((version, date, Vec::new()));
        } else if let Some((_, _, body)) = current.as_mut() {
            let definition = line.starts_with('[') && line.contains("]: ");
            if !definition {
                body.push(line);
            }
        }
    }
    if let Some((version, date, body)) = current {
        sections.push((version, date, body.join("\n")));
    }
    sections
}

/// `(#26)` becomes a link to issue 26.
fn link_issues(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('#') {
        let (before, from_hash) = rest.split_at(at);
        out.push_str(before);
        let digits: String = from_hash[1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let starts_word = before
            .chars()
            .last()
            .is_none_or(|c| c == '(' || c.is_whitespace());
        let after = &from_hash[1 + digits.len()..];
        let ends_word = after.chars().next().is_none_or(|c| !c.is_alphanumeric());
        if !digits.is_empty() && starts_word && ends_word {
            let _ = write!(out, "[#{digits}]({ISSUES_URL}{digits})");
        } else {
            out.push_str(&from_hash[..=digits.len()]);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n- not yet\n\n\
                       ## [0.4.5] - 2026-12-01\n\n### Fixed\n\n- Relay paths (#26).\n\n\
                       ## [0.4.4] - 2026-11-23\n\n### Added\n\n- Models over SSH.\n\n\
                       ## [0.4.3] - 2026-11-20\n\n- Older.\n\n\
                       [Unreleased]: https://x/compare/v0.4.5...HEAD\n[0.4.5]: https://x/0.4.5\n";

    #[test]
    fn shows_the_versions_since_the_last_run() {
        let notes = notes_between(LOG, "0.4.3", "0.4.5").unwrap();
        assert_eq!(
            notes,
            "## Pistachio 0.4.5\n\n*2026-12-01*\n\n### Fixed\n\n- Relay paths \
             ([#26](https://github.com/seba2390/pistachio/issues/26)).\n\n\
             ## Pistachio 0.4.4\n\n*2026-11-23*\n\n### Added\n\n- Models over SSH."
        );
        assert_eq!(notes_between(LOG, "0.4.5", "0.4.5"), None);
        // Not past the running version, and never the unreleased section.
        let notes = notes_between(LOG, "0.4.3", "0.4.4").unwrap();
        assert!(notes.starts_with("## Pistachio 0.4.4") && !notes.contains("0.4.5"));
        assert!(!notes.contains("not yet") && !notes.contains("https://x"));
    }

    #[test]
    fn links_issue_numbers_only() {
        assert_eq!(
            link_issues("fixed (#26), #3 and C# or a#1 #x"),
            "fixed ([#26](https://github.com/seba2390/pistachio/issues/26)), \
             [#3](https://github.com/seba2390/pistachio/issues/3) and C# or a#1 #x"
        );
    }

    #[test]
    fn the_built_in_changelog_has_released_sections() {
        assert!(
            sections(CHANGELOG)
                .iter()
                .any(|(version, _, _)| *version == "0.4.4")
        );
    }
}
