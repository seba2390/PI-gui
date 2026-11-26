//! The model relay: pi on an SSH host uses this computer's models, and its model requests come
//! back over the SSH connection.
//!
//! - A hidden pi here (`pi --mode rpc --no-session …`) loads `relay.ts`, which listens on a Unix
//!   socket in the private connection folder (on Windows: a loopback port). It resolves
//!   credentials with pi's own code (API keys, `$ENV` and `!command` keys, logins and their
//!   refresh, provider headers), so they never leave this computer and behave exactly as in a
//!   local session.
//! - Each host gets a reverse port forward to it on its connection
//!   ([`remote::Ssh::forward_to`]), bound to the host's loopback.
//! - pi on the host runs with `PI_CODING_AGENT_DIR` set to a folder Pistachio manages there
//!   ([`remote::MANAGED_AGENT_DIR`]); its `models.json` lists this computer's models with every
//!   `baseUrl` pointing at the forward and a placeholder key: a token only valid for this
//!   connection to that host (an HMAC with a key that lives as long as this app). The relay
//!   refuses requests without a valid token and only forwards to the endpoints of this
//!   computer's models.
//! - Requests that take a key are relayed as they are, with the real key. Those that need more
//!   (ChatGPT/Codex, GitHub Copilot, Vertex, Bedrock) use pi's own protocol, `pi-messages`: the
//!   host's pi sends the conversation, and the pi here sends that to the provider.
//! - Before pi starts on the host, the relay proves that it is the one answering there: the
//!   forward may be gone, and on a shared host someone else could listen on its port.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{Map, Value, json};

mod process;

/// The relay extension pi loads.
pub const EXTENSION_SOURCE: &str = include_str!("relay.ts");
const SOCKET_ENV: &str = "PISTACHIO_RELAY_SOCKET";
const PORT_FILE_ENV: &str = "PISTACHIO_RELAY_PORT_FILE";
const KEY_ENV: &str = "PISTACHIO_RELAY_KEY";
const CONTROL_KEY_ENV: &str = "PISTACHIO_RELAY_CONTROL_KEY";

/// The API for requests the pi here sends itself.
const MESSAGES_API: &str = "pi-messages";

/// The first pi on a host that gets models over [`MESSAGES_API`].
const MESSAGES_PI: [u32; 2] = [1, 5];

/// APIs whose requests need more than a key (cloud signing or account credentials).
const MORE_THAN_A_KEY: &[&str] = &[
    "openai-codex-responses",
    "bedrock-converse-stream",
    "google-vertex",
];

/// Model fields copied to the host. Not `headers`: their values may be credentials (the relay
/// adds them here).
const MODEL_FIELDS: &[&str] = &[
    "id",
    "name",
    "api",
    "reasoning",
    "thinkingLevelMap",
    "input",
    "inputLimits",
    "cost",
    "promptCache",
    "contextWindow",
    "maxTokens",
    "samplingParams",
    "samplingParamsByThinkingLevel",
    "compat",
];

/// How to start the relay's pi.
#[derive(Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub env: crate::config::ChildEnv,
    /// Where `relay.ts` and its log are written.
    pub dir: PathBuf,
    /// Listen on a loopback port instead of a Unix socket.
    pub tcp: bool,
}

/// Whether the relay listens on a port: where the connection can't forward to a socket.
pub const TCP: bool = !cfg!(unix);

/// Environment variables for pi on a host.
pub type RemoteEnv = Vec<(String, String)>;

/// A host whose pi uses this computer's models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The forwarded port on the host.
    pub port: u16,
    /// What it is forwarded to here.
    pub relay: remote::Endpoint,
    /// The connection the forward lives on.
    pub master_pid: Option<u32>,
    /// Environment for pi on the host.
    pub env: RemoteEnv,
    /// This computer's pi settings when the host's were written.
    pub stamp: Stamp,
    /// The models the host was given.
    pub models: Served,
}

