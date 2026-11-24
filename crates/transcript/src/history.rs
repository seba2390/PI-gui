//! Rebuilding a transcript from a session's stored messages (`get_messages`).

use std::time::{Duration, Instant};

use pi_rpc::protocol::{AgentMessage, StopReason};

use crate::model::TurnSummary;
use crate::reducer::Transcript;

impl Transcript {
    /// Builds a finished transcript from history. Each user prompt followed by
    /// agent work gets a "Worked for" summary computed from message timestamps,
    /// since pi stores no run timing.
    pub fn from_messages(messages: &[AgentMessage]) -> Self {
        let now = Instant::now();
        let mut transcript = Self::new();
        let mut turn = TurnTiming::default();

        for message in messages {
            if let AgentMessage::User(user) = message {
                transcript.end_interrupted_research();
                if let Some(summary) = turn.finish() {
                    transcript.push_turn_summary(summary);
                }
                turn = TurnTiming {
                    start: user.timestamp,
                    ..TurnTiming::default()
                };
            } else {
                turn.observe(message);
            }
            transcript.push_message(message, now);
        }
        transcript.end_research();
        if let Some(summary) = turn.finish() {
            transcript.push_turn_summary(summary);
        }
        transcript
    }
}

#[derive(Default)]
struct TurnTiming {
    start: Option<i64>,
    end: Option<i64>,
    saw_assistant: bool,
    aborted: bool,
}

impl TurnTiming {
    fn observe(&mut self, message: &AgentMessage) {
        let end = match message {
            AgentMessage::Assistant(assistant) => {
                self.saw_assistant = true;
                self.aborted = assistant.stop_reason == StopReason::Aborted;
                let duration = assistant.duration_ms.and_then(|ms| i64::try_from(ms).ok());
                assistant
                    .timestamp
                    .map(|start| start + duration.unwrap_or(0))
            }
            AgentMessage::ToolResult(result) => result.timestamp,
            _ => None,
        };
        if let Some(end) = end {
            self.end = Some(self.end.map_or(end, |current| current.max(end)));
        }
    }

    fn finish(&self) -> Option<TurnSummary> {
        if !self.saw_assistant {
            return None;
        }
        let millis = self.end?.checked_sub(self.start?)?;
        Some(TurnSummary {
            duration: Duration::from_millis(u64::try_from(millis).ok()?),
            aborted: self.aborted,
        })
    }
}
