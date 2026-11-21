//! Unified diffs of the working tree against HEAD (staged and unstaged changes combined).

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::parse;
use super::{ChangeKind, FileChange, RepoStatus, command, diff_base, discover_required};
use crate::{Location, WorkspaceError};

/// First characters of the line [`full_diff`] appends when its output was cut off.
pub const TRUNCATION_MARKER: &str = "[diff truncated:";

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// Arguments shared by every tracked-changes diff.
const DIFF_ARGS: &[&str] = &["diff", "--no-color", "--no-ext-diff", "--find-renames"];

/// Exit codes of `git diff --no-index`: 0 = identical, 1 = differences.
const NO_INDEX_OK: &[i32] = &[0, 1];

/// Unified diff (with `diff --git` headers) of one file, working tree versus HEAD.
///
/// `path` is either relative to the repository root (as in [`FileChange::path`]) or absolute.
/// Untracked files produce a full "new file" diff; renamed files show the rename. In a
/// repository without commits the diff is against the empty tree. Unchanged files yield an
/// empty string.
///
/// This costs three or four git invocations because it has to look the file up first; when a
/// [`RepoStatus`] is at hand, [`change_diff`] is cheaper.
pub fn file_diff(cwd: &Path, path: &Path) -> Result<String, WorkspaceError> {
    file_diff_at(&Location::Local, cwd, path)
}

/// [`file_diff`] for a folder at `loc`.
pub fn file_diff_at(loc: &Location, cwd: &Path, path: &Path) -> Result<String, WorkspaceError> {
    let head = discover_required(loc, cwd)?;
    let root = head.root.as_path();
    let rel = if loc.is_local() {
        relativize(root, path)?
    } else {
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    };
    let has_commits = head.head_short.is_some();

    // Tracked changes only (cheap), to learn whether the file was renamed.
    let tracked = command::run_ok(
        loc,
        root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=no",
            "--find-renames",
        ],
        &[0],
    )?;
    let entry = parse::parse_status(&tracked)
        .entries
        .into_iter()
        .find(|e| e.path == rel);
    let kind = match entry {
        Some(entry) => entry.kind,
        None if is_untracked(loc, root, &rel)? => ChangeKind::Untracked,
        None => ChangeKind::Modified,
    };
    diff_one(loc, root, has_commits, &rel, &kind)
}

/// Unified diff of one entry of a [`RepoStatus`]: like [`file_diff`], but a single git
/// invocation since the change kind is already known.
pub fn change_diff(repo: &RepoStatus, change: &FileChange) -> Result<String, WorkspaceError> {
    change_diff_at(&Location::Local, repo, change)
}

/// [`change_diff`] for a repository at `loc`.
pub fn change_diff_at(
    loc: &Location,
    repo: &RepoStatus,
    change: &FileChange,
) -> Result<String, WorkspaceError> {
    diff_one(
        loc,
        &repo.root,
        repo.has_commits(),
        &change.path,
        &change.kind,
    )
}

