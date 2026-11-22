//! The startup model and thinking level in pi's agent `settings.json` (`defaultProvider`,
//! `defaultModel`, `defaultThinkingLevel`), the same keys pi's terminal `/settings` edits.
//! Every other key in the file is kept as it is.

use serde_json::{Map, Value};

/// pi's thinking levels, in order.
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// What pi starts with; `None` lets pi decide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupDefaults {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings.json is not valid JSON: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("settings.json is not a JSON object")]
    NotAnObject,
}

/// Reads the startup defaults from `settings.json` text (empty text: nothing set).
pub fn startup_defaults(text: &str) -> Result<StartupDefaults, SettingsError> {
    let object = parse(text)?;
    let get = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Ok(StartupDefaults {
        provider: get("defaultProvider"),
        model: get("defaultModel"),
        thinking_level: get("defaultThinkingLevel"),
    })
}

/// `text` with the startup defaults replaced by `defaults` (unset ones removed), keeping every
/// other key and its order.
pub fn with_startup_defaults(
    text: &str,
    defaults: &StartupDefaults,
) -> Result<String, SettingsError> {
    let mut object = parse(text)?;
    for (key, value) in [
        ("defaultProvider", &defaults.provider),
        ("defaultModel", &defaults.model),
        ("defaultThinkingLevel", &defaults.thinking_level),
    ] {
        match value.as_deref().filter(|value| !value.is_empty()) {
            Some(value) => {
                object.insert(key.to_owned(), Value::String(value.to_owned()));
            }
            None => {
                object.remove(key);
            }
        }
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(object))?;
    out.push('\n');
    Ok(out)
}

fn parse(text: &str) -> Result<Map<String, Value>, SettingsError> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text)? {
        Value::Object(object) => Ok(object),
        _ => Err(SettingsError::NotAnObject),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn reads_and_writes_only_the_startup_keys() {
        let text = r#"{
  "theme": "dark",
  "defaultProvider": "anthropic",
  "defaultModel": "claude",
  "packages": ["a"]
}"#;
        let defaults = startup_defaults(text).unwrap();
        assert_eq!(defaults.provider.as_deref(), Some("anthropic"));
        assert_eq!(defaults.thinking_level, None);

        let changed = with_startup_defaults(
            text,
            &StartupDefaults {
                provider: Some("ollama".into()),
                model: Some("qwen".into()),
                thinking_level: Some("high".into()),
            },
        )
        .unwrap();
        // Other keys keep their values and order; the startup keys are updated in place.
        assert_eq!(
            changed,
            "{\n  \"theme\": \"dark\",\n  \"defaultProvider\": \"ollama\",\n  \"defaultModel\": \
             \"qwen\",\n  \"packages\": [\n    \"a\"\n  ],\n  \"defaultThinkingLevel\": \
             \"high\"\n}\n"
        );
    }

    #[test]
    fn unset_values_are_removed_and_empty_files_work() {
        let cleared = with_startup_defaults(
            r#"{"defaultModel": "x", "other": 1}"#,
            &StartupDefaults::default(),
        )
        .unwrap();
        assert_eq!(cleared, "{\n  \"other\": 1\n}\n");
        assert_eq!(startup_defaults("").unwrap(), StartupDefaults::default());
        assert!(matches!(
            startup_defaults("[1]"),
            Err(SettingsError::NotAnObject)
        ));
    }
}
