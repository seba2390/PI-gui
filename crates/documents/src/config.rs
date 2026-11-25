//! The settings of document parsing: `config.json` in [`default_dir`]. Re-read on every use, also
//! by the helper processes, so a change applies to the next document.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Parser;

pub const CONFIG_FILE: &str = "config.json";
/// The environment variable that overrides [`default_dir`] (set by the app for pi's children).
pub const DIR_ENV: &str = "PISTACHIO_DOCUMENTS_DIR";

/// `$PISTACHIO_DOCUMENTS_DIR`, else `<pi config dir>/pistachio/documents` (pi's config dir is
/// `$PI_CODING_AGENT_DIR` or `~/.pi/agent`).
pub fn default_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    let agent = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".pi").join("agent"))
        })?;
    Some(agent.join("pistachio").join("documents"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `builtin`, `pymupdf4llm`, `docling` or `mineru`.
    pub parser: Parser,
    /// How long a downloaded parser may take for one document; then the built-in reader is used.
    pub timeout_secs: u64,
    /// The same for a PDF a chat reads from the web.
    pub web_timeout_secs: u64,
    /// Larger files are not read.
    pub max_file_bytes: u64,
    /// Longer texts are shortened when they go along with a message of a chat; `null` for only
    /// those the model has no room for.
    pub whole_text_bytes: Option<u64>,
}

pub const WHOLE_TEXT_BYTES: u64 = 256 * 1024;

impl Default for Config {
    fn default() -> Self {
        Self {
            parser: Parser::Builtin,
            timeout_secs: 600,
            web_timeout_secs: 120,
            max_file_bytes: 100_000_000,
            whole_text_bytes: Some(WHOLE_TEXT_BYTES),
        }
    }
}

impl Config {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub fn web_timeout(&self) -> Duration {
        Duration::from_secs(self.web_timeout_secs)
    }
}

/// The settings in `dir`. A missing file or key has its default, and so has a value that cannot
/// be read: a document is better read with the built-in reader than not at all.
pub fn load(dir: &Path) -> Config {
    let written: serde_json::Map<String, serde_json::Value> =
        fs::read_to_string(dir.join(CONFIG_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
    let mut config = serde_json::Map::new();
    for (key, value) in written {
        let alone = serde_json::json!({ key.as_str(): value.clone() });
        if serde_json::from_value::<Config>(alone).is_ok() {
            config.insert(key, value);
        }
    }
    serde_json::from_value(config.into()).unwrap_or_default()
}

/// Saves `config` as `dir`'s `config.json` (from the Settings page), atomically.
pub fn save(dir: &Path, config: &Config) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(CONFIG_FILE);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(config)? + "\n")?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn missing_and_unreadable_settings_have_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Config::default());
        assert_eq!(Config::default().parser, Parser::Builtin);
        let write = |text: &str| fs::write(dir.path().join(CONFIG_FILE), text).unwrap();
        write(r#"{"parser": "docling"}"#);
        let config = load(dir.path());
        assert_eq!(config.parser, Parser::Docling);
        assert_eq!(config.timeout_secs, Config::default().timeout_secs);
        // A parser of a newer version of the app.
        write(r#"{"parser": "marker", "timeout_secs": 30}"#);
        let config = load(dir.path());
        assert_eq!((config.parser, config.timeout_secs), (Parser::Builtin, 30));
        write(r#"{"whole_text_bytes": null}"#);
        assert_eq!(load(dir.path()).whole_text_bytes, None);
        write("{ not json");
        assert_eq!(load(dir.path()), Config::default());
    }

    #[test]
    fn saved_settings_are_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            parser: Parser::Mineru,
            web_timeout_secs: 20,
            ..Config::default()
        };
        let dir = dir.path().join("documents");
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir), config);
        assert!(!dir.join("config.json.tmp").exists());
    }
}
