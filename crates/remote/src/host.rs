//! Saved remote connections, stored as JSON at `<agent dir>/pi-gui/hosts.json`.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many recent folders are remembered per host.
const MAX_RECENT_FOLDERS: usize = 8;

/// Stable identifier of a saved host: a slug of its label, unique within the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HostId(pub String);

/// What to connect to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    /// A `Host` from `~/.ssh/config`; everything else comes from the config.
    Alias { alias: String },
    /// A host entered by hand. Unset fields fall back to the ssh config and defaults.
    Custom {
        host: String,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        identity_file: Option<PathBuf>,
        #[serde(default)]
        proxy_jump: Option<String>,
    },
}

impl Target {
    /// Short human-readable form: the alias, or `user@host:port` leaving out what is unset
    /// (and port 22).
    pub fn display(&self) -> String {
        match self {
            Self::Alias { alias } => alias.clone(),
            Self::Custom {
                host, user, port, ..
            } => {
                let mut text = String::new();
                if let Some(user) = user {
                    let _ = write!(text, "{user}@");
                }
                match port.filter(|&port| port != 22) {
                    Some(port) if host.contains(':') => {
                        let _ = write!(text, "[{host}]:{port}");
                    }
                    Some(port) => {
                        let _ = write!(text, "{host}:{port}");
                    }
                    None => text.push_str(host),
                }
                text
            }
        }
    }
}

/// One saved connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedHost {
    pub id: HostId,
    pub label: String,
    pub target: Target,
    /// The command that starts pi on the host (a program name or path, `~/` allowed);
    /// `None` means `pi`.
    #[serde(default)]
    pub pi_command: Option<String>,
    /// Remote absolute paths opened recently, newest first, at most 8.
    #[serde(default)]
    pub recent_folders: Vec<String>,
}

impl SavedHost {
    /// The command that starts pi on this host.
    pub fn pi_command(&self) -> &str {
        self.pi_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .unwrap_or("pi")
    }
}

/// All saved connections, in the order the user added them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostStore {
    pub hosts: Vec<SavedHost>,
}

impl HostStore {
    /// Where the store lives, or `None` if pi's agent dir is unknown.
    pub fn path() -> Option<PathBuf> {
        pi_sessions::agent_dir().map(|agent| agent.join("pi-gui").join("hosts.json"))
    }

    /// Reads the store; a missing file gives an empty store, and so does a corrupt one
    /// (with a warning).
    pub fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::warn!("ignoring corrupt host list {}: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                log::warn!("could not read host list {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes atomically (temp file + rename), creating the directory if needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        fs::rename(tmp, path)
    }

    /// Saves a new host and returns its id; if a host with the same target is already saved,
    /// returns that one's id and changes nothing.
    pub fn add(&mut self, label: String, target: Target) -> HostId {
        if let Some(existing) = self.hosts.iter().find(|host| host.target == target) {
            return existing.id.clone();
        }
        let id = self.unique_id(&slug(&label));
        self.hosts.push(SavedHost {
            id: id.clone(),
            label,
            target,
            pi_command: None,
            recent_folders: Vec::new(),
        });
        id
    }

    pub fn get(&self, id: &HostId) -> Option<&SavedHost> {
        self.hosts.iter().find(|host| &host.id == id)
    }

    pub fn get_mut(&mut self, id: &HostId) -> Option<&mut SavedHost> {
        self.hosts.iter_mut().find(|host| &host.id == id)
    }

    pub fn remove(&mut self, id: &HostId) {
        self.hosts.retain(|host| &host.id != id);
    }

    /// Records that `folder` was opened on host `id`: it moves to the front of the recent
    /// list, which keeps at most 8 entries.
    pub fn touch_folder(&mut self, id: &HostId, folder: &str) {
        let Some(host) = self.get_mut(id) else {
            return;
        };
        host.recent_folders.retain(|recent| recent != folder);
        host.recent_folders.insert(0, folder.to_owned());
        host.recent_folders.truncate(MAX_RECENT_FOLDERS);
    }

    fn unique_id(&self, base: &str) -> HostId {
        let taken = |candidate: &str| self.hosts.iter().any(|host| host.id.0 == candidate);
        if !taken(base) {
            return HostId(base.to_owned());
        }
        // At most `hosts.len()` suffixes can be taken, so one of these is free.
        (1..=self.hosts.len() + 1)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| !taken(candidate))
            .map_or_else(|| HostId(base.to_owned()), HostId)
    }
}

