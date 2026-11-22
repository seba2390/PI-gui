//! `status()`: changed files with line counts, in a constant number of git invocations.

use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::parse::{self, NumstatEntry};
use super::{FileChange, RepoStatus, command, diff_base, discover};
use crate::{Location, RemoteShell, WorkspaceError, with_args};

/// Untracked files larger than this get no line counts.
const MAX_COUNTED_FILE_BYTES: u64 = 1024 * 1024;

/// How far into a file to look for a NUL byte when deciding whether it is binary (as git does).
const BINARY_SNIFF_BYTES: usize = 8000;

/// Reads the status of the repository containing `cwd`.
///
/// Returns `Ok(None)` when `cwd` is not inside a git work tree, or when git is not installed
/// (logged as a warning). Runs `git rev-parse`, `git status --porcelain=v2` and
/// `git diff --numstat` (plus `git hash-object` in a repository without commits); line counts
/// for untracked files are read directly from disk.
pub fn status(cwd: &Path) -> Result<Option<RepoStatus>, WorkspaceError> {
    status_at(&Location::Local, cwd)
}

/// [`status`] for a folder at `loc`. On a remote machine, line counts of untracked files are
/// computed there in one round trip (for at most [`MAX_COUNTED`] files).
pub fn status_at(loc: &Location, cwd: &Path) -> Result<Option<RepoStatus>, WorkspaceError> {
    let head = match discover(loc, cwd) {
        Ok(Some(head)) => head,
        Ok(None) => return Ok(None),
        Err(WorkspaceError::GitNotFound) => {
            log::warn!("git is not installed; the diff view is unavailable");
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let root = head.root;

    let status_out = command::run_ok(
        loc,
        &root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--find-renames",
            "--ignore-submodules=dirty",
        ],
        &[0],
    )?;
    let parsed = parse::parse_status(&status_out);

    let base = diff_base(loc, &root, head.head_short.is_some())?;
    let numstat_out = command::run_ok(
        loc,
        &root,
        [
            "diff",
            "--numstat",
            "-z",
            "--find-renames",
            "--no-ext-diff",
            "--ignore-submodules=dirty",
            base.as_str(),
        ],
        &[0],
    )?;
    let stats = sum_numstat(parse::parse_numstat(&numstat_out));

    let remote_counts: HashMap<PathBuf, Option<u32>> = match loc {
        Location::Local => HashMap::new(),
        Location::Remote(shell) => {
            let untracked: Vec<&Path> = parsed
                .entries
                .iter()
                .filter(|entry| entry.kind == super::ChangeKind::Untracked)
                .map(|entry| entry.path.as_path())
                .take(MAX_COUNTED)
                .collect();
            count_lines_remote(shell.as_ref(), &root, &untracked)
        }
    };
    // Each count reads up to 1 MB; a fresh `node_modules` or venv without a `.gitignore`
    // can hold tens of thousands of untracked files.
    let mut local_counted = 0;
    let mut files: Vec<FileChange> = parsed
        .entries
        .into_iter()
        .map(|entry| {
            let (additions, deletions) = if entry.kind == super::ChangeKind::Untracked {
                let counted = match loc {
                    Location::Local if local_counted < MAX_COUNTED => {
                        local_counted += 1;
                        count_lines(&root.join(&entry.path))
                    }
                    Location::Local => None,
                    Location::Remote(_) => remote_counts.get(&entry.path).copied().flatten(),
                };
                match counted {
                    Some(lines) => (Some(lines), Some(0)),
                    None => (None, None),
                }
            } else {
                // Not in the numstat output (e.g. added then deleted in the working tree):
                // nothing differs from HEAD.
                stats
                    .get(&entry.path)
                    .copied()
                    .unwrap_or((Some(0), Some(0)))
            };
            FileChange {
                path: entry.path,
                kind: entry.kind,
                staged: entry.staged,
                unstaged: entry.unstaged,
                additions,
                deletions,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Some(RepoStatus {
        root,
        branch: parsed.branch,
        head_short: head.head_short,
        files,
    }))
}

type Counts = (Option<u32>, Option<u32>);

/// Untracked files whose lines are counted (per status load); the rest show no counts.
const MAX_COUNTED: usize = 500;

/// Counts lines of untracked files the way [`count_lines`] does, by one script on the remote
/// machine (NUL-separated paths on stdin, one count or `-` per line out). Failures only mean
/// no counts.
const COUNT_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || exit 1
xargs -0 sh -c 'for f; do
  if [ -L "$f" ]; then echo 1; continue; fi
  if [ ! -f "$f" ]; then echo -; continue; fi
  s=$(wc -c < "$f" | tr -d " ")
  if [ "$s" -gt 1048576 ]; then echo -; continue; fi
  a=$(head -c 8000 "$f" | wc -c | tr -d " ")
  b=$(head -c 8000 "$f" | tr -d "\000" | wc -c | tr -d " ")
  if [ "$a" != "$b" ]; then echo -; continue; fi
  n=$(wc -l < "$f" | tr -d " ")
  if [ -n "$(tail -c 1 "$f")" ]; then n=$((n + 1)); fi
  echo "$n"
done' sh"#;

fn count_lines_remote(
    shell: &dyn RemoteShell,
    root: &Path,
    paths: &[&Path],
) -> HashMap<PathBuf, Option<u32>> {
    if paths.is_empty() {
        return HashMap::new();
    }
    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.to_string_lossy().as_bytes());
        stdin.push(0);
    }
    let script = with_args(COUNT_SCRIPT, &[root.to_string_lossy()]);
    let Ok(output) = shell.run(&script, Some(stdin), None) else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    paths
        .iter()
        .zip(text.lines())
        .map(|(path, line)| (path.to_path_buf(), line.trim().parse().ok()))
        .collect()
}

/// Indexes numstat records by path, summing duplicates (a type change can be reported as a
/// deletion plus an addition of the same path). Binary on either side makes the sum binary.
fn sum_numstat(entries: Vec<NumstatEntry>) -> HashMap<PathBuf, Counts> {
    let mut map: HashMap<PathBuf, Counts> = HashMap::with_capacity(entries.len());
    for entry in entries {
        let counts = (entry.additions, entry.deletions);
        map.entry(entry.path)
            .and_modify(|acc| {
                acc.0 = acc.0.zip(counts.0).map(|(a, b)| a.saturating_add(b));
                acc.1 = acc.1.zip(counts.1).map(|(a, b)| a.saturating_add(b));
            })
            .or_insert(counts);
    }
    map
}

/// Counts lines like `git diff --numstat` would for a new file. `None` for binary files, files
/// over [`MAX_COUNTED_FILE_BYTES`], and anything unreadable. A symlink counts as one line (git
/// diffs its target path).
pub(crate) fn count_lines(path: &Path) -> Option<u32> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return Some(1);
    }
    if !meta.is_file() || meta.len() > MAX_COUNTED_FILE_BYTES {
        return None;
    }
    let mut data = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_COUNTED_FILE_BYTES)
        .read_to_end(&mut data)
        .ok()?;
    count_text_lines(&data)
}

