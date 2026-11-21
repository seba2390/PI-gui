#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::TestRepo;
use workspace::watch::Watcher;

const DEBOUNCE: Duration = Duration::from_millis(100);
const TIMEOUT: Duration = Duration::from_secs(5);

fn start(root: &Path) -> (Watcher, Receiver<Vec<PathBuf>>) {
    let (tx, rx) = mpsc::channel();
    let watcher = Watcher::new(root, DEBOUNCE, move |paths| {
        let _ = tx.send(paths);
    })
    .unwrap();
    // Give the OS watcher (FSEvents in particular) a moment to start delivering events.
    std::thread::sleep(Duration::from_millis(300));
    (watcher, rx)
}

/// Waits until a batch containing `expected` arrives, retriggering `poke` every second in case
/// the first event was missed while the backend was still starting.
fn wait_for(
    rx: &Receiver<Vec<PathBuf>>,
    expected: &Path,
    mut poke: impl FnMut(),
) -> Vec<Vec<PathBuf>> {
    let start = Instant::now();
    let mut last_poke = Instant::now();
    let mut seen = Vec::new();
    poke();
    while start.elapsed() < TIMEOUT {
        if let Ok(batch) = rx.recv_timeout(Duration::from_millis(100)) {
            let hit = batch.iter().any(|p| p == expected);
            seen.push(batch);
            if hit {
                return seen;
            }
        }
        if last_poke.elapsed() > Duration::from_secs(1) {
            poke();
            last_poke = Instant::now();
        }
    }
    panic!(
        "no event for {} within {TIMEOUT:?}; saw {seen:#?}",
        expected.display()
    );
}

#[test]
fn reports_file_writes() {
    let dir = TestRepo::plain();
    let (watcher, rx) = start(&dir.root);
    assert_eq!(watcher.root(), dir.root);
    let target = dir.path("sub/file.txt");
    std::fs::create_dir_all(dir.path("sub")).unwrap();
    let mut n = 0;
    let batches = wait_for(&rx, &target, || {
        n += 1;
        dir.write("sub/file.txt", format!("write {n}\n"));
    });
    for batch in &batches {
        let mut sorted = batch.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(&sorted, batch, "batches are sorted and de-duplicated");
    }
}

#[test]
fn reports_git_index_changes_as_git_dir() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    let (_watcher, rx) = start(&repo.root);
    let git_dir = repo.path(".git");
    let mut n = 0;
    let batches = wait_for(&rx, &git_dir, || {
        n += 1;
        repo.write("a.txt", format!("{n}\n"));
        repo.git(&["add", "a.txt"]);
    });
    for path in batches.iter().flatten() {
        assert!(
            !path.starts_with(&git_dir) || path == &git_dir,
            "internal git paths must be collapsed: {}",
            path.display()
        );
    }
}

#[test]
fn drop_stops_the_watcher() {
    let dir = TestRepo::plain();
    let (watcher, rx) = start(&dir.root);
    drop(watcher);
    dir.write("after-drop.txt", "x\n");
    // The callback (and its sender) must be gone: after any batches flushed before the drop, the
    // channel disconnects instead of timing out, and nothing written afterwards shows up.
    loop {
        match rx.recv_timeout(TIMEOUT) {
            Ok(batch) => assert!(
                !batch.iter().any(|p| p.ends_with("after-drop.txt")),
                "event after drop: {batch:?}"
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("watcher thread still alive"),
        }
    }
}
