//! Transcript data types. Plain data: the reducer mutates, views read.

use std::time::{Duration, Instant};

use pi_rpc::protocol::StopReason;
use serde::Serialize;
use serde_json::Value;

/// Stable identity of one transcript item, for UI state such as "expanded".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ItemId(pub u64);

/// One row of the chat.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TranscriptItem {
    pub id: ItemId,
    /// Bumped on every change, so views can cache derived data (parsed markdown).
    pub version: u64,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ItemKind {
    User(UserItem),
    Assistant(AssistantItem),
    /// A user-run shell command (`!cmd`), not an agent tool call.
    Bash {
        command: String,
        output: String,
        exit_code: Option<i32>,
    },
    Notice(Notice),
    /// "Worked for 1m 3s" after the agent settles.
    TurnSummary(TurnSummary),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserItem {
    pub text: String,
    pub image_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantItem {
    pub blocks: Vec<Block>,
    pub model: String,
    pub stop_reason: StopReason,
    pub error: Option<String>,
}

impl AssistantItem {
    pub fn is_streaming(&self) -> bool {
        self.stop_reason == StopReason::Pending
    }
}

/// One content block of an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    /// pi's `contentIndex` within the assistant message.
    pub content_index: usize,
    pub kind: BlockKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
// Tool calls are the most common block; boxing them would add an allocation to each.
#[allow(clippy::large_enum_variant)]
pub enum BlockKind {
    Thinking(ThinkingBlock),
    Text(TextBlock),
    Tool(ToolBlock),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThinkingBlock {
    pub text: String,
    pub redacted: bool,
    pub done: bool,
    /// Measured live; `None` for history, where pi does not record it.
    pub duration: Option<Duration>,
    #[serde(skip)]
    pub started_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextBlock {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ToolStatus {
    /// The model is still streaming the call's arguments.
    Streaming,
    /// Arguments complete; waiting for pi to execute it.
    Pending,
    Running,
    Done,
    Failed,
    /// The run ended (abort) before the tool finished.
    Cancelled,
}

impl ToolStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Streaming | Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolBlock {
    pub call_id: String,
    pub name: String,
    /// Final arguments (`toolcall_end` / history).
    pub args: Value,
    /// Raw argument JSON while it streams, for a live preview.
    pub args_partial: String,
    pub status: ToolStatus,
    /// Text output (partial while running).
    pub output: String,
    /// Tool-specific details, e.g. `{"diff": ..., "patch": ...}` for `edit`.
    pub details: Option<Value>,
    pub duration_ms: Option<u64>,
    /// Work done inside this call (a subagent's or code mode's own tool calls and
    /// replies), in order. Live only: pi does not store it in the session.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nested: Vec<NestedEntry>,
}

/// One step inside a tool call (see [`ToolBlock::nested`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NestedEntry {
    Tool(Box<ToolBlock>),
    /// What a nested assistant (e.g. a subagent) replied.
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NoticeKind {
    Info,
    Warning,
    Error,
    Compaction,
    BranchSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnSummary {
    pub duration: Duration,
    pub aborted: bool,
}

/// What the agent is doing right now (drives the composer and status line).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RunStatus {
    Idle,
    Working {
        #[serde(skip)]
        since: Instant,
    },
    Compacting,
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
    },
}

/// Messages queued while the agent works (shown above the composer).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Queue {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

impl Queue {
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }
}
