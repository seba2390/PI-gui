//! What a run is doing that pi has not saved yet: the reply being written and the tools still
//! running. pi saves a message when it ends, so a side chat (`pi_sessions::side_chat`) takes
//! these from here to start with everything the conversation shows at that moment.
//!
//! Only the conversation's own messages and tools count; those of a tool that runs others
//! (`parent_tool_call_id`) are part of that tool's result.

use std::collections::BTreeMap;

use pi_rpc::protocol::{
    AgentMessage, AssistantMessageEvent, ContentBlock, Event, ThinkingLevel, ToolCall, Usage,
};
use serde_json::{Value, json};

/// Added to a reply that was cut off by the side chat, so the model (and you) can tell.
pub const CUT_OFF: &str =
    "[Cut off here: the side chat was opened while this reply was still being written.]";
/// The result of a tool that was still running.
pub const STILL_RUNNING: &str = "[The side chat was opened while this tool was still running, so its result is not known here.]";
/// The result of a tool call in a reply that was cut off.
pub const NOT_RUN: &str =
    "[The side chat was opened before this tool ran, so it has no result here.]";

/// The run's unsaved state; feed it every event ([`InFlight::apply`]).
#[derive(Debug, Default, Clone)]
pub struct InFlight {
    reply: Option<Reply>,
    /// Running tools, in the order they started.
    tools: Vec<RunningTool>,
}

#[derive(Debug, Clone)]
struct Reply {
    provider: String,
    model: String,
    thinking_level: Option<ThinkingLevel>,
    timestamp: Option<i64>,
    usage: Usage,
    blocks: BTreeMap<usize, Block>,
}

#[derive(Debug, Clone)]
enum Block {
    Text(String),
    Thinking(String),
    /// A tool call; `None` while its arguments are still being written.
    ToolCall(Option<ToolCall>),
}

#[derive(Debug, Clone)]
struct RunningTool {
    id: String,
    name: String,
    partial: Value,
}

impl InFlight {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::AgentStart | Event::AgentEnd { .. } | Event::AgentSettled { .. } => {
                *self = Self::default();
            }
            Event::MessageStart {
                message: AgentMessage::Assistant(message),
                parent_tool_call_id: None,
            } => {
                let blocks = message
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(i, block)| {
                        let block = match block {
                            ContentBlock::Text(text) => Block::Text(text.text.clone()),
                            ContentBlock::Thinking(thinking) => {
                                Block::Thinking(thinking.thinking.clone())
                            }
                            ContentBlock::ToolCall(call) => Block::ToolCall(Some(call.clone())),
                            _ => return None,
                        };
                        Some((i, block))
                    })
                    .collect();
                self.reply = Some(Reply {
                    provider: message.provider.clone(),
                    model: message.model.clone(),
                    thinking_level: message.thinking_level,
                    timestamp: message.timestamp,
                    usage: message.usage.clone(),
                    blocks,
                });
            }
            Event::MessageUpdate {
                usage,
                assistant_message_event,
                parent_tool_call_id: None,
            } => {
                if let Some(reply) = &mut self.reply {
                    if let Some(usage) = usage {
                        reply.usage = usage.clone();
                    }
                    reply.apply(assistant_message_event);
                }
            }
            Event::MessageEnd {
                message: AgentMessage::Assistant(_),
                parent_tool_call_id: None,
            } => self.reply = None,
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                parent_tool_call_id: None,
                ..
            } => self.tools.push(RunningTool {
                id: tool_call_id.clone(),
                name: tool_name.clone(),
                partial: Value::Null,
            }),
            Event::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                parent_tool_call_id: None,
                ..
            } => {
                if let Some(tool) = self.tools.iter_mut().find(|tool| tool.id == *tool_call_id) {
                    tool.partial = partial_result.clone();
                }
            }
            Event::ToolExecutionEnd {
                tool_call_id,
                parent_tool_call_id: None,
                ..
            } => self.tools.retain(|tool| tool.id != *tool_call_id),
            _ => {}
        }
    }

    /// Whether anything is unsaved.
    pub fn is_empty(&self) -> bool {
        self.reply.is_none() && self.tools.is_empty()
    }

    /// pi's messages for what is unsaved, in order: the reply so far (with [`CUT_OFF`], or
    /// with results for the tool calls it finished writing), then a result for each tool still
    /// running, with its output so far. `api` is the model's API (`Model::api`), which pi
    /// needs to replay the reply's thinking to the same model; `now_ms` dates the results.
    pub fn messages(&self, api: Option<&str>, now_ms: i64) -> Vec<Value> {
        let mut messages = Vec::new();
        if let Some(reply) = &self.reply
            && let Some(message) = reply.message(api)
        {
            for call in &reply.finished_calls() {
                messages.push(tool_result(&call.id, &call.name, NOT_RUN, now_ms));
            }
            messages.insert(0, message);
        }
        for tool in &self.tools {
            let output = output_text(&tool.partial);
            let text = if output.trim().is_empty() {
                STILL_RUNNING.to_owned()
            } else {
                format!("{STILL_RUNNING}\n\nOutput so far:\n{output}")
            };
            messages.push(tool_result(&tool.id, &tool.name, &text, now_ms));
        }
        messages
    }
}

