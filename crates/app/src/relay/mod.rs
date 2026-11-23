//! The model relay: pi on an SSH host uses this computer's models, and its model requests come
//! back over the SSH connection.
//!
//! - A hidden pi here (`pi --mode rpc --no-session …`) loads `relay.ts`, which listens on a Unix
//!   socket in the private connection folder. It resolves credentials with pi's own code
//!   (API keys, `$ENV` and `!command` keys, logins and their refresh, provider headers), so
//!   they never leave this computer and behave exactly as in a local session.
//! - Each host gets a reverse port forward to that socket on its shared SSH connection
//!   ([`remote::Ssh::forward_to_socket`]), bound to the host's loopback.
//! - pi on the host runs with `PI_CODING_AGENT_DIR` set to a folder Pistachio manages there
//!   ([`remote::MANAGED_AGENT_DIR`]); its `models.json` lists this computer's models with every
//!   `baseUrl` pointing at the forward and a placeholder key: a token only valid for that host
//!   (an HMAC of the host with a key that lives as long as this app). The relay refuses
//!   requests without a valid token and only forwards to the endpoints of this computer's
//!   models.
//!
//! Logins whose requests need more than a key swap (ChatGPT/Codex, GitHub Copilot, Gemini
//! CLI) and cloud credentials (Vertex, Bedrock) are not relayed yet; see #25.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value, json};

/// The relay extension pi loads.
pub const EXTENSION_SOURCE: &str = include_str!("relay.ts");
const SOCKET_ENV: &str = "PISTACHIO_RELAY_SOCKET";
const KEY_ENV: &str = "PISTACHIO_RELAY_KEY";

/// APIs whose requests need more than a key (cloud signing or account credentials).
const UNSUPPORTED_APIS: &[&str] = &[
    "openai-codex-responses",
    "bedrock-converse-stream",
    "google-vertex",
    "google-gemini-cli",
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
}

/// Environment variables for pi on a host.
pub type RemoteEnv = Vec<(String, String)>;

/// A host whose pi uses this computer's models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The forwarded port on the host.
    pub port: u16,
    /// The shared connection the forward lives on.
    pub master_pid: Option<u32>,
    /// Environment for pi on the host.
    pub env: RemoteEnv,
    /// This computer's pi settings when the host's were written.
    pub stamp: Stamp,
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
    let models = imp::models(launch)?;
    let master_pid = ssh.master_pid();
    if master_pid.is_none() {
        return Err("the connection to the host is closed".to_owned());
    }
    // Same connection (so its forward is there) and same settings here: nothing to write.
    if let Some(link) = previous
        && link.master_pid == master_pid
        && link.stamp == stamp
    {
        return Ok(link.clone());
    }
    let port = match previous {
        Some(link) if link.master_pid == master_pid => link.port,
        _ => {
            let socket = imp::socket_path();
            let wanted = previous.map_or(0, |link| link.port);
            ssh.forward_to_socket(wanted, &socket)
                .or_else(|error| {
                    if wanted == 0 {
                        Err(error)
                    } else {
                        ssh.forward_to_socket(0, &socket)
                    }
                })
                .map_err(|error| format!("the host does not allow port forwarding: {error}"))?
        }
    };
    let remote_models = remote_models(&models, port, &token(host));
    if remote_models["providers"]
        .as_object()
        .is_none_or(Map::is_empty)
    {
        return Err("none of this computer's models can be used on a host yet".to_owned());
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
        host_settings.as_ref(),
        local_settings.as_ref(),
        &remote_models,
    );
    let models_text = serde_json::to_vec_pretty(&remote_models).map_err(|e| e.to_string())?;
    let settings_text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    let agent_dir = ssh
        .write_managed_agent(&probe.agent_dir, &settings_text, models_text)
        .map_err(|error| error.to_string())?;
    Ok(Link {
        port,
        master_pid,
        env: vec![
            ("PI_CODING_AGENT_DIR".to_owned(), agent_dir),
            (
                "PI_CODING_AGENT_SESSION_DIR".to_owned(),
                probe.sessions_dir.clone(),
            ),
        ],
        stamp,
    })
}

/// Stops the relay (when the app quits).
pub fn stop() {
    imp::stop();
}

