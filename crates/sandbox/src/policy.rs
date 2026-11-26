//! What a sandboxed command in one project may do: the [`Settings`] applied to a folder.
//!
//! The home folder is private: commands see only the project, what the user allowed, the
//! folders of common tools and pi's skills in it. The rest of the computer (programs, libraries,
//! system settings) can be read, since commands need it, but changed only in temporary folders.
//! Keys and logins ([`ALWAYS_HIDDEN`]) stay hidden even inside an allowed folder.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::paths::{expand, resolve};
use crate::settings::{ALWAYS_HIDDEN, Network, Settings, TOOL_CACHES, TOOL_PROGRAMS};

/// The rules for commands in one project. Paths are resolved (see [`resolve`]) and may not
/// exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    /// The session's folder.
    pub project: PathBuf,
    /// Folders hidden as a whole, except what [`Self::readable`] and [`Self::writable`] open in
    /// them: the home folder.
    pub private: Vec<PathBuf>,
    /// What commands may read inside a private folder (besides what they may write).
    pub readable: Vec<PathBuf>,
    /// Where commands may write: the project, temporary folders, tool caches and folders the
    /// user allowed to change. Everything else is read-only (or hidden).
    pub writable: Vec<PathBuf>,
    /// What commands may neither read nor write, wherever it is.
    pub hidden: Vec<PathBuf>,
    pub network: Network,
    /// With [`Network::Custom`]: the loopback port of the app's proxy, the only place commands
    /// may connect to (set by the helper, which knows where the proxy listens).
    #[serde(skip)]
    pub proxy: Option<u16>,
}

/// What the rules depend on besides the settings.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub home: Option<PathBuf>,
    /// Temporary folders, always writable.
    pub temp: Vec<PathBuf>,
    /// pi's settings folder: its skills can be read, its `auth.json` never.
    pub agent_dir: Option<PathBuf>,
    /// The folders of `$PATH`: those in the home folder hold the user's own tools, which stay
    /// readable with the tool folders.
    pub path: Vec<PathBuf>,
}

impl Context {
    /// This process's: `$HOME`, the system's temporary folders, `$PI_CODING_AGENT_DIR` and
    /// `$PATH`.
    pub fn current() -> Self {
        let home = crate::paths::home();
        let mut temp = vec![std::env::temp_dir()];
        if cfg!(unix) {
            temp.extend(["/tmp", "/var/tmp"].map(PathBuf::from));
        }
        if cfg!(target_os = "linux") {
            temp.push(PathBuf::from("/dev/shm"));
        }
        // macOS: `$TMPDIR` is `/var/folders/…/T`; its sibling `C` holds the user's caches.
        if cfg!(target_os = "macos") {
            let user_temp = std::env::temp_dir();
            if user_temp.file_name().is_some_and(|name| name == "T")
                && let Some(parent) = user_temp.parent()
            {
                temp.push(parent.to_path_buf());
            }
        }
        let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".pi").join("agent")));
        let path = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        Self {
            home,
            temp,
            agent_dir,
            path,
        }
    }
}

