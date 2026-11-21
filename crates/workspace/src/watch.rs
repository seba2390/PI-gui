//! Debounced recursive filesystem watcher.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::WorkspaceError;

/// Paths inside `.git/` whose changes affect what the diff view shows. `refs` matches the whole
/// subtree.
const GIT_TRIGGERS: &[&str] = &["HEAD", "index", "packed-refs", "refs"];

/// The longest a burst of events can postpone a flush, as a multiple of the debounce.
const MAX_WAIT_FACTOR: u32 = 4;

enum Message {
    Event(notify::Result<Event>),
    Stop,
}

/// Watches a directory tree and reports changed paths in debounced batches.
///
/// Events inside `.git/` are dropped, except changes to `.git/HEAD`, `.git/index`,
/// `.git/packed-refs` and `.git/refs/**`, which are reported as the `.git` directory itself.
/// Gitignored paths are reported like any other. Reported paths are absolute and start with the
/// `root` passed to [`Watcher::new`] (even if the OS reports a canonicalised path).
///
/// Dropping the watcher stops watching and waits for the background thread to finish (including
/// a callback that is currently running).
pub struct Watcher {
    root: PathBuf,
    backend: Option<RecommendedWatcher>,
    tx: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching `root` recursively.
    ///
    /// `on_change` is called from a background thread with the sorted, de-duplicated set of
    /// changed paths once `debounce` has passed without new events. A constant stream of events
    /// is still flushed after at most four times `debounce`.
    pub fn new(
        root: &Path,
        debounce: Duration,
        on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> Result<Self, WorkspaceError> {
        let filter = PathFilter::new(root)?;
        let (tx, rx) = mpsc::channel();

        let event_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            // The receiver is gone only during shutdown.
            let _ = event_tx.send(Message::Event(event));
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        let thread = thread::Builder::new()
            .name("workspace-watch".to_owned())
            .spawn(move || debounce_loop(&rx, &filter, debounce, &on_change))?;

        Ok(Self {
            root: root.to_path_buf(),
            backend: Some(watcher),
            tx,
            thread: Some(thread),
        })
    }

    /// The watched directory, as passed to [`Watcher::new`].
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl fmt::Debug for Watcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watcher")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Stop OS notifications first so nothing new is queued, then wake the thread.
        drop(self.backend.take());
        let _ = self.tx.send(Message::Stop);
        if let Some(handle) = self.thread.take() {
            // Joining from inside the callback (dropping the watcher in `on_change`) would
            // deadlock; the loop exits on its own after the callback returns.
            if handle.thread().id() != thread::current().id() {
                let _ = handle.join();
            }
        }
    }
}