/// The ids of the models a host was given, by provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Served(Arc<HashMap<String, HashSet<String>>>);

impl Served {
    fn new(remote_models: &Value) -> Self {
        let providers = remote_models["providers"].as_object();
        let ids = |config: &Value| {
            let models = config["models"].as_array().into_iter().flatten();
            models
                .filter_map(|model| model["id"].as_str().map(str::to_owned))
                .collect()
        };
        Self(Arc::new(
            providers
                .into_iter()
                .flatten()
                .map(|(provider, config)| (provider.clone(), ids(config)))
                .collect(),
        ))
    }

    /// Whether pi on the host can use a model it lists. It adds the models it knows itself to the
    /// providers it was given, also those this computer does not have; other providers are the
    /// host's own.
    pub fn usable(&self, provider: &str, id: &str) -> bool {
        self.0.get(provider).is_none_or(|ids| ids.contains(id))
    }
}

/// When this computer's `models.json`, `auth.json` and `settings.json` last changed: a host's
/// settings are written again when this differs.
pub type Stamp = [Option<SystemTime>; 3];

pub fn local_stamp() -> Stamp {
    let modified = |name: &str| {
        let path = pi_sessions::agent_dir()?.join(name);
        std::fs::metadata(path).ok()?.modified().ok()
    };
    [
        modified("models.json"),
        modified("auth.json"),
        modified("settings.json"),
    ]
}

