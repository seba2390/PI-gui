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

/// How a session is used in the GUI.
///
/// pi has no notion of modes. The GUI runs every *chat* in one dedicated, empty folder
/// ([`chat_dir`]) without file or shell tools (only deep search's web tools, off until the
/// chat turns deep search on), so a session's mode follows from its working folder alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Plain conversation: no file or shell tools, no project folder.
    Chat,
    /// The coding agent working in a project folder.
    #[default]
    Code,
}

impl Mode {
    /// The mode of a session that runs in `cwd`, given the GUI's chat folder. Chats started
    /// before the rename from PI-gui ran in `<agent dir>/pi-gui/chat` and are chats too.
    pub fn of(cwd: &Path, chat_dir: &Path) -> Self {
        let legacy = chat_dir
            .parent()
            .and_then(Path::parent)
            .map(|agent| agent.join(LEGACY_APP_DIR).join("chat"));
        if cwd == chat_dir || legacy.as_deref() == Some(cwd) {
            Self::Chat
        } else {
            Self::Code
        }
    }
}

/// The app's own folder inside pi's agent dir (settings, chat folder, web search).
pub const APP_DIR: &str = "pistachio";

/// [`APP_DIR`] before the rename from PI-gui; its settings move on first start.
pub const LEGACY_APP_DIR: &str = "pi-gui";

/// `<agent dir>/pistachio`.
pub fn app_dir() -> Option<PathBuf> {
    agent_dir().map(|agent| agent.join(APP_DIR))
}

/// The folder chat-mode sessions run in: `<agent dir>/pistachio/chat`.
pub fn chat_dir() -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("chat"))
}

/// Moves the settings of the app's folder from before the rename (`<agent dir>/pi-gui`) to
/// [`app_dir`], once: UI state, SSH hosts and web search settings. The old `chat` folder stays,
/// because earlier chats' session files point at it ([`Mode::of`] still treats it as a chat).
pub fn migrate_legacy_app_dir(agent: &Path) -> std::io::Result<()> {
    let (old, new) = (agent.join(LEGACY_APP_DIR), agent.join(APP_DIR));
    if !old.is_dir() || new.exists() {
        return Ok(());
    }
    fs::create_dir_all(&new)?;
    for name in ["state.json", "hosts.json", "deepsearch"] {
        let from = old.join(name);
        if from.exists() {
            fs::rename(&from, new.join(name))?;
        }
    }
    Ok(())
}

/// Sessions sharing a working folder, for the Code sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderGroup<'a> {
    pub cwd: &'a Path,
    /// Newest first (as given).
    pub sessions: Vec<&'a SessionSummary>,
}

impl FolderGroup<'_> {
    /// When any session in the folder last changed.
    pub fn latest(&self) -> SystemTime {
        self.sessions
            .iter()
            .map(|session| session.modified)
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

/// Groups `sessions` (newest first) by working folder; the folder with the most recent
/// activity comes first, and sessions keep their order within a folder.
pub fn group_by_folder<'a>(
    sessions: impl IntoIterator<Item = &'a SessionSummary>,
) -> Vec<FolderGroup<'a>> {
    let mut groups: Vec<FolderGroup<'a>> = Vec::new();
    for session in sessions {
        match groups.iter_mut().find(|group| group.cwd == session.cwd) {
            Some(group) => group.sessions.push(session),
            None => groups.push(FolderGroup {
                cwd: &session.cwd,
                sessions: vec![session],
            }),
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.latest()));
    groups
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
    let Some(first) = lines.next().transpose()? else {
        return Ok(None);
    };
    let mut info = None;
    let mut user = None;
    for line in lines {
        let line = line?;
        // Cheap prefilters before parsing; pi writes `type` first.
        if line.starts_with(SESSION_INFO_PREFIX) {
            info = Some(line);
        } else if user.is_none() && line.contains(USER_ROLE) {
            user = user_prompt(&line);
        }
    }
    let mut summary = summary_from_lines(path, modified, &first, info.as_deref(), None);
    if let Some(summary) = &mut summary {
        summary.first_prompt = user;
    }
    Ok(summary)
}

/// Lines starting with this are `session_info` entries (pi writes `type` first).
pub const SESSION_INFO_PREFIX: &str = r#"{"type":"session_info""#;
/// Lines containing this may be the first user message.
pub const USER_ROLE: &str = r#""role":"user""#;

/// Builds a summary from the few lines that matter: the header (first line), the last
/// `session_info` line, and the first line containing a user message. The user line may be
/// cut off (remote listings read only its start); its text is then recovered leniently.
/// `None` if `header` is not a session header.
pub fn summary_from_lines(
    path: &Path,
    modified: SystemTime,
    header: &str,
    info: Option<&str>,
    user: Option<&str>,
) -> Option<SessionSummary> {
    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "type")]
        kind: String,
        id: String,
        cwd: PathBuf,
    }
    #[derive(Deserialize)]
    struct Info {
        name: Option<String>,
    }
    let header = serde_json::from_str::<Header>(header.trim()).ok()?;
    if header.kind != "session" {
        return None;
    }
    let name = info
        .and_then(|line| serde_json::from_str::<Info>(line.trim()).ok())
        .and_then(|info| info.name);
    let first_prompt = user.and_then(|line| user_prompt(line).or_else(|| truncated_prompt(line)));
    Some(SessionSummary {
        path: path.to_owned(),
        id: header.id,
        cwd: header.cwd,
        name,
        first_prompt,
        modified,
    })
}

