//! What an agent's bash command does, in words: "Read Cargo.toml", "Searched “fn main” in
//! src", "Ran cargo test", worked out by the harness, never by the model.
//!
//! [`summarize`] parses the command with [`crate::shell`] and classifies every simple command
//! in it. It is deliberately conservative: an unknown program, flags that could change
//! files (`find -delete`, `sed -i`, `rg --pre`), `xargs`, or anything the tokenizer refuses
//! make it return `None`, and the raw command is shown instead. A label never hides a
//! command it did not understand. [`Summary::is_exploration`] (only reading, listing,
//! searching and checking) decides which calls may be folded into an "Explored" group. The
//! result is for display only.

use crate::shell::{self, Redirect, Simple};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Read(Vec<String>),
    /// Folders listed (empty: the current one).
    List(Vec<String>),
    /// `find` / `fd` by name.
    Find {
        name: String,
        paths: Vec<String>,
    },
    Search {
        pattern: String,
        paths: Vec<String>,
    },
    /// Tools, versions or system facts looked up.
    Check(Vec<String>),
    /// A read-only git command (`git status`, `git diff`, …), by subcommand.
    GitRead(String),
    /// A git command that changes things.
    Git(String),
    /// A build, test, script or other program: `cargo test`, `./run.sh`.
    Run(String),
    /// Packages installed or added (`verb` is "Added" or "Installed").
    Install {
        verb: &'static str,
        packages: Vec<String>,
    },
    CreateProject(Option<String>),
    InitRepo,
    MakeDir(Vec<String>),
    Create(Vec<String>),
    Write(Vec<String>),
    Edit(Vec<String>),
    Remove(Vec<String>),
    Move(String, String),
    Copy(String, String),
    Link(String, String),
    Chmod(Vec<String>),
    Fetch(String),
    Wait(String),
    Stop,
    Open(String),
    /// Inline code run by an interpreter (`python3 -c`, `node -e`, a heredoc).
    Snippet(&'static str),
    /// `diff a b`, `cmp a b`.
    Compare(Vec<String>),
    /// `source .venv/bin/activate`.
    Load(String),
    /// `ssh host [command]`: the command's own summary, on that host (none: a login).
    Remote {
        host: String,
        inner: Option<Box<Summary>>,
    },
}

impl Action {
    /// Only looks: reads, lists, searches, checks.
    pub fn is_exploration(&self) -> bool {
        matches!(
            self,
            Self::Read(_)
                | Self::List(_)
                | Self::Find { .. }
                | Self::Search { .. }
                | Self::Check(_)
                | Self::GitRead(_)
                | Self::Compare(_)
        ) || matches!(self, Self::Remote { inner: Some(inner), .. } if inner.is_exploration())
    }

    /// The past-tense verb and its object: ("Read", "Cargo.toml, src/main.rs").
    pub fn phrase(&self) -> (String, String) {
        let (verb, object): (&str, String) = match self {
            Self::Read(files) => ("Read", list(files)),
            Self::List(paths) if paths.is_empty() => ("Listed", "files".into()),
            Self::List(paths) => ("Listed", list(paths)),
            Self::Find { name, paths } if paths.is_empty() => ("Looked for", quote(name)),
            Self::Find { name, paths } => {
                ("Looked for", format!("{} in {}", quote(name), list(paths)))
            }
            Self::Search { pattern, paths } if paths.is_empty() => ("Searched for", quote(pattern)),
            Self::Search { pattern, paths } => {
                ("Searched", format!("{} in {}", quote(pattern), list(paths)))
            }
            Self::Check(things) => ("Checked", list(things)),
            Self::GitRead(sub) => match sub.as_str() {
                "diff" | "log" | "show" | "blame" | "reflog" | "shortlog" => {
                    ("Viewed", format!("git {sub}"))
                }
                _ => ("Checked", format!("git {sub}")),
            },
            Self::Git(sub) => ("Ran", format!("git {sub}")),
            Self::Run(command) => ("Ran", command.clone()),
            Self::Install { verb, packages } if packages.is_empty() => {
                (*verb, "dependencies".into())
            }
            Self::Install { verb, packages } => (*verb, list(packages)),
            Self::CreateProject(Some(name)) => ("Created", format!("project {name}")),
            Self::CreateProject(None) => ("Created", "a project".into()),
            Self::InitRepo => ("Initialized", "a git repository".into()),
            Self::MakeDir(folders) => (
                "Created",
                list(
                    &folders
                        .iter()
                        .map(|folder| {
                            if folder.ends_with('/') {
                                folder.clone()
                            } else {
                                format!("{folder}/")
                            }
                        })
                        .collect::<Vec<_>>(),
                ),
            ),
            Self::Create(files) => ("Created", list(files)),
            Self::Write(files) => ("Wrote", list(files)),
            Self::Edit(files) => ("Edited", list(files)),
            Self::Remove(paths) => ("Removed", list(paths)),
            Self::Move(from, to) => ("Moved", format!("{from} to {to}")),
            Self::Copy(from, to) => ("Copied", format!("{from} to {to}")),
            Self::Link(from, to) => ("Linked", format!("{to} to {from}")),
            Self::Chmod(files) => ("Changed permissions of", list(files)),
            Self::Fetch(what) => ("Fetched", what.clone()),
            Self::Wait(time) => ("Waited", time.clone()),
            Self::Stop => ("Stopped", "a process".into()),
            Self::Open(what) => ("Opened", what.clone()),
            Self::Snippet(language) => ("Ran", format!("a {language} snippet")),
            Self::Compare(files) => ("Compared", files.join(" and ")),
            Self::Load(file) => ("Loaded", file.clone()),
            Self::Remote {
                host,
                inner: Some(inner),
            } => {
                return (inner.title(), format!("{} on {host}", inner.detail()));
            }
            Self::Remote { host, inner: None } => ("Connected to", host.clone()),
        };
        (verb.to_owned(), object)
    }
}

/// What a whole command line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub actions: Vec<Action>,
}

impl Summary {
    pub fn is_exploration(&self) -> bool {
        self.actions.iter().all(Action::is_exploration)
    }

    /// The first verb, shown bold: "Listed".
    pub fn title(&self) -> String {
        self.actions
            .first()
            .map(|action| action.phrase().0)
            .unwrap_or_default()
    }

    /// The rest of the sentence: "files, checked cargo, rustc".
    pub fn detail(&self) -> String {
        // Up to four steps; longer chains show three and "and N more steps".
        let mut parts = Vec::new();
        let shown = if self.actions.len() > 4 {
            3
        } else {
            self.actions.len()
        };
        for (index, action) in self.actions.iter().take(shown).enumerate() {
            let (verb, object) = action.phrase();
            if index == 0 {
                parts.push(object);
            } else {
                parts.push(format!("{} {object}", lowercase_first(&verb)));
            }
        }
        if self.actions.len() > shown {
            parts.push(format!("and {} more steps", self.actions.len() - shown));
        }
        parts.join(", ")
    }

    /// The whole sentence: "Listed files, checked cargo, rustc".
    pub fn sentence(&self) -> String {
        format!("{} {}", self.title(), self.detail())
    }
}

