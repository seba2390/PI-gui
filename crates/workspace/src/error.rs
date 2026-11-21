//! Error type shared by all modules of the crate.

use std::path::PathBuf;

use thiserror::Error;

/// Failure modes of the workspace backend.
#[derive(Debug, Error)]
pub enum WorkspaceError {
    /// A git command ran but exited unsuccessfully.
    #[error("`git {args}` failed ({status}): {stderr}")]
    Git {
        /// The arguments the command was run with (lossily rendered, for diagnostics).
        args: String,
        /// Human-readable exit status (e.g. `exit status: 128`).
        status: String,
        /// Trimmed stderr output of the command.
        stderr: String,
    },

    /// The `git` executable could not be found on `PATH`.
    #[error("git is not installed or not on PATH")]
    GitNotFound,

    /// The given directory is not inside a git work tree.
    #[error("not inside a git work tree: {}", .0.display())]
    NotARepo(PathBuf),

    /// A path that must lie inside a root directory does not.
    #[error("{} is not inside {}", path.display(), root.display())]
    OutsideRoot {
        /// The offending path.
        path: PathBuf,
        /// The root it was expected to be inside of.
        root: PathBuf,
    },

    /// The filesystem watcher could not be created or attached.
    #[error("file watcher error: {0}")]
    Watch(#[from] notify::Error),

    /// Any other I/O failure (spawning processes, reading files and directories).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
