//! Downloading SearXNG and what it needs.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::catalog::{self, Platform};
use crate::fetch::{ChecksumOf, download_tar, unpack};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for InstallError {}

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
        self.python_dir().join("bin").join("python3")
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
    match fs::remove_dir_all(root) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
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
    let result = install_into(&partial, platform, cancel, progress).and_then(|()| {
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
    fs::create_dir_all(&tools).map_err(io("Creating", &tools))?;

    let tar = download_tar(
        &platform.uv_url(),
        platform.uv_sha256,
        ChecksumOf::Compressed,
        cancel,
        &mut |bytes| report(Stage::Installer, bytes),
    )?;
    unpack(&tar, &tools, |path| path == Path::new("uv")).map_err(io("Unpacking", &tools))?;
    drop(tar);
    before += platform.uv_size;

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
    let pythons = tools.join("pythons");
    run(
        uv(&tools)
            .args(["python", "install", "--no-bin", "--install-dir"])
            .arg(&pythons)
            .arg(catalog::PYTHON_VERSION),
        "Downloading Python",
        cancel,
    )?;
    let python = only_python(&pythons).ok_or_else(|| {
        InstallError::Failed("Downloading Python failed: it is not where it should be".into())
    })?;
    fs::rename(&python, layout.python_dir()).map_err(io("Installing", &python))?;
    before += platform.python_size;

    report(Stage::Packages, before);
    let requirements = tools.join("requirements.txt");
    fs::write(&requirements, catalog::REQUIREMENTS).map_err(io("Writing", &requirements))?;
    run(
        uv(&tools)
            .args(["pip", "sync", "--require-hashes", "--only-binary", ":all:"])
            .arg("--python")
            .arg(layout.python())
            .arg("--target")
            .arg(layout.packages())
            .arg(&requirements),
        "Downloading the Python packages",
        cancel,
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

/// The Python folder in `dir` (beside it is a link with a shorter name).
fn only_python(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().starts_with("cpython-")
        })
        .map(|entry| entry.path())
}

/// uv, kept away from the user's own uv, Pythons and settings.
fn uv(tools: &Path) -> Command {
    let mut command = Command::new(tools.join("uv"));
    command
        .env_clear()
        .envs(crate::network_env())
        .env("HOME", tools)
        // On a Mac uv looks for a developer tool it can do without, and finding Apple's
        // placeholder for it would open a window offering to install them.
        .env(
            "PATH",
            if cfg!(target_os = "macos") {
                tools.join("none").into_os_string()
            } else {
                OsStr::new("/usr/bin:/bin").to_owned()
            },
        )
        .env("UV_NO_CONFIG", "1")
        .env("UV_CACHE_DIR", tools.join("cache"))
        .env("UV_PYTHON_INSTALL_DIR", tools.join("pythons"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_PROGRESS", "1")
        .current_dir(tools)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// Runs `command` to its end, or kills it when cancelled.
fn run(command: &mut Command, what: &str, cancel: &AtomicBool) -> Result<(), InstallError> {
    let failed = |error: &str| InstallError::Failed(format!("{what} failed: {error}"));
    let mut child = command
        .spawn()
        .map_err(|error| failed(&error.to_string()))?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        String::from_utf8_lossy(&text).into_owned()
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(InstallError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(failed(&error.to_string())),
        }
    };
    let text = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    let error: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("warning:"))
        .collect();
    let last = &error[error.len().saturating_sub(4)..];
    Err(failed(&match last {
        [] => status.to_string(),
        lines => lines.join(" "),
    }))
}

fn io<'a>(what: &'a str, path: &'a Path) -> impl FnOnce(std::io::Error) -> InstallError + 'a {
    move |error| InstallError::Failed(format!("{what} {} failed: {error}", path.display()))
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

    #[test]
    fn programs_are_stopped_or_say_why_they_failed() {
        let sh = |script: &str| {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", script])
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            command
        };
        let go = AtomicBool::new(false);
        assert_eq!(run(&mut sh("exit 0"), "Trying", &go), Ok(()));
        let script = "echo 'warning: minor' >&2; echo 'error: no network' >&2; exit 2";
        let error = run(&mut sh(script), "Trying", &go).unwrap_err();
        assert_eq!(error.to_string(), "Trying failed: error: no network");
        let error = run(&mut sh("exit 3"), "Trying", &go).unwrap_err();
        assert!(error.to_string().contains('3'), "{error}");
        let stop = AtomicBool::new(true);
        assert_eq!(
            run(&mut sh("exec sleep 60"), "Trying", &stop),
            Err(InstallError::Cancelled)
        );
    }
}
