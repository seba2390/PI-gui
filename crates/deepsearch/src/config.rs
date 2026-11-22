//! Web search settings: `config.json` and the instructions for each mode (`prompt-search.md`,
//! `prompt-deep.md`), all in [`default_dir`]. Missing files are written with the defaults on
//! first use, missing keys are added with their defaults, and the files are re-read on every
//! use, so edits apply to the next message.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.json";
pub const SEARCH_PROMPT_FILE: &str = "prompt-search.md";
pub const DEEP_PROMPT_FILE: &str = "prompt-deep.md";
/// The environment variable that overrides [`default_dir`] (set by the app for pi's children).
pub const DIR_ENV: &str = "PISTACHIO_DEEPSEARCH_DIR";

/// The instructions added to the system prompt in each mode. `{{date}}`, `{{max_searches}}`
/// and `{{max_fetches}}` are filled in.
pub const DEFAULT_SEARCH_PROMPT: &str = include_str!("prompt-search.md");
pub const DEFAULT_DEEP_PROMPT: &str = include_str!("prompt-deep.md");

/// How a chat uses the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// No web tools.
    #[default]
    Off,
    /// A quick lookup: a few searches and pages, then a concise answer.
    Search,
    /// A thorough investigation: plan, many searches and pages, cross-checking, a report.
    Deep,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Search => "search",
            Self::Deep => "deep",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "off" => Some(Self::Off),
            "search" => Some(Self::Search),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// `$PISTACHIO_DEEPSEARCH_DIR`, else `<pi config dir>/pistachio/deepsearch` (pi's config dir is
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
    Some(agent.join("pistachio").join("deepsearch"))
}

/// Which search service answers `web_search`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    #[default]
    Duckduckgo,
    Searxng,
    Brave,
    Tavily,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `duckduckgo`, `searxng`, `brave` or `tavily`; each has its own section below.
    pub backend: BackendKind,
    /// The mode new chats start in: `off`, `search` or `deep` (each chat remembers its own).
    pub default_mode: Mode,
    pub duckduckgo: DuckDuckGo,
    pub searxng: Searxng,
    pub brave: ApiKey,
    pub tavily: Tavily,
    pub search: Search,
    pub fetch: Fetch,
    pub modes: Modes,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendKind::default(),
            default_mode: Mode::Off,
            duckduckgo: DuckDuckGo::default(),
            searxng: Searxng::default(),
            brave: ApiKey {
                api_key: None,
                api_key_env: "BRAVE_API_KEY".into(),
            },
            tavily: Tavily::default(),
            search: Search::default(),
            fetch: Fetch::default(),
            modes: Modes::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckDuckGo {
    /// Region code such as `us-en` or `de-de`; empty for no region.
    pub region: String,
    /// `strict`, `moderate` or `off`.
    pub safe_search: String,
}

