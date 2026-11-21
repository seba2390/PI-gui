#![allow(clippy::unwrap_used)] // test helpers may unwrap
//! Replays real pi RPC recordings (sanitized) through the reducer.
//!
//! Fixtures were captured from `pi --mode rpc` against local models; see
//! `tests/fixtures/README.md`.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use pi_rpc::protocol::{AgentMessage, Event, Incoming};
use transcript::{BlockKind, ItemKind, RunStatus, ToolStatus, Transcript, format_duration};

const STEP: Duration = Duration::from_millis(250);

struct Replay {
    live: Transcript,
    history: Option<Vec<AgentMessage>>,
}

/// Feeds every event of `fixture` with a fixed 250 ms clock step.
fn replay(fixture: &str) -> Replay {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("fixture exists");
    let start = Instant::now();
    let mut live = Transcript::new();
    let mut history = None;
    for (step, line) in text.lines().enumerate() {
        let now = start + STEP * u32::try_from(step).unwrap();
        match Incoming::from_slice(line.as_bytes()).unwrap_or_else(|e| panic!("{e}: {line}")) {
            Incoming::Event(event) => {
                assert!(
                    !matches!(*event, Event::Unknown),
                    "unknown event in fixture: {line}"
                );
                live.apply(&event, now);
            }
            Incoming::Response(response) if response.command == "get_messages" => {
                let data = response.data.unwrap();
                history = Some(serde_json::from_value(data["messages"].clone()).unwrap());
            }
            Incoming::Response(_) | Incoming::ExtensionUi(_) => {}
        }
    }
    Replay { live, history }
}

/// `value` with object keys sorted, so snapshots do not depend on whether some other crate in
/// the build enabled `serde_json`'s `preserve_order` feature (Cargo unifies features).
fn canonical(value: &serde_json::Value) -> String {
    fn sort(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                serde_json::Value::Object(
                    entries
                        .into_iter()
                        .map(|(k, v)| (k.clone(), sort(v)))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sort).collect())
            }
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

/// A stable, human-readable rendering for snapshots (timings omitted unless asked).
fn describe(transcript: &Transcript, with_timing: bool) -> String {
    let mut out = String::new();
    for item in transcript.items() {
        match &item.kind {
            ItemKind::User(user) => writeln!(out, "USER: {}", user.text).unwrap(),
            ItemKind::Assistant(assistant) => {
                writeln!(out, "ASSISTANT ({:?}):", assistant.stop_reason).unwrap();
                if let Some(error) = &assistant.error {
                    writeln!(out, "  error: {error}").unwrap();
                }
                for block in &assistant.blocks {
                    match &block.kind {
                        BlockKind::Thinking(thinking) => {
                            let timing = match (with_timing, thinking.duration) {
                                (true, Some(duration)) => {
                                    format!(" [{}]", format_duration(duration))
                                }
                                _ => String::new(),
                            };
                            writeln!(out, "  thinking{timing}: {}", thinking.text.trim()).unwrap();
                        }
                        BlockKind::Text(text) => {
                            writeln!(
                                out,
                                "  text: {}",
                                text.text.trim().replace('\n', "\n        ")
                            )
                            .unwrap();
                        }
                        BlockKind::Tool(tool) => {
                            let summary =
                                transcript::tool::result_summary(tool).unwrap_or_default();
                            writeln!(
                                out,
                                "  tool {} {:?} args={} -> {} {}",
                                tool.name,
                                tool.status,
                                canonical(&tool.args),
                                tool.output.trim().replace('\n', "⏎"),
                                summary
                            )
                            .unwrap();
                        }
                    }
                }
            }
            ItemKind::Bash { command, .. } => writeln!(out, "BASH: {command}").unwrap(),
            ItemKind::Notice(notice) => {
                writeln!(out, "NOTICE {:?}: {}", notice.kind, notice.text).unwrap();
            }
            ItemKind::TurnSummary(summary) => {
                let timing = if with_timing {
                    format_duration(summary.duration)
                } else {
                    "…".into()
                };
                writeln!(out, "WORKED {timing} aborted={}", summary.aborted).unwrap();
            }
        }
    }
    out
}

#[test]
fn tool_run_renders_thinking_tools_and_timing() {
    let replay = replay("tool_run.jsonl");
    let live = &replay.live;
    assert_eq!(*live.status(), RunStatus::Idle);
    assert!(!live.is_running());
    insta::assert_snapshot!("tool_run_live", describe(live, true));

    // Every tool finished, and the edit carries pi's diff.
    let tools: Vec<_> = live
        .items()
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Assistant(a) => Some(a.blocks.iter()),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match &block.kind {
            BlockKind::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 3);
    assert!(tools.iter().all(|tool| tool.status == ToolStatus::Done));
    assert!(tools[1].details.as_ref().unwrap().get("diff").is_some());
}

#[test]
fn history_matches_live_rendering() {
    let replay = replay("tool_run.jsonl");
    let history = Transcript::from_messages(&replay.history.expect("fixture has get_messages"));
    // Same content, independent of whether it was streamed or loaded.
    assert_eq!(describe(&history, false), describe(&replay.live, false));
}

#[test]
fn abort_run_cancels_and_marks_interrupted() {
    let replay = replay("abort_run.jsonl");
    insta::assert_snapshot!("abort_run_live", describe(&replay.live, true));
    let last = replay.live.items().last().unwrap();
    assert!(matches!(&last.kind, ItemKind::TurnSummary(summary) if summary.aborted));
}

#[test]
fn markdown_run_keeps_authoritative_text() {
    let replay = replay("markdown_run.jsonl");
    insta::assert_snapshot!("markdown_run_live", describe(&replay.live, true));
    assert!(replay.live.last_usage().is_some());
}

#[test]
fn item_versions_change_only_for_touched_items() {
    let replay = replay("tool_run.jsonl");
    let mut transcript = replay.live;
    let before: Vec<_> = transcript.items().iter().map(|item| item.version).collect();
    transcript.push_notice(transcript::NoticeKind::Info, "hello");
    let after: Vec<_> = transcript.items().iter().map(|item| item.version).collect();
    assert_eq!(&after[..before.len()], &before[..]);
    assert_eq!(after.len(), before.len() + 1);
}
