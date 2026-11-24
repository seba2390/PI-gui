//! Deep search for pistachio: web search and page reading for pi, done in Rust.
//!
//! Chats have three modes ([`config::Mode`]): off, search (a quick lookup) and deep (a
//! thorough investigation). pi gets its tools from TypeScript extensions, so a small embedded
//! extension ([`EXTENSION_SOURCE`]) registers `web_search` and `web_fetch` and the
//! `/websearch` command, adds the mode's instructions and limits, and runs this crate's
//! helper ([`cli`]: the app's own executable started with [`cli::FLAG`]) for the actual work: searching ([`search`]), fetching only public pages
//! ([`http`]), extracting the readable text ([`extract`]) and keeping the passages that matter
//! ([`rank`]), and checking a report's links and claims ([`check`]). Settings live in [`config`].

pub mod check;
pub mod cli;
pub mod config;
pub mod effort;
pub mod extract;
pub mod http;
pub mod pages;
pub mod pdf;
pub mod rank;
pub mod search;
pub mod tools;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The pi extension that connects the tools to pi.
pub const EXTENSION_SOURCE: &str = include_str!("extension.ts");
/// The environment variable that tells the extension which executable runs the tools.
pub const BIN_ENV: &str = "PISTACHIO_DEEPSEARCH_BIN";
/// The tools the extension registers (the allowlist chat sessions start pi with).
pub const TOOL_NAMES: [&str; 2] = ["web_search", "web_fetch"];
/// The extension's `setStatus` key; its text is the chat's [`config::Mode`] (absent: off).
pub const STATUS_KEY: &str = "pistachio-deepsearch";
/// The key of the status that says deep search's [`effort::Effort`] (absent: not deep).
pub const EFFORT_KEY: &str = "pistachio-deepsearch-effort";
/// The extension's command that sets the mode, `/websearch off|search|deep`, and the effort,
/// `/websearch effort light|standard|thorough|max`.
pub const COMMAND: &str = "websearch";

/// Writes the extension into `dir` (only when it changed) and returns its path.
pub fn install_extension(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("extension.ts");
    if fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}