/// Lowercase ASCII letters and digits, with every other run of characters turned into one
/// `-`; `host` if nothing is left.
fn slug(label: &str) -> String {
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "host".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn alias(name: &str) -> Target {
        Target::Alias {
            alias: name.to_owned(),
        }
    }

    fn custom(host: &str) -> Target {
        Target::Custom {
            host: host.to_owned(),
            user: None,
            port: None,
            identity_file: None,
            proxy_jump: None,
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Spark 2"), "spark-2");
        assert_eq!(slug("  dgx_spark__admin!! "), "dgx-spark-admin");
        assert_eq!(slug("Ærø"), "r");
        assert_eq!(slug("!!!"), "host");
    }

    #[test]
    fn add_dedupes_targets_and_makes_ids_unique() {
        let mut store = HostStore::default();
        let a = store.add("Spark 2".into(), alias("spark2"));
        let b = store.add("spark-2".into(), custom("10.0.0.2"));
        let c = store.add("Spark 2".into(), custom("10.0.0.3"));
        let again = store.add("Whatever".into(), alias("spark2"));
        assert_eq!(a, HostId("spark-2".into()));
        assert_eq!(b, HostId("spark-2-1".into()));
        assert_eq!(c, HostId("spark-2-2".into()));
        assert_eq!(again, a);
        assert_eq!(store.hosts.len(), 3);
        assert_eq!(store.get(&a).unwrap().label, "Spark 2");
        assert_eq!(store.get(&a).unwrap().pi_command(), "pi");

        store.remove(&b);
        assert!(store.get(&b).is_none());
        assert_eq!(store.hosts.len(), 2);
        // A freed id can be handed out again.
        assert_eq!(
            store.add("spark 2".into(), custom("10.0.0.4")).0,
            "spark-2-1"
        );
    }

    #[test]
    fn touch_folder_moves_to_front_and_caps() {
        let mut store = HostStore::default();
        let id = store.add("x".into(), alias("x"));
        for n in 0..10 {
            store.touch_folder(&id, &format!("/f{n}"));
        }
        store.touch_folder(&id, "/f5");
        let recent = &store.get(&id).unwrap().recent_folders;
        assert_eq!(
            recent,
            &["/f5", "/f9", "/f8", "/f7", "/f6", "/f4", "/f3", "/f2"]
        );
        // Unknown ids are ignored.
        store.touch_folder(&HostId("nope".into()), "/x");
    }

    #[test]
    fn save_load_round_trip_and_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/hosts.json");
        assert_eq!(HostStore::load(&path), HostStore::default());

        let mut store = HostStore::default();
        let id = store.add("Spark".into(), alias("spark"));
        store.add(
            "Box".into(),
            Target::Custom {
                host: "box.example.com".into(),
                user: Some("me".into()),
                port: Some(2222),
                identity_file: Some("/Users/me/.ssh/id_box".into()),
                proxy_jump: Some("bastion".into()),
            },
        );
        store.get_mut(&id).unwrap().pi_command = Some("~/bin/pi".into());
        store.touch_folder(&id, "/home/me/code");
        store.save(&path).unwrap();
        assert_eq!(HostStore::load(&path), store);
        assert!(!path.with_extension("json.tmp").exists());

        let json = fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""kind": "alias""#), "{json}");
        assert!(json.contains(r#""kind": "custom""#), "{json}");

        fs::write(&path, "{ not json").unwrap();
        assert_eq!(HostStore::load(&path), HostStore::default());
    }

    #[test]
    fn tolerates_minimal_json() {
        let store: HostStore = serde_json::from_str(
            r#"{"hosts":[{"id":"a","label":"A","target":{"kind":"custom","host":"h"}}]}"#,
        )
        .unwrap();
        let host = &store.hosts[0];
        assert_eq!(host.target, custom("h"));
        assert_eq!(host.pi_command(), "pi");
        assert_eq!(host.recent_folders, Vec::<String>::new());
    }

    #[test]
    fn display() {
        assert_eq!(alias("spark").display(), "spark");
        assert_eq!(custom("h").display(), "h");
        let full = Target::Custom {
            host: "h".into(),
            user: Some("u".into()),
            port: Some(2222),
            identity_file: None,
            proxy_jump: None,
        };
        assert_eq!(full.display(), "u@h:2222");
        let default_port = Target::Custom {
            host: "::1".into(),
            user: None,
            port: Some(22),
            identity_file: None,
            proxy_jump: None,
        };
        assert_eq!(default_port.display(), "::1");
        let v6 = Target::Custom {
            host: "::1".into(),
            user: None,
            port: Some(2200),
            identity_file: None,
            proxy_jump: None,
        };
        assert_eq!(v6.display(), "[::1]:2200");
    }
}
