//! Editing pi's configuration files for the GUI: `models.json` (custom providers and models,
//! for the "Add model" dialog), skills ([`skills`]) and the project trust store ([`trust`]).
//!
//! The file is kept as a JSON value with its key order preserved, so adding a model changes
//! only what was added. See pi's `docs/models.md` and `docs/models-configuration.md` for the
//! format: `{"providers": {"<id>": {baseUrl, api, apiKey, models: [{id, …}]}}}`.

pub mod skills;
pub mod trust;

use serde_json::{Map, Value, json};
use thiserror::Error;

/// The `$schema` pi documents for editor validation; added to files the GUI creates.
pub const SCHEMA_URL: &str = "https://pi.dev/schemas/models.schema.json";

/// The APIs a custom provider can speak, with a short explanation for the UI.
pub const APIS: &[(&str, &str)] = &[
    (
        "openai-completions",
        "OpenAI-compatible chat completions (Ollama, LM Studio, vLLM, llama.cpp, OpenRouter, …)",
    ),
    ("openai-responses", "OpenAI Responses API"),
    (
        "anthropic-messages",
        "Anthropic Messages API (and compatible proxies)",
    ),
    ("google-generative-ai", "Google Gemini API"),
];

/// A common server, to prefill the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub label: &'static str,
    pub provider_id: &'static str,
    pub provider_name: &'static str,
    pub base_url: &'static str,
    pub api: &'static str,
    /// What to put in `apiKey` (a dummy for local servers, an environment variable otherwise).
    pub api_key: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Ollama",
        provider_id: "ollama",
        provider_name: "Ollama",
        base_url: "http://localhost:11434/v1",
        api: "openai-completions",
        api_key: "ollama",
    },
    Preset {
        label: "LM Studio",
        provider_id: "lmstudio",
        provider_name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        api: "openai-completions",
        api_key: "lmstudio",
    },
    Preset {
        label: "vLLM",
        provider_id: "vllm",
        provider_name: "vLLM",
        base_url: "http://localhost:8000/v1",
        api: "openai-completions",
        api_key: "vllm",
    },
    Preset {
        label: "llama.cpp server",
        provider_id: "llamacpp",
        provider_name: "llama.cpp",
        base_url: "http://localhost:8080/v1",
        api: "openai-completions",
        api_key: "llamacpp",
    },
    Preset {
        label: "OpenRouter",
        provider_id: "openrouter",
        provider_name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        api: "openai-completions",
        api_key: "$OPENROUTER_API_KEY",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// The text is not valid JSON (1-based line and column).
    #[error("Invalid JSON at line {line}, column {column}: {message}")]
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    /// Valid JSON with the wrong shape, or a form value that cannot be used.
    #[error("{0}")]
    Invalid(String),
}

/// What the file says about one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub has_key: bool,
    /// The `apiKey` value as written (a key, `$ENV` or `!command`).
    pub api_key: Option<String>,
    pub model_ids: Vec<String>,
}

impl ProviderInfo {
    /// The name to show: the display name, else the id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// A provider to create along with the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub api_key: String,
}

/// A model to add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub reasoning: bool,
    pub images: bool,
}

/// A parsed `models.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsFile {
    doc: Map<String, Value>,
}

impl Default for ModelsFile {
    fn default() -> Self {
        let mut doc = Map::new();
        doc.insert("$schema".into(), SCHEMA_URL.into());
        doc.insert("providers".into(), Value::Object(Map::new()));
        Self { doc }
    }
}