impl Reply {
    fn apply(&mut self, event: &AssistantMessageEvent) {
        match event {
            AssistantMessageEvent::TextStart { content_index } => {
                self.blocks
                    .insert(*content_index, Block::Text(String::new()));
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                if let Block::Text(text) = self
                    .blocks
                    .entry(*content_index)
                    .or_insert_with(|| Block::Text(String::new()))
                {
                    text.push_str(delta);
                }
            }
            AssistantMessageEvent::TextEnd {
                content_index,
                content,
            } if !content.is_empty() => {
                self.blocks
                    .insert(*content_index, Block::Text(content.clone()));
            }
            AssistantMessageEvent::ThinkingStart { content_index } => {
                self.blocks
                    .insert(*content_index, Block::Thinking(String::new()));
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                if let Block::Thinking(thinking) = self
                    .blocks
                    .entry(*content_index)
                    .or_insert_with(|| Block::Thinking(String::new()))
                {
                    thinking.push_str(delta);
                }
            }
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
            } if !content.is_empty() => {
                self.blocks
                    .insert(*content_index, Block::Thinking(content.clone()));
            }
            AssistantMessageEvent::ToolcallStart { content_index, .. } => {
                self.blocks.insert(*content_index, Block::ToolCall(None));
            }
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                self.blocks
                    .insert(*content_index, Block::ToolCall(Some(tool_call.clone())));
            }
            _ => {}
        }
    }

    /// The tool calls whose arguments are complete.
    fn finished_calls(&self) -> Vec<ToolCall> {
        self.blocks
            .values()
            .filter_map(|block| match block {
                Block::ToolCall(Some(call)) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }

    /// The reply as pi's assistant message, or `None` while it has nothing in it yet.
    ///
    /// It is saved as a finished reply (`stop`, or `toolUse` with its finished calls): pi
    /// leaves cut-off replies (`aborted`) out of what the model sees, and the side chat should
    /// see everything the conversation showed. A tool call whose arguments were still being
    /// written is left out.
    fn message(&self, api: Option<&str>) -> Option<Value> {
        let mut content: Vec<Value> = Vec::new();
        for block in self.blocks.values() {
            match block {
                Block::Text(text) if !text.is_empty() => {
                    content.push(json!({"type": "text", "text": text}));
                }
                Block::Thinking(thinking) if !thinking.trim().is_empty() => {
                    content.push(json!({"type": "thinking", "thinking": thinking}));
                }
                Block::ToolCall(Some(call)) => content.push(json!({
                    "type": "toolCall",
                    "id": call.id,
                    "name": call.name,
                    "arguments": call.arguments,
                })),
                _ => {}
            }
        }
        if content.is_empty() {
            return None;
        }
        let calls = !self.finished_calls().is_empty();
        if !calls {
            content.push(json!({"type": "text", "text": CUT_OFF}));
        }
        let usage = &self.usage;
        let mut message = json!({
            "role": "assistant",
            "content": content,
            "provider": self.provider,
            "model": self.model,
            "usage": {
                "input": usage.input,
                "output": usage.output,
                "cacheRead": usage.cache_read,
                "cacheWrite": usage.cache_write,
                "totalTokens": usage.total_tokens,
                "cost": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "total": usage.cost.total,
                },
            },
            "stopReason": if calls { "toolUse" } else { "stop" },
            "timestamp": self.timestamp,
        });
        if let Some(api) = api {
            message["api"] = api.into();
        }
        if let Some(level) = self.thinking_level {
            message["thinkingLevel"] = serde_json::to_value(level).unwrap_or(Value::Null);
        }
        Some(message)
    }
}

fn tool_result(id: &str, name: &str, text: &str, now_ms: i64) -> Value {
    json!({
        "role": "toolResult",
        "toolCallId": id,
        "toolName": name,
        "content": [{"type": "text", "text": text}],
        "isError": false,
        "timestamp": now_ms,
    })
}

