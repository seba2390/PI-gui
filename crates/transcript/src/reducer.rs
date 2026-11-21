//! The event reducer.

use std::collections::HashMap;
use std::time::Instant;

use pi_rpc::protocol::{
    AgentMessage, AssistantMessage, AssistantMessageEvent, ContentBlock, Event, ThinkingLevel,
    ToolResultMessage, Usage,
};
use serde_json::Value;

use crate::format::format_tokens;
use crate::model::{
    AssistantItem, Block, BlockKind, ItemId, ItemKind, Notice, NoticeKind, Queue, RunStatus,
    TextBlock, ThinkingBlock, ToolBlock, ToolStatus, TranscriptItem, TurnSummary, UserItem,
};

/// The live transcript of one pi session.
#[derive(Debug, Clone)]
pub struct Transcript {
    items: Vec<TranscriptItem>,
    next_id: u64,
    next_version: u64,
    revision: u64,
    status: RunStatus,
    run_started: Option<Instant>,
    /// Index (into `items`) of the assistant message currently streaming.
    current_assistant: Option<usize>,
    /// Tool call id → (item index, block index). Items are append-only, so stable.
    tools: HashMap<String, (usize, usize)>,
    queue: Queue,
    session_name: Option<String>,
    thinking_level: Option<ThinkingLevel>,
    last_usage: Option<Usage>,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            next_version: 1,
            revision: 0,
            status: RunStatus::Idle,
            run_started: None,
            current_assistant: None,
            tools: HashMap::new(),
            queue: Queue::default(),
            session_name: None,
            thinking_level: None,
            last_usage: None,
        }
    }

    // ---- reads ---------------------------------------------------------------

    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    /// Increases on every change; cheap "did anything change" check for views.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn status(&self) -> &RunStatus {
        &self.status
    }

    /// True from `agent_start` until `agent_settled`.
    pub fn is_running(&self) -> bool {
        self.run_started.is_some()
    }

    pub fn run_started(&self) -> Option<Instant> {
        self.run_started
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn session_name(&self) -> Option<&str> {
        self.session_name.as_deref()
    }

    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        self.thinking_level
    }

    /// Latest provider-reported usage of the streaming/last assistant response.
    pub fn last_usage(&self) -> Option<&Usage> {
        self.last_usage.as_ref()
    }

    // ---- writes from outside the event stream --------------------------------

    /// Adds a GUI-originated notice (e.g. "pi exited unexpectedly").
    pub fn push_notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.push(ItemKind::Notice(Notice {
            kind,
            text: text.into(),
        }));
    }

    /// Adds the result of a user-run shell command (`!cmd`).
    pub fn push_bash(&mut self, command: String, output: String, exit_code: Option<i32>) {
        self.push(ItemKind::Bash {
            command,
            output,
            exit_code,
        });
    }

    pub fn set_session_name(&mut self, name: Option<String>) {
        self.session_name = name;
        self.revision += 1;
    }

    /// Ends any in-flight run as aborted, e.g. when the pi process died.
    pub fn force_settle(&mut self, now: Instant) {
        self.settle(true, now);
    }

    // ---- the reducer ---------------------------------------------------------

    /// Applies one pi session event observed at `now`.
    pub fn apply(&mut self, event: &Event, now: Instant) {
        match event {
            Event::AgentStart => {
                if self.run_started.is_none() {
                    self.run_started = Some(now);
                }
                self.status = RunStatus::Working {
                    since: self.run_started.unwrap_or(now),
                };
                self.revision += 1;
            }
            Event::AgentSettled { aborted } => self.settle(*aborted, now),
            Event::MessageStart {
                message,
                parent_tool_call_id: None,
            } => {
                self.message_start(message, now);
            }
            Event::MessageEnd {
                message,
                parent_tool_call_id: None,
            } => {
                self.message_end(message, now);
            }
            Event::MessageUpdate {
                usage,
                assistant_message_event,
                parent_tool_call_id: None,
            } => {
                if let Some(usage) = usage.as_ref().filter(|usage| usage.total_tokens > 0) {
                    self.last_usage = Some(usage.clone());
                }
                self.message_update(assistant_message_event, now);
            }
            Event::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                parent_tool_call_id: None,
            } => {
                let (item, block) = self.tool_slot(tool_call_id, tool_name);
                self.edit_tool(item, block, |tool| {
                    tool.status = ToolStatus::Running;
                    if tool.args.is_null() {
                        tool.args = args.clone();
                    }
                });
            }
            Event::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                partial_result,
                parent_tool_call_id: None,
            } => {
                let (item, block) = self.tool_slot(tool_call_id, tool_name);
                self.edit_tool(item, block, |tool| {
                    tool.output = result_text(partial_result);
                });
            }
            Event::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
                duration_ms,
                parent_tool_call_id: None,
            } => {
                let (item, block) = self.tool_slot(tool_call_id, tool_name);
                self.edit_tool(item, block, |tool| {
                    tool.status = if *is_error {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Done
                    };
                    tool.output = result_text(result);
                    if let Some(details) = result.get("details").filter(|d| !d.is_null()) {
                        tool.details = Some(details.clone());
                    }
                    tool.duration_ms = *duration_ms;
                });
            }
            Event::QueueUpdate {
                steering,
                follow_up,
            } => {
                self.queue = Queue {
                    steering: steering.clone(),
                    follow_up: follow_up.clone(),
                };
                self.revision += 1;
            }
            Event::SessionInfoChanged { name } => self.set_session_name(name.clone()),
            Event::ThinkingLevelChanged { level } => {
                self.thinking_level = Some(*level);
                self.revision += 1;
            }
            Event::CompactionStart { .. } => {
                self.status = RunStatus::Compacting;
                self.revision += 1;
            }
            Event::CompactionEnd {
                result,
                aborted,
                error_message,
                ..
            } => {
                if let Some(result) = result {
                    self.push_notice(NoticeKind::Compaction, compaction_text(result));
                } else if let Some(error) = error_message {
                    self.push_notice(NoticeKind::Error, format!("Compaction failed: {error}"));
                } else if *aborted {
                    self.push_notice(NoticeKind::Warning, "Compaction cancelled");
                }
                self.restore_status();
            }
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                error_message,
            } => {
                self.status = RunStatus::Retrying {
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    error: error_message.clone(),
                };
                self.push_notice(
                    NoticeKind::Warning,
                    format!(
                        "{error_message} · retrying in {}s (attempt {attempt}/{max_attempts})",
                        delay_ms.div_ceil(1000)
                    ),
                );
            }
            Event::AutoRetryEnd {
                success,
                final_error,
                ..
            } => {
                if !success {
                    let error = final_error.as_deref().unwrap_or("unknown error");
                    self.push_notice(NoticeKind::Error, format!("Retry failed: {error}"));
                }
                self.restore_status();
            }
            Event::ExtensionError {
                extension_path,
                event,
                error,
            } => {
                self.push_notice(
                    NoticeKind::Error,
                    format!("Extension error in {extension_path} ({event}): {error}"),
                );
            }
            // Nested tool activity and bookkeeping events are not rendered (yet).
            _ => {}
        }
    }

    // ---- message lifecycle ---------------------------------------------------

    fn message_start(&mut self, message: &AgentMessage, now: Instant) {
        match message {
            AgentMessage::Assistant(assistant) => {
                let index = self.push(ItemKind::Assistant(AssistantItem {
                    blocks: Vec::new(),
                    model: assistant.model.clone(),
                    stop_reason: assistant.stop_reason,
                    error: None,
                }));
                self.current_assistant = Some(index);
                if !assistant.content.is_empty() {
                    self.reconcile_assistant(index, assistant, now, false);
                }
            }
            AgentMessage::ToolResult(result) => self.apply_tool_result(result),
            other => self.push_message(other, now),
        }
    }

    fn message_end(&mut self, message: &AgentMessage, now: Instant) {
        match message {
            AgentMessage::Assistant(assistant) => {
                let index = self.current_assistant.take().unwrap_or_else(|| {
                    self.push(ItemKind::Assistant(AssistantItem {
                        blocks: Vec::new(),
                        model: assistant.model.clone(),
                        stop_reason: assistant.stop_reason,
                        error: None,
                    }))
                });
                self.reconcile_assistant(index, assistant, now, true);
            }
            AgentMessage::ToolResult(result) => self.apply_tool_result(result),
            // Everything else was rendered at message_start.
            _ => {}
        }
    }

    /// Appends a complete message (history, or a non-streaming live message).
    pub(crate) fn push_message(&mut self, message: &AgentMessage, now: Instant) {
        match message {
            AgentMessage::User(user) => {
                self.push(ItemKind::User(UserItem {
                    text: user.content.text(),
                    image_count: user.content.image_count(),
                }));
            }
            AgentMessage::Assistant(assistant) => {
                let index = self.push(ItemKind::Assistant(AssistantItem {
                    blocks: Vec::new(),
                    model: assistant.model.clone(),
                    stop_reason: assistant.stop_reason,
                    error: None,
                }));
                self.reconcile_assistant(index, assistant, now, true);
            }
            AgentMessage::ToolResult(result) => self.apply_tool_result(result),
            AgentMessage::BashExecution(bash) => {
                self.push(ItemKind::Bash {
                    command: bash.command.clone(),
                    output: bash.output.clone(),
                    exit_code: bash.exit_code,
                });
            }
            AgentMessage::Custom(custom) if custom.display => {
                self.push_notice(NoticeKind::Info, custom.content.text());
            }
            AgentMessage::CompactionSummary(summary) => {
                self.push_notice(
                    NoticeKind::Compaction,
                    format!(
                        "Context compacted from {} tokens\n\n{}",
                        format_tokens(summary.tokens_before),
                        summary.summary
                    ),
                );
            }
            AgentMessage::BranchSummary(summary) => {
                self.push_notice(NoticeKind::BranchSummary, summary.summary.clone());
            }
            AgentMessage::System(_) | AgentMessage::Custom(_) | AgentMessage::Unknown => {}
        }
    }

    fn message_update(&mut self, update: &AssistantMessageEvent, now: Instant) {
        let index = if let Some(index) = self.current_assistant {
            index
        } else {
            let index = self.push(ItemKind::Assistant(AssistantItem {
                blocks: Vec::new(),
                model: String::new(),
                stop_reason: pi_rpc::protocol::StopReason::Pending,
                error: None,
            }));
            self.current_assistant = Some(index);
            index
        };
        match update {
            AssistantMessageEvent::ThinkingStart { content_index } => {
                self.edit_block(index, *content_index, new_thinking(Some(now)), |_| {});
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                self.edit_block(index, *content_index, new_thinking(Some(now)), |kind| {
                    if let BlockKind::Thinking(thinking) = kind {
                        thinking.text.push_str(delta);
                    }
                });
            }
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
            } => {
                self.edit_block(index, *content_index, new_thinking(Some(now)), |kind| {
                    if let BlockKind::Thinking(thinking) = kind {
                        finish_thinking(thinking, Some(content), now);
                    }
                });
            }
            AssistantMessageEvent::TextStart { content_index } => {
                self.edit_block(index, *content_index, new_text(), |_| {});
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                self.edit_block(index, *content_index, new_text(), |kind| {
                    if let BlockKind::Text(text) = kind {
                        text.text.push_str(delta);
                    }
                });
            }
            AssistantMessageEvent::TextEnd {
                content_index,
                content,
            } => {
                self.edit_block(index, *content_index, new_text(), |kind| {
                    if let BlockKind::Text(text) = kind {
                        if !content.is_empty() {
                            text.text.clone_from(content);
                        }
                        text.done = true;
                    }
                });
            }
            AssistantMessageEvent::ToolcallStart {
                content_index,
                id,
                tool_name,
            } => {
                let block = self.edit_block(index, *content_index, new_tool(id, tool_name), |_| {});
                self.tools.insert(id.clone(), (index, block));
            }
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
            } => {
                self.edit_block(index, *content_index, new_tool("", ""), |kind| {
                    if let BlockKind::Tool(tool) = kind {
                        tool.args_partial.push_str(delta);
                    }
                });
            }
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                let block = self.edit_block(
                    index,
                    *content_index,
                    new_tool(&tool_call.id, &tool_call.name),
                    |kind| {
                        if let BlockKind::Tool(tool) = kind {
                            tool.call_id.clone_from(&tool_call.id);
                            tool.name.clone_from(&tool_call.name);
                            tool.args = tool_call.arguments.clone();
                            if tool.status == ToolStatus::Streaming {
                                tool.status = ToolStatus::Pending;
                            }
                        }
                    },
                );
                self.tools.insert(tool_call.id.clone(), (index, block));
            }
            AssistantMessageEvent::Start
            | AssistantMessageEvent::Done { .. }
            | AssistantMessageEvent::Error { .. }
            | AssistantMessageEvent::Unknown => {}
        }
    }

    /// Makes the item at `index` match the authoritative `message`.
    fn reconcile_assistant(
        &mut self,
        index: usize,
        message: &AssistantMessage,
        now: Instant,
        is_final: bool,
    ) {
        for (content_index, content) in message.content.iter().enumerate() {
            match content {
                ContentBlock::Thinking(source) => {
                    self.edit_block(index, content_index, new_thinking(None), |kind| {
                        if let BlockKind::Thinking(thinking) = kind {
                            thinking.redacted = source.redacted;
                            if is_final {
                                finish_thinking(thinking, Some(&source.thinking), now);
                            } else if !source.thinking.is_empty() {
                                thinking.text.clone_from(&source.thinking);
                            }
                        }
                    });
                }
                ContentBlock::Text(source) => {
                    self.edit_block(index, content_index, new_text(), |kind| {
                        if let BlockKind::Text(text) = kind {
                            if !source.text.is_empty() || is_final {
                                text.text.clone_from(&source.text);
                            }
                            text.done |= is_final;
                        }
                    });
                }
                ContentBlock::ToolCall(call) => {
                    let block = self.edit_block(
                        index,
                        content_index,
                        new_tool(&call.id, &call.name),
                        |kind| {
                            if let BlockKind::Tool(tool) = kind {
                                tool.call_id.clone_from(&call.id);
                                tool.name.clone_from(&call.name);
                                tool.args = call.arguments.clone();
                                if tool.status == ToolStatus::Streaming {
                                    tool.status = ToolStatus::Pending;
                                }
                            }
                        },
                    );
                    self.tools.insert(call.id.clone(), (index, block));
                }
                ContentBlock::Image(_) | ContentBlock::Unknown => {}
            }
        }
        if let ItemKind::Assistant(item) = &mut self.items[index].kind {
            item.stop_reason = message.stop_reason;
            item.error.clone_from(&message.error_message);
            if !message.model.is_empty() {
                item.model.clone_from(&message.model);
            }
        }
        self.touch(index);
    }

    fn apply_tool_result(&mut self, result: &ToolResultMessage) {
        let (item, block) = self.tool_slot(&result.tool_call_id, &result.tool_name);
        self.edit_tool(item, block, |tool| {
            tool.status = if result.is_error {
                ToolStatus::Failed
            } else {
                ToolStatus::Done
            };
            let text = blocks_text(&result.content);
            if !text.is_empty() || tool.output.is_empty() {
                tool.output = text;
            }
            if result.details.as_ref().is_some_and(|d| !d.is_null()) {
                tool.details.clone_from(&result.details);
            }
        });
    }

    fn settle(&mut self, aborted: bool, now: Instant) {
        // Close anything left streaming or running.
        for index in 0..self.items.len() {
            let mut changed = false;
            if let ItemKind::Assistant(item) = &mut self.items[index].kind {
                for block in &mut item.blocks {
                    match &mut block.kind {
                        BlockKind::Thinking(thinking) if !thinking.done => {
                            finish_thinking(thinking, None, now);
                            changed = true;
                        }
                        BlockKind::Text(text) if !text.done => {
                            text.done = true;
                            changed = true;
                        }
                        BlockKind::Tool(tool) if tool.status.is_active() => {
                            tool.status = ToolStatus::Cancelled;
                            changed = true;
                        }
                        _ => {}
                    }
                }
                if item.is_streaming() {
                    item.stop_reason = if aborted {
                        pi_rpc::protocol::StopReason::Aborted
                    } else {
                        pi_rpc::protocol::StopReason::Stop
                    };
                    changed = true;
                }
            }
            if changed {
                self.touch(index);
            }
        }
        self.current_assistant = None;
        if let Some(started) = self.run_started.take() {
            self.push(ItemKind::TurnSummary(TurnSummary {
                duration: now.saturating_duration_since(started),
                aborted,
            }));
        }
        self.status = RunStatus::Idle;
        self.revision += 1;
    }

    fn restore_status(&mut self) {
        self.status = match self.run_started {
            Some(since) => RunStatus::Working { since },
            None => RunStatus::Idle,
        };
        self.revision += 1;
    }

    // ---- primitives ----------------------------------------------------------

    pub(crate) fn push(&mut self, kind: ItemKind) -> usize {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        let version = self.bump();
        self.items.push(TranscriptItem { id, version, kind });
        self.revision += 1;
        self.items.len() - 1
    }

    pub(crate) fn push_turn_summary(&mut self, summary: TurnSummary) {
        self.push(ItemKind::TurnSummary(summary));
    }

    fn bump(&mut self) -> u64 {
        let version = self.next_version;
        self.next_version += 1;
        version
    }

    fn touch(&mut self, index: usize) {
        self.items[index].version = self.bump();
        self.revision += 1;
    }

    /// Finds (or creates with `default`) the block for `content_index` in the
    /// assistant item at `index`, applies `edit`, and returns its block index.
    fn edit_block(
        &mut self,
        index: usize,
        content_index: usize,
        default: BlockKind,
        edit: impl FnOnce(&mut BlockKind),
    ) -> usize {
        let ItemKind::Assistant(item) = &mut self.items[index].kind else {
            unreachable!("edit_block on a non-assistant item");
        };
        let position = if let Some(position) = item
            .blocks
            .iter()
            .position(|b| b.content_index == content_index)
        {
            position
        } else {
            // Keep blocks ordered by content index.
            let position = item
                .blocks
                .partition_point(|block| block.content_index < content_index);
            item.blocks.insert(
                position,
                Block {
                    content_index,
                    kind: default,
                },
            );
            // Insertion shifts later blocks: re-point their tool index entries.
            for (shifted, block) in item.blocks.iter().enumerate().skip(position + 1) {
                if let BlockKind::Tool(tool) = &block.kind {
                    self.tools.insert(tool.call_id.clone(), (index, shifted));
                }
            }
            position
        };
        edit(&mut item.blocks[position].kind);
        self.touch(index);
        position
    }

    /// The (item, block) holding tool call `id`, creating a detached block if pi
    /// reports a tool we never saw requested (e.g. extension-injected).
    fn tool_slot(&mut self, id: &str, name: &str) -> (usize, usize) {
        if let Some(&slot) = self.tools.get(id) {
            return slot;
        }
        let index = self.current_assistant.unwrap_or_else(|| {
            self.push(ItemKind::Assistant(AssistantItem {
                blocks: Vec::new(),
                model: String::new(),
                stop_reason: pi_rpc::protocol::StopReason::Stop,
                error: None,
            }))
        });
        let next_index = match &self.items[index].kind {
            ItemKind::Assistant(item) => item.blocks.last().map_or(0, |b| b.content_index + 1),
            _ => 0,
        };
        let block = self.edit_block(index, next_index, new_tool(id, name), |_| {});
        self.tools.insert(id.to_owned(), (index, block));
        (index, block)
    }

    fn edit_tool(&mut self, index: usize, block: usize, edit: impl FnOnce(&mut ToolBlock)) {
        if let ItemKind::Assistant(item) = &mut self.items[index].kind
            && let Some(Block {
                kind: BlockKind::Tool(tool),
                ..
            }) = item.blocks.get_mut(block)
        {
            edit(tool);
            self.touch(index);
        }
    }
}