fn debounce_loop(
    rx: &Receiver<Message>,
    filter: &PathFilter,
    debounce: Duration,
    on_change: &dyn Fn(Vec<PathBuf>),
) {
    let max_wait = debounce.saturating_mul(MAX_WAIT_FACTOR);
    let mut pending = BTreeSet::new();
    loop {
        // Idle: block until the first relevant event.
        match rx.recv() {
            Ok(Message::Event(event)) => {
                filter.collect(event, &mut pending);
            }
            Ok(Message::Stop) | Err(_) => return,
        }
        if pending.is_empty() {
            continue;
        }

        // Burst: wait for `debounce` of quiet (irrelevant events don't count), capped.
        let hard_deadline = Instant::now() + max_wait;
        let mut quiet_deadline = Instant::now() + debounce;
        loop {
            let now = Instant::now();
            let deadline = quiet_deadline.min(hard_deadline);
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(Message::Event(event)) => {
                    if filter.collect(event, &mut pending) {
                        quiet_deadline = Instant::now() + debounce;
                    }
                }
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        on_change(std::mem::take(&mut pending).into_iter().collect());
    }
}

/// Maps raw notify events to the paths we report.
struct PathFilter {
    root: PathBuf,
    /// Canonical form of `root`; `FSEvents` reports paths with symlinks resolved.
    canonical_root: PathBuf,
    git_dir: PathBuf,
}

impl PathFilter {
    fn new(root: &Path) -> Result<Self, WorkspaceError> {
        Ok(Self {
            root: root.to_path_buf(),
            canonical_root: root.canonicalize()?,
            git_dir: root.join(".git"),
        })
    }

    /// Adds the reportable paths of `event` to `out`. Returns whether the event was relevant.
    fn collect(&self, event: notify::Result<Event>, out: &mut BTreeSet<PathBuf>) -> bool {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                log::warn!("file watcher error under {}: {err}", self.root.display());
                return false;
            }
        };
        if event.need_rescan() {
            out.insert(self.root.clone());
            return true;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        let mut relevant = false;
        for path in &event.paths {
            if let Some(reported) = self.map(path) {
                out.insert(reported);
                relevant = true;
            }
        }
        relevant
    }

    /// The path to report for a raw event path, or `None` to ignore it.
    fn map(&self, path: &Path) -> Option<PathBuf> {
        let Some(rel) = path
            .strip_prefix(&self.root)
            .or_else(|_| path.strip_prefix(&self.canonical_root))
            .ok()
        else {
            // Outside the root (shouldn't happen); pass it through unchanged.
            return Some(path.to_path_buf());
        };
        let mut components = rel.components();
        match components.next() {
            Some(Component::Normal(first)) if first == ".git" => {
                let inside = components.as_path();
                let triggers = inside.as_os_str().is_empty()
                    || GIT_TRIGGERS.iter().any(|t| inside.starts_with(t));
                triggers.then(|| self.git_dir.clone())
            }
            // `join("")` would add a trailing separator.
            None => Some(self.root.clone()),
            _ => Some(self.root.join(rel)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> PathFilter {
        PathFilter {
            root: PathBuf::from("/work"),
            canonical_root: PathBuf::from("/private/work"),
            git_dir: PathBuf::from("/work/.git"),
        }
    }

    #[test]
    fn maps_paths() {
        let f = filter();
        let git = Some(PathBuf::from("/work/.git"));
        assert_eq!(
            f.map(Path::new("/work/src/a.rs")),
            Some("/work/src/a.rs".into())
        );
        assert_eq!(
            f.map(Path::new("/private/work/a.rs")),
            Some("/work/a.rs".into())
        );
        let root = f.map(Path::new("/private/work/")).unwrap();
        assert_eq!(root.as_os_str(), "/work");
        assert_eq!(f.map(Path::new("/work/.git/index")), git);
        assert_eq!(f.map(Path::new("/work/.git/HEAD")), git);
        assert_eq!(f.map(Path::new("/private/work/.git/refs/heads/main")), git);
        assert_eq!(f.map(Path::new("/work/.git")), git);
        assert_eq!(f.map(Path::new("/work/.git/index.lock")), None);
        assert_eq!(f.map(Path::new("/work/.git/objects/ab/cdef")), None);
        assert_eq!(f.map(Path::new("/work/.git/refsx")), None);
        assert_eq!(
            f.map(Path::new("/work/sub/.git/index")),
            Some("/work/sub/.git/index".into())
        );
        assert_eq!(
            f.map(Path::new("/work/.gitignore")),
            Some("/work/.gitignore".into())
        );
    }

    #[test]
    fn collect_skips_access_and_errors() {
        let f = filter();
        let mut out = BTreeSet::new();
        let access = Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path("/work/a".into());
        assert!(!f.collect(Ok(access), &mut out));
        assert!(!f.collect(Err(notify::Error::generic("boom")), &mut out));
        let objects = Event::new(EventKind::Any).add_path("/work/.git/objects/x".into());
        assert!(!f.collect(Ok(objects), &mut out));
        assert_eq!(out, BTreeSet::new());

        let modify = Event::new(EventKind::Any)
            .add_path("/work/a".into())
            .add_path("/work/a".into());
        assert!(f.collect(Ok(modify), &mut out));
        assert_eq!(
            out.into_iter().collect::<Vec<_>>(),
            vec![PathBuf::from("/work/a")]
        );
    }
}
