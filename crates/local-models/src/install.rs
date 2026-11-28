//! Downloading what a [`Choice`] needs: its engine and its model, each pinned and checked.
//!
//! An engine goes to `engines/<id>-<version>/` and a model to
//! `models/<model>/<build>-<revision>/`. Each is put together in a `.partial` folder and moved into
//! place, with a marker, only when complete, so a half-finished one is never run. A model's files
//! that already arrived and check out are kept, so a cancelled download goes on where it stopped.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use flate2::read::GzDecoder;
pub use python_env::InstallError;
use python_env::fetch::{ChecksumOf, Pinned, download_file, download_tar};
use python_env::{Tools, io};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::catalog::{Build, Catalog, Engine, Format, Install, Model};
use crate::recommend::Choice;
use crate::setups::Setup;

const MARKER: &str = ".installed";
/// An unpacked engine larger than this is refused.
const MAX_UNPACKED: u64 = 4 << 30;

/// What is being done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Engine,
    /// The installer (uv) and a private Python, for a Python engine.
    Python,
    Packages,
    /// Building the engine from source; the line is the build's latest output.
    Building(String),
    Model,
}

impl Stage {
    pub fn label(&self) -> String {
        match self {
            Self::Engine => "Downloading the engine".into(),
            Self::Python => "Downloading Python".into(),
            Self::Packages => "Downloading Python packages".into(),
            Self::Building(line) if line.is_empty() => "Building the engine".into(),
            Self::Building(line) => format!("Building: {line}"),
            Self::Model => "Downloading the model".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// Bytes of the whole download so far, and in all (builds and uv say nothing of theirs).
    pub done: u64,
    pub total: u64,
}

impl Progress {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // display only
    pub fn fraction(&self) -> f32 {
        (self.done as f64 / self.total.max(1) as f64).clamp(0., 1.) as f32
    }
}

/// What an installed engine's marker says.
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    version: String,
    /// The program to run, relative to the engine's folder.
    binary: String,
}

pub fn engine_dir(root: &Path, engine: &Engine) -> PathBuf {
    root.join("engines")
        .join(format!("{}-{}", engine.id, engine.version()))
}

pub fn model_dir(root: &Path, model: &Model, build: &Build) -> PathBuf {
    let revision = build
        .revision
        .as_deref()
        .map_or("engine", |rev| &rev[..12.min(rev.len())]);
    root.join("models")
        .join(&model.id)
        .join(format!("{}-{revision}", build.id))
}

/// The installed engine's program, if it is installed.
pub fn engine_binary(root: &Path, engine: &Engine) -> Option<PathBuf> {
    let dir = engine_dir(root, engine);
    let marker: Marker = serde_json::from_str(&fs::read_to_string(dir.join(MARKER)).ok()?).ok()?;
    let binary = dir.join(marker.binary);
    binary.exists().then_some(binary)
}

pub fn model_installed(root: &Path, model: &Model, build: &Build) -> bool {
    let dir = model_dir(root, model, build);
    dir.join(MARKER).is_file()
        && build.files.iter().all(|file| {
            fs::metadata(dir.join(&file.name)).is_ok_and(|meta| meta.len() == file.size)
        })
}

/// One thing a choice still needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub engine: bool,
    pub name: String,
    pub note: String,
    pub size: u64,
}

/// What `choice` still needs: its engine and its model, when they are not installed.
pub fn missing(root: &Path, choice: &Choice) -> Vec<Part> {
    let mut parts = Vec::new();
    let engine = choice.engine;
    if engine_binary(root, engine).is_none() {
        let size = crate::recommend::engine_download(engine, &choice.platform);
        let note = match &engine.install {
            Install::Archive { assets, .. } => assets
                .get(&choice.platform)
                .map_or_else(String::new, |asset| format!("{}, from GitHub", asset.name)),
            Install::Build { steps, .. } => format!(
                "Source code from GitHub, built on this computer ({}); takes several minutes",
                steps.join("; ")
            ),
            Install::Python { requirements, .. } => {
                let packages = requirements
                    .lines()
                    .filter(|line| line.contains("=="))
                    .count();
                format!("A private Python and {packages} locked packages, with uv")
            }
        };
        parts.push(Part {
            engine: true,
            name: format!("{} {}", engine.name, engine.version()),
            note,
            size,
        });
    }
    if !model_installed(root, choice.model, choice.build) {
        let build = choice.build;
        let note = match (&build.note, build.files.len()) {
            (Some(note), _) => note.clone(),
            (None, 1) => format!(
                "{}, from Hugging Face ({})",
                build.files[0].name, build.repo
            ),
            (None, n) => format!("{n} files from Hugging Face ({})", build.repo),
        };
        parts.push(Part {
            engine: false,
            name: format!("{} · {}", choice.model.name, build.quant),
            note,
            size: build.size,
        });
    }
    parts
}

