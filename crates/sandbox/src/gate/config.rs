//! A repository's git settings, read as data (never by running git outside the sandbox): which
//! remotes it has and where they are, and its branch. git with the user's keys never reads them;
//! it works in the gate's own mirror ([`super::relay`]).

use std::path::{Path, PathBuf};

/// One setting: `section.subsection.name` (section and name in lowercase, the subsection as
/// written) and its value (`true` when it has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub section: String,
    pub subsection: Option<String>,
    pub name: String,
    pub value: String,
}

impl Entry {
    /// `section.subsection.name`.
    pub fn key(&self) -> String {
        match &self.subsection {
            Some(sub) => format!("{}.{sub}.{}", self.section, self.name),
            None => format!("{}.{}", self.section, self.name),
        }
    }
}

/// The settings in a git config file's text. Lines that can't be read are left out; continued
/// lines (a `\` at the end) are joined.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut section = String::new();
    let mut subsection: Option<String> = None;
    let mut joined = String::new();
    for raw in text.lines() {
        if let Some(rest) = raw.strip_suffix('\\') {
            joined.push_str(rest);
            continue;
        }
        joined.push_str(raw);
        let line = std::mem::take(&mut joined);
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let Some(end) = header.find(']') else {
                continue;
            };
            let header = header[..end].trim();
            if let Some((name, rest)) = header.split_once(char::is_whitespace) {
                section = name.to_lowercase();
                let rest = rest.trim();
                subsection = Some(
                    rest.strip_prefix('"')
                        .and_then(|rest| rest.strip_suffix('"'))
                        .unwrap_or(rest)
                        .replace("\\\"", "\"")
                        .replace("\\\\", "\\"),
                );
            } else if let Some((name, sub)) = header.split_once('.') {
                // The old `[section.subsection]` form.
                section = name.to_lowercase();
                subsection = Some(sub.to_owned());
            } else {
                section = header.to_lowercase();
                subsection = None;
            }
            continue;
        }
        if section.is_empty() {
            continue;
        }
        let (name, value) = match line.split_once('=') {
            Some((name, value)) => (name.trim(), unquote(value.trim())),
            None => (line, "true".to_owned()),
        };
        entries.push(Entry {
            section: section.clone(),
            subsection: subsection.clone(),
            name: name.to_lowercase(),
            value,
        });
    }
    entries
}

/// A value without its quotes and trailing comment.
fn unquote(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut quoted = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            },
            '#' | ';' if !quoted => break,
            other => out.push(other),
        }
    }
    out.trim_end().to_owned()
}

/// A remote's address that git hands to a program of its own (`helper::address`, `ext::…`),
/// or that reads files on this computer, or could be read as an option.
pub fn unsafe_url(url: &str) -> bool {
    let url = url.trim();
    let lower = url.to_lowercase();
    url.is_empty()
        || url.starts_with('-')
        || url.contains("::")
        || lower.starts_with("file:")
        || url.starts_with('/')
        || url.starts_with('.')
        || url.starts_with('~')
        // A Windows path, or `C:/…`.
        || url.get(1..2) == Some(":")
}

/// Where a repository keeps its git data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The folder with the `.git` in it.
    pub top: PathBuf,
    /// The `.git` folder (or where a `.git` file points).
    pub git_dir: PathBuf,
    /// What linked worktrees share: the main `.git` folder.
    pub common_dir: PathBuf,
}

impl Repo {
    /// The repository `dir` is in, if any.
    pub fn find(dir: &Path) -> Option<Self> {
        for top in dir.ancestors() {
            let dot_git = top.join(".git");
            let git_dir = if dot_git.is_dir() {
                dot_git
            } else if dot_git.is_file() {
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let pointer = text.trim().strip_prefix("gitdir:")?.trim();
                top.join(pointer)
            } else {
                continue;
            };
            let git_dir = git_dir.canonicalize().ok()?;
            let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
                .ok()
                .and_then(|common| git_dir.join(common.trim()).canonicalize().ok())
                .unwrap_or_else(|| git_dir.clone());
            return Some(Self {
                top: top.canonicalize().ok()?,
                git_dir,
                common_dir,
            });
        }
        None
    }