/// Labels `command`, or `None` when any part of it is not understood.
pub fn summarize(command: &str) -> Option<Summary> {
    summarize_at(command, 0)
}

fn summarize_at(command: &str, depth: usize) -> Option<Summary> {
    if depth > 2 {
        return None;
    }
    let script = shell::parse(command)?;
    let mut actions = Vec::new();
    // `D=some/dir; cat $D/a.rs` reads "Read some/dir/a.rs".
    let mut variables: Vec<(String, String)> = Vec::new();
    for pipeline in &script {
        if let [simple] = pipeline.as_slice() {
            remember_assignments(simple, &mut variables);
        }
        for (index, simple) in pipeline.iter().enumerate() {
            let simple = substitute(simple, &variables);
            // Later stages that only reshape the output (`| head`, `| grep x`) add nothing.
            if index > 0 && is_filter(&simple) {
                continue;
            }
            actions.extend(classify(&simple, depth)?);
        }
    }
    let actions = merge(actions);
    (!actions.is_empty()).then_some(Summary { actions })
}

/// Why [`summarize`] gives up on `command`: the first simple command it does not
/// understand, or "syntax" when the tokenizer refuses it. `None` when it is labelled.
pub fn unknown_part(command: &str) -> Option<String> {
    let Some(script) = shell::parse(command) else {
        return Some("syntax".into());
    };
    for pipeline in &script {
        for (index, simple) in pipeline.iter().enumerate() {
            if !(index > 0 && is_filter(simple)) && classify(simple, 0).is_none() {
                return Some(simple.words.join(" "));
            }
        }
    }
    summarize(command)
        .is_none()
        .then(|| "nothing to say".into())
}

/// Records `NAME=value` (and `export NAME=value`) statements for [`substitute`].
fn remember_assignments(simple: &Simple, variables: &mut Vec<(String, String)>) {
    let words = match simple.words.first().map(String::as_str) {
        Some("export") => &simple.words[1..],
        _ => &simple.words[..],
    };
    if words.is_empty() || !words.iter().all(|word| is_assignment(word)) {
        return;
    }
    for word in words {
        if let Some((name, value)) = word.split_once('=') {
            let value = substitute_word(value, variables);
            variables.retain(|(known, _)| known != name);
            variables.push((name.to_owned(), value));
        }
    }
}

fn substitute(simple: &Simple, variables: &[(String, String)]) -> Simple {
    if variables.is_empty() {
        return simple.clone();
    }
    Simple {
        words: simple
            .words
            .iter()
            .map(|word| substitute_word(word, variables))
            .collect(),
        redirects: simple
            .redirects
            .iter()
            .map(|redirect| match redirect {
                Redirect::Write { target } => Redirect::Write {
                    target: substitute_word(target, variables),
                },
                Redirect::Read { target } => Redirect::Read {
                    target: substitute_word(target, variables),
                },
                other => other.clone(),
            })
            .collect(),
    }
}

