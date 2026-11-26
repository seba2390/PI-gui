//! The sandbox for the agent's tools in Pistachio's local Code sessions (Settings → Security).
//!
//! pi itself runs as usual, with its logins and keys. A pi extension ([`EXTENSION_SOURCE`])
//! runs each `bash` command through this crate's helper (`<app> --sandbox-tool run`, see
//! [`cli`]), which starts it in the sandbox this computer has ([`backend`]):
//!
//! - macOS: Seatbelt ([`seatbelt`]);
//! - Linux: bubblewrap ([`bwrap`]), or Landlock ([`landlock`]) where bubblewrap can't run.
//!
//! Commands may write only to the project, temporary folders, tool caches and folders the user
//! adds; they can't read hidden paths (keys, logins), don't get secret environment variables
//! ([`secrets`]) and, when the user says so, have no network. The extension holds pi's file tools
//! (`read`, `write`, `edit`, `grep`, …) to the same rules ([`Policy`]).

pub mod backend;
mod bwrap;
pub mod cli;
pub mod landlock;
pub mod note;
pub mod paths;
pub mod policy;
pub mod seatbelt;
pub mod secrets;
pub mod settings;

use std::io;
use std::path::{Path, PathBuf};

pub use backend::{Backend, Status};
pub use cli::{Check, PolicyReply, TestReport};
pub use policy::{Context, Policy};
pub use settings::{DEFAULT_HIDDEN, Network, Settings, TOOL_CACHES};

/// The pi extension that runs the agent's tools in the sandbox.
pub const EXTENSION_SOURCE: &str = include_str!("extension.ts");
/// The app's executable, which the extension runs as the helper.
pub const BIN_ENV: &str = "PISTACHIO_SANDBOX_BIN";
/// The sandbox's folder: its settings and what it found out about this computer.
pub const DIR_ENV: &str = "PISTACHIO_SANDBOX_DIR";

/// Whether this system has a sandbox the app can use (macOS and Linux).
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// `$PISTACHIO_SANDBOX_DIR`, else `<pi's agent folder>/pistachio/sandbox`.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return PathBuf::from(dir);
    }
    std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| paths::home().map(|home| home.join(".pi").join("agent")))
        .map_or_else(
            || std::env::temp_dir().join("pistachio-sandbox"),
            |agent| agent.join("pistachio").join("sandbox"),
        )
}

/// Writes the extension into `dir` (when it changed) and returns its path.
pub fn install_extension(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("extension.ts");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        std::fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}