impl Policy {
    /// The rules for commands in `project`, or `None` when they run without the sandbox (it is
    /// off, or off for this project).
    pub fn for_project(settings: &Settings, project: &Path, cx: &Context) -> Option<Self> {
        let home = cx.home.as_deref();
        if !settings.enabled || settings.is_unsandboxed(project, home) {
            return None;
        }
        let at = |path: &str| resolve(&expand(path, home));
        let project =
            resolve(&std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf()));
        let mut writable = vec![project.clone()];
        writable.extend(cx.temp.iter().map(|dir| resolve(dir)));
        writable.extend(git_dirs(&project));
        let mut readable = Vec::new();
        // A subfolder of a repository: git reads the rest of it (unless that is the home folder,
        // kept in git by some).
        if let Some(top) = project
            .ancestors()
            .skip(1)
            .find(|dir| dir.join(".git").exists())
            && !home.is_some_and(|home| resolve(home).starts_with(top))
        {
            readable.push(top.to_path_buf());
        }
        if let Some(agent) = &cx.agent_dir {
            // Skills are read with `read`; pi's own folder otherwise holds other sessions.
            for name in ["skills", "prompts", "extensions", "pistachio/skills"] {
                readable.push(resolve(&agent.join(name)));
            }
        }
        readable.push(at("~/.agents"));
        if settings.tool_folders && home.is_some() {
            writable.extend(TOOL_CACHES.iter().map(|path| at(path)));
            readable.extend(TOOL_PROGRAMS.iter().map(|path| at(path)));
            readable.extend(own_tools(&cx.path, home));
        }
        for allowed in &settings.allowed {
            let path = allowed.path.trim();
            if path.is_empty() {
                continue;
            }
            if allowed.write {
                writable.push(at(path));
            } else {
                readable.push(at(path));
            }
        }
        let mut hidden: Vec<PathBuf> = ALWAYS_HIDDEN.iter().map(|path| at(path)).collect();
        if let Some(agent) = &cx.agent_dir {
            hidden.push(resolve(&agent.join("auth.json")));
        }
        let mut private: Vec<PathBuf> = home.map(resolve).into_iter().collect();
        // Only absolute paths mean something to the kernel.
        for list in [&mut writable, &mut readable, &mut hidden, &mut private] {
            list.retain(|path| path.is_absolute());
            dedup(list);
        }
        Some(Self {
            project,
            private,
            readable,
            writable,
            hidden,
            network: settings.network,
            proxy: None,
        })
    }

    /// Whether commands may write `path` (resolved).
    pub fn may_write(&self, path: &Path) -> bool {
        self.writable.iter().any(|root| path.starts_with(root)) && self.may_read(path)
    }

    /// Whether commands may read `path` (resolved).
    pub fn may_read(&self, path: &Path) -> bool {
        if self.hidden.iter().any(|hidden| path.starts_with(hidden)) {
            return false;
        }
        // Inside a private folder only what is open in it counts, not a folder around it.
        match self
            .private
            .iter()
            .find(|private| path.starts_with(private))
        {
            None => true,
            Some(private) => self
                .readable
                .iter()
                .chain(&self.writable)
                .any(|root| root.starts_with(private) && path.starts_with(root)),
        }
    }

    /// What is open inside the private folders: everything readable or writable there.
    pub fn open_in_private(&self) -> Vec<PathBuf> {
        let mut open: Vec<PathBuf> = self
            .readable
            .iter()
            .chain(&self.writable)
            .filter(|path| self.private.iter().any(|private| path.starts_with(private)))
            .cloned()
            .collect();
        dedup(&mut open);
        open
    }

    /// The folders a command passes through to reach what is open in the private folders
    /// (the home folder itself, say): their own entries can be looked up, not listed or read.
    pub fn passages(&self) -> Vec<PathBuf> {
        let mut passages = Vec::new();
        for open in self.open_in_private() {
            for ancestor in open.ancestors().skip(1) {
                if !self
                    .private
                    .iter()
                    .any(|private| ancestor.starts_with(private))
                {
                    break;
                }
                passages.push(ancestor.to_path_buf());
            }
        }
        dedup(&mut passages);
        passages
    }
}

/// The user's own tools: folders of `$PATH` in the home folder (for `…/bin`, the folder it is in,
/// which holds the libraries next to it), never the home folder itself.
fn own_tools(path: &[PathBuf], home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home.map(resolve) else {
        return Vec::new();
    };
    path.iter()
        .map(|dir| resolve(dir))
        .filter(|dir| dir.starts_with(&home) && *dir != home)
        .map(|dir| match dir.parent() {
            Some(parent) if dir.file_name().is_some_and(|name| name == "bin") && parent != home => {
                parent.to_path_buf()
            }
            _ => dir,
        })
        .collect()
}

/// The git folders a project's commits write to when they are not inside it: the repository's
/// `.git` when the project is a subfolder, and a worktree's own and common folders.
fn git_dirs(project: &Path) -> Vec<PathBuf> {
    let Some((top, dot_git)) = project
        .ancestors()
        .map(|dir| (dir, dir.join(".git")))
        .find(|(_, dot_git)| dot_git.exists())
    else {
        return Vec::new();
    };
    if dot_git.is_dir() {
        return vec![resolve(&dot_git)];
    }
    // A worktree: `.git` is a file saying `gitdir: <its folder>`, which names the common
    // folder in `commondir`.
    let Some(git_dir) = std::fs::read_to_string(&dot_git).ok().and_then(|text| {
        let line = text
            .lines()
            .next()?
            .strip_prefix("gitdir:")?
            .trim()
            .to_owned();
        Some(resolve(&top.join(line)))
    }) else {
        return Vec::new();
    };
    let mut dirs = vec![git_dir.clone()];
    if let Ok(common) = std::fs::read_to_string(git_dir.join("commondir")) {
        dirs.push(resolve(&git_dir.join(common.trim())));
    }
    dirs
}

