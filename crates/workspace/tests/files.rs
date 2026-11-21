#![allow(clippy::unwrap_used)]

mod common;

use common::TestRepo;
use pretty_assertions::assert_eq;
use workspace::WorkspaceError;
use workspace::files::{DirEntry, list_dir};

fn names(entries: &[DirEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.name)
            } else {
                e.name.clone()
            }
        })
        .collect()
}

fn populate(dir: &TestRepo) {
    dir.write(".gitignore", "target/\n*.log\n");
    dir.write("file10.txt", "");
    dir.write("file2.txt", "");
    dir.write("File1.txt", "");
    dir.write("b.txt", "");
    dir.write("A.txt", "");
    dir.write("debug.log", "");
    dir.write(".hidden", "");
    dir.write("src/main.rs", "");
    dir.write("Docs/readme.md", "");
    dir.write("target/out.bin", "");
    dir.write(".config/settings.toml", "");
}

#[test]
fn git_repo_listing_respects_gitignore_hidden_and_order() {
    let repo = TestRepo::init();
    populate(&repo);

    let visible = list_dir(&repo.root, &repo.root, false).unwrap();
    assert_eq!(
        names(&visible),
        vec![
            "Docs/",
            "src/",
            "A.txt",
            "b.txt",
            "File1.txt",
            "file2.txt",
            "file10.txt"
        ]
    );

    let all = list_dir(&repo.root, &repo.root, true).unwrap();
    assert_eq!(
        names(&all),
        vec![
            ".config/",
            "Docs/",
            "src/",
            ".gitignore",
            ".hidden",
            "A.txt",
            "b.txt",
            "File1.txt",
            "file2.txt",
            "file10.txt",
        ],
        ".git must stay hidden even with show_hidden"
    );

    let src = &visible[1];
    assert_eq!(src.path, repo.path("src"));
    assert!(src.is_dir && !src.is_symlink);
}

#[test]
fn non_git_root_still_respects_gitignore() {
    let dir = TestRepo::plain();
    populate(&dir);
    let visible = list_dir(&dir.root, &dir.root, false).unwrap();
    assert_eq!(
        names(&visible),
        vec![
            "Docs/",
            "src/",
            "A.txt",
            "b.txt",
            "File1.txt",
            "file2.txt",
            "file10.txt"
        ]
    );
}

#[test]
fn subdirectory_uses_root_gitignore() {
    let repo = TestRepo::init();
    repo.write(".gitignore", "*.log\nbuild/\n");
    repo.write("src/lib.rs", "");
    repo.write("src/trace.log", "");
    repo.write("src/build/x.o", "");
    repo.write("src/nested/.ignore", "secret.txt\n");
    repo.write("src/nested/secret.txt", "");
    repo.write("src/nested/public.txt", "");

    let src = list_dir(&repo.root, &repo.path("src"), false).unwrap();
    assert_eq!(names(&src), vec!["nested/", "lib.rs"]);
    assert_eq!(src[1].path, repo.path("src/lib.rs"));

    // Relative `dir` is resolved against the root; `.ignore` files are honoured.
    let nested = list_dir(&repo.root, std::path::Path::new("src/nested"), false).unwrap();
    assert_eq!(names(&nested), vec!["public.txt"]);
}

#[cfg(unix)]
#[test]
fn symlinks_are_flagged() {
    let dir = TestRepo::plain();
    dir.write("real/file.txt", "");
    dir.write("plain.txt", "");
    std::os::unix::fs::symlink(dir.path("real"), dir.path("link-to-dir")).unwrap();
    std::os::unix::fs::symlink(dir.path("plain.txt"), dir.path("link-to-file")).unwrap();
    std::os::unix::fs::symlink(dir.path("missing"), dir.path("dangling")).unwrap();

    let entries = list_dir(&dir.root, &dir.root, false).unwrap();
    assert_eq!(
        names(&entries),
        vec![
            "link-to-dir/",
            "real/",
            "dangling",
            "link-to-file",
            "plain.txt"
        ]
    );
    let link = &entries[0];
    assert!(link.is_symlink && link.is_dir);
    let dangling = &entries[2];
    assert!(dangling.is_symlink && !dangling.is_dir);
}

#[test]
fn errors() {
    let dir = TestRepo::plain();
    dir.write("file.txt", "");
    let other = TestRepo::plain();
    assert!(matches!(
        list_dir(&dir.root, &other.root, false),
        Err(WorkspaceError::OutsideRoot { .. })
    ));
    assert!(matches!(
        list_dir(&dir.root, &dir.path("missing"), false),
        Err(WorkspaceError::Io(_))
    ));
    assert!(matches!(
        list_dir(&dir.root, &dir.path("file.txt"), false),
        Err(WorkspaceError::Io(_))
    ));
}

#[test]
fn empty_directory() {
    let dir = TestRepo::plain();
    assert_eq!(list_dir(&dir.root, &dir.root, true).unwrap(), vec![]);
}
