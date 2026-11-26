//! Editing pi's configuration files for the GUI: `models.json` (custom providers and models,
//! for the Models dialog), skills ([`skills`]) and the project trust store ([`trust`]).
//!
//! The file is kept as a JSON value with its key order preserved, so adding a model changes
//! only what was added. See pi's `docs/models.md` and `docs/models-configuration.md` for the
//! format: `{"providers": {"<id>": {baseUrl, api, apiKey, models: [{id, …}]}}}`.

pub mod available;
pub mod providers;
pub mod server;
pub mod settings;
pub mod skills;
pub mod trust;

use serde_json::{Map, Value, json};
use thiserror::Error;

/// The `$schema` pi documents for editor validation; added to files the GUI creates.
pub const SCHEMA_URL: &str = "https://pi.dev/schemas/models.schema.json";

/// The APIs a custom provider can speak, with a short explanation for the UI.
pub const APIS: &[(&str, &str)] = &[
    ("openai-completions", "OpenAI chat completions"),
    ("openai-responses", "OpenAI responses"),
    ("anthropic-messages", "Anthropic messages"),
    ("google-generative-ai", "Google Gemini"),
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
    /// The `apiKey` value as written (a key, `$ENV` or `!command`).
    pub api_key: Option<String>,
    pub models: Vec<SavedModel>,
}

impl ProviderInfo {
    /// The name to show: the display name, else pi's own for a built-in provider, else the id.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| providers::built_in(&self.id).map(|provider| provider.label))
            .unwrap_or(&self.id)
    }

    /// pi's own provider of this name, when the file does no more than give it a key.
    pub fn built_in(&self) -> Option<&'static providers::BuiltIn> {
        providers::built_in(&self.id)
            .filter(|_| self.base_url.is_none() && self.api.is_none() && self.models.is_empty())
    }
}

/// A model in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedModel {
    pub spec: ModelSpec,
    /// Its own address, when it is not the provider's.
    pub base_url: Option<String>,
}

/// What the form sets of a provider. Empty text leaves the field out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderSpec {
    pub name: String,
    pub base_url: String,
    pub api: String,
    pub api_key: String,
}

/// What the form sets of a model.
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
                    api_key: text("apiKey"),
                    models: provider
                        .get("models")
                        .and_then(Value::as_array)
                        .map_or_default(|models| models.iter().filter_map(saved_model).collect()),
                }
            })
            .collect()
    }

    /// The `providers` object, made if there is none.
    fn providers_mut(&mut self) -> Result<&mut Map<String, Value>, ConfigError> {
        self.doc
            .entry("providers")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| ConfigError::Invalid("\"providers\" must be an object.".into()))
    }

    fn provider_mut(&mut self, id: &str) -> Result<&mut Map<String, Value>, ConfigError> {
        self.providers_mut()?
            .get_mut(id)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                ConfigError::Invalid(format!("There is no provider \"{id}\" in models.json."))
            })
    }

    /// Adds a provider with a server of its own, which must not be in the file yet.
    pub fn add_provider(&mut self, id: &str, spec: &ProviderSpec) -> Result<(), ConfigError> {
        let id = id.trim();
        if id.is_empty() || id.chars().any(char::is_whitespace) || id.contains('/') {
            return Err(ConfigError::Invalid(
                "The provider ID must be a single word, like \"ollama\".".into(),
            ));
        }
        if self.providers_mut()?.contains_key(id) {
            return Err(ConfigError::Invalid(format!(
                "There already is a provider \"{id}\". Choose another ID."
            )));
        }
        if spec.base_url.trim().is_empty() {
            return Err(ConfigError::Invalid("Enter the server's address.".into()));
        }
        self.providers_mut()?
            .insert(id.to_owned(), Value::Object(Map::new()));
        let added = self.set_provider(id, spec);
        if added.is_err() {
            self.providers_mut()?.shift_remove(id);
        }
        added
    }

    /// Sets the fields of a provider that is in the file, keeping all others.
    pub fn set_provider(&mut self, id: &str, spec: &ProviderSpec) -> Result<(), ConfigError> {
        let base_url = spec.base_url.trim().trim_end_matches('/');
        if !(base_url.is_empty()
            || base_url.starts_with("http://")
            || base_url.starts_with("https://"))
        {
            return Err(ConfigError::Invalid(
                "The address must start with http:// or https://.".into(),
            ));
        }
        let provider = self.provider_mut(id)?;
        set_text(provider, "name", &spec.name);
        set_text(provider, "baseUrl", base_url);
        set_text(provider, "api", &spec.api);
        set_text(provider, "apiKey", &spec.api_key);
        Ok(())
    }

    /// Sets the key of a provider, which is added if it is not in the file: pi's own ones need
    /// no more than that. Without a key, the provider goes when there is nothing else in it.
    pub fn set_key(&mut self, id: &str, key: &str) -> Result<(), ConfigError> {
        let providers = self.providers_mut()?;
        let provider = providers
            .entry(id)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| ConfigError::Invalid(format!("Provider \"{id}\" must be an object.")))?;
        set_text(provider, "apiKey", key);
        if provider.is_empty() {
            providers.shift_remove(id);
        }
        Ok(())
    }

    /// Removes a provider and its models.
    pub fn remove_provider(&mut self, id: &str) {
        if let Ok(providers) = self.providers_mut() {
            providers.shift_remove(id);
        }
    }

    /// Adds a model to a provider that is in the file, or sets the fields of the one with that
    /// id, keeping all its others (how it thinks, what it costs, its own address).
    pub fn set_model(&mut self, provider: &str, model: &ModelSpec) -> Result<(), ConfigError> {
        let id = model.id.trim();
        if id.is_empty() {
            return Err(ConfigError::Invalid("Enter the model ID.".into()));
        }
        let models = self
            .provider_mut(provider)?
            .entry("models")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "\"models\" of provider \"{provider}\" must be a list."
                ))
            })?;
        let existing = models
            .iter_mut()
            .find(|existing| existing.get("id").and_then(Value::as_str) == Some(id))
            .and_then(Value::as_object_mut);
        match existing {
            Some(entry) => set_model_fields(entry, model),
            None => {
                let mut entry = Map::new();
                entry.insert("id".into(), id.into());
                set_model_fields(&mut entry, model);
                models.push(Value::Object(entry));
            }
        }
        Ok(())
    }

    /// Removes a model from a provider.
    pub fn remove_model(&mut self, provider: &str, id: &str) {
        if let Ok(provider) = self.provider_mut(provider)
            && let Some(models) = provider.get_mut("models").and_then(Value::as_array_mut)
        {
            models.retain(|model| model.get("id").and_then(Value::as_str) != Some(id));
        }
    }

    /// The file's text: pretty-printed, two-space indent, trailing newline.
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(&Value::Object(self.doc.clone()))
            .unwrap_or_else(|_| "{}".to_owned());
        text.push('\n');
        text
    }
}

