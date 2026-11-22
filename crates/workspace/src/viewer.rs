//! Reading a file for the read-only viewer in the "Files" tab.
//!
//! Only the start of a file is read ([`read_head_at`]), so a huge log or a disk image never
//! loads whole; [`FileContent::classify`] then decides whether it is text worth showing.

use std::fs;
use std::io::Read as _;
use std::path::Path;

use crate::{Location, WorkspaceError, with_args};

/// The viewer shows at most this many bytes of a file; longer files open cut, with a notice.
pub const MAX_VIEW_BYTES: usize = 512 * 1024;

/// Bytes looked at to tell text from binary.
const SNIFF_BYTES: usize = 8 * 1024;

/// Prints the file's size on the first line, then up to `$2` bytes of it.
const READ_HEAD_SCRIPT: &str = r#"[ -f "$1" ] || { printf 'Not a file: %s\n' "$1" >&2; exit 1; }
[ -r "$1" ] || { printf 'Cannot read %s\n' "$1" >&2; exit 1; }
wc -c < "$1" | tr -d ' '
head -c "$2" -- "$1""#;

/// The start of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHead {
    /// Up to the requested number of bytes from the start.
    pub bytes: Vec<u8>,
    /// The whole file's size.
    pub size: u64,
}

impl FileHead {
    /// Only part of the file was read.
    pub fn truncated(&self) -> bool {
        (self.bytes.len() as u64) < self.size
    }
}

/// Reads up to `max_bytes` from the start of `path`, which must be inside `root`, on this
/// computer or at `loc`.
pub fn read_head_at(
    loc: &Location,
    root: &Path,
    path: &Path,
    max_bytes: usize,
) -> Result<FileHead, WorkspaceError> {
    let path = if path.is_relative() {
        root.join(path)
    } else {
        path.to_path_buf()
    };
    if !path.starts_with(root) {
        return Err(WorkspaceError::OutsideRoot {
            path,
            root: root.to_path_buf(),
        });
    }
    if let Location::Remote(shell) = loc {
        let path_text = path.to_string_lossy();
        let script = with_args(
            READ_HEAD_SCRIPT,
            &[path_text.as_ref(), &max_bytes.to_string()],
        );
        // The size line comes first; allow for it on top of the content.
        let output = shell
            .run(&script, None, Some(max_bytes + 32))
            .map_err(|error| WorkspaceError::Remote(error.to_string()))?;
        if output.code.is_some_and(|code| code != 0) {
            let message = output.stderr.trim();
            return Err(WorkspaceError::Remote(if message.is_empty() {
                format!("could not read {path_text}")
            } else {
                message.to_owned()
            }));
        }
        return parse_remote_head(&output.stdout, max_bytes)
            .ok_or_else(|| WorkspaceError::Remote(format!("could not read {path_text}")));
    }
    let meta = fs::metadata(&path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a file: {}", path.display()),
        )
        .into());
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(meta.len())
            .unwrap_or(max_bytes)
            .min(max_bytes),
    );
    fs::File::open(&path)?
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)?;
    Ok(FileHead {
        bytes,
        size: meta.len(),
    })
}

/// Splits [`READ_HEAD_SCRIPT`]'s output into the size line and the content.
fn parse_remote_head(stdout: &[u8], max_bytes: usize) -> Option<FileHead> {
    let newline = stdout.iter().position(|&byte| byte == b'\n')?;
    let size = std::str::from_utf8(&stdout[..newline])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let mut bytes = stdout[newline + 1..].to_vec();
    bytes.truncate(max_bytes);
    Some(FileHead { bytes, size })
}

/// What the viewer can show of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    /// Not text (a NUL byte, or mostly invalid UTF-8, near the start).
    Binary,
    Text {
        text: String,
        /// Only the first part is shown; it ends at a whole line.
        truncated: bool,
    },
}

impl FileContent {
    pub fn classify(head: &FileHead) -> Self {
        let sniff = &head.bytes[..head.bytes.len().min(SNIFF_BYTES)];
        if sniff.contains(&0) {
            return Self::Binary;
        }
        let truncated = head.truncated();
        let mut bytes = head.bytes.as_slice();
        if truncated {
            // Cut at the last whole line, which also drops a character split by the cut.
            if let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') {
                bytes = &bytes[..=end];
            }
        }
        let text = String::from_utf8_lossy(bytes);
        // A few bad bytes (a Latin-1 name in a comment) still read as text; a file made of
        // them is not.
        let replaced = text
            .chars()
            .take(SNIFF_BYTES)
            .filter(|&c| c == '\u{FFFD}')
            .count();
        let checked = text.chars().take(SNIFF_BYTES).count();
        if replaced * 10 > checked {
            return Self::Binary;
        }
        Self::Text {
            text: text.into_owned(),
            truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::testing::LocalSh;
    use std::sync::Arc;

    fn head(bytes: &[u8], size: u64) -> FileHead {
        FileHead {
            bytes: bytes.to_vec(),
            size,
        }
    }

    #[test]
    fn text_and_binary() {
        assert_eq!(
            FileContent::classify(&head(b"fn main() {}\n", 13)),
            FileContent::Text {
                text: "fn main() {}\n".into(),
                truncated: false,
            }
        );
        assert_eq!(
            FileContent::classify(&head(b"\x89PNG\r\n\x1a\n\0\0", 10)),
            FileContent::Binary
        );
        // Mostly invalid UTF-8: binary. One stray byte in text: still text.
        assert_eq!(
            FileContent::classify(&head(&[0xff; 64], 64)),
            FileContent::Binary
        );
        let mut latin1 = b"# caf".to_vec();
        latin1.push(0xe9);
        latin1.extend_from_slice(b" menu\nprint('ok')\n");
        assert!(matches!(
            FileContent::classify(&head(&latin1, latin1.len() as u64)),
            FileContent::Text { .. }
        ));
    }

    #[test]
    fn a_cut_file_ends_at_a_whole_line() {
        // "é" is two bytes; the cut falls inside it.
        let bytes = "one\ntwo\nthré".as_bytes();
        let cut = &bytes[..bytes.len() - 1];
        assert_eq!(
            FileContent::classify(&head(cut, 1000)),
            FileContent::Text {
                text: "one\ntwo\n".into(),
                truncated: true,
            }
        );
    }

    #[test]
    fn reads_only_the_head_locally_and_remotely() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("big.txt");
        let content = (0..1000).fold(String::new(), |mut text, i| {
            use std::fmt::Write as _;
            let _ = writeln!(text, "line {i}");
            text
        });
        fs::write(&file, &content).unwrap();
        let remote = Location::Remote(Arc::new(LocalSh));
        for loc in [Location::Local, remote] {
            let head = read_head_at(&loc, &root, Path::new("big.txt"), 100).unwrap();
            assert_eq!(head.size, content.len() as u64, "{loc:?}");
            assert_eq!(head.bytes, content.as_bytes()[..100], "{loc:?}");
            assert!(head.truncated());
            let whole = read_head_at(&loc, &root, &file, MAX_VIEW_BYTES).unwrap();
            assert!(!whole.truncated(), "{loc:?}");
            assert!(read_head_at(&loc, &root, Path::new("missing"), 10).is_err());
            assert!(
                read_head_at(&loc, &root, &root, 10).is_err(),
                "a folder is not a file"
            );
        }
        assert!(matches!(
            read_head_at(&Location::Local, &root, Path::new("/etc/hosts"), 10),
            Err(WorkspaceError::OutsideRoot { .. })
        ));
    }
}
