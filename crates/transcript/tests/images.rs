#![allow(clippy::unwrap_used)] // test helpers may unwrap
//! Images in tool results and prompts reach the transcript, live and from history.

use std::time::Instant;

use pi_rpc::protocol::{AgentMessage, Event, Incoming};
use transcript::{BlockKind, ItemKind, Transcript};

fn event(json: &str) -> Event {
    match Incoming::from_slice(json.as_bytes()).unwrap() {
        Incoming::Event(event) => *event,
        _ => panic!("not an event: {json}"),
    }
}

fn tool_images(transcript: &Transcript) -> Vec<(String, String)> {
    transcript
        .items()
        .iter()
        .flat_map(|item| match &item.kind {
            ItemKind::Assistant(assistant) => assistant.blocks.clone(),
            _ => Vec::new(),
        })
        .filter_map(|block| match block.kind {
            BlockKind::Tool(tool) => Some(tool.images),
            _ => None,
        })
        .flatten()
        .map(|image| (image.data.to_string(), image.mime_type))
        .collect()
}

const ASSISTANT_CALL: &str = r#"{"role":"assistant","content":[{"type":"toolCall","id":"t1","name":"read","arguments":{"path":"shot.png"}}],"model":"m","stopReason":"toolUse"}"#;

#[test]
fn live_tool_results_keep_their_images() {
    let now = Instant::now();
    let mut live = Transcript::new();
    live.apply(&event(r#"{"type":"agent_start"}"#), now);
    live.apply(
        &event(&format!(
            r#"{{"type":"message_end","message":{ASSISTANT_CALL}}}"#
        )),
        now,
    );
    live.apply(
        &event(
            r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"read","result":{"content":[{"type":"text","text":"Read image file [image/png]"},{"type":"image","data":"iVBORw0K","mimeType":"image/png"}]},"isError":false}"#,
        ),
        now,
    );
    assert_eq!(
        tool_images(&live),
        [("iVBORw0K".to_owned(), "image/png".to_owned())]
    );
}

#[test]
fn history_keeps_tool_and_prompt_images() {
    let messages: Vec<AgentMessage> = serde_json::from_str(&format!(
        r#"[
            {{"role":"user","content":[{{"type":"text","text":"what is this?"}},{{"type":"image","data":"R0lGOD","mimeType":"image/gif"}}]}},
            {ASSISTANT_CALL},
            {{"role":"toolResult","toolCallId":"t1","toolName":"read","content":[{{"type":"image","data":"/9j/4AAQ","mimeType":"image/jpeg"}}],"isError":false}}
        ]"#
    ))
    .unwrap();
    let history = Transcript::from_messages(&messages);
    let ItemKind::User(user) = &history.items()[0].kind else {
        panic!("first item is the prompt");
    };
    assert_eq!(user.text, "what is this?");
    assert_eq!(user.images.len(), 1);
    assert_eq!(&*user.images[0].data, "R0lGOD");
    assert_eq!(user.images[0].mime_type, "image/gif");
    assert_eq!(
        tool_images(&history),
        [("/9j/4AAQ".to_owned(), "image/jpeg".to_owned())]
    );
}