/// Points pi on the host behind `ssh` at this computer's models: starts the relay if needed,
/// forwards a port to it (reusing `previous`'s while its connection lasts) and writes the
/// managed settings folder, unless `previous` is still current. Blocks.
pub fn link(
    ssh: &remote::Ssh,
    host: &remote::HostId,
    probe: &remote::Probe,
    previous: Option<&Link>,
    launch: &Launch,
) -> Result<Link, String> {
    let stamp = local_stamp();
    let (relay, models) = process::models(launch)?;
    let master_pid = ssh.master_pid();
    if master_pid.is_none() {
        return Err("the connection to the host is closed".to_owned());
    }
    // Same connection (so its forward is there) to the same relay.
    let previous_port = previous.map_or(0, |link| link.port);
    let kept = previous.filter(|link| link.master_pid == master_pid && link.relay == relay);
    if let Some(link) = kept
        && link.stamp == stamp
    {
        return Ok(link.clone());
    }
    let port = match kept {
        Some(link) => link.port,
        None => ssh
            .forward_to(previous_port, &relay)
            .or_else(|error| {
                if previous_port == 0 {
                    Err(error)
                } else {
                    ssh.forward_to(0, &relay)
                }
            })
            .map_err(|error| format!("the host does not allow port forwarding: {error}"))?,
    };
    if kept.is_none() {
        process::forwarded(ssh, &relay);
    }
    // A forward may take a connection of its own.
    let master_pid = ssh.master_pid();
    tokens::issue(host, master_pid);
    // Before the host knows the token: what it has then works.
    process::allow_tokens()?;
    let remote_models = remote_models(
        &models,
        port,
        &token(host),
        speaks_messages(probe.pi_version.as_deref()),
    );
    if remote_models["providers"]
        .as_object()
        .is_none_or(Map::is_empty)
    {
        return Err("none of this computer's models can be used on the host".to_owned());
    }
    let host_settings = ssh
        .read_file(&format!("{}/settings.json", probe.agent_dir))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let local_settings = pi_sessions::agent_dir()
        .and_then(|dir| std::fs::read(dir.join("settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let settings = remote_settings(
        &probe.agent_dir,
        host_settings.as_ref(),
        local_settings.as_ref(),
        &remote_models,
    );
    let models_text = serde_json::to_vec_pretty(&remote_models).map_err(|e| e.to_string())?;
    let settings_text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    let agent_dir = ssh
        .write_managed_agent(&probe.agent_dir, &settings_text, models_text, &proof(port))
        .map_err(|error| error.to_string())?;
    Ok(Link {
        port,
        relay,
        master_pid,
        env: vec![
            ("PI_CODING_AGENT_DIR".to_owned(), agent_dir),
            (
                "PI_CODING_AGENT_SESSION_DIR".to_owned(),
                probe.sessions_dir.clone(),
            ),
            // pi starts only while the forward is there (see `remote::RELAY_PORT_ENV`).
            (remote::RELAY_PORT_ENV.to_owned(), port.to_string()),
        ],
        stamp,
        models: Served::new(&remote_models),
    })
}

/// Stops the relay (when the app quits).
pub fn stop() {
    process::stop();
}

/// Ends the relay's pi as if it crashed.
#[cfg(all(test, unix))]
pub fn kill_for_tests() {
    process::kill_for_tests();
}

/// The placeholder key pi on `host` sends: only the relay of this app can check it, it is worth
/// nothing anywhere else, and the relay only accepts that of the host's current connection
/// (empty before there is one).
pub fn token(host: &remote::HostId) -> String {
    let Some(nonce) = tokens::nonce(host) else {
        return String::new();
    };
    let mac = hmac_sha256(process::key(), nonce.as_bytes());
    format!("pistachio-{nonce}-{}", hex(&mac))
}

/// What pi's launch on a host checks: the forwarded port, a challenge, and the answer only this
/// app's relay can give to it.
fn proof(port: u16) -> remote::RelayProof {
    let challenge = hex(&random::<16>());
    let answer = hex(&hmac_sha256(
        process::key(),
        format!("prove {challenge}").as_bytes(),
    ));
    remote::RelayProof {
        port,
        challenge,
        answer,
    }
}

/// Whether the pi with this `--version` on a host can be given models over [`MESSAGES_API`].
fn speaks_messages(version: Option<&str>) -> bool {
    let Some(version) = version.and_then(|text| text.split_whitespace().last()) else {
        return false;
    };
    let mut parts = version
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => [major, minor] >= MESSAGES_PI,
        _ => false,
    }
}

/// The `models.json` for a host: this computer's models that the relay can serve, grouped by
/// provider, each with its `baseUrl` on the forwarded `port` and `token` as its key. `messages`:
/// the host's pi speaks [`MESSAGES_API`].
pub fn remote_models(models: &[Value], port: u16, token: &str, messages: bool) -> Value {
    let mut providers = Map::new();
    for model in models {
        let Some(route) = route(model, port, messages) else {
            continue;
        };
        let (provider, base) = (route.provider, route.base);
        let entry = providers.entry(provider.to_owned()).or_insert_with(|| {
            // pi recognises an Anthropic login by its key, and then sends what a login needs.
            let key = if route.oauth && !route.messages {
                format!("sk-ant-oat01-{token}")
            } else {
                token.to_owned()
            };
            json!({ "baseUrl": base, "apiKey": key, "models": [] })
        });
        let mut copy = Map::new();
        for field in MODEL_FIELDS {
            if let Some(value) = model.get(*field) {
                copy.insert((*field).to_owned(), value.clone());
            }
        }
        copy.insert("baseUrl".to_owned(), Value::String(base));
        if route.messages {
            copy.insert("api".to_owned(), MESSAGES_API.into());
            // About the provider's own requests, which the pi here makes.
            copy.remove("compat");
        }
        if let Some(list) = entry["models"].as_array_mut() {
            list.push(Value::Object(copy));
        }
    }
    json!({ "providers": providers })
}

/// How a host's pi reaches a model through the relay.
struct Route<'a> {
    provider: &'a str,
    base: String,
    oauth: bool,
    /// Over [`MESSAGES_API`], not the provider's own.
    messages: bool,
}

