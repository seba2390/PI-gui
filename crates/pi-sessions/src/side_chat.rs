//! Side chats: a new session that starts with everything another one holds at that moment,
//! opened next to it while it keeps working.
//!
//! pi's own copy (`--fork`, `/clone`) takes what is in the session file, and a run still in
//! progress has only its finished messages there. So the side chat's file is written here:
//! pi's header with a new id (its `parentSession` is the original), every entry of the
//! original as it is, and then what the original's run was still doing (the reply being
//! written, tools still running; see `transcript::InFlight`) as messages of its own. pi
//! then opens that file like any other (`--session`).
//!
//! The header also says `"pistachioSideChat": true`, so the sidebar can list the side chat
//! under the conversation it came from ([`nest`]). pi keeps unknown header fields.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::SessionSummary;

/// The header field that marks a side chat.
pub const MARKER: &str = "pistachioSideChat";
/// The header field with how many prompts the side chat took over: the app shows them folded,
/// up to the first prompt of the side chat's own.
pub const INHERITED_PROMPTS: &str = "pistachioInheritedPrompts";

/// pi's session format version this module writes when it starts a file from messages.
const VERSION: u64 = 3;

/// Where a side chat's history comes from.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    /// The original's session file, as read now (pi may be appending to it: a torn last line
    /// is left out).
    File(&'a str),
    /// The original's messages (pi's `get_messages`), for a session whose file pi has not
    /// written yet (it does so with the first reply).
    Messages(&'a [Value]),
}

/// A side chat's session file, ready to be written next to the original's.
#[derive(Debug, Clone, PartialEq)]
pub struct SideChatFile {
    /// `<timestamp>_<id>.jsonl`, as pi names its files.
    pub file_name: String,
    pub text: String,
    /// The prompts it takes over, as pi will show them (after the last compaction).
    pub prompts: usize,
}

/// Builds the side chat of the session at `parent` (in `cwd`): its history from `source`,
/// followed by `tail`, the messages of the run in progress that pi has not saved yet. Messages
/// of `tail` that the file already has (pi saved them meanwhile) are left out.
///
/// # Errors
/// When the file has no session header.
pub fn build(
    source: Source<'_>,
    parent: &Path,
    cwd: &Path,
    tail: &[Value],
    now: SystemTime,
) -> Result<SideChatFile, String> {
    let mut ids = HashSet::new();
    let mut lines = Vec::new();
    let mut last = None;
    let mut saved = Saved::default();
    let mut entries: Vec<Value> = Vec::new();
    let (version, cwd) = match source {
        Source::File(text) => {
            let mut header = None;
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                // A torn last line (pi writing as we read) is left out.
                let Ok(entry) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if entry.get("type").and_then(Value::as_str) == Some("session") {
                    header.get_or_insert(entry);
                    continue;
                }
                let Some(id) = entry.get("id").and_then(Value::as_str) else {
                    continue;
                };
                ids.insert(id.to_owned());
                last = Some(id.to_owned());
                if let Some(message) = entry.get("message") {
                    saved.add(message);
                }
                lines.push(line.to_owned());
                entries.push(entry);
            }
            let header = header.ok_or("The conversation's file has no session header.")?;
            let version = header.get("version").and_then(Value::as_u64).unwrap_or(1);
            let cwd = header
                .get("cwd")
                .and_then(Value::as_str)
                .map_or_else(|| cwd.to_string_lossy().into_owned(), str::to_owned);
            (version, cwd)
        }
        Source::Messages(messages) => {
            entries.extend(
                messages
                    .iter()
                    .map(|message| json!({"type": "message", "message": message})),
            );
            for message in messages {
                saved.add(message);
                lines.push(message_entry(message, &mut ids, &mut last, now));
            }
            (VERSION, cwd.to_string_lossy().into_owned())
        }
    };
    for message in tail.iter().filter(|message| !saved.has(message)) {
        lines.push(message_entry(message, &mut ids, &mut last, now));
    }

    let id = uuid_v7(now);
    let timestamp = iso_time(now);
    let mut header = Map::new();
    header.insert("type".into(), "session".into());
    header.insert("version".into(), version.into());
    header.insert("id".into(), id.clone().into());
    header.insert("timestamp".into(), timestamp.clone().into());
    header.insert("cwd".into(), cwd.into());
    header.insert(
        "parentSession".into(),
        parent.to_string_lossy().into_owned().into(),
    );
    let prompts = match source {
        Source::File(_) => prompts_on_branch(&entries),
        Source::Messages(_) => entries.iter().filter(|entry| is_prompt(entry)).count(),
    };
    header.insert(MARKER.into(), true.into());
    header.insert(INHERITED_PROMPTS.into(), prompts.into());

    let mut text = Value::Object(header).to_string();
    text.push('\n');
    for line in lines {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(SideChatFile {
        file_name: format!("{}_{id}.jsonl", timestamp.replace([':', '.'], "-")),
        text,
        prompts,
    })
}

/// Whether `entry` is a prompt (a user message).
fn is_prompt(entry: &Value) -> bool {
    entry.get("type").and_then(Value::as_str) == Some("message")
        && entry.pointer("/message/role").and_then(Value::as_str) == Some("user")
}

/// The prompts pi shows for the active branch (from the last entry to the root along
/// `parentId`): those kept by the last compaction, or all without one.
fn prompts_on_branch(entries: &[Value]) -> usize {
    let id = |entry: &Value| entry.get("id").and_then(Value::as_str).map(str::to_owned);
    let by_id: HashMap<String, &Value> = entries
        .iter()
        .filter_map(|entry| Some((id(entry)?, entry)))
        .collect();
    let mut path = Vec::new();
    let mut next = entries.last().and_then(id);
    while let Some(at) = next {
        // A cycle or a missing parent ends the walk.
        let Some(entry) = by_id.get(&at) else { break };
        if path.len() > entries.len() {
            break;
        }
        path.push(*entry);
        next = entry
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    path.reverse();
    let kept_from = path
        .iter()
        .rposition(|entry| entry.get("type").and_then(Value::as_str) == Some("compaction"))
        .map_or(0, |compaction| {
            let first = path[compaction]
                .get("firstKeptEntryId")
                .and_then(Value::as_str);
            path.iter()
                .position(|entry| entry.get("id").and_then(Value::as_str) == first)
                .unwrap_or(compaction)
        });
    path[kept_from..]
        .iter()
        .filter(|entry| is_prompt(entry))
        .count()
}

/// The path of the side chat's file: next to the original's (pi's folder for that working
/// folder). `/`-separated, so it is right for a path on an SSH host too.
pub fn path_beside(parent: &Path, file_name: &str) -> PathBuf {
    match parent.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(file_name),
        _ => PathBuf::from(file_name),
    }
}

/// Which messages a session file has already, to recognise the ones of a run that pi saved
/// while the side chat was being made.
#[derive(Debug, Default)]
struct Saved {
    /// Replies by when they started (pi's `timestamp`, Unix ms).
    replies: HashSet<i64>,
    /// Tool results by their call.
    results: HashSet<String>,
}

impl Saved {
    fn add(&mut self, message: &Value) {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(at) = message.get("timestamp").and_then(Value::as_i64) {
                    self.replies.insert(at);
                }
            }
            Some("toolResult") => {
                if let Some(id) = message.get("toolCallId").and_then(Value::as_str) {
                    self.results.insert(id.to_owned());
                }
            }
            _ => {}
        }
    }

    fn has(&self, message: &Value) -> bool {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => message
                .get("timestamp")
                .and_then(Value::as_i64)
                .is_some_and(|at| self.replies.contains(&at)),
            Some("toolResult") => message
                .get("toolCallId")
                .and_then(Value::as_str)
                .is_some_and(|id| self.results.contains(id)),
            _ => false,
        }
    }
}

