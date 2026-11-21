//! Lazily listed, gitignore-aware directory contents for the "Files" tab.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::{Location, RemoteShell, WorkspaceError, with_args};

/// Lists one folder on a remote machine: `t l name` records (`t` = `d`/`f`, `l` = 1 for a
/// symlink), NUL-terminated, then an empty record, then the names `git check-ignore` reports
/// as ignored (folders with a trailing `/`). `$2` = 1 shows dotfiles.
const LIST_DIR_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || { printf 'No such folder: %s\n' "$1" >&2; exit 1; }
rec() { t=f; [ -d "$1" ] && t=d; l=0; [ -L "$1" ] && l=1; printf '%s%s%s\000' "$t" "$l" "$1"; }
name() { if [ -d "$1" ] && [ ! -L "$1" ]; then printf '%s/\000' "$1"; else printf '%s\000' "$1"; fi; }
h=$2; set --
for e in * .*; do
  case $e in .|..|.git) continue ;; esac
  [ -e "$e" ] || [ -L "$e" ] || continue
  if [ "$h" != 1 ]; then case $e in .*) continue ;; esac; fi
  set -- "$@" "$e"
done
for e in "$@"; do rec "$e"; done
printf '\000'
if [ $# -gt 0 ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  for e in "$@"; do name "$e"; done | git check-ignore -z --stdin 2>/dev/null
fi
exit 0"#;

/// Every file under `$1` (gitignore-aware inside a work tree), NUL-separated, relative.
const LIST_FILES_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || exit 1
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git -c core.quotepath=off ls-files -z --cached --others --exclude-standard 2>/dev/null
else
  find . -mindepth 1 \( -name '.*' -o -name node_modules \) -prune -o -type f -print0 2>/dev/null
fi
exit 0"#;

/// Bytes of file paths read from a remote machine at most (about 100k paths).
const MAX_REMOTE_INDEX_BYTES: usize = 8 * 1024 * 1024;

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
    list_dir_at(&Location::Local, root, dir, show_hidden)
}

