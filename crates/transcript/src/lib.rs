//! The transcript model: everything the chat view renders, derived purely from
//! pi's RPC events (live) or `get_messages` (history).
//!
//! The reducer never talks to pi and never renders. Time is injected (`now`) so
//! timing behaviour ("Thought for 4s", "Worked for 1m 3s") is deterministic in tests.

pub mod command;
mod context;
pub mod explore;
mod format;
mod history;
mod model;
mod reducer;
pub mod search;
pub mod shell;
pub mod tool;

pub use context::ContextEstimate;
pub use format::{format_duration, format_tokens};
pub use model::{
    AssistantItem, BashItem, BashStatus, Block, BlockKind, Image, ItemId, ItemKind, NestedEntry,
    Notice, NoticeKind, Queue, RunStatus, TextBlock, ThinkingBlock, ToolBlock, ToolStatus,
    TranscriptItem, TurnSummary, UserItem,
};
pub use reducer::Transcript;