/// A `message` entry for `message`, following `last`.
fn message_entry(
    message: &Value,
    ids: &mut HashSet<String>,
    last: &mut Option<String>,
    now: SystemTime,
) -> String {
    let id = entry_id(ids);
    let entry = json!({
        "type": "message",
        "id": id,
        "parentId": last.clone(),
        "timestamp": iso_time(now),
        "message": message,
    });
    *last = Some(id);
    entry.to_string()
}

/// A new entry id: 8 hex digits, like pi's, unique in the file.
fn entry_id(ids: &mut HashSet<String>) -> String {
    loop {
        let id = format!("{:08x}", fastrand::u32(..));
        if ids.insert(id.clone()) {
            return id;
        }
    }
}

/// A UUID version 7 (time-ordered), as pi names sessions.
fn uuid_v7(now: SystemTime) -> String {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let ms = u64::try_from(ms).unwrap_or(u64::MAX) & 0xffff_ffff_ffff;
    let random = fastrand::u128(..);
    let mut value = (u128::from(ms) << 80) | (random & ((1 << 80) - 1));
    // Version 7 and the RFC 4122 variant.
    value = (value & !(0xf << 76)) | (0x7 << 76);
    value = (value & !(0x3 << 62)) | (0x2 << 62);
    let hex = format!("{value:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `2026-11-25T10:20:30.123Z`, like JavaScript's `toISOString`.
pub fn iso_time(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since.subsec_millis()
    )
}

/// Year, month and day of the `days`th day after 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// One sidebar row of a nested list: a session and where it sits under the one it is a side
/// chat of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nested<'a> {
    pub session: &'a SessionSummary,
    /// 0 for a conversation, 1 for its side chats, 2 for theirs…
    pub depth: usize,
    /// The last side chat of its conversation (its line ends here).
    pub last: bool,
    /// For each level above this one (from 1): whether the line of that level goes on past
    /// this row (a later side chat of that ancestor follows).
    pub through: Vec<bool>,
}

