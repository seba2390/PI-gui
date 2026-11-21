//! Git status and diffs for the "Diff" tab, via the `git` CLI.
//!
//! All commands run with `GIT_OPTIONAL_LOCKS=0`, so reading status never takes the index lock
//! while pi is working in the same repository. Output is requested in NUL-separated porcelain
//! formats and parsed by the pure functions in `parse`.

mod command;
mod diff;
mod parse;
mod status;

use std::io;
use std::path::{Path, PathBuf};

pub use diff::{
    TRUNCATION_MARKER, change_diff, change_diff_at, file_diff, file_diff_at, full_diff,
    full_diff_at,
};
pub use status::{status, status_at};

use crate::{Location, WorkspaceError};

/// Snapshot of a repository's working-tree state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
    /// Top-level directory of the work tree (`git rev-parse --show-toplevel`).
    pub root: PathBuf,
    /// Current branch name; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated sha of HEAD; `None` when the repository has no commits yet.
    pub head_short: Option<String>,
    /// Changed files, sorted by path.
    pub files: Vec<FileChange>,
}

impl RepoStatus {
    /// Whether HEAD points at a commit (false for a freshly initialised repository).
    pub fn has_commits(&self) -> bool {
        self.head_short.is_some()
    }

    /// Total `(additions, deletions)` over all files with known (non-binary) counts.
    pub fn totals(&self) -> (u64, u64) {
        self.files.iter().fold((0, 0), |(add, del), f| {
            (
                add + u64::from(f.additions.unwrap_or(0)),
                del + u64::from(f.deletions.unwrap_or(0)),
            )
        })
    }
}

/// One changed path in a [`RepoStatus`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// What kind of change this is.
    pub kind: ChangeKind,
    /// The index differs from HEAD for this path.
    pub staged: bool,
    /// The working tree differs from the index for this path (always true for untracked files).
    pub unstaged: bool,
    /// Added lines versus HEAD (staged and unstaged combined); `None` for binary files, and for
    /// untracked files over 1 MB.
    pub additions: Option<u32>,
    /// Deleted lines versus HEAD (staged and unstaged combined); `None` when `additions` is.
    pub deletions: Option<u32>,
}

/// Kind of change for a [`FileChange`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    /// New file in the index (including intent-to-add and copies).
    Added,
    /// Content changed.
    Modified,
    /// Removed from the index or the working tree.
    Deleted,
    /// Renamed in the index; `from` is the old path relative to the repository root.
    Renamed {
        /// Previous path, relative to the repository root.
        from: PathBuf,
    },
    /// Not tracked and not ignored.
    Untracked,
    /// Unmerged (merge/rebase conflict).
    Conflicted,
    /// File type changed (e.g. regular file to symlink).
    TypeChanged,
}

impl ChangeKind {
    /// Single-letter badge in the style of `git status --short` (`?` for untracked).
    pub fn letter(&self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed { .. } => 'R',
            Self::Untracked => '?',
            Self::Conflicted => 'U',
            Self::TypeChanged => 'T',
        }
    }
}

/// Where a work tree lives and what HEAD is.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    pub root: PathBuf,
    pub head_short: Option<String>,
}

/// Finds the work tree containing `cwd`. `Ok(None)` when `cwd` is not inside a work tree.
pub(crate) fn discover(loc: &Location, cwd: &Path) -> Result<Option<Head>, WorkspaceError> {
    if loc.is_local() && !cwd.is_dir() {
        // Spawning in a missing directory would otherwise look like "git not found".
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("not a directory: {}", cwd.display()),
        )
        .into());
    }
    let output = command::run(
        loc,
        cwd,
        [
            "rev-parse",
            "--show-toplevel",
            "--verify",
            "-q",
            "--short",
            "HEAD",
        ],
    )?;
    // Exit 0: HEAD resolved. Exit 1 (with the toplevel printed): no commits yet.
    if matches!(output.status.code(), Some(0 | 1))
        && let Some(parsed) = parse::parse_rev_parse(&output.stdout)
    {
        return Ok(Some(Head {
            root: parsed.toplevel,
            head_short: parsed.head_short,
        }));
    }
    let stderr = output.stderr.to_ascii_lowercase();
    if stderr.contains("not a git repository") || stderr.contains("must be run in a work tree") {
        return Ok(None);
    }
    Err(output.into_error())
}

/// Like [`discover`], but not being in a repository is an error.
pub(crate) fn discover_required(loc: &Location, cwd: &Path) -> Result<Head, WorkspaceError> {
    discover(loc, cwd)?.ok_or_else(|| WorkspaceError::NotARepo(cwd.to_path_buf()))
}

/// The tree to diff the working tree against: `HEAD`, or the empty tree when there are no
/// commits yet (computed, so SHA-256 repositories work too).
pub(crate) fn diff_base(
    loc: &Location,
    root: &Path,
    has_commits: bool,
) -> Result<String, WorkspaceError> {
    if has_commits {
        return Ok("HEAD".to_owned());
    }
    let out = command::run_ok(loc, root, ["hash-object", "-t", "tree", "--stdin"], &[0])?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}
