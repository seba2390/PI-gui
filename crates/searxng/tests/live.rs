//! Really downloads and runs SearXNG (about 80 MB):
//! `cargo test -p searxng --test live -- --ignored`.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use searxng::{Installed, Probe, Server, install, installed, probe, running_url, wait_for_url};

#[test]
#[ignore = "downloads SearXNG"]
fn downloads_and_runs() {
    let dir = tempfile::tempdir().unwrap();
    // Spaces and letters outside ASCII are fine in a user's folder name.
    let root = dir.path().join("Søren's files").join("searxng");
    install(&root, &AtomicBool::new(false), &|progress| {
        println!("{:?} {:.0}%", progress.stage, progress.fraction() * 100.);
    })
    .unwrap();
    assert_eq!(installed(&root), Installed::Current);
    assert!(!root.join("partial").exists() && !root.join("app/tools").exists());

    let mut server = Server::start(&root).unwrap();
    assert!(server.is_running());
    let url = running_url(&root).unwrap();
    assert_eq!(url, format!("http://127.0.0.1:{}", server.port()));
    assert_eq!(wait_for_url(&root, Duration::ZERO), Ok(url.clone()));
    assert_eq!(probe(&url), Probe::Ready);
    drop(server);
    assert_eq!(probe(&url), Probe::Nothing);

    // Again over itself, as an update would.
    install(&root, &AtomicBool::new(false), &|_| {}).unwrap();
    assert_eq!(installed(&root), Installed::Current);
}
