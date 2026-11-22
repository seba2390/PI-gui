//! The messages of a session file's active branch, read straight from disk.
//!
//! Opening a session starts pi, and its history only arrives once pi is up (a second or so).
//! The GUI shows this preview immediately instead, and pi's own answer replaces it if they
//! differ. So the preview must never be wrong: it follows pi's rules (walk from the leaf, the
//! last entry, to the root along `parentId`) only for what it can reproduce exactly. A branch
//! with a compaction, a context edit, a branch summary or an extension message gives `None`,
//! and the GUI waits for pi as before.

use std::collections::HashMap;

use serde_json::Value;

/// The `AgentMessage` objects on the active branch, oldest first, or `None` if this file
/// needs pi to rebuild its context (see the module docs).
pub fn active_messages(text: &str) -> Option<Vec<Value>> {
    let mut entries: HashMap<String, Value> = HashMap::new();
    let mut leaf = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // A torn last line (pi writing as we read) is skipped, not fatal.
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // The `session` header carries the session's id but is not part of the tree.
        if entry.get("type").and_then(Value::as_str) == Some("session") {
            continue;
        }
        let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        leaf = Some(id.clone());
        entries.insert(id, entry);
    }

    let mut path = Vec::new();
    let mut next = leaf;
    while let Some(id) = next {
        let entry = entries.remove(&id)?; // a cycle or a missing parent: let pi sort it out
        next = entry
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        path.push(entry);
    }
    path.reverse();

    let mut messages = Vec::new();
    for mut entry in path {
        match entry.get("type").and_then(Value::as_str)? {
            "message" => messages.push(entry.get_mut("message")?.take()),
            // Settings and bookkeeping: no message in the context.
            "model_change"
            | "thinking_level_change"
            | "usage"
            | "custom"
            | "label"
            | "session_info" => {}
            // Rewrites the context in ways pi owns.
            _ => return None,
        }
    }
    Some(messages)
}

#[cfg(test)]
mod tests {
    use super::active_messages;
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};

    fn roles(messages: &[Value]) -> Vec<String> {
        messages
            .iter()
            .map(|message| {
                let role = message["role"].as_str().unwrap_or("?");
                let text = match &message["content"] {
                    Value::String(text) => text.clone(),
                    Value::Array(blocks) => blocks
                        .first()
                        .and_then(|block| block["text"].as_str())
                        .unwrap_or("")
                        .to_owned(),
                    _ => String::new(),
                };
                format!("{role}:{text}")
            })
            .collect()
    }

    fn file(entries: &[Value]) -> String {
        let mut text = r#"{"type":"session","version":3,"id":"s","cwd":"/w"}"#.to_owned();
        for entry in entries {
            text.push('\n');
            text.push_str(&entry.to_string());
        }
        text
    }

    fn message(id: &str, parent: Option<&str>, role: &str, text: &str) -> Value {
        json!({"type":"message","id":id,"parentId":parent,"message":{"role":role,"content":text}})
    }

    #[test]
    fn follows_the_branch_ending_at_the_last_entry() {
        // a → b → c, then the user edited b: a → d (the active branch).
        let text = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"model_change","id":"m","parentId":"a","provider":"p","modelId":"x"}),
            message("b", Some("m"), "assistant", "hello"),
            message("c", Some("b"), "user", "old question"),
            message("d", Some("m"), "user", "new question"),
        ]);
        let messages = active_messages(&text).unwrap();
        assert_eq!(roles(&messages), ["user:hi", "user:new question"]);
    }

    #[test]
    fn leaves_compactions_and_edits_to_pi() {
        let compacted = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"compaction","id":"c","parentId":"a","summary":"s","firstKeptEntryId":"c"}),
        ]);
        assert_eq!(active_messages(&compacted), None);
        let edited = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"context_edit","id":"e","parentId":"a","targetId":"a","replacement":null}),
        ]);
        assert_eq!(active_messages(&edited), None);
        // Only on the active branch: an abandoned branch's compaction doesn't matter.
        let elsewhere = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"compaction","id":"c","parentId":"a","summary":"s","firstKeptEntryId":"c"}),
            message("b", Some("a"), "user", "again"),
        ]);
        assert_eq!(
            roles(&active_messages(&elsewhere).unwrap()),
            ["user:hi", "user:again"]
        );
    }

    #[test]
    fn tolerates_a_torn_last_line_and_empty_files() {
        let mut text = file(&[message("a", None, "user", "hi")]);
        text.push_str("\n{\"type\":\"message\",\"id\":\"b\",\"par");
        assert_eq!(roles(&active_messages(&text).unwrap()), ["user:hi"]);
        assert_eq!(active_messages(&file(&[])), Some(Vec::new()));
        // A broken chain is pi's to resolve.
        let orphan = file(&[message("b", Some("missing"), "user", "hi")]);
        assert_eq!(active_messages(&orphan), None);
    }
}