/// Line count of `data`, or `None` if it looks binary.
fn count_text_lines(data: &[u8]) -> Option<u32> {
    let sniff = &data[..data.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return None;
    }
    #[expect(clippy::naive_bytecount, reason = "inputs are capped at 1 MB")]
    let newlines = data.iter().filter(|&&b| b == b'\n').count();
    let unterminated = usize::from(data.last().is_some_and(|&b| b != b'\n'));
    u32::try_from(newlines + unterminated).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remote code path (scripts run by a shell) gives the same status and diffs as
    /// running git locally.
    #[test]
    fn remote_status_and_diff_match_local() {
        use crate::location::testing::LocalSh;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        fs::write(root.join("tracked.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        fs::write(root.join("tracked.txt"), "one\ntwo\n").unwrap();
        fs::write(root.join("new file.txt"), "a\nb\nc").unwrap();
        fs::write(root.join("bin.dat"), b"x\0y").unwrap();

        let remote = Location::Remote(std::sync::Arc::new(LocalSh));
        let local = status(&root).unwrap().unwrap();
        let over_sh = status_at(&remote, &root).unwrap().unwrap();
        assert_eq!(over_sh, local);
        let new = local
            .files
            .iter()
            .find(|f| f.path == Path::new("new file.txt"))
            .unwrap();
        assert_eq!(new.additions, Some(3));
        let bin = local
            .files
            .iter()
            .find(|f| f.path == Path::new("bin.dat"))
            .unwrap();
        assert_eq!(bin.additions, None);
        for change in &local.files {
            assert_eq!(
                crate::git::change_diff_at(&remote, &over_sh, change).unwrap(),
                crate::git::change_diff(&local, change).unwrap()
            );
        }
        assert_eq!(
            crate::git::full_diff_at(&remote, &root, 1 << 20).unwrap(),
            crate::git::full_diff(&root, 1 << 20).unwrap()
        );
        // Outside a repository: no status, same as locally.
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(status_at(&remote, plain.path()).unwrap(), None);
        assert!(status_at(&remote, Path::new("/no/such/dir")).is_err());
    }

    #[test]
    fn text_line_counts() {
        assert_eq!(count_text_lines(b""), Some(0));
        assert_eq!(count_text_lines(b"one"), Some(1));
        assert_eq!(count_text_lines(b"one\n"), Some(1));
        assert_eq!(count_text_lines(b"one\ntwo"), Some(2));
        assert_eq!(count_text_lines(b"\n\n\n"), Some(3));
        assert_eq!(count_text_lines(b"bin\0ary\n"), None);
    }

    #[test]
    fn numstat_duplicates_are_summed() {
        let entries = vec![
            NumstatEntry {
                path: "a".into(),
                additions: Some(0),
                deletions: Some(3),
            },
            NumstatEntry {
                path: "a".into(),
                additions: Some(1),
                deletions: Some(0),
            },
            NumstatEntry {
                path: "b".into(),
                additions: Some(1),
                deletions: Some(1),
            },
            NumstatEntry {
                path: "b".into(),
                additions: None,
                deletions: None,
            },
        ];
        let map = sum_numstat(entries);
        assert_eq!(map[Path::new("a")], (Some(1), Some(3)));
        assert_eq!(map[Path::new("b")], (None, None));
    }
}