/// The first text of a user message line that was cut off mid-JSON: the first `"text":"…"`
/// string after `"content"`, decoded up to where it ends (or where the line was cut).
fn truncated_prompt(line: &str) -> Option<String> {
    if !line.contains(USER_ROLE) {
        return None;
    }
    let content = line.find(r#""content":"#)?;
    let rest = &line[content + 10..];
    let start = if rest.starts_with('"') {
        0
    } else {
        rest.find(r#""text":""#)? + 7
    };
    let body = &rest[start + 1..];
    // Find the closing quote (skipping escapes); a cut-off string ends where the line ends.
    let mut end = body.len();
    let mut escaped = false;
    for (index, c) in body.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    let mut raw = body[..end].to_owned();
    // Drop a dangling escape left by the cut, then decode as a JSON string.
    let trailing = raw.len() - raw.trim_end_matches('\\').len();
    if trailing % 2 == 1 {
        raw.pop();
    }
    let text: String = serde_json::from_str(&format!("\"{raw}\"")).ok()?;
    let first_line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(first_line.chars().take(120).collect())
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

    fn summary(cwd: &str, age_secs: u64) -> SessionSummary {
        SessionSummary {
            path: PathBuf::from(format!("{cwd}/{age_secs}.jsonl")),
            id: format!("{cwd}{age_secs}"),
            cwd: PathBuf::from(cwd),
            name: None,
            first_prompt: None,
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 - age_secs),
        }
    }

    #[test]
    fn mode_follows_the_chat_folder() {
        let chat = Path::new("/home/u/.pi/agent/pistachio/chat");
        assert_eq!(Mode::of(chat, chat), Mode::Chat);
        assert_eq!(Mode::of(Path::new("/home/u/code/app"), chat), Mode::Code);
        assert_eq!(
            Mode::of(Path::new("/home/u/.pi/agent/pistachio"), chat),
            Mode::Code
        );
        assert_eq!(serde_json::to_string(&Mode::Chat).unwrap(), "\"chat\"");
    }

    #[test]
    fn groups_by_folder_with_most_recent_activity_first() {
        // Input is newest first, as `scan` returns it.
        let sessions = [
            summary("/b", 10),
            summary("/a", 20),
            summary("/b", 30),
            summary("/c", 40),
            summary("/a", 50),
        ];
        let groups = group_by_folder(&sessions);
        let shape: Vec<(&str, Vec<&str>)> = groups
            .iter()
            .map(|group| {
                (
                    group.cwd.to_str().unwrap(),
                    group.sessions.iter().map(|s| s.id.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                ("/b", vec!["/b10", "/b30"]),
                ("/a", vec!["/a20", "/a50"]),
                ("/c", vec!["/c40"])
            ]
        );
        assert_eq!(groups[0].latest(), sessions[0].modified);
        assert_eq!(group_by_folder(&[]), Vec::new());
    }

    #[test]
    fn summaries_from_cut_off_lines() {
        let header = r#"{"type":"session","version":3,"id":"r","timestamp":"t","cwd":"/srv/app"}"#;
        let info = r#"{"type":"session_info","id":"1","parentId":null,"timestamp":"t","name":"Remote Fix"}"#;
        let user = r#"{"type":"message","id":"2","parentId":"1","timestamp":"t","message":{"role":"user","content":[{"type":"text","text":"Fix \"the\" build\nnow"},{"type":"image","data":"AAAA"#;
        let summary = summary_from_lines(
            Path::new("/r.jsonl"),
            SystemTime::UNIX_EPOCH,
            header,
            Some(info),
            Some(user),
        )
        .unwrap();
        assert_eq!(summary.name.as_deref(), Some("Remote Fix"));
        assert_eq!(summary.first_prompt.as_deref(), Some("Fix \"the\" build"));
        assert_eq!(summary.cwd, PathBuf::from("/srv/app"));

        // Cut inside the text itself, and right after a backslash.
        let cut = r#"{"message":{"role":"user","content":[{"type":"text","text":"Hello wor"#;
        assert_eq!(truncated_prompt(cut).as_deref(), Some("Hello wor"));
        let cut = r#"{"message":{"role":"user","content":"tab\"#;
        assert_eq!(truncated_prompt(cut).as_deref(), Some("tab"));
        assert!(
            summary_from_lines(Path::new("/x"), SystemTime::UNIX_EPOCH, "junk", None, None)
                .is_none()
        );
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

    #[test]
    fn legacy_chat_folder_is_still_chat() {
        let chat = Path::new("/home/u/.pi/agent/pistachio/chat");
        assert_eq!(
            Mode::of(Path::new("/home/u/.pi/agent/pi-gui/chat"), chat),
            Mode::Chat
        );
        assert_eq!(
            Mode::of(Path::new("/home/u/.pi/agent/pi-gui"), chat),
            Mode::Code
        );
    }

    #[test]
    fn migrates_settings_once_and_keeps_old_chats() {
        let agent = tempfile::tempdir().unwrap();
        let old = agent.path().join(LEGACY_APP_DIR);
        fs::create_dir_all(old.join("chat")).unwrap();
        fs::create_dir_all(old.join("deepsearch")).unwrap();
        fs::write(old.join("state.json"), "{}").unwrap();
        fs::write(old.join("deepsearch/config.json"), "{}").unwrap();
        migrate_legacy_app_dir(agent.path()).unwrap();
        let new = agent.path().join(APP_DIR);
        assert!(new.join("state.json").is_file());
        assert!(new.join("deepsearch/config.json").is_file());
        assert!(!new.join("hosts.json").exists());
        assert!(
            old.join("chat").is_dir(),
            "old chats stay where their sessions point"
        );
        // A second start changes nothing, even with files left in the old folder.
        fs::write(old.join("hosts.json"), "{}").unwrap();
        migrate_legacy_app_dir(agent.path()).unwrap();
        assert!(!new.join("hosts.json").exists());
    }
}
