//! pi's project trust store, `<agent dir>/trust.json`: `{"<canonical folder>": true | false}`.
//!
//! pi loads a folder's own settings and resources (project skills, prompts, extensions, …)
//! only when the folder is trusted. The closest saved decision for the folder or one of its
//! parents applies; without one, the `defaultProjectTrust` setting decides (default `"ask"`,
//! which pi's RPC mode treats as "no", since it cannot ask). The GUI reads and writes the same
//! file as pi's `/trust`, so both agree.

use serde_json::{Map, Value};

use crate::ConfigError;

/// Whether a folder is trusted and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// A saved decision for `path` (the folder itself or a parent).
    Saved { path: String, trusted: bool },
    /// No saved decision; `defaultProjectTrust` is `"always"`.
    DefaultAlways,
    /// No saved decision; pi skips the project's resources (`"ask"` or `"never"`).
    NotDecided,
}

impl Trust {
    pub fn is_trusted(&self) -> bool {
        matches!(
            self,
            Self::Saved { trusted: true, .. } | Self::DefaultAlways
        )
    }
}

/// Parses `trust.json` (blank = empty).
pub fn parse_store(text: &str) -> Result<Map<String, Value>, ConfigError> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(ConfigError::Invalid("trust.json must be an object".into())),
        Err(error) => Err(ConfigError::Invalid(format!(
            "trust.json is not valid JSON: {error}"
        ))),
    }
}

/// The trust of `folder` (a canonical absolute path): the closest saved decision, else
/// `default_project_trust` (pi's setting; `None` means its default, `"ask"`).
pub fn trust_of(
    store: &Map<String, Value>,
    folder: &str,
    default_project_trust: Option<&str>,
) -> Trust {
    let mut current = folder.trim_end_matches('/').to_owned();
    if current.is_empty() {
        current.push('/');
    }
    loop {
        if let Some(Value::Bool(trusted)) = store.get(&current) {
            return Trust::Saved {
                path: current,
                trusted: *trusted,
            };
        }
        match current.rfind('/') {
            Some(0) if current != "/" => "/".clone_into(&mut current),
            Some(index) if index > 0 => current.truncate(index),
            _ => break,
        }
    }
    if default_project_trust == Some("always") {
        Trust::DefaultAlways
    } else {
        Trust::NotDecided
    }
}

/// `trust.json`'s new text with `folder` set to `decision` (`None` removes it), written the way
/// pi writes it: keys sorted, two-space indent, trailing newline.
pub fn set_trust(store: &Map<String, Value>, folder: &str, decision: Option<bool>) -> String {
    let mut map = store.clone();
    map.remove(folder);
    if let Some(decision) = decision {
        map.insert(folder.to_owned(), Value::Bool(decision));
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut sorted = Map::new();
    for key in keys {
        if let Some(value @ (Value::Bool(_) | Value::Null)) = map.get(key) {
            sorted.insert(key.clone(), value.clone());
        }
    }
    let mut text =
        serde_json::to_string_pretty(&Value::Object(sorted)).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

/// `defaultProjectTrust` from pi's agent `settings.json` text, if set.
pub fn default_project_trust(settings_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(settings_json)
        .ok()?
        .get("defaultProjectTrust")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn closest_decision_wins() {
        let store =
            parse_store(r#"{"/home/me": true, "/home/me/untrusted": false, "/x": null}"#).unwrap();
        assert_eq!(
            trust_of(&store, "/home/me/code/app", None),
            Trust::Saved {
                path: "/home/me".into(),
                trusted: true
            }
        );
        assert!(!trust_of(&store, "/home/me/untrusted/sub", None).is_trusted());
        assert_eq!(trust_of(&store, "/x/y", None), Trust::NotDecided);
        assert_eq!(
            trust_of(&store, "/srv", Some("always")),
            Trust::DefaultAlways
        );
        assert_eq!(trust_of(&store, "/srv", Some("never")), Trust::NotDecided);
        let root = parse_store(r#"{"/": true}"#).unwrap();
        assert!(trust_of(&root, "/a/b", None).is_trusted());
    }

    #[test]
    fn writes_sorted_like_pi() {
        let store = parse_store(r#"{"/b": true, "/a": false}"#).unwrap();
        assert_eq!(
            set_trust(&store, "/c", Some(true)),
            "{\n  \"/a\": false,\n  \"/b\": true,\n  \"/c\": true\n}\n"
        );
        assert_eq!(set_trust(&store, "/b", None), "{\n  \"/a\": false\n}\n");
        assert_eq!(
            set_trust(&Map::new(), "/x", Some(false)),
            "{\n  \"/x\": false\n}\n"
        );
        assert!(parse_store("").unwrap().is_empty());
        assert!(parse_store("[]").is_err());
        assert_eq!(
            default_project_trust(r#"{"defaultProjectTrust": "always"}"#).as_deref(),
            Some("always")
        );
        assert_eq!(default_project_trust("{}"), None);
    }
}