/// Downloads (and builds) what `choice` still needs. `path` is the `PATH` builds run with (the
/// login shell's). Blocks; reports progress.
pub fn install(
    root: &Path,
    choice: &Choice,
    path: Option<&OsStr>,
    cancel: &AtomicBool,
    progress: &dyn Fn(Progress),
) -> Result<(), InstallError> {
    let total: u64 = missing(root, choice).iter().map(|part| part.size).sum();
    let mut done = 0;
    if engine_binary(root, choice.engine).is_none() {
        install_engine(root, choice, path, cancel, &mut |stage, bytes| {
            progress(Progress {
                stage,
                done: done + bytes,
                total,
            });
        })?;
        done += crate::recommend::engine_download(choice.engine, &choice.platform);
    }
    if !model_installed(root, choice.model, choice.build) {
        install_model(root, choice, path, cancel, &mut |bytes| {
            progress(Progress {
                stage: Stage::Model,
                done: done + bytes,
                total,
            });
        })?;
    }
    Ok(())
}

fn install_engine(
    root: &Path,
    choice: &Choice,
    path: Option<&OsStr>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Stage, u64),
) -> Result<(), InstallError> {
    let engine = choice.engine;
    let target = engine_dir(root, engine);
    let partial = target.with_extension("partial");
    python_env::remove(&partial).map_err(io("Removing", &partial))?;
    fs::create_dir_all(&partial).map_err(io("Creating", &partial))?;
    let binary = match &engine.install {
        Install::Archive { binary, assets, .. } => {
            let asset = assets.get(&choice.platform).ok_or_else(|| {
                InstallError::Failed(format!("{} has no download for this computer", engine.name))
            })?;
            let archive = partial.join(&asset.name);
            let pinned = Pinned {
                url: &asset.url,
                size: asset.size,
                sha256: &asset.sha256,
            };
            download_file(pinned, &archive, cancel, &mut |bytes| {
                progress(Stage::Engine, bytes);
            })?;
            let files = partial.join("files");
            unpack_archive(&archive, &files).map_err(io("Unpacking", &archive))?;
            let _ = fs::remove_file(&archive);
            let found = find_file(&files, binary).ok_or_else(|| {
                InstallError::Failed(format!("{binary} is not in {}", asset.name))
            })?;
            make_executable(&found).map_err(io("Preparing", &found))?;
            found
                .strip_prefix(&partial)
                .map_or_else(|_| found.clone(), Path::to_path_buf)
        }
        Install::Python { requirements, .. } => {
            let platform = python_env::catalog::platform().ok_or_else(|| {
                InstallError::Failed("Python engines do not run on this computer".into())
            })?;
            let tools_dir = partial.join("tools");
            progress(Stage::Python, 0);
            let tools = Tools::download(&tools_dir, platform, cancel, &mut |bytes| {
                progress(Stage::Python, bytes);
            })?;
            tools.python(&partial.join("python"))?;
            progress(Stage::Packages, platform.uv_size + platform.python_size);
            tools.packages(
                &partial.join("python"),
                requirements,
                &partial.join("site"),
                &mut |_| {},
            )?;
            drop(tools);
            python_env::remove(&tools_dir).map_err(io("Removing", &tools_dir))?;
            PathBuf::from("python")
        }
        Install::Build {
            binary,
            source,
            steps,
            ..
        } => {
            let tar = download_tar(
                &source.url,
                &source.tar_sha256,
                ChecksumOf::Tar,
                cancel,
                &mut |bytes| {
                    progress(Stage::Engine, bytes);
                },
            )?;
            let src = partial.join("src");
            python_env::fetch::unpack(&tar, &src, |_| true).map_err(io("Unpacking", &src))?;
            for step in steps {
                progress(Stage::Building(step.clone()), source.size);
                let mut command = shell(step, &src, path);
                python_env::run(
                    &mut command,
                    &format!("Building ({step})"),
                    cancel,
                    &mut |line| {
                        progress(
                            Stage::Building(line.chars().take(120).collect()),
                            source.size,
                        );
                    },
                )?;
            }
            let built = src.join(binary);
            if !built.exists() {
                return Err(InstallError::Failed(format!("the build made no {binary}")));
            }
            Path::new("src").join(binary)
        }
    };
    let marker = Marker {
        version: engine.version().to_owned(),
        binary: binary.to_string_lossy().into_owned(),
    };
    let text = serde_json::to_string(&marker).map_err(|e| InstallError::Failed(e.to_string()))?;
    fs::write(partial.join(MARKER), text).map_err(io("Writing", &partial))?;
    python_env::remove(&target).map_err(io("Removing", &target))?;
    fs::rename(&partial, &target).map_err(io("Installing", &target))
}

