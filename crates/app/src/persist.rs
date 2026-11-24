//! Small settings files: loading that loses as little as possible, and background saves that
//! never land out of order.
//!
//! Saves run off the UI thread, so two quick changes could otherwise finish in the wrong
//! order (an older snapshot renamed over a newer one) or write the same temp file at once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use gpui_kit::App;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

static NEXT: AtomicU64 = AtomicU64::new(1);
/// The newest save written per file. Holding the lock while writing also serializes writes.
static WRITTEN: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

/// Reads the JSON object in `path`; a missing file gives the defaults.
///
/// A value that can't be read (one a newer version wrote, or a typo) only loses itself: every
/// other key keeps its value. When something is lost, the file is kept as `<path>.corrupt`,
/// since the next save replaces it. `T` must default its missing fields (`#[serde(default)]`).
pub fn load<T: Default + Serialize + DeserializeOwned>(path: &Path) -> T {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("could not read {}: {error}", path.display());
            }
            return T::default();
        }
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut backup = path.as_os_str().to_owned();
            backup.push(".corrupt");
            let backup = PathBuf::from(backup);
            let _ = std::fs::copy(path, &backup);
            log::warn!(
                "using defaults for what could not be read in {} (kept as {}): {error}",
                path.display(),
                backup.display()
            );
            serde_json::from_slice(&bytes).map_or_else(|_| T::default(), readable_values)
        }
    }
}

/// `T` from the values in `saved` that can be read, each on its own.
fn readable_values<T: Default + DeserializeOwned>(saved: Map<String, Value>) -> T {
    let readable: Map<String, Value> = saved
        .into_iter()
        .filter(|(key, value)| {
            let alone = Map::from_iter([(key.clone(), value.clone())]);
            let read = serde_json::from_value::<T>(Value::Object(alone));
            if let Err(error) = &read {
                log::warn!("ignoring the saved value of \"{key}\": {error}");
            }
            read.is_ok()
        })
        .collect();
    serde_json::from_value(Value::Object(readable)).unwrap_or_default()
}

/// Runs `write` for `path` in the background, skipping it if a newer save of the same file
/// already ran.
pub fn save_in_background(
    cx: &App,
    path: PathBuf,
    write: impl FnOnce(&Path) -> std::io::Result<()> + Send + 'static,
) {
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    cx.background_executor()
        .spawn(async move { save_ordered(&path, sequence, write) })
        .detach();
}

fn save_ordered(path: &Path, sequence: u64, write: impl FnOnce(&Path) -> std::io::Result<()>) {
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    let written = written.get_or_insert_with(HashMap::new);
    if written.get(path).is_some_and(|newest| *newest > sequence) {
        return;
    }
    match write(path) {
        Ok(()) => {
            written.insert(path.to_owned(), sequence);
        }
        Err(error) => log::warn!("could not save {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::save_ordered;

    #[test]
    fn an_older_save_never_overwrites_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let write = |text: &'static str| move |path: &std::path::Path| std::fs::write(path, text);
        save_ordered(&path, 2, write("new"));
        save_ordered(&path, 1, write("old"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        save_ordered(&path, 3, write("newest"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "newest");
    }
}