impl Default for DuckDuckGo {
    fn default() -> Self {
        Self {
            region: String::new(),
            safe_search: "moderate".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Searxng {
    /// The instance's base URL. Its `settings.yml` must list `json` under `search.formats`.
    pub url: String,
    /// Comma-separated engines (e.g. `google,bing,wikipedia`); empty for the instance default.
    pub engines: String,
    /// Language code such as `en`; empty for the instance default.
    pub language: String,
}

impl Default for Searxng {
    fn default() -> Self {
        Self {
            url: "http://localhost:8080".into(),
            engines: String::new(),
            language: String::new(),
        }
    }
}

/// A hosted API's key: given directly, or (preferably) read from an environment variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ApiKey {
    pub api_key: Option<String>,
    pub api_key_env: String,
}

impl ApiKey {
    pub fn resolve(&self) -> Option<String> {
        self.api_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| std::env::var(&self.api_key_env).ok())
            .filter(|key| !key.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tavily {
    pub api_key: Option<String>,
    pub api_key_env: String,
    /// `basic` (1 credit) or `advanced` (2 credits).
    pub search_depth: String,
}

impl Default for Tavily {
    fn default() -> Self {
        Self {
            api_key: None,
            api_key_env: "TAVILY_API_KEY".into(),
            search_depth: "basic".into(),
        }
    }
}

impl Tavily {
    pub fn key(&self) -> ApiKey {
        ApiKey {
            api_key: self.api_key.clone(),
            api_key_env: self.api_key_env.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    /// Results per search when the model does not ask for a number.
    pub max_results: usize,
    pub timeout_secs: u64,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            max_results: 8,
            timeout_secs: 15,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fetch {
    pub timeout_secs: u64,
    /// Pages are cut off after this many bytes.
    pub max_download_bytes: usize,
    /// At most this much page text goes back to the model per `web_fetch`. Lower it for small
    /// context windows (roughly 4 characters per token).
    pub max_chars: usize,
    /// Pages are split into passages of about this size, and the ones most relevant to the
    /// model's `focus` are kept.
    pub passage_chars: usize,
    /// Allow `web_fetch` to reach localhost and private networks (off: only public addresses).
    pub allow_private_networks: bool,
    pub user_agent: String,
}

impl Default for Fetch {
    fn default() -> Self {
        Self {
            timeout_secs: 20,
            max_download_bytes: 4_000_000,
            max_chars: 12_000,
            passage_chars: 900,
            allow_private_networks: false,
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/131.0 Safari/537.36"
                .into(),
        }
    }
}

/// Limits per user message in each mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Modes {
    pub search: SearchBudget,
    pub deep: DeepBudget,
}

macro_rules! budget {
    ($name:ident, $searches:expr, $fetches:expr) => {
        /// Further calls are refused and the model is told to answer with what it has.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name {
            pub max_searches: u32,
            pub max_fetches: u32,
        }

        impl Default for $name {
            fn default() -> Self {
                Self {
                    max_searches: $searches,
                    max_fetches: $fetches,
                }
            }
        }
    };
}

budget!(SearchBudget, 3, 3);
budget!(DeepBudget, 15, 25);

/// The instructions for each mode, as read from disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Prompts {
    pub search: String,
    pub deep: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read or write {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path} is not valid: {source}")]
    Invalid {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// The settings and instructions in `dir`, writing the defaults for missing files and keys.
pub fn load(dir: &Path) -> Result<(Config, Prompts), ConfigError> {
    fs::create_dir_all(dir).map_err(io_error(dir))?;

    let config_path = dir.join(CONFIG_FILE);
    let config = match fs::read_to_string(&config_path) {
        Ok(text) => {
            let invalid = |source| ConfigError::Invalid {
                path: config_path.clone(),
                source,
            };
            let written: serde_json::Value = serde_json::from_str(&text).map_err(invalid)?;
            let config: Config = serde_json::from_value(written.clone()).map_err(invalid)?;
            // Show settings added since the file was written (and drop retired ones).
            let complete = serde_json::to_value(&config).unwrap_or_default();
            if has_missing_keys(&written, &complete) {
                write_config(&config_path, &config)?;
            }
            config
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let config = Config::default();
            write_config(&config_path, &config)?;
            config
        }
        Err(error) => return Err(io_error(&config_path)(error)),
    };

    let prompts = Prompts {
        search: read_or_write(&dir.join(SEARCH_PROMPT_FILE), DEFAULT_SEARCH_PROMPT)?,
        deep: read_or_write(&dir.join(DEEP_PROMPT_FILE), DEFAULT_DEEP_PROMPT)?,
    };
    Ok((config, prompts))
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ConfigError {
    let path = path.to_path_buf();
    move |source| ConfigError::Io { path, source }
}

/// Saves `config` as `dir`'s `config.json` (from the Settings page).
pub fn save(dir: &Path, config: &Config) -> Result<(), ConfigError> {
    fs::create_dir_all(dir).map_err(io_error(dir))?;
    write_config(&dir.join(CONFIG_FILE), config)
}

/// Writes atomically (temp file + rename) and readable only by the user: the file can hold
/// search API keys.
fn write_config(path: &Path, config: &Config) -> Result<(), ConfigError> {
    use std::io::Write as _;
    let text = serde_json::to_string_pretty(config).unwrap_or_default() + "\n";
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp).map_err(io_error(&tmp))?;
    file.write_all(text.as_bytes()).map_err(io_error(&tmp))?;
    drop(file);
    fs::rename(&tmp, path).map_err(io_error(path))
}

fn read_or_write(path: &Path, default: &str) -> Result<String, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::write(path, default).map_err(io_error(path))?;
            Ok(default.to_owned())
        }
        Err(error) => Err(io_error(path)(error)),
    }
}

/// Whether `complete` has an object key that `written` lacks, at any depth.
fn has_missing_keys(written: &serde_json::Value, complete: &serde_json::Value) -> bool {
    match (written, complete) {
        (serde_json::Value::Object(written), serde_json::Value::Object(complete)) => {
            complete.iter().any(|(key, value)| match written.get(key) {
                None => true,
                Some(written) => has_missing_keys(written, value),
            })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn writes_defaults_then_reads_partial_edits() {
        let dir = tempfile::tempdir().unwrap();
        let (config, prompts) = load(dir.path()).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(prompts.deep, DEFAULT_DEEP_PROMPT);
        assert_eq!(prompts.search, DEFAULT_SEARCH_PROMPT);
        assert!(dir.path().join(CONFIG_FILE).exists());

        fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{"backend":"searxng","searxng":{"url":"http://box:8888"},"modes":{"deep":{"max_fetches":3}}}"#,
        )
        .unwrap();
        let (config, _) = load(dir.path()).unwrap();
        assert_eq!(config.backend, BackendKind::Searxng);
        assert_eq!(config.searxng.url, "http://box:8888");
        assert_eq!(config.modes.deep.max_fetches, 3);
        assert_eq!(
            config.modes.deep.max_searches,
            DeepBudget::default().max_searches
        );
        assert_eq!(config.modes.search, Modes::default().search);
        // The file now lists every setting, keeping the edits.
        let rewritten = fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
        assert!(rewritten.contains("\"default_mode\": \"off\""));
        assert!(rewritten.contains("http://box:8888"));

        fs::write(dir.path().join(CONFIG_FILE), r#"{"backend":"bing"}"#).unwrap();
        assert!(matches!(load(dir.path()), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn save_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            backend: BackendKind::Brave,
            ..Config::default()
        };
        config.brave.api_key = Some("secret".into());
        save(dir.path(), &config).unwrap();
        let (loaded, _) = load(dir.path()).unwrap();
        assert_eq!(loaded, config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(dir.path().join(CONFIG_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
