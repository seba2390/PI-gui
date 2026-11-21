//! End-to-end test of spawning a pi-compatible process and driving it over RPC.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use futures::executor::block_on;
use pi_rpc::protocol::{AssistantMessageEvent, Event};
use pi_rpc::{ClientEvent, PiProcess, RpcError, SpawnOptions};

fn fake_pi() -> SpawnOptions {
    let mut options = SpawnOptions::new(std::env::temp_dir());
    options.program = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_pi.py");
    options
}

#[test]
fn spawns_queries_streams_and_shuts_down() {
    let (process, events) = PiProcess::spawn(&fake_pi()).unwrap();
    let client = process.client().clone();

    let state = block_on(client.get_state()).unwrap();
    assert_eq!(state.model.unwrap().qualified_id(), "test/fake-1");
    assert!(matches!(
        block_on(client.set_model("x".into(), "y".into())),
        Err(RpcError::Rejected { .. })
    ));

    block_on(client.prompt("hi".into(), Vec::new(), None)).unwrap();
    let mut text = String::new();
    loop {
        match block_on(events.recv()).unwrap() {
            ClientEvent::Event(event) => match *event {
                Event::MessageUpdate {
                    assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
                    ..
                } => text.push_str(&delta),
                Event::AgentSettled { aborted } => {
                    assert!(!aborted);
                    break;
                }
                _ => {}
            },
            other => panic!("unexpected {other:?}"),
        }
    }
    // U+2028 inside a JSON string must not split the record.
    assert_eq!(text, "Hello \u{2028} world");

    drop(process);
    // Closing stdin ends the fake; the reader then reports the disconnect.
    loop {
        if block_on(events.recv()).unwrap() == ClientEvent::Disconnected {
            break;
        }
    }
    assert!(!client.is_connected());
}

#[test]
fn missing_executable_is_an_io_error() {
    let mut options = fake_pi();
    options.program = PathBuf::from("/definitely/not/pi");
    assert!(PiProcess::spawn(&options).is_err());
}
