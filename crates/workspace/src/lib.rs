//! UI-agnostic backend for PI-gui's right-hand panel.
//!
//! * [`git`] — repository status, per-file and whole-tree diffs (the "Diff" tab), implemented
//!   on top of the `git` CLI.
//! * [`files`] — lazily listed, gitignore-aware directory contents (the "Files" tab).
//! * [`watch`] — a debounced recursive filesystem watcher so the UI knows when to refresh.
//!
//! Everything here is synchronous and may block on disk or process I/O; callers are expected to
//! run it on a background executor.

mod error;
pub mod files;
pub mod git;
pub mod watch;

pub use error::WorkspaceError;
