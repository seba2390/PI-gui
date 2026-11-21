//! Session events streamed on stdout (pi's `JsonAgentSessionEvent`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message::{AgentMessage, ToolCall, Usage};
use super::model::ThinkingLevel;

/// Why compaction ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

/// A session event. Unknown event types deserialize to [`Event::Unknown`].
///
/// Message and tool events may carry `parent_tool_call_id` when they belong to a
/// tool call nested inside another tool (for example code mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    AgentStart,
    AgentEnd {
        #[serde(default)]
        messages: Vec<AgentMessage>,
        #[serde(default)]
        will_retry: bool,
    },
    AgentSettled {
        #[serde(default)]
        aborted: bool,
    },
    TurnStart,
    TurnEnd {
        message: Option<AgentMessage>,
        #[serde(default)]
        tool_results: Vec<AgentMessage>,
    },
    MessageStart {
        message: AgentMessage,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    MessageUpdate {
        #[serde(default)]
        usage: Option<Usage>,
        assistant_message_event: AssistantMessageEvent,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    MessageEnd {
        message: AgentMessage,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        partial_result: Value,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        result: Value,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        parent_tool_call_id: Option<String>,
    },
    QueueUpdate {
        #[serde(default)]
        steering: Vec<String>,
        #[serde(default)]
        follow_up: Vec<String>,
    },
    EntryAppended {
        entry: Value,
    },
    SessionInfoChanged {
        #[serde(default)]
        name: Option<String>,
    },
    ThinkingLevelChanged {
        level: ThinkingLevel,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    CompactionEnd {
        reason: CompactionReason,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        aborted: bool,
        #[serde(default)]
        will_retry: bool,
        #[serde(default)]
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        #[serde(default)]
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u32,
        #[serde(default)]
        final_error: Option<String>,
    },
    SummarizationRetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        #[serde(default)]
        error_message: String,
    },
    SummarizationRetryAttemptStart {
        #[serde(default)]
        source: String,
    },
    SummarizationRetryFinished,
    BashExecutionUpdate {
        #[serde(default)]
        id: Option<String>,
        delta: String,
    },
    ExtensionError {
        #[serde(default)]
        extension_path: String,
        #[serde(default)]
        event: String,
        #[serde(default)]
        error: String,
    },
    #[serde(other)]
    Unknown,
}

/// Streaming update nested in `message_update`. Delta-only on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AssistantMessageEvent {
    Start,
    TextStart {
        content_index: usize,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
        #[serde(default)]
        content: String,
    },
    ThinkingStart {
        content_index: usize,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        #[serde(default)]
        content: String,
    },
    ToolcallStart {
        content_index: usize,
        #[serde(default)]
        id: String,
        #[serde(default)]
        tool_name: String,
    },
    ToolcallDelta {
        content_index: usize,
        delta: String,
    },
    ToolcallEnd {
        content_index: usize,
        tool_call: ToolCall,
    },
    Done {
        #[serde(default)]
        reason: String,
    },
    Error {
        #[serde(default)]
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn decodes_streaming_deltas() {
        let event: Event = serde_json::from_value(json!({
            "type":"message_update",
            "usage":{"input":100,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":101,"cost":{"total":0}},
            "assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"Hm"}
        }))
        .unwrap();
        let Event::MessageUpdate {
            usage,
            assistant_message_event,
            ..
        } = event
        else {
            panic!("expected message_update")
        };
        assert_eq!(usage.unwrap().total_tokens, 101);
        assert_eq!(
            assistant_message_event,
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "Hm".into()
            }
        );
    }

    #[test]
    fn decodes_tool_lifecycle() {
        let end: Event = serde_json::from_value(json!({
            "type":"tool_execution_end","toolCallId":"c1","toolName":"bash",
            "result":{"content":[{"type":"text","text":"ok"}]},"isError":false,"durationMs":12
        }))
        .unwrap();
        assert!(matches!(
            end,
            Event::ToolExecutionEnd {
                duration_ms: Some(12),
                ..
            }
        ));

        let start: Event = serde_json::from_value(json!({
            "type":"message_update",
            "assistantMessageEvent":{"type":"toolcall_start","contentIndex":2,"id":"c1","toolName":"read"}
        }))
        .unwrap();
        assert!(matches!(
            start,
            Event::MessageUpdate {
                assistant_message_event: AssistantMessageEvent::ToolcallStart {
                    content_index: 2,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn unknown_events_do_not_fail() {
        let event: Event = serde_json::from_value(json!({"type":"brand_new","x":1})).unwrap();
        assert_eq!(event, Event::Unknown);
        let settled: Event =
            serde_json::from_value(json!({"type":"agent_settled","aborted":true})).unwrap();
        assert_eq!(settled, Event::AgentSettled { aborted: true });
    }
}
