//! Read-only index of pi's session files.
//!
//! pi stores each session as `<sessions>/--<cwd>--/<timestamp>_<id>.jsonl`. The
//! sidebar needs only a few facts per file (title, working folder, recency), so the
//! scanner reads lines cheaply and fully parses only the ones it needs. Opening a
//! session is always done by pi itself (`switch_session` / `--session`).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

/// Facts about one session file, enough to render a sidebar row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub id: String,
    /// The working folder pi runs this session in (fixed at creation).
    pub cwd: PathBuf,
    /// The latest `session_info` name (pi auto-titles sessions), if any.
    pub name: Option<String>,
    /// First line of the first user prompt, a fallback title.
    pub first_prompt: Option<String>,
    pub modified: SystemTime,
}

impl SessionSummary {
    /// The title the sidebar shows.
    pub fn title(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty() && *name != "New thread")
            .or(self.first_prompt.as_deref())
            .unwrap_or("New chat")
    }

    /// Last path component of the working folder (`~/code/app` → `app`).
    pub fn folder_name(&self) -> String {
        self.cwd.file_name().map_or_else(
            || self.cwd.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

/// pi's config dir: `PI_CODING_AGENT_DIR` or `~/.pi/agent`.
pub fn agent_dir() -> Option<PathBuf> {
    std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".pi").join("agent")))
}

/// Where pi stores sessions, following its precedence:
/// `PI_CODING_AGENT_SESSION_DIR`, then an absolute `sessionDir` setting, then
/// `<agent dir>/sessions`.
pub fn sessions_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    let agent = agent_dir()?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Settings {
        session_dir: Option<PathBuf>,
    }
    let configured = fs::read(agent.join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
        .and_then(|settings| settings.session_dir)
        .filter(|dir| dir.is_absolute());
    Some(configured.unwrap_or_else(|| agent.join("sessions")))
}

/// All sessions under `root`, most recently modified first. Unreadable or
/// malformed files are skipped.
pub fn scan(root: &Path) -> Vec<SessionSummary> {
    scan_reusing(root, &[])
}

/// Like [`scan`], but reuses entries of `previous` whose file has not been modified since,
/// so a rescan after pi appends to one session reads only that file instead of every
/// session in full.
pub fn scan_reusing(root: &Path, previous: &[SessionSummary]) -> Vec<SessionSummary> {
    let known: std::collections::HashMap<&Path, &SessionSummary> = previous
        .iter()
        .map(|summary| (summary.path.as_path(), summary))
        .collect();
    let Ok(groups) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionSummary> = groups
        .filter_map(Result::ok)
        .filter(|group| group.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|group| fs::read_dir(group.path()).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|path| {
            let modified = fs::metadata(&path).and_then(|meta| meta.modified()).ok();
            if let Some(summary) = known.get(path.as_path())
                && Some(summary.modified) == modified
            {
                return Some((*summary).clone());
            }
            match read_summary(&path) {
                Ok(summary) => summary,
                Err(error) => {
                    log::debug!("skipping session {}: {error}", path.display());
                    None
                }
            }
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
    sessions
}

/// Distinct working folders of `sessions` (expected newest first), most recent first, at most
/// `limit`. Feeds the composer's folder menu.
pub fn recent_folders(sessions: &[SessionSummary], limit: usize) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = Vec::new();
    for session in sessions {
        if folders.len() == limit {
            break;
        }
        if !folders.contains(&session.cwd) {
            folders.push(session.cwd.clone());
        }
    }
    folders
}

/// Reads the summary of one session file. `Ok(None)` if it has no header.
pub fn read_summary(path: &Path) -> std::io::Result<Option<SessionSummary>> {
    let modified = fs::metadata(path)?.modified()?;
    let mut lines = BufReader::new(fs::File::open(path)?).lines();

    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        cwd: PathBuf,
    }
    let Some(first) = lines.next().transpose()? else {
        return Ok(None);
    };
    let Ok(header) = serde_json::from_str::<Header>(&first) else {
        return Ok(None);
    };
    if header.kind != "session" {
        return Ok(None);
    }

    let mut name = None;
    let mut first_prompt = None;
    for line in lines {
        let line = line?;
        // Cheap prefilters before parsing; pi writes `type` first.
        if line.starts_with(r#"{"type":"session_info""#) {
            #[derive(Deserialize)]
            struct Info {
                name: Option<String>,
            }
            if let Ok(info) = serde_json::from_str::<Info>(&line) {
                name = info.name;
            }
        } else if first_prompt.is_none() && line.contains(r#""role":"user""#) {
            first_prompt = user_prompt(&line);
        }
    }

    Ok(Some(SessionSummary {
        path: path.to_owned(),
        id: header.id,
        cwd: header.cwd,
        name,
        first_prompt,
        modified,
    }))
}

