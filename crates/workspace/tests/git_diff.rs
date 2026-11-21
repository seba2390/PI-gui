#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use common::TestRepo;
use workspace::WorkspaceError;
use workspace::git::{self, ChangeKind};

#[test]
fn tracked_file_diff() {
    let repo = TestRepo::with_commit(&[("a.txt", "one\ntwo\n"), ("b.txt", "b\n")]);
    repo.write("a.txt", "one\n2\n");
    repo.write("b.txt", "changed\n");
    let diff = git::file_diff(&repo.root, Path::new("a.txt")).unwrap();
    assert!(diff.starts_with("diff --git a/a.txt b/a.txt\n"), "{diff}");
    assert!(diff.contains("\n-two\n+2\n"), "{diff}");
    assert!(!diff.contains("b.txt"), "only the requested file: {diff}");
}

#[test]
fn staged_and_unstaged_changes_are_combined() {
    let repo = TestRepo::with_commit(&[("a.txt", "1\n")]);
    repo.write("a.txt", "1\n2\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "1\n2\n3\n");
    let diff = git::file_diff(&repo.root, Path::new("a.txt")).unwrap();
    assert!(diff.contains("\n+2\n+3\n"), "{diff}");
}

#[test]
fn absolute_path_and_subdirectory_cwd() {
    let repo = TestRepo::with_commit(&[("sub/a.txt", "x\n")]);
    repo.write("sub/a.txt", "y\n");
    let diff = git::file_diff(&repo.path("sub"), &repo.path("sub/a.txt")).unwrap();
    assert!(
        diff.starts_with("diff --git a/sub/a.txt b/sub/a.txt\n"),
        "{diff}"
    );
}

#[test]
fn untracked_file_diff() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    repo.write("new dir/new file.txt", "hello\nworld\n");
    let diff = git::file_diff(&repo.root, Path::new("new dir/new file.txt")).unwrap();
    assert!(
        diff.starts_with("diff --git a/new dir/new file.txt b/new dir/new file.txt\n"),
        "{diff}"
    );
    assert!(diff.contains("new file mode"), "{diff}");
    assert!(diff.contains("--- /dev/null"), "{diff}");
    assert!(diff.contains("\n+hello\n+world\n"), "{diff}");
}

#[test]
fn deleted_file_diff() {
    let repo = TestRepo::with_commit(&[("gone.txt", "bye\n")]);
    repo.remove("gone.txt");
    let diff = git::file_diff(&repo.root, Path::new("gone.txt")).unwrap();
    assert!(diff.contains("deleted file mode"), "{diff}");
    assert!(diff.contains("\n-bye\n"), "{diff}");
    let abs = git::file_diff(&repo.root, &repo.path("gone.txt")).unwrap();
    assert_eq!(abs, diff);
}

#[test]
fn renamed_file_diff_shows_rename() {
    let body = "line\n".repeat(20);
    let repo = TestRepo::with_commit(&[("old.txt", &body)]);
    repo.git(&["mv", "old.txt", "new.txt"]);
    let diff = git::file_diff(&repo.root, Path::new("new.txt")).unwrap();
    assert!(
        diff.contains("rename from old.txt\nrename to new.txt"),
        "{diff}"
    );
}

#[test]
fn unchanged_file_has_empty_diff() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    assert_eq!(git::file_diff(&repo.root, Path::new("a.txt")).unwrap(), "");
}

#[test]
fn glob_characters_are_literal() {
    let repo = TestRepo::with_commit(&[("a*.txt", "1\n"), ("ab.txt", "1\n")]);
    repo.write("a*.txt", "2\n");
    repo.write("ab.txt", "2\n");
    let diff = git::file_diff(&repo.root, Path::new("a*.txt")).unwrap();
    assert!(diff.contains("a/a*.txt"), "{diff}");
    assert!(!diff.contains("ab.txt"), "pathspec must not glob: {diff}");
}