/// The relay URL for `model`'s endpoint and its provider, or `None` if the relay can't serve
/// it. The path of a request that is relayed as it is keeps the original host name, which pi also
/// uses to recognise some providers.
fn route(model: &Value, port: u16, messages: bool) -> Option<Route<'_>> {
    let provider = model["provider"].as_str()?;
    let api = model["api"].as_str().unwrap_or_default();
    let oauth = model["pistachioOAuth"].as_bool().unwrap_or(false);
    let valid_id = !provider.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid_id {
        return None;
    }
    let login_ok = !oauth || (provider == "anthropic" && api == "anthropic-messages");
    if MORE_THAN_A_KEY.contains(&api) || !login_ok {
        let served = messages && model["pistachioMessages"].as_bool().unwrap_or(false);
        return served.then(|| Route {
            provider,
            base: format!("http://127.0.0.1:{port}/m/{provider}"),
            oauth,
            messages: true,
        });
    }
    let (scheme, rest) = model["baseUrl"].as_str()?.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    // A URL with a user name or password in it is a credential of its own.
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let path = if path.is_empty() { "/" } else { path };
    Some(Route {
        provider,
        base: format!("http://127.0.0.1:{port}/p/{provider}/{scheme}/{authority}{path}"),
        oauth,
        messages: false,
    })
}

