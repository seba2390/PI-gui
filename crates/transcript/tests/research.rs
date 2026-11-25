//! Deep search: its report is the answer once its extension says so, live and from history.

use std::time::Instant;

use pi_rpc::protocol::{AgentMessage, Event};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use transcript::{ItemKind, ProblemKind, Standing, Transcript};

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text, "timestamp": 1})
}

fn assistant(text: &str, tools: &[&str], stop: &str) -> Value {
    let mut content = vec![];
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    for id in tools {
        content.push(json!({
            "type": "toolCall", "id": id, "name": "web_search", "arguments": {"query": "q"},
        }));
    }
    json!({"role": "assistant", "content": content, "model": "m", "provider": "p", "api": "a",
        "stopReason": stop, "timestamp": 1})
}

fn result(id: &str, deep: bool) -> Value {
    let details = if deep {
        json!({"deep": "max"})
    } else {
        json!({})
    };
    json!({"role": "toolResult", "toolCallId": id, "toolName": "web_search", "isError": false,
        "content": [{"type": "text", "text": "found"}], "details": details, "timestamp": 1})
}

fn check(text: &str, details: Value) -> Value {
    json!({"role": "custom", "customType": transcript::research::CHECK, "content": text,
        "display": details["stage"] == "final", "details": details, "timestamp": 1})
}

fn step(stage: &str, title: &str) -> Value {
    check(
        "for the model",
        json!({"stage": stage, "steps": [{"title": title, "detail": "x", "lines": ["y"]}]}),
    )
}

fn last(checked: bool, back: u64) -> Value {
    check(
        "The note.",
        json!({"stage": "final", "checked": checked, "back": back}),
    )
}

/// The messages as pi streams them.
fn events(messages: &[Value]) -> Vec<Event> {
    let mut events = vec![json!({"type": "agent_start"})];
    for message in messages {
        match message["role"].as_str().expect("valid") {
            "custom" => {
                let mut entry = message.clone();
                entry["type"] = json!("custom_message");
                events.push(json!({"type": "entry_appended", "entry": entry}));
            }
            "assistant" => {
                let mut start = message.clone();
                start["content"] = json!([]);
                start["stopReason"] = json!("pending");
                events.push(json!({"type": "message_start", "message": start}));
                for (index, block) in message["content"]
                    .as_array()
                    .expect("valid")
                    .iter()
                    .enumerate()
                {
                    if block["type"] == "text" {
                        events.push(json!({"type": "message_update", "assistantMessageEvent":
                            {"type": "text_delta", "contentIndex": index, "delta": block["text"]}}));
                    }
                }
                events.push(json!({"type": "message_end", "message": message}));
            }
            "toolResult" => {
                events.push(json!({"type": "tool_execution_end",
                    "toolCallId": message["toolCallId"], "toolName": "web_search",
                    "result": {"content": message["content"], "details": message["details"]},
                    "isError": false}));
                events.push(json!({"type": "message_start", "message": message}));
                events.push(json!({"type": "message_end", "message": message}));
            }
            _ => {
                events.push(json!({"type": "message_start", "message": message}));
                events.push(json!({"type": "message_end", "message": message}));
            }
        }
    }
    events
        .into_iter()
        .map(|event| serde_json::from_value(event).expect("valid"))
        .collect()
}

/// What the chat shows, a line per row: answers, and the work before them.
fn shown(transcript: &Transcript) -> Vec<String> {
    transcript
        .items()
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::User(user) => Some(format!("user: {}", user.text)),
            ItemKind::Assistant(assistant) => {
                let text = assistant.text();
                let mut line = match assistant.standing {
                    Standing::Answer => "answer",
                    Standing::Unchecked => "unchecked",
                    Standing::Held => "held",
                    Standing::Draft => "draft",
                    Standing::Remark => "remark",
                }
                .to_owned();
                if assistant.has_tools() {
                    line.push_str(" +tools");
                }
                (!text.is_empty() || assistant.has_tools()).then(|| format!("{line}: {text}"))
            }
            ItemKind::Notice(notice) => Some(format!("{:?}: {}", notice.kind, notice.text)),
            ItemKind::Check(check) => {
                let kind = if check.checked {
                    "checked"
                } else {
                    "not checked"
                };
                Some(format!("{kind}: {}", check.text))
            }
            ItemKind::Step(step) => Some(format!("step: {}", step.title)),
            ItemKind::Bash(_) | ItemKind::TurnSummary(_) => None,
        })
        .collect()
}

fn live(messages: &[Value]) -> Transcript {
    let mut transcript = Transcript::new();
    for event in events(messages) {
        transcript.apply(&event, Instant::now());
    }
    transcript
}

/// What is shown at the end, which must be the same when the chat is opened again.
fn at_the_end(messages: &[Value]) -> Vec<String> {
    let mut transcript = live(messages);
    let settled: Event =
        serde_json::from_value(json!({"type": "agent_settled", "aborted": false})).expect("valid");
    transcript.apply(&settled, Instant::now());
    let history: Vec<AgentMessage> = messages
        .iter()
        .map(|message| serde_json::from_value(message.clone()).expect("valid"))
        .collect();
    let reopened = shown(&Transcript::from_messages(&history));
    assert_eq!(shown(&transcript), reopened, "live and reopened");
    reopened
}

