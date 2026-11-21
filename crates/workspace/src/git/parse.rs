//! Pure parsers for git's machine-readable output. No process execution happens here, so every
//! function can be unit-tested with literal byte strings.

use std::path::PathBuf;

use super::ChangeKind;

/// Converts raw path bytes from git into a `PathBuf` without losing non-UTF-8 names on Unix.
pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        // Git emits UTF-8 paths on Windows.
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Output of `git rev-parse --show-toplevel --verify -q --short HEAD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevParse {
    pub toplevel: PathBuf,
    /// `None` when HEAD does not resolve (repository without commits).
    pub head_short: Option<String>,
}

/// Parses the stdout of `git rev-parse --show-toplevel --verify -q --short HEAD`: the toplevel
/// on the first line and, if HEAD resolves, the abbreviated sha on the second.
pub(crate) fn parse_rev_parse(stdout: &[u8]) -> Option<RevParse> {
    let mut lines = stdout
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let toplevel = lines.next().filter(|l| !l.is_empty())?;
    let head_short = lines
        .next()
        .filter(|l| !l.is_empty())
        .map(|l| String::from_utf8_lossy(l).into_owned());
    Some(RevParse {
        toplevel: path_from_bytes(toplevel),
        head_short,
    })
}

/// One path record from `git status --porcelain=v2 -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusEntry {
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub staged: bool,
    pub unstaged: bool,
}

/// Parsed `git status --porcelain=v2 -z [--branch]` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedStatus {
    /// `branch.head`; `None` when detached (or `--branch` was not passed).
    pub branch: Option<String>,
    /// `branch.oid`; `None` for `(initial)` (or `--branch` was not passed).
    pub oid: Option<String>,
    pub entries: Vec<StatusEntry>,
}

fn split_fields(record: &[u8], n: usize) -> Vec<&[u8]> {
    record.splitn(n, |&b| b == b' ').collect()
}

/// Splits an `XY` field into (staged, unstaged) status letters.
fn xy(field: &[u8]) -> (u8, u8) {
    (
        field.first().copied().unwrap_or(b'.'),
        field.get(1).copied().unwrap_or(b'.'),
    )
}

/// Classifies an ordinary (`1`) record from its `XY` letters.
fn ordinary_kind(x: u8, y: u8) -> ChangeKind {
    match (x, y) {
        (b'A', _) | (_, b'A') => ChangeKind::Added,
        (b'D', _) | (_, b'D') => ChangeKind::Deleted,
        (b'T', _) | (_, b'T') => ChangeKind::TypeChanged,
        _ => ChangeKind::Modified,
    }
}

/// Parses `git status --porcelain=v2 -z` output. Unknown or malformed records are skipped;
/// ignored (`!`) entries are dropped.
///
/// Record formats (each NUL-terminated):
/// * `# branch.oid <sha|(initial)>`, `# branch.head <name|(detached)>`, other headers
/// * `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
/// * `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>` NUL `<origPath>`
/// * `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
/// * `? <path>`, `! <path>`
pub(crate) fn parse_status(out: &[u8]) -> ParsedStatus {
    let mut parsed = ParsedStatus::default();
    let mut records = out.split(|&b| b == 0);
    while let Some(record) = records.next() {
        let Some(&tag) = record.first() else {
            continue;
        };
        match tag {
            b'#' => parse_header(record, &mut parsed),
            b'1' => {
                if let [_, xy_field, .., path] = split_fields(record, 9)[..]
                    && !path.is_empty()
                {
                    let (x, y) = xy(xy_field);
                    parsed.entries.push(StatusEntry {
                        path: path_from_bytes(path),
                        kind: ordinary_kind(x, y),
                        staged: x != b'.',
                        unstaged: y != b'.',
                    });
                }
            }
            b'2' => {
                // The original path is always the next NUL-separated token; consume it even if
                // this record turns out to be malformed so the stream stays in sync.
                let orig = records.next();
                let fields = split_fields(record, 10);
                if fields.len() == 10
                    && let (Some(path), Some(orig)) = (fields.last(), orig)
                {
                    let (x, y) = xy(fields[1]);
                    let kind = if x == b'C' {
                        // A copy leaves the source in place: from the diff's view it's a new file.
                        ChangeKind::Added
                    } else {
                        ChangeKind::Renamed {
                            from: path_from_bytes(orig),
                        }
                    };
                    parsed.entries.push(StatusEntry {
                        path: path_from_bytes(path),
                        kind,
                        staged: x != b'.',
                        unstaged: y != b'.',
                    });
                }
            }
            b'u' => {
                let fields = split_fields(record, 11);
                if fields.len() == 11
                    && let Some(path) = fields.last()
                {
                    parsed.entries.push(StatusEntry {
                        path: path_from_bytes(path),
                        kind: ChangeKind::Conflicted,
                        staged: false,
                        unstaged: true,
                    });
                }
            }
            b'?' => {
                if let Some(path) = record.get(2..).filter(|p| !p.is_empty()) {
                    parsed.entries.push(StatusEntry {
                        path: path_from_bytes(path),
                        kind: ChangeKind::Untracked,
                        staged: false,
                        unstaged: true,
                    });
                }
            }
            b'!' => {}
            _ => log::debug!(
                "skipping unknown git status record: {}",
                String::from_utf8_lossy(record)
            ),
        }
    }
    parsed
}

