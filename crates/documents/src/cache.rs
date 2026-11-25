//! What downloaded parsers read lately. pi's `read` returns a long document in parts, and each
//! part would parse all of it again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

const MAX_FILES: usize = 24;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A parser that failed gets another chance after this long.
const RETRY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Entry {
    Markdown(String),
    Failed(String),
}

fn folder(dir: &Path) -> PathBuf {
    dir.join("cache")
}

/// Where the result for a document is kept: by its contents and what read it.
fn file(dir: &Path, document: &[u8], revision: &str) -> PathBuf {
    let key = format!("{} {revision}", python_env::fetch::sha256(document));
    let key = python_env::fetch::sha256(key.as_bytes());
    folder(dir).join(format!("{}.json", &key[..32]))
}

pub fn load(dir: &Path, document: &[u8], revision: &str) -> Option<Entry> {
    let path = file(dir, document, revision);
    let entry = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    if matches!(entry, Entry::Failed(_)) {
        let age = SystemTime::now().duration_since(fs::metadata(&path).ok()?.modified().ok()?);
        if age.is_ok_and(|age| age > RETRY) {
            return None;
        }
    }
    Some(entry)
}

/// Keeps `entry`. Losing it only costs reading the document again.
pub fn store(dir: &Path, document: &[u8], revision: &str, entry: &Entry) {
    let path = file(dir, document, revision);
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let Ok(json) = serde_json::to_vec(entry) else {
        return;
    };
    let written = fs::create_dir_all(folder(dir))
        .and_then(|()| write_private(&tmp, &json))
        .and_then(|()| fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    trim(&folder(dir), MAX_FILES, MAX_BYTES);
}

/// Readable only by the user: it is the text of their documents.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Deletes the oldest files beyond the limits.
pub fn trim(folder: &Path, max_files: usize, max_bytes: u64) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    files.sort();
    let mut bytes = 0;
    for (kept, (_, size, path)) in files.into_iter().rev().enumerate() {
        bytes += size;
        if kept >= max_files || bytes > max_bytes {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn clear(dir: &Path) -> io::Result<()> {
    python_env::remove(&folder(dir))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn a_document_is_found_by_its_contents_and_what_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert_eq!(load(dir, b"pdf", "docling 1"), None);
        let entry = Entry::Markdown("# Hi".into());
        store(dir, b"pdf", "docling 1", &entry);
        assert_eq!(load(dir, b"pdf", "docling 1"), Some(entry));
        assert_eq!(load(dir, b"pdf", "docling 2"), None);
        assert_eq!(load(dir, b"other", "docling 1"), None);
        clear(dir).unwrap();
        assert_eq!(load(dir, b"pdf", "docling 1"), None);
        clear(dir).unwrap();
    }

    #[test]
    fn a_failure_is_forgotten_after_a_while() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let entry = Entry::Failed("it took longer than 2 minutes".into());
        store(dir, b"pdf", "x", &entry);
        assert_eq!(load(dir, b"pdf", "x"), Some(entry));
        let path = file(dir, b"pdf", "x");
        let long_ago = SystemTime::now() - 2 * RETRY;
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(long_ago).unwrap();
        assert_eq!(load(dir, b"pdf", "x"), None);
    }

    #[test]
    fn the_oldest_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        for (age, name) in [(3, "a"), (2, "b"), (1, "c")] {
            let path = dir.path().join(name);
            fs::write(&path, "12345").unwrap();
            let time = SystemTime::now() - Duration::from_secs(age * 60);
            let file = fs::File::options().write(true).open(path).unwrap();
            file.set_modified(time).unwrap();
        }
        let left = || fs::read_dir(dir.path()).unwrap().count();
        trim(dir.path(), 3, 100);
        assert_eq!(left(), 3);
        trim(dir.path(), 3, 12);
        assert_eq!(left(), 2);
        assert!(!dir.path().join("a").exists());
        trim(dir.path(), 1, 100);
        assert_eq!(left(), 1);
        assert!(dir.path().join("c").exists());
    }
}
