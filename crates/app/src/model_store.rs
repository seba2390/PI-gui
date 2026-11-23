//! Downloadable models: which are installed, and the one download that may be running.
//!
//! Shared by speech to text ([`crate::stt`]) and conversation names ([`crate::naming`]).
//! None of these models ship with the app: each file is downloaded on request from a pinned
//! revision and checked against its size and SHA-256.
//!
//! Models live in `<dir>/<id>/`. A download writes into `<id>.partial/`, checks every
//! file's size and SHA-256, writes a `.verified` marker and only then renames the folder into
//! place, so a half-finished or corrupt download is never taken for an installed model. Files
//! that already verified in `.partial` are kept, so a cancelled download resumes file by file.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use sha2::{Digest, Sha256};

const VERIFIED: &str = ".verified";

/// One file of a model.
#[derive(Debug)]
pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// A file to download: where from, and what it must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub url: String,
}

/// A model a [`ModelStore`] can download.
pub trait Downloadable: Sync + 'static {
    /// Stable id: the settings value and the folder name.
    fn id(&self) -> &'static str;
    /// The files and where to get them.
    fn fetches(&self) -> Vec<Fetch>;
    /// Written into the `.verified` marker (the pinned revision).
    fn revision(&self) -> &'static str;

    /// Download size in bytes.
    fn size(&self) -> u64 {
        self.fetches().iter().map(|fetch| fetch.size).sum()
    }
}

/// The installed models of one catalog and the download that may be running.
pub struct ModelStore<M: 'static> {
    dir: Option<PathBuf>,
    installed: HashSet<&'static str>,
    download: Option<Download<M>>,
}

/// A running download.
pub struct Download<M: 'static> {
    pub model: &'static M,
    /// Bytes received so far, across all files.
    pub done: u64,
    pub total: u64,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

impl<M> Download<M> {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // display only
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.
        } else {
            (self.done as f64 / self.total as f64).clamp(0., 1.) as f32
        }
    }
}

#[derive(Debug, Clone)]
pub enum StoreEvent {
    Installed(&'static str),
    Failed(&'static str, String),
    Cancelled,
}

impl<M: Downloadable> EventEmitter<StoreEvent> for ModelStore<M> {}

/// Progress messages from the download thread.
enum Progress {
    Bytes(u64),
    Done,
    Failed(String),
}

impl<M: Downloadable> ModelStore<M> {
    /// The store for `catalog` in `dir` (`None`: nowhere to keep models).
    pub fn new(dir: Option<PathBuf>, catalog: &'static [M]) -> Self {
        let installed = dir
            .as_deref()
            .map(|dir| {
                catalog
                    .iter()
                    .filter(|model| is_installed(dir, *model))
                    .map(Downloadable::id)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            dir,
            installed,
            download: None,
        }
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.installed.contains(id)
    }

    pub fn download(&self) -> Option<&Download<M>> {
        self.download.as_ref()
    }

    /// The folder holding an installed model's files.
    pub fn model_dir(&self, id: &str) -> Option<PathBuf> {
        self.is_installed(id)
            .then(|| self.dir.as_ref().map(|dir| dir.join(id)))
            .flatten()
    }

    /// Starts downloading `model` unless a download is already running.
    pub fn start_download(&mut self, model: &'static M, cx: &mut Context<Self>) {
        if self.download.is_some() || self.is_installed(model.id()) {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            cx.emit(StoreEvent::Failed(
                model.id(),
                "No folder for models (pi's agent folder is unknown).".into(),
            ));
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let thread_cancel = cancel.clone();
        std::thread::Builder::new()
            .name(format!("model-download-{}", model.id()))
            .spawn(move || {
                let result = download_model(&dir, model, &thread_cancel, &|bytes| {
                    let _ = tx.send_blocking(Progress::Bytes(bytes));
                });
                let _ = tx.send_blocking(match result {
                    Ok(()) => Progress::Done,
                    Err(error) => Progress::Failed(error),
                });
            })
            .ok();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(progress) = rx.recv().await {
                let done = matches!(progress, Progress::Done | Progress::Failed(_));
                let _ = this.update(cx, |this, cx| this.on_progress(model, progress, cx));
                if done {
                    break;
                }
            }
        });
        self.download = Some(Download {
            model,
            done: 0,
            total: model.size(),
            cancel,
            _task: task,
        });
        cx.notify();
    }

    fn on_progress(&mut self, model: &'static M, progress: Progress, cx: &mut Context<Self>) {
        match progress {
            Progress::Bytes(done) => {
                if let Some(download) = self.download.as_mut() {
                    download.done = done;
                }
            }
            Progress::Done => {
                self.download = None;
                self.installed.insert(model.id());
                cx.emit(StoreEvent::Installed(model.id()));
            }
            Progress::Failed(error) => {
                let cancelled = self
                    .download
                    .take()
                    .is_some_and(|download| download.cancel.load(Ordering::Relaxed));
                cx.emit(if cancelled {
                    StoreEvent::Cancelled
                } else {
                    StoreEvent::Failed(model.id(), error)
                });
            }
        }
        cx.notify();
    }

    /// Stops the running download; the files that finished stay for the next attempt.
    pub fn cancel_download(&mut self) {
        if let Some(download) = &self.download {
            download.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Deletes an installed model's files.
    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(path) = self.model_dir(id) else {
            return;
        };
        self.installed.remove(id);
        cx.background_spawn(async move {
            if let Err(error) = fs::remove_dir_all(&path) {
                log::warn!("removing {}: {error}", path.display());
            }
        })
        .detach();
        cx.notify();
    }
}

fn is_installed(dir: &Path, model: &impl Downloadable) -> bool {
    let folder = dir.join(model.id());
    folder.join(VERIFIED).is_file()
        && model.fetches().iter().all(|file| {
            fs::metadata(folder.join(file.name)).is_ok_and(|meta| meta.len() == file.size)
        })
}

/// Downloads every file of `model` into `<dir>/<id>.partial`, verifies them and moves the
/// folder to `<dir>/<id>`. Reports the bytes received so far through `progress`.
fn download_model(
    dir: &Path,
    model: &impl Downloadable,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let partial = dir.join(format!("{}.partial", model.id()));
    fs::create_dir_all(&partial).map_err(|e| format!("creating {}: {e}", partial.display()))?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("pistachio/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();

    let mut done = 0u64;
    for file in model.fetches() {
        let path = partial.join(file.name);
        if fs::metadata(&path).is_ok_and(|meta| meta.len() == file.size)
            && sha256_file(&path).is_ok_and(|hash| hash == file.sha256)
        {
            done += file.size;
            progress(done);
            continue;
        }
        download_file(&agent, &path, &file, cancel, &mut |bytes| {
            progress(done + bytes);
        })?;
        done += file.size;
    }
    fs::write(partial.join(VERIFIED), model.revision())
        .map_err(|e| format!("writing the marker: {e}"))?;
    let target = dir.join(model.id());
    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }
    fs::rename(&partial, &target).map_err(|e| format!("installing the model: {e}"))?;
    Ok(())
}

fn download_file(
    agent: &ureq::Agent,
    path: &Path,
    file: &Fetch,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let mut response = agent
        .get(&file.url)
        .call()
        .map_err(|e| format!("downloading {}: {e}", file.name))?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(file.size + 1)
        .reader();
    let tmp = path.with_extension("download");
    let mut out = fs::File::create(&tmp).map_err(|e| format!("creating {}: {e}", tmp.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut received = 0u64;
    let mut reported = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(&tmp);
            return Err("cancelled".into());
        }
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("downloading {}: {e}", file.name))?;
        if n == 0 {
            break;
        }
        out.write_all(&buffer[..n])
            .map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        hasher.update(&buffer[..n]);
        received += n as u64;
        if reported.elapsed() >= Duration::from_millis(100) {
            progress(received);
            reported = Instant::now();
        }
    }
    out.flush()
        .map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    drop(out);
    progress(received);
    let hash = hex(&hasher.finalize());
    if received != file.size || hash != file.sha256 {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "{} did not match its checksum (got {received} bytes); try again",
            file.name
        ));
    }
    fs::rename(&tmp, path).map_err(|e| format!("saving {}: {e}", file.name))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// `639 MB`, `1.0 GB`.
#[allow(clippy::cast_precision_loss)] // display only
pub fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.;
    if mb >= 1000. {
        format!("{:.1} GB", mb / 1000.)
    } else {
        format!("{mb:.0} MB")
    }
}