fn parse_header(record: &[u8], parsed: &mut ParsedStatus) {
    let text = String::from_utf8_lossy(record);
    if let Some(oid) = text.strip_prefix("# branch.oid ") {
        parsed.oid = (oid != "(initial)").then(|| oid.to_owned());
    } else if let Some(head) = text.strip_prefix("# branch.head ") {
        parsed.branch = (head != "(detached)").then(|| head.to_owned());
    }
}

/// One record of `git diff --numstat -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumstatEntry {
    /// Destination path (the new name for renames).
    pub path: PathBuf,
    /// `None` for binary files (`-`).
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

fn parse_count(field: &[u8]) -> Option<u32> {
    std::str::from_utf8(field).ok()?.trim().parse().ok()
}

/// Parses `git diff --numstat -z` output.
///
/// Records are `<added>\t<deleted>\t<path>` NUL, or for renames/copies
/// `<added>\t<deleted>\t` NUL `<src>` NUL `<dst>` NUL. Binary files report `-` for both counts.
pub(crate) fn parse_numstat(out: &[u8]) -> Vec<NumstatEntry> {
    let mut entries = Vec::new();
    let mut tokens = out.split(|&b| b == 0);
    while let Some(record) = tokens.next() {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |&b| b == b'\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            log::debug!(
                "skipping malformed numstat record: {}",
                String::from_utf8_lossy(record)
            );
            continue;
        };
        let path = if path.is_empty() {
            let _src = tokens.next();
            tokens.next()
        } else {
            Some(path)
        };
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            continue;
        };
        entries.push(NumstatEntry {
            path: path_from_bytes(path),
            additions: parse_count(added),
            deletions: parse_count(deleted),
        });
    }
    entries
}

