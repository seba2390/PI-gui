//! The tools pi offers a model, as it describes them to the model (Settings → Tools).
//!
//! pi has no command that lists its tools, so a small extension ([`PROBE_SOURCE`]) loaded into
//! a pi that keeps no session reports them through a status line ([`STATUS_KEY`]), which
//! [`parse`] reads.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The probe extension's source.
pub const PROBE_SOURCE: &str = include_str!("tools_probe.ts");
/// The status the probe reports the tools in.
pub const STATUS_KEY: &str = "pistachio-tools";
/// Where built-in tools come from (pi's `sourceInfo.path` prefix).
const BUILTIN_PREFIX: &str = "builtin:";

/// A tool pi has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    /// What the model is told it does.
    pub description: String,
    /// Lines pi adds to the system prompt while the tool is active.
    pub guidelines: Vec<String>,
    /// The parameters' JSON schema, as the model gets it.
    pub schema: String,
    pub parameters: Vec<Parameter>,
    /// The extension file it comes from; `None` for pi's own.
    pub extension: Option<PathBuf>,
    /// Whether pi offers it to the model when a conversation starts.
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub required: bool,
    /// The JSON schema type (`string`, `integer`, …); empty for a composite one.
    pub kind: String,
    pub description: String,
}

/// Characters per token, roughly, for English text and JSON with common tokenizers.
pub const CHARS_PER_TOKEN: usize = 4;

impl Tool {
    /// The characters the tool puts in front of the model with every request: its name,
    /// description and parameter schema, and its guidelines in the system prompt.
    pub fn context_chars(&self) -> usize {
        let chars = |text: &str| text.chars().count();
        chars(&self.name)
            + chars(&self.description)
            + chars(&self.schema)
            + self
                .guidelines
                .iter()
                .map(|line| chars(line))
                .sum::<usize>()
    }

    /// [`Self::context_chars`] in tokens, roughly.
    pub fn context_tokens(&self) -> usize {
        self.context_chars().div_ceil(CHARS_PER_TOKEN)
    }
}

/// Writes the probe extension to `dir` (unless it is there already) and returns its path.
pub fn install_probe(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("tools-probe.ts");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(PROBE_SOURCE) {
        std::fs::write(&path, PROBE_SOURCE)?;
    }
    Ok(path)
}

/// The tools in the probe's status text, in pi's order; `None` if it can't be read.
pub fn parse(text: &str) -> Option<Vec<Tool>> {
    let tools: Value = serde_json::from_str(text).ok()?;
    let string = |value: &Value| value.as_str().map(str::to_owned);
    tools
        .as_array()?
        .iter()
        .map(|tool| {
            let path = tool["path"].as_str().unwrap_or_default();
            Some(Tool {
                name: string(&tool["name"])?,
                description: string(&tool["description"]).unwrap_or_default(),
                schema: string(&tool["schema"]).unwrap_or_default(),
                guidelines: tool["guidelines"]
                    .as_array()
                    .map_or_else(Vec::new, |lines| lines.iter().filter_map(string).collect()),
                parameters: tool["parameters"]
                    .as_array()
                    .map_or_else(Vec::new, |parameters| {
                        parameters
                            .iter()
                            .filter_map(|parameter| {
                                Some(Parameter {
                                    name: string(&parameter["name"])?,
                                    required: parameter["required"].as_bool().unwrap_or(false),
                                    kind: string(&parameter["type"]).unwrap_or_default(),
                                    description: string(&parameter["description"])
                                        .unwrap_or_default(),
                                })
                            })
                            .collect()
                    }),
                extension: (!path.is_empty() && !path.starts_with(BUILTIN_PREFIX))
                    .then(|| PathBuf::from(path)),
                active: tool["active"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn reads_what_the_probe_reports() {
        let text = r#"[
            {"name":"read","description":"Read a file.","guidelines":["Use read."],
             "schema":"{\"type\":\"object\"}",
             "parameters":[{"name":"path","required":true,"type":"string","description":"The file"},
                           {"name":"limit","required":false}],
             "path":"builtin:read","active":true},
            {"name":"python","description":"Run Python.","guidelines":[],"parameters":[],
             "path":"/x/python/extension.ts","active":false}
        ]"#;
        let tools = parse(text).expect("readable");
        assert_eq!(
            tools[0],
            Tool {
                name: "read".into(),
                description: "Read a file.".into(),
                guidelines: vec!["Use read.".into()],
                schema: r#"{"type":"object"}"#.into(),
                parameters: vec![
                    Parameter {
                        name: "path".into(),
                        required: true,
                        kind: "string".into(),
                        description: "The file".into(),
                    },
                    Parameter {
                        name: "limit".into(),
                        required: false,
                        kind: String::new(),
                        description: String::new(),
                    },
                ],
                extension: None,
                active: true,
            }
        );
        assert_eq!(
            tools[1].extension.as_deref(),
            Some(Path::new("/x/python/extension.ts"))
        );
        assert!(!tools[1].active);
    }

    #[test]
    fn what_a_tool_costs_in_context() {
        let tool = Tool {
            name: "read".into(),
            description: "Read a file.".into(),
            guidelines: vec!["Use read.".into()],
            schema: r#"{"type":"object"}"#.into(),
            parameters: Vec::new(),
            extension: None,
            active: true,
        };
        assert_eq!(tool.context_chars(), 4 + 12 + 17 + 9);
        assert_eq!(tool.context_tokens(), 11);
    }

    #[test]
    fn odd_reports_are_not_read() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"name":"read"}"#), None);
        assert_eq!(parse(r#"[{"description":"no name"}]"#), None);
        assert_eq!(parse("[]"), Some(Vec::new()));
    }

    #[test]
    fn the_probe_is_written_once() {
        let dir = std::env::temp_dir().join(format!("pi-config-probe-{}", std::process::id()));
        let path = install_probe(&dir).expect("written");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PROBE_SOURCE);
        assert_eq!(install_probe(&dir).unwrap(), path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