/// Replaces `$NAME` and `${NAME}` of known variables; others stay as written.
fn substitute_word(word: &str, variables: &[(String, String)]) -> String {
    if !word.contains('$') {
        return word.to_owned();
    }
    let mut out = word.to_owned();
    // Longest names first, so `$DIR` is not replaced as `$D` + "IR".
    let mut sorted: Vec<&(String, String)> = variables.iter().collect();
    sorted.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    for (name, value) in sorted {
        out = out.replace(&format!("${{{name}}}"), value);
        let pattern = format!("${name}");
        let mut result = String::new();
        let mut rest = out.as_str();
        while let Some(index) = rest.find(&pattern) {
            let after = &rest[index + pattern.len()..];
            result.push_str(&rest[..index]);
            if after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                result.push_str(&pattern);
            } else {
                result.push_str(value);
            }
            rest = after;
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// What one simple command does: `None` when unknown, empty when it changes nothing worth
/// saying (`cd`, `echo`).
fn classify(simple: &Simple, depth: usize) -> Option<Vec<Action>> {
    let words = strip_prefixes(&simple.words)?;
    let mut actions = Vec::new();
    let writes: Vec<String> = simple
        .redirects
        .iter()
        .filter_map(|redirect| redirect.writes_file().map(str::to_owned))
        .collect();
    let input = simple
        .redirects
        .iter()
        .any(|redirect| matches!(redirect, Redirect::Input { .. }));
    if let Some((program, args)) = words.split_first() {
        actions.extend(command(program, args, input, depth)?);
    }
    if !writes.is_empty() {
        actions.push(Action::Write(writes));
    }
    Some(actions)
}

/// Drops `FOO=bar`, `sudo`, `time`, `nohup`, `env X=y`, `timeout 10`, `command`.
fn strip_prefixes(words: &[String]) -> Option<&[String]> {
    let mut words = words;
    loop {
        let Some(first) = words.first() else {
            return Some(words);
        };
        if is_assignment(first) {
            words = &words[1..];
            continue;
        }
        match first.as_str() {
            "sudo" => {
                words = &words[1..];
                while let Some(flag) = words.first().filter(|word| word.starts_with('-')) {
                    let valued = matches!(flag.as_str(), "-u" | "-g" | "-C" | "-h" | "-p");
                    words = &words[if valued { 2 } else { 1 }.min(words.len())..];
                }
            }
            "time" | "nohup" | "exec" | "nice" => words = &words[1..],
            "env" if words.len() > 1 && (is_assignment(&words[1]) || words[1] == "-i") => {
                words = &words[1..];
            }
            "timeout" if words.len() > 2 => words = &words[2..],
            "command" if words.get(1).is_some_and(|word| !word.starts_with('-')) => {
                words = &words[1..];
            }
            _ => return Some(words),
        }
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

fn command(program: &str, args: &[String], input: bool, depth: usize) -> Option<Vec<Action>> {
    let name = basename(program);
    // `cargo --version`, `rustc -vV`, `node -v`: looking up a tool.
    if let [flag] = args
        && (matches!(
            flag.as_str(),
            "--version" | "-V" | "-vV" | "version" | "--help" | "help"
        ) || (flag == "-v"
            && matches!(
                name,
                "node"
                    | "npm"
                    | "pnpm"
                    | "yarn"
                    | "bun"
                    | "deno"
                    | "ruby"
                    | "php"
                    | "go"
                    | "gcc"
                    | "clang"
                    | "nvm"
            )))
        && !matches!(name, "cat" | "ls" | "sort" | "head" | "tail")
    {
        return Some(vec![Action::Check(vec![name.to_owned()])]);
    }
    let action = match name {
        // Neutral: change nothing worth a word.
        "cd" | "pushd" | "popd" | "echo" | "printf" | "true" | "false" | ":" | "set" | "export"
        | "unset" | "clear" | "wait" | "exit" | "shopt" => return Some(Vec::new()),

        "cat" | "nl" | "bat" | "batcat" | "less" | "more" | "wc" | "file" | "stat" | "xxd"
        | "hexdump" | "od" | "strings" | "tac" | "md5sum" | "md5" | "shasum" | "sha256sum" => {
            let files = operands(args, &[]);
            if files.is_empty() {
                return Some(Vec::new());
            }
            Action::Read(files)
        }
        "head" | "tail" => {
            let files = operands(args, valued_flags(name));
            if files.is_empty() {
                return Some(Vec::new());
            }
            Action::Read(files)
        }
        "sed" => return sed(args),
        "awk" | "gawk" => return awk(args),
        "jq" | "yq" => {
            let files = operands(args, valued_flags(name));
            match files.get(1..) {
                Some(files) if !files.is_empty() => Action::Read(files.to_vec()),
                _ => return Some(Vec::new()),
            }
        }
        "sort" | "uniq" | "cut" | "tr" | "column" | "fmt" | "fold" | "rev" | "paste" | "expand"
        | "unexpand" | "base64" => {
            let files = operands(args, valued_flags(name));
            if has_flag(args, &["-o", "--output"]) || (name == "uniq" && files.len() > 1) {
                return None;
            }
            if files.is_empty() || name == "tr" {
                return Some(Vec::new());
            }
            Action::Read(files)
        }
        "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack" => return search(name, args),
        "ls" | "exa" | "eza" | "tree" | "du" | "lsd" => {
            if name == "tree" && has_flag(args, &["-o"]) {
                return None;
            }
            Action::List(
                operands(args, &["-L", "-I", "-P", "--level", "-d"])
                    .into_iter()
                    .filter(|a| a != "-d")
                    .collect(),
            )
        }
        "find" => return find(args),
        "fd" | "fdfind" => {
            if has_flag(args, &["-x", "--exec", "-X", "--exec-batch"]) {
                return None;
            }
            let found = operands(
                args,
                &[
                    "-e",
                    "--extension",
                    "-t",
                    "--type",
                    "-d",
                    "--max-depth",
                    "-E",
                    "--exclude",
                    "-S",
                    "--size",
                ],
            );
            match found.split_first() {
                Some((name, paths)) => Action::Find {
                    name: name.clone(),
                    paths: paths.to_vec(),
                },
                None => Action::List(Vec::new()),
            }
        }
        "which" | "whereis" | "type" => Action::Check(operands(args, &[])),
        "command"
            if args
                .first()
                .is_some_and(|flag| flag == "-v" || flag == "-V") =>
        {
            Action::Check(operands(args, &[]))
        }
        "pwd" => Action::Check(vec!["the current folder".into()]),
        "whoami" | "id" => Action::Check(vec!["the user".into()]),
        "uname" | "sw_vers" | "hostname" | "arch" | "nproc" | "lscpu" => {
            Action::Check(vec!["the system".into()])
        }
        "date" => Action::Check(vec!["the date".into()]),
        "env" | "printenv" => Action::Check(vec!["the environment".into()]),
        "df" => Action::Check(vec!["disk space".into()]),
        "free" | "vm_stat" => Action::Check(vec!["memory".into()]),
        "ps" | "pgrep" | "lsof" | "top" | "htop" => Action::Check(vec!["processes".into()]),
        "nvidia-smi" => Action::Check(vec!["the GPU".into()]),
        "realpath" | "readlink" | "dirname" | "basename" => return Some(Vec::new()),
        "uptime" => Action::Check(vec!["the system".into()]),
        "sysctl"
            if !args
                .iter()
                .any(|arg| arg.contains('=') && !arg.starts_with('-')) =>
        {
            Action::Check(vec!["the system".into()])
        }
        "journalctl"
            if !has_flag(
                args,
                &[
                    "--vacuum-size",
                    "--vacuum-time",
                    "--vacuum-files",
                    "--rotate",
                    "--flush",
                ],
            ) =>
        {
            Action::Check(vec!["the logs".into()])
        }
        "systemctl" => match operands(args, &["-n", "--lines", "-p", "--property", "-t", "--type"])
            .first()
            .map(String::as_str)
        {
            Some(
                "status" | "is-active" | "is-enabled" | "is-failed" | "list-units"
                | "list-unit-files" | "show" | "cat" | "list-timers",
            )
            | None => Action::Check(vec!["services".into()]),
            Some(verb) => Action::Run(format!("systemctl {verb}")),
        },
        "objdump" | "nm" | "otool" | "readelf" | "size" | "ldd" | "ffprobe" | "pdfinfo"
        | "identify" => {
            let files = operands(
                args,
                &[
                    "-d",
                    "--disassemble",
                    "-j",
                    "--section",
                    "--start-address",
                    "--stop-address",
                    "-v",
                    "-show_entries",
                    "-select_streams",
                    "-of",
                    "-print_format",
                ],
            );
            if files.is_empty() {
                return None;
            }
            Action::Read(files)
        }
        "diff" | "cmp" | "comm" => {
            let files = operands(
                args,
                &[
                    "-U",
                    "-C",
                    "--unified",
                    "--context",
                    "-x",
                    "--exclude",
                    "-X",
                    "-I",
                    "--label",
                    "-W",
                    "--width",
                    "--color",
                ],
            );
            if files.len() < 2 || has_flag(args, &["--to-file", "--from-file"]) {
                return None;
            }
            Action::Compare(files)
        }
        "source" | "." => Action::Load(args.first()?.clone()),
        "ssh" | "mosh" => return remote(args, depth),
        "scp" | "rsync" => {
            let valued: &[&str] = if name == "scp" {
                &["-P", "-i", "-o", "-F", "-J", "-l", "-S", "-c"]
            } else {
                &[
                    "-e",
                    "--rsh",
                    "--exclude",
                    "--include",
                    "--exclude-from",
                    "--include-from",
                    "--filter",
                    "-f",
                    "--chmod",
                    "--chown",
                    "--rsync-path",
                    "--files-from",
                    "--log-file",
                    "--partial-dir",
                    "--temp-dir",
                    "-T",
                    "--backup-dir",
                    "--suffix",
                    "--max-size",
                    "--min-size",
                    "--timeout",
                    "--port",
                ]
            };
            let paths = operands(args, valued);
            let [sources @ .., target] = paths.as_slice() else {
                return None;
            };
            if sources.is_empty() {
                return None;
            }
            Action::Copy(list(sources), target.clone())
        }
        "gh" => return github(args),

        "git" => return git(args),
        "cargo" => cargo(args),
        "npm" | "pnpm" | "yarn" | "bun" => node_packages(name, args),
        "npx" | "pnpx" | "bunx" | "uvx" => Action::Run(join_first(name, args, 1)),
        "pip" | "pip3" => pip(args),
        "uv" => uv(args),
        "python" | "python3" | "py" | "node" | "ruby" | "perl" | "php" | "deno" => {
            return interpreter(name, args, input);
        }
        "bash" | "sh" | "zsh" | "dash" => return shell_wrapper(args, input, depth),
        "go" => match args.first().map(String::as_str) {
            Some("get" | "install") => Action::Install {
                verb: "Installed",
                packages: operands(&args[1..], &[]),
            },
            Some("mod") if args.get(1).is_some_and(|sub| sub == "init") => {
                Action::CreateProject(args.get(2).cloned())
            }
            _ => Action::Run(join_first(name, args, 1)),
        },
        "brew" | "apt" | "apt-get" | "dnf" | "yum" | "pacman" | "apk" => {
            match args
                .iter()
                .find(|arg| !arg.starts_with('-'))
                .map(String::as_str)
            {
                Some("install" | "add") => Action::Install {
                    verb: "Installed",
                    packages: operands(&args[1..], &[]),
                },
                Some("list" | "info" | "search" | "show") => {
                    Action::Check(vec![format!("{name} packages")])
                }
                _ => Action::Run(join_first(name, args, 1)),
            }
        }
        "pytest" | "jest" | "vitest" | "mocha" | "tsc" | "eslint" | "prettier" | "ruff"
        | "black" | "mypy" | "pyright" | "rustfmt" | "make" | "just" | "cmake" | "ninja"
        | "meson" | "gradle" | "gradlew" | "mvn" | "swift" | "xcodebuild" | "docker" | "podman"
        | "docker-compose" | "kubectl" | "terraform" | "bazel" | "zig" | "gcc" | "g++"
        | "clang" | "clang++" | "rustc" | "rustup" | "javac" | "java" | "dotnet" | "flutter"
        | "dart" | "mix" | "ollama" | "pi" | "hyperfine" | "valgrind" | "ffmpeg" | "magick"
        | "sips" | "pdftoppm" | "pdftotext" | "blender" | "unzip" | "zip" | "tar" | "gzip"
        | "gunzip" | "screencapture" | "osascript" | "nc" | "uvicorn" | "gunicorn" | "flask"
        | "streamlit" | "jupyter" | "hugo" | "vite" | "next" | "wasm-pack" | "trunk"
        | "maturin" | "sqlite3" | "psql" | "redis-cli" | "codesign" | "lipo" | "xcrun"
        | "plutil" | "defaults" | "launchctl" | "mdfind" | "ditto" | "hdiutil" => {
            Action::Run(join_first(name, args, 1))
        }

        "mkdir" => Action::MakeDir(operands(args, &["-m", "--mode"])),
        "touch" => Action::Create(operands(args, &["-t", "-d", "-r"])),
        "rm" | "rmdir" | "unlink" | "trash" => Action::Remove(operands(args, &[])),
        "mv" | "cp" | "ln" => {
            let paths = operands(args, &["-t", "--target-directory", "-S", "--suffix"]);
            let [sources @ .., target] = paths.as_slice() else {
                return None;
            };
            if sources.is_empty() {
                return None;
            }
            let (from, to) = (list(sources), target.clone());
            match name {
                "mv" => Action::Move(from, to),
                "cp" => Action::Copy(from, to),
                _ => Action::Link(from, to),
            }
        }
        "chmod" => Action::Chmod(operands(args, &[]).into_iter().skip(1).collect()),
        "tee" => {
            let files: Vec<String> = operands(args, &[])
                .into_iter()
                .filter(|file| file != "/dev/null")
                .collect();
            if files.is_empty() {
                return Some(Vec::new());
            }
            Action::Write(files)
        }
        "curl" | "wget" | "http" | "https" | "xh" => {
            let url = operands(
                args,
                &[
                    "-o",
                    "--output",
                    "-X",
                    "--request",
                    "-H",
                    "--header",
                    "-d",
                    "--data",
                    "--data-raw",
                    "--data-binary",
                    "--data-urlencode",
                    "-F",
                    "--form",
                    "-u",
                    "--user",
                    "-A",
                    "--user-agent",
                    "-e",
                    "--referer",
                    "-m",
                    "--max-time",
                    "--connect-timeout",
                    "-w",
                    "--write-out",
                    "-b",
                    "--cookie",
                    "-c",
                    "--cookie-jar",
                    "-T",
                    "--upload-file",
                    "-x",
                    "--proxy",
                    "--retry",
                    "-K",
                    "--config",
                    "-r",
                    "--range",
                ],
            )
            .into_iter()
            .find(|arg| {
                arg.contains("://")
                    || arg.starts_with("localhost")
                    || arg.contains('.')
                    || arg
                        .split_once(':')
                        .is_some_and(|(_, port)| port.starts_with(|c: char| c.is_ascii_digit()))
            })?;
            let host = url
                .split("://")
                .nth(1)
                .unwrap_or(&url)
                .split(['/', '?'])
                .next()
                .unwrap_or_default()
                .to_owned();
            let mut actions = vec![Action::Fetch(host)];
            if let Some(file) = flag_value(args, &["-o", "--output"])
                .filter(|file| *file != "/dev/null" && *file != "-")
            {
                actions.push(Action::Write(vec![file.to_owned()]));
            }
            return Some(actions);
        }
        "sleep" => Action::Wait(args.first().map_or_else(String::new, |time| {
            if time.chars().all(|c| c.is_ascii_digit() || c == '.') {
                format!("{time}s")
            } else {
                time.clone()
            }
        })),
        "kill" | "pkill" | "killall" => Action::Stop,
        "open" | "xdg-open" => Action::Open(operands(args, &["-a"]).first()?.clone()),
        _ if program.contains('/') => Action::Run(program.to_owned()),
        _ => return None,
    };
    Some(vec![action])
}

/// A later pipeline stage that only reshapes the output before it.
fn is_filter(simple: &Simple) -> bool {
    if simple
        .redirects
        .iter()
        .any(|redirect| redirect.writes_file().is_some())
    {
        return false;
    }
    let Some((program, args)) = simple.words.split_first() else {
        return false;
    };
    let name = basename(program);
    match name {
        "grep" | "egrep" | "fgrep" | "rg" => {
            !has_flag(args, &["--pre", "--pre-glob"])
                && search_parts(name, args).is_some_and(|(_, paths)| paths.is_empty())
        }
        "head" | "tail" | "wc" | "sort" | "uniq" | "cut" | "tr" | "column" | "fmt" | "fold"
        | "rev" | "cat" | "nl" | "less" | "more" | "tac" | "jq" | "yq" | "base64" | "paste" => {
            // The program (jq) or character sets (tr) are not files.
            let allowed = match name {
                "jq" | "yq" => 1,
                "tr" => 2,
                _ => 0,
            };
            !has_flag(args, &["-o", "--output"])
                && operands(args, valued_flags(name)).len() <= allowed
        }
        "sed" => sed(args).is_some_and(|actions| actions.is_empty()),
        "awk" | "gawk" => awk(args).is_some_and(|actions| actions.is_empty()),
        _ => false,
    }
}

/// `sed`: printing (`-n '1,40p'`) and plain substitutions read; `-i` edits; scripts that
/// could write or run things (`w`, `e`) are not understood.
fn sed(args: &[String]) -> Option<Vec<Action>> {
    let in_place = args.iter().any(|arg| {
        arg == "--in-place"
            || (arg.starts_with("-i") && !arg.starts_with("--"))
            || arg.starts_with("--in-place=")
    });
    // BSD `sed -i '' …`: the empty backup suffix is not the script.
    let args: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(index, arg)| !(arg.is_empty() && *index > 0 && args[index - 1] == "-i"))
        .map(|(_, arg)| arg.clone())
        .collect();
    let args = args.as_slice();
    let mut script = flag_value(args, &["-e", "--expression"]).map(str::to_owned);
    let mut files = operands(args, &["-e", "--expression", "-f"]);
    if has_flag(args, &["-f", "--file"]) {
        return None;
    }
    if script.is_none() {
        if files.is_empty() {
            return None;
        }
        script = Some(files.remove(0));
    }
    let script = script.unwrap_or_default();
    let addresses = without_regex_addresses(&script);
    let printing = !script.is_empty()
        && addresses.chars().all(|c| {
            c.is_ascii_digit()
                || matches!(
                    c,
                    ',' | '$' | 'p' | ';' | ' ' | 'q' | 'd' | '!' | '{' | '}' | '=' | '\n'
                )
        });
    let substitution = script
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(is_plain_substitution);
    if !printing && !substitution {
        return None;
    }
    if in_place {
        return (!files.is_empty()).then(|| vec![Action::Edit(files)]);
    }
    Some(if files.is_empty() {
        Vec::new()
    } else {
        vec![Action::Read(files)]
    })
}

/// `script` with its `/regex/` addresses removed (so `/^fn/,/^}/p` checks as `,p`).
fn without_regex_addresses(script: &str) -> String {
    let mut out = String::new();
    let mut chars = script.chars();
    while let Some(c) = chars.next() {
        if c == '/' {
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '/' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `s/a/b/g` (any delimiter) without the `w` or `e` flags.
fn is_plain_substitution(script: &str) -> bool {
    let mut chars = script.chars();
    if chars.next() != Some('s') {
        return false;
    }
    let Some(delimiter) = chars.next() else {
        return false;
    };
    if delimiter.is_alphanumeric() || delimiter == '\\' || delimiter == '\n' {
        return false;
    }
    let rest: String = chars.collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in rest.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            current.push(c);
            escaped = true;
        } else if c == delimiter {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    parts.push(current);
    parts.len() == 3
        && parts[2]
            .chars()
            .all(|c| matches!(c, 'g' | 'p' | 'i' | 'I' | 'm' | 'M') || c.is_ascii_digit())
}

/// `awk` programs that only print (no output redirection, pipes or `system()`).
fn awk(args: &[String]) -> Option<Vec<Action>> {
    if has_flag(args, &["-f", "--file"]) {
        return None;
    }
    let mut operands = operands(args, &["-F", "-v", "--field-separator", "--assign"]);
    if operands.is_empty() {
        return None;
    }
    let program = operands.remove(0);
    if !awk_only_prints(&program) {
        return None;
    }
    Some(if operands.is_empty() {
        Vec::new()
    } else {
        vec![Action::Read(operands)]
    })
}

/// Flags of `name` that take the next argument as their value.
fn valued_flags(name: &str) -> &'static [&'static str] {
    match name {
        "head" | "tail" => &["-n", "-c", "--lines", "--bytes"],
        "sort" => &[
            "-k",
            "--key",
            "-t",
            "--field-separator",
            "-S",
            "--buffer-size",
            "-T",
            "--parallel",
        ],
        "cut" => &[
            "-d",
            "--delimiter",
            "-f",
            "--fields",
            "-c",
            "--characters",
            "-b",
            "--bytes",
        ],
        "uniq" => &[
            "-f",
            "--skip-fields",
            "-s",
            "--skip-chars",
            "-w",
            "--check-chars",
        ],
        "fold" | "fmt" => &["-w", "--width"],
        "paste" => &["-d", "--delimiters"],
        "column" => &["-s", "-c", "-o"],
        "jq" | "yq" => &["--arg", "--argjson", "--slurpfile", "--rawfile", "--indent"],
        "grep" | "egrep" | "fgrep" => &[
            "-e",
            "--regexp",
            "-f",
            "--file",
            "-A",
            "-B",
            "-C",
            "--context",
            "--after-context",
            "--before-context",
            "-m",
            "--max-count",
            "-d",
            "--directories",
            "-D",
            "--devices",
            "--include",
            "--exclude",
            "--exclude-dir",
            "--color",
            "--colour",
            "--label",
        ],
        "rg" => &[
            "-e",
            "--regexp",
            "-f",
            "--file",
            "-A",
            "-B",
            "-C",
            "--context",
            "--after-context",
            "--before-context",
            "-m",
            "--max-count",
            "-g",
            "--glob",
            "--iglob",
            "-t",
            "--type",
            "-T",
            "--type-not",
            "-M",
            "--max-columns",
            "-j",
            "--threads",
            "--max-depth",
            "-d",
            "--sort",
            "--sortr",
            "-r",
            "--replace",
            "-E",
            "--encoding",
            "--max-filesize",
            "--ignore-file",
            "--type-add",
            "--color",
            "--colors",
            "--context-separator",
            "--path-separator",
        ],
        _ => &[],
    }
}

/// No `system()`, `getline`, and no `print … > file` or `print … | "cmd"`; comparisons such as
/// `$2 > 5` are fine.
fn awk_only_prints(program: &str) -> bool {
    if ["system", "getline", "close(", "fflush", "| \"", "|\""]
        .iter()
        .any(|danger| program.contains(danger))
    {
        return false;
    }
    let mut rest = program;
    while let Some(index) = rest.find("print") {
        let statement = &rest[index..];
        let end = statement.find([';', '}', '\n']).unwrap_or(statement.len());
        let statement = &statement[..end];
        if statement.contains('>') || statement.contains('|') {
            return false;
        }
        rest = &rest[index + 5..];
    }
    true
}

/// `gh`: reading issues, pull requests, runs and the API looks; anything else changes things.
fn github(args: &[String]) -> Option<Vec<Action>> {
    let words = operands(
        args,
        &[
            "-R",
            "--repo",
            "-L",
            "--limit",
            "--json",
            "-q",
            "--jq",
            "-t",
            "--template",
            "-s",
            "--state",
            "-l",
            "--label",
            "-a",
            "--assignee",
            "-A",
            "--author",
            "--interval",
            "-H",
            "--header",
            "-X",
            "--method",
            "-f",
            "-F",
            "--field",
            "--raw-field",
            "--input",
            "-B",
            "--base",
            "--head",
            "--title",
            "-b",
            "--body",
            "--body-file",
            "-w",
            "--workflow",
            "-j",
            "--job",
            "-p",
            "--paginate",
        ],
    );
    let noun = words.first()?.as_str();
    let verb = words.get(1).map_or("", String::as_str);
    if noun == "api" {
        let writes = has_flag(args, &["-f", "-F", "--field", "--raw-field", "--input"])
            || flag_value(args, &["-X", "--method"])
                .is_some_and(|method| !method.eq_ignore_ascii_case("GET"));
        return Some(vec![if writes {
            Action::Run("gh api".into())
        } else {
            Action::Check(vec!["the GitHub API".into()])
        }]);
    }
    let reads = matches!(
        verb,
        "view" | "list" | "status" | "checks" | "diff" | "watch" | "search" | "ls" | "download"
    ) || matches!(noun, "status" | "search" | "browse")
        || (noun == "auth" && verb == "status");
    Some(vec![if reads {
        Action::Check(vec![format!("gh {noun} {verb}").trim_end().to_owned()])
    } else {
        Action::Run(format!("gh {noun} {verb}").trim_end().to_owned())
    }])
}

/// `ssh host 'command'`: the remote command's own summary, on that host.
fn remote(args: &[String], depth: usize) -> Option<Vec<Action>> {
    let words = operands(
        args,
        &[
            "-p", "-i", "-o", "-J", "-l", "-F", "-L", "-R", "-D", "-W", "-S", "-E", "-b", "-c",
            "-m", "-O", "-Q", "-w", "-B", "-I",
        ],
    );
    let (host, command) = words.split_first()?;
    let host = host.rsplit('@').next().unwrap_or(host).to_owned();
    if command.is_empty() {
        return Some(vec![Action::Remote { host, inner: None }]);
    }
    // A remote command that is not understood leaves the whole line raw.
    let inner = summarize_at(&command.join(" "), depth + 1)?;
    Some(vec![Action::Remote {
        host,
        inner: Some(Box::new(inner)),
    }])
}

/// The pattern and paths of a `grep`/`rg` command.
fn search_parts(name: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let mut operands = operands(args, valued_flags(name));
    let pattern = match flag_value(args, &["-e", "--regexp"]) {
        Some(pattern) => pattern.to_owned(),
        None => match flag_value(args, &["-f", "--file"]) {
            Some(file) => format!("patterns in {file}"),
            None => {
                if operands.is_empty() {
                    return None;
                }
                operands.remove(0)
            }
        },
    };
    Some((pattern, operands))
}

fn search(name: &str, args: &[String]) -> Option<Vec<Action>> {
    if has_flag(args, &["--pre", "--pre-glob"]) {
        return None;
    }
    if name == "rg" && has_flag(args, &["--files"]) {
        let paths = operands(args, &["-g", "--glob", "-t", "--type", "--max-depth"]);
        return Some(vec![Action::List(paths)]);
    }
    let (pattern, paths) = search_parts(name, args)?;
    Some(vec![Action::Search { pattern, paths }])
}

/// `find`: listing or looking up by name, unless it acts on what it finds.
fn find(args: &[String]) -> Option<Vec<Action>> {
    const ACTING: &[&str] = &[
        "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf", "-fls",
    ];
    if args.iter().any(|arg| ACTING.contains(&arg.as_str())) {
        return None;
    }
    let paths: Vec<String> = args
        .iter()
        .take_while(|arg| !arg.starts_with('-') && *arg != "(" && *arg != "!")
        .cloned()
        .collect();
    let name = flag_value(
        args,
        &["-name", "-iname", "-path", "-ipath", "-regex", "-iregex"],
    );
    Some(vec![match name {
        Some(name) => Action::Find {
            name: name.to_owned(),
            paths: paths.into_iter().filter(|path| path != ".").collect(),
        },
        None => Action::List(paths),
    }])
}

fn git(args: &[String]) -> Option<Vec<Action>> {
    // Global options before the subcommand.
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => index += 2,
            _ if arg.starts_with('-') => index += 1,
            _ => break,
        }
    }
    let sub = args.get(index)?.as_str();
    let rest = &args[index + 1..];
    let bare = operands(rest, &["--format", "--pretty", "-n", "--sort"]).is_empty();
    let read_only = match sub {
        "status" | "diff" | "log" | "show" | "blame" | "describe" | "shortlog" | "reflog"
        | "rev-parse" | "rev-list" | "ls-remote" | "ls-tree" | "cat-file" | "whatchanged"
        | "count-objects" | "check-ignore" | "name-rev" | "merge-base" | "for-each-ref"
        | "show-ref" | "help" | "version" => !has_flag(rest, &["--output"]),
        "branch" => {
            bare && !has_flag(
                rest,
                &[
                    "-d",
                    "-D",
                    "-m",
                    "-M",
                    "-c",
                    "-C",
                    "-f",
                    "--delete",
                    "--move",
                    "--copy",
                    "-u",
                    "--set-upstream-to",
                    "--unset-upstream",
                    "--edit-description",
                ],
            )
        }
        "tag" => bare || has_flag(rest, &["-l", "--list"]),
        "remote" => {
            bare || matches!(
                rest.iter()
                    .find(|arg| !arg.starts_with('-'))
                    .map(String::as_str),
                Some("show" | "get-url")
            )
        }
        "stash" => matches!(rest.first().map(String::as_str), Some("list" | "show")),
        "worktree" => matches!(rest.first().map(String::as_str), Some("list")),
        "config" => {
            has_flag(
                rest,
                &["--get", "--get-all", "--list", "-l", "--get-regexp"],
            ) || operands(rest, &[]).len() == 1
        }
        "ls-files" => return Some(vec![Action::List(vec!["tracked files".into()])]),
        "grep" => {
            let (pattern, paths) = search_parts("grep", rest)?;
            return Some(vec![Action::Search { pattern, paths }]);
        }
        "init" => return Some(vec![Action::InitRepo]),
        _ => false,
    };
    Some(vec![if read_only {
        Action::GitRead(sub.to_owned())
    } else {
        Action::Git(sub.to_owned())
    }])
}

fn cargo(args: &[String]) -> Action {
    let args: Vec<String> = args
        .iter()
        .filter(|arg| !arg.starts_with('+'))
        .cloned()
        .collect();
    let sub = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map_or("", String::as_str);
    let rest = args
        .iter()
        .position(|arg| arg == sub)
        .map_or(args.as_slice(), |index| &args[index + 1..]);
    match sub {
        "add" => Action::Install {
            verb: "Added",
            packages: operands(
                rest,
                &[
                    "-F",
                    "--features",
                    "--path",
                    "--git",
                    "--branch",
                    "--tag",
                    "--rev",
                    "--rename",
                    "-p",
                    "--package",
                    "--manifest-path",
                    "--registry",
                    "--target",
                ],
            ),
        },
        "install" => Action::Install {
            verb: "Installed",
            packages: operands(
                rest,
                &[
                    "--version",
                    "--git",
                    "--path",
                    "--root",
                    "-F",
                    "--features",
                    "--branch",
                    "--tag",
                    "--rev",
                ],
            ),
        },
        "init" | "new" => {
            let name = flag_value(rest, &["--name"])
                .map(str::to_owned)
                .or_else(|| {
                    operands(rest, &["--name", "--vcs", "--edition"])
                        .into_iter()
                        .find(|path| path != ".")
                        .map(|path| basename(&path).to_owned())
                });
            Action::CreateProject(name)
        }
        "tree" | "metadata" | "pkgid" | "search" | "locate-project" | "verify-project"
        | "read-manifest" | "outdated" => Action::Check(vec![format!("cargo {sub}")]),
        "" => Action::Run("cargo".into()),
        _ => Action::Run(format!("cargo {}", cargo_alias(sub))),
    }
}

fn cargo_alias(sub: &str) -> &str {
    match sub {
        "b" => "build",
        "c" => "check",
        "t" => "test",
        "r" => "run",
        "d" => "doc",
        other => other,
    }
}

fn node_packages(name: &str, args: &[String]) -> Action {
    let sub = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map_or("", String::as_str);
    let rest = args
        .iter()
        .position(|arg| arg == sub)
        .map_or(args, |index| &args[index + 1..]);
    match sub {
        "" if name == "yarn" => Action::Install {
            verb: "Installed",
            packages: Vec::new(),
        },
        "install" | "i" | "ci" | "add" => Action::Install {
            verb: if sub == "add" { "Added" } else { "Installed" },
            packages: operands(rest, &["--registry", "--filter", "-w"]),
        },
        "init" | "create" => Action::CreateProject(operands(rest, &[]).get(1).cloned()),
        "ls" | "list" | "outdated" | "view" | "info" | "why" | "audit" => {
            Action::Check(vec![format!("{name} packages")])
        }
        "run" | "exec" | "x" | "dlx" => Action::Run(
            format!(
                "{name} {sub} {}",
                rest.iter()
                    .find(|arg| !arg.starts_with('-'))
                    .map_or("", String::as_str)
            )
            .trim_end()
            .to_owned(),
        ),
        _ => Action::Run(format!("{name} {sub}").trim_end().to_owned()),
    }
}

fn pip(args: &[String]) -> Action {
    match args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
    {
        Some("install") => {
            let rest = &args[1..];
            Action::Install {
                verb: "Installed",
                packages: if has_flag(rest, &["-r", "--requirement"]) {
                    vec!["requirements".into()]
                } else {
                    operands(
                        rest,
                        &["-i", "--index-url", "-c", "--constraint", "--target", "-t"],
                    )
                },
            }
        }
        Some("list" | "show" | "freeze" | "check") => Action::Check(vec!["Python packages".into()]),
        _ => Action::Run(join_first("pip", args, 1)),
    }
}

fn uv(args: &[String]) -> Action {
    match args.first().map(String::as_str) {
        Some("add") => Action::Install {
            verb: "Added",
            packages: operands(&args[1..], &["--group", "--optional", "--extra"]),
        },
        Some("pip") if args.get(1).is_some_and(|sub| sub == "install") => pip(&args[1..]),
        Some("sync") => Action::Install {
            verb: "Installed",
            packages: Vec::new(),
        },
        Some("init") => Action::CreateProject(operands(&args[1..], &[]).first().cloned()),
        Some("run") => Action::Run(join_first("uv run", &args[1..], 1)),
        _ => Action::Run(join_first("uv", args, 1)),
    }
}

fn interpreter(name: &str, args: &[String], input: bool) -> Option<Vec<Action>> {
    let language = match name {
        "python" | "python3" | "py" => "Python",
        "node" | "deno" => "JavaScript",
        "ruby" => "Ruby",
        "perl" => "Perl",
        _ => "PHP",
    };
    let action = match args.first().map(String::as_str) {
        Some("-c" | "-e" | "-p" | "--eval" | "--print") => Action::Snippet(language),
        Some("-m") => match args.get(1).map(String::as_str) {
            Some("pip") => pip(&args[2..]),
            Some(module) => Action::Run(module.to_owned()),
            None => return None,
        },
        Some("-" | "-i") | None if input => Action::Snippet(language),
        None => return None,
        Some(_) => match operands(args, &["-W", "-X"]).first() {
            Some(script) => Action::Run(script.clone()),
            None if input => Action::Snippet(language),
            None => return None,
        },
    };
    Some(vec![action])
}

/// `bash -lc "…"` is summarized as its inner script; `bash script.sh` runs a script.
fn shell_wrapper(args: &[String], input: bool, depth: usize) -> Option<Vec<Action>> {
    let command_flag = args.iter().position(|arg| {
        arg.starts_with('-')
            && !arg.starts_with("--")
            && arg.contains('c')
            && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
    });
    if let Some(index) = command_flag {
        let script = args.get(index + 1)?;
        return summarize_at(script, depth + 1).map(|summary| summary.actions);
    }
    match operands(args, &[]).first() {
        Some(script) => Some(vec![Action::Run(script.clone())]),
        None if input => Some(vec![Action::Snippet("shell")]),
        None => None,
    }
}

/// Non-flag arguments, skipping the values of `valued` flags (`-n 20`) and everything up to
/// `--` counting as operands after it.
fn operands(args: &[String], valued: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    let mut flags_done = false;
    while let Some(arg) = args.get(index) {
        index += 1;
        if flags_done {
            out.push(arg.clone());
        } else if arg == "--" {
            flags_done = true;
        } else if arg.starts_with('-') && arg.len() > 1 {
            if valued.contains(&arg.as_str()) {
                index += 1;
            }
        } else if arg != "-" {
            out.push(arg.clone());
        }
    }
    out
}

fn has_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| {
        flags.iter().any(|flag| {
            arg == flag || (flag.starts_with("--") && arg.starts_with(&format!("{flag}=")))
        })
    })
}

fn flag_value<'a>(args: &'a [String], flags: &[&str]) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(index, arg)| {
        flags.iter().find_map(|flag| {
            if arg == flag {
                args.get(index + 1).map(String::as_str)
            } else {
                arg.strip_prefix(&format!("{flag}="))
            }
        })
    })
}

