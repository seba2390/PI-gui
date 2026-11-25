//! Downloading a parser and what it needs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use python_env::fetch::download_file;
use python_env::{InstallError, Tools, io};

use crate::Parser;
use crate::catalog::Download;
use crate::helper;

/// Written last into an installation: the [`Download::revision`] it was made of.
const MARKER: &str = ".installed";
/// How long the first run may take: Python compiles everything.
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);
/// What a new installation must read to be used.
const CHECK_PDF: &[u8] = include_bytes!("check.pdf");
const CHECK_TEXT: &str = "Pistachio";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    No,
    /// It runs, but the app would now install another version.
    Outdated,
    Current,
}

/// What is being done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Installer,
    Python,
    Packages,
    Models,
    Check,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Installer => "Downloading the installer (uv)",
            Self::Python => "Downloading Python",
            Self::Packages => "Downloading Python packages",
            Self::Models => "Downloading models",
            Self::Check => "Trying it out",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// Bytes of the whole download, as far as known: of the packages uv only names the large
    /// ones it has finished.
    pub done: u64,
    pub total: u64,
}

impl Progress {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // display only
    pub fn fraction(self) -> f32 {
        (self.done as f64 / self.total.max(1) as f64).clamp(0., 1.) as f32
    }
}

/// The parts of an installation.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    dir: PathBuf,
}

impl Layout {
    /// The finished installation of `parser` below `dir`.
    pub fn installed(dir: &Path, parser: Parser) -> Self {
        Self {
            dir: root(dir, parser).join("app"),
        }
    }

    fn python_dir(&self) -> PathBuf {
        self.dir.join("python")
    }

    pub fn python(&self) -> PathBuf {
        python_env::python_in(&self.python_dir())
    }

    pub fn packages(&self) -> PathBuf {
        self.dir.join("site")
    }

    pub fn models(&self) -> PathBuf {
        self.dir.join("models")
    }

    pub fn marker(&self) -> PathBuf {
        self.dir.join(MARKER)
    }
}

fn root(dir: &Path, parser: Parser) -> PathBuf {
    dir.join("parsers").join(parser.id())
}

/// Whether `parser` is installed below `dir`. The built-in reader always is.
pub fn installed(dir: &Path, parser: Parser) -> Installed {
    if parser == Parser::Builtin {
        return Installed::Current;
    }
    let layout = Layout::installed(dir, parser);
    match fs::read_to_string(layout.marker()) {
        Ok(_) if !layout.python().is_file() => Installed::No,
        Ok(revision)
            if parser
                .download()
                .is_ok_and(|download| download.revision() == revision) =>
        {
            Installed::Current
        }
        Ok(_) => Installed::Outdated,
        Err(_) => Installed::No,
    }
}

/// Deletes `parser` and everything that was downloaded for it.
pub fn remove(dir: &Path, parser: Parser) -> std::io::Result<()> {
    python_env::remove(&root(dir, parser))?;
    crate::cache::clear(dir)
}

/// Downloads `parser`, replacing one that is there, and reads a small PDF with it before it is
/// used. Blocks.
pub fn install(
    dir: &Path,
    parser: Parser,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), InstallError> {
    let download = parser
        .download()
        .map_err(|reason| InstallError::Failed(format!("{}: {reason}.", parser.label())))?;
    let root = root(dir, parser);
    let partial = root.join("partial");
    // What an interrupted download left.
    python_env::remove(&partial).map_err(io("Clearing", &partial))?;
    let result = install_into(&partial, dir, &download, cancel, progress).and_then(|()| {
        let target = Layout::installed(dir, parser).dir;
        python_env::remove(&target).map_err(io("Replacing", &target))?;
        fs::rename(&partial, &target).map_err(io("Installing", &target))
    });
    if result.is_err() {
        let _ = python_env::remove(&partial);
        // Only if nothing else is in it.
        let _ = fs::remove_dir(&root);
    }
    result
}

