//! Finding out what a model server is and offers, from its address: which URLs to ask, and what
//! the answers of Ollama, LM Studio, vLLM, llama.cpp, `OpenRouter` and other servers say about
//! their models. Sends nothing itself.

use serde_json::{Value, json};

use crate::ConfigError;

/// A server program for models, and the port it listens on unless told otherwise. It may run on
/// this device or on another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownServer {
    pub label: &'static str,
    pub port: u16,
    /// Where its API is.
    path: &'static str,
    /// The program on that port; `Other` when several listen there by default.
    pub kind: Kind,
    /// How to get it to answer, for the ones many people use: these are also shown when they don't.
    pub help: Option<Help>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    /// How to start it.
    pub start: &'static str,
    /// How to have it answer other devices too: by default some only answer their own.
    pub share: &'static str,
}

pub const KNOWN_SERVERS: &[KnownServer] = &[
    KnownServer {
        label: "Ollama",
        port: 11434,
        path: "/v1",
        kind: Kind::Ollama,
        help: Some(Help {
            start: "Open Ollama, or run “ollama serve”.",
            share: "Ollama only answers its own device unless it runs with OLLAMA_HOST=0.0.0.0.",
        }),
    },
    KnownServer {
        label: "LM Studio",
        port: 1234,
        path: "/v1",
        kind: Kind::LmStudio,
        help: Some(Help {
            start: "In LM Studio, open Developer and turn the server on.",
            share: "In LM Studio's Developer settings, turn on “Serve on Local Network”.",
        }),
    },
    KnownServer {
        label: "llama.cpp or MLX",
        port: 8080,
        path: "/v1",
        kind: Kind::Other,
        help: Some(Help {
            start: "Run “llama-server -m <model.gguf>” or “mlx_lm.server --model <model>”.",
            share: "Start it with “--host 0.0.0.0”.",
        }),
    },
    KnownServer {
        label: "vLLM",
        port: 8000,
        path: "/v1",
        kind: Kind::Other,
        help: Some(Help {
            start: "Run “vllm serve <model>”.",
            share: "It answers other devices by default.",
        }),
    },
    KnownServer {
        label: "Jan",
        port: 1337,
        path: "/v1",
        kind: Kind::Jan,
        help: Some(Help {
            start: "In Jan, open Settings → Local API Server and start the server.",
            share: "In its Configuration, set “Server Host” to 0.0.0.0.",
        }),
    },
    KnownServer {
        label: "Docker Model Runner",
        port: 12434,
        path: "/engines/v1",
        kind: Kind::Docker,
        help: Some(Help {
            start: "Run “docker desktop enable model-runner --tcp 12434”.",
            share: "Docker Desktop only answers its own device.",
        }),
    },
    KnownServer {
        label: "SGLang",
        port: 30000,
        path: "/v1",
        kind: Kind::SgLang,
        help: None,
    },
    KnownServer {
        label: "Lemonade",
        port: 13305,
        path: "/v1",
        kind: Kind::Lemonade,
        help: None,
    },
    KnownServer {
        label: "KoboldCpp",
        port: 5001,
        path: "/v1",
        kind: Kind::KoboldCpp,
        help: None,
    },
];

impl KnownServer {
    /// Where it answers on `host` (a name or an IP address).
    pub fn base_url(&self, host: &str) -> String {
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        format!("http://{host}:{}{}", self.port, self.path)
    }
}

/// The program that listens on the port of `base_url`, on one's own network: for servers that do
/// not say what they are. `Other` when that can't be told.
pub fn kind_on_port(base_url: &str) -> Kind {
    if !is_private_url(base_url) {
        return Kind::Other;
    }
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    let port = authority
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok());
    KNOWN_SERVERS
        .iter()
        .find(|server| Some(server.port) == port)
        .map_or(Kind::Other, |server| server.kind)
}

/// The name of this device in addresses.
pub const THIS_DEVICE: &str = "localhost";

/// Where to look for a server, from what was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Only a device on one's own network: look on the usual ports of [`KNOWN_SERVERS`].
    Device(String),
    /// A server's own address: its base URLs, the likeliest first (see [`base_urls`]).
    Address(Vec<String>),
}

/// What an address means: a device to look for servers on, when it has no scheme, port or path
/// and is on one's own network, else a server's address.
pub fn lookup(address: &str) -> Result<Lookup, ConfigError> {
    let urls = base_urls(address)?;
    let address = address.trim().trim_end_matches('/');
    let bare = !address.contains("://") && !address.contains('/');
    let host = host(address);
    let has_port = match address.strip_prefix('[') {
        Some(v6) => v6.split_once(']').is_some_and(|(_, rest)| !rest.is_empty()),
        None => address.contains(':'),
    };
    Ok(if bare && !has_port && is_private(host) {
        Lookup::Device(host.to_owned())
    } else {
        Lookup::Address(urls)
    })
}

/// The program behind a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ollama,
    LmStudio,
    Vllm,
    SgLang,
    LlamaCpp,
    Jan,
    Docker,
    Lemonade,
    KoboldCpp,
    OpenRouter,
    Other,
}

