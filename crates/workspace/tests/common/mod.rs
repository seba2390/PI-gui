//! Shared helpers: throwaway git repositories isolated from the user's configuration.

#![allow(dead_code, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
    /// Canonical root (git reports real paths, e.g. /private/var on macOS).
    pub root: PathBuf,
}

impl TestRepo {
    /// An empty directory that is not a git repository.
    pub fn plain() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        Self { dir, root }
    }

    /// A freshly initialised repository without commits, on branch `main`.
    pub fn init() -> Self {
        let repo = Self::plain();
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// A repository with one commit containing `files`.
    pub fn with_commit(files: &[(&str, &str)]) -> Self {
        let repo = Self::init();
        for (path, contents) in files {
            repo.write(path, contents);
        }
        repo.git(&["add", "-A"]);
        repo.commit("initial");
        repo
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn write(&self, rel: &str, contents: impl AsRef<[u8]>) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub fn remove(&self, rel: &str) {
        fs::remove_file(self.path(rel)).unwrap();
    }

    pub fn commit(&self, message: &str) {
        self.git(&["commit", "-q", "--no-verify", "-m", message]);
    }

    /// Runs git in the repository with an isolated configuration; panics on failure.
    pub fn git(&self, args: &[&str]) -> String {
        let output = git_command(&self.root, self.dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

fn git_command(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]);
    cmd
}