fn install_into(
    partial: &Path,
    dir: &Path,
    download: &Download,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), InstallError> {
    let total = download.size();
    let report = |stage, done| progress(Progress { stage, done, total });
    let layout = Layout {
        dir: partial.to_path_buf(),
    };
    // Not part of the installation: removed when it is complete.
    let tools = partial.join("tools");
    let installer = Tools::download(&tools, download.platform, cancel, &mut |bytes| {
        report(Stage::Installer, bytes);
    })?;
    let mut before = download.platform.uv_size;

    report(Stage::Python, before);
    installer.python(&layout.python_dir())?;
    before += download.platform.python_size;

    report(Stage::Packages, before);
    let mut packages = Packages::default();
    installer.packages(
        &layout.python_dir(),
        download.requirements(),
        &layout.packages(),
        &mut |line| {
            let done = packages.line(line).min(download.packages_size());
            report(Stage::Packages, before + done);
        },
    )?;
    before += download.packages_size();
    // Frees the space the packages took as they were downloaded.
    python_env::remove(&tools).map_err(io("Removing", &tools))?;

    for model in download.parser().models() {
        let path = layout.models().join(model.path);
        download_file(model.pinned(), &path, cancel, &mut |bytes| {
            report(Stage::Models, before + bytes);
        })?;
        before += model.size;
    }

    report(Stage::Check, total);
    check(dir, &layout, download.parser(), cancel)?;
    let marker = layout.marker();
    fs::write(&marker, download.revision()).map_err(io("Writing", &marker))
}

/// Reads a small PDF: what is missing on this computer shows now, not with the first document.
fn check(
    dir: &Path,
    layout: &Layout,
    parser: Parser,
    cancel: &AtomicBool,
) -> Result<(), InstallError> {
    let failed = |error: String| {
        InstallError::Failed(format!(
            "{} was downloaded but does not run: {error}",
            parser.label()
        ))
    };
    let run = helper::Run::new(dir).map_err(failed)?;
    let file = run.dir.join("check.pdf");
    fs::write(&file, CHECK_PDF).map_err(io("Writing", &file))?;
    let text = helper::parse(dir, layout, parser, &file, CHECK_TIMEOUT, cancel);
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    let text = text.map_err(failed)?;
    if text.contains(CHECK_TEXT) {
        Ok(())
    } else {
        Err(failed("it read nothing in a test document".into()))
    }
}

/// Follows what uv says it downloads: `Downloading torch (132.1MiB)`, and later
/// `Downloaded torch`.
#[derive(Default)]
struct Packages {
    sizes: Vec<(String, u64)>,
    done: u64,
}

impl Packages {
    /// Takes a line and returns the bytes of the packages that are finished.
    fn line(&mut self, line: &str) -> u64 {
        if let Some(rest) = line.strip_prefix("Downloading ") {
            if let Some((name, size)) = rest.rsplit_once(" (")
                && let Some(size) = bytes(size.trim_end_matches(')'))
            {
                self.sizes.push((name.to_owned(), size));
            }
        } else if let Some(name) = line.strip_prefix("Downloaded ")
            && let Some(index) = self.sizes.iter().position(|(other, _)| other == name)
        {
            self.done += self.sizes.swap_remove(index).1;
        }
        self.done
    }
}

/// `132.1MiB` in bytes.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // display only
fn bytes(text: &str) -> Option<u64> {
    let at = text.find(|c: char| c.is_ascii_alphabetic())?;
    let unit = match &text[at..] {
        "KiB" => 1024.,
        "MiB" => 1024. * 1024.,
        "GiB" => 1024. * 1024. * 1024.,
        _ => return None,
    };
    let number: f64 = text[..at].parse().ok()?;
    Some((number * unit) as u64)
}

