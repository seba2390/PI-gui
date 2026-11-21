#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;
use pretty_assertions::assert_eq;
use workspace::git::{self, ChangeKind, FileChange, RepoStatus};

fn status(repo: &TestRepo) -> RepoStatus {
    git::status(&repo.root).unwrap().expect("inside a repo")
}

fn change<'a>(status: &'a RepoStatus, path: &str) -> &'a FileChange {
    status
        .files
        .iter()
        .find(|f| f.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} not in {:#?}", status.files))
}

#[test]
fn not_a_repo_is_none() {
    let dir = TestRepo::plain();
    assert_eq!(git::status(&dir.root).unwrap(), None);
}

#[test]
fn missing_directory_is_an_error() {
    let dir = TestRepo::plain();
    assert!(git::status(&dir.path("does-not-exist")).is_err());
}

#[test]
fn clean_repo() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    let s = status(&repo);
    assert_eq!(s.root, repo.root);
    assert_eq!(s.branch.as_deref(), Some("main"));
    let short = s.head_short.clone().unwrap();
    assert!(short.len() >= 4 && short.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(s.has_commits());
    assert_eq!(s.files, vec![]);
}

#[test]
fn modified_file_counts() {
    let repo = TestRepo::with_commit(&[("a.txt", "one\ntwo\nthree\n")]);
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let s = status(&repo);
    assert_eq!(
        s.files,
        vec![FileChange {
            path: "a.txt".into(),
            kind: ChangeKind::Modified,
            staged: false,
            unstaged: true,
            additions: Some(2),
            deletions: Some(1),
        }]
    );
    assert_eq!(s.totals(), (2, 1));
}

#[test]
fn staged_and_unstaged_changes_are_combined() {
    let repo = TestRepo::with_commit(&[("a.txt", "1\n2\n3\n")]);
    repo.write("a.txt", "1\n2\n3\n4\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "1\n2\n3\n4\n5\n");
    let c = change(&status(&repo), "a.txt").clone();
    assert_eq!(c.kind, ChangeKind::Modified);
    assert!(c.staged && c.unstaged);
    assert_eq!((c.additions, c.deletions), (Some(2), Some(0)));
}

#[test]
fn staged_added_file() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    repo.write("new.txt", "x\ny\n");
    repo.git(&["add", "new.txt"]);
    let c = change(&status(&repo), "new.txt").clone();
    assert_eq!(c.kind, ChangeKind::Added);
    assert!(c.staged && !c.unstaged);
    assert_eq!((c.additions, c.deletions), (Some(2), Some(0)));
}

#[test]
fn deleted_files() {
    let repo = TestRepo::with_commit(&[("gone.txt", "1\n2\n"), ("staged-gone.txt", "1\n")]);
    repo.remove("gone.txt");
    repo.git(&["rm", "-q", "staged-gone.txt"]);
    let s = status(&repo);
    let gone = change(&s, "gone.txt");
    assert_eq!(gone.kind, ChangeKind::Deleted);
    assert!(!gone.staged && gone.unstaged);
    assert_eq!((gone.additions, gone.deletions), (Some(0), Some(2)));
    let staged = change(&s, "staged-gone.txt");
    assert_eq!(staged.kind, ChangeKind::Deleted);
    assert!(staged.staged && !staged.unstaged);
    assert_eq!((staged.additions, staged.deletions), (Some(0), Some(1)));
}

#[test]
fn renamed_file() {
    let body = "line\n".repeat(20);
    let repo = TestRepo::with_commit(&[("old name.txt", &body)]);
    repo.git(&["mv", "old name.txt", "new name.txt"]);
    let s = status(&repo);
    assert_eq!(s.files.len(), 1, "{:#?}", s.files);
    let c = &s.files[0];
    assert_eq!(c.path, Path::new("new name.txt"));
    assert_eq!(
        c.kind,
        ChangeKind::Renamed {
            from: "old name.txt".into()
        }
    );
    assert!(c.staged);
    assert_eq!((c.additions, c.deletions), (Some(0), Some(0)));
}