fn user_prompt(line: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Entry {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Message {
        role: String,
        content: serde_json::Value,
    }
    let entry: Entry = serde_json::from_str(line).ok()?;
    if entry.message.role != "user" {
        return None;
    }
    let text = match &entry.message.content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .find_map(|block| block.get("text").and_then(serde_json::Value::as_str))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    };
    let first_line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(first_line.chars().take(120).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Write;

    fn write_session(root: &Path, group: &str, file: &str, lines: &[&str]) -> PathBuf {
        let dir = root.join(group);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        let mut out = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(out, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn scans_titles_cwd_and_ignores_noise() {
        let root = tempfile::tempdir().unwrap();
        write_session(
            root.path(),
            "--w-app--",
            "2026_a.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"a","timestamp":"t","cwd":"/w/app"}"#,
                r#"{"type":"session_info","id":"1","parentId":null,"timestamp":"t","name":"New thread"}"#,
                r#"{"type":"message","id":"2","parentId":"1","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"\n  Fix the build\nplease"}],"timestamp":1}}"#,
                r#"{"type":"session_info","id":"3","parentId":"2","timestamp":"t","name":"Fix Build"}"#,
            ],
        );
        write_session(
            root.path(),
            "--w-app--",
            "2026_b.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"b","timestamp":"t","cwd":"/w/app"}"#,
                r#"{"type":"message","id":"2","parentId":null,"timestamp":"t","message":{"role":"user","content":"hello there","timestamp":1}}"#,
            ],
        );
        write_session(root.path(), "--w-app--", "x.jsonl.lease", &["junk"]);
        write_session(root.path(), "--w-app--", "broken.jsonl", &["not json"]);

        let sessions = scan(root.path());
        let mut titles: Vec<_> = sessions
            .iter()
            .map(|s| (s.id.as_str(), s.title()))
            .collect();
        titles.sort_unstable();
        assert_eq!(titles, vec![("a", "Fix Build"), ("b", "hello there")]);
        assert_eq!(sessions[0].folder_name(), "app");
    }

    #[test]
    fn rescan_reuses_unmodified_sessions_and_rereads_changed_ones() {
        let root = tempfile::tempdir().unwrap();
        let header = r#"{"type":"session","version":3,"id":"a","timestamp":"t","cwd":"/w"}"#;
        let path = write_session(root.path(), "--w--", "a.jsonl", &[header]);
        let first = scan(root.path());
        assert_eq!(first[0].title(), "New chat");

        // A stale entry with the file's current mtime is trusted (not re-read)...
        let mut stale = first.clone();
        stale[0].name = Some("Cached".to_owned());
        assert_eq!(scan_reusing(root.path(), &stale)[0].title(), "Cached");

        // ...but once the file changes, it is read again.
        let info =
            r#"{"type":"session_info","id":"1","parentId":null,"timestamp":"t","name":"Renamed"}"#;
        write_session(root.path(), "--w--", "a.jsonl", &[header, info]);
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(scan_reusing(root.path(), &stale)[0].title(), "Renamed");
    }

    #[test]
    fn recent_folders_are_distinct_newest_first_and_capped() {
        let session = |cwd: &str| SessionSummary {
            path: PathBuf::from(format!("{cwd}.jsonl")),
            id: cwd.to_owned(),
            cwd: PathBuf::from(cwd),
            name: None,
            first_prompt: None,
            modified: SystemTime::UNIX_EPOCH,
        };
        let sessions = [
            session("/a"),
            session("/b"),
            session("/a"),
            session("/c"),
            session("/d"),
        ];
        assert_eq!(
            recent_folders(&sessions, 3),
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
        assert_eq!(recent_folders(&[], 3), Vec::<PathBuf>::new());
    }

    #[test]
    fn placeholder_name_falls_back_to_prompt_then_default() {
        let root = tempfile::tempdir().unwrap();
        let path = write_session(
            root.path(),
            "--w--",
            "s.jsonl",
            &[
                r#"{"type":"session","version":3,"id":"s","timestamp":"t","cwd":"/w"}"#,
                r#"{"type":"session_info","id":"1","parentId":null,"timestamp":"t","name":"New thread"}"#,
            ],
        );
        assert_eq!(read_summary(&path).unwrap().unwrap().title(), "New chat");
    }
}