/// The host's `settings.json` for the managed folder: the host's own (read from its agent
/// folder `host_agent_dir`), with this computer's default model and thinking level and
/// without settings that point at the host's models or sessions folder (pi gets that from the
/// environment). Its relative resource paths are made absolute: pi reads them from the folder
/// the settings are in, and that is no longer `host_agent_dir`.
pub fn remote_settings(
    host_agent_dir: &str,
    host: Option<&Value>,
    local: Option<&Value>,
    models: &Value,
) -> Value {
    let mut settings = host.and_then(Value::as_object).cloned().unwrap_or_default();
    absolute_resource_paths(&mut settings, host_agent_dir);
    for key in [
        "sessionDir",
        "enabledModels",
        "defaultProvider",
        "defaultModel",
    ] {
        settings.remove(key);
    }
    let Some(local) = local.and_then(Value::as_object) else {
        return Value::Object(settings);
    };
    let provider = local.get("defaultProvider").and_then(Value::as_str);
    let model = local.get("defaultModel").and_then(Value::as_str);
    if let (Some(provider), Some(model)) = (provider, model) {
        let listed = models["providers"][provider]["models"]
            .as_array()
            .is_some_and(|list| list.iter().any(|entry| entry["id"] == model));
        if listed {
            settings.insert("defaultProvider".to_owned(), provider.into());
            settings.insert("defaultModel".to_owned(), model.into());
        }
    }
    for key in ["defaultThinkingLevel", "enabledModels"] {
        if let Some(value) = local.get(key) {
            settings.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(settings)
}

/// Rewrites the relative paths in pi's resource settings against `base` (see pi's
/// `docs/settings.md`, Resources). Plain entries are files or folders, made absolute. Patterns
/// (`!glob`, `+path`, `-path`, or with `*`/`?`) are matched against paths relative to the
/// settings folder too, which still holds what pi finds through the managed folder's links, so
/// they stay and an absolute copy is added for what a rewritten entry finds. A local package
/// source starts with `.`; npm, git and URL sources stay.
fn absolute_resource_paths(settings: &mut Map<String, Value>, base: &str) {
    let base = base.trim_end_matches('/');
    let join = |path: &str| {
        let path = path.strip_prefix("./").unwrap_or(path);
        if path.is_empty() || path == "." {
            base.to_owned()
        } else {
            format!("{base}/{path}")
        }
    };
    // Absolute, under `~`, or something other than a path (`builtin:mcp`, `npm:…`).
    let relative =
        |path: &str| !path.is_empty() && !path.starts_with(['/', '~']) && !path.contains(':');
    for key in ["extensions", "skills", "prompts", "themes"] {
        let Some(entries) = settings.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries.drain(..) {
            let Some(text) = entry.as_str() else {
                out.push(entry);
                continue;
            };
            let (prefix, path) = match text.chars().next() {
                Some(c @ ('!' | '+' | '-')) => (Some(c), &text[1..]),
                _ => (None, text),
            };
            if !relative(path) {
                out.push(entry);
            } else if prefix.is_none() && !path.contains(['*', '?']) {
                out.push(Value::String(join(path)));
            } else {
                let absolute = format!("{}{}", prefix.map_or_default(String::from), join(path));
                out.push(entry);
                out.push(Value::String(absolute));
            }
        }
        *entries = out;
    }
    let Some(packages) = settings.get_mut("packages").and_then(Value::as_array_mut) else {
        return;
    };
    for package in packages {
        let source = match package {
            Value::Object(object) => object.get_mut("source"),
            other => Some(other),
        };
        if let Some(Value::String(source)) = source
            && source.starts_with('.')
        {
            *source = join(source);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut block = [0_u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Writes the extension into `dir` (only when it changed) and returns its path.
fn install_extension(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("relay.ts");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(EXTENSION_SOURCE) {
        std::fs::write(&path, EXTENSION_SOURCE)?;
    }
    Ok(path)
}

/// Bytes from the system's random source.
fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

/// The nonces in the hosts' tokens: one for each host's current connection.
mod tokens {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

    use super::{hex, random};

    struct Token {
        nonce: String,
        /// The connection it is for.
        master_pid: Option<u32>,
    }

    static TOKENS: LazyLock<Mutex<HashMap<remote::HostId, Token>>> = LazyLock::new(Mutex::default);

    fn tokens() -> MutexGuard<'static, HashMap<remote::HostId, Token>> {
        TOKENS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives the host a token for its connection `master_pid`, unless it has one; that of an
    /// earlier connection is worth nothing any more.
    pub fn issue(host: &remote::HostId, master_pid: Option<u32>) {
        let mut tokens = tokens();
        if tokens
            .get(host)
            .is_none_or(|token| token.master_pid != master_pid)
        {
            let nonce = hex(&random::<8>());
            tokens.insert(host.clone(), Token { nonce, master_pid });
        }
    }

    pub fn nonce(host: &remote::HostId) -> Option<String> {
        tokens().get(host).map(|token| token.nonce.clone())
    }

    pub fn all() -> Vec<String> {
        tokens().values().map(|token| token.nonce.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn tokens_differ_per_host_and_connection() {
        let (host, other) = (remote::HostId("a".into()), remote::HostId("b".into()));
        assert_eq!(token(&host), "");
        tokens::issue(&host, Some(1));
        tokens::issue(&other, Some(1));
        let a = token(&host);
        assert!(a.starts_with("pistachio-") && a.len() == "pistachio-".len() + 16 + 1 + 64);
        assert_ne!(a, token(&other));
        // Sessions that run keep theirs when the host's settings are written again.
        tokens::issue(&host, Some(1));
        assert_eq!(a, token(&host));
        tokens::issue(&host, Some(2));
        assert_ne!(a, token(&host));
        // The relay is told the new one only.
        let nonce = |token: &str| token["pistachio-".len()..][..16].to_owned();
        assert!(tokens::all().contains(&nonce(&token(&host))));
        assert!(!tokens::all().contains(&nonce(&a)));
    }

    #[test]
    fn proofs_are_different_each_time() {
        let (a, b) = (proof(1), proof(1));
        assert_ne!(a.challenge, b.challenge);
        assert_ne!(a.answer, b.answer);
        assert_eq!((a.challenge.len(), a.answer.len()), (32, 64));
    }

    #[test]
    fn only_a_pi_that_speaks_its_protocol_gets_those_models() {
        for version in ["1.5.0", "1.5.2", "v1.6.0", "2.0.0-beta.1", "pi 1.10.0"] {
            assert!(speaks_messages(Some(version)), "{version}");
        }
        for version in ["1.4.9", "0.87.1", "", "pi", "1"] {
            assert!(!speaks_messages(Some(version)), "{version}");
        }
        assert!(!speaks_messages(None));
    }

    #[test]
    fn remote_models_for_more_than_a_key_go_through_the_pi_here() {
        let more = |api: &str, provider: &str, oauth: bool| {
            json!({ "id": "m", "provider": provider, "api": api, "baseUrl": "https://x.example",
                    "reasoning": true, "thinkingLevelMap": { "high": "max" },
                    "compat": { "supportsStore": false },
                    "pistachioOAuth": oauth, "pistachioMessages": true })
        };
        let models = [
            more("openai-codex-responses", "openai-codex", true),
            more("openai-completions", "github-copilot", true),
            more("anthropic-messages", "github-copilot", true),
            more("bedrock-converse-stream", "amazon-bedrock", false),
            more("google-vertex", "google-vertex", false),
            // pi here can't: too old.
            json!({ "id": "old", "provider": "old", "api": "google-vertex",
                    "baseUrl": "https://x.example", "pistachioMessages": false }),
        ];
        let config = remote_models(&models, 4321, "pistachio-t", true);
        let listed = |provider: &str, count: usize| {
            let base = format!("http://127.0.0.1:4321/m/{provider}");
            let model = json!({ "id": "m", "api": "pi-messages", "baseUrl": base,
                                "reasoning": true, "thinkingLevelMap": { "high": "max" } });
            json!({ "baseUrl": base, "apiKey": "pistachio-t", "models": vec![model; count] })
        };
        assert_eq!(
            config,
            json!({ "providers": {
                "openai-codex": listed("openai-codex", 1),
                "github-copilot": listed("github-copilot", 2),
                "amazon-bedrock": listed("amazon-bedrock", 1),
                "google-vertex": listed("google-vertex", 1),
            }})
        );
        // pi on the host can't.
        let config = remote_models(&models, 4321, "pistachio-t", false);
        assert_eq!(config, json!({ "providers": {} }));
    }

    #[test]
    fn remote_models_point_every_provider_at_the_relay() {
        let models = [
            json!({ "id": "qwen", "name": "Qwen", "provider": "spark", "api": "openai-completions",
                    "baseUrl": "http://192.168.1.89:8888/v1", "contextWindow": 262_144,
                    "headers": { "x-key": "secret" }, "compat": { "supportsStore": false } }),
            json!({ "id": "opus", "provider": "anthropic", "api": "anthropic-messages",
                    "baseUrl": "https://api.anthropic.com", "pistachioOAuth": true }),
            json!({ "id": "mistral", "provider": "mistral", "api": "mistral-conversations",
                    "baseUrl": "https://api.mistral.ai" }),
            // These take more than a key, and the pi here is too old to send them itself.
            json!({ "id": "gpt", "provider": "openai-codex", "api": "openai-codex-responses",
                    "baseUrl": "https://chatgpt.com/backend-api", "pistachioOAuth": true }),
            json!({ "id": "gpt-4.1", "provider": "github-copilot", "api": "openai-completions",
                    "baseUrl": "https://api.individual.githubcopilot.com", "pistachioOAuth": true }),
            json!({ "id": "claude", "provider": "amazon-bedrock", "api": "bedrock-converse-stream",
                    "baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com" }),
            json!({ "id": "x", "provider": "creds", "api": "openai-completions",
                    "baseUrl": "https://user:pass@example.com/v1" }),
        ];
        let config = remote_models(&models, 4321, "pistachio-t", true);
        assert_eq!(
            config,
            json!({ "providers": {
                "spark": {
                    "baseUrl": "http://127.0.0.1:4321/p/spark/http/192.168.1.89:8888/v1",
                    "apiKey": "pistachio-t",
                    "models": [{
                        "id": "qwen", "name": "Qwen", "api": "openai-completions",
                        "contextWindow": 262_144, "compat": { "supportsStore": false },
                        "baseUrl": "http://127.0.0.1:4321/p/spark/http/192.168.1.89:8888/v1",
                    }],
                },
                "anthropic": {
                    "baseUrl": "http://127.0.0.1:4321/p/anthropic/https/api.anthropic.com/",
                    "apiKey": "sk-ant-oat01-pistachio-t",
                    "models": [{
                        "id": "opus", "api": "anthropic-messages",
                        "baseUrl": "http://127.0.0.1:4321/p/anthropic/https/api.anthropic.com/",
                    }],
                },
                "mistral": {
                    "baseUrl": "http://127.0.0.1:4321/p/mistral/https/api.mistral.ai/",
                    "apiKey": "pistachio-t",
                    "models": [{
                        "id": "mistral", "api": "mistral-conversations",
                        "baseUrl": "http://127.0.0.1:4321/p/mistral/https/api.mistral.ai/",
                    }],
                },
            }})
        );
    }

    #[test]
    fn models_only_the_hosts_pi_knows_are_not_usable() {
        let served = Served::new(&json!({ "providers": {
            "openai-codex": { "models": [{ "id": "gpt-6" }] },
            "spark": { "models": [{ "id": "qwen" }] },
        }}));
        assert!(served.usable("openai-codex", "gpt-6"));
        assert!(!served.usable("openai-codex", "gpt-7"));
        assert!(!served.usable("spark", "gpt-6"));
        // From an extension on the host, say.
        assert!(served.usable("hosts-own", "any"));
    }

    #[test]
    fn remote_settings_take_this_computers_defaults() {
        let models = json!({ "providers": { "spark": { "models": [{ "id": "qwen" }] } } });
        let host = json!({ "theme": "dark", "sessionDir": "/srv/s", "defaultProvider": "openai",
                           "defaultModel": "gpt", "enabledModels": ["openai/*"] });
        let local = json!({ "defaultProvider": "spark", "defaultModel": "qwen",
                            "defaultThinkingLevel": "high", "theme": "light" });
        assert_eq!(
            remote_settings("/home/me/.pi/agent", Some(&host), Some(&local), &models),
            json!({ "theme": "dark", "defaultProvider": "spark", "defaultModel": "qwen",
                    "defaultThinkingLevel": "high" })
        );
        // A local default the host can't use is left to pi.
        let local = json!({ "defaultProvider": "openai-codex", "defaultModel": "gpt" });
        assert_eq!(
            remote_settings("/home/me/.pi/agent", None, Some(&local), &models),
            json!({})
        );
    }

    #[test]
    fn remote_settings_make_resource_paths_absolute() {
        let host = json!({
            "extensions": ["./my-ext", "../shared/ext.ts", "/opt/ext", "~/ext", "builtin:mcp",
                           "-builtin:codemode", "!old/*", "+extensions/keep.ts"],
            "skills": ["skills-extra", "*.md"],
            "themes": [],
            "packages": ["npm:@example/tools@1.0.0", "./pkg", "git:github.com/x/y",
                         { "source": "../local-pkg", "extensions": ["./a.ts"] }],
            "theme": "./not-a-resource",
        });
        assert_eq!(
            remote_settings("/home/me/.pi/agent/", Some(&host), None, &json!({})),
            json!({
                "extensions": ["/home/me/.pi/agent/my-ext", "/home/me/.pi/agent/../shared/ext.ts",
                               "/opt/ext", "~/ext", "builtin:mcp", "-builtin:codemode",
                               "!old/*", "!/home/me/.pi/agent/old/*",
                               "+extensions/keep.ts", "+/home/me/.pi/agent/extensions/keep.ts"],
                "skills": ["/home/me/.pi/agent/skills-extra", "*.md", "/home/me/.pi/agent/*.md"],
                "themes": [],
                "packages": ["npm:@example/tools@1.0.0", "/home/me/.pi/agent/pkg",
                             "git:github.com/x/y",
                             { "source": "/home/me/.pi/agent/../local-pkg", "extensions": ["./a.ts"] }],
                "theme": "./not-a-resource",
            })
        );
    }
}