/// Splits NUL-separated path output (e.g. `git ls-files -z`).
pub(crate) fn parse_nul_paths(out: &[u8]) -> Vec<PathBuf> {
    out.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(path_from_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn entry(path: &str, kind: ChangeKind, staged: bool, unstaged: bool) -> StatusEntry {
        StatusEntry {
            path: PathBuf::from(path),
            kind,
            staged,
            unstaged,
        }
    }

    #[test]
    fn rev_parse_with_and_without_head() {
        assert_eq!(
            parse_rev_parse(b"/repo/root\nabc1234\n"),
            Some(RevParse {
                toplevel: PathBuf::from("/repo/root"),
                head_short: Some("abc1234".into()),
            })
        );
        assert_eq!(
            parse_rev_parse(b"/repo/root\n"),
            Some(RevParse {
                toplevel: PathBuf::from("/repo/root"),
                head_short: None,
            })
        );
        assert_eq!(parse_rev_parse(b""), None);
    }

    #[test]
    fn status_headers() {
        let out = b"# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +1 -0\0";
        let parsed = parse_status(out);
        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert_eq!(parsed.oid.as_deref(), Some("1234567890abcdef"));
        assert_eq!(parsed.entries, vec![]);

        let parsed = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(parsed.branch, None);
        assert_eq!(parsed.oid, None);
    }

    #[test]
    fn status_ordinary_records() {
        let z = "0000000000000000000000000000000000000000";
        let h = "de980441c3ab03a8c07dda1ad27b8a11f39deb1e";
        let out = format!(
            "1 .M N... 100644 100644 100644 {h} {h} modified.txt\0\
             1 A. N... 000000 100644 100644 {z} {h} added file.txt\0\
             1 AM N... 000000 100644 100644 {z} {h} added-then-edited.txt\0\
             1 D. N... 100644 000000 000000 {h} {z} staged-delete.txt\0\
             1 .D N... 100644 100644 000000 {h} {h} worktree-delete.txt\0\
             1 MM N... 100644 100644 100644 {h} {h} both.txt\0\
             1 .T N... 100644 100644 120000 {h} {h} typechange\0\
             1 .A N... 000000 000000 100644 {z} {z} intent-to-add.txt\0"
        );
        let parsed = parse_status(out.as_bytes());
        assert_eq!(
            parsed.entries,
            vec![
                entry("modified.txt", ChangeKind::Modified, false, true),
                entry("added file.txt", ChangeKind::Added, true, false),
                entry("added-then-edited.txt", ChangeKind::Added, true, true),
                entry("staged-delete.txt", ChangeKind::Deleted, true, false),
                entry("worktree-delete.txt", ChangeKind::Deleted, false, true),
                entry("both.txt", ChangeKind::Modified, true, true),
                entry("typechange", ChangeKind::TypeChanged, false, true),
                entry("intent-to-add.txt", ChangeKind::Added, false, true),
            ]
        );
    }

    #[test]
    fn status_rename_copy_conflict_untracked() {
        let h = "de980441c3ab03a8c07dda1ad27b8a11f39deb1e";
        let out = format!(
            "2 R. N... 100644 100644 100644 {h} {h} R100 new name.txt\0old name.txt\0\
             2 C. N... 100644 100644 100644 {h} {h} C75 copy.txt\0orig.txt\0\
             u UU N... 100644 100644 100644 100644 {h} {h} {h} conflict.txt\0\
             ? untracked dir/file.txt\0\
             ! ignored.log\0"
        );
        let parsed = parse_status(out.as_bytes());
        assert_eq!(
            parsed.entries,
            vec![
                entry(
                    "new name.txt",
                    ChangeKind::Renamed {
                        from: PathBuf::from("old name.txt")
                    },
                    true,
                    false
                ),
                entry("copy.txt", ChangeKind::Added, true, false),
                entry("conflict.txt", ChangeKind::Conflicted, false, true),
                entry("untracked dir/file.txt", ChangeKind::Untracked, false, true),
            ]
        );
    }

    #[test]
    fn status_malformed_input_does_not_panic() {
        let inputs: &[&[u8]] = &[
            b"",
            b"\0\0\0",
            b"1\0",
            b"1 .M\0",
            b"2 R. N... 1 2 3 a b R100 path-without-orig",
            b"u UU\0",
            b"?\0? \0",
            b"#\0# branch.head\0",
            b"x garbage\0",
            &[0xff, 0xfe, 0x00, b'?', b' ', 0xff, 0x00],
        ];
        for input in inputs {
            let _ = parse_status(input);
        }
        let parsed = parse_status(b"2 R. N... 1 2 3 a b R100 path-without-orig");
        assert_eq!(parsed.entries, vec![]);
    }

    #[cfg(unix)]
    #[test]
    fn status_non_utf8_path_is_preserved() {
        use std::os::unix::ffi::OsStrExt as _;
        let parsed = parse_status(b"? caf\xe9.txt\0");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(
            parsed.entries[0].path.as_os_str().as_bytes(),
            b"caf\xe9.txt"
        );
    }

    #[test]
    fn numstat_records() {
        let out = b"3\t1\tsrc/main.rs\0-\t-\timage.png\0\
                    0\t0\t\0old.txt\0new.txt\0\
                    10\t0\twith\ttab.txt\0";
        assert_eq!(
            parse_numstat(out),
            vec![
                NumstatEntry {
                    path: "src/main.rs".into(),
                    additions: Some(3),
                    deletions: Some(1),
                },
                NumstatEntry {
                    path: "image.png".into(),
                    additions: None,
                    deletions: None,
                },
                NumstatEntry {
                    path: "new.txt".into(),
                    additions: Some(0),
                    deletions: Some(0),
                },
                NumstatEntry {
                    path: "with\ttab.txt".into(),
                    additions: Some(10),
                    deletions: Some(0),
                },
            ]
        );
    }

    #[test]
    fn numstat_malformed_input_does_not_panic() {
        for input in [
            &b""[..],
            b"\0",
            b"1\0",
            b"1\t2\t\0",
            b"1\t2\t\0only-src\0",
            b"x\ty\tp\0",
        ] {
            let _ = parse_numstat(input);
        }
        assert_eq!(
            parse_numstat(b"x\ty\tp\0"),
            vec![NumstatEntry {
                path: "p".into(),
                additions: None,
                deletions: None,
            }]
        );
    }

    #[test]
    fn nul_paths() {
        assert_eq!(
            parse_nul_paths(b"a.txt\0dir/b c.txt\0\0"),
            vec![PathBuf::from("a.txt"), PathBuf::from("dir/b c.txt")]
        );
    }
}