#[test]
fn repo_without_commits_diffs_against_empty_tree() {
    let repo = TestRepo::init();
    repo.write("staged.txt", "s\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("untracked.txt", "u\n");
    let staged = git::file_diff(&repo.root, Path::new("staged.txt")).unwrap();
    assert!(staged.contains("new file mode"), "{staged}");
    assert!(staged.contains("\n+s\n"), "{staged}");
    let untracked = git::file_diff(&repo.root, Path::new("untracked.txt")).unwrap();
    assert!(untracked.contains("\n+u\n"), "{untracked}");

    let full = git::full_diff(&repo.root, 1 << 20).unwrap();
    assert!(
        full.contains("b/staged.txt") && full.contains("b/untracked.txt"),
        "{full}"
    );
}

#[test]
fn change_diff_matches_file_diff() {
    let body = "line\n".repeat(20);
    let repo = TestRepo::with_commit(&[("m.txt", "1\n"), ("old.txt", &body)]);
    repo.write("m.txt", "2\n");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("u.txt", "u\n");
    let status = git::status(&repo.root).unwrap().unwrap();
    assert_eq!(status.files.len(), 3);
    for change in &status.files {
        let fast = git::change_diff(&status, change).unwrap();
        let slow = git::file_diff(&repo.root, &change.path).unwrap();
        assert_eq!(fast, slow, "{:?}", change.path);
        assert_ne!(fast, "");
        if let ChangeKind::Renamed { .. } = change.kind {
            assert!(fast.contains("rename from old.txt"), "{fast}");
        }
    }
}

#[test]
fn full_diff_includes_tracked_then_untracked() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n"), (".gitignore", "*.log\n")]);
    repo.write("a.txt", "A\n");
    repo.write("z-untracked.txt", "new\n");
    repo.write("ignored.log", "nope\n");
    let full = git::full_diff(&repo.root, 1 << 20).unwrap();
    let tracked = full.find("diff --git a/a.txt").unwrap();
    let untracked = full.find("diff --git a/z-untracked.txt").unwrap();
    assert!(tracked < untracked, "{full}");
    assert!(!full.contains("ignored.log"), "{full}");
    assert!(!full.contains(git::TRUNCATION_MARKER), "{full}");
}

#[test]
fn full_diff_is_truncated_at_line_boundary() {
    let repo = TestRepo::with_commit(&[("a.txt", &"a\n".repeat(1000))]);
    repo.write("a.txt", "b\n".repeat(1000));
    repo.write("u.txt", "u\n".repeat(1000));
    let full = git::full_diff(&repo.root, 500).unwrap();
    let (body, marker) = full.split_at(full.find(git::TRUNCATION_MARKER).unwrap());
    assert!(body.len() <= 500, "{}", body.len());
    assert!(body.ends_with('\n'));
    assert!(marker.ends_with("]\n"), "{marker}");
    assert!(!full.contains("u.txt"));

    // Budget exhausted while appending untracked files.
    let uncapped = git::full_diff(&repo.root, 1 << 20).unwrap();
    let cap = uncapped.find("diff --git a/u.txt").unwrap() + 100;
    let cut = git::full_diff(&repo.root, cap).unwrap();
    assert!(cut.contains("diff --git a/u.txt"), "{cut}");
    assert!(cut.contains(git::TRUNCATION_MARKER), "{cut}");
}

#[test]
fn not_a_repo_is_an_error() {
    let dir = TestRepo::plain();
    dir.write("a.txt", "a\n");
    assert!(matches!(
        git::file_diff(&dir.root, Path::new("a.txt")),
        Err(WorkspaceError::NotARepo(_))
    ));
    assert!(matches!(
        git::full_diff(&dir.root, 100),
        Err(WorkspaceError::NotARepo(_))
    ));
}

#[test]
fn path_outside_repo_is_rejected() {
    let repo = TestRepo::with_commit(&[("a.txt", "a\n")]);
    let other = TestRepo::plain();
    other.write("x.txt", "x\n");
    assert!(matches!(
        git::file_diff(&repo.root, &other.path("x.txt")),
        Err(WorkspaceError::OutsideRoot { .. })
    ));
}
