//! Reading documents (PDF files, and Office files with Docling) as Markdown.
//!
//! The built-in reader ([`builtin`]) comes with the app. The others ([`Parser`]) are downloaded on
//! request ([`install`]): nothing of them ships with the app, each has a private Python, packages
//! and models of its own, all pinned and checked, and each document is read by a separate process
//! without network ([`helper`]). [`read`] reads a document with the parser in the settings
//! ([`config`]) and falls back to the built-in reader.
//!
//! The app and its pi extension ([`EXTENSION_SOURCE`], for Code sessions) read documents through
//! [`cli`]: the app's own executable started with [`cli::FLAG`], so that a file that breaks its
//! reader never takes the app down.
//!
//! Everything is below one folder ([`config::default_dir`]):
//! - `config.json`: the settings;
//! - `parsers/<parser>/app/`: an installation, moved into place when it is complete;
//! - `cache/`: what was read lately, so that a file read in parts is parsed once;
//! - `tmp/`: what parsers write while they run;
//! - `extension.ts`: the pi extension.

pub mod builtin;
mod cache;
pub mod catalog;
pub mod cli;
pub mod client;
pub mod config;
pub mod helper;
mod install;
mod read;
mod text;

pub use catalog::Parser;
pub use install::{Installed, Progress, Stage, install, installed, remove};
pub use python_env::InstallError;
pub use read::{Content, Document, Kind, can_read, read, read_bytes};

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The pi extension that has `read` return PDF files as Markdown.
pub const EXTENSION_SOURCE: &str = include_str!("extension.ts");
/// The environment variable that tells the extension which executable reads documents.
pub const BIN_ENV: &str = "PISTACHIO_DOCUMENTS_BIN";

/// Writes the extension into `dir` (only when it changed) and returns its path.
pub fn install_extension(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("extension.ts");
    if fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}
