//! Paths as the sandbox sees them: `~/` expanded, links resolved.

use std::path::{Component, Path, PathBuf};

/// The home folder (`$HOME`).
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `path` with a leading `~` or `~/` replaced by `home`.
pub fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// `path` with the home folder written as `~` (for settings people read).
pub fn tilde(path: &Path) -> String {
    match home().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `path` with every link resolved, as the kernel checks it: the part that exists is
/// canonicalized and the rest appended (without `.` and `..`). A relative path stays relative.
pub fn resolve(path: &Path) -> PathBuf {
    let path = lexical(path);
    let mut existing = path.clone();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut resolved = real;
            for part in rest.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_owned());
                existing = parent.to_path_buf();
            }
            _ => return path,
        }
    }
}

/// `path` without `.` and with `..` applied to what comes before it.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_the_home_folder_only_at_the_start() {
        let home = Path::new("/home/ana");
        assert_eq!(expand("~", Some(home)), PathBuf::from("/home/ana"));
        assert_eq!(
            expand("~/.ssh", Some(home)),
            PathBuf::from("/home/ana/.ssh")
        );
        assert_eq!(expand("~bob/x", Some(home)), PathBuf::from("~bob/x"));
        assert_eq!(expand("/etc/~/x", Some(home)), PathBuf::from("/etc/~/x"));
        assert_eq!(expand("~/.ssh", None), PathBuf::from("~/.ssh"));
    }

    #[test]
    fn resolves_links_also_below_missing_folders() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir(real.join("target")).expect("mkdir");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(real.join("target"), real.join("link")).expect("symlink");
            assert_eq!(
                resolve(&dir.path().join("link/new/file.txt")),
                real.join("target/new/file.txt")
            );
        }
        assert_eq!(
            resolve(&dir.path().join("missing/../target")),
            real.join("target")
        );
    }
}
