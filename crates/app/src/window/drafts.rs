//! Messages typed but not sent in chats the window let go (only the visible chat and working
//! ones keep their pi), until their session is shown again. Kept in memory only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use remote::HostId;

use crate::chat::Draft;
use crate::chat::session::PiSession;

/// Where a draft belongs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DraftKey {
    /// A session with a file (on its SSH host, or this computer).
    Session { host: Option<HostId>, file: PathBuf },
    /// A new session in this folder: nothing was sent yet, so there is no file to find it by.
    /// The folder also decides whether it is a chat or a Code session.
    New {
        host: Option<HostId>,
        folder: PathBuf,
    },
}

impl DraftKey {
    pub fn of(session: &PiSession) -> Self {
        let host = session.host().cloned();
        match session.session_file() {
            Some(file) if !session.is_new() => Self::Session { host, file },
            _ => Self::New {
                host,
                folder: session.cwd().to_path_buf(),
            },
        }
    }
}

/// The drafts of sessions that are not shown.
#[derive(Default)]
pub struct Drafts(HashMap<DraftKey, Draft>);

impl Drafts {
    /// Keeps `draft`, after one that already waits there.
    pub fn put(&mut self, key: DraftKey, draft: Draft) {
        if draft.is_empty() {
            return;
        }
        match self.0.get_mut(&key) {
            Some(waiting) => waiting.append(draft),
            None => drop(self.0.insert(key, draft)),
        }
    }

    /// The draft waiting there, which no longer waits.
    pub fn take(&mut self, key: &DraftKey) -> Option<Draft> {
        self.0.remove(key)
    }

    /// Drops the draft of a session moved to the trash.
    pub fn forget(&mut self, host: Option<&HostId>, file: &Path) {
        self.0.remove(&DraftKey::Session {
            host: host.cloned(),
            file: file.to_path_buf(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use remote::HostId;

    use super::{DraftKey, Drafts};
    use crate::chat::Draft;

    fn session(host: Option<&str>, file: &str) -> DraftKey {
        DraftKey::Session {
            host: host.map(|host| HostId(host.into())),
            file: file.into(),
        }
    }

    fn take(drafts: &mut Drafts, key: &DraftKey) -> Option<String> {
        drafts.take(key).map(|draft| draft.text().to_owned())
    }

    #[test]
    fn a_draft_comes_back_once_and_only_where_it_belongs() {
        let mut drafts = Drafts::default();
        let key = session(None, "/s.jsonl");
        drafts.put(key.clone(), Draft::for_tests("hello "));
        assert_eq!(take(&mut drafts, &session(Some("box"), "/s.jsonl")), None);
        assert_eq!(take(&mut drafts, &session(None, "/other.jsonl")), None);
        let new = DraftKey::New {
            host: None,
            folder: PathBuf::from("/s.jsonl"),
        };
        assert_eq!(take(&mut drafts, &new), None);
        assert_eq!(take(&mut drafts, &key).as_deref(), Some("hello "));
        assert_eq!(take(&mut drafts, &key), None);
    }

    #[test]
    fn empty_drafts_are_not_kept_and_later_ones_follow() {
        let mut drafts = Drafts::default();
        let key = session(None, "/s.jsonl");
        drafts.put(key.clone(), Draft::for_tests(" \n"));
        assert_eq!(take(&mut drafts, &key), None);
        drafts.put(key.clone(), Draft::for_tests("first"));
        drafts.put(key.clone(), Draft::for_tests(""));
        drafts.put(key.clone(), Draft::for_tests("second"));
        assert_eq!(take(&mut drafts, &key).as_deref(), Some("first\n\nsecond"));
    }

    #[test]
    fn trashed_sessions_lose_their_draft() {
        let mut drafts = Drafts::default();
        let host = HostId("box".into());
        for host in [None, Some("box")] {
            drafts.put(session(host, "/s.jsonl"), Draft::for_tests("text"));
        }
        drafts.forget(Some(&host), Path::new("/s.jsonl"));
        assert_eq!(take(&mut drafts, &session(Some("box"), "/s.jsonl")), None);
        assert!(take(&mut drafts, &session(None, "/s.jsonl")).is_some());
    }
}