impl Kind {
    /// What to call it, when it is known.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Ollama => Some("Ollama"),
            Self::LmStudio => Some("LM Studio"),
            Self::Vllm => Some("vLLM"),
            Self::SgLang => Some("SGLang"),
            Self::LlamaCpp => Some("llama.cpp"),
            Self::Jan => Some("Jan"),
            Self::Docker => Some("Docker Model Runner"),
            Self::Lemonade => Some("Lemonade"),
            Self::KoboldCpp => Some("KoboldCpp"),
            Self::OpenRouter => Some("OpenRouter"),
            Self::Other => None,
        }
    }

    /// Whether it runs on one's own computers, where there often is no key. pi needs one all the
    /// same, so a dummy is written for it.
    pub fn is_local(self) -> bool {
        !matches!(self, Self::OpenRouter | Self::Other)
    }
}

/// A model a server offers. `None` is what it does not say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfferedModel {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub reasoning: Option<bool>,
    pub images: Option<bool>,
    /// Not for conversations: embeddings, speech and so on.
    pub not_chat: bool,
}

/// What a server's model list says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub kind: Kind,
    /// The API the list looks like it belongs to.
    pub api: &'static str,
    pub models: Vec<OfferedModel>,
}

/// One way to ask for the list of models: with which URL, with which headers, and what the
/// provider's `baseUrl` is when it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub url: String,
    pub base_url: String,
    /// The API the headers are for.
    pub api: &'static str,
}

/// The headers that carry `key` to servers of `api`.
pub fn headers(api: &str, key: Option<&str>) -> Vec<(&'static str, String)> {
    let mut headers = Vec::new();
    match api {
        "anthropic-messages" => {
            headers.push(("anthropic-version", "2023-06-01".to_owned()));
            if let Some(key) = key {
                headers.push(("x-api-key", key.to_owned()));
            }
        }
        "google-generative-ai" => {
            if let Some(key) = key {
                headers.push(("x-goog-api-key", key.to_owned()));
            }
        }
        _ => {
            if let Some(key) = key {
                headers.push(("Authorization", format!("Bearer {key}")));
            }
        }
    }
    headers
}

/// The base URL an address means: `host`, `host:port`, or any URL of the server. Without a scheme
/// it is `http` for an address on one's own network, else `https`.
pub fn base_url(address: &str) -> Result<String, ConfigError> {
    let invalid = || {
        ConfigError::Invalid(
            "Enter the server's address, like 192.168.1.20:8000 or https://example.com/v1.".into(),
        )
    };
    let address = address.trim();
    if address.is_empty() || address.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    let (scheme, rest) = match address.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(invalid());
            }
            (Some(scheme), rest)
        }
        None => (None, address),
    };
    // Of a URL copied from somewhere, what comes after the API's root.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut rest = rest.trim_end_matches('/');
    for end in [
        "/chat/completions",
        "/completions",
        "/responses",
        "/messages",
        "/models",
    ] {
        if let Some(root) = rest.strip_suffix(end) {
            rest = root.trim_end_matches('/');
            break;
        }
    }
    let host = host(rest);
    if host.is_empty() {
        return Err(invalid());
    }
    let scheme = scheme.unwrap_or_else(|| if is_private(host) { "http" } else { "https" }.into());
    Ok(format!("{scheme}://{rest}"))
}

/// The base URLs an address may mean, the likeliest first: without a scheme, also the other one.
pub fn base_urls(address: &str) -> Result<Vec<String>, ConfigError> {
    let url = base_url(address)?;
    let mut urls = vec![url.clone()];
    if !address.contains("://") {
        urls.push(match url.split_once("://") {
            Some(("http", rest)) => format!("https://{rest}"),
            Some((_, rest)) => format!("http://{rest}"),
            None => return Ok(urls),
        });
    }
    Ok(urls)
}

/// The host in `host[:port][/path]`, without the port.
fn host(rest: &str) -> &str {
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6);
    }
    authority.split(':').next().unwrap_or(authority)
}

/// Whether a URL is of this computer or one's own network.
pub fn is_private_url(url: &str) -> bool {
    is_private(host_of(url))
}

/// Whether a host is on this computer or one's own network. A name without dots is.
fn is_private(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(ip) => {
                let [a, b, ..] = ip.octets();
                ip.is_loopback() || ip.is_private() || ip.is_link_local()
                    // Carrier-grade NAT, which Tailscale uses.
                    || (a == 100 && (64..128).contains(&b))
                    || ip.is_unspecified()
            }
            std::net::IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        };
    }
    !host.contains('.')
        || [".local", ".lan", ".internal", ".home.arpa", ".ts.net"]
            .iter()
            .any(|end| host.ends_with(end))
}

/// Where a server runs, as far as its address tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    ThisDevice,
    /// Another computer on one's own network.
    OtherDevice,
    /// A service on the internet.
    Online,
}

pub fn location(base_url: &str) -> Location {
    let host = host_of(base_url).to_ascii_lowercase();
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified());
    if loopback {
        Location::ThisDevice
    } else if is_private(&host) {
        Location::OtherDevice
    } else {
        Location::Online
    }
}

/// The host of a URL.
pub fn host_of(url: &str) -> &str {
    host(url.split_once("://").map_or(url, |(_, rest)| rest))
}

/// The server's address without scheme and API path, for display: `192.168.1.20:8000`.
pub fn short_address(base_url: &str) -> &str {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .trim_end_matches('/');
    rest.strip_suffix("/v1").unwrap_or(rest)
}

/// The server's root: the base URL without `/v1`, where Ollama, LM Studio and llama.cpp answer
/// on their own APIs.
fn root(base_url: &str) -> &str {
    let base = base_url.trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base)
}