/// Makes `parser` look installed, with `python` as the program it runs.
#[cfg(test)]
#[cfg(unix)]
pub(crate) fn fake(dir: &Path, parser: Parser, python: &str) {
    let layout = Layout::installed(dir, parser);
    let path = layout.python();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::write_program(&path, python);
    let revision = parser.download().map_or("other".into(), |d| d.revision());
    fs::write(layout.marker(), revision).unwrap();
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    #[cfg(unix)]
    fn says_what_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert_eq!(installed(dir, Parser::Builtin), Installed::Current);
        assert_eq!(installed(dir, Parser::Docling), Installed::No);
        fake(dir, Parser::Pymupdf4llm, "exit 0");
        let layout = Layout::installed(dir, Parser::Pymupdf4llm);
        assert_eq!(
            installed(dir, Parser::Pymupdf4llm) == Installed::Current,
            Parser::Pymupdf4llm.download().is_ok()
        );
        assert_eq!(installed(dir, Parser::Docling), Installed::No);
        fs::write(layout.marker(), "older").unwrap();
        assert_eq!(installed(dir, Parser::Pymupdf4llm), Installed::Outdated);
        fs::remove_file(layout.python()).unwrap();
        assert_eq!(installed(dir, Parser::Pymupdf4llm), Installed::No);
        remove(dir, Parser::Pymupdf4llm).unwrap();
        assert!(!root(dir, Parser::Pymupdf4llm).exists());
        remove(dir, Parser::Pymupdf4llm).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn a_cancelled_install_leaves_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        if Parser::Pymupdf4llm.download().is_err() {
            return;
        }
        let stop = AtomicBool::new(true);
        let result = install(dir, Parser::Pymupdf4llm, &stop, &|_| {});
        assert_eq!(result, Err(InstallError::Cancelled));
        assert!(!root(dir, Parser::Pymupdf4llm).exists());
        fake(dir, Parser::Pymupdf4llm, "exit 0");
        let result = install(dir, Parser::Pymupdf4llm, &stop, &|_| {});
        assert_eq!(result, Err(InstallError::Cancelled));
        assert_eq!(installed(dir, Parser::Pymupdf4llm), Installed::Current);
        assert!(!root(dir, Parser::Pymupdf4llm).join("partial").exists());
    }

    #[test]
    fn what_is_not_available_is_not_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let go = AtomicBool::new(false);
        let result = install(dir.path(), Parser::Builtin, &go, &|_| {});
        assert!(matches!(result, Err(InstallError::Failed(_))));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn one_that_cannot_read_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let go = AtomicBool::new(false);
        let try_it = |python: &str| {
            fake(dir, Parser::Docling, python);
            let layout = Layout::installed(dir, Parser::Docling);
            check(dir, &layout, Parser::Docling, &go)
        };
        // $3 is the document, $4 where the text goes.
        assert_eq!(
            try_it("test -s \"$3\" && echo 'Hello Pistachio' > \"$4\""),
            Ok(())
        );
        let error = try_it("echo 'ImportError: no torch' >&2; exit 1").unwrap_err();
        assert!(error.to_string().contains("no torch"), "{error}");
        let error = try_it("echo > \"$4\"").unwrap_err();
        assert!(error.to_string().contains("read nothing"), "{error}");
    }

    #[test]
    fn follows_the_packages() {
        let mut packages = Packages::default();
        assert_eq!(packages.line("Resolved 87 packages in 1ms"), 0);
        assert_eq!(packages.line("Downloading torch (2.0MiB)"), 0);
        assert_eq!(packages.line("Downloading numpy (512.0KiB)"), 0);
        assert_eq!(packages.line("Downloaded numpy"), 512 * 1024);
        assert_eq!(packages.line("Downloaded six"), 512 * 1024);
        assert_eq!(packages.line("Downloaded torch"), 2560 * 1024);
        assert_eq!(bytes("1.5GiB"), Some(1_610_612_736));
        assert_eq!(bytes("12B"), None);
        assert_eq!(bytes(""), None);
    }
}
