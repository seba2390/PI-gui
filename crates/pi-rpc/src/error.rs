//! Error type shared by the client and process layers.

use thiserror::Error;

/// Failure modes when talking to a pi RPC process.
#[derive(Debug, Error)]
pub enum RpcError {
    /// pi handled the command but reported failure (`success: false`).
    #[error("pi rejected `{command}`: {message}")]
    Rejected { command: String, message: String },

    /// The process exited (or its pipes closed) before the response arrived.
    #[error("pi process is not running")]
    Disconnected,

    /// The response arrived but its `data` did not match the expected shape.
    #[error("unexpected response data for `{command}`: {source}")]
    Decode {
        command: String,
        #[source]
        source: serde_json::Error,
    },

    /// Spawning or talking to the process failed at the OS level.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
