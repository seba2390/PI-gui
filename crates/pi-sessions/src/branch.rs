//! The messages of a session file's active branch, read straight from disk.
//!
//! Opening a session starts pi, and its history only arrives once pi is up (a second or so).
//! The GUI shows this preview immediately instead, and pi's own answer replaces it if they
//! differ. So the preview must never be wrong: it follows pi's rules (walk from the leaf, the
//! last entry, to the root along `parentId`; then pi's projection of the entries into
//! messages: the latest compaction's summary and kept entries, branch summaries, extension
//! messages and context edits) only for what it can reproduce exactly. Anything else (an
//! entry type it doesn't know, a broken chain, an odd value) gives `None`, and the GUI waits
//! for pi as before.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

/// Entry types that put no message in the context.
const NO_MESSAGE: [&str; 7] = [
    "model_change",
    "thinking_level_change",
    "usage",
    "custom",
    "label",
    "session_info",
    "context_edit",
];

/// The `AgentMessage` objects on the active branch, oldest first, or `None` if this file
/// needs pi to rebuild its context (see the module docs).
pub fn active_messages(text: &str) -> Option<Vec<Value>> {
    Branch::parse(text)?.messages()
}

/// A session file's active branch.
pub struct Branch {
    path: Vec<Value>,
}

/// What a session's branch selected, as pi restores it when the session is resumed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// `(provider, model id)`: the last model change or answering model (pi's
    /// `getBranchSelection`, without its virtual models).
    pub model: Option<(String, String)>,
    /// The last thinking level change, if any.
    pub thinking_level: Option<String>,
    /// The `data` of the branch's `custom` entries of the type asked for, oldest first.
    pub custom: Vec<Value>,
}

impl Branch {
    /// The branch ending at the file's last entry, or `None` for a broken chain.
    pub fn parse(text: &str) -> Option<Self> {
        active_path(text).map(|path| Self { path })
    }

    /// See [`active_messages`].
    pub fn messages(&self) -> Option<Vec<Value>> {
        let context = context_entries(&self.path);
        // The latest edit of each entry.
        let mut edits: HashMap<&str, &Value> = HashMap::new();
        for entry in &context {
            if kind(entry) == Some("context_edit") {
                edits.insert(entry.get("targetId")?.as_str()?, entry);
            }
        }
        let mut messages = Vec::new();
        for (index, entry) in context.iter().enumerate() {
            // An older compaction kept in the range only marks its place.
            if index > 0 && kind(entry) == Some("compaction") {
                continue;
            }
            let edit = entry
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| edits.get(id));
            messages.extend(project(entry, edit.copied())?);
        }
        Some(messages)
    }

    /// The model, thinking level and `custom_type` entries on the branch.
    pub fn selection(&self, custom_type: &str) -> Selection {
        let text =
            |entry: &Value, key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        let mut selection = Selection::default();
        for entry in &self.path {
            match kind(entry) {
                Some("model_change") => {
                    selection.model = text(entry, "provider").zip(text(entry, "modelId"));
                }
                Some("message") if entry["message"]["role"] == "assistant" => {
                    let message = &entry["message"];
                    selection.model = text(message, "provider").zip(text(message, "model"));
                }
                Some("thinking_level_change") => {
                    selection.thinking_level = text(entry, "thinkingLevel");
                }
                Some("custom") if entry["customType"] == custom_type => {
                    selection
                        .custom
                        .push(entry.get("data").cloned().unwrap_or(Value::Null));
                }
                _ => {}
            }
        }
        selection
    }
}

fn kind(entry: &Value) -> Option<&str> {
    entry.get("type").and_then(Value::as_str)
}