/// `name` and its first `count` non-flag arguments: "make test", "npx vite".
fn join_first(name: &str, args: &[String], count: usize) -> String {
    let mut out = name.to_owned();
    for arg in args.iter().filter(|arg| !arg.starts_with('-')).take(count) {
        if arg.len() > 40 || arg.contains(char::is_whitespace) {
            break;
        }
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// Adjacent actions of the same kind become one ("Read a, b"; "Checked cargo, rustc").
fn merge(actions: Vec<Action>) -> Vec<Action> {
    let mut out: Vec<Action> = Vec::new();
    for action in actions {
        match (out.last_mut(), action) {
            (Some(Action::Read(a)), Action::Read(b))
            | (Some(Action::Check(a)), Action::Check(b))
            | (Some(Action::Write(a)), Action::Write(b))
            | (Some(Action::MakeDir(a)), Action::MakeDir(b))
            | (Some(Action::Remove(a)), Action::Remove(b))
            | (Some(Action::Create(a)), Action::Create(b)) => {
                for item in b {
                    if !a.contains(&item) {
                        a.push(item);
                    }
                }
            }
            (Some(Action::List(a)), Action::List(b)) if !a.is_empty() && !b.is_empty() => {
                for item in b {
                    if !a.contains(&item) {
                        a.push(item);
                    }
                }
            }
            (_, action) => {
                if out.last() != Some(&action) {
                    out.push(action);
                }
            }
        }
    }
    out
}

/// "a, b, c and 2 more"
fn list(items: &[String]) -> String {
    const SHOWN: usize = 3;
    let items: Vec<String> = items.iter().map(|item| shorten(item)).collect();
    match items.len() {
        0 => String::new(),
        n if n <= SHOWN + 1 => items.join(", "),
        n => format!("{} and {} more", items[..SHOWN].join(", "), n - SHOWN),
    }
}

/// A long path as its last two parts: `…/src/audio.rs`.
fn shorten(path: &str) -> String {
    const LONG: usize = 48;
    if path.chars().count() <= LONG || !path.contains('/') {
        return path.to_owned();
    }
    let trailing = path.ends_with('/');
    let parts: Vec<&str> = path.trim_end_matches('/').rsplit('/').take(2).collect();
    let tail: Vec<&str> = parts.into_iter().rev().collect();
    format!("…/{}{}", tail.join("/"), if trailing { "/" } else { "" })
}

fn quote(text: &str) -> String {
    let short: String = text.chars().take(60).collect();
    if short.len() < text.len() {
        format!("“{short}…”")
    } else {
        format!("“{short}”")
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn sentence(command: &str) -> Option<String> {
        summarize(command).map(|summary| summary.sentence())
    }

    fn explores(command: &str) -> bool {
        summarize(command).is_some_and(|summary| summary.is_exploration())
    }

    #[test]
    fn labels_the_commands_agents_run() {
        let cases = [
            (
                "ls -la && which cargo rustc && cargo --version && rustc --version",
                "Listed files, checked cargo, rustc",
            ),
            (
                "cargo init --name snake --bin . 2>&1 | head -20",
                "Created project snake",
            ),
            (
                "cat Cargo.toml && cat src/main.rs",
                "Read Cargo.toml, src/main.rs",
            ),
            ("cargo add macroquad 2>&1 | tail -20", "Added macroquad"),
            (
                "ls ~/.cargo/registry/src/*/macroquad-0.4.16/src/ 2>/dev/null",
                "Listed ~/.cargo/registry/src/*/macroquad-0.4.16/src/",
            ),
            (
                r#"find / -name "macroquad-0.4.16" -type d 2>/dev/null | head; echo ---"#,
                "Looked for “macroquad-0.4.16” in /",
            ),
            ("rg -n 'fn main' src | head -5", "Searched “fn main” in src"),
            ("grep -rn TODO .", "Searched “TODO” in ."),
            ("sed -n '1,80p' src/lib.rs", "Read src/lib.rs"),
            (
                "cd crates/app && cargo test -p pistachio 2>&1 | tail -20",
                "Ran cargo test",
            ),
            ("cargo build --release", "Ran cargo build"),
            (
                "git status --short && git diff --stat",
                "Checked git status, viewed git diff",
            ),
            (
                "git add -A && git commit -m 'x'",
                "Ran git add, ran git commit",
            ),
            (
                "mkdir -p src/bin && touch src/bin/a.rs",
                "Created src/bin/, created src/bin/a.rs",
            ),
            (
                "cat > src/main.rs <<'EOF'\nfn main() {}\nEOF",
                "Wrote src/main.rs",
            ),
            ("rm -rf target/tmp", "Removed target/tmp"),
            ("python3 -c 'print(1)'", "Ran a Python snippet"),
            ("python3 - <<'EOF'\nprint(1)\nEOF", "Ran a Python snippet"),
            ("bash -lc 'cat README.md | head'", "Read README.md"),
            ("npm install", "Installed dependencies"),
            ("pip install -r requirements.txt", "Installed requirements"),
            ("./target/debug/snake", "Ran ./target/debug/snake"),
            (
                "curl -s https://api.github.com/repos/x/y",
                "Fetched api.github.com",
            ),
            ("sleep 2", "Waited 2s"),
            ("wc -l src/*.rs | sort -n | tail -5", "Read src/*.rs"),
            ("RUST_LOG=debug cargo run -- --help", "Ran cargo run"),
            ("ls > files.txt", "Listed files, wrote files.txt"),
            ("echo hi | tee notes.txt", "Wrote notes.txt"),
            ("sed -i 's/a/b/g' src/x.rs", "Edited src/x.rs"),
            ("sed -i '' 's/a/b/' src/x.rs", "Edited src/x.rs"),
            ("sed -n '/^fn main/,/^}/p' src/main.rs", "Read src/main.rs"),
            ("awk 'NR>1 && $2 > 5 {print $1}' data.txt", "Read data.txt"),
            (
                "M=crates/app; cat $M/src/main.rs ${M}/Cargo.toml",
                "Read crates/app/src/main.rs, crates/app/Cargo.toml",
            ),
            (
                "cat /Users/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/macroquad-0.4.16/src/audio.rs",
                "Read …/src/audio.rs",
            ),
            (
                "ssh -o BatchMode=yes box 'ls -la /srv && df -h'",
                "Listed /srv, checked disk space on box",
            ),
            ("gh pr checks 622 --repo me/repo", "Checked gh pr checks"),
            ("gh pr create --title x --body y", "Ran gh pr create"),
            (
                "gh api repos/me/repo/issues/1 --jq .body",
                "Checked the GitHub API",
            ),
            ("scp -q a.py box:work/a.py", "Copied a.py to box:work/a.py"),
            ("diff -u old.txt new.txt", "Compared old.txt and new.txt"),
            (
                "source .venv/bin/activate && pytest -q",
                "Loaded .venv/bin/activate, ran pytest",
            ),
            (
                "curl -s localhost:8080/api/health",
                "Fetched localhost:8080",
            ),
            (
                "cat a; cargo test; ls; git status; cargo build",
                "Read a, ran cargo test, listed files, and 2 more steps",
            ),
        ];
        for (command, expected) in cases {
            assert_eq!(sentence(command).as_deref(), Some(expected), "{command:?}");
        }
    }

    #[test]
    fn exploration_is_only_looking() {
        for command in [
            "ls -la && which cargo",
            "cat Cargo.toml",
            "rg -n foo src | head",
            "find . -name '*.rs' | wc -l",
            "git log --oneline -5",
            "git branch -a",
            "sed -n '1,40p' a.rs",
            "awk '{print $1}' a.txt",
        ] {
            assert!(explores(command), "{command:?} should explore");
        }
        for command in [
            "ls > files.txt",
            "rg --files | tee out.txt",
            "git branch new-feature",
            "git stash",
            "sed -i 's/a/b/' a.rs",
            "cat a.txt > b.txt",
            "cargo test",
            "ssh box 'rm -rf /tmp/x'",
            "gh issue edit 1 --add-label bug",
            "gh api -X POST repos/x/y/issues -f title=t | jq .",
        ] {
            assert!(
                !explores(command),
                "{command:?} must not count as exploring"
            );
        }
    }

    #[test]
    fn unsure_means_the_raw_command() {
        for command in [
            "find . -name '*.tmp' -delete",
            "find . -type f -exec rm {} +",
            "rg --pre ./x.sh foo",
            "rg --files | xargs rm",
            "sed -n 'w out.txt' a",
            "sed 's/a/b/w out.txt' a",
            "awk '{print > \"f\"}' a",
            "somebinary --flag",
            "echo $(rm -rf /)",
            "cd somewhere",
            "echo hello",
            "ls &&",
            "cat > a.rs <<'EOF'\nfn main() {}\n",
            "ssh box 'frobnicate --all'",
            "awk '{print $1 > \"out.txt\"}' a",
            "for f in *.rs; do wc -l $f; done",
        ] {
            assert_eq!(sentence(command), None, "{command:?}");
        }
    }
}
