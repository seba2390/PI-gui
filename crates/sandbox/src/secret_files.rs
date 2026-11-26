//! Files with secrets in a project (`.env`, private keys): the sandbox keeps commands and file
//! tools from reading them, in the project and the folders the user allowed (Settings →
//! Sandboxing & Security → Keys and secrets). Examples and templates (`.env.example`) stay
//! readable.
//!
//! - macOS: Seatbelt denies reading them by name, also ones made later.
//! - Linux, bubblewrap: the ones there when a command starts are covered by an empty file.
//! - Linux, Landlock: can't hide a file inside a folder it opens; only the file tools keep to it.

use std::path::{Path, PathBuf};

/// The names hidden, as globs (`*` is any part of a name).
pub const NAMES: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials.json",
    "service-account*.json",
];

/// Endings of names that are examples, not secrets.
pub const EXAMPLES: &[&str] = &[".example", ".sample", ".template", ".dist", ".defaults"];

/// Whether a file called `name` is hidden.
pub fn matches(name: &str) -> bool {
    !EXAMPLES.iter().any(|ending| name.ends_with(ending))
        && NAMES.iter().any(|glob| glob_matches(glob, name))
}

fn glob_matches(glob: &str, name: &str) -> bool {
    match glob.split_once('*') {
        None => glob == name,
        Some((start, end)) => {
            name.len() >= start.len() + end.len() && name.starts_with(start) && name.ends_with(end)
        }
    }
}

/// [`NAMES`] as one regular expression for a name (Seatbelt's syntax).
pub fn names_regex() -> String {
    let parts: Vec<String> = NAMES
        .iter()
        .map(|glob| {
            glob.split('*')
                .map(escape)
                .collect::<Vec<_>>()
                .join("[^/]*")
        })
        .collect();
    format!("({})", parts.join("|"))
}

/// `text` with the characters regular expressions use escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()[]{}|^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Folders not worth looking into (dependencies, build output, git's own).
const SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    "dist",
    "build",
];

/// The hidden files under `roots` now, at most `limit` (for bubblewrap, which hides files one
/// by one).
pub fn find(roots: &[PathBuf], limit: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut visited = 0usize;
    for root in roots {
        walk(root, 0, &mut found, &mut visited, limit);
    }
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, visited: &mut usize, limit: usize) {
    if depth > 8 || found.len() >= limit || *visited > 50_000 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        *visited += 1;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIP.contains(&name.as_ref()) {
                walk(&entry.path(), depth + 1, found, visited, limit);
            }
        } else if matches(&name) && found.len() < limit {
            found.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_but_not_examples() {
        for name in [
            ".env",
            ".env.local",
            "server.pem",
            "tls.key",
            "id_ed25519",
            "credentials.json",
            "service-account-prod.json",
        ] {
            assert!(matches(name), "{name}");
        }
        for name in [
            ".env.example",
            ".env.sample",
            "env.ts",
            "key.rs",
            "id_ed25519.pub",
            "README.md",
            ".envrc",
        ] {
            assert!(!matches(name), "{name}");
        }
    }

    #[test]
    fn the_regex_matches_the_same_names() {
        let regex = names_regex();
        assert!(
            regex.starts_with(r"(\.env|\.env\.[^/]*|[^/]*\.pem|"),
            "{regex}"
        );
        assert_eq!(escape("/a.b/(c)"), r"/a\.b/\(c\)");
    }

    #[test]
    fn finds_them_but_not_in_dependencies() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        for path in [
            "app/.env",
            "app/.env.example",
            "node_modules/x/.env",
            "certs/server.pem",
            "src/main.rs",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
            std::fs::write(path, "x").expect("write");
        }
        let mut found = find(&[root.to_path_buf()], 10);
        found.sort();
        assert_eq!(
            found,
            [root.join("app/.env"), root.join("certs/server.pem")]
        );
    }
}
