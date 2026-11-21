//! Typed client for pi's RPC mode.
//!
//! pi (`@earendil-works/pi-coding-agent`) runs as a long-lived child process with
//! `pi --mode rpc`, reading JSON commands from stdin and writing responses, session
//! events and extension UI requests to stdout as strict JSONL.
//!
//! This crate is UI-agnostic and runtime-agnostic:
//!
//! - [`protocol`] holds serde types mirroring pi's `rpc-types.ts`, `json-event.ts`
//!   and message types.
//! - [`framing`] reads and writes JSONL records exactly as pi specifies (split only on
//!   LF, tolerate CRLF, never split on Unicode line separators).
//! - [`client`] correlates commands with responses over any byte stream pair and
//!   delivers everything else as [`client::ClientEvent`]s on an async channel.
//! - [`process`] spawns and supervises the `pi` executable.

pub mod client;
pub mod error;
pub mod framing;
pub mod process;
pub mod protocol;

pub use client::{ClientEvent, PiClient};
pub use error::RpcError;
pub use process::{Launcher, PiProcess, SpawnOptions};
