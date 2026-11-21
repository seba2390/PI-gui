//! Commands written to pi's stdin.

use serde::Serialize;

use super::message::ImageContent;
use super::model::{QueueMode, ThinkingLevel};

/// How a prompt sent during a run is queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingBehavior {
    /// Delivered after the current turn's tool calls, before the next model call.
    Steer,
    /// Delivered only when the agent would otherwise stop.
    FollowUp,
}

/// Every stdin command of pi's RPC protocol (`RpcCommand` in `rpc-types.ts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    Prompt {
        message: String,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            serialize_with = "serialize_images"
        )]
        images: Vec<ImageContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        streaming_behavior: Option<StreamingBehavior>,
    },
    Steer {
        message: String,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            serialize_with = "serialize_images"
        )]
        images: Vec<ImageContent>,
    },
    FollowUp {
        message: String,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            serialize_with = "serialize_images"
        )]
        images: Vec<ImageContent>,
    },
    Abort,
    ClearQueue,
    NewSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_session: Option<String>,
    },
    GetState,
    GetMessages,
    SetModel {
        provider: String,
        model_id: String,
    },
    CycleModel,
    GetAvailableModels,
    SetThinkingLevel {
        level: ThinkingLevel,
    },
    CycleThinkingLevel,
    GetAvailableThinkingLevels,
    SetSteeringMode {
        mode: QueueMode,
    },
    SetFollowUpMode {
        mode: QueueMode,
    },
    Compact {
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    SetAutoCompaction {
        enabled: bool,
    },
    SetAutoRetry {
        enabled: bool,
    },
    AbortRetry,
    Bash {
        command: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        exclude_from_context: bool,
    },
    AbortBash,
    GetSessionStats,
    ExportHtml {
        #[serde(skip_serializing_if = "Option::is_none")]
        output_path: Option<String>,
    },
    SwitchSession {
        session_path: String,
    },
    Fork {
        entry_id: String,
    },
    Clone,
    GetForkMessages,
    GetEntries {
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    GetTree,
    GetLastAssistantText,
    SetSessionName {
        name: String,
    },
    GetCommands,
}

impl Command {
    /// The `type` string pi echoes back as `response.command`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Steer { .. } => "steer",
            Self::FollowUp { .. } => "follow_up",
            Self::Abort => "abort",
            Self::ClearQueue => "clear_queue",
            Self::NewSession { .. } => "new_session",
            Self::GetState => "get_state",
            Self::GetMessages => "get_messages",
            Self::SetModel { .. } => "set_model",
            Self::CycleModel => "cycle_model",
            Self::GetAvailableModels => "get_available_models",
            Self::SetThinkingLevel { .. } => "set_thinking_level",
            Self::CycleThinkingLevel => "cycle_thinking_level",
            Self::GetAvailableThinkingLevels => "get_available_thinking_levels",
            Self::SetSteeringMode { .. } => "set_steering_mode",
            Self::SetFollowUpMode { .. } => "set_follow_up_mode",
            Self::Compact { .. } => "compact",
            Self::SetAutoCompaction { .. } => "set_auto_compaction",
            Self::SetAutoRetry { .. } => "set_auto_retry",
            Self::AbortRetry => "abort_retry",
            Self::Bash { .. } => "bash",
            Self::AbortBash => "abort_bash",
            Self::GetSessionStats => "get_session_stats",
            Self::ExportHtml { .. } => "export_html",
            Self::SwitchSession { .. } => "switch_session",
            Self::Fork { .. } => "fork",
            Self::Clone => "clone",
            Self::GetForkMessages => "get_fork_messages",
            Self::GetEntries { .. } => "get_entries",
            Self::GetTree => "get_tree",
            Self::GetLastAssistantText => "get_last_assistant_text",
            Self::SetSessionName { .. } => "set_session_name",
            Self::GetCommands => "get_commands",
        }
    }
}

/// Serializes images as pi's `ImageContent` (`{"type":"image",...}`).
fn serialize_images<S: serde::Serializer>(
    images: &[ImageContent],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Tagged<'a> {
        r#type: &'static str,
        data: &'a str,
        mime_type: &'a str,
    }
    serializer.collect_seq(images.iter().map(|image| Tagged {
        r#type: "image",
        data: &image.data,
        mime_type: &image.mime_type,
    }))
}

/// A command plus its correlation id, as written to stdin.
#[derive(Debug, Clone, Serialize)]
pub struct CommandEnvelope<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'a str>,
    #[serde(flatten)]
    pub command: &'a Command,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};

    fn encode(id: Option<&str>, command: &Command) -> Value {
        serde_json::to_value(CommandEnvelope { id, command }).unwrap()
    }

    #[test]
    fn prompt_encodes_like_pi_expects() {
        let command = Command::Prompt {
            message: "Hello".into(),
            images: vec![ImageContent::new("AA==", "image/png")],
            streaming_behavior: Some(StreamingBehavior::FollowUp),
        };
        assert_eq!(
            encode(Some("r1"), &command),
            json!({
                "id":"r1","type":"prompt","message":"Hello",
                "images":[{"type":"image","data":"AA==","mimeType":"image/png"}],
                "streamingBehavior":"followUp"
            })
        );
    }

    #[test]
    fn optional_fields_are_omitted() {
        let prompt = Command::Prompt {
            message: "x".into(),
            images: vec![],
            streaming_behavior: None,
        };
        assert_eq!(
            encode(None, &prompt),
            json!({"type":"prompt","message":"x"})
        );
        assert_eq!(
            encode(
                None,
                &Command::Bash {
                    command: "ls".into(),
                    exclude_from_context: false
                }
            ),
            json!({"type":"bash","command":"ls"})
        );
    }

    #[test]
    fn names_match_serialized_type() {
        let commands = [
            Command::GetState,
            Command::FollowUp {
                message: String::new(),
                images: vec![],
            },
            Command::SetModel {
                provider: "p".into(),
                model_id: "m".into(),
            },
            Command::SetThinkingLevel {
                level: ThinkingLevel::High,
            },
            Command::GetAvailableThinkingLevels,
            Command::SwitchSession {
                session_path: "/x".into(),
            },
            Command::Clone,
            Command::GetCommands,
        ];
        for command in commands {
            let value = encode(None, &command);
            assert_eq!(value["type"], json!(command.name()), "{command:?}");
        }
        assert_eq!(
            encode(
                None,
                &Command::SetModel {
                    provider: "p".into(),
                    model_id: "m".into()
                }
            ),
            json!({"type":"set_model","provider":"p","modelId":"m"})
        );
    }
}