#[test]
fn untracked_files_and_line_counts() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n"), (".gitignore", "*.log\n")]);
    repo.write("dir/nested/u.txt", "1\n2\n3");
    repo.write("empty.txt", "");
    repo.write("bin.dat", b"\x00\x01\x02binary");
    repo.write("ignored.log", "nope\n");
    repo.write("big.txt", "x\n".repeat(600_000));
    let s = status(&repo);
    let paths: Vec<PathBuf> = s.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("big.txt"),
            PathBuf::from("bin.dat"),
            PathBuf::from("dir/nested/u.txt"),
            PathBuf::from("empty.txt"),
        ]
    );
    for f in &s.files {
        assert_eq!(f.kind, ChangeKind::Untracked);
        assert!(!f.staged && f.unstaged);
    }
    assert_eq!(change(&s, "dir/nested/u.txt").additions, Some(3));
    assert_eq!(change(&s, "dir/nested/u.txt").deletions, Some(0));
    assert_eq!(change(&s, "empty.txt").additions, Some(0));
    assert_eq!(change(&s, "bin.dat").additions, None);
    assert_eq!(change(&s, "big.txt").additions, None, "over 1 MB");
}

#[test]
fn binary_tracked_change_has_no_counts() {
    let repo = TestRepo::with_commit(&[("img.bin", "\0\x01")]);
    repo.write("img.bin", b"\0\x02\x03");
    let c = change(&status(&repo), "img.bin").clone();
    assert_eq!(c.kind, ChangeKind::Modified);
    assert_eq!((c.additions, c.deletions), (None, None));
}

#[test]
fn unusual_file_names() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    repo.write("spaces in name.txt", "1\n");
    repo.write("ünïcødé.txt", "1\n2\n");
    repo.write("quote\"and\\slash.txt", "1\n");
    let s = status(&repo);
    assert_eq!(change(&s, "spaces in name.txt").additions, Some(1));
    assert_eq!(change(&s, "ünïcødé.txt").additions, Some(2));
    assert_eq!(change(&s, "quote\"and\\slash.txt").additions, Some(1));
}

#[test]
fn subdirectory_cwd_reports_root_relative_paths() {
    let repo = TestRepo::with_commit(&[("sub/a.txt", "a\n"), ("b.txt", "b\n")]);
    repo.write("sub/a.txt", "changed\n");
    repo.write("b.txt", "changed\n");
    let s = git::status(&repo.path("sub")).unwrap().unwrap();
    assert_eq!(s.root, repo.root);
    let paths: Vec<_> = s.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        paths,
        vec![PathBuf::from("b.txt"), PathBuf::from("sub/a.txt")]
    );
}

#[test]
fn detached_head_has_no_branch() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    repo.git(&["checkout", "-q", "--detach"]);
    let s = status(&repo);
    assert_eq!(s.branch, None);
    assert!(s.head_short.is_some());
}

#[test]
fn repo_without_commits() {
    let repo = TestRepo::init();
    repo.write("staged.txt", "1\n2\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "1\n");
    let s = status(&repo);
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.head_short, None);
    assert!(!s.has_commits());
    assert_eq!(
        s.files,
        vec![
            FileChange {
                path: "staged.txt".into(),
                kind: ChangeKind::Added,
                staged: true,
                unstaged: false,
                additions: Some(2),
                deletions: Some(0),
            },
            FileChange {
                path: "untracked.txt".into(),
                kind: ChangeKind::Untracked,
                staged: false,
                unstaged: true,
                additions: Some(1),
                deletions: Some(0),
            },
        ]
    );
}

#[test]
fn merge_conflict_is_reported() {
    let repo = TestRepo::with_commit(&[("c.txt", "base\n")]);
    repo.git(&["checkout", "-q", "-b", "other"]);
    repo.write("c.txt", "other\n");
    repo.git(&["commit", "-q", "-am", "other"]);
    repo.git(&["checkout", "-q", "main"]);
    repo.write("c.txt", "main\n");
    repo.git(&["commit", "-q", "-am", "main"]);
    // Expected to fail with a conflict; run without the success assertion.
    let _ = std::process::Command::new("git")
        .current_dir(&repo.root)
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@t",
            "merge",
            "-q",
            "other",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    let c = change(&status(&repo), "c.txt").clone();
    assert_eq!(c.kind, ChangeKind::Conflicted);
}

#[test]
fn status_does_not_take_index_lock() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    repo.write("a.txt", "b\n");
    // With an index.lock present (as while pi runs a git command), status must still succeed.
    std::fs::write(repo.path(".git/index.lock"), "").unwrap();
    let s = status(&repo);
    assert_eq!(s.files.len(), 1);
    assert!(repo.path(".git/index.lock").exists());
}