/// `sessions` (in their order) with each side chat moved right under the conversation it
/// came from, after the ones before it in `sessions`. A side chat whose conversation is not
/// in `sessions` stays where it is, as a conversation of its own.
pub fn nest<'a>(sessions: impl IntoIterator<Item = &'a SessionSummary>) -> Vec<Nested<'a>> {
    let sessions: Vec<&SessionSummary> = sessions.into_iter().collect();
    let index: HashMap<&Path, usize> = sessions
        .iter()
        .enumerate()
        .map(|(i, session)| (session.path.as_path(), i))
        .collect();
    let parent_of = |i: usize| {
        sessions[i]
            .side_chat_of
            .as_deref()
            .and_then(|parent| index.get(parent).copied())
            .filter(|&parent| parent != i)
    };
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); sessions.len()];
    for i in 0..sessions.len() {
        if let Some(parent) = parent_of(i) {
            children[parent].push(i);
        }
    }
    let mut out = Vec::with_capacity(sessions.len());
    let mut seen = vec![false; sessions.len()];
    // Roots first; anything left (side chats of each other in a loop) after, as roots.
    let roots: Vec<usize> = (0..sessions.len())
        .filter(|&i| parent_of(i).is_none())
        .collect();
    for root in roots.into_iter().chain(0..sessions.len()) {
        if seen[root] {
            continue;
        }
        let mut stack = vec![(root, 0usize, true, Vec::<bool>::new())];
        while let Some((i, depth, last, through)) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            out.push(Nested {
                session: sessions[i],
                depth,
                last,
                through: through.clone(),
            });
            let kids: Vec<usize> = children[i].iter().copied().filter(|&c| !seen[c]).collect();
            let mut below = through;
            if depth > 0 {
                below.push(!last);
            }
            for (n, &child) in kids.iter().enumerate().rev() {
                stack.push((child, depth + 1, n + 1 == kids.len(), below.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    const NOW: Duration = Duration::from_millis(1_764_065_430_123);

    fn lines(text: &str) -> Vec<Value> {
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn formats_times_like_javascript() {
        assert_eq!(iso_time(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_time(UNIX_EPOCH + NOW), "2025-11-25T10:10:30.123Z");
        assert_eq!(
            iso_time(UNIX_EPOCH + Duration::from_hours(264_384)),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn session_ids_are_uuid_v7() {
        let id = uuid_v7(UNIX_EPOCH + NOW);
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "7");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(&id[..8], &format!("{:012x}", NOW.as_millis())[..8]);
    }

    #[test]
    fn copies_the_file_and_adds_what_is_not_saved_yet() {
        let source = [
            r#"{"type":"session","version":3,"id":"old","timestamp":"t","cwd":"/w"}"#,
            r#"{"type":"message","id":"a1","parentId":null,"timestamp":"t","message":{"role":"user","content":"hi","timestamp":1}}"#,
            r#"{"type":"label","id":"a2","parentId":"a1","timestamp":"t","targetId":"a1","label":"x"}"#,
            r#"{"type":"message","id":"a3","parentId":"a2","timestamp":"t","message":{"role":"assistant","content":[],"timestamp":5,"stopReason":"toolUse"}}"#,
            // Torn: pi was writing it.
            r#"{"type":"message","id":"a4","pa"#,
        ]
        .join("\n");
        let tail = [
            // Saved already (same start), so left out.
            json!({"role": "assistant", "timestamp": 5, "content": []}),
            json!({"role": "toolResult", "toolCallId": "c1", "content": []}),
        ];
        let file = build(
            Source::File(&source),
            Path::new("/s/--w--/old.jsonl"),
            Path::new("/elsewhere"),
            &tail,
            UNIX_EPOCH + NOW,
        )
        .unwrap();
        let entries = lines(&file.text);
        assert_eq!(entries.len(), 5);
        let header = &entries[0];
        assert_eq!(header["type"], "session");
        assert_eq!(header["version"], 3);
        assert_eq!(header["cwd"], "/w");
        assert_eq!(header["parentSession"], "/s/--w--/old.jsonl");
        assert_eq!(header[MARKER], true);
        assert_eq!(header[INHERITED_PROMPTS], 1);
        assert_eq!(file.prompts, 1);
        assert_eq!(header["timestamp"], "2025-11-25T10:10:30.123Z");
        let id = header["id"].as_str().unwrap();
        assert_ne!(id, "old");
        assert_eq!(
            file.file_name,
            format!("2025-11-25T10-10-30-123Z_{id}.jsonl")
        );
        // The original's entries, unchanged.
        let kept: Vec<&str> = source.lines().skip(1).take(3).collect();
        assert_eq!(&entries[1..4], &lines(&kept.join("\n"))[..]);
        let added = &entries[4];
        assert_eq!(added["type"], "message");
        assert_eq!(added["parentId"], "a3");
        assert_eq!(added["message"], tail[1]);
    }

    #[test]
    fn counts_the_prompts_pi_shows() {
        let entry = |id: &str, parent: Option<&str>, role: &str| {
            json!({"type": "message", "id": id, "parentId": parent,
                   "message": {"role": role, "content": "x"}})
        };
        let branched = [
            entry("a", None, "user"),
            entry("b", Some("a"), "assistant"),
            // Another branch, not on the path to the last entry.
            entry("c", Some("b"), "user"),
            entry("d", Some("b"), "user"),
            entry("e", Some("d"), "assistant"),
        ];
        assert_eq!(prompts_on_branch(&branched), 2);
        let compacted = [
            entry("a", None, "user"),
            entry("b", Some("a"), "user"),
            entry("c", Some("b"), "user"),
            json!({"type": "compaction", "id": "k", "parentId": "c", "firstKeptEntryId": "c"}),
            entry("d", Some("k"), "user"),
        ];
        assert_eq!(prompts_on_branch(&compacted), 2);
    }

    #[test]
    fn starts_from_messages_when_there_is_no_file_yet() {
        let history = [json!({"role": "user", "content": "hi", "timestamp": 1})];
        let tail = [json!({"role": "assistant", "content": [], "timestamp": 2})];
        let file = build(
            Source::Messages(&history),
            Path::new("/s/x.jsonl"),
            Path::new("/w"),
            &tail,
            UNIX_EPOCH + NOW,
        )
        .unwrap();
        let entries = lines(&file.text);
        assert_eq!(entries[0]["cwd"], "/w");
        assert_eq!(entries[0]["version"], 3);
        assert_eq!(entries[1]["parentId"], Value::Null);
        assert_eq!(entries[1]["message"], history[0]);
        assert_eq!(entries[2]["parentId"], entries[1]["id"]);
        assert_eq!(entries[2]["message"], tail[0]);
    }

    #[test]
    fn a_file_without_header_is_refused() {
        let source = r#"{"type":"message","id":"a1","parentId":null}"#;
        let result = build(
            Source::File(source),
            Path::new("/x"),
            Path::new("/w"),
            &[],
            UNIX_EPOCH,
        );
        assert!(result.is_err());
    }

    #[test]
    fn the_file_goes_next_to_the_original() {
        assert_eq!(
            path_beside(Path::new("/s/--w--/old.jsonl"), "new.jsonl"),
            PathBuf::from("/s/--w--/new.jsonl")
        );
    }

    fn summary(path: &str, parent: Option<&str>) -> SessionSummary {
        SessionSummary {
            path: PathBuf::from(path),
            id: path.to_owned(),
            cwd: PathBuf::from("/w"),
            name: None,
            first_prompt: None,
            modified: UNIX_EPOCH,
            side_chat_of: parent.map(PathBuf::from),
            inherited_prompts: None,
        }
    }

    #[test]
    fn side_chats_follow_their_conversation() {
        let sessions = [
            summary("side2", Some("main")),
            summary("other", None),
            summary("main", None),
            summary("side1", Some("main")),
            summary("deep", Some("side2")),
            summary("orphan", Some("gone")),
        ];
        let rows: Vec<(&str, usize, bool, Vec<bool>)> = nest(&sessions)
            .into_iter()
            .map(|row| (row.session.id.as_str(), row.depth, row.last, row.through))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("other", 0, true, vec![]),
                ("main", 0, true, vec![]),
                ("side2", 1, false, vec![]),
                ("deep", 2, true, vec![true]),
                ("side1", 1, true, vec![]),
                ("orphan", 0, true, vec![]),
            ]
        );
    }

    #[test]
    fn side_chats_of_each_other_are_all_listed() {
        let sessions = [summary("a", Some("b")), summary("b", Some("a"))];
        let rows = nest(&sessions);
        assert_eq!(rows.len(), 2);
    }
}
