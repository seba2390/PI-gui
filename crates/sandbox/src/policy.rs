//! What a sandboxed command in one project may do: the [`Settings`] applied to a folder.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::paths::{expand, resolve};
use crate::settings::{Network, Settings, TOOL_CACHES};

/// The rules for commands in one project. Paths are resolved (see [`resolve`]) and may not
/// exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    /// The session's folder.
    pub project: PathBuf,
    /// Where commands may write: the project, temporary folders, tool caches and the user's
    /// own additions. Everything else can only be read.
    pub writable: Vec<PathBuf>,
    /// What commands may neither read nor write.
    pub hidden: Vec<PathBuf>,
    pub network: Network,
}

/// What the rules depend on besides the settings.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub home: Option<PathBuf>,
    /// Temporary folders, always writable.
    pub temp: Vec<PathBuf>,
    /// pi's settings folder, whose `auth.json` is always hidden.
    pub agent_dir: Option<PathBuf>,
}

impl Context {
    /// This process's: `$HOME`, the system's temporary folders and `$PI_CODING_AGENT_DIR`.
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
        Self {
            home,
            temp,
            agent_dir,
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
        let project = resolve(project);
        let mut writable = vec![project.clone()];
        writable.extend(cx.temp.iter().map(|dir| resolve(dir)));
        writable.extend(git_dirs(&project));
        if settings.tool_caches && home.is_some() {
            writable.extend(TOOL_CACHES.iter().map(|path| resolve(&expand(path, home))));
        }
        writable.extend(
            settings
                .writable
                .iter()
                .map(|path| resolve(&expand(path, home))),
        );
        let mut hidden: Vec<PathBuf> = settings
            .hidden
            .iter()
            .filter(|path| !path.trim().is_empty())
            .map(|path| resolve(&expand(path.trim(), home)))
            .collect();
        if let Some(agent) = &cx.agent_dir {
            hidden.push(resolve(&agent.join("auth.json")));
        }
        // Only absolute paths mean something to the kernel.
        writable.retain(|path| path.is_absolute());
        hidden.retain(|path| path.is_absolute());
        dedup(&mut writable);
        dedup(&mut hidden);
        Some(Self {
            project,
            writable,
            hidden,
            network: settings.network,
        })
    }

    /// Whether commands may write `path` (resolved).
    pub fn may_write(&self, path: &Path) -> bool {
        self.writable.iter().any(|root| path.starts_with(root)) && self.may_read(path)
    }

    /// Whether commands may read `path` (resolved).
    pub fn may_read(&self, path: &Path) -> bool {
        !self.hidden.iter().any(|hidden| path.starts_with(hidden))
    }
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

    fn context(home: &Path) -> Context {
        Context {
            home: Some(home.to_path_buf()),
            temp: vec![home.join("tmp")],
            agent_dir: Some(home.join(".pi/agent")),
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
    fn the_project_temp_and_caches_are_writable_and_secrets_hidden() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let project = home.join("app");
        std::fs::create_dir_all(project.join("src")).expect("mkdir");
        let settings = Settings {
            writable: vec!["~/shared".into()],
            ..Settings::default()
        };
        let policy = Policy::for_project(&settings, &project, &context(&home)).expect("policy");
        assert!(policy.may_write(&project.join("src/main.rs")));
        assert!(policy.may_write(&home.join("tmp/x")));
        assert!(policy.may_write(&home.join(".cargo/registry/x")));
        assert!(policy.may_write(&home.join("shared/notes.txt")));
        assert!(!policy.may_write(&home.join(".bashrc")));
        assert!(policy.may_read(&home.join(".bashrc")));
        assert!(!policy.may_read(&home.join(".ssh/id_ed25519")));
        assert!(!policy.may_read(&home.join(".pi/agent/auth.json")));
        assert!(policy.may_read(&home.join(".pi/agent/settings.json")));

        let no_caches = Settings {
            tool_caches: false,
            ..Settings::default()
        };
        let policy = Policy::for_project(&no_caches, &project, &context(&home)).expect("policy");
        assert!(!policy.may_write(&home.join(".cargo/registry/x")));
    }

    #[test]
    fn a_hidden_folder_inside_the_project_stays_hidden() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        let settings = Settings {
            hidden: vec!["~/app/.env".into()],
            ..Settings::default()
        };
        let policy =
            Policy::for_project(&settings, &home.join("app"), &context(&home)).expect("policy");
        assert!(!policy.may_write(&home.join("app/.env")));
        assert!(!policy.may_read(&home.join("app/.env")));
        assert!(policy.may_write(&home.join("app/.envrc")));
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
