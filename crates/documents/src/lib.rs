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
//! reader never takes the app down. [`viewer`] shows PDF files the same way.
//!
//! Everything is below one folder ([`config::default_dir`]):
//! - `config.json`: the settings;
//! - `parsers/<parser>/app/`: an installation, moved into place when it is complete;
//! - `cache/`: what was read lately, so that a file read in parts is parsed once;
//! - `attached/`: where files attached to chats are, with a small picture of PDFs;
//! - `tmp/`: what parsers write while they run;
//! - `extension.ts`: the pi extension.

pub mod attached;
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
pub mod viewer;

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

/// Writes a shell script that can be run. Another process writes it: one that was just started
/// here could still hold the file open, and Linux does not run a file that is being written.
#[cfg(test)]
#[cfg(unix)]
pub(crate) fn write_program(path: &Path, script: &str) {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut writer = Command::new("/bin/sh")
        .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let text = format!("#!/bin/sh\n{script}\n");
    let mut stdin = writer.stdin.take().unwrap();
    stdin.write_all(text.as_bytes()).unwrap();
    drop(stdin);
    assert!(writer.wait().unwrap().success());
}

/// Writes the extension into `dir` (only when it changed) and returns its path.
pub fn install_extension(dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("extension.ts");
    if fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}
