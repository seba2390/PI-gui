//! A repository's git settings, as far as the gate needs them: read without running git, and
//! checked against what may hold when git runs outside the sandbox.
//!
//! The agent can write the repository's `.git` folder, so its `config` may name programs that
//! git would start (`core.sshCommand`, `core.fsmonitor`, `credential.helper`, filter and merge
//! drivers, `url.*.insteadOf`, remote helpers, …). Outside the sandbox those would run with the
//! user's keys. So the gate runs git only in repositories whose settings are all on a list of
//! ones that start nothing ([`unsafe_entries`]); anything else is refused and named.

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

/// Whether one setting may hold while git runs outside the sandbox: it starts no program and
/// reads no file on this computer.
fn allowed(entry: &Entry) -> bool {
    let name = entry.name.as_str();
    let value = entry.value.trim().to_lowercase();
    match (entry.section.as_str(), entry.subsection.is_some()) {
        ("core", false) => matches!(
            name,
            "repositoryformatversion"
                | "filemode"
                | "bare"
                | "logallrefupdates"
                | "ignorecase"
                | "precomposeunicode"
                | "symlinks"
                | "autocrlf"
                | "eol"
                | "safecrlf"
                | "quotepath"
                | "longpaths"
                | "checkstat"
                | "trustctime"
                | "untrackedcache"
                | "splitindex"
                | "commitgraph"
                | "multipackindex"
                | "preloadindex"
                | "sparsecheckout"
                | "sparsecheckoutcone"
                | "abbrev"
                | "compression"
                | "loosecompression"
                | "bigfilethreshold"
                | "fsyncmethod"
                | "warnambiguousrefs"
                | "whitespace"
        ),
        ("remote", true) => match name {
            "url" | "pushurl" => !unsafe_url(&entry.value),
            "fetch" | "push" | "tagopt" | "prune" | "prunetags" | "promisor"
            | "partialclonefilter" | "skipdefaultupdate" | "skipfetchall" | "mirror"
            | "followremotehead" => true,
            _ => false,
        },
        // Which remote and branch a branch follows, and editors' notes about it.
        ("branch", true) => true,
        ("branch", false) => matches!(name, "autosetupmerge" | "autosetuprebase" | "sort"),
        ("user", false) => matches!(name, "name" | "email" | "signingkey" | "useconfigonly"),
        ("author" | "committer", false) => matches!(name, "name" | "email"),
        ("pull", false) => matches!(name, "rebase" | "ff"),
        ("push", false) => matches!(
            name,
            "default"
                | "autosetupremote"
                | "followtags"
                | "recursesubmodules"
                | "gpgsign"
                | "usebitmaps"
                | "negotiate"
        ),
        ("fetch", false) => matches!(
            name,
            "prune"
                | "prunetags"
                | "recursesubmodules"
                | "writecommitgraph"
                | "parallel"
                | "showforcedupdates"
                | "negotiationalgorithm"
                | "output"
                | "all"
        ),
        ("init", false) => name == "defaultbranch",
        ("submodule", true) => match name {
            "url" => !unsafe_url(&entry.value),
            // `!command` would run a program.
            "update" => matches!(value.as_str(), "checkout" | "rebase" | "merge" | "none"),
            "active" | "branch" | "fetchrecursesubmodules" | "ignore" | "shallow" => true,
            _ => false,
        },
        ("submodule", false) => matches!(name, "recurse" | "active" | "propagatebranches"),
        // git-lfs: where its files are (checked against the remote, see the plan).
        ("lfs", _) => true,
        ("extensions", false) => matches!(
            name,
            "objectformat"
                | "worktreeconfig"
                | "partialclone"
                | "refstorage"
                | "preciousobjects"
                | "noop"
        ),
        (
            "gc" | "maintenance" | "color" | "column" | "advice" | "status" | "log" | "rerere"
            | "index" | "pack" | "feature" | "receive" | "blame" | "grep" | "am" | "apply"
            | "checkout" | "clone" | "transfer",
            _,
        ) => !matches!(name, "hook" | "command" | "program"),
        ("commit" | "tag", false) => matches!(name, "gpgsign" | "verbose" | "cleanup" | "sort"),
        // The program is the user's own setting, never the repository's.
        ("gpg", false) => name == "format",
        ("merge", false) => matches!(
            name,
            "ff" | "conflictstyle"
                | "log"
                | "renames"
                | "renamelimit"
                | "defaulttoupstream"
                | "autostash"
                | "verifysignatures"
                | "stat"
                | "branchdesc"
                | "directoryrenames"
        ),
        ("rebase", false) => matches!(
            name,
            "autosquash"
                | "autostash"
                | "updaterefs"
                | "abbreviatecommands"
                | "missingcommitscheck"
                | "stat"
                | "instructionformat"
                | "forkpoint"
                | "rescheduledfailedexec"
                | "rebasemerges"
        ),
        ("diff", false) => matches!(
            name,
            "algorithm"
                | "renames"
                | "renamelimit"
                | "mnemonicprefix"
                | "noprefix"
                | "colormoved"
                | "colormovedws"
                | "indentheuristic"
                | "statgraphwidth"
                | "submodule"
                | "ignoresubmodules"
                | "context"
                | "interhunkcontext"
                | "relative"
                | "wserrorhighlight"
                | "dirstat"
                | "suppressblankempty"
        ),
        ("http", false) => matches!(
            name,
            "postbuffer" | "version" | "lowspeedlimit" | "lowspeedtime" | "maxrequests"
        ),
        // An alias that starts a shell; the gate runs no aliases, but a reader might.
        ("alias", false) => !value.starts_with('!'),
        _ => false,
    }
}