/// What to try, in order, to find the API at `base_url`.
pub fn attempts(base_url: &str) -> Vec<Attempt> {
    let base = base_url.trim_end_matches('/');
    let root = root(base);
    // pi's OpenAI base URLs end in the version, Anthropic's do not and Gemini's do.
    let openai = |base: String| Attempt {
        url: format!("{base}/models"),
        base_url: base,
        api: "openai-completions",
    };
    let has_path = base
        .split_once("://")
        .is_some_and(|(_, rest)| rest.contains('/'));
    let mut attempts = Vec::new();
    if base.ends_with("/v1") || !has_path {
        attempts.push(openai(format!("{root}/v1")));
        if !has_path {
            attempts.push(openai(root.to_owned()));
        }
    } else {
        attempts.push(openai(base.to_owned()));
        attempts.push(openai(format!("{base}/v1")));
    }
    attempts.push(Attempt {
        url: format!("{root}/v1/models"),
        base_url: root.to_owned(),
        api: "anthropic-messages",
    });
    let gemini = if base.ends_with("/v1beta") {
        base.to_owned()
    } else {
        format!("{root}/v1beta")
    };
    attempts.push(Attempt {
        url: format!("{gemini}/models"),
        base_url: gemini,
        api: "google-generative-ai",
    });
    attempts
}

/// Reads a model list: `OpenAI`'s (`{"data": […]}`) with what vLLM, llama.cpp, LM Studio, `OpenRouter`
/// and others add to it, Ollama's and Gemini's (`{"models": […]}`), Anthropic's, or a bare list.
pub fn parse_offer(body: &str) -> Result<Offer, ConfigError> {
    let value: Value = serde_json::from_str(body).map_err(|_| {
        let start: String = body.trim().chars().take(160).collect();
        ConfigError::Invalid(if start.is_empty() {
            "The server sent an empty reply.".to_owned()
        } else {
            format!("The server did not send a model list: {start}")
        })
    })?;
    let list = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or_else(|| {
            let message = value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .or_else(|| value.get("message"))
                .or_else(|| value.get("detail"))
                .and_then(Value::as_str)
                .map_or_else(
                    || "The reply has no model list.".to_owned(),
                    |error| format!("The server answered: {error}"),
                );
            ConfigError::Invalid(message)
        })?;
    let gemini = list
        .iter()
        .any(|model| model.get("supportedGenerationMethods").is_some());
    let anthropic = !gemini
        && list
            .iter()
            .any(|model| text(model, &["type"]) == Some("model"))
        && value.get("has_more").is_some();
    let kind = if anthropic || gemini {
        Kind::Other
    } else {
        list.iter()
            .map(kind_of)
            .find(|kind| *kind != Kind::Other)
            .unwrap_or(Kind::Other)
    };
    let mut models: Vec<OfferedModel> = list.iter().filter_map(offered).collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(Offer {
        kind,
        api: if gemini {
            "google-generative-ai"
        } else if anthropic {
            "anthropic-messages"
        } else {
            "openai-completions"
        },
        models,
    })
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(key)?.as_str())
        .filter(|text| !text.trim().is_empty())
}

fn number(value: &Value, pointers: &[&str]) -> Option<u64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer)?.as_u64())
        .filter(|number| *number > 0)
}

fn lists(value: &Value, pointer: &str, entry: &str) -> Option<bool> {
    let list = value.pointer(pointer)?.as_array()?;
    Some(list.iter().any(|item| item.as_str() == Some(entry)))
}

fn kind_of(model: &Value) -> Kind {
    match text(model, &["owned_by"]) {
        Some("vllm") => return Kind::Vllm,
        Some("sglang") => return Kind::SgLang,
        Some("llamacpp") => return Kind::LlamaCpp,
        Some("docker") => return Kind::Docker,
        Some("lemonade") => return Kind::Lemonade,
        Some("koboldcpp") => return Kind::KoboldCpp,
        Some("library") => return Kind::Ollama,
        Some("organization_owner") => return Kind::LmStudio,
        _ => {}
    }
    if model.pointer("/details/quantization_level").is_some() {
        Kind::Ollama
    } else if model.get("loaded_context_length").is_some()
        || model.get("max_context_length").is_some() && model.get("publisher").is_some()
    {
        Kind::LmStudio
    } else if model.pointer("/architecture/input_modalities").is_some()
        && model.get("top_provider").is_some()
    {
        Kind::OpenRouter
    } else {
        Kind::Other
    }
}