#[test]
fn a_report_is_the_answer_once_it_is_final() {
    let start = [
        user("q"),
        assistant("I will look.", &["1"], "toolUse"),
        result("1", true),
        step("plan", "Planned"),
        assistant("First report", &[], "stop"),
    ];
    let before = [
        "user: q",
        "remark +tools: I will look.",
        "step: Planned",
        "held: First report",
    ];
    assert_eq!(shown(&live(&start)), before);

    let mut whole = start.to_vec();
    whole.extend([
        step("draft", "Continuing research"),
        assistant("", &["2"], "toolUse"),
        result("2", true),
        assistant("Second report", &[], "stop"),
        step("draft", "Revising"),
        assistant("Third report", &[], "stop"),
        last(true, 0),
    ]);
    assert_eq!(
        at_the_end(&whole),
        [
            "user: q",
            "remark +tools: I will look.",
            "step: Planned",
            "draft: First report",
            "step: Continuing research",
            "remark +tools: ",
            "draft: Second report",
            "step: Revising",
            "answer: Third report",
            "checked: The note.",
        ]
    );
}

#[test]
fn a_report_does_not_flash_up_while_it_streams() {
    let messages = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("The report", &[], "stop"),
    ];
    let mut transcript = Transcript::new();
    for event in events(&messages) {
        transcript.apply(&event, Instant::now());
        for item in transcript.items() {
            if let ItemKind::Assistant(assistant) = &item.kind {
                assert_eq!(assistant.answer().unwrap_or_default(), "");
            }
        }
    }
}

#[test]
fn stopping_keeps_the_last_complete_report() {
    let stopped = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("The report", &[], "stop"),
        step("draft", "Revising"),
        assistant("", &["2"], "toolUse"),
        result("2", true),
        assistant("The rep", &[], "aborted"),
        last(false, 0),
    ];
    assert_eq!(
        at_the_end(&stopped),
        [
            "user: q",
            "remark +tools: ",
            "step: Revising",
            "remark +tools: ",
            "draft: The rep",
            "unchecked: The report",
            "not checked: The note.",
        ]
    );
}

#[test]
fn a_version_before_can_be_the_answer() {
    let messages = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("The whole report", &[], "stop"),
        step("draft", "Revising"),
        assistant("Only a bit", &[], "stop"),
        last(false, 1),
    ];
    assert_eq!(
        at_the_end(&messages),
        [
            "user: q",
            "remark +tools: ",
            "step: Revising",
            "draft: Only a bit",
            "unchecked: The whole report",
            "not checked: The note.",
        ]
    );
}

/// pi was ended before its extension could say anything.
#[test]
fn a_report_nothing_was_said_about_is_not_lost() {
    let messages = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("The report", &[], "stop"),
    ];
    let ended = [
        "user: q",
        "remark +tools: ",
        "unchecked: The report",
        "not checked: This report was not checked: it was stopped.",
    ];
    assert_eq!(at_the_end(&messages), ended);

    let mut later = messages.to_vec();
    later.extend([user("thanks"), assistant("Welcome", &[], "stop")]);
    let history: Vec<AgentMessage> = later
        .into_iter()
        .map(|message| serde_json::from_value(message).expect("valid"))
        .collect();
    let mut expected = ended.map(str::to_owned).to_vec();
    expected.extend(["user: thanks".to_owned(), "answer: Welcome".to_owned()]);
    assert_eq!(shown(&Transcript::from_messages(&history)), expected);
}

#[test]
fn a_check_says_what_it_found() {
    let about = json!({
        "effort": "max", "searches": 64, "pages": 44, "sites": 22,
        "links": {"total": 22, "broken": 0, "unread": 1},
        "claims": {"total": 59, "backed": 50, "partly": 4, "unbacked": 3, "unsourced": 1,
            "unchecked": 1},
        "problems": [
            {"kind": "partly", "text": "A claim", "why": "it says less", "urls": ["https://a.org"]},
            {"kind": "a later one", "text": "B", "why": "", "urls": []},
        ],
        "notes": ["Thinking was minimal."],
    });
    let details = json!({"stage": "final", "checked": true, "back": 0, "about": about});
    let messages = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("The report", &[], "stop"),
        check("The note.", details),
    ];
    let transcript = live(&messages);
    let Some(ItemKind::Check(check)) = transcript.items().last().map(|item| &item.kind) else {
        panic!("no check");
    };
    let about = check.about.as_ref().expect("about");
    assert_eq!((about.searches, about.links.unread), (64, 1));
    assert_eq!(about.claims.as_ref().expect("claims").backed, 50);
    let kinds: Vec<_> = about.problems.iter().map(|problem| problem.kind).collect();
    assert_eq!(kinds, [ProblemKind::Partly, ProblemKind::Other]);
}

#[test]
fn steering_does_not_end_the_research() {
    let messages = [
        user("q"),
        assistant("", &["1"], "toolUse"),
        result("1", true),
        assistant("A report", &[], "stop"),
        step("draft", "Revising"),
        user("also in Sweden"),
        assistant("A better report", &[], "stop"),
        last(true, 0),
    ];
    assert_eq!(
        at_the_end(&messages),
        [
            "user: q",
            "remark +tools: ",
            "draft: A report",
            "step: Revising",
            "user: also in Sweden",
            "answer: A better report",
            "checked: The note.",
        ]
    );
}

#[test]
fn other_chats_do_not_change() {
    let search = [
        user("q"),
        assistant("I will look.", &["1"], "toolUse"),
        result("1", false),
        assistant("The answer", &[], "stop"),
        // Notes of an extension, and of chats from before there were drafts.
        check("An old note.", json!(null)),
    ];
    let mut search = search.to_vec();
    search[4]["display"] = json!(true);
    assert_eq!(
        at_the_end(&search),
        [
            "user: q",
            "answer +tools: I will look.",
            "answer: The answer",
            "Info: An old note.",
        ]
    );
    let no_research = [user("hi"), assistant("Hello", &[], "stop")];
    assert_eq!(at_the_end(&no_research), ["user: hi", "answer: Hello"]);
}