fn install_model(
    root: &Path,
    choice: &Choice,
    path: Option<&OsStr>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), InstallError> {
    let (model, build) = (choice.model, choice.build);
    let target = model_dir(root, model, build);
    let partial = target.with_extension("partial");
    fs::create_dir_all(&partial).map_err(io("Creating", &partial))?;
    if build.format == Format::Gturbo {
        // The engine's own installer fetches and repacks it, and resumes on its own.
        let Install::Build {
            model_step: Some(step),
            ..
        } = &choice.engine.install
        else {
            return Err(InstallError::Failed("no way to fetch this model".into()));
        };
        let src = engine_dir(root, choice.engine).join("src");
        let step = step.replace("{model_dir}", &quote(&partial.join("model.gturbo")));
        let mut command = shell(&step, &src, path);
        python_env::run(&mut command, "Fetching the model", cancel, &mut |_| {
            progress(0);
        })?;
    } else {
        let mut done = 0;
        for file in &build.files {
            let path = partial.join(&file.name);
            if !inside(&partial, &path) {
                return Err(InstallError::Failed(format!("odd file name {}", file.name)));
            }
            let present = fs::metadata(&path).is_ok_and(|meta| meta.len() == file.size)
                && file_sha256(&path).is_ok_and(|hash| hash == file.sha256);
            if !present {
                let tmp = path.with_extension("download");
                let pinned = Pinned {
                    url: &file.url,
                    size: file.size,
                    sha256: &file.sha256,
                };
                download_file(pinned, &tmp, cancel, &mut |bytes| progress(done + bytes))?;
                fs::rename(&tmp, &path).map_err(io("Saving", &path))?;
            }
            done += file.size;
            progress(done);
        }
    }
    let revision = build.revision.clone().unwrap_or_default();
    fs::write(partial.join(MARKER), revision).map_err(io("Writing", &partial))?;
    python_env::remove(&target).map_err(io("Removing", &target))?;
    fs::rename(&partial, &target).map_err(io("Installing", &target))
}

/// Deletes a model's files.
pub fn remove_model(root: &Path, model: &Model, build: &Build) -> std::io::Result<()> {
    python_env::remove(&model_dir(root, model, build))?;
    // The model's folder too, once no other build of it is left.
    let _ = fs::remove_dir(root.join("models").join(&model.id));
    Ok(())
}

/// Deletes what `setup` downloaded: its model and its engine, each unless one of `others` (the
/// setups that stay) uses it too.
pub fn uninstall(
    root: &Path,
    catalog: &Catalog,
    setup: &Setup,
    others: &[Setup],
) -> std::io::Result<()> {
    let shares_model = others
        .iter()
        .any(|other| other.model == setup.model && other.build == setup.build);
    if !shares_model
        && let Some(model) = catalog.model(&setup.model)
        && let Some(build) = model.build(&setup.build)
    {
        remove_model(root, model, build)?;
    }
    let shares_engine = others.iter().any(|other| other.engine == setup.engine);
    if !shares_engine && let Some(engine) = catalog.engine(&setup.engine) {
        python_env::remove(&engine_dir(root, engine))?;
    }
    Ok(())
}

/// `step` run by the shell in `dir`, its output going to standard error (which is read).
fn shell(step: &str, dir: &Path, path: Option<&OsStr>) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", &format!("exec 2>&1 1>&2; {step}")])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(python_env::fetch::hex(&hasher.finalize()))
}

/// Whether `path` stays inside `dir` (no `..`, not absolute).
fn inside(dir: &Path, path: &Path) -> bool {
    path.strip_prefix(dir).is_ok_and(|rest| {
        rest.components()
            .all(|part| matches!(part, Component::Normal(_)))
    })
}