fn dedup(paths: &mut Vec<PathBuf>) {
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Allowed;

    fn context(home: &Path) -> Context {
        Context {
            home: Some(home.to_path_buf()),
            temp: vec![PathBuf::from("/tmp")],
            agent_dir: Some(home.join(".pi/agent")),
            path: vec![
                home.join(".nvm/versions/node/v24/bin"),
                home.join("bin"),
                home.to_path_buf(),
            ],
        }
    }

    #[test]
    fn off_or_unsandboxed_projects_have_no_policy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let off = Settings {
            enabled: false,
            ..Settings::default()
        };
        assert_eq!(
            Policy::for_project(&off, dir.path(), &context(dir.path())),
            None
        );
        let unsandboxed = Settings {
            unsandboxed: vec!["~/app".into()],
            ..Settings::default()
        };
        let cx = context(dir.path());
        assert_eq!(
            Policy::for_project(&unsandboxed, &dir.path().join("app/web"), &cx),
            None
        );
        assert!(Policy::for_project(&unsandboxed, &dir.path().join("other"), &cx).is_some());
    }

    #[test]
    fn the_home_folder_is_hidden_except_the_project_tools_and_what_is_allowed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let project = home.join("code/app");
        std::fs::create_dir_all(project.join("src")).expect("mkdir");
        let settings = Settings {
            allowed: vec![
                Allowed {
                    path: "~/Documents/specs".into(),
                    write: false,
                },
                Allowed {
                    path: "~/shared".into(),
                    write: true,
                },
            ],
            ..Settings::default()
        };
        let policy = Policy::for_project(&settings, &project, &context(&home)).expect("policy");
        assert!(policy.may_write(&project.join("src/main.rs")));
        assert!(policy.may_write(&resolve(Path::new("/tmp")).join("x")));
        assert!(policy.may_read(Path::new("/usr/bin/git")));
        assert!(!policy.may_write(Path::new("/usr/bin/git")));
        // Other projects, documents and dotfiles are hidden.
        assert!(!policy.may_read(&home.join("code/other/secret.txt")));
        assert!(!policy.may_read(&home.join("Documents/taxes.pdf")));
        assert!(!policy.may_read(&home.join(".bashrc")));
        assert!(!policy.may_read(&home));
        // Tools, skills and what the user allowed are not.
        assert!(policy.may_write(&home.join(".cargo/registry/x")));
        assert!(policy.may_read(&home.join(".gitconfig")));
        assert!(!policy.may_write(&home.join(".gitconfig")));
        assert!(policy.may_read(&home.join(".nvm/versions/node/v24/lib/node_modules")));
        assert!(policy.may_read(&home.join("bin/tool")));
        assert!(policy.may_read(&home.join(".pi/agent/skills/review/SKILL.md")));
        assert!(!policy.may_read(&home.join(".pi/agent/sessions/x.jsonl")));
        assert!(!policy.may_read(&home.join(".pi/agent/auth.json")));
        assert!(policy.may_read(&home.join("Documents/specs/a.md")));
        assert!(!policy.may_write(&home.join("Documents/specs/a.md")));
        assert!(policy.may_write(&home.join("shared/notes.txt")));
        // The home folder in `$PATH` does not open it, nor does a folder around it.
        assert!(!policy.readable.contains(&home));
        let around = Settings {
            allowed: vec![Allowed {
                path: dir
                    .path()
                    .parent()
                    .expect("parent")
                    .to_string_lossy()
                    .into_owned(),
                write: false,
            }],
            ..Settings::default()
        };
        let policy = Policy::for_project(&around, &project, &context(&home)).expect("policy");
        assert!(!policy.may_read(&home.join(".bashrc")));

        let no_tools = Settings {
            tool_folders: false,
            ..Settings::default()
        };
        let policy = Policy::for_project(&no_tools, &project, &context(&home)).expect("policy");
        assert!(!policy.may_read(&home.join(".cargo/registry/x")));
    }

    #[test]
    fn keys_stay_hidden_inside_an_allowed_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let settings = Settings {
            allowed: vec![Allowed {
                path: "~".into(),
                write: false,
            }],
            ..Settings::default()
        };
        let policy =
            Policy::for_project(&settings, &home.join("app"), &context(&home)).expect("policy");
        assert!(policy.may_read(&home.join("Documents/a.txt")));
        assert!(!policy.may_read(&home.join(".ssh/id_ed25519")));
        assert!(!policy.may_read(&home.join(".aws/credentials")));
    }

    #[test]
    fn the_way_to_what_is_open_passes_through_the_home_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let cx = Context {
            path: Vec::new(),
            ..context(&home)
        };
        let settings = Settings {
            tool_folders: false,
            ..Settings::default()
        };
        let policy = Policy::for_project(&settings, &home.join("code/app"), &cx).expect("policy");
        let passages = policy.passages();
        assert!(passages.contains(&home));
        assert!(passages.contains(&home.join("code")));
        assert!(!passages.contains(&home.join("code/app")));
        assert!(!passages.iter().any(|path| !path.starts_with(&home)));
    }

    #[test]
    fn commits_from_a_subfolder_or_worktree_can_write_the_git_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let repo = home.join("repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/feature")).expect("mkdir");
        std::fs::create_dir_all(repo.join("web")).expect("mkdir");
        let policy = Policy::for_project(&Settings::default(), &repo.join("web"), &context(&home))
            .expect("policy");
        assert!(policy.may_write(&repo.join(".git/index")));
        assert!(!policy.may_write(&repo.join("README.md")));

        let worktree = home.join("feature");
        std::fs::create_dir_all(&worktree).expect("mkdir");
        std::fs::write(
            worktree.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/feature").display()
            ),
        )
        .expect("write");
        std::fs::write(repo.join(".git/worktrees/feature/commondir"), "../..\n").expect("write");
        let policy =
            Policy::for_project(&Settings::default(), &worktree, &context(&home)).expect("policy");
        assert!(policy.may_write(&repo.join(".git/worktrees/feature/index")));
        assert!(policy.may_write(&repo.join(".git/objects/ab/cdef")));
        assert!(!policy.may_write(&repo.join("README.md")));
    }
}
