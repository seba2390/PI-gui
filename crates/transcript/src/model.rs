//! Transcript data types. Plain data: the reducer mutates, views read.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pi_rpc::protocol::StopReason;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolKind;

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
    Bash(BashItem),
    Notice(Notice),
    /// What an extension did for the answer besides the model's own steps.
    Step(StepItem),
    /// How a deep search's report was checked.
    Check(CheckItem),
    /// "Worked for 1m 3s" after the agent settles.
    TurnSummary(TurnSummary),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserItem {
    pub text: String,
    /// Images attached to the prompt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}

/// An image in the conversation (attached to a prompt, or returned by a tool such as
/// `read` on a picture), base64-encoded as pi sent it. The data is shared, so copies of
/// the transcript don't copy it; views decode it only when they show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Image {
    #[serde(skip)]
    pub data: Arc<str>,
    pub mime_type: String,
}

impl Image {
    pub fn new(data: &str, mime_type: &str) -> Self {
        Self {
            data: Arc::from(data),
            mime_type: mime_type.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantItem {
    pub blocks: Vec<Block>,
    pub model: String,
    pub stop_reason: StopReason,
    pub error: Option<String>,
    /// What its text is.
    #[serde(skip_serializing_if = "Standing::is_answer")]
    pub standing: Standing,
}

impl AssistantItem {
    pub fn is_streaming(&self) -> bool {
        self.stop_reason == StopReason::Pending
    }

    /// Its text, if it is an answer (and not a step on the way to one).
    pub fn answer(&self) -> Option<String> {
        self.standing.is_shown().then(|| self.text())
    }

    /// All of its text.
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Text(text) => Some(text.text.trim()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn has_tools(&self) -> bool {
        self.blocks
            .iter()
            .any(|block| matches!(block.kind, BlockKind::Tool(_)))
    }
}

/// What the text of an assistant message is. Outside deep search it is always the answer; a
/// report of deep search is only once it was checked (see [`crate::research`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum Standing {
    #[default]
    Answer,
    /// The answer, but it was not checked.
    Unchecked,
    /// Being written or checked.
    Held,
    /// A report that another one replaces.
    Draft,
    /// Said along the way, between steps.
    Remark,
}

impl Standing {
    #[allow(clippy::trivially_copy_pass_by_ref)] // How serde calls it.
    fn is_answer(&self) -> bool {
        *self == Self::Answer
    }

    /// Whether the text is for the user to read, not part of the work.
    pub fn is_shown(self) -> bool {
        matches!(self, Self::Answer | Self::Unchecked)
    }
}

/// A step of an extension: what it did, and the lines behind it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepItem {
    pub title: String,
    pub detail: String,
    pub lines: Vec<String>,
}

impl StepItem {
    pub fn kind(&self) -> ToolKind {
        match self.title.as_str() {
            "Planned" => ToolKind::List,
            "Checked" => ToolKind::Search,
            "Continuing research" => ToolKind::Web,
            _ => ToolKind::Write,
        }
    }
}

/// What deep search says about its report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckItem {
    /// Otherwise the report is the answer as it is.
    pub checked: bool,
    /// All of it as text.
    pub text: String,
    /// The same in parts; chats from before there were any have only the text.
    pub about: Option<Box<AboutReport>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AboutReport {
    pub effort: String,
    pub searches: u32,
    pub pages: u32,
    pub sites: u32,
    pub links: LinkCounts,
    /// From the effort on that checks them.
    pub claims: Option<ClaimCounts>,
    pub problems: Vec<Problem>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkCounts {
    pub total: u32,
    pub broken: u32,
    pub unread: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaimCounts {
    pub total: u32,
    pub backed: u32,
    pub partly: u32,
    pub unbacked: u32,
    pub unsourced: u32,
    pub unchecked: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Problem {
    pub kind: ProblemKind,
    pub text: String,
    pub why: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProblemKind {
    Unbacked,
    Partly,
    Unsourced,
    Broken,
    Unread,
    #[default]
    #[serde(other)]
    Other,
}

impl ProblemKind {
    /// In the order they are shown.
    pub const ALL: [Self; 6] = [
        Self::Unbacked,
        Self::Partly,
        Self::Unsourced,
        Self::Broken,
        Self::Unread,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Unbacked => "Not backed by their source",
            Self::Partly => "Only partly backed",
            Self::Unsourced => "Without a source that was read",
            Self::Broken => "Broken links",
            Self::Unread => "Cited, but not read",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BashItem {
    pub command: String,
    /// What it printed (while it runs, at most the last part of it).
    pub output: String,
    pub status: BashStatus,
    /// The file with the whole output, when `output` is only the end of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_output: Option<String>,
}

/// Lines of output a running command shows.
const LIVE_LINES: usize = 16;

/// Bytes of output a running command shows (its lines may be very long).
const LIVE_BYTES: usize = 8 * 1024;

impl BashItem {
    pub fn is_running(&self) -> bool {
        matches!(self.status, BashStatus::Running { .. })
    }

    /// The output to show: the last lines while the command runs, all of it afterwards.
    pub fn shown_output(&self) -> &str {
        if !self.is_running() {
            return &self.output;
        }
        let output = self.output.trim_end();
        let mut start = output.len().saturating_sub(LIVE_BYTES);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        let tail = &output[start..];
        match tail.rmatch_indices('\n').nth(LIVE_LINES - 1) {
            Some((newline, _)) => &tail[newline + 1..],
            None => tail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BashStatus {
    Running {
        #[serde(skip)]
        since: Instant,
    },
    Done {
        exit_code: Option<i32>,
    },
    /// Stopped by the user, or because pi stopped.
    Cancelled,
    /// pi could not run it.
    Failed,
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
    /// Images in the result (e.g. `read` on a picture), in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
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
