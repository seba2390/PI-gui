//! Lazily listed, gitignore-aware directory contents for the "Files" tab.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::WorkspaceError;

/// One child of a listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// File name (lossily converted to UTF-8 for display; `path` keeps the exact name).
    pub name: String,
    /// Absolute path (`dir` joined with the file name).
    pub path: PathBuf,
    /// Whether this is a directory. For symlinks this describes the link target, so a symlink to
    /// a directory can be expanded; dangling links are not directories.
    pub is_dir: bool,
    /// Whether the entry itself is a symbolic link.
    pub is_symlink: bool,
}

/// Lists the immediate children of `dir`, which must be `root` or a directory inside it.
///
/// Respects `.gitignore`, `.ignore`, `.git/info/exclude` and the global git excludes file
/// (including ignore files in `root`'s ancestors), even when `root` is not a git repository.
/// `.git` is always hidden; other dotfiles only appear when `show_hidden` is set. The result
/// is sorted with directories first, then by case-insensitive natural name order (`file2`
/// before `file10`).
///
/// A relative `dir` is resolved against `root`.
pub fn list_dir(
    root: &Path,
    dir: &Path,
    show_hidden: bool,
) -> Result<Vec<DirEntry>, WorkspaceError> {
    let dir = if dir.is_relative() {
        root.join(dir)
    } else {
        dir.to_path_buf()
    };
    if !dir.starts_with(root) {
        return Err(WorkspaceError::OutsideRoot {
            path: dir,
            root: root.to_path_buf(),
        });
    }
    let meta = fs::metadata(&dir)?;
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("not a directory: {}", dir.display()),
        )
        .into());
    }

    let walker = WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .build();

    let mut entries = Vec::new();
    for result in walker {
        let entry = match result {
            Ok(entry) => entry,
            Err(err) => {
                log::debug!("skipping unreadable entry in {}: {err}", dir.display());
                continue;
            }
        };
        if entry.depth() == 0 {
            continue; // `dir` itself
        }
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let is_symlink = entry.path_is_symlink();
        let is_dir = if is_symlink {
            fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            entry.file_type().is_some_and(|t| t.is_dir())
        };
        entries.push(DirEntry {
            name: file_name.to_string_lossy().into_owned(),
            path: entry.path().to_path_buf(),
            is_dir,
            is_symlink,
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural_cmp(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

/// Case-insensitive comparison that orders embedded digit runs numerically.
///
/// Numbers that differ only in leading zeros compare equal until the very end, where fewer
/// leading zeros sorts first.
fn natural_cmp(mut a: &str, mut b: &str) -> Ordering {
    let mut zeros_tiebreak = Ordering::Equal;
    loop {
        let (Some(ca), Some(cb)) = (a.chars().next(), b.chars().next()) else {
            return (!a.is_empty()).cmp(&!b.is_empty()).then(zeros_tiebreak);
        };
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (da, rest_a) = a.split_at(a.bytes().take_while(u8::is_ascii_digit).count());
            let (db, rest_b) = b.split_at(b.bytes().take_while(u8::is_ascii_digit).count());
            let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
            let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
            if ord != Ordering::Equal {
                return ord;
            }
            zeros_tiebreak = zeros_tiebreak.then(da.len().cmp(&db.len()));
            (a, b) = (rest_a, rest_b);
        } else {
            let ord = ca.to_lowercase().cmp(cb.to_lowercase());
            if ord != Ordering::Equal {
                return ord;
            }
            (a, b) = (&a[ca.len_utf8()..], &b[cb.len_utf8()..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_by(|a, b| natural_cmp(a, b).then_with(|| a.cmp(b)));
        names
    }

    #[test]
    fn natural_order() {
        assert_eq!(
            sorted(vec!["file10", "file2", "File1", "file1", "a", "B", "c"]),
            vec!["a", "B", "c", "File1", "file1", "file2", "file10"]
        );
        assert_eq!(
            sorted(vec!["v1.10", "v1.9", "v1.09", "v01.9"]),
            vec!["v1.9", "v1.09", "v01.9", "v1.10"]
        );
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("ab", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("Ärger", "ärger"), Ordering::Equal);
        assert_eq!(
            natural_cmp("x99999999999999999999999", "x100000000000000000000000"),
            Ordering::Less
        );
    }
}
