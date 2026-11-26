//! Which sandbox this computer can use: Seatbelt on macOS; on Linux bubblewrap, or Landlock
//! where bubblewrap is missing or can't run. Checking bubblewrap starts it once, so the result
//! is kept (`backend.json` in the sandbox's folder) until bubblewrap or the kernel changes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

/// What sandboxed commands run under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Backend {
    /// macOS's `sandbox-exec`.
    Seatbelt,
    /// bubblewrap at this path.
    Bubblewrap {
        path: PathBuf,
    },
    Landlock,
}

impl Backend {
    /// Its name in the app.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Seatbelt => "macOS Seatbelt",
            Self::Bubblewrap { .. } => "bubblewrap",
            Self::Landlock => "Landlock",
        }
    }
}

/// What the app shows about the sandbox on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// `None`: commands can't be sandboxed here.
    pub backend: Option<Backend>,
    /// Whether turning the network off is enforced.
    pub network: bool,
    /// Why the preferred sandbox isn't used, or none is.
    pub problem: Option<String>,
    /// A command that would fix it.
    pub fix: Option<String>,
}

impl Status {
    fn unavailable(problem: impl Into<String>, fix: Option<&str>) -> Self {
        Self {
            backend: None,
            network: false,
            problem: Some(problem.into()),
            fix: fix.map(str::to_owned),
        }
    }
}

const CACHE: &str = "backend.json";

#[derive(Serialize, Deserialize)]
struct Cached {
    key: String,
    status: Status,
}

/// The sandbox to use, from the cache in `dir` when it is still valid. `exe` is this program,
/// which checks Landlock in a process of its own.
pub fn current(dir: &Path, exe: &Path) -> Status {
    let key = cache_key();
    let cached = std::fs::read_to_string(dir.join(CACHE))
        .ok()
        .and_then(|text| serde_json::from_str::<Cached>(&text).ok())
        .filter(|cached| cached.key == key);
    match cached {
        Some(cached) => cached.status,
        None => detect(dir, exe),
    }
}

/// Checks again which sandbox works and remembers it in `dir`.
pub fn detect(dir: &Path, exe: &Path) -> Status {
    let status = check(exe);
    let cached = Cached {
        key: cache_key(),
        status: status.clone(),
    };
    if let Ok(text) = serde_json::to_string(&cached) {
        let _ = std::fs::create_dir_all(dir);
        let temp = dir.join(format!("{CACHE}.tmp"));
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(temp, dir.join(CACHE));
        }
    }
    status
}

/// What the cached result depends on: bubblewrap (where and which build) and the kernel.
fn cache_key() -> String {
    let bwrap = find_bwrap().map(|path| {
        let modified = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        format!("{}@{modified}", path.display())
    });
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    format!("{}|{}", bwrap.unwrap_or_default(), kernel.trim())
}

#[cfg(target_os = "macos")]
fn check(_exe: &Path) -> Status {
    let works = Command::new(crate::seatbelt::SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if works {
        Status {
            backend: Some(Backend::Seatbelt),
            network: true,
            problem: None,
            fix: None,
        }
    } else {
        Status::unavailable("macOS's sandbox-exec is missing or does not work", None)
    }
}

#[cfg(target_os = "linux")]
fn check(exe: &Path) -> Status {
    const INSTALL: &str = "sudo apt install bubblewrap";
    let bwrap = find_bwrap();
    if let Some(path) = &bwrap
        && crate::bwrap::works(path)
    {
        return Status {
            backend: Some(Backend::Bubblewrap { path: path.clone() }),
            network: true,
            problem: None,
            fix: None,
        };
    }
    let (problem, fix) = match &bwrap {
        Some(path) => (
            format!(
                "bubblewrap ({}) can't make a sandbox here: this system does not let it \
                 (on Ubuntu, AppArmor needs bubblewrap installed from Ubuntu's own packages)",
                path.display()
            ),
            Some(INSTALL),
        ),
        None => ("bubblewrap is not installed".to_owned(), Some(INSTALL)),
    };
    match probe_landlock(exe) {
        Ok(enforced) => Status {
            backend: Some(Backend::Landlock),
            network: enforced.network,
            problem: Some(problem),
            fix: fix.map(str::to_owned),
        },
        Err(landlock) => Status::unavailable(format!("{problem}, and {landlock}"), fix),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn check(_exe: &Path) -> Status {
    Status::unavailable("there is no sandbox on Windows yet", None)
}

/// Applies Landlock in a process of its own (it can't be undone) and reports what it enforced.
#[cfg(target_os = "linux")]
fn probe_landlock(exe: &Path) -> Result<crate::landlock::Enforced, String> {
    let output = Command::new(exe)
        .args([crate::cli::FLAG, crate::cli::LANDLOCK_PROBE])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("Landlock could not be checked: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let reply: serde_json::Value = serde_json::from_str(text.trim().lines().last().unwrap_or(""))
        .map_err(|_| "Landlock could not be checked".to_owned())?;
    if output.status.success() {
        Ok(crate::landlock::Enforced {
            network: reply["network"].as_bool().unwrap_or(false),
        })
    } else {
        Err(reply["error"]
            .as_str()
            .unwrap_or("Landlock is not available")
            .to_owned())
    }
}

/// bubblewrap on `PATH` (or where distributions put it).
pub fn find_bwrap() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .map(|dir| dir.join("bwrap"))
        .find(|candidate| candidate.is_file())
}