fn diff_one(
    loc: &Location,
    root: &Path,
    has_commits: bool,
    rel: &Path,
    kind: &ChangeKind,
) -> Result<String, WorkspaceError> {
    let out = if *kind == ChangeKind::Untracked {
        command::run_ok(loc, root, untracked_diff_args(rel), NO_INDEX_OK)?
    } else {
        let base = diff_base(loc, root, has_commits)?;
        let mut paths = vec![rel];
        if let ChangeKind::Renamed { from } = kind {
            // Both sides must be in the pathspec for git to pair them up as a rename.
            paths.insert(0, from.as_path());
        }
        command::run_ok(loc, root, tracked_diff_args(&base, &paths), &[0])?
    };
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Unified diff of every change in the repository containing `cwd`: tracked changes versus HEAD
/// followed by "new file" diffs of all untracked files.
///
/// Output is capped at `max_bytes`; when cut off it ends at a line boundary followed by a line
/// starting with [`TRUNCATION_MARKER`]. Git is stopped as soon as the cap is reached, so huge
/// diffs are never fully buffered.
pub fn full_diff(cwd: &Path, max_bytes: usize) -> Result<String, WorkspaceError> {
    full_diff_at(&Location::Local, cwd, max_bytes)
}

/// [`full_diff`] for a folder at `loc`.
pub fn full_diff_at(
    loc: &Location,
    cwd: &Path,
    max_bytes: usize,
) -> Result<String, WorkspaceError> {
    let head = discover_required(loc, cwd)?;
    let root = head.root.as_path();
    let base = diff_base(loc, root, head.head_short.is_some())?;

    let no_paths: [&Path; 0] = [];
    let (mut buf, mut truncated) = command::run_capped(
        loc,
        root,
        tracked_diff_args(&base, &no_paths),
        max_bytes,
        &[0],
    )?;

    if !truncated {
        let untracked = command::run_ok(
            loc,
            root,
            ["ls-files", "-z", "--others", "--exclude-standard"],
            &[0],
        )?;
        for rel in parse::parse_nul_paths(&untracked) {
            let remaining = max_bytes - buf.len();
            if remaining == 0 {
                truncated = true;
                break;
            }
            let (out, cut) =
                command::run_capped(loc, root, untracked_diff_args(&rel), remaining, NO_INDEX_OK)?;
            buf.extend_from_slice(&out);
            if cut {
                truncated = true;
                break;
            }
        }
    }

    if truncated {
        // Drop the partial last line so the diff stays parseable.
        let keep = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        buf.truncate(keep);
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        let _ = writeln!(
            text,
            "{TRUNCATION_MARKER} output exceeded {max_bytes} bytes]"
        );
    }
    Ok(text)
}

fn tracked_diff_args(base: &str, paths: &[&Path]) -> Vec<OsString> {
    let mut args: Vec<OsString> = DIFF_ARGS.iter().map(OsString::from).collect();
    args.push(base.into());
    args.push("--".into());
    args.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    args
}

fn untracked_diff_args(rel: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["diff", "--no-color", "--no-ext-diff", "--no-index", "--"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(NULL_DEVICE.into());
    args.push(rel.as_os_str().to_owned());
    args
}

fn is_untracked(loc: &Location, root: &Path, rel: &Path) -> Result<bool, WorkspaceError> {
    let args: [&OsStr; 6] = [
        "ls-files".as_ref(),
        "-z".as_ref(),
        "--others".as_ref(),
        "--exclude-standard".as_ref(),
        "--".as_ref(),
        rel.as_os_str(),
    ];
    let out = command::run_ok(loc, root, args, &[0])?;
    Ok(!out.is_empty())
}

/// Turns `path` into a path relative to `root`. Relative paths are taken as root-relative.
fn relativize(root: &Path, path: &Path) -> Result<PathBuf, WorkspaceError> {
    if path.is_relative() {
        return Ok(path.to_path_buf());
    }
    if let Ok(rel) = path.strip_prefix(root) {
        return Ok(rel.to_path_buf());
    }
    // Git reports the real path of the root (e.g. /private/var on macOS); try the real path of
    // `path` too. It may not exist (deleted file), so resolve its parent instead.
    let real_root = root.canonicalize()?;
    let real_path = path.canonicalize().ok().or_else(|| {
        let parent = path.parent()?.canonicalize().ok()?;
        Some(parent.join(path.file_name()?))
    });
    real_path
        .as_deref()
        .and_then(|p| p.strip_prefix(&real_root).ok())
        .map(Path::to_path_buf)
        .ok_or_else(|| WorkspaceError::OutsideRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativize_paths() {
        let root = Path::new("/repo");
        assert_eq!(
            relativize(root, Path::new("src/a.rs")).ok(),
            Some(PathBuf::from("src/a.rs"))
        );
        assert_eq!(
            relativize(root, Path::new("/repo/src/a.rs")).ok(),
            Some(PathBuf::from("src/a.rs"))
        );
    }

    #[test]
    fn tracked_args_put_paths_after_separator() {
        let args = tracked_diff_args("HEAD", &[Path::new("-weird"), Path::new("b")]);
        let rendered: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            rendered,
            [
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--find-renames",
                "HEAD",
                "--",
                "-weird",
                "b"
            ]
        );
    }
}
