//! What was set up (`setups.json`): a model, its build, the engine that runs it and its port, so
//! it can be started again when a chat uses it. The port stays the same, because pi's
//! `models.json` names it.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ports from here on are given to setups, one each.
const FIRST_PORT: u16 = 39_280;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setup {
    pub model: String,
    pub build: String,
    pub engine: String,
    pub port: u16,
    /// Context the engine is started with.
    pub context: u64,
}

impl Setup {
    /// `<model>/<build>/<engine>`, as [`crate::Choice::key`].
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.model, self.build, self.engine)
    }

    /// The address pi connects to.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn file(root: &Path) -> PathBuf {
    root.join("setups.json")
}

pub fn load(root: &Path) -> Vec<Setup> {
    fs::read_to_string(file(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(root: &Path, setups: &[Setup]) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    let text = serde_json::to_string_pretty(setups).map_err(std::io::Error::other)?;
    let tmp = file(root).with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, file(root))
}

/// A port no setup has and nothing listens on.
pub fn free_port(setups: &[Setup]) -> Option<u16> {
    (FIRST_PORT..FIRST_PORT + 200).find(|port| {
        !setups.iter().any(|setup| setup.port == *port)
            && TcpListener::bind(("127.0.0.1", *port)).is_ok()
    })
}

/// The setup listening at `base_url` (`http://127.0.0.1:<port>/v1` or `localhost`), if any.
pub fn at<'a>(setups: &'a [Setup], base_url: &str) -> Option<&'a Setup> {
    let rest = base_url
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| base_url.strip_prefix("http://localhost:"))?;
    let port: u16 = rest.split('/').next()?.parse().ok()?;
    setups.iter().find(|setup| setup.port == port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_finds_and_allocates_ports() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
        let setup = Setup {
            model: "qwen3.5-9b".into(),
            build: "mlx-4bit".into(),
            engine: "rapid-mlx".into(),
            port: free_port(&[]).unwrap(),
            context: 32_768,
        };
        save(dir.path(), std::slice::from_ref(&setup)).unwrap();
        let setups = load(dir.path());
        assert_eq!(setups, [setup.clone()]);
        assert_ne!(free_port(&setups), Some(setup.port));
        assert_eq!(at(&setups, &setup.base_url()), Some(&setup));
        let localhost = format!("http://localhost:{}/v1", setup.port);
        assert_eq!(at(&setups, &localhost), Some(&setup));
        assert_eq!(at(&setups, "http://192.168.1.2:8000/v1"), None);
    }
}
