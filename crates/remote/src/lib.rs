//! UI-agnostic SSH support for PI-gui: pi runs *on* the remote machine and the GUI talks to it
//! over `ssh host … pi --mode rpc`.
//!
//! * [`config`] — host aliases discovered from the user's OpenSSH config, and `ssh -G`
//!   resolution of a target to its effective hostname, user and port.
//! * [`host`] — saved connections (`<agent dir>/pi-gui/hosts.json`).
//! * [`ssh`] — talking to one host: a shared (`ControlMaster`) connection, remote shell scripts,
//!   probing for pi, listing remote folders, and the command line that starts pi remotely.
//!
//! Everything goes through the system `ssh` binary so the user's `~/.ssh/config`, keys,
//! `ssh-agent`, `known_hosts` and `ProxyJump` all behave exactly as in a terminal. All functions
//! are synchronous and may block on process I/O; callers are expected to run them on a
//! background executor.

pub mod config;
mod error;
pub mod host;
mod process;
pub mod ssh;

pub use config::{ConfigHost, Resolved, discover_hosts, parse_config, resolve};
pub use error::{RemoteError, classify};
pub use host::{HostId, HostStore, SavedHost, Target};
pub use ssh::{DirListing, Probe, Ssh, sh_quote};