/// Unpacks a `.tar.gz` or `.zip` into `into`, keeping its folders, refusing anything that would
/// land outside it, and links that point outside it.
fn unpack_archive(archive: &Path, into: &Path) -> std::io::Result<()> {
    fs::create_dir_all(into)?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
        let mut total = 0;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(relative) = entry.enclosed_name() else {
                continue;
            };
            let target = into.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&target)?;
                continue;
            }
            total += entry.size();
            if total > MAX_UNPACKED {
                return Err(std::io::Error::other("the archive unpacks to too much"));
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut fs::File::create(&target)?)?;
        }
        return Ok(());
    }
    let reader = std::io::Read::take(GzDecoder::new(fs::File::open(archive)?), MAX_UNPACKED);
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if !path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        {
            continue;
        }
        let target = into.join(&path);
        let kind = entry.header().entry_type();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            entry.unpack(&target)?;
        } else if kind.is_symlink() {
            // Libraries are linked to their versioned names; only links within the archive.
            let Some(link) = entry.link_name()? else {
                continue;
            };
            let resolved = target.parent().unwrap_or(into).join(&link);
            let normal = normalize(&resolved);
            if link.is_absolute() || !normal.starts_with(normalize(into)) {
                continue;
            }
            #[cfg(unix)]
            {
                let _ = fs::remove_file(&target);
                std::os::unix::fs::symlink(&link, &target)?;
            }
        }
    }
    Ok(())
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The first file called `name` (its last part, for a path like `build/bin/llama-server`) below
/// `dir`, the shallowest first.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let wanted = Path::new(name).file_name()?;
    let windows = format!("{}.exe", wanted.to_string_lossy());
    let mut queue = std::collections::VecDeque::from([dir.to_path_buf()]);
    while let Some(next) = queue.pop_front() {
        let mut entries: Vec<_> = fs::read_dir(&next).ok()?.flatten().collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let file_name = entry.file_name();
            if path.is_dir() {
                queue.push_back(path);
            } else if file_name == wanted || file_name.to_string_lossy() == windows {
                return Some(path);
            }
        }
    }
    None
}

fn make_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)?;
    }
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(entries: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, link, data) in entries {
            let mut header = tar::Header::new_gnu();
            match link {
                Some(target) => {
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_size(0);
                    builder.append_link(&mut header, path, target).unwrap();
                }
                None => {
                    header.set_size(data.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    builder.append_data(&mut header, path, *data).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// A model and engine another setup still uses are kept; the rest is deleted.
    #[test]
    fn uninstall_keeps_what_others_use() {
        let catalog = crate::catalog();
        let model = &catalog.models[0];
        let build = &model.builds[0];
        let engine = catalog.engine(&build.engines[0]).unwrap();
        let other = catalog
            .engines
            .iter()
            .find(|other| other.id != engine.id)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            model_dir(root, model, build),
            engine_dir(root, engine),
            engine_dir(root, other),
        ] {
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(MARKER), "").unwrap();
        }
        let setup = |engine: &Engine| Setup {
            model: model.id.clone(),
            build: build.id.clone(),
            engine: engine.id.clone(),
            port: 1,
            context: 1,
        };
        let (gone, stays) = (setup(engine), setup(other));

        uninstall(root, catalog, &gone, std::slice::from_ref(&stays)).unwrap();
        assert!(model_dir(root, model, build).exists(), "still used");
        assert!(!engine_dir(root, engine).exists());
        assert!(engine_dir(root, other).exists());

        uninstall(root, catalog, &stays, &[]).unwrap();
        assert!(!root.join("models").join(&model.id).exists());
        assert!(!engine_dir(root, other).exists());
    }

    #[test]
    fn unpacks_engines_with_their_links_and_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("llama.tar.gz");
        fs::write(
            &archive,
            tar_gz(&[
                ("llama-b1/llama-server", None, b"#!/bin/sh\n"),
                ("llama-b1/libllama.0.dylib", None, b"lib"),
                ("llama-b1/libllama.dylib", Some("libllama.0.dylib"), b""),
                ("llama-b1/evil", Some("../../../etc/passwd"), b""),
            ]),
        )
        .unwrap();
        let into = dir.path().join("out");
        unpack_archive(&archive, &into).unwrap();
        let binary = find_file(&into, "build/bin/llama-server").unwrap();
        assert_eq!(binary, into.join("llama-b1/llama-server"));
        make_executable(&binary).unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(into.join("llama-b1/libllama.dylib")).unwrap(),
            Path::new("libllama.0.dylib")
        );
        assert!(!into.join("llama-b1/evil").exists());
    }

    #[test]
    fn keeps_downloads_inside_their_folder() {
        let dir = Path::new("/models/x.partial");
        assert!(inside(dir, &dir.join("a/b.gguf")));
        assert!(!inside(dir, &dir.join("../escape")));
        assert_eq!(quote(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }
}
