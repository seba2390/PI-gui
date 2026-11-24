//! Agent messages and content blocks (pi's `AgentMessage` union).
//!
//! Every type is deliberately tolerant: unknown roles, block types and stop reasons
//! deserialize into `Unknown` variants, and optional or provider-specific fields
//! default instead of failing, so a newer pi never breaks the GUI.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::ThinkingLevel;

/// One message in a session's conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum AgentMessage {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BashExecution(BashExecutionMessage),
    Custom(CustomMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
    /// A role this client does not know (extensions can add roles).
    #[serde(other)]
    Unknown,
}

/// System prompt / tool loadout patch. The GUI never renders its content.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: MessageContent,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

/// `string | (TextContent | ImageContent)[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(ContentBlock::as_text)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Number of image blocks.
    pub fn image_count(&self) -> usize {
        match self {
            Self::Text(_) => 0,
            Self::Blocks(blocks) => blocks
                .iter()
                .filter(|block| matches!(block, ContentBlock::Image(_)))
                .count(),
        }
    }
}

/// A content block inside a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
    Image(ImageContent),
    #[serde(other)]
    Unknown,
}

impl ContentBlock {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingContent {
    #[serde(default)]
    pub thinking: String,
    #[serde(default)]
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Base64 image. Used both in messages and as prompt attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

impl ImageContent {
    /// Builds the `{"type":"image",...}` value pi expects in `prompt.images`.
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub stop_reason: StopReason,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,
    /// When the response started (Unix ms).
    #[serde(default)]
    pub timestamp: Option<i64>,
    /// How long the model call took.
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    #[default]
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub details: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    /// The file with the whole output, when `output` is only the end of it.
    #[serde(default)]
    pub full_output_path: Option<String>,
    #[serde(default)]
    pub exclude_from_context: bool,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    #[serde(default)]
    pub custom_type: String,
    pub content: MessageContent,
    #[serde(default)]
    pub display: bool,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    #[serde(default)]
    pub tokens_before: u64,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

/// Token usage reported for one assistant response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: UsageCost,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageCost {
    pub total: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn user_message_accepts_string_and_block_content() {
        let plain: AgentMessage =
            serde_json::from_value(json!({"role":"user","content":"hi","timestamp":1})).unwrap();
        let blocks: AgentMessage = serde_json::from_value(json!({
            "role":"user",
            "content":[{"type":"text","text":"look"},{"type":"image","data":"AA==","mimeType":"image/png"}],
            "timestamp":1
        }))
        .unwrap();

        let AgentMessage::User(plain) = plain else {
            panic!("expected user")
        };
        let AgentMessage::User(blocks) = blocks else {
            panic!("expected user")
        };
        assert_eq!(plain.content.text(), "hi");
        assert_eq!(blocks.content.text(), "look");
        assert_eq!(blocks.content.image_count(), 1);
    }

    #[test]
    fn unknown_roles_blocks_and_stop_reasons_are_tolerated() {
        let message: AgentMessage =
            serde_json::from_value(json!({"role":"futureRole","whatever":1})).unwrap();
        assert_eq!(message, AgentMessage::Unknown);

        let assistant: AgentMessage = serde_json::from_value(json!({
            "role":"assistant",
            "content":[{"type":"hologram"},{"type":"thinking","thinking":"hmm","thinkingSignature":"x"}],
            "stopReason":"teleported",
            "usage":{"input":1,"output":2,"totalTokens":3,"cost":{"total":0}}
        }))
        .unwrap();
        let AgentMessage::Assistant(assistant) = assistant else {
            panic!("expected assistant")
        };
        assert_eq!(assistant.stop_reason, StopReason::Unknown);
        assert_eq!(assistant.content[0], ContentBlock::Unknown);
        assert_eq!(assistant.usage.total_tokens, 3);
    }

    #[test]
    fn tool_result_keeps_details() {
        let message: AgentMessage = serde_json::from_value(json!({
            "role":"toolResult","toolCallId":"c1","toolName":"edit",
            "content":[{"type":"text","text":"ok"}],"details":{"diff":"+a"},"isError":false
        }))
        .unwrap();
        let AgentMessage::ToolResult(result) = message else {
            panic!("expected tool result")
        };
        assert_eq!(result.details, Some(json!({"diff":"+a"})));
    }
}
