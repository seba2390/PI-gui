//! Shell commands the user runs (`!cmd`): their row while they run and once they end.
#![allow(clippy::unwrap_used)] // test helpers may unwrap

use std::fmt::Write as _;
use std::time::Instant;

use pi_rpc::protocol::{AgentMessage, BashResult, Event};
use transcript::{BashItem, BashStatus, ItemKind, Transcript};

fn update(id: &str, delta: &str) -> Event {
    Event::BashExecutionUpdate {
        id: Some(id.to_owned()),
        delta: delta.to_owned(),
    }
}

fn bash(transcript: &Transcript, row: usize) -> &BashItem {
    match &transcript.items()[row].kind {
        ItemKind::Bash(bash) => bash,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_running_command_streams_its_output_and_ends() {
    let now = Instant::now();
    let mut transcript = Transcript::new();
    transcript.start_bash("gui-1", "make".to_owned(), now);
    transcript.start_bash("gui-2", "ls".to_owned(), now);
    assert!(transcript.is_bash_running());
    assert!(!transcript.is_running(), "no agent run");
    let version = transcript.items()[0].version;
    transcript.apply(&update("gui-1", "one\n"), now);
    transcript.apply(&update("gui-2", "two\n"), now);
    transcript.apply(&update("gui-1", "three\n"), now);
    transcript.apply(&update("unknown", "lost"), now);
    assert!(transcript.items()[0].version > version);
    assert_eq!(bash(&transcript, 0).output, "one\nthree\n");
    assert_eq!(bash(&transcript, 1).output, "two\n");
    assert!(bash(&transcript, 0).is_running());

    // An agent run that ends meanwhile leaves the command alone.
    transcript.apply(&Event::AgentStart, now);
    transcript.apply(&Event::AgentSettled { aborted: false }, now);
    assert!(transcript.is_bash_running());

    transcript.finish_bash(
        "gui-1",
        BashResult {
            output: "one\nthree\nfour\n".to_owned(),
            exit_code: Some(2),
            ..BashResult::default()
        },
    );
    // pi's result has it all; an update still on its way changes nothing.
    transcript.apply(&update("gui-1", "four\n"), now);
    assert_eq!(bash(&transcript, 0).output, "one\nthree\nfour\n");
    assert_eq!(
        bash(&transcript, 0).status,
        BashStatus::Done { exit_code: Some(2) }
    );
    assert!(transcript.is_bash_running(), "the other one still runs");
    transcript.stop_bash("gui-2", BashStatus::Failed);
    assert!(!transcript.is_bash_running());
    assert_eq!(bash(&transcript, 1).output, "two\n");
}

#[test]
fn a_cancelled_command_matches_its_history() {
    let mut transcript = Transcript::new();
    transcript.start_bash("gui-1", "yes".to_owned(), Instant::now());
    transcript.finish_bash(
        "gui-1",
        BashResult {
            output: "y\ny".to_owned(),
            exit_code: None,
            cancelled: true,
            truncated: true,
            full_output_path: Some("/tmp/pi-bash.log".to_owned()),
        },
    );
    assert_eq!(bash(&transcript, 0).status, BashStatus::Cancelled);
    let stored: AgentMessage = serde_json::from_value(serde_json::json!({
        "role": "bashExecution", "command": "yes", "output": "y\ny", "cancelled": true,
        "truncated": true, "fullOutputPath": "/tmp/pi-bash.log", "timestamp": 1,
    }))
    .unwrap();
    let history = Transcript::from_messages(&[stored]);
    assert_eq!(history.items()[0].kind, transcript.items()[0].kind);
}

#[test]
fn no_command_runs_on_when_pi_stops() {
    let mut transcript = Transcript::new();
    transcript.start_bash("gui-1", "sleep 100".to_owned(), Instant::now());
    transcript.force_settle(Instant::now());
    assert!(!transcript.is_bash_running());
    assert_eq!(bash(&transcript, 0).status, BashStatus::Cancelled);
}

#[test]
fn live_output_is_capped_and_shows_its_end() {
    let now = Instant::now();
    let mut transcript = Transcript::new();
    transcript.start_bash("gui-1", "seq 100000".to_owned(), now);
    for chunk in 0..1000 {
        let lines = (0..100).fold(String::new(), |mut text, line| {
            let _ = writeln!(text, "æ{}", chunk * 100 + line);
            text
        });
        transcript.apply(&update("gui-1", &lines), now);
    }
    let item = bash(&transcript, 0);
    assert!(item.output.len() <= 128 * 1024, "{}", item.output.len());
    assert!(item.output.ends_with("æ99999\n"));
    let shown: Vec<&str> = item.shown_output().lines().collect();
    assert_eq!(shown.len(), 16);
    assert_eq!((shown[0], shown[15]), ("æ99984", "æ99999"));

    // One long line is cut too.
    transcript.apply(&update("gui-1", &"æ".repeat(20_000)), now);
    let shown = bash(&transcript, 0).shown_output();
    assert!(shown.len() <= 8 * 1024 && shown.chars().all(|c| c == 'æ'));
}