/// The entries from the root to the leaf (the file's last entry).
fn active_path(text: &str) -> Option<Vec<Value>> {
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
        if kind(&entry) == Some("session") {
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
    Some(path)
}

/// pi's `buildContextEntries`: with a compaction on the path, the latest one, then the
/// entries it kept (from `firstKeptEntryId`, without system messages), then everything
/// after it.
fn context_entries(path: &[Value]) -> Vec<&Value> {
    let Some(at) = path
        .iter()
        .rposition(|entry| kind(entry) == Some("compaction"))
    else {
        return path.iter().collect();
    };
    let compaction = &path[at];
    let first_kept = compaction.get("firstKeptEntryId");
    let mut context = vec![compaction];
    let mut kept = false;
    for entry in &path[..at] {
        if first_kept.is_some() && entry.get("id") == first_kept {
            kept = true;
        }
        let system = kind(entry) == Some("message") && entry["message"]["role"] == "system";
        if kept && !system {
            context.push(entry);
        }
    }
    context.extend(&path[at + 1..]);
    context
}

/// pi's `projectContextEntry`: the messages `entry` puts in the context, with its latest
/// `edit` applied.
fn project(entry: &Value, edit: Option<&Value>) -> Option<Vec<Value>> {
    let messages = entry_messages(entry)?;
    let Some(edit) = edit else {
        return Some(messages);
    };
    let replacement = edit.get("replacement")?;
    if replacement.is_null() {
        return Some(Vec::new());
    }
    let content = replacement.get("content")?;
    messages
        .into_iter()
        .map(|mut message| {
            let role = message.get("role").and_then(Value::as_str)?.to_owned();
            if !matches!(
                role.as_str(),
                "user" | "assistant" | "toolResult" | "custom"
            ) {
                return Some(message);
            }
            let content = match content {
                Value::String(text) if matches!(role.as_str(), "assistant" | "toolResult") => {
                    json!([{ "type": "text", "text": text }])
                }
                other => other.clone(),
            };
            message.as_object_mut()?.insert("content".into(), content);
            Some(message)
        })
        .collect()
}

/// pi's `sessionEntryToContextMessages`.
fn entry_messages(entry: &Value) -> Option<Vec<Value>> {
    let entry_kind = kind(entry)?;
    match entry_kind {
        "message" => {
            let mut message = entry.get("message")?.clone();
            let role = message.get("role").and_then(Value::as_str)?.to_owned();
            let object = message.as_object_mut()?;
            if object.get("content").is_none_or(Value::is_null) {
                match role.as_str() {
                    "system" => drop(object.insert("content".into(), json!(""))),
                    "user" | "assistant" | "toolResult" => {
                        drop(object.insert("content".into(), json!([])));
                    }
                    _ => {}
                }
            }
            Some(vec![message])
        }
        "custom_message" => {
            let mut message = Map::new();
            message.insert("role".into(), json!("custom"));
            message.insert("customType".into(), entry.get("customType")?.clone());
            let content = entry.get("content").filter(|content| !content.is_null());
            message.insert("content".into(), content.cloned().unwrap_or(json!([])));
            for key in ["display", "details"] {
                if let Some(value) = entry.get(key) {
                    message.insert(key.into(), value.clone());
                }
            }
            message.insert("timestamp".into(), timestamp(entry)?);
            Some(vec![Value::Object(message)])
        }
        "branch_summary" => {
            let summary = entry.get("summary")?;
            // An empty summary adds nothing.
            if summary.as_str().is_none_or(str::is_empty) {
                return if summary.is_null() || summary == "" {
                    Some(Vec::new())
                } else {
                    None
                };
            }
            let mut message = Map::new();
            message.insert("role".into(), json!("branchSummary"));
            message.insert("summary".into(), summary.clone());
            if let Some(from) = entry.get("fromId") {
                message.insert("fromId".into(), from.clone());
            }
            message.insert("timestamp".into(), timestamp(entry)?);
            Some(vec![Value::Object(message)])
        }
        "compaction" => {
            let mut summary = Map::new();
            summary.insert("role".into(), json!("compactionSummary"));
            summary.insert("summary".into(), entry.get("summary")?.clone());
            if let Some(tokens) = entry.get("tokensBefore") {
                summary.insert("tokensBefore".into(), tokens.clone());
            }
            summary.insert("timestamp".into(), timestamp(entry)?);
            let system = entry
                .get("systemMessage")
                .filter(|system| !system.is_null());
            Some(
                system
                    .cloned()
                    .into_iter()
                    .chain([Value::Object(summary)])
                    .collect(),
            )
        }
        _ if NO_MESSAGE.contains(&entry_kind) => Some(Vec::new()),
        // A kind this doesn't know: pi's to rebuild.
        _ => None,
    }
}

/// An entry's `timestamp` in milliseconds since 1970, as JavaScript's `Date` reads it: a
/// number as it is, an ISO 8601 date and time (what pi writes) converted.
fn timestamp(entry: &Value) -> Option<Value> {
    match entry.get("timestamp")? {
        Value::Number(number) => Some(Value::Number(number.clone())),
        Value::String(text) => iso_millis(text).map(Value::from),
        _ => None,
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` or with a `±HH:MM` offset, in milliseconds since 1970.
fn iso_millis(text: &str) -> Option<i64> {
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let end = fraction
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(fraction.len());
        // JavaScript keeps milliseconds: further digits are dropped.
        let kept = &fraction[..end.min(3)];
        if kept.is_empty() {
            return None;
        }
        millis = kept.parse::<i64>().ok()? * 10_i64.pow(3 - u32::try_from(kept.len()).ok()?);
        rest = &fraction[end..];
    }
    let offset = match rest {
        "Z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = rest.get(1..)?;
            if zone.len() != 5 || zone.as_bytes()[2] != b':' {
                return None;
            }
            let hours: i64 = zone.get(0..2)?.parse().ok()?;
            let minutes: i64 = zone.get(3..5)?.parse().ok()?;
            sign * (hours * 60 + minutes)
        }
    };
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset * 60;
    Some(seconds * 1_000 + millis)
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
    fn a_compaction_keeps_its_summary_and_the_kept_entries() {
        let text = file(&[
            message("a", None, "user", "first"),
            message("b", Some("a"), "assistant", "answer"),
            message("c", Some("b"), "user", "kept"),
            json!({"type":"compaction","id":"k","parentId":"c","summary":"sum",
                   "firstKeptEntryId":"c","tokensBefore":1200,
                   "timestamp":"2026-11-23T09:00:00.250Z"}),
            message("d", Some("k"), "user", "after"),
        ]);
        let messages = active_messages(&text).unwrap();
        assert_eq!(
            roles(&messages),
            ["compactionSummary:", "user:kept", "user:after"]
        );
        assert_eq!(
            messages[0],
            json!({"role":"compactionSummary","summary":"sum","tokensBefore":1200,
                   "timestamp":1_795_424_400_250_i64})
        );
        // Only on the active branch: an abandoned branch's compaction doesn't matter.
        let elsewhere = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"compaction","id":"c","parentId":"a","summary":"s","firstKeptEntryId":"c",
                   "timestamp":"2026-11-23T09:00:00.000Z"}),
            message("b", Some("a"), "user", "again"),
        ]);
        assert_eq!(
            roles(&active_messages(&elsewhere).unwrap()),
            ["user:hi", "user:again"]
        );
    }

    #[test]
    fn context_edits_replace_or_drop_what_they_target() {
        let text = file(&[
            message("a", None, "user", "hi"),
            message("b", Some("a"), "toolResult", "a long page"),
            message("c", Some("b"), "assistant", "gone"),
            json!({"type":"context_edit","id":"e1","parentId":"c","targetId":"b",
                   "replacement":{"content":"first"}}),
            json!({"type":"context_edit","id":"e2","parentId":"e1","targetId":"b",
                   "replacement":{"content":"short"}}),
            json!({"type":"context_edit","id":"e3","parentId":"e2","targetId":"c",
                   "replacement":null}),
        ]);
        let messages = active_messages(&text).unwrap();
        assert_eq!(roles(&messages), ["user:hi", "toolResult:short"]);
        assert_eq!(
            messages[1]["content"],
            json!([{"type":"text","text":"short"}])
        );
    }

    #[test]
    fn extension_messages_and_branch_summaries() {
        let text = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"custom_message","id":"x","parentId":"a","customType":"note",
                   "content":"from an extension","display":true,
                   "timestamp":"1970-01-01T00:00:01.000Z"}),
            json!({"type":"branch_summary","id":"s","parentId":"x","summary":"tried",
                   "fromId":"q","timestamp":"1970-01-01T00:00:02+01:00"}),
            json!({"type":"custom","id":"y","parentId":"s","customType":"state","data":{}}),
        ]);
        let messages = active_messages(&text).unwrap();
        assert_eq!(
            messages[1],
            json!({"role":"custom","customType":"note","content":"from an extension",
                   "display":true,"timestamp":1000})
        );
        assert_eq!(
            messages[2],
            json!({"role":"branchSummary","summary":"tried","fromId":"q","timestamp":-3_598_000})
        );
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn leaves_what_it_does_not_know_to_pi() {
        let unknown = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"something_new","id":"n","parentId":"a"}),
        ]);
        assert_eq!(active_messages(&unknown), None);
        let odd_time = file(&[
            json!({"type":"custom_message","id":"x","customType":"note","content":"x",
                   "timestamp":"yesterday"}),
        ]);
        assert_eq!(active_messages(&odd_time), None);
    }

    #[test]
    fn the_selection_is_the_last_change_on_the_branch() {
        let text = file(&[
            message("a", None, "user", "hi"),
            json!({"type":"model_change","id":"m","parentId":"a","provider":"p","modelId":"x"}),
            json!({"type":"thinking_level_change","id":"t","parentId":"m","thinkingLevel":"high"}),
            json!({"type":"message","id":"b","parentId":"t",
                   "message":{"role":"assistant","content":[],"provider":"q","model":"y"}}),
            json!({"type":"custom","id":"w","parentId":"b","customType":"web","data":{"mode":"deep"}}),
            json!({"type":"custom","id":"o","parentId":"w","customType":"other","data":{}}),
            // Another branch's change doesn't count.
            json!({"type":"model_change","id":"z","parentId":"b","provider":"r","modelId":"z"}),
            message("c", Some("o"), "user", "more"),
        ]);
        let selection = super::Branch::parse(&text).unwrap().selection("web");
        assert_eq!(selection.model, Some(("q".into(), "y".into())));
        assert_eq!(selection.thinking_level.as_deref(), Some("high"));
        assert_eq!(selection.custom, [json!({"mode":"deep"})]);
    }

    #[test]
    fn reads_times_like_javascript() {
        assert_eq!(super::iso_millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            super::iso_millis("2000-03-01T00:00:00Z"),
            Some(951_868_800_000)
        );
        assert_eq!(
            super::iso_millis("2026-11-23T09:00:00.1234Z"),
            Some(1_795_424_400_123)
        );
        assert_eq!(
            super::iso_millis("2026-11-23T10:00:00+01:00"),
            Some(1_795_424_400_000)
        );
        assert_eq!(super::iso_millis("2026-13-23T10:00:00Z"), None);
        assert_eq!(super::iso_millis("2026-11-23 10:00:00Z"), None);
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
