//! The transcript model: everything the chat view renders, derived purely from
//! pi's RPC events (live) or `get_messages` (history).
//!
//! The reducer never talks to pi and never renders. Time is injected (`now`) so
//! timing behaviour ("Thought for 4s", "Worked for 1m 3s") is deterministic in tests.

mod format;
mod history;
mod model;
mod reducer;
pub mod tool;

pub use format::{format_duration, format_tokens};
pub use model::{
    AssistantItem, Block, BlockKind, ItemId, ItemKind, Notice, NoticeKind, Queue, RunStatus,
    TextBlock, ThinkingBlock, ToolBlock, ToolStatus, TranscriptItem, TurnSummary, UserItem,
};
pub use reducer::Transcript;
