//! SearXNG on this computer, for web search without an account or Docker.
//!
//! Nothing of it ships with the app. On request [`install`] downloads uv (which installs Python
//! programs), SearXNG, a private Python and the packages SearXNG needs, each pinned to a version
//! and checked against a SHA-256 ([`catalog`]). [`Server`] runs it for this computer only, and
//! [`probe`] says whether the SearXNG at some address can be used.
//!
//! Everything is below one folder, `root`:
//! - `app/`: the installation (`python/`, `site/` with the packages, `searxng/`), moved into place
//!   when it is complete, so a half-finished one is never run;
//! - `settings.yml`: SearXNG's settings, written once and the user's to edit;
//! - `data/`: its caches;
//! - `port`: the port of the running server, for the processes that search.

pub mod catalog;
mod install;
mod probe;
mod server;

pub use install::{InstallError, Installed, Progress, Stage, install, installed, remove};
pub use probe::{Probe, probe};
pub use server::{Server, running_url, wait_for_url};

use std::path::{Path, PathBuf};

/// The folder of SearXNG in web search's settings folder.
pub fn root_in(deepsearch_dir: &Path) -> PathBuf {
    deepsearch_dir.join("searxng")
}
