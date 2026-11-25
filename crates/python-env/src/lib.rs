//! A private Python for the programs the app can download (SearXNG, document parsers).
//!
//! Nothing of it ships with the app, and no other Python on the computer is touched. [`Tools`]
//! downloads uv, installs a Python with it and puts locked packages in a folder; [`fetch`]
//! downloads further files. Everything is pinned and checked against a SHA-256.

pub mod catalog;
pub mod fetch;
mod install;
mod run;

pub use install::{InstallError, Tools, io, python_in, remove};
pub use run::run;

/// Network settings the programs started here keep from the app's environment.
pub fn network_env() -> impl Iterator<Item = (&'static str, std::ffi::OsString)> {
    [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ]
    .into_iter()
    .filter_map(|name| Some((name, std::env::var_os(name)?)))
}
