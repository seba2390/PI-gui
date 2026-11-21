//! Serde mirror of pi's RPC protocol records.

mod command;
mod event;
mod extension_ui;
mod message;
mod model;

use serde::Deserialize;
use serde_json::Value;

pub use command::{Command, CommandEnvelope, StreamingBehavior};
pub use event::{AssistantMessageEvent, CompactionReason, Event};
pub(crate) use extension_ui::ExtensionUiResponse;
pub use extension_ui::{
    ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest, NotifyKind, WidgetPlacement,
};
pub use message::{
    AgentMessage, AssistantMessage, BashExecutionMessage, BranchSummaryMessage,
    CompactionSummaryMessage, ContentBlock, CustomMessage, ImageContent, MessageContent,
    StopReason, SystemMessage, TextContent, ThinkingContent, ToolCall, ToolResultMessage, Usage,
    UsageCost, UserMessage,
};
pub use model::{
    BashResult, ContextUsage, Disposition, ForkMessage, Model, QueueMode, SessionState,
    SessionStats, SlashCommand, ThinkingLevel, TokenTotals,
};

/// The reply to one command.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Option<String>,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Any record pi writes to stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response(Response),
    ExtensionUi(ExtensionUiRequest),
    Event(Box<Event>),
}

impl Incoming {
    /// Decodes one JSONL record, dispatching on its `type` field.
    pub fn from_slice(line: &[u8]) -> serde_json::Result<Self> {
        #[derive(Deserialize)]
        struct Tag<'a> {
            #[serde(borrow, rename = "type")]
            kind: std::borrow::Cow<'a, str>,
        }
        let tag: Tag<'_> = serde_json::from_slice(line)?;
        Ok(match tag.kind.as_ref() {
            "response" => Self::Response(serde_json::from_slice(line)?),
            "extension_ui_request" => Self::ExtensionUi(serde_json::from_slice(line)?),
            _ => Self::Event(Box::new(serde_json::from_slice(line)?)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_on_record_type() {
        let response =
            br#"{"id":"r1","type":"response","command":"get_state","success":true,"data":{}}"#;
        let failure = br#"{"type":"response","command":"parse","success":false,"error":"bad"}"#;
        let ui = br#"{"type":"extension_ui_request","id":"u","method":"notify","message":"hi"}"#;
        let event = br#"{"type":"agent_start"}"#;

        assert!(
            matches!(Incoming::from_slice(response).unwrap(), Incoming::Response(r) if r.success)
        );
        assert!(matches!(
            Incoming::from_slice(failure).unwrap(),
            Incoming::Response(Response {
                success: false,
                error: Some(_),
                ..
            })
        ));
        assert!(matches!(
            Incoming::from_slice(ui).unwrap(),
            Incoming::ExtensionUi(_)
        ));
        assert!(
            matches!(Incoming::from_slice(event).unwrap(), Incoming::Event(e) if *e == Event::AgentStart)
        );
    }
}