fn set_text(object: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        object.shift_remove(key);
    } else {
        object.insert(key.to_owned(), value.into());
    }
}

/// Sets what a [`ModelSpec`] says; what pi assumes anyway is left out.
fn set_model_fields(entry: &mut Map<String, Value>, model: &ModelSpec) {
    set_text(entry, "name", &model.name);
    if model.reasoning {
        entry.insert("reasoning".into(), true.into());
    } else {
        entry.shift_remove("reasoning");
    }
    if model.images != reads_images(entry) {
        if model.images {
            entry.insert("input".into(), json!(["text", "image"]));
        } else {
            entry.shift_remove("input");
        }
    }
    for (key, value) in [
        ("contextWindow", model.context_window),
        ("maxTokens", model.max_tokens),
    ] {
        match value {
            Some(value) => {
                entry.insert(key.into(), value.into());
            }
            None => {
                entry.shift_remove(key);
            }
        }
    }
}

fn reads_images(entry: &Map<String, Value>) -> bool {
    entry
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|input| input.iter().any(|kind| kind.as_str() == Some("image")))
}

fn saved_model(value: &Value) -> Option<SavedModel> {
    let entry = value.as_object()?;
    let text = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Some(SavedModel {
        spec: ModelSpec {
            id: text("id")?,
            name: text("name").unwrap_or_default(),
            context_window: entry.get("contextWindow").and_then(Value::as_u64),
            max_tokens: entry.get("maxTokens").and_then(Value::as_u64),
            reasoning: entry.get("reasoning").and_then(Value::as_bool) == Some(true),
            images: reads_images(entry),
        },
        base_url: text("baseUrl"),
    })
}

/// `serde_json` appends " at line L column C"; the error type reports those separately.
fn strip_position(message: &str) -> String {
    message
        .rfind(" at line ")
        .map_or(message, |index| &message[..index])
        .to_owned()
}