    /// The repository's settings (not its submodules').
    pub fn entries(&self) -> Vec<Entry> {
        let mut files = vec![self.common_dir.join("config")];
        if self.git_dir != self.common_dir {
            files.push(self.git_dir.join("config.worktree"));
        }
        files
            .iter()
            .filter_map(|file| std::fs::read_to_string(file).ok())
            .flat_map(|text| parse(&text))
            .collect()
    }

    /// The first value of `key` (`section.subsection.name`, matched the way git does).
    pub fn get(&self, section: &str, subsection: Option<&str>, name: &str) -> Option<String> {
        self.entries()
            .into_iter()
            .rev()
            .find(|entry| {
                entry.section == section
                    && entry.subsection.as_deref() == subsection
                    && entry.name == name
            })
            .map(|entry| entry.value)
    }

    /// The checked-out branch, unless `HEAD` is detached.
    pub fn branch(&self) -> Option<String> {
        let head = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        head.trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_owned)
    }
}

/// A remote's address as `host/owner/name` (no scheme, user, port or `.git`), to show and to
/// remember approvals by: `git@github.com:a/b.git` and `https://github.com/a/b` are the same.
pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.to_owned(),
        // scp-like `user@host:path`.
        None => match url.split_once(':') {
            Some((host, path)) => format!("{host}/{path}"),
            None => url.to_owned(),
        },
    };
    let rest = rest
        .rsplit_once('@')
        .map_or(rest.as_str(), |(_, rest)| rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.split(':').next().unwrap_or(host).to_lowercase();
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        host
    } else {
        format!("{host}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const CONFIG: &str = r#"
[core]
	repositoryformatversion = 0
	filemode = true
	bare = false
[remote "origin"]
	url = git@github.com:seba2390/pistachio.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
	remote = origin
	merge = refs/heads/main ; a comment
	vscode-merge-base = origin/main
[user]
	email = "a@b.c" # comment
"#;

    #[test]
    fn reads_sections_subsections_and_values() {
        let entries = parse(CONFIG);
        let keys: Vec<String> = entries.iter().map(Entry::key).collect();
        assert_eq!(
            keys,
            [
                "core.repositoryformatversion",
                "core.filemode",
                "core.bare",
                "remote.origin.url",
                "remote.origin.fetch",
                "branch.main.remote",
                "branch.main.merge",
                "branch.main.vscode-merge-base",
                "user.email",
            ]
        );
        assert_eq!(entries[6].value, "refs/heads/main");
        assert_eq!(entries[8].value, "a@b.c");
    }

    #[test]
    fn urls_that_run_programs_or_read_files_are_unsafe() {
        for url in [
            "ext::sh -c x",
            "fd::3",
            "file:///etc",
            "/Users/a/repo",
            "../repo",
            "~/repo",
            "-u x",
            "C:/repo",
        ] {
            assert!(unsafe_url(url), "{url}");
        }
        for url in [
            "git@github.com:a/b.git",
            "https://github.com/a/b",
            "ssh://git@host:22/a/b",
            "git://host/a",
        ] {
            assert!(!unsafe_url(url), "{url}");
        }
    }

    #[test]
    fn addresses_are_compared_without_scheme_user_or_suffix() {
        for url in [
            "git@github.com:Seba/App.git",
            "https://github.com/Seba/App",
            "https://user@github.com/Seba/App.git/",
            "ssh://git@github.com:22/Seba/App.git",
        ] {
            assert_eq!(normalize_url(url), "github.com/Seba/App", "{url}");
        }
    }

    #[test]
    fn finds_the_repository_its_branch_and_settings() {
        let dir = tempfile::tempdir().expect("temp dir");
        let top = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir_all(top.join(".git")).expect("mkdir");
        std::fs::create_dir_all(top.join("src/deep")).expect("mkdir");
        std::fs::write(top.join(".git/config"), CONFIG).expect("write");
        std::fs::write(top.join(".git/HEAD"), "ref: refs/heads/0.5.5\n").expect("write");
        let repo = Repo::find(&top.join("src/deep")).expect("a repository");
        assert_eq!(repo.top, top);
        assert_eq!(repo.branch().as_deref(), Some("0.5.5"));
        assert_eq!(
            repo.get("remote", Some("origin"), "url").as_deref(),
            Some("git@github.com:seba2390/pistachio.git")
        );
    }
}