/// Checks a catalog's pins: full commit revisions, SHA-256 digests and sizes (for tests).
#[cfg(test)]
pub fn assert_pinned(model: &impl Downloadable) {
    assert_eq!(
        model.revision().len(),
        40,
        "{}: pin a full commit",
        model.id()
    );
    for file in model.fetches() {
        assert_eq!(file.sha256.len(), 64, "{} {}", model.id(), file.name);
        assert!(
            file.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{} {}: lowercase hex",
            model.id(),
            file.name
        );
        assert!(file.size > 0);
        assert!(
            file.url.starts_with("https://huggingface.co/") && file.url.contains("/resolve/"),
            "{}",
            file.url
        );
        let pinned = file
            .url
            .split("/resolve/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default();
        assert_eq!(pinned.len(), 40, "{}: pin a full commit", file.url);
    }
}

#[cfg(test)]
mod tests {
    use super::{Downloadable, Fetch, VERIFIED, is_installed, sha256_file, size_label};
    use std::fs;

    struct Fake;

    impl Downloadable for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn revision(&self) -> &'static str {
            "r"
        }
        fn fetches(&self) -> Vec<Fetch> {
            vec![
                Fetch {
                    name: "a.bin",
                    size: 3,
                    sha256: "x",
                    url: String::new(),
                },
                Fetch {
                    name: "b.json",
                    size: 5,
                    sha256: "y",
                    url: String::new(),
                },
            ]
        }
    }

    #[test]
    fn installed_needs_marker_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(Fake.id());
        fs::create_dir_all(&folder).unwrap();
        for file in Fake.fetches() {
            let f = fs::File::create(folder.join(file.name)).unwrap();
            f.set_len(file.size).unwrap();
        }
        assert!(!is_installed(dir.path(), &Fake), "no marker yet");
        fs::write(folder.join(VERIFIED), "x").unwrap();
        assert!(is_installed(dir.path(), &Fake));
        fs::File::create(folder.join("a.bin"))
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(!is_installed(dir.path(), &Fake), "wrong size");
        assert_eq!(Fake.size(), 8);
    }

    #[test]
    fn hashes_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn size_labels() {
        assert_eq!(size_label(670_576_784), "671 MB");
        assert_eq!(size_label(1_036_613_791), "1.0 GB");
    }
}
