//! Models, thinking levels, session state and statistics returned by commands.

use std::fmt;

use serde::{Deserialize, Serialize};

/// pi's thinking level. `Xhigh` and `Max` exist only on models that support them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// The wire name, identical to what the pi TUI shows.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A configured model, as returned by `get_available_models` / `get_state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_tokens: u64,
}

impl Model {
    /// `provider/id`, the form pi's `--model` flag and TUI use.
    pub fn qualified_id(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    /// Human label: the configured name, falling back to the id.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub fn supports_images(&self) -> bool {
        self.input.iter().any(|modality| modality == "image")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueMode {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "one-at-a-time")]
    OneAtATime,
}

/// `get_state` response data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    #[serde(default)]
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    #[serde(default)]
    pub is_streaming: bool,
    #[serde(default)]
    pub is_compacting: bool,
    #[serde(default)]
    pub session_file: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub auto_compaction_enabled: bool,
    #[serde(default)]
    pub message_count: u64,
    #[serde(default)]
    pub pending_message_count: u64,
}

/// `get_session_stats` response data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStats {
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub total_messages: u64,
    pub tokens: TokenTotals,
    pub cost: f64,
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// Current context-window estimate. `tokens`/`percent` are `None` right after a
/// compaction, until a fresh assistant response reports usage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
}

/// An extension command, prompt template or skill from `get_commands`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: String,
    /// Where it was loaded from (e.g. a skill's `SKILL.md`), when pi reports it.
    #[serde(default)]
    pub source_info: Option<SourceInfo>,
}

/// Where a command, prompt template or skill was loaded from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SourceInfo {
    /// The file (e.g. `…/skills/pdf-tools/SKILL.md`).
    pub path: String,
    /// `user`, `project`, or another scope pi defines.
    pub scope: String,
    /// `auto`, `settings`, a package, …
    pub source: String,
    /// `top-level` or `package`.
    pub origin: String,
    pub base_dir: Option<String>,
}

/// `clear_queue` response data: the texts of the messages that were queued, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClearedQueue {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

/// `bash` response data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BashResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

/// One entry of `get_fork_messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkMessage {
    pub entry_id: String,
    pub text: String,
}

/// What pi did with a submitted prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    Started,
    Queued,
    Handled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_levels_round_trip_with_wire_names() {
        for level in ThinkingLevel::ALL {
            let encoded = serde_json::to_value(level).unwrap();
            assert_eq!(encoded, json!(level.as_str()));
            assert_eq!(
                serde_json::from_value::<ThinkingLevel>(encoded).unwrap(),
                level
            );
        }
    }

    #[test]
    fn stats_tolerate_null_context_after_compaction() {
        let stats: SessionStats = serde_json::from_value(json!({
            "sessionId":"s","contextUsage":{"tokens":null,"contextWindow":1000,"percent":null}
        }))
        .unwrap();
        let usage = stats.context_usage.unwrap();
        assert_eq!(usage.tokens, None);
        assert_eq!(usage.context_window, 1000);
    }
}