impl ModelsFile {
    /// Parses and checks the file. Blank text is an empty file (with `$schema`).
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_str(text).map_err(|error| ConfigError::Syntax {
            line: error.line(),
            column: error.column(),
            message: strip_position(&error.to_string()),
        })?;
        let Value::Object(doc) = value else {
            return Err(ConfigError::Invalid(
                "models.json must be a JSON object, like {\"providers\": {…}}.".into(),
            ));
        };
        let this = Self { doc };
        this.check()?;
        Ok(this)
    }

    /// Checks the shape pi needs: `providers` an object of objects, each `models` a list of
    /// objects with a non-empty `id`.
    fn check(&self) -> Result<(), ConfigError> {
        let Some(providers) = self.doc.get("providers") else {
            return Ok(());
        };
        let Some(providers) = providers.as_object() else {
            return Err(ConfigError::Invalid(
                "\"providers\" must be an object.".into(),
            ));
        };
        for (id, provider) in providers {
            let Some(provider) = provider.as_object() else {
                return Err(ConfigError::Invalid(format!(
                    "Provider \"{id}\" must be an object."
                )));
            };
            match provider.get("models") {
                None => {}
                Some(Value::Array(models)) => {
                    for (index, model) in models.iter().enumerate() {
                        let has_id = model
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| !id.trim().is_empty());
                        if !has_id {
                            return Err(ConfigError::Invalid(format!(
                                "Model {} of provider \"{id}\" needs an \"id\".",
                                index + 1
                            )));
                        }
                    }
                }
                Some(_) => {
                    return Err(ConfigError::Invalid(format!(
                        "\"models\" of provider \"{id}\" must be a list."
                    )));
                }
            }
        }
        Ok(())
    }

    /// The providers the file defines or changes, in file order.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        let Some(providers) = self.doc.get("providers").and_then(Value::as_object) else {
            return Vec::new();
        };
        providers
            .iter()
            .map(|(id, provider)| {
                let text = |key: &str| {
                    provider
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .filter(|value| !value.is_empty())
                };
                ProviderInfo {
                    id: id.clone(),
                    name: text("name"),
                    base_url: text("baseUrl"),
                    api: text("api"),
                    has_key: text("apiKey").is_some(),
                    api_key: text("apiKey"),
                    model_ids: provider
                        .get("models")
                        .and_then(Value::as_array)
                        .map(|models| {
                            models
                                .iter()
                                .filter_map(|model| model.get("id")?.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Adds `model` to provider `provider_id` (an entry with the same id is replaced, as pi
    /// itself would). With `new_provider`, the provider is created first (its id must not be
    /// in the file yet; it may name a built-in provider, which pi then extends).
    pub fn add_model(
        &mut self,
        provider_id: &str,
        new_provider: Option<&NewProvider>,
        model: &ModelSpec,
    ) -> Result<(), ConfigError> {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() || provider_id.chars().any(char::is_whitespace) {
            return Err(ConfigError::Invalid(
                "The provider ID must be a single word, like \"ollama\".".into(),
            ));
        }
        let model_id = model.id.trim();
        if model_id.is_empty() {
            return Err(ConfigError::Invalid("Enter the model ID.".into()));
        }
        let providers = self
            .doc
            .entry("providers")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(providers) = providers.as_object_mut() else {
            return Err(ConfigError::Invalid(
                "\"providers\" must be an object.".into(),
            ));
        };
        if let Some(new) = new_provider {
            if providers.contains_key(provider_id) {
                return Err(ConfigError::Invalid(format!(
                    "A provider \"{provider_id}\" already exists; choose it instead."
                )));
            }
            let base_url = new.base_url.trim();
            if base_url.is_empty() {
                return Err(ConfigError::Invalid(
                    "Enter the server's base URL, like http://localhost:11434/v1.".into(),
                ));
            }
            if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
                return Err(ConfigError::Invalid(
                    "The base URL must start with http:// or https://.".into(),
                ));
            }
            let mut provider = Map::new();
            if !new.name.trim().is_empty() {
                provider.insert("name".into(), new.name.trim().into());
            }
            provider.insert("baseUrl".into(), base_url.trim_end_matches('/').into());
            let api = if new.api.trim().is_empty() {
                "openai-completions"
            } else {
                new.api.trim()
            };
            provider.insert("api".into(), api.into());
            if !new.api_key.trim().is_empty() {
                provider.insert("apiKey".into(), new.api_key.trim().into());
            }
            provider.insert("models".into(), Value::Array(Vec::new()));
            providers.insert(provider_id.to_owned(), Value::Object(provider));
        }
        let Some(provider) = providers
            .get_mut(provider_id)
            .and_then(Value::as_object_mut)
        else {
            return Err(ConfigError::Invalid(format!(
                "There is no provider \"{provider_id}\" in models.json."
            )));
        };
        let models = provider
            .entry("models")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(models) = models.as_array_mut() else {
            return Err(ConfigError::Invalid(format!(
                "\"models\" of provider \"{provider_id}\" must be a list."
            )));
        };
        let entry = model_entry(model_id, model);
        match models
            .iter_mut()
            .find(|existing| existing.get("id").and_then(Value::as_str) == Some(model_id))
        {
            Some(existing) => *existing = entry,
            None => models.push(entry),
        }
        Ok(())
    }

    /// The file's text: pretty-printed, two-space indent, trailing newline.
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(&Value::Object(self.doc.clone()))
            .unwrap_or_else(|_| "{}".to_owned());
        text.push('\n');
        text
    }
}

fn model_entry(id: &str, model: &ModelSpec) -> Value {
    let mut entry = Map::new();
    entry.insert("id".into(), id.into());
    if !model.name.trim().is_empty() {
        entry.insert("name".into(), model.name.trim().into());
    }
    if model.reasoning {
        entry.insert("reasoning".into(), true.into());
    }
    if model.images {
        entry.insert("input".into(), json!(["text", "image"]));
    }
    if let Some(window) = model.context_window {
        entry.insert("contextWindow".into(), window.into());
    }
    if let Some(max) = model.max_tokens {
        entry.insert("maxTokens".into(), max.into());
    }
    Value::Object(entry)
}

/// `serde_json` appends " at line L column C"; the error type reports those separately.
fn strip_position(message: &str) -> String {
    message
        .rfind(" at line ")
        .map_or(message, |index| &message[..index])
        .to_owned()
}

/// The URL that lists a server's models (`<base>/models`), for OpenAI-compatible servers.
pub fn models_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim().trim_end_matches('/'))
}

/// An `apiKey` value that can be sent as is: not an environment reference (`$NAME`) or a
/// command (`!cmd`), which pi resolves itself.
pub fn literal_key(api_key: &str) -> Option<&str> {
    let key = api_key.trim();
    (!key.is_empty() && !key.starts_with('$') && !key.starts_with('!')).then_some(key)
}

/// Model IDs from a model-list response: `OpenAI` style (`{"data": [{"id"}]}`) or Ollama style
/// (`{"models": [{"name"}]}`), sorted and without duplicates.
pub fn parse_model_list(body: &str) -> Result<Vec<String>, ConfigError> {
    let value: Value = serde_json::from_str(body).map_err(|_| {
        let start: String = body.trim().chars().take(160).collect();
        ConfigError::Invalid(if start.is_empty() {
            "The server sent an empty reply.".to_owned()
        } else {
            format!("The server did not send a model list: {start}")
        })
    })?;
    let list = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            let message = value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
                .map_or_else(
                    || "The reply has no model list.".to_owned(),
                    |error| format!("The server answered: {error}"),
                );
            ConfigError::Invalid(message)
        })?;
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|model| {
            model
                .get("id")
                .or_else(|| model.get("name"))
                .or_else(|| model.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const EXISTING: &str = r#"{
  "providers": {
    "spark": {
      "baseUrl": "http://192.168.1.50:8888/v1",
      "api": "openai-completions",
      "apiKey": "secret",
      "compat": { "thinkingFormat": "openai" },
      "models": [
        { "id": "qwen3.8-27b", "name": "Qwen", "baseUrl": "http://192.168.1.50:8890/v1" }
      ]
    }
  }
}"#;

    fn model(id: &str) -> ModelSpec {
        ModelSpec {
            id: id.into(),
            ..ModelSpec::default()
        }
    }

    #[test]
    fn lists_providers() {
        let file = ModelsFile::parse(EXISTING).unwrap();
        assert_eq!(
            file.providers(),
            vec![ProviderInfo {
                id: "spark".into(),
                name: None,
                base_url: Some("http://192.168.1.50:8888/v1".into()),
                api: Some("openai-completions".into()),
                has_key: true,
                api_key: Some("secret".into()),
                model_ids: vec!["qwen3.8-27b".into()],
            }]
        );
        assert_eq!(file.providers()[0].label(), "spark");
    }

    #[test]
    fn adds_to_an_existing_provider_keeping_everything_else() {
        let mut file = ModelsFile::parse(EXISTING).unwrap();
        file.add_model(
            "spark",
            None,
            &ModelSpec {
                id: "inkling-2-mini".into(),
                name: "Inkling".into(),
                context_window: Some(131_072),
                max_tokens: None,
                reasoning: true,
                images: true,
            },
        )
        .unwrap();
        let text = file.to_text();
        let again = ModelsFile::parse(&text).unwrap();
        assert_eq!(
            again.providers()[0].model_ids,
            ["qwen3.8-27b", "inkling-2-mini"]
        );
        // Untouched keys stay, in their order.
        let base = text.find("\"baseUrl\"").unwrap();
        let compat = text.find("\"compat\"").unwrap();
        assert!(base < compat, "{text}");
        assert!(text.contains(r#""thinkingFormat": "openai""#));
        assert!(
            text.contains(
                r#""input": [
            "text",
            "image"
          ]"#
            ),
            "{text}"
        );
        assert!(text.contains(r#""contextWindow": 131072"#));
        assert!(!text.contains("maxTokens"));
        assert!(text.ends_with("}\n"));

        // Same id again replaces the entry.
        file.add_model("spark", None, &model("inkling-2-mini"))
            .unwrap();
        let providers = file.providers();
        assert_eq!(providers[0].model_ids, ["qwen3.8-27b", "inkling-2-mini"]);
        assert!(!file.to_text().contains("Inkling"));
    }

    #[test]
    fn creates_providers_and_files() {
        let mut file = ModelsFile::parse("").unwrap();
        let new = NewProvider {
            id: "ollama".into(),
            name: "Ollama".into(),
            base_url: "http://localhost:11434/v1/".into(),
            api: String::new(),
            api_key: "ollama".into(),
        };
        file.add_model("ollama", Some(&new), &model("qwen2.5-coder:7b"))
            .unwrap();
        let text = file.to_text();
        assert!(
            text.starts_with("{\n  \"$schema\": \"https://pi.dev/schemas/models.schema.json\"")
        );
        let provider = &ModelsFile::parse(&text).unwrap().providers()[0];
        assert_eq!(
            provider.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(provider.api.as_deref(), Some("openai-completions"));
        assert_eq!(provider.model_ids, ["qwen2.5-coder:7b"]);

        // Creating it again is refused; a missing provider without details too.
        assert!(file.add_model("ollama", Some(&new), &model("x")).is_err());
        assert!(file.add_model("nope", None, &model("x")).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        let mut file = ModelsFile::default();
        let new = NewProvider {
            id: "p".into(),
            base_url: "localhost:1234".into(),
            ..NewProvider::default()
        };
        let error = file.add_model("p", Some(&new), &model("m")).unwrap_err();
        assert!(error.to_string().contains("http://"), "{error}");
        assert!(file.add_model("p q", Some(&new), &model("m")).is_err());
        assert!(file.add_model("p", Some(&new), &model(" ")).is_err());
        let no_url = NewProvider {
            id: "p".into(),
            ..NewProvider::default()
        };
        assert!(file.add_model("p", Some(&no_url), &model("m")).is_err());
    }

    #[test]
    fn validates_shape_and_reports_positions() {
        match ModelsFile::parse("{\n  \"providers\": {\n    \"a\": {,}\n  }\n}") {
            Err(ConfigError::Syntax {
                line,
                column,
                message,
            }) => {
                assert_eq!((line, column), (3, 11));
                assert!(!message.contains("at line"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        for (text, needle) in [
            ("[]", "must be a JSON object"),
            (r#"{"providers": []}"#, "\"providers\" must be an object"),
            (r#"{"providers": {"a": 1}}"#, "Provider \"a\""),
            (r#"{"providers": {"a": {"models": {}}}}"#, "must be a list"),
            (
                r#"{"providers": {"a": {"models": [{"name": "x"}]}}}"#,
                "needs an \"id\"",
            ),
        ] {
            let error = ModelsFile::parse(text).unwrap_err().to_string();
            assert!(error.contains(needle), "{text}: {error}");
        }
        assert_eq!(
            ModelsFile::parse(r#"{"other": 1}"#).unwrap().providers(),
            Vec::new()
        );
    }

    #[test]
    fn model_lists_and_keys() {
        assert_eq!(
            parse_model_list(r#"{"object":"list","data":[{"id":"b"},{"id":"a"},{"id":"a"}]}"#)
                .unwrap(),
            ["a", "b"]
        );
        assert_eq!(
            parse_model_list(r#"{"models":[{"name":"qwen2.5:7b"}]}"#).unwrap(),
            ["qwen2.5:7b"]
        );
        let error = parse_model_list(r#"{"error":{"message":"invalid key"}}"#).unwrap_err();
        assert!(error.to_string().contains("invalid key"));
        assert!(parse_model_list("<html>").is_err());
        assert_eq!(models_url("http://h:1/v1/"), "http://h:1/v1/models");
        assert_eq!(literal_key(" sk-1 "), Some("sk-1"));
        assert_eq!(literal_key("$OPENAI_API_KEY"), None);
        assert_eq!(literal_key("!pass show x"), None);
        assert_eq!(literal_key(""), None);
    }
}
