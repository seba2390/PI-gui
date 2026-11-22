//! Installed speech models and the one download that may be running.
//!
//! Models live in `<agent dir>/pistachio/speech/<id>/`. A download writes into
//! `<id>.partial/`, checks every file's size and SHA-256, writes a `.verified` marker and
//! only then renames the folder into place, so a half-finished or corrupt download is never
//! taken for an installed model. Files that already verified in `.partial` are kept, so a
//! cancelled download resumes file by file.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use sha2::{Digest, Sha256};

use super::catalog::{self, MODELS, Model, ModelFile};

const VERIFIED: &str = ".verified";

pub struct SpeechModels {
    dir: Option<PathBuf>,
    installed: HashSet<&'static str>,
    download: Option<Download>,
}

/// A running download.
pub struct Download {
    pub model: &'static Model,
    /// Bytes received so far, across all files.
    pub done: u64,
    pub total: u64,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Download {
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
pub enum SpeechModelsEvent {
    Installed(&'static str),
    Failed(&'static str, String),
    Cancelled,
}

impl EventEmitter<SpeechModelsEvent> for SpeechModels {}

struct SpeechModelsHandle(Entity<SpeechModels>);

impl Global for SpeechModelsHandle {}

pub fn init(cx: &mut App) {
    let dir = pi_sessions::app_dir().map(|dir| dir.join("speech"));
    let models = cx.new(|_| SpeechModels::new(dir));
    cx.set_global(SpeechModelsHandle(models));
}

pub fn models(cx: &App) -> Entity<SpeechModels> {
    cx.global::<SpeechModelsHandle>().0.clone()
}

/// Progress messages from the download thread.
enum Progress {
    Bytes(u64),
    Done,
    Failed(String),
}

impl SpeechModels {
    fn new(dir: Option<PathBuf>) -> Self {
        let installed = dir
            .as_deref()
            .map(|dir| {
                MODELS
                    .iter()
                    .filter(|model| is_installed(dir, model))
                    .map(|model| model.id)
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

    pub fn download(&self) -> Option<&Download> {
        self.download.as_ref()
    }

    /// The folder holding an installed model's files.
    pub fn model_dir(&self, id: &str) -> Option<PathBuf> {
        self.is_installed(id)
            .then(|| self.dir.as_ref().map(|dir| dir.join(id)))
            .flatten()
    }

    /// Starts downloading `model` unless a download is already running.
    pub fn start_download(&mut self, model: &'static Model, cx: &mut Context<Self>) {
        if self.download.is_some() || self.is_installed(model.id) {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            cx.emit(SpeechModelsEvent::Failed(
                model.id,
                "No folder for speech models (pi's agent folder is unknown).".into(),
            ));
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = async_channel::unbounded();
        let thread_cancel = cancel.clone();
        std::thread::Builder::new()
            .name(format!("speech-download-{}", model.id))
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

    fn on_progress(&mut self, model: &'static Model, progress: Progress, cx: &mut Context<Self>) {
        match progress {
            Progress::Bytes(done) => {
                if let Some(download) = self.download.as_mut() {
                    download.done = done;
                }
            }
            Progress::Done => {
                self.download = None;
                self.installed.insert(model.id);
                cx.emit(SpeechModelsEvent::Installed(model.id));
            }
            Progress::Failed(error) => {
                let cancelled = self
                    .download
                    .take()
                    .is_some_and(|download| download.cancel.load(Ordering::Relaxed));
                cx.emit(if cancelled {
                    SpeechModelsEvent::Cancelled
                } else {
                    SpeechModelsEvent::Failed(model.id, error)
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

fn is_installed(dir: &Path, model: &Model) -> bool {
    let folder = dir.join(model.id);
    folder.join(VERIFIED).is_file()
        && model.files.iter().all(|file| {
            fs::metadata(folder.join(file.name)).is_ok_and(|meta| meta.len() == file.size)
        })
}

/// Downloads every file of `model` into `<dir>/<id>.partial`, verifies them and moves the
/// folder to `<dir>/<id>`. Reports the bytes received so far through `progress`.
fn download_model(
    dir: &Path,
    model: &Model,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let partial = dir.join(format!("{}.partial", model.id));
    fs::create_dir_all(&partial).map_err(|e| format!("creating {}: {e}", partial.display()))?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("pistachio/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();

    let mut done = 0u64;
    for file in model.files {
        let path = partial.join(file.name);
        if fs::metadata(&path).is_ok_and(|meta| meta.len() == file.size)
            && sha256_file(&path).is_ok_and(|hash| hash == file.sha256)
        {
            done += file.size;
            progress(done);
            continue;
        }
        download_file(
            &agent,
            &model.url(file),
            &path,
            file,
            cancel,
            &mut |bytes| {
                progress(done + bytes);
            },
        )?;
        done += file.size;
    }
    fs::write(partial.join(VERIFIED), model.revision)
        .map_err(|e| format!("writing the marker: {e}"))?;
    let target = dir.join(model.id);
    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }
    fs::rename(&partial, &target).map_err(|e| format!("installing the model: {e}"))?;
    Ok(())
}

fn download_file(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let mut response = agent
        .get(url)
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

/// The model to use: the chosen one if installed, else any installed one.
pub fn active_model(cx: &App) -> Option<&'static Model> {
    let models = models(cx);
    let models = models.read(cx);
    let chosen = cx
        .global::<crate::settings::AppSettings>()
        .speech_model
        .as_str();
    catalog::model(chosen)
        .filter(|model| models.is_installed(model.id))
        .or_else(|| MODELS.iter().find(|model| models.is_installed(model.id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_needs_marker_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let model = catalog::model("moonshine-base-en").unwrap();
        let folder = dir.path().join(model.id);
        fs::create_dir_all(&folder).unwrap();
        for file in model.files {
            let f = fs::File::create(folder.join(file.name)).unwrap();
            f.set_len(file.size).unwrap();
        }
        assert!(!is_installed(dir.path(), model), "no marker yet");
        fs::write(folder.join(VERIFIED), "x").unwrap();
        assert!(is_installed(dir.path(), model));
        fs::File::create(folder.join(model.files[0].name))
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(!is_installed(dir.path(), model), "wrong size");
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
}
