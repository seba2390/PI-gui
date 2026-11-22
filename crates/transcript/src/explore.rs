//! Folding runs of "looking around" tool calls into one collapsible "Explored" row, the way
//! the agent reads, lists and searches before it acts.
//!
//! A run is a sequence of consecutive assistant messages that hold nothing but thinking and
//! exploration calls (pi's `read`, `ls`, `grep`, `find`, and bash commands that
//! [`crate::command`] labels as only looking). Text, edits, builds or anything unrecognized
//! end it, so a change is never hidden inside a group. The first message of a run shows the
//! group; the others are folded into it.

use crate::command;
use crate::model::{BlockKind, ItemKind, ToolBlock, TranscriptItem};

/// How a transcript item is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowRole {
    Plain,
    /// The first item of a run: it shows the whole group.
    Group(Group),
    /// Part of the group shown by an earlier row.
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The last item of the run (by index).
    pub last: usize,
    /// The exploration calls in order, as (item index, block index).
    pub tools: Vec<(usize, usize)>,
}

/// Fewer calls than this are shown as they are.
const MIN_GROUP: usize = 2;

/// Whether a tool call only looks (reads, lists, searches, checks). Bash commands count once
/// their arguments are complete and every part of them is understood as looking.
pub fn is_exploration(tool: &ToolBlock) -> bool {
    match tool.name.as_str() {
        "read" | "ls" | "grep" | "find" => true,
        "bash" => tool
            .args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .and_then(command::summarize)
            .is_some_and(|summary| summary.is_exploration()),
        _ => false,
    }
}

/// The role of every item in `items`.
pub fn row_roles(items: &[TranscriptItem]) -> Vec<RowRole> {
    let members: Vec<Option<Vec<usize>>> = items.iter().map(exploration_tools).collect();
    group_rows(&members)
}

/// Roles from each item's [`exploration_tools`] (so callers can cache those per item).
pub fn group_rows(members: &[Option<Vec<usize>>]) -> Vec<RowRole> {
    let mut roles = vec![RowRole::Plain; members.len()];
    let mut index = 0;
    while index < members.len() {
        let Some(first_tools) = &members[index] else {
            index += 1;
            continue;
        };
        let start = index;
        let mut tools: Vec<(usize, usize)> =
            first_tools.iter().map(|block| (start, *block)).collect();
        let mut end = index;
        while let Some(Some(more)) = members.get(end + 1) {
            end += 1;
            tools.extend(more.iter().map(|block| (end, *block)));
        }
        if tools.len() >= MIN_GROUP {
            roles[start] = RowRole::Group(Group { last: end, tools });
            for role in &mut roles[start + 1..=end] {
                *role = RowRole::Hidden;
            }
        }
        index = end + 1;
    }
    roles
}

/// The exploration calls of an assistant message that holds nothing else (besides thinking
/// and empty text); `None` for any other item.
pub fn exploration_tools(item: &TranscriptItem) -> Option<Vec<usize>> {
    let ItemKind::Assistant(assistant) = &item.kind else {
        return None;
    };
    if assistant.error.is_some() {
        return None;
    }
    let mut tools = Vec::new();
    for (index, block) in assistant.blocks.iter().enumerate() {
        match &block.kind {
            BlockKind::Thinking(_) => {}
            BlockKind::Text(text) if text.text.trim().is_empty() => {}
            BlockKind::Tool(tool) if is_exploration(tool) => tools.push(index),
            _ => return None,
        }
    }
    (!tools.is_empty()).then_some(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssistantItem, Block, ItemId, TextBlock, ToolStatus};
    use pi_rpc::protocol::StopReason;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn tool(name: &str, args: serde_json::Value) -> BlockKind {
        BlockKind::Tool(ToolBlock {
            call_id: "c".into(),
            name: name.into(),
            args,
            args_partial: String::new(),
            status: ToolStatus::Done,
            output: String::new(),
            details: None,
            duration_ms: None,
            nested: Vec::new(),
        })
    }

    fn bash(command: &str) -> BlockKind {
        tool("bash", json!({ "command": command }))
    }

    fn assistant(id: u64, blocks: Vec<BlockKind>) -> TranscriptItem {
        TranscriptItem {
            id: ItemId(id),
            version: 0,
            kind: ItemKind::Assistant(AssistantItem {
                blocks: blocks
                    .into_iter()
                    .enumerate()
                    .map(|(content_index, kind)| Block {
                        content_index,
                        kind,
                    })
                    .collect(),
                model: String::new(),
                stop_reason: StopReason::ToolUse,
                error: None,
            }),
        }
    }

    fn text(text: &str) -> BlockKind {
        BlockKind::Text(TextBlock {
            text: text.into(),
            done: true,
        })
    }

    #[test]
    fn folds_runs_of_looking_and_stops_at_changes() {
        let items = vec![
            assistant(0, vec![bash("ls -la && which cargo")]),
            assistant(
                1,
                vec![
                    tool("read", json!({"path": "Cargo.toml"})),
                    bash("rg -n foo src"),
                ],
            ),
            assistant(2, vec![bash("cargo add macroquad")]),
            assistant(3, vec![bash("cat src/main.rs")]),
            assistant(4, vec![text("Now I'll edit it."), bash("cat src/lib.rs")]),
            assistant(5, vec![bash("git status"), bash("git diff")]),
        ];
        assert_eq!(
            row_roles(&items),
            vec![
                RowRole::Group(Group {
                    last: 1,
                    tools: vec![(0, 0), (1, 0), (1, 1)],
                }),
                RowRole::Hidden,
                RowRole::Plain,
                // A single call is shown as it is.
                RowRole::Plain,
                // Text explains what follows: not folded.
                RowRole::Plain,
                RowRole::Group(Group {
                    last: 5,
                    tools: vec![(5, 0), (5, 1)],
                }),
            ]
        );
    }

    #[test]
    fn streaming_and_unknown_commands_are_not_exploration() {
        let mut streaming = ToolBlock {
            call_id: "c".into(),
            name: "bash".into(),
            args: serde_json::Value::Null,
            args_partial: r#"{"command":"ls"#.into(),
            status: ToolStatus::Streaming,
            output: String::new(),
            details: None,
            duration_ms: None,
            nested: Vec::new(),
        };
        assert!(!is_exploration(&streaming));
        streaming.args = json!({"command": "ls"});
        assert!(is_exploration(&streaming));
        streaming.args = json!({"command": "find . -delete"});
        assert!(!is_exploration(&streaming));
    }
}