/// The text of a tool's partial result (`{content: [{type: "text", text}]}`).
fn output_text(partial: &Value) -> String {
    partial
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn event(value: Value) -> Event {
        serde_json::from_value(value).unwrap()
    }

    fn feed(in_flight: &mut InFlight, events: &[Value]) {
        for value in events {
            in_flight.apply(&event(value.clone()));
        }
    }

    fn start() -> Value {
        json!({"type": "message_start", "message": {
            "role": "assistant", "content": [], "provider": "anthropic",
            "model": "m", "timestamp": 7, "stopReason": "stop"
        }})
    }

    fn update(event: Value) -> Value {
        json!({"type": "message_update", "assistantMessageEvent": event})
    }

    #[test]
    fn a_reply_being_written_is_kept_and_marked_cut_off() {
        let mut in_flight = InFlight::default();
        feed(
            &mut in_flight,
            &[
                json!({"type": "agent_start"}),
                start(),
                update(json!({"type": "thinking_start", "contentIndex": 0})),
                update(json!({"type": "thinking_delta", "contentIndex": 0, "delta": "Hm"})),
                update(json!({"type": "thinking_end", "contentIndex": 0, "content": "Hmm"})),
                update(json!({"type": "text_start", "contentIndex": 1})),
                update(json!({"type": "text_delta", "contentIndex": 1, "delta": "The ans"})),
                // A call whose arguments are still coming is left out.
                update(
                    json!({"type": "toolcall_start", "contentIndex": 2, "id": "c", "toolName": "bash"}),
                ),
                json!({"type": "message_update",
                       "usage": {"input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                                 "totalTokens": 12, "cost": {"total": 0.5}},
                       "assistantMessageEvent": {"type": "toolcall_delta", "contentIndex": 2, "delta": "{"}}),
            ],
        );
        let messages = in_flight.messages(Some("anthropic-messages"), 99);
        assert_eq!(messages.len(), 1);
        let reply = &messages[0];
        assert_eq!(
            reply["content"],
            json!([
                {"type": "thinking", "thinking": "Hmm"},
                {"type": "text", "text": "The ans"},
                {"type": "text", "text": CUT_OFF},
            ])
        );
        assert_eq!(reply["stopReason"], "stop");
        assert_eq!(reply["api"], "anthropic-messages");
        assert_eq!(reply["timestamp"], 7);
        assert_eq!(reply["usage"]["totalTokens"], 12);
        assert_eq!(reply["usage"]["cost"]["total"], 0.5);
    }

    #[test]
    fn finished_calls_of_a_cut_off_reply_get_results() {
        let mut in_flight = InFlight::default();
        feed(
            &mut in_flight,
            &[
                start(),
                update(
                    json!({"type": "toolcall_start", "contentIndex": 0, "id": "c1", "toolName": "read"}),
                ),
                update(json!({"type": "toolcall_end", "contentIndex": 0,
                              "toolCall": {"id": "c1", "name": "read", "arguments": {"path": "a"}}})),
            ],
        );
        let messages = in_flight.messages(None, 99);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["stopReason"], "toolUse");
        assert_eq!(messages[0]["content"][0]["arguments"], json!({"path": "a"}));
        assert_eq!(messages[1]["role"], "toolResult");
        assert_eq!(messages[1]["toolCallId"], "c1");
        assert_eq!(messages[1]["content"][0]["text"], NOT_RUN);
    }

    #[test]
    fn running_tools_get_their_output_so_far() {
        let mut in_flight = InFlight::default();
        feed(
            &mut in_flight,
            &[
                start(),
                json!({"type": "message_end", "message": {"role": "assistant", "content": []}}),
                json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {}}),
                json!({"type": "tool_execution_start", "toolCallId": "c2", "toolName": "read", "args": {}}),
                json!({"type": "tool_execution_update", "toolCallId": "c1", "toolName": "bash",
                       "partialResult": {"content": [{"type": "text", "text": "line 1"}]}}),
                // A tool inside another tool is part of that tool's result.
                json!({"type": "tool_execution_start", "toolCallId": "n", "toolName": "bash",
                       "args": {}, "parentToolCallId": "c1"}),
                json!({"type": "tool_execution_end", "toolCallId": "c2", "toolName": "read",
                       "result": {}, "isError": false}),
            ],
        );
        let messages = in_flight.messages(None, 99);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["toolCallId"], "c1");
        assert_eq!(messages[0]["timestamp"], 99);
        assert_eq!(
            messages[0]["content"][0]["text"],
            format!("{STILL_RUNNING}\n\nOutput so far:\nline 1")
        );
    }

    #[test]
    fn nothing_is_left_once_the_run_ends() {
        let mut in_flight = InFlight::default();
        feed(
            &mut in_flight,
            &[
                start(),
                update(json!({"type": "text_delta", "contentIndex": 0, "delta": "x"})),
                json!({"type": "agent_end", "messages": []}),
            ],
        );
        assert!(in_flight.is_empty());
        assert_eq!(in_flight.messages(None, 0), Vec::<Value>::new());
        // A reply with nothing in it yet adds nothing.
        feed(&mut in_flight, &[start()]);
        assert_eq!(in_flight.messages(None, 0), Vec::<Value>::new());
    }
}