fn offered(model: &Value) -> Option<OfferedModel> {
    let gemini = model.get("supportedGenerationMethods").is_some();
    let mut id = text(model, &["id", "name", "model"])?;
    let mut name = text(model, &["display_name", "displayName", "name"]);
    if gemini {
        id = id.strip_prefix("models/").unwrap_or(id);
    }
    if let Some(shown) = name
        && shown.trim_start_matches("models/") == id
    {
        name = None;
    }
    let images = lists(model, "/architecture/input_modalities", "image")
        .or_else(|| lists(model, "/input_modalities", "image"))
        .or_else(|| model.pointer("/capabilities/vision")?.as_bool())
        .or_else(|| lists(model, "/capabilities", "vision"))
        .or_else(|| (text(model, &["type"]) == Some("vlm")).then_some(true))
        .or_else(|| has_word(id, IMAGE_WORDS).then_some(true));
    let reasoning = lists(model, "/supported_parameters", "reasoning")
        .or_else(|| model.pointer("/capabilities/reasoning")?.as_bool())
        .or_else(|| lists(model, "/capabilities", "thinking"))
        .or_else(|| model.get("thinking")?.as_bool())
        .or_else(|| has_word(id, REASONING_WORDS).then_some(true));
    let mut found = OfferedModel {
        id: id.to_owned(),
        name: name.map(str::to_owned),
        context_window: number(
            model,
            &[
                // What it runs with comes before what the model could do.
                "/meta/n_ctx",
                "/loaded_context_length",
                "/max_model_len",
                "/top_provider/context_length",
                "/context_length",
                "/context_window",
                "/inputTokenLimit",
                "/max_input_tokens",
                "/max_context_length",
                "/meta/n_ctx_train",
            ],
        ),
        max_tokens: number(
            model,
            &[
                "/top_provider/max_completion_tokens",
                "/max_completion_tokens",
                "/outputTokenLimit",
                "/max_output_tokens",
            ],
        ),
        reasoning,
        images,
        not_chat: is_not_chat(model, id),
    };
    // Some servers give the whole context as the limit of an answer: nothing would be left.
    if found.max_tokens >= found.context_window && found.context_window.is_some() {
        found.max_tokens = None;
    }
    Some(found)
}

fn is_not_chat(model: &Value, id: &str) -> bool {
    if let Some(kind) = text(model, &["type"]) {
        match kind {
            "embeddings" | "embedding" | "image" | "audio" | "rerank" | "moderation"
            | "transcribe" => return true,
            "llm" | "vlm" | "chat" | "language" => return false,
            _ => {}
        }
    }
    if let Some(methods) = model
        .get("supportedGenerationMethods")
        .and_then(Value::as_array)
    {
        return !methods
            .iter()
            .any(|method| method.as_str() == Some("generateContent"));
    }
    if let Some(chat) = model
        .pointer("/capabilities/completion_chat")
        .and_then(Value::as_bool)
    {
        return !chat;
    }
    if let Some(output) = model
        .pointer("/architecture/output_modalities")
        .and_then(Value::as_array)
    {
        return !output.iter().any(|kind| kind.as_str() == Some("text"));
    }
    has_word(
        id,
        &[
            "embed",
            "embedding",
            "embeddings",
            "reranker",
            "rerank",
            "bge",
            "whisper",
            "tts",
            "moderation",
        ],
    )
}

/// Words in a model's ID that say it reads pictures, for servers that do not say so.
const IMAGE_WORDS: &[&str] = &["vl", "vlm", "vision", "llava", "pixtral", "omni"];

/// Words in a model's ID that say it thinks before it answers.
const REASONING_WORDS: &[&str] = &[
    "thinking",
    "think",
    "reasoning",
    "reasoner",
    "r1",
    "qwq",
    "magistral",
];

/// Whether `id` has one of `words` as a word of its own: `qwen2.5-vl:7b` has `vl`.
fn has_word(id: &str, words: &[&str]) -> bool {
    id.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| words.iter().any(|word| part.eq_ignore_ascii_case(word)))
        // "gpt-oss" is two.
        || (words.contains(&"reasoning") && id.to_ascii_lowercase().contains("gpt-oss"))
}

/// Something more to ask a server about its models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub url: String,
    /// A JSON body to POST; without one it is a GET.
    pub body: Option<String>,
    about: About,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum About {
    /// Ollama: one model's details.
    OllamaModel(String),
    /// Ollama: the models that are loaded, and with how much context.
    OllamaLoaded,
    /// LM Studio: every model's details.
    LmStudioModels,
    /// llama.cpp: what the server was started with.
    LlamaCppProps,
}

/// Most models asked about one by one.
const MOST_QUESTIONS: usize = 60;

/// What else to ask the server at `base_url` about the models it offers.
pub fn questions(base_url: &str, offer: &Offer) -> Vec<Question> {
    let root = root(base_url);
    match offer.kind {
        Kind::Ollama => offer
            .models
            .iter()
            .take(MOST_QUESTIONS)
            .map(|model| Question {
                url: format!("{root}/api/show"),
                body: Some(json!({ "model": model.id }).to_string()),
                about: About::OllamaModel(model.id.clone()),
            })
            .chain(std::iter::once(Question {
                url: format!("{root}/api/ps"),
                body: None,
                about: About::OllamaLoaded,
            }))
            .collect(),
        Kind::LmStudio => vec![Question {
            url: format!("{root}/api/v0/models"),
            body: None,
            about: About::LmStudioModels,
        }],
        Kind::LlamaCpp => vec![Question {
            url: format!("{root}/props"),
            body: None,
            about: About::LlamaCppProps,
        }],
        _ => Vec::new(),
    }
}

