//! Downloading SearXNG and what it needs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use python_env::InstallError;
use python_env::fetch::{ChecksumOf, download_tar, unpack};
use python_env::{Tools, io};

use crate::catalog::{self, Platform};

/// Written last into an installation: the [`catalog::revision`] it was made of.
const MARKER: &str = ".installed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    No,
    /// It runs, but the app would now install another version.
    Outdated,
    Current,
}

/// What is being downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Installer,
    Searxng,
    Python,
    Packages,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Installer => "the installer (uv)",
            Self::Searxng => "SearXNG",
            Self::Python => "Python",
            Self::Packages => "Python packages",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// Bytes of the whole download, as far as known: uv does not say how far it is.
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
pub(crate) struct Layout {
    pub dir: PathBuf,
}

impl Layout {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The finished installation in `root`.
    pub fn installed(root: &Path) -> Self {
        Self::new(root.join("app"))
    }

    pub fn python_dir(&self) -> PathBuf {
        self.dir.join("python")
    }

    pub fn python(&self) -> PathBuf {
        python_env::python_in(&self.python_dir())
    }

    pub fn packages(&self) -> PathBuf {
        self.dir.join("site")
    }

    pub fn searxng(&self) -> PathBuf {
        self.dir.join("searxng")
    }

    fn marker(&self) -> PathBuf {
        self.dir.join(MARKER)
    }
}

/// Whether SearXNG is installed in `root`.
pub fn installed(root: &Path) -> Installed {
    let layout = Layout::installed(root);
    match fs::read_to_string(layout.marker()) {
        Ok(_) if !layout.python().is_file() => Installed::No,
        Ok(revision) if revision == catalog::revision() => Installed::Current,
        Ok(_) => Installed::Outdated,
        Err(_) => Installed::No,
    }
}

/// Deletes SearXNG and everything it kept. Stop it first.
pub fn remove(root: &Path) -> std::io::Result<()> {
    python_env::remove(root)
}

/// Downloads SearXNG into `root`, replacing one that is there (stop it first). Blocks.
pub fn install(
    root: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), InstallError> {
    let Some(platform) = catalog::platform() else {
        return Err(InstallError::Failed(
            "SearXNG does not run on this system.".into(),
        ));
    };
    let partial = root.join("partial");
    // What an interrupted download left.
    remove(&partial).map_err(io("Clearing", &partial))?;
    let result = install_into(&partial, &platform, cancel, progress).and_then(|()| {
        let target = Layout::installed(root).dir;
        remove(&target).map_err(io("Replacing", &target))?;
        fs::rename(&partial, &target).map_err(io("Installing", &target))
    });
    if result.is_err() {
        let _ = remove(&partial);
    }
    result
}

fn install_into(
    dir: &Path,
    platform: &Platform,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), InstallError> {
    let total = platform.download_size();
    let mut before = 0;
    let report = |stage, done| progress(Progress { stage, done, total });
    let layout = Layout::new(dir.to_path_buf());
    // Not part of the installation: removed when it is complete.
    let tools = dir.join("tools");
    let installer = Tools::download(&tools, platform.python, cancel, &mut |bytes| {
        report(Stage::Installer, bytes);
    })?;
    before += platform.python.uv_size;

    let tar = download_tar(
        &catalog::searxng_url(),
        catalog::SEARXNG_TAR_SHA256,
        ChecksumOf::Tar,
        cancel,
        &mut |bytes| report(Stage::Searxng, before + bytes),
    )?;
    let searxng = layout.searxng();
    unpack(&tar, &searxng, |path| {
        path.starts_with("searx") || path == Path::new("LICENSE")
    })
    .map_err(io("Unpacking", &searxng))?;
    drop(tar);
    // Without it SearXNG asks git for its version, and on a Mac without Apple's developer tools
    // that opens a window offering to install them.
    let version = searxng.join("searx").join("version_frozen.py");
    fs::write(&version, frozen_version()).map_err(io("Writing", &version))?;
    before += catalog::SEARXNG_SIZE;

    report(Stage::Python, before);
    installer.python(&layout.python_dir())?;
    before += platform.python.python_size;

    report(Stage::Packages, before);
    installer.packages(
        &layout.python_dir(),
        catalog::REQUIREMENTS,
        &layout.packages(),
        &mut |_| {},
    )?;
    report(Stage::Packages, total);

    remove(&tools).map_err(io("Removing", &tools))?;
    let marker = layout.marker();
    fs::write(&marker, catalog::revision()).map_err(io("Writing", &marker))
}

fn frozen_version() -> String {
    let version = &catalog::SEARXNG_COMMIT[..9];
    format!(
        "VERSION_STRING = \"{version}\"\nVERSION_TAG = \"{version}\"\nDOCKER_TAG = \"{version}\"\n\
         GIT_URL = \"https://github.com/searxng/searxng\"\nGIT_BRANCH = \"master\"\n"
    )
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn fake_installation(root: &Path, revision: &str) {
        let layout = Layout::installed(root);
        fs::create_dir_all(layout.python().parent().unwrap()).unwrap();
        fs::write(layout.python(), "").unwrap();
        fs::write(layout.marker(), revision).unwrap();
    }

    #[test]
    fn says_what_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("searxng");
        assert_eq!(installed(&root), Installed::No);
        fake_installation(&root, "older");
        assert_eq!(installed(&root), Installed::Outdated);
        fake_installation(&root, &catalog::revision());
        assert_eq!(installed(&root), Installed::Current);
        fs::remove_file(Layout::installed(&root).python()).unwrap();
        assert_eq!(installed(&root), Installed::No);
        remove(&root).unwrap();
        assert!(!root.exists());
        remove(&root).unwrap();
    }

    #[test]
    fn a_cancelled_install_leaves_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        fake_installation(dir.path(), "older");
        let result = install(dir.path(), &AtomicBool::new(true), &|_| {});
        assert_eq!(result, Err(InstallError::Cancelled));
        assert_eq!(installed(dir.path()), Installed::Outdated);
        assert!(!dir.path().join("partial").exists());
    }
}