/// [`list_dir`] for a folder at `loc`. On a remote machine `.gitignore` rules apply through
/// `git check-ignore` (inside a work tree only), and symlinks to folders are expandable.
pub fn list_dir_at(
    loc: &Location,
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
    if let Location::Remote(shell) = loc {
        let mut entries = list_dir_remote(shell.as_ref(), &dir, show_hidden)?;
        sort_entries(&mut entries);
        return Ok(entries);
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

    sort_entries(&mut entries);
    Ok(entries)
}

/// Folders first, then case-insensitive natural name order.
fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural_cmp(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn list_dir_remote(
    shell: &dyn RemoteShell,
    dir: &Path,
    show_hidden: bool,
) -> Result<Vec<DirEntry>, WorkspaceError> {
    let dir_text = dir.to_string_lossy();
    let script = with_args(
        LIST_DIR_SCRIPT,
        &[dir_text.as_ref(), if show_hidden { "1" } else { "0" }],
    );
    let output = shell
        .run(&script, None, None)
        .map_err(|error| WorkspaceError::Remote(error.to_string()))?;
    if output.code != Some(0) {
        let message = output.stderr.trim();
        return Err(WorkspaceError::Remote(if message.is_empty() {
            format!("could not list {dir_text}")
        } else {
            message.to_owned()
        }));
    }
    Ok(parse_remote_listing(dir, &output.stdout))
}

/// Parses [`LIST_DIR_SCRIPT`]'s output into entries of `dir`, minus the ignored ones.
fn parse_remote_listing(dir: &Path, stdout: &[u8]) -> Vec<DirEntry> {
    let mut records = stdout.split(|&byte| byte == 0);
    let mut entries = Vec::new();
    for record in records.by_ref() {
        if record.is_empty() {
            break;
        }
        if record.len() < 3 {
            continue;
        }
        let name = String::from_utf8_lossy(&record[2..]).into_owned();
        entries.push(DirEntry {
            path: dir.join(&name),
            name,
            is_dir: record[0] == b'd',
            is_symlink: record[1] == b'1',
        });
    }
    let ignored: std::collections::HashSet<String> = records
        .filter(|record| !record.is_empty())
        .map(|record| {
            String::from_utf8_lossy(record)
                .trim_end_matches('/')
                .to_owned()
        })
        .collect();
    entries.retain(|entry| !ignored.contains(&entry.name));
    entries
}

/// Every file under `root` (recursively), as `/`-separated paths relative to `root`, for quick
/// fuzzy search. Same ignore rules as [`list_dir`] with hidden files excluded; symlinks are not
/// followed. Stops after `limit` files; the flag reports whether the walk was cut short.
///
/// Order is the walker's (directory by directory), not sorted: callers rank results anyway.
pub fn list_files(root: &Path, limit: usize) -> (Vec<String>, bool) {
    list_files_at(&Location::Local, root, limit)
}

/// [`list_files`] for a folder at `loc`. On a remote machine this is `git ls-files` inside a
/// work tree (so `.gitignore` applies), else `find` without dotfiles and `node_modules`.
pub fn list_files_at(loc: &Location, root: &Path, limit: usize) -> (Vec<String>, bool) {
    if let Location::Remote(shell) = loc {
        return list_files_remote(shell.as_ref(), root, limit);
    }
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if files.len() == limit {
            return (files, true);
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let mut text = relative.to_string_lossy().into_owned();
        if std::path::MAIN_SEPARATOR != '/' {
            text = text.replace(std::path::MAIN_SEPARATOR, "/");
        }
        files.push(text);
    }
    (files, false)
}

fn list_files_remote(shell: &dyn RemoteShell, root: &Path, limit: usize) -> (Vec<String>, bool) {
    let script = with_args(LIST_FILES_SCRIPT, &[root.to_string_lossy()]);
    let output = match shell.run(&script, None, Some(MAX_REMOTE_INDEX_BYTES)) {
        Ok(output) => output,
        Err(error) => {
            log::debug!("could not index {}: {error}", root.display());
            return (Vec::new(), false);
        }
    };
    parse_remote_files(&output.stdout, output.truncated, limit)
}

/// NUL-separated relative paths (a partial last one when `truncated`) to the index: hidden
/// paths dropped, `./` stripped, at most `limit`.
fn parse_remote_files(stdout: &[u8], truncated: bool, limit: usize) -> (Vec<String>, bool) {
    let mut fields: Vec<&[u8]> = stdout.split(|&byte| byte == 0).collect();
    if truncated {
        fields.pop();
    }
    let mut files = Vec::new();
    for field in fields {
        if field.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(field);
        let text = text.strip_prefix("./").unwrap_or(&text);
        if text.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        if files.len() == limit {
            return (files, true);
        }
        files.push(text.to_owned());
    }
    (files, truncated)
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
    use crate::location::testing::LocalSh;
    use std::sync::Arc;

    #[test]
    fn remote_listing_matches_local_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        for d in ["src", "target", "it's a dir", ".hidden"] {
            fs::create_dir(root.join(d)).unwrap();
        }
        for f in ["b.txt", "a10.txt", "a2.txt", "x.log", "src/lib.rs"] {
            fs::write(root.join(f), "x").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("src"), root.join("link")).unwrap();

        let remote = Location::Remote(Arc::new(LocalSh));
        let names = |entries: Vec<DirEntry>| -> Vec<(String, bool)> {
            entries.into_iter().map(|e| (e.name, e.is_dir)).collect()
        };
        let local = names(list_dir(&root, &root, false).unwrap());
        let over_sh = names(list_dir_at(&remote, &root, &root, false).unwrap());
        assert_eq!(over_sh, local);
        assert!(over_sh.iter().any(|(name, dir)| name == "link" && *dir));
        assert!(
            !over_sh
                .iter()
                .any(|(name, _)| name == "target" || name == "x.log")
        );

        let hidden = names(list_dir_at(&remote, &root, &root, true).unwrap());
        assert!(hidden.iter().any(|(name, _)| name == ".hidden"));
        assert!(!hidden.iter().any(|(name, _)| name == ".git"));

        let error = list_dir_at(&remote, &root, &root.join("nope"), false).unwrap_err();
        assert!(error.to_string().contains("No such folder"), "{error}");

        let (mut files, cut) = list_files_at(&remote, &root, 100);
        // git lists an untracked symlink as a path of its own; the local walker skips links.
        files.retain(|file| file != "link");
        files.sort();
        assert!(!cut);
        assert_eq!(files, ["a10.txt", "a2.txt", "b.txt", "src/lib.rs"]);
        let (files, cut) = list_files_at(&remote, &root, 2);
        assert_eq!((files.len(), cut), (2, true));
    }

    #[test]
    fn remote_file_index_parsing() {
        assert_eq!(
            parse_remote_files(b"./a\0.b/c\0d/e\0par", true, 10),
            (vec!["a".to_owned(), "d/e".to_owned()], true)
        );
        assert_eq!(
            parse_remote_files(b"a\0b\0", false, 10),
            (vec!["a".to_owned(), "b".to_owned()], false)
        );
    }

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