impl Offer {
    /// Adds what the server answered to a question. An answer that makes no sense adds nothing.
    /// The answers are added in the order of the questions.
    pub fn add_answer(&mut self, question: &Question, body: &str) {
        let Ok(answer) = serde_json::from_str::<Value>(body) else {
            return;
        };
        match &question.about {
            About::OllamaModel(id) => {
                if let Some(model) = self.models.iter_mut().find(|model| &model.id == id) {
                    add_ollama(model, &answer);
                }
            }
            About::OllamaLoaded => {
                for loaded in answer
                    .get("models")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let id = text(loaded, &["model", "name"]);
                    let context = number(loaded, &["/context_length"]);
                    if let Some(model) = self
                        .models
                        .iter_mut()
                        .find(|model| Some(model.id.as_str()) == id)
                        && context.is_some()
                    {
                        model.context_window = context;
                    }
                }
            }
            About::LmStudioModels => {
                let Ok(details) = parse_offer(body) else {
                    return;
                };
                for detail in details.models {
                    if let Some(model) = self.models.iter_mut().find(|model| model.id == detail.id)
                    {
                        *model = detail;
                    }
                }
            }
            About::LlamaCppProps => {
                let context = number(&answer, &["/default_generation_settings/n_ctx"]);
                let images = answer
                    .pointer("/modalities/vision")
                    .and_then(Value::as_bool);
                // The server has one model, unless it routes to several.
                if let [model] = self.models.as_mut_slice() {
                    model.context_window = context.or(model.context_window);
                    model.images = images.or(model.images);
                }
            }
        }
    }
}

fn add_ollama(model: &mut OfferedModel, answer: &Value) {
    let capability = |name: &str| lists(answer, "/capabilities", name);
    if let Some(images) = capability("vision") {
        model.images = Some(images);
    }
    if let Some(thinking) = capability("thinking") {
        model.reasoning = Some(thinking);
    }
    if let Some(chat) = capability("completion") {
        model.not_chat = !chat;
    }
    // "num_ctx 8192" among the model's parameters is what it runs with.
    let set = answer
        .get("parameters")
        .and_then(Value::as_str)
        .into_iter()
        .flat_map(str::lines)
        .find_map(|line| line.trim().strip_prefix("num_ctx")?.trim().parse().ok());
    let most = answer
        .get("model_info")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .find_map(|(key, value)| {
            key.ends_with(".context_length")
                .then(|| value.as_u64())
                .flatten()
        });
    model.context_window = set.or(most).or(model.context_window);
}

/// How long an answer may be, for a model whose context is too small for pi's default of 16,384:
/// a quarter of the context. `None` keeps the default.
pub fn fitting_max_tokens(context_window: u64) -> Option<u64> {
    (context_window < 4 * 16_384).then_some((context_window / 4).max(256))
}

/// The smallest conversation there is, to try a model: the URL and the body to POST. `None` for
/// an API it is not known for.
pub fn trial(api: &str, base_url: &str, model: &str) -> Option<(String, String)> {
    let base = base_url.trim_end_matches('/');
    let hello = json!([{ "role": "user", "content": "Say hi." }]);
    match api {
        "openai-completions" => Some((
            format!("{base}/chat/completions"),
            json!({ "model": model, "messages": hello, "max_tokens": 16 }).to_string(),
        )),
        "openai-responses" => Some((
            format!("{base}/responses"),
            json!({ "model": model, "input": "Say hi.", "max_output_tokens": 16 }).to_string(),
        )),
        "anthropic-messages" => Some((
            format!("{}/v1/messages", root(base)),
            json!({ "model": model, "messages": hello, "max_tokens": 16 }).to_string(),
        )),
        "google-generative-ai" => Some((
            format!("{base}/models/{model}:generateContent"),
            json!({
                "contents": [{ "parts": [{ "text": "Say hi." }] }],
                "generationConfig": { "maxOutputTokens": 16 },
            })
            .to_string(),
        )),
        _ => None,
    }
}

