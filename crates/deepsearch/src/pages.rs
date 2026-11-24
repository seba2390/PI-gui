//! Pages that were read, kept in a folder while a chat's pi runs: so that nothing is loaded or
//! shown twice, and reports can be checked against the whole page.

use std::fs;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rank::Passage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPage {
    /// Where the page really is, after redirects.
    pub final_url: String,
    pub title: String,
    pub published: Option<String>,
    pub description: Option<String>,
    pub passages: Vec<Passage>,
    pub cut_off: bool,
    /// The passages the model has in front of it.
    #[serde(default)]
    pub shown: Vec<usize>,
    /// The model read it; otherwise only a check did.
    #[serde(default)]
    pub read: bool,
}

/// `url` without what does not change the page: a fragment, or a trailing slash.
pub fn key(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url).trim();
    url.trim_end_matches('/').to_owned()
}

fn file(dir: &Path, url: &str) -> PathBuf {
    let mut hasher = std::hash::DefaultHasher::new();
    key(url).hash(&mut hasher);
    dir.join(format!("{:016x}.json", hasher.finish()))
}

pub fn load(dir: &Path, url: &str) -> Option<StoredPage> {
    serde_json::from_slice(&fs::read(file(dir, url)).ok()?).ok()
}

/// Keeps `page` under `url` and where it really is. Losing it only costs loading it again.
pub fn store(dir: &Path, url: &str, page: &StoredPage) {
    let Ok(json) = serde_json::to_vec(page) else {
        return;
    };
    for url in [url, page.final_url.as_str()] {
        let path = file(dir, url);
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if fs::write(&tmp, &json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_found_by_its_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let page = StoredPage {
            final_url: "https://example.com/new/".into(),
            title: "T".into(),
            published: None,
            description: None,
            passages: vec![Passage {
                heading: None,
                text: "text".into(),
            }],
            cut_off: false,
            shown: vec![0],
            read: true,
        };
        store(dir.path(), "https://example.com/old", &page);
        for url in [
            "https://example.com/old",
            "https://example.com/old#part",
            "https://example.com/new",
        ] {
            assert_eq!(load(dir.path(), url).as_ref(), Some(&page), "{url}");
        }
        assert_eq!(load(dir.path(), "https://example.com/other"), None);
    }
}