fn new_thinking(started_at: Option<Instant>) -> BlockKind {
    BlockKind::Thinking(ThinkingBlock {
        text: String::new(),
        redacted: false,
        done: false,
        duration: None,
        started_at,
    })
}

fn new_text() -> BlockKind {
    BlockKind::Text(TextBlock {
        text: String::new(),
        done: false,
    })
}

fn new_tool(id: &str, name: &str) -> BlockKind {
    BlockKind::Tool(ToolBlock {
        call_id: id.to_owned(),
        name: name.to_owned(),
        args: Value::Null,
        args_partial: String::new(),
        status: ToolStatus::Streaming,
        output: String::new(),
        details: None,
        duration_ms: None,
    })
}

fn finish_thinking(thinking: &mut ThinkingBlock, content: Option<&String>, now: Instant) {
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        thinking.text.clone_from(content);
    }
    if !thinking.done {
        thinking.done = true;
        if let Some(started) = thinking.started_at {
            thinking.duration = Some(now.saturating_duration_since(started));
        }
    }
}

/// Text of a tool result value (`{"content":[{"type":"text","text":...}]}`).
fn result_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn blocks_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn compaction_text(result: &Value) -> String {
    let before = result.get("tokensBefore").and_then(Value::as_u64);
    let after = result.get("estimatedTokensAfter").and_then(Value::as_u64);
    match (before, after) {
        (Some(before), Some(after)) => format!(
            "Context compacted · {} → ~{} tokens",
            format_tokens(before),
            format_tokens(after)
        ),
        _ => "Context compacted".to_owned(),
    }
}
