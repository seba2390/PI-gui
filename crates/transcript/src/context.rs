//! A rough composition of the model's context, estimated from the transcript.
//!
//! pi reports the context *total* (from the provider's usage) but not what it consists of.
//! This estimates each category from the visible conversation at roughly four characters per
//! token, which is good enough to show where the space goes. Only items after the last
//! compaction count: earlier ones were replaced by the compaction summary.

use crate::model::{BlockKind, ItemKind, NoticeKind, TranscriptItem};

/// Characters per token for the estimate (a common rule of thumb for English and code).
const CHARS_PER_TOKEN: usize = 4;
/// Rough cost of one attached image.
const TOKENS_PER_IMAGE: u64 = 1_000;

/// Estimated tokens per category.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ContextEstimate {
    /// Prompts typed by the user (including attached images).
    pub user: u64,
    /// The assistant's visible replies.
    pub assistant: u64,
    /// Reasoning ("thinking") blocks.
    pub thinking: u64,
    /// Tool call names and arguments.
    pub tool_calls: u64,
    /// Tool output and `!cmd` shell output.
    pub tool_results: u64,
    /// The summary that replaced older messages at the last compaction.
    pub summary: u64,
}

impl ContextEstimate {
    pub fn of(items: &[TranscriptItem]) -> Self {
        let start = items
            .iter()
            .rposition(|item| matches!(&item.kind, ItemKind::Notice(notice) if notice.kind == NoticeKind::Compaction))
            .unwrap_or(0);
        let mut out = Self::default();
        for item in &items[start..] {
            match &item.kind {
                ItemKind::User(user) => {
                    out.user +=
                        tokens(user.text.len()) + TOKENS_PER_IMAGE * user.images.len() as u64;
                }
                ItemKind::Assistant(assistant) => {
                    for block in &assistant.blocks {
                        match &block.kind {
                            BlockKind::Text(text) => out.assistant += tokens(text.text.len()),
                            BlockKind::Thinking(thinking) => {
                                out.thinking += tokens(thinking.text.len());
                            }
                            BlockKind::Tool(tool) => {
                                let args = if tool.args.is_null() {
                                    tool.args_partial.len()
                                } else {
                                    tool.args.to_string().len()
                                };
                                out.tool_calls += tokens(tool.name.len() + args);
                                out.tool_results += tokens(tool.output.len());
                            }
                        }
                    }
                }
                ItemKind::Bash(bash) => {
                    out.tool_results += tokens(bash.command.len() + bash.output.len());
                }
                ItemKind::Notice(notice) if notice.kind == NoticeKind::Compaction => {
                    out.summary += tokens(notice.text.len());
                }
                ItemKind::Notice(_) | ItemKind::Step(_) | ItemKind::TurnSummary(_) => {}
            }
        }
        out
    }

    pub fn total(&self) -> u64 {
        self.user
            + self.assistant
            + self.thinking
            + self.tool_calls
            + self.tool_results
            + self.summary
    }
}

fn tokens(chars: usize) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN) as u64
}

#[cfg(test)]
mod tests {
    use pi_rpc::protocol::StopReason;
    use serde_json::json;

    use super::ContextEstimate;
    use crate::model::{
        AssistantItem, BashItem, BashStatus, Block, BlockKind, ItemId, ItemKind, Notice,
        NoticeKind, TextBlock, ThinkingBlock, ToolBlock, ToolStatus, TranscriptItem, UserItem,
    };

    fn item(kind: ItemKind) -> TranscriptItem {
        TranscriptItem {
            id: ItemId(0),
            version: 0,
            kind,
        }
    }

    fn user(text: &str, images: usize) -> TranscriptItem {
        item(ItemKind::User(UserItem {
            text: text.to_owned(),
            images: vec![crate::model::Image::new("", "image/png"); images],
        }))
    }

    #[test]
    fn counts_categories_after_the_last_compaction() {
        let assistant = AssistantItem {
            blocks: vec![
                Block {
                    content_index: 0,
                    kind: BlockKind::Thinking(ThinkingBlock {
                        text: "t".repeat(40),
                        redacted: false,
                        done: true,
                        duration: None,
                        started_at: None,
                    }),
                },
                Block {
                    content_index: 1,
                    kind: BlockKind::Text(TextBlock {
                        text: "a".repeat(80),
                        done: true,
                    }),
                },
                Block {
                    content_index: 2,
                    kind: BlockKind::Tool(ToolBlock {
                        call_id: "c1".to_owned(),
                        name: "read".to_owned(),
                        args: json!({"path": "a.rs"}),
                        args_partial: String::new(),
                        status: ToolStatus::Done,
                        output: "o".repeat(400),
                        details: None,
                        duration_ms: None,
                        images: Vec::new(),
                        nested: Vec::new(),
                    }),
                },
            ],
            model: "m".to_owned(),
            stop_reason: StopReason::Stop,
            error: None,
            standing: crate::model::Standing::Answer,
        };
        let items = vec![
            user(&"x".repeat(4000), 0),
            item(ItemKind::Notice(Notice {
                kind: NoticeKind::Compaction,
                text: "s".repeat(20),
            })),
            user("hello there", 2),
            item(ItemKind::Assistant(assistant)),
            item(ItemKind::Bash(BashItem {
                command: "ls".to_owned(),
                output: "b".repeat(10),
                status: BashStatus::Done { exit_code: Some(0) },
                full_output: None,
            })),
        ];
        let estimate = ContextEstimate::of(&items);
        assert_eq!(estimate.summary, 5);
        assert_eq!(estimate.user, 3 + 2_000); // the pre-compaction prompt is not counted
        assert_eq!(estimate.thinking, 10);
        assert_eq!(estimate.assistant, 20);
        assert_eq!(estimate.tool_calls, 5); // "read" + {"path":"a.rs"}: 4 + 15 = 19 chars
        assert_eq!(estimate.tool_results, 100 + 3);
        assert_eq!(estimate.total(), 5 + 2_003 + 10 + 20 + 5 + 103);
        assert_eq!(ContextEstimate::of(&[]), ContextEstimate::default());
    }
}
