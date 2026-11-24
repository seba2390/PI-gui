//! Which text of a deep search is its answer.
//!
//! Deep search's report is checked and often written again, so a text of the model is not the
//! answer until its extension says so. The extension leaves that in the session: the results of
//! its tools carry `details.deep`, and its messages ([`CHECK`]) say in `details.stage` that the
//! text above was a draft (and what is done with it), or that the answer is there. Until then the
//! text is held back ([`Standing::Held`]); if nothing is said any more (pi ended), the last
//! complete text is the answer, not checked.
//!
//! Live events and history take the same path, so a chat looks the same when it is opened again.

use pi_rpc::protocol::StopReason;
use serde_json::Value;

use crate::model::{AssistantItem, Block, BlockKind, ItemKind, NoticeKind, Standing, StepItem};
use crate::reducer::Transcript;

/// The extension's messages.
pub const CHECK: &str = "pistachio-deepsearch-check";

impl Transcript {
    /// What the text of a new assistant message is, as far as that is known.
    pub(crate) fn new_standing(&self) -> Standing {
        if self.researching.is_some() {
            Standing::Held
        } else {
            Standing::Answer
        }
    }

    /// A message with tool calls is on its way to the answer.
    pub(crate) fn settle_remark(&mut self, index: usize) {
        if let Some(item) = self.assistant_mut(index)
            && item.standing == Standing::Held
            && item.has_tools()
        {
            item.standing = Standing::Remark;
        }
    }

    /// The result of a tool call of `item`: from deep search, or not.
    pub(crate) fn note_research(&mut self, item: usize, details: Option<&Value>) {
        let deep = details.is_some_and(|details| details["deep"].is_string());
        if !deep || self.researching.is_some() {
            return;
        }
        let start = self.items[..item]
            .iter()
            .rposition(|item| matches!(item.kind, ItemKind::User(_)))
            .map_or(0, |user| user + 1);
        self.researching = Some(start);
        // What was said before the first result.
        for index in start..self.items.len() {
            if let Some(item) = self.assistant_mut(index)
                && item.has_tools()
            {
                item.standing = Standing::Remark;
                self.touch(index);
            }
        }
    }

    /// A message of an extension.
    pub(crate) fn custom_message(
        &mut self,
        custom_type: &str,
        text: &str,
        display: bool,
        details: Option<&Value>,
    ) {
        let stage = details
            .filter(|_| custom_type == CHECK)
            .and_then(|details| details["stage"].as_str());
        match (stage, details) {
            (Some("plan" | "draft"), Some(details)) => {
                if stage == Some("draft")
                    && let Some(index) = self.reports().pop()
                {
                    self.set_standing(index, Standing::Draft);
                }
                let steps: Vec<StepItem> =
                    serde_json::from_value(details["steps"].clone()).unwrap_or_default();
                for step in steps {
                    self.push(ItemKind::Step(step));
                }
            }
            (Some("final"), Some(details)) => {
                let checked = details["checked"] == true;
                let back = details["back"]
                    .as_u64()
                    .and_then(|back| usize::try_from(back).ok())
                    .unwrap_or(0);
                self.resolve(checked, back);
                let kind = if checked {
                    NoticeKind::Info
                } else {
                    NoticeKind::Warning
                };
                self.push_notice(kind, text);
            }
            _ if display => self.push_notice(NoticeKind::Info, text),
            _ => {}
        }
    }

    /// The run is over, also if the extension had no time to say so.
    pub(crate) fn end_research(&mut self) {
        if self.researching.is_some() && self.resolve(false, 0) {
            self.push_notice(
                NoticeKind::Warning,
                "This report was not checked: it was stopped.",
            );
        }
    }

    /// The next prompt in history. One that steered a run finds its last text marked as a draft;
    /// a text nothing was said about was cut off by pi's end.
    pub(crate) fn end_interrupted_research(&mut self) {
        let cut_off = self.reports().pop().is_some_and(|index| {
            matches!(&self.items[index].kind, ItemKind::Assistant(item) if item.standing == Standing::Held)
        });
        if cut_off {
            self.end_research();
        }
    }

    /// The reports written so far, in order: texts without tool calls that are not answers yet.
    fn reports(&self) -> Vec<usize> {
        let start = self.researching.unwrap_or_else(|| {
            // Its first results are no longer in the context.
            self.items
                .iter()
                .rposition(|item| matches!(item.kind, ItemKind::User(_)))
                .map_or(0, |user| user + 1)
        });
        let researching = self.researching.is_some();
        (start..self.items.len())
            .filter(|index| match &self.items[*index].kind {
                ItemKind::Assistant(item) => {
                    let open = match item.standing {
                        Standing::Held | Standing::Draft => true,
                        Standing::Answer => !researching,
                        Standing::Unchecked | Standing::Remark => false,
                    };
                    open && !item.has_tools() && !item.text().is_empty()
                }
                _ => false,
            })
            .collect()
    }

    /// Makes the last complete report (or the `back`th before it) the answer; one that was cut
    /// off only if there is no other. Returns whether there was one.
    fn resolve(&mut self, checked: bool, back: usize) -> bool {
        let reports = self.reports();
        self.researching = None;
        let complete: Vec<usize> = reports
            .iter()
            .copied()
            .filter(|index| {
                matches!(
                    &self.items[*index].kind,
                    ItemKind::Assistant(item)
                        if matches!(item.stop_reason, StopReason::Stop | StopReason::Length)
                )
            })
            .collect();
        let chosen = match complete.len() {
            0 => reports.last().copied(),
            len => Some(complete[(len - 1).saturating_sub(back)]),
        };
        for index in reports {
            if Some(index) != chosen {
                self.set_standing(index, Standing::Draft);
            }
        }
        let Some(chosen) = chosen else {
            return false;
        };
        let standing = if checked {
            Standing::Answer
        } else {
            Standing::Unchecked
        };
        if self.items[chosen + 1..]
            .iter()
            .any(|item| matches!(item.kind, ItemKind::Assistant(_) | ItemKind::Step(_)))
        {
            self.move_answer(chosen, standing);
        } else {
            self.set_standing(chosen, standing);
        }
        true
    }

    /// Moves a report to the end, below the work that came after it.
    fn move_answer(&mut self, index: usize, standing: Standing) {
        let Some(item) = self.assistant_mut(index) else {
            return;
        };
        let (blocks, rest): (Vec<Block>, Vec<Block>) = std::mem::take(&mut item.blocks)
            .into_iter()
            .partition(|block| matches!(block.kind, BlockKind::Text(_)));
        item.blocks = rest;
        item.standing = Standing::Remark;
        let answer = AssistantItem {
            blocks,
            model: item.model.clone(),
            stop_reason: item.stop_reason,
            error: None,
            standing,
        };
        self.touch(index);
        self.push(ItemKind::Assistant(answer));
    }

    fn set_standing(&mut self, index: usize, standing: Standing) {
        if let Some(item) = self.assistant_mut(index) {
            item.standing = standing;
            self.touch(index);
        }
    }

    fn assistant_mut(&mut self, index: usize) -> Option<&mut AssistantItem> {
        match &mut self.items.get_mut(index)?.kind {
            ItemKind::Assistant(item) => Some(item),
            _ => None,
        }
    }
}