/// The placeholder key pi on `host` sends: only the relay of this app can check it, and it
/// is worth nothing anywhere else.
pub fn token(host: &remote::HostId) -> String {
    use sha2::{Digest, Sha256};
    let nonce = hex(&Sha256::digest(host.0.as_bytes())[..8]);
    let mac = hmac_sha256(imp::key(), nonce.as_bytes());
    format!("pistachio-{nonce}-{}", hex(&mac))
}

/// The `models.json` for a host: this computer's models that the relay can serve, grouped by
/// provider, each with its `baseUrl` on the forwarded `port` and `token` as its key.
pub fn remote_models(models: &[Value], port: u16, token: &str) -> Value {
    let mut providers = Map::new();
    for model in models {
        let Some((provider, base)) = relayed_base(model, port) else {
            continue;
        };
        let oauth = model["pistachioOAuth"].as_bool().unwrap_or(false);
        let entry = providers.entry(provider.to_owned()).or_insert_with(|| {
            // pi recognises an Anthropic login by its key, and then sends what a login needs.
            let key = if oauth {
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
        if let Some(list) = entry["models"].as_array_mut() {
            list.push(Value::Object(copy));
        }
    }
    json!({ "providers": providers })
}

/// The relay URL for `model`'s endpoint and its provider, or `None` if the relay can't serve
/// it. The path keeps the original host name, which pi also uses to recognise some providers.
fn relayed_base(model: &Value, port: u16) -> Option<(&str, String)> {
    let provider = model["provider"].as_str()?;
    let api = model["api"].as_str().unwrap_or_default();
    let oauth = model["pistachioOAuth"].as_bool().unwrap_or(false);
    let valid_id = !provider.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let login_ok = !oauth || (provider == "anthropic" && api == "anthropic-messages");
    if !valid_id || UNSUPPORTED_APIS.contains(&api) || !login_ok {
        return None;
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
    Some((
        provider,
        format!("http://127.0.0.1:{port}/p/{provider}/{scheme}/{authority}{path}"),
    ))
}

/// The host's `settings.json` for the managed folder: the host's own, with this computer's
/// default model and thinking level and without settings that point at the host's models or
/// sessions folder (pi gets that from the environment).
pub fn remote_settings(host: Option<&Value>, local: Option<&Value>, models: &Value) -> Value {
    let mut settings = host.and_then(Value::as_object).cloned().unwrap_or_default();
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

#[cfg(unix)]
mod imp {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::{Duration, Instant};

    use serde_json::Value;

    use super::{KEY_ENV, Launch, SOCKET_ENV, hex, install_extension};

    const START_TIMEOUT: Duration = Duration::from_secs(30);

    struct Running {
        child: Child,
        /// pi exits when its input closes, so the relay ends with the app even if it crashes.
        _stdin: Option<ChildStdin>,
        socket: PathBuf,
    }

    static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();

    pub fn key() -> &'static [u8; 32] {
        KEY.get_or_init(|| {
            let mut key = [0_u8; 32];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut random| random.read_exact(&mut key))
                .expect("/dev/urandom");
            key
        })
    }

    /// In the private folder of the shared SSH connections.
    pub fn socket_path() -> PathBuf {
        remote::Ssh::default_control_dir().join(format!("relay-{}.sock", std::process::id()))
    }

    pub fn models(launch: &Launch) -> Result<Vec<Value>, String> {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(relay) = running.as_mut()
            && matches!(relay.child.try_wait(), Ok(None))
            && let Ok(models) = fetch_models(&relay.socket)
        {
            return Ok(models);
        }
        if let Some(mut old) = running.take() {
            let _ = old.child.kill();
            let _ = old.child.wait();
        }
        let (relay, models) = start(launch)?;
        *running = Some(relay);
        Ok(models)
    }

    fn start(launch: &Launch) -> Result<(Running, Vec<Value>), String> {
        let socket = socket_path();
        let _ = std::fs::remove_file(&socket);
        let fail =
            |error: std::io::Error| format!("could not start pi for the model relay: {error}");
        let extension = install_extension(&launch.dir).map_err(fail)?;
        let log_path = launch.dir.join("relay.log");
        let log = std::fs::File::create(&log_path).map_err(fail)?;
        let mut child = Command::new(&launch.program)
            .args([
                "--mode",
                "rpc",
                "--no-session",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-tools",
                "--extension",
            ])
            .arg(&extension)
            .current_dir(&launch.dir)
            .envs(launch.env.vars().iter().map(|(name, value)| (name, value)))
            .env(SOCKET_ENV, &socket)
            .env(KEY_ENV, hex(key()))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .map_err(fail)?;
        let stdin = child.stdin.take();
        let started = Instant::now();
        loop {
            if let Ok(models) = fetch_models(&socket) {
                log::info!(
                    "model relay started (pid {}) in {:?}",
                    child.id(),
                    started.elapsed()
                );
                let relay = Running {
                    child,
                    _stdin: stdin,
                    socket,
                };
                return Ok((relay, models));
            }
            let exited = child.try_wait().ok().flatten();
            if exited.is_some() || started.elapsed() > START_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(5).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                return Err(format!(
                    "pi for the model relay did not start{}",
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", tail.join("\n"))
                    }
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// This computer's available models, from the relay (which reloads pi's settings first).
    fn fetch_models(socket: &std::path::Path) -> Result<Vec<Value>, String> {
        let fail = |error: std::io::Error| error.to_string();
        let mut stream = UnixStream::connect(socket).map_err(fail)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .map_err(fail)?;
        write!(
            stream,
            "GET /_pistachio/models HTTP/1.0\r\nHost: relay\r\nx-pistachio-relay-key: {}\r\n\r\n",
            hex(key())
        )
        .map_err(fail)?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).map_err(fail)?;
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or("no response from the model relay")?;
        let status = String::from_utf8_lossy(&response[..split]);
        if !status.starts_with("HTTP/1.1 200") && !status.starts_with("HTTP/1.0 200") {
            return Err(format!(
                "the model relay answered {}",
                status.lines().next().unwrap_or_default()
            ));
        }
        let body: Value =
            serde_json::from_slice(&response[split + 4..]).map_err(|error| error.to_string())?;
        Ok(body["models"].as_array().cloned().unwrap_or_default())
    }

    pub fn stop() {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mut relay) = running.take() {
            let _ = relay.child.kill();
            let _ = relay.child.wait();
            let _ = std::fs::remove_file(&relay.socket);
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::Launch;

    pub fn key() -> &'static [u8; 32] {
        &[0; 32]
    }

    pub fn socket_path() -> PathBuf {
        PathBuf::new()
    }

    pub fn models(_launch: &Launch) -> Result<Vec<Value>, String> {
        let _ = super::install_extension;
        Err("using this computer's models on a host is not supported on this system yet".into())
    }

    pub fn stop() {}
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
    fn tokens_differ_per_host() {
        let a = token(&remote::HostId("a".into()));
        assert_eq!(a, token(&remote::HostId("a".into())));
        assert_ne!(a, token(&remote::HostId("b".into())));
        assert!(a.starts_with("pistachio-") && a.len() == "pistachio-".len() + 16 + 1 + 64);
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
            // Not relayed yet (#25).
            json!({ "id": "gpt", "provider": "openai-codex", "api": "openai-codex-responses",
                    "baseUrl": "https://chatgpt.com/backend-api", "pistachioOAuth": true }),
            json!({ "id": "gpt-4.1", "provider": "github-copilot", "api": "openai-completions",
                    "baseUrl": "https://api.individual.githubcopilot.com", "pistachioOAuth": true }),
            json!({ "id": "claude", "provider": "amazon-bedrock", "api": "bedrock-converse-stream",
                    "baseUrl": "https://bedrock-runtime.us-east-1.amazonaws.com" }),
            json!({ "id": "x", "provider": "creds", "api": "openai-completions",
                    "baseUrl": "https://user:pass@example.com/v1" }),
        ];
        let config = remote_models(&models, 4321, "pistachio-t");
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
    fn remote_settings_take_this_computers_defaults() {
        let models = json!({ "providers": { "spark": { "models": [{ "id": "qwen" }] } } });
        let host = json!({ "theme": "dark", "sessionDir": "/srv/s", "defaultProvider": "openai",
                           "defaultModel": "gpt", "enabledModels": ["openai/*"] });
        let local = json!({ "defaultProvider": "spark", "defaultModel": "qwen",
                            "defaultThinkingLevel": "high", "theme": "light" });
        assert_eq!(
            remote_settings(Some(&host), Some(&local), &models),
            json!({ "theme": "dark", "defaultProvider": "spark", "defaultModel": "qwen",
                    "defaultThinkingLevel": "high" })
        );
        // A local default the host can't use is left to pi.
        let local = json!({ "defaultProvider": "openai-codex", "defaultModel": "gpt" });
        assert_eq!(remote_settings(None, Some(&local), &models), json!({}));
    }
}