/// An `apiKey` value that can be sent as is: not an environment reference (`$NAME`) or a
/// command (`!cmd`), which pi resolves itself.
pub fn literal_key(api_key: &str) -> Option<&str> {
    let key = api_key.trim();
    (!key.is_empty() && !key.starts_with('$') && !key.starts_with('!')).then_some(key)
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
        { "id": "qwen3.8-27b", "name": "Qwen", "baseUrl": "http://192.168.1.50:8890/v1",
          "reasoning": true, "thinkingLevelMap": { "high": "xhigh" },
          "input": ["text", "image"], "contextWindow": 262144 }
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

    fn ids(file: &ModelsFile, provider: usize) -> Vec<String> {
        file.providers()[provider]
            .models
            .iter()
            .map(|model| model.spec.id.clone())
            .collect()
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
                api_key: Some("secret".into()),
                models: vec![SavedModel {
                    spec: ModelSpec {
                        id: "qwen3.8-27b".into(),
                        name: "Qwen".into(),
                        context_window: Some(262_144),
                        max_tokens: None,
                        reasoning: true,
                        images: true,
                    },
                    base_url: Some("http://192.168.1.50:8890/v1".into()),
                }],
            }]
        );
        assert_eq!(file.providers()[0].label(), "spark");
        assert_eq!(file.providers()[0].built_in(), None);
    }

    #[test]
    fn adds_models_keeping_everything_else() {
        let mut file = ModelsFile::parse(EXISTING).unwrap();
        file.set_model(
            "spark",
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
        assert_eq!(ids(&file, 0), ["qwen3.8-27b", "inkling-2-mini"]);
        let text = file.to_text();
        // Untouched keys stay, in their order.
        let places: Vec<_> = ["baseUrl", "compat", "thinkingLevelMap", "inkling"]
            .iter()
            .map(|key| text.find(key).unwrap())
            .collect();
        assert!(places.is_sorted(), "{text}");
        assert!(text.contains(r#""thinkingFormat": "openai""#));
        assert!(text.contains(r#""contextWindow": 131072"#));
        assert!(!text.contains("maxTokens"));
        assert!(text.ends_with("}\n"));
        assert!(file.set_model("nope", &model("x")).is_err());
        assert!(file.set_model("spark", &model(" ")).is_err());
    }

    #[test]
    fn changes_models_keeping_what_the_form_does_not_set() {
        let mut file = ModelsFile::parse(EXISTING).unwrap();
        let mut spec = file.providers()[0].models[0].spec.clone();
        // Nothing changed: nothing changes.
        file.set_model("spark", &spec).unwrap();
        assert_eq!(file, ModelsFile::parse(EXISTING).unwrap());

        spec.name = "Qwen 3.8".into();
        spec.context_window = None;
        spec.reasoning = false;
        spec.images = false;
        file.set_model("spark", &spec).unwrap();
        let text = file.to_text();
        assert!(text.contains(r#""high": "xhigh""#), "{text}");
        assert!(text.contains("8890"), "{text}");
        assert!(!text.contains("contextWindow") && !text.contains("reasoning"));
        assert!(!text.contains("\"input\""));
        assert_eq!(file.providers()[0].models[0].spec, spec);
        file.remove_model("spark", "qwen3.8-27b");
        assert_eq!(ids(&file, 0), [""; 0]);
        file.remove_provider("spark");
        assert_eq!(file.providers(), Vec::new());
    }

    #[test]
    fn creates_providers_and_files() {
        let mut file = ModelsFile::parse("").unwrap();
        let spec = ProviderSpec {
            name: "Ollama".into(),
            base_url: "http://localhost:11434/v1/".into(),
            api: "openai-completions".into(),
            api_key: "ollama".into(),
        };
        file.add_provider("ollama", &spec).unwrap();
        file.set_model("ollama", &model("qwen2.5-coder:7b"))
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
        assert_eq!(ids(&file, 0), ["qwen2.5-coder:7b"]);
        // Creating it again is refused.
        assert!(file.add_provider("ollama", &spec).is_err());
        file.set_provider(
            "ollama",
            &ProviderSpec {
                name: String::new(),
                ..spec
            },
        )
        .unwrap();
        assert_eq!(file.providers()[0].label(), "ollama");
        assert_eq!(ids(&file, 0), ["qwen2.5-coder:7b"]);
    }

    #[test]
    fn built_in_providers_need_only_a_key() {
        let mut file = ModelsFile::parse(EXISTING).unwrap();
        file.set_key("openrouter", "sk-1").unwrap();
        let provider = file.providers().pop().unwrap();
        assert_eq!(provider.label(), "OpenRouter");
        assert_eq!(provider.built_in().unwrap().id, "openrouter");
        assert!(
            file.to_text()
                .contains("\"openrouter\": {\n      \"apiKey\": \"sk-1\"\n    }")
        );
        file.set_key("openrouter", "").unwrap();
        assert_eq!(file, ModelsFile::parse(EXISTING).unwrap());
        // What else is there stays.
        file.set_key("spark", "").unwrap();
        assert_eq!(file.providers()[0].api_key, None);
        assert_eq!(ids(&file, 0), ["qwen3.8-27b"]);
    }

    #[test]
    fn rejects_bad_input() {
        let mut file = ModelsFile::default();
        let spec = ProviderSpec {
            base_url: "localhost:1234".into(),
            ..ProviderSpec::default()
        };
        let error = file.add_provider("p", &spec).unwrap_err();
        assert!(error.to_string().contains("http://"), "{error}");
        assert_eq!(file.providers(), Vec::new(), "not half added");
        assert!(file.add_provider("p q", &spec).is_err());
        assert!(file.add_provider("p/q", &spec).is_err());
        assert!(file.add_provider("p", &ProviderSpec::default()).is_err());
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
    fn keys() {
        assert_eq!(literal_key(" sk-1 "), Some("sk-1"));
        assert_eq!(literal_key("$OPENAI_API_KEY"), None);
        assert_eq!(literal_key("!pass show x"), None);
        assert_eq!(literal_key(""), None);
    }
}