/// What a server says went wrong, from its reply: the message in it, else its first words.
pub fn error_message(body: &str) -> Option<String> {
    let body = body.trim();
    let message = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .pointer("/error/message")
            .or_else(|| value.get("error"))
            .or_else(|| value.get("message"))
            .or_else(|| value.get("detail"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    // A web page is no message.
    let message =
        message.or_else(|| (!body.starts_with(['<', '{', '['])).then(|| body.to_owned()))?;
    let mut line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if let Some((cut, _)) = line.char_indices().nth(200) {
        line.truncate(cut);
        line.push('…');
    }
    Some(line)
}

/// Providers pi has built in under these IDs (`llama.cpp` since pi 1.5). One in `models.json`
/// with the same ID is taken over by pi's, which is configured on its own, so pi says the
/// provider "is not configured" and nothing reaches the server.
const PI_PROVIDERS: &[&str] = &["llama.cpp"];

/// A provider ID for a new server: its program's name, else its host, made one word, and unlike
/// the ones `taken`.
pub fn provider_id(kind: Kind, base_url: &str, taken: &[&str]) -> String {
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let host = host(rest);
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0");
    let wanted = match kind.label() {
        Some(label) if local || kind == Kind::OpenRouter => label.to_owned(),
        _ if local => "local".to_owned(),
        // "api.example.com" is Example's.
        _ if host.parse::<std::net::IpAddr>().is_err() => {
            let parts: Vec<&str> = host.split('.').collect();
            match parts.as_slice() {
                [.., name, _] if !is_private(host) => (*name).to_owned(),
                [name, ..] => (*name).to_owned(),
                [] => "server".to_owned(),
            }
        }
        Some(label) => format!("{label} {host}"),
        None => host.to_owned(),
    };
    let mut id = String::new();
    for c in wanted.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            id.push(c.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = match id.trim_matches(['-', '.']) {
        "" => "server".to_owned(),
        // pi's own provider of that name, which needs its own sign-in, would take its place.
        id if PI_PROVIDERS.contains(&id) => id.replace('.', "-"),
        id => id.to_owned(),
    };
    let free = |id: &str| !taken.contains(&id);
    if free(&id) {
        return id;
    }
    (2..=taken.len() + 2)
        .map(|n| format!("{id}-{n}"))
        .find(|id| free(id))
        .unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn addresses_become_base_urls() {
        for (address, url) in [
            ("192.168.1.89:8888", "http://192.168.1.89:8888"),
            ("localhost:11434", "http://localhost:11434"),
            ("spark:8000/v1", "http://spark:8000/v1"),
            ("spark.local:8000", "http://spark.local:8000"),
            ("100.100.1.2:8000", "http://100.100.1.2:8000"),
            ("[::1]:8000", "http://[::1]:8000"),
            ("api.example.com", "https://api.example.com"),
            (
                " https://api.example.com/v1/ ",
                "https://api.example.com/v1",
            ),
            (
                "https://api.example.com/v1/chat/completions",
                "https://api.example.com/v1",
            ),
            ("http://h:1/v1/models?limit=2", "http://h:1/v1"),
            ("HTTP://8.8.8.8:80", "http://8.8.8.8:80"),
        ] {
            assert_eq!(base_url(address).unwrap(), url, "{address}");
        }
        assert_eq!(base_urls("h:1").unwrap(), ["http://h:1", "https://h:1"]);
        assert_eq!(base_urls("http://h:1").unwrap(), ["http://h:1"]);
        assert!(is_private_url("http://10.0.0.2:1/v1"));
        assert!(!is_private_url("https://openrouter.ai/api/v1"));
        for (url, at) in [
            ("http://127.0.0.1:39280/v1", Location::ThisDevice),
            ("http://localhost:11434/v1", Location::ThisDevice),
            ("http://[::1]:8000/v1", Location::ThisDevice),
            ("http://192.168.1.89:8888/v1", Location::OtherDevice),
            ("http://spark:8000/v1", Location::OtherDevice),
            ("http://spark.local:8000/v1", Location::OtherDevice),
            ("https://openrouter.ai/api/v1", Location::Online),
        ] {
            assert_eq!(location(url), at, "{url}");
        }
        for address in ["", "  ", "two words", "ftp://host", "http://", "/v1"] {
            assert!(base_url(address).is_err(), "{address}");
        }
        assert_eq!(
            short_address("http://192.168.1.89:8888/v1/"),
            "192.168.1.89:8888"
        );
        assert_eq!(short_address("https://x.com/openai/v1"), "x.com/openai");
    }

    #[test]
    fn devices_and_addresses() {
        let device = |host: &str| Ok(Lookup::Device(host.to_owned()));
        assert_eq!(lookup("192.168.1.89"), device("192.168.1.89"));
        assert_eq!(lookup(" spark.local/ "), device("spark.local"));
        assert_eq!(lookup("spark"), device("spark"));
        assert_eq!(lookup("[fd00::1]"), device("fd00::1"));
        assert_eq!(
            lookup("192.168.1.89:8888"),
            Ok(Lookup::Address(vec![
                "http://192.168.1.89:8888".into(),
                "https://192.168.1.89:8888".into()
            ]))
        );
        assert_eq!(
            lookup("[fd00::1]:8000"),
            Ok(Lookup::Address(vec![
                "http://[fd00::1]:8000".into(),
                "https://[fd00::1]:8000".into()
            ]))
        );
        assert_eq!(
            lookup("http://192.168.1.89"),
            Ok(Lookup::Address(vec!["http://192.168.1.89".into()]))
        );
        assert!(matches!(lookup("spark/v1"), Ok(Lookup::Address(_))));
        // A server on the internet is not a device to look around on.
        assert!(matches!(lookup("api.example.com"), Ok(Lookup::Address(_))));
        assert!(lookup("not an address").is_err());
    }

    #[test]
    fn known_servers_on_a_device() {
        let ollama = KNOWN_SERVERS[0];
        assert_eq!(ollama.kind, Kind::Ollama);
        assert_eq!(ollama.base_url(THIS_DEVICE), "http://localhost:11434/v1");
        assert_eq!(
            ollama.base_url("192.168.1.89"),
            "http://192.168.1.89:11434/v1"
        );
        assert_eq!(ollama.base_url("fd00::1"), "http://[fd00::1]:11434/v1");
        let ports: Vec<u16> = KNOWN_SERVERS.iter().map(|server| server.port).collect();
        assert_eq!(
            ports,
            [11434, 1234, 8080, 8000, 1337, 12434, 30000, 13305, 5001]
        );
        assert_eq!(
            KNOWN_SERVERS[5].base_url(THIS_DEVICE),
            "http://localhost:12434/engines/v1"
        );
    }

    #[test]
    fn known_servers_by_their_port() {
        for server in KNOWN_SERVERS {
            for host in [THIS_DEVICE, "192.168.1.89", "fd00::1"] {
                assert_eq!(kind_on_port(&server.base_url(host)), server.kind);
            }
        }
        assert_eq!(kind_on_port("http://localhost:1337"), Kind::Jan);
        // Several programs listen there by default.
        assert_eq!(kind_on_port("http://localhost:8080/v1"), Kind::Other);
        assert_eq!(kind_on_port("http://localhost/v1"), Kind::Other);
        assert_eq!(kind_on_port("https://example.com:1337/v1"), Kind::Other);
    }

    #[test]
    fn tries_the_apis_in_order() {
        let urls = |base: &str| -> Vec<(String, String)> {
            attempts(base)
                .into_iter()
                .map(|attempt| (attempt.url, attempt.base_url))
                .collect()
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            urls("http://h:1"),
            [
                pair("http://h:1/v1/models", "http://h:1/v1"),
                pair("http://h:1/models", "http://h:1"),
                pair("http://h:1/v1/models", "http://h:1"),
                pair("http://h:1/v1beta/models", "http://h:1/v1beta"),
            ]
        );
        assert_eq!(
            urls("http://h:1/v1")[0],
            pair("http://h:1/v1/models", "http://h:1/v1")
        );
        assert_eq!(
            urls("https://x.com/openai")[..2],
            [
                pair("https://x.com/openai/models", "https://x.com/openai"),
                pair("https://x.com/openai/v1/models", "https://x.com/openai/v1"),
            ]
        );
    }

    #[test]
    fn reads_vllm() {
        let offer = parse_offer(
            r#"{"object":"list","data":[{"id":"Qwen/Qwen3-VL-30B","object":"model",
            "owned_by":"vllm","root":"/m","max_model_len":262144}]}"#,
        )
        .unwrap();
        assert_eq!(offer.kind, Kind::Vllm);
        assert_eq!(offer.api, "openai-completions");
        let model = &offer.models[0];
        assert_eq!(model.context_window, Some(262_144));
        assert_eq!(model.images, Some(true));
        assert_eq!(model.reasoning, None);
        assert_eq!(model.name, None);
    }

    #[test]
    fn reads_openrouter_and_relatives() {
        let offer = parse_offer(
            r#"{"data":[{"id":"openai/gpt-oss-120b","name":"OpenAI: gpt-oss-120b","context_length":131072,
            "architecture":{"input_modalities":["text"],"output_modalities":["text"]},
            "top_provider":{"context_length":131000,"max_completion_tokens":32768},
            "supported_parameters":["reasoning","tools"]},
            {"id":"a/painter","architecture":{"input_modalities":["text"],"output_modalities":["image"]},"top_provider":{}}]}"#,
        )
        .unwrap();
        assert_eq!(offer.kind, Kind::OpenRouter);
        assert!(offer.models[0].not_chat);
        assert_eq!(
            offer.models[1],
            OfferedModel {
                id: "openai/gpt-oss-120b".into(),
                name: Some("OpenAI: gpt-oss-120b".into()),
                context_window: Some(131_000),
                max_tokens: Some(32_768),
                reasoning: Some(true),
                images: Some(false),
                not_chat: false,
            }
        );
        // Groq; the whole context as the longest answer is no limit to write down.
        let offer = parse_offer(
            r#"{"data":[{"id":"llama","owned_by":"Meta","context_window":131072,"max_completion_tokens":131072}]}"#,
        )
        .unwrap();
        assert_eq!(offer.models[0].context_window, Some(131_072));
        assert_eq!(offer.models[0].max_tokens, None);
        // Together sends a bare list.
        let offer = parse_offer(
            r#"[{"id":"a","type":"chat","context_length":8192},{"id":"b","type":"embedding"}]"#,
        )
        .unwrap();
        assert!(!offer.models[0].not_chat);
        assert!(offer.models[1].not_chat);
    }

    #[test]
    fn recognizes_anthropic_and_gemini() {
        let offer = parse_offer(
            r#"{"data":[{"type":"model","id":"claude-x","display_name":"Claude X","created_at":"","max_input_tokens":200000}],"has_more":false}"#,
        )
        .unwrap();
        assert_eq!(offer.api, "anthropic-messages");
        assert_eq!(offer.models[0].name.as_deref(), Some("Claude X"));
        assert_eq!(offer.models[0].context_window, Some(200_000));
        let offer = parse_offer(
            r#"{"models":[{"name":"models/gemini-x","displayName":"Gemini X","inputTokenLimit":1000000,
            "outputTokenLimit":65536,"supportedGenerationMethods":["generateContent"],"thinking":true},
            {"name":"models/text-emb","supportedGenerationMethods":["embedContent"]}]}"#,
        )
        .unwrap();
        assert_eq!(offer.api, "google-generative-ai");
        assert_eq!(offer.models[0].id, "gemini-x");
        assert_eq!(offer.models[0].reasoning, Some(true));
        assert_eq!(offer.models[0].max_tokens, Some(65_536));
        assert!(offer.models[1].not_chat);
    }

    #[test]
    fn asks_ollama_about_each_model() {
        let mut offer = parse_offer(
            r#"{"object":"list","data":[{"id":"qwen3:8b","object":"model","owned_by":"library"},
            {"id":"nomic-embed-text:latest","object":"model","owned_by":"library"}]}"#,
        )
        .unwrap();
        assert_eq!(offer.kind, Kind::Ollama);
        assert!(offer.models[0].not_chat, "by its name");
        let questions = questions("http://localhost:11434/v1", &offer);
        assert_eq!(questions.len(), 3);
        assert_eq!(questions[1].url, "http://localhost:11434/api/show");
        assert_eq!(
            questions[1].body.as_deref(),
            Some(r#"{"model":"qwen3:8b"}"#)
        );
        offer.add_answer(
            &questions[1],
            r#"{"capabilities":["completion","tools","thinking"],"parameters":"stop  \"x\"\nnum_ctx    16384",
            "model_info":{"qwen3.context_length":40960}}"#,
        );
        assert_eq!(offer.models[1].context_window, Some(16_384));
        assert_eq!(offer.models[1].reasoning, Some(true));
        assert_eq!(offer.models[1].images, Some(false));
        offer.add_answer(&questions[1], "oops");
        offer.add_answer(
            &questions[2],
            r#"{"models":[{"model":"qwen3:8b","context_length":4096}]}"#,
        );
        assert_eq!(offer.models[1].context_window, Some(4096));
    }

    #[test]
    fn asks_lm_studio_and_llama_cpp() {
        let mut offer = parse_offer(
            r#"{"data":[{"id":"qwen2-vl-7b","object":"model","owned_by":"organization_owner"}]}"#,
        )
        .unwrap();
        let ask = questions("http://localhost:1234/v1", &offer);
        assert_eq!(ask[0].url, "http://localhost:1234/api/v0/models");
        offer.add_answer(
            &ask[0],
            r#"{"data":[{"id":"qwen2-vl-7b","type":"vlm","publisher":"q","max_context_length":32768,"loaded_context_length":4096}]}"#,
        );
        assert_eq!(offer.models[0].context_window, Some(4096));
        assert_eq!(offer.models[0].images, Some(true));

        let mut offer = parse_offer(
            r#"{"models":[{"name":"x.gguf","capabilities":["completion"]}],
            "data":[{"id":"x.gguf","owned_by":"llamacpp","meta":{"n_ctx_train":131072}}]}"#,
        )
        .unwrap();
        assert_eq!(offer.kind, Kind::LlamaCpp);
        assert_eq!(offer.models[0].context_window, Some(131_072));
        let ask = questions("http://h:8080/v1", &offer);
        assert_eq!(ask[0].url, "http://h:8080/props");
        offer.add_answer(
            &ask[0],
            r#"{"default_generation_settings":{"n_ctx":8192},"modalities":{"vision":true}}"#,
        );
        assert_eq!(offer.models[0].context_window, Some(8192));
        assert_eq!(offer.models[0].images, Some(true));
    }

    #[test]
    fn says_what_is_wrong_with_a_list() {
        for (body, message) in [
            ("", "The server sent an empty reply."),
            ("<html>", "The server did not send a model list: <html>"),
            (
                r#"{"error":{"message":"no key"}}"#,
                "The server answered: no key",
            ),
            (
                r#"{"detail":"Not Found"}"#,
                "The server answered: Not Found",
            ),
            ("{}", "The reply has no model list."),
        ] {
            assert_eq!(parse_offer(body).unwrap_err().to_string(), message);
        }
        assert_eq!(
            error_message(r#"{"error":"nope"}"#).as_deref(),
            Some("nope")
        );
        assert_eq!(
            error_message("  Bad\n gateway ").as_deref(),
            Some("Bad gateway")
        );
        assert_eq!(error_message("<html>x</html>"), None);
        assert_eq!(error_message(""), None);
        assert_eq!(
            error_message(&"å".repeat(300)).unwrap().chars().count(),
            201
        );
    }

    #[test]
    fn words_in_names() {
        assert!(has_word("qwen2.5-vl:7b", IMAGE_WORDS));
        assert!(!has_word("vlad-7b", IMAGE_WORDS));
        assert!(has_word("DeepSeek-R1-Distill", REASONING_WORDS));
        assert!(has_word("openai/gpt-oss-20b", REASONING_WORDS));
        assert!(!has_word("llama-3.1-8b", REASONING_WORDS));
    }

    #[test]
    fn names_new_providers() {
        let id = |kind, url| provider_id(kind, url, &["ollama", "spark"]);
        assert_eq!(id(Kind::Ollama, "http://localhost:11434/v1"), "ollama-2");
        assert_eq!(id(Kind::LmStudio, "http://localhost:1234/v1"), "lm-studio");
        assert_eq!(id(Kind::Other, "http://localhost:9/v1"), "local");
        assert_eq!(id(Kind::LlamaCpp, "http://127.0.0.1:8080/v1"), "llama-cpp");
        assert_eq!(
            id(Kind::Vllm, "http://192.168.1.89:8888/v1"),
            "vllm-192.168.1.89"
        );
        assert_eq!(id(Kind::Vllm, "http://spark:8888/v1"), "spark-2");
        assert_eq!(id(Kind::Other, "http://box.local:1/v1"), "box");
        assert_eq!(id(Kind::Other, "https://api.example.com/v1"), "example");
        assert_eq!(id(Kind::Other, "https://[2001:db8::1]:1/v1"), "2001-db8-1");
    }

    #[test]
    fn limits_that_fit() {
        assert_eq!(fitting_max_tokens(8192), Some(2048));
        assert_eq!(fitting_max_tokens(131_072), None);
        assert_eq!(fitting_max_tokens(512), Some(256));
    }

    #[test]
    fn trials_are_sent_where_the_api_is() {
        let url = |api, base| trial(api, base, "m").unwrap().0;
        assert_eq!(
            url("openai-completions", "http://h/v1/"),
            "http://h/v1/chat/completions"
        );
        assert_eq!(
            url("anthropic-messages", "http://h"),
            "http://h/v1/messages"
        );
        assert_eq!(
            url("anthropic-messages", "http://h/v1"),
            "http://h/v1/messages"
        );
        assert_eq!(
            url("google-generative-ai", "http://h/v1beta"),
            "http://h/v1beta/models/m:generateContent"
        );
        assert_eq!(trial("other", "http://h", "m"), None);
        assert_eq!(
            headers("openai-completions", Some("k")),
            [("Authorization", "Bearer k".to_owned())]
        );
        assert_eq!(headers("openai-completions", None), []);
    }
}