/// The settings among `entries` that git must not obey outside the sandbox, by key.
pub fn unsafe_entries(entries: &[Entry]) -> Vec<String> {
    let mut keys: Vec<String> = entries
        .iter()
        .filter(|entry| !allowed(entry))
        .map(Entry::key)
        .collect();
    keys.dedup();
    keys
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

    /// The config files git reads for this repository (not the user's own), and those of its
    /// submodules' repositories.
    pub fn config_files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.common_dir.join("config")];
        if self.git_dir != self.common_dir {
            files.push(self.git_dir.join("config.worktree"));
        }
        files.push(self.common_dir.join("config.worktree"));
        collect_module_configs(&self.common_dir.join("modules"), 0, &mut files);
        files.retain(|file| file.is_file());
        files.dedup();
        files
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

    /// The settings anywhere in the repository that git must not obey outside the sandbox,
    /// with the file they are in.
    pub fn unsafe_settings(&self) -> Vec<String> {
        let mut found = Vec::new();
        for file in self.config_files() {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for key in unsafe_entries(&parse(&text)) {
                found.push(key);
            }
        }
        found.dedup();
        found
    }

    /// The checked-out branch, unless `HEAD` is detached.
    pub fn branch(&self) -> Option<String> {
        let head = std::fs::read_to_string(self.git_dir.join("HEAD")).ok()?;
        head.trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_owned)
    }
}

fn collect_module_configs(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("config").is_file() {
            files.push(path.join("config"));
        }
        collect_module_configs(&path, depth + 1, files);
        collect_module_configs(&path.join("modules"), depth + 1, files);
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
        assert_eq!(unsafe_entries(&entries), Vec::<String>::new());
    }

    #[test]
    fn settings_that_start_programs_are_unsafe() {
        let text = r#"
[core]
	sshCommand = /tmp/evil
	fsmonitor = /tmp/evil
	hooksPath = /tmp/hooks
	pager = evil
[credential]
	helper = !evil
[url "https://evil.example/"]
	insteadOf = https://github.com/
[filter "lfs"]
	smudge = evil %f
[merge "ours"]
	driver = evil
[include]
	path = /tmp/more
[remote "x"]
	url = ext::sh -c evil
	receivepack = evil
[submodule "s"]
	update = !evil
	url = /Users/a/private
[gpg]
	program = /tmp/evil
[http]
	cookiefile = /Users/a/.ssh/id_rsa
[alias]
	x = !evil
	st = status
"#;
        assert_eq!(
            unsafe_entries(&parse(text)),
            [
                "core.sshcommand",
                "core.fsmonitor",
                "core.hookspath",
                "core.pager",
                "credential.helper",
                "url.https://evil.example/.insteadof",
                "filter.lfs.smudge",
                "merge.ours.driver",
                "include.path",
                "remote.x.url",
                "remote.x.receivepack",
                "submodule.s.update",
                "submodule.s.url",
                "gpg.program",
                "http.cookiefile",
                "alias.x",
            ]
        );
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
        std::fs::create_dir_all(top.join(".git/modules/lib")).expect("mkdir");
        std::fs::create_dir_all(top.join("src/deep")).expect("mkdir");
        std::fs::write(top.join(".git/config"), CONFIG).expect("write");
        std::fs::write(top.join(".git/HEAD"), "ref: refs/heads/0.5.5\n").expect("write");
        std::fs::write(
            top.join(".git/modules/lib/config"),
            "[core]\n\tfsmonitor = evil\n",
        )
        .expect("write");
        let repo = Repo::find(&top.join("src/deep")).expect("a repository");
        assert_eq!(repo.top, top);
        assert_eq!(repo.branch().as_deref(), Some("0.5.5"));
        assert_eq!(
            repo.get("remote", Some("origin"), "url").as_deref(),
            Some("git@github.com:seba2390/pistachio.git")
        );
        assert_eq!(repo.unsafe_settings(), ["core.fsmonitor"]);
    }
}
