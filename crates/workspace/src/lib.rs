//! UI-agnostic backend for Pistachio's right-hand panel.
//!
//! * [`git`] — repository status, per-file and whole-tree diffs (the "Diff" tab), implemented
//!   on top of the `git` CLI.
//! * [`files`] — lazily listed, gitignore-aware directory contents (the "Files" tab).
//! * [`watch`] — a debounced recursive filesystem watcher so the UI knows when to refresh.
//! * [`Location`] — whether the folder is on this computer or on another machine reached
//!   through a [`RemoteShell`] (SSH); `*_at` functions take one and run remotely as `sh`
//!   scripts there.
//!
//! Everything here is synchronous and may block on disk or process I/O; callers are expected to
//! run it on a background executor.

mod error;
pub mod files;
pub mod git;
mod location;
pub mod watch;

pub use error::WorkspaceError;
pub use location::{Location, RemoteShell, ShellOutput, sh_quote, with_args};
