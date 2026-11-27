//! git's commands that need the network and the user's keys: what each may be given, what the
//! user is asked, and how the gate does it through its mirror of the remote ([`relay`]), so that
//! git with the user's keys never runs in the agent's repository.
//!
//! The agent's repository is read here only as data (its remotes' addresses, its branch) and in
//! the sandbox (which commits a branch has).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::config::{self, Repo, normalize_url, unsafe_url};
use super::relay::{Env, Mirror};
use super::{Class, Options, Plan, Request, inside, parse_args, project_of};

/// Options before the command that may come along (`git -C dir push`).
const GLOBAL_FLAGS: &[&str] = &[
    "--no-pager",
    "-P",
    "--no-optional-locks",
    "--no-replace-objects",
];

/// How the gate does a git request (made with the plan, so the user is asked about it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Get `mirrors` up to date, then run the agent's own command in the sandbox, reading the
    /// mirrors in place of the remotes. `repeat`: again while it finds more submodules.
    Through { mirrors: Vec<Get>, repeat: bool },
    /// `git ls-remote`, in the mirror.
    List { url: String, args: Vec<String> },
    /// Pushes from the mirror, which has the commits.
    Push(Push),
    /// Uploads the LFS files of `refs` (in the mirror) or the files `oids`.
    LfsPush {
        url: String,
        options: Vec<String>,
        refs: Vec<String>,
        oids: Vec<String>,
    },
}

/// A remote to get into its mirror first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Get {
    pub url: String,
    /// The agent's `--depth`, `--shallow-since`, … (the mirror takes them while it is shallow).
    pub depth: Vec<String>,
    /// Whether the agent wants the whole history: `None` when that is whether its repository
    /// isn't shallow.
    pub whole: Option<bool>,
    /// The mirror's `HEAD` must be the remote's default branch (cloning).
    pub head: bool,
    /// LFS files to get into the mirror: git-lfs's options and the revisions.
    pub lfs: Option<(Vec<String>, Vec<String>)>,
}

impl Get {
    fn new(url: String) -> Self {
        Self {
            url,
            depth: Vec::new(),
            whole: None,
            head: false,
            lfs: None,
        }
    }
}

/// A push, as the mirror does it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Push {
    pub url: String,
    pub options: Vec<String>,
    /// `[+]<object>:<ref>` or `:<ref>`.
    pub specs: Vec<String>,
    /// What the remote's refs are after it: `(refs/heads/x, object or None when deleted)`.
    pub updates: Vec<(String, Option<String>)>,
    /// The remote's name in the agent's repository, whose `refs/remotes/<name>/` follow.
    pub remote: Option<String>,
    /// Branches that follow what they were pushed to (`-u`): `(local, remote)`.
    pub upstream: Vec<(String, String)>,
}

/// The plan for a `git` request, `None` when it is git's local work (run in the sandbox), or why
/// it is refused.
pub fn plan(request: &Request, env: &Env) -> Result<Option<Plan>, String> {
    let args = &request.args;
    let mut dir = request.cwd.clone();
    let mut odd_globals = Vec::new();
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if !arg.starts_with('-') {
            break;
        }
        if arg == "-C" {
            let Some(value) = args.get(at + 1) else {
                return Ok(None);
            };
            dir = dir.join(value);
            at += 2;
            continue;
        }
        if arg == "-c" || arg == "--config-env" {
            odd_globals.push(arg.clone());
            at += 2;
            continue;
        }
        if !GLOBAL_FLAGS.contains(&arg.as_str()) {
            odd_globals.push(arg.clone());
        }
        at += 1;
    }
    let Some(command) = args.get(at) else {
        return Ok(None);
    };
    let rest = &args[at + 1..];
    let kind = match command.as_str() {
        "clone" | "fetch" | "pull" | "push" | "ls-remote" => command.as_str(),
        "submodule" if rest.first().map(String::as_str) == Some("update") => "submodule",
        "lfs"
            if matches!(
                rest.first().map(String::as_str),
                Some("fetch" | "pull" | "push")
            ) =>
        {
            "lfs"
        }
        _ => return Ok(None),
    };
    // What runs in the sandbox may take any of git's options; what the mirror does is made here.
    let in_mirror = matches!(kind, "push" | "ls-remote")
        || (kind == "lfs" && rest.first().map(String::as_str) == Some("push"));
    if in_mirror && let Some(odd) = odd_globals.first() {
        return Err(format!(
            "Pistachio runs `git {command}` outside the sandbox only without `{odd}` before it."
        ));
    }
    let project = project_of(request);
    if !inside(&dir, &project) {
        return Err(format!(
            "Pistachio runs `git {command}` outside the sandbox only in the session's folder ({}).",
            project.display()
        ));
    }
    let cx = Cx {
        dir: &dir,
        project: &project,
        env,
        request,
    };
    let plan = match kind {
        "clone" => clone(&cx, rest)?,
        "fetch" => fetch(&cx, rest)?,
        "pull" => pull(&cx, rest)?,
        "push" => push(&cx, rest)?,
        "ls-remote" => ls_remote(&cx, rest)?,
        "submodule" => submodule_update(&cx, &rest[1..])?,
        _ => lfs(&cx, &rest[0], &rest[1..])?,
    };
    Ok(Some(plan))
}

struct Cx<'a> {
    request: &'a Request,
    /// Where git runs (with `-C`).
    dir: &'a Path,
    project: &'a Path,
    env: &'a Env,
}

impl Cx<'_> {
    /// The repository the command is for, read as data.
    fn repo(&self, command: &str) -> Result<Repo, String> {
        Repo::find(self.dir).ok_or_else(|| {
            format!(
                "`git {command}`: {} is not in a git repository.",
                self.dir.display()
            )
        })
    }

    fn plan(&self, title: String, class: Class, target: String, job: Job) -> Plan {
        Plan {
            title,
            details: Vec::new(),
            warnings: Vec::new(),
            class,
            target,
            rememberable: true,
            program: "git".to_owned(),
            args: self.request.args.clone(),
            cwd: self.request.cwd.clone(),
            stdin: None,
            accounts: Vec::new(),
            account: None,
            job: Some(job),
            after: None,
        }
    }

    /// git's output in the sandbox, where the command runs.
    fn local(&self, args: &[&str]) -> Option<String> {
        self.env
            .sandboxed_output(self.project, self.dir, args, None)
    }

    fn mirror(&self, url: &str) -> Result<Mirror, String> {
        Mirror::open(self.env, self.project, url)
    }
}

/// The remote a branch pushes to or fetches from: its own setting, else `origin`.
fn default_remote(repo: &Repo, push: bool) -> String {
    let branch = repo.branch();
    let from_branch = |name: &str| {
        branch
            .as_deref()
            .and_then(|branch| repo.get("branch", Some(branch), name))
    };
    let chosen = if push {
        from_branch("pushremote").or_else(|| repo.get("remote", None, "pushdefault"))
    } else {
        None
    };
    chosen
        .or_else(|| from_branch("remote"))
        .unwrap_or_else(|| "origin".to_owned())
}

/// Whether `name` is an address rather than a remote's name.
fn is_address(name: &str) -> bool {
    name.contains(':') || name.contains('/')
}

/// A remote's name and address: `name` may be an address itself.
fn resolve_remote(repo: &Repo, name: &str, push: bool) -> Result<(String, String), String> {
    if is_address(name) {
        if unsafe_url(name) {
            return Err(format!(
                "Pistachio won't use `{name}` outside the sandbox: only addresses of remote \
                 servers (https, ssh), not local folders or helper programs."
            ));
        }
        return Ok((name.to_owned(), name.to_owned()));
    }
    let url = push
        .then(|| repo.get("remote", Some(name), "pushurl"))
        .flatten()
        .or_else(|| repo.get("remote", Some(name), "url"))
        .ok_or_else(|| format!("There is no remote called `{name}` in this repository."))?;
    if unsafe_url(&url) {
        return Err(format!(
            "Pistachio won't use the remote {name} ({url}) outside the sandbox: only addresses of \
             remote servers (https, ssh), not local folders or helper programs."
        ));
    }
    Ok((name.to_owned(), url))
}

/// The branch a remote's `HEAD` points to (its default branch), if the agent's repository
/// knows it.
fn remote_default_branch(repo: &Repo, remote: &str) -> Option<String> {
    let text = std::fs::read_to_string(
        repo.common_dir
            .join("refs/remotes")
            .join(remote)
            .join("HEAD"),
    )
    .ok()?;
    text.trim()
        .strip_prefix(&format!("ref: refs/remotes/{remote}/"))
        .map(str::to_owned)
}

/// The depth options among `parsed`'s.
fn depth_options(parsed: &super::Parsed) -> Vec<String> {
    let mut depth = Vec::new();
    for (name, value) in &parsed.values {
        if matches!(
            name.as_str(),
            "--depth" | "--deepen" | "--shallow-since" | "--shallow-exclude"
        ) {
            depth.push(format!("{name}={value}"));
        }
    }
    if parsed.has("--unshallow") {
        depth.push("--unshallow".to_owned());
    }
    depth
}

const PUSH: Options<'_> = Options {
    flags: &[
        "-u",
        "--set-upstream",
        "--tags",
        "--follow-tags",
        "--no-follow-tags",
        "-f",
        "--force",
        "--force-with-lease",
        "--force-if-includes",
        "--no-force-if-includes",
        "-d",
        "--delete",
        "-n",
        "--dry-run",
        "--no-verify",
        "--verify",
        "-v",
        "--verbose",
        "-q",
        "--quiet",
        "--porcelain",
        "--atomic",
        "--no-atomic",
        "--prune",
        "--all",
        "--branches",
        "--progress",
        "--no-progress",
        "--no-signed",
    ],
    valued: &[],
};

/// Options of `git push` the mirror's push takes as they are.
const PUSH_AS_IS: &[&str] = &[
    "-n",
    "--dry-run",
    "-v",
    "--verbose",
    "-q",
    "--quiet",
    "--porcelain",
    "--atomic",
    "--no-atomic",
    "--progress",
    "--no-progress",
    "--tags",
    "--follow-tags",
];

/// One thing to push: from the agent's `source` (a full ref name; empty to delete) to the
/// remote's `destination` (a full ref name).
struct Pair {
    source: String,
    destination: String,
    force: bool,
}

/// `refs/heads/x` → `x` (and `refs/tags/x` → `x`), for showing.
fn short(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .unwrap_or(name)
}

fn push(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &PUSH, "git push")?;
    let repo = cx.repo("push")?;
    let remote_arg = parsed.positional.first().cloned();
    let remote_name = remote_arg
        .clone()
        .unwrap_or_else(|| default_remote(&repo, true));
    let (remote, url) = resolve_remote(&repo, &remote_name, true)?;
    let named = !is_address(&remote);
    let target = normalize_url(&url);
    let current = repo.branch();
    let mirror = cx.mirror(&url)?;
    let default = mirror
        .git_output(&["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|name| name.strip_prefix("origin/").map(str::to_owned))
        .or_else(|| remote_default_branch(&repo, &remote));
    let is_default = |branch: &str| match &default {
        Some(default) => short(branch) == default,
        None => matches!(short(branch), "main" | "master"),
    };
    let lease = parsed
        .flags
        .iter()
        .any(|given| given == "--force-with-lease" || given.starts_with("--force-with-lease="));
    let force_all = parsed.has("-f") || parsed.has("--force");
    let delete = parsed.has("-d") || parsed.has("--delete");
    let all = parsed.has("--all") || parsed.has("--branches");
    let prune = parsed.has("--prune");
    let tags = parsed.has("--tags");
    let follow_tags = parsed.has("--follow-tags")
        || (!parsed.has("--no-follow-tags")
            && repo
                .get("push", None, "followtags")
                .is_some_and(|value| value == "true"));
    if prune && !all {
        return Err(
            "Pistachio runs `git push --prune` outside the sandbox only with --all.".to_owned(),
        );
    }
    // A name in the agent's repository as a full ref name (`main` → `refs/heads/main`).
    let full_name = |name: &str| -> Result<String, String> {
        if name == "HEAD" {
            return current
                .as_ref()
                .map(|branch| format!("refs/heads/{branch}"))
                .ok_or_else(|| {
                    "`git push` of HEAD needs a checked-out branch (HEAD is detached).".to_owned()
                });
        }
        if name.starts_with("refs/") {
            return Ok(name.to_owned());
        }
        if name.starts_with('-') {
            return Err(format!("`{name}` isn't a branch or tag."));
        }
        cx.local(&["rev-parse", "--symbolic-full-name", name])
            .filter(|full| full.starts_with("refs/"))
            .ok_or_else(|| {
                format!(
                    "Pistachio pushes branches and tags; `{name}` isn't one in this repository."
                )
            })
    };
    let mut pairs: Vec<Pair> = Vec::new();
    for spec in parsed.positional.iter().skip(1) {
        let (plus, spec) = match spec.strip_prefix('+') {
            Some(rest) => (true, rest),
            None => (false, spec.as_str()),
        };
        let (source, destination) = match spec.split_once(':') {
            Some((source, destination)) => (source.to_owned(), destination.to_owned()),
            None if delete => (String::new(), spec.to_owned()),
            None => (spec.to_owned(), spec.to_owned()),
        };
        let source = if source.is_empty() {
            String::new()
        } else {
            full_name(&source)?
        };
        let destination = if destination.starts_with("refs/") {
            destination
        } else if destination == "HEAD" || destination.is_empty() {
            source.clone()
        } else if source.starts_with("refs/tags/") {
            format!("refs/tags/{destination}")
        } else {
            format!("refs/heads/{destination}")
        };
        pairs.push(Pair {
            source,
            destination,
            force: plus,
        });
    }
    if all {
        let branches = cx
            .local(&["for-each-ref", "--format=%(refname)", "refs/heads"])
            .unwrap_or_default();
        for branch in branches.lines().filter(|line| !line.is_empty()) {
            pairs.push(Pair {
                source: branch.to_owned(),
                destination: branch.to_owned(),
                force: false,
            });
        }
        if prune {
            let theirs = mirror
                .git_output(&["for-each-ref", "--format=%(refname)", "refs/heads"])
                .unwrap_or_default();
            for branch in theirs.lines().filter(|line| !line.is_empty()) {
                if !branches.lines().any(|ours| ours == branch) {
                    pairs.push(Pair {
                        source: String::new(),
                        destination: branch.to_owned(),
                        force: false,
                    });
                }
            }
        }
    }
    if pairs.is_empty() && !tags {
        let Some(branch) = current.clone() else {
            return Err(
                "`git push` without a branch needs a checked-out branch (HEAD is detached)."
                    .to_owned(),
            );
        };
        let upstream = repo
            .get("branch", Some(&branch), "merge")
            .filter(|merge| merge.starts_with("refs/heads/"))
            .unwrap_or_else(|| format!("refs/heads/{branch}"));
        pairs.push(Pair {
            source: format!("refs/heads/{branch}"),
            destination: upstream,
            force: false,
        });
    }
    // The commits (and tags) go into the mirror now: the question says what is in it.
    let mut sources: Vec<String> = pairs
        .iter()
        .map(|pair| pair.source.clone())
        .filter(|source| !source.is_empty())
        .collect();
    let tag_refs: Vec<String> = if tags || follow_tags {
        cx.local(&["for-each-ref", "--format=%(refname)", "refs/tags"])
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    sources.extend(tag_refs.iter().cloned());
    sources.dedup();
    let objects = if sources.is_empty() {
        Vec::new()
    } else {
        mirror.import(cx.project, cx.dir, &sources)?
    };
    let object_of = |source: &str| {
        sources
            .iter()
            .position(|given| given == source)
            .map(|at| objects[at].clone())
    };
    // Tags the remote doesn't have go where the mirror's push takes them from.
    for tag in &tag_refs {
        if mirror.resolve(tag).is_none()
            && let Some(object) = object_of(tag)
        {
            mirror.git_output(&["update-ref", tag, &object])?;
        }
    }
    // What the agent's repository says the remote has (for leases and counting).
    let tracking = |destination: &str| -> Option<String> {
        if !named {
            return None;
        }
        let branch = destination.strip_prefix("refs/heads/")?;
        cx.local(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{branch}"),
        ])
        .filter(|sha| mirror.has(sha))
    };
    let mut rewrite = delete || prune || force_all || lease;
    let mut details = Vec::new();
    let mut warnings = Vec::new();
    let mut specs = Vec::new();
    let mut updates = Vec::new();
    let mut options: Vec<String> = parsed
        .flags
        .iter()
        .filter(|flag| PUSH_AS_IS.contains(&flag.as_str()))
        .cloned()
        .collect();
    if follow_tags && !options.iter().any(|flag| flag == "--follow-tags") {
        options.push("--follow-tags".to_owned());
    }
    let mut counted = Vec::new();
    for pair in &pairs {
        rewrite |= pair.force || pair.source.is_empty();
        if pair.source.is_empty() {
            specs.push(format!(":{}", pair.destination));
            updates.push((pair.destination.clone(), None));
            continue;
        }
        let object = object_of(&pair.source).ok_or("A source wasn't packed.")?;
        let plus = if pair.force || force_all { "+" } else { "" };
        specs.push(format!("{plus}{object}:{}", pair.destination));
        updates.push((pair.destination.clone(), Some(object.clone())));
        let base = mirror
            .resolve(&pair.destination)
            .or_else(|| tracking(&pair.destination));
        counted.push((pair, object, base));
    }
    for flag in parsed
        .flags
        .iter()
        .filter(|flag| flag.starts_with("--force-with-lease"))
    {
        match flag.strip_prefix("--force-with-lease=") {
            None => {
                for pair in pairs
                    .iter()
                    .filter(|pair| !pair.source.is_empty() || delete)
                {
                    let expected = tracking(&pair.destination).unwrap_or_default();
                    options.push(format!(
                        "--force-with-lease={}:{expected}",
                        pair.destination
                    ));
                }
            }
            Some(given) => {
                let (name, expected) = match given.split_once(':') {
                    Some((name, expected)) => (name, Some(expected)),
                    None => (given, None),
                };
                let destination = if name.starts_with("refs/") {
                    name.to_owned()
                } else {
                    format!("refs/heads/{name}")
                };
                let expected = match expected {
                    Some("") => String::new(),
                    Some(rev) => cx
                        .local(&["rev-parse", "--verify", "--quiet", "--end-of-options", rev])
                        .ok_or_else(|| format!("`{rev}` isn't something in this repository."))?,
                    None => tracking(&destination).unwrap_or_default(),
                };
                options.push(format!("--force-with-lease={destination}:{expected}"));
            }
        }
    }
    let to_default = pairs
        .iter()
        .any(|pair| pair.destination.starts_with("refs/heads/") && is_default(&pair.destination));
    let class = if rewrite {
        Class::Rewrite
    } else if to_default {
        Class::PushDefault
    } else {
        Class::Push
    };
    let deletes: Vec<&str> = pairs
        .iter()
        .filter(|pair| pair.source.is_empty())
        .map(|pair| short(&pair.destination))
        .collect();
    let title = if !deletes.is_empty() && deletes.len() == pairs.len() {
        format!("Delete {} on {target}", deletes.join(", "))
    } else if all {
        format!("Push all branches to {target}")
    } else if pairs.is_empty() {
        format!("Push tags to {target}")
    } else if let [(pair, object, base)] = counted.as_slice() {
        let (source, destination) = (short(&pair.source), short(&pair.destination));
        match base {
            Some(base) => {
                let range = format!("{base}..{object}");
                let count: usize = mirror
                    .git_output(&["rev-list", "--count", &range])
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or(0);
                details.extend(
                    mirror
                        .git_output(&["log", "--format=%h %s", "-n", "6", &range])
                        .unwrap_or_default()
                        .lines()
                        .map(str::to_owned),
                );
                if mirror
                    .git_output(&["merge-base", "--is-ancestor", base, object])
                    .is_err()
                {
                    warnings.push(format!(
                        "{destination} on the remote has commits this doesn't: they would be lost."
                    ));
                }
                let commits = if count == 1 {
                    "1 commit".to_owned()
                } else {
                    format!("{count} commits")
                };
                if source == destination {
                    format!("Push {commits} on {source} to {target}")
                } else {
                    format!("Push {commits} from {source} to {destination} on {target}")
                }
            }
            None => {
                details.push(format!("At {}", &object[..object.len().min(7)]));
                format!("Push {source} to {target} as {destination} (new there)")
            }
        }
    } else {
        let names: Vec<&str> = counted
            .iter()
            .map(|(pair, _, _)| short(&pair.source))
            .collect();
        format!("Push {} to {target}", names.join(", "))
    };
    if !deletes.is_empty() {
        if deletes.len() != pairs.len() {
            details.push(format!("And deletes {} there.", deletes.join(", ")));
        }
        warnings.push("Deletes what is on the remote; it can't be undone from here.".to_owned());
    }
    if tags && !pairs.is_empty() {
        details.push("And all tags.".to_owned());
    }
    if force_all || lease || pairs.iter().any(|pair| pair.force) {
        warnings.push(
            "Force push: replaces the remote's history, which can lose others' work.".to_owned(),
        );
    }
    if to_default && deletes.len() != pairs.len() {
        let default = default.clone().unwrap_or_else(|| "main".to_owned());
        warnings.push(format!("Goes straight to the default branch ({default})."));
    }
    if remote_arg.as_deref().is_some_and(is_address) {
        warnings.push("An address given directly, not one of the repository's remotes.".to_owned());
    }
    let upstream = if parsed.has("-u") || parsed.has("--set-upstream") {
        pairs
            .iter()
            .filter(|pair| {
                pair.source.starts_with("refs/heads/")
                    && pair.destination.starts_with("refs/heads/")
            })
            .map(|pair| (short(&pair.source).to_owned(), pair.destination.clone()))
            .collect()
    } else {
        Vec::new()
    };
    let job = Job::Push(Push {
        url: url.clone(),
        options,
        specs,
        updates,
        remote: named.then_some(remote),
        upstream,
    });
    let mut plan = cx.plan(title, class, target, job);
    plan.details = details;
    plan.warnings = warnings;
    Ok(plan)
}

const FETCH: Options<'_> = Options {
    flags: &[
        "--all",
        "--prune",
        "-p",
        "--prune-tags",
        "-P",
        "--tags",
        "-t",
        "--no-tags",
        "-n",
        "--unshallow",
        "--update-shallow",
        "-v",
        "--verbose",
        "-q",
        "--quiet",
        "--progress",
        "--no-progress",
        "--dry-run",
        "-f",
        "--force",
        "--no-recurse-submodules",
        "--set-upstream",
        "--atomic",
        "--write-fetch-head",
        "--no-write-fetch-head",
        "--show-forced-updates",
        "--no-show-forced-updates",
    ],
    valued: &[
        "--depth",
        "--deepen",
        "--shallow-since",
        "--shallow-exclude",
        "--filter",
        "-j",
        "--jobs",
        "--recurse-submodules",
    ],
};

/// `--recurse-submodules` would fetch other repositories, which the question doesn't list.
fn no_submodules(parsed: &super::Parsed, command: &str) -> Result<(), String> {
    match parsed.value("--recurse-submodules") {
        None | Some("no") => Ok(()),
        Some(_) => Err(format!(
            "Pistachio runs `git {command}` without submodules; get them with \
             `git submodule update --init`."
        )),
    }
}

fn fetch(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &FETCH, "git fetch")?;
    no_submodules(&parsed, "fetch")?;
    let repo = cx.repo("fetch")?;
    let mut urls: Vec<String> = if parsed.has("--all") {
        repo.entries()
            .into_iter()
            .filter(|entry| entry.section == "remote" && entry.name == "url")
            .map(|entry| entry.value)
            .collect()
    } else {
        let name = parsed
            .positional
            .first()
            .cloned()
            .unwrap_or_else(|| default_remote(&repo, false));
        vec![resolve_remote(&repo, &name, false)?.1]
    };
    urls.sort();
    urls.dedup();
    if let Some(unsafe_one) = urls.iter().find(|url| unsafe_url(url)) {
        return Err(format!(
            "Pistachio won't fetch from {unsafe_one} outside the sandbox: only from remote servers \
             (https, ssh)."
        ));
    }
    let mut shown: Vec<String> = urls.iter().map(|url| normalize_url(url)).collect();
    shown.dedup();
    let target = shown.join(", ");
    let depth = depth_options(&parsed);
    let mirrors = urls
        .into_iter()
        .map(|url| Get {
            depth: depth.clone(),
            ..Get::new(url)
        })
        .collect();
    Ok(cx.plan(
        format!("Fetch from {target}"),
        Class::Fetch,
        target,
        Job::Through {
            mirrors,
            repeat: false,
        },
    ))
}

const PULL: Options<'_> = Options {
    flags: &[
        "--rebase",
        "-r",
        "--no-rebase",
        "--ff-only",
        "--ff",
        "--no-ff",
        "--autostash",
        "--no-autostash",
        "--tags",
        "--no-tags",
        "--prune",
        "-p",
        "-v",
        "--verbose",
        "-q",
        "--quiet",
        "--progress",
        "--no-progress",
        "--no-edit",
        "--squash",
        "--no-squash",
        "--commit",
        "--no-commit",
        "--stat",
        "--no-stat",
        "-n",
        "--unshallow",
        "--allow-unrelated-histories",
        "--no-verify",
        "--no-recurse-submodules",
        "-S",
        "--gpg-sign",
        "--no-gpg-sign",
    ],
    valued: &[
        "--depth",
        "--deepen",
        "--shallow-since",
        "-s",
        "--strategy",
        "-X",
        "--strategy-option",
        "--recurse-submodules",
    ],
};

fn pull(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &PULL, "git pull")?;
    no_submodules(&parsed, "pull")?;
    let repo = cx.repo("pull")?;
    let name = parsed
        .positional
        .first()
        .cloned()
        .unwrap_or_else(|| default_remote(&repo, false));
    let (_, url) = resolve_remote(&repo, &name, false)?;
    let target = normalize_url(&url);
    let into = repo.branch().unwrap_or_else(|| "HEAD".to_owned());
    // The branch merged: the one named, else what the current one follows.
    let merged = parsed.positional.get(1).cloned().or_else(|| {
        repo.branch().map(|branch| {
            repo.get("branch", Some(&branch), "merge")
                .and_then(|merge| merge.strip_prefix("refs/heads/").map(str::to_owned))
                .unwrap_or(branch)
        })
    });
    let get = Get {
        depth: depth_options(&parsed),
        lfs: merged.map(|branch| (Vec::new(), vec![format!("refs/heads/{branch}")])),
        ..Get::new(url)
    };
    Ok(cx.plan(
        format!("Pull from {target} into {into}"),
        Class::Fetch,
        target,
        Job::Through {
            mirrors: vec![get],
            repeat: false,
        },
    ))
}

const CLONE: Options<'_> = Options {
    flags: &[
        "--single-branch",
        "--no-single-branch",
        "--no-tags",
        "--sparse",
        "-q",
        "--quiet",
        "-v",
        "--verbose",
        "--progress",
        "--no-progress",
        "--no-checkout",
        "-n",
        "--no-recurse-submodules",
    ],
    valued: &[
        "--depth",
        "--branch",
        "-b",
        "--filter",
        "--origin",
        "-o",
        "--shallow-since",
        "--shallow-exclude",
        "-j",
        "--jobs",
    ],
};

fn clone(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &CLONE, "git clone")?;
    let Some(url) = parsed.positional.first() else {
        return Err("`git clone` needs an address.".to_owned());
    };
    if unsafe_url(url) {
        return Err(format!(
            "Pistachio clones only from remote servers (https, ssh) outside the sandbox, not `{url}`."
        ));
    }
    let name = parsed.positional.get(1).cloned().unwrap_or_else(|| {
        let tail = url
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("repo");
        tail.strip_suffix(".git").unwrap_or(tail).to_owned()
    });
    let destination = cx.dir.join(&name);
    if !inside(&destination, cx.project) {
        return Err(format!(
            "Pistachio clones only into the session's folder ({}).",
            cx.project.display()
        ));
    }
    let occupied = std::fs::read_dir(&destination)
        .is_ok_and(|mut entries| entries.next().is_some())
        || destination.is_file();
    if occupied {
        return Err(format!(
            "`git clone`: {} is not empty.",
            destination.display()
        ));
    }
    let target = normalize_url(url);
    let depth = depth_options(&parsed);
    let branch = parsed.value("--branch").or_else(|| parsed.value("-b"));
    let checkout = !(parsed.has("--no-checkout") || parsed.has("-n"));
    let get = Get {
        whole: Some(depth.is_empty()),
        depth,
        head: true,
        lfs: checkout.then(|| {
            (
                Vec::new(),
                vec![branch.map_or_else(
                    || "HEAD".to_owned(),
                    |branch| format!("refs/heads/{branch}"),
                )],
            )
        }),
        ..Get::new(url.clone())
    };
    let mut plan = cx.plan(
        format!("Clone {target} into {}", shown(&destination, cx.project)),
        Class::Fetch,
        target,
        Job::Through {
            mirrors: vec![get],
            repeat: false,
        },
    );
    plan.details.push(format!("From {url}"));
    Ok(plan)
}

const LS_REMOTE: Options<'_> = Options {
    flags: &[
        "--branches",
        "--heads",
        "--tags",
        "-t",
        "--refs",
        "-q",
        "--quiet",
        "--symref",
        "--exit-code",
        "--get-url",
    ],
    valued: &["--sort"],
};

fn ls_remote(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &LS_REMOTE, "git ls-remote")?;
    let name = parsed.positional.first().cloned();
    let url = match (&name, Repo::find(cx.dir)) {
        (Some(name), _) if is_address(name) => {
            if unsafe_url(name) {
                return Err(format!("Pistachio won't use `{name}` outside the sandbox."));
            }
            name.clone()
        }
        (name, Some(repo)) => {
            let name = name.clone().unwrap_or_else(|| default_remote(&repo, false));
            resolve_remote(&repo, &name, false)?.1
        }
        (_, None) => {
            return Err("`git ls-remote` needs an address outside a repository.".to_owned());
        }
    };
    let target = normalize_url(&url);
    let mut list_args: Vec<String> = parsed.flags.clone();
    for (option, value) in &parsed.values {
        list_args.push(format!("{option}={value}"));
    }
    list_args.push("origin".to_owned());
    // Patterns (`--` would be taken for one).
    list_args.extend(parsed.positional.iter().skip(1).cloned());
    Ok(cx.plan(
        format!("List the branches and tags of {target}"),
        Class::Fetch,
        target,
        Job::List {
            url,
            args: list_args,
        },
    ))
}

const SUBMODULE: Options<'_> = Options {
    flags: &[
        "--init",
        "--recursive",
        "--remote",
        "--no-fetch",
        "-N",
        "--checkout",
        "--rebase",
        "--merge",
        "--force",
        "-f",
        "--single-branch",
        "--no-single-branch",
        "--recommend-shallow",
        "--no-recommend-shallow",
        "-q",
        "--quiet",
        "--progress",
    ],
    valued: &["--depth", "--jobs", "-j", "--filter"],
};

/// A submodule's address relative to its superproject's remote (`../lib.git`) as a whole one.
fn relative_url(base: &str, relative: &str) -> Option<String> {
    let mut base = base.trim_end_matches('/').to_owned();
    let mut rest = relative;
    loop {
        if let Some(next) = rest.strip_prefix("./") {
            rest = next;
        } else if let Some(next) = rest.strip_prefix("../") {
            let cut = base.rfind(['/', ':'])?;
            base.truncate(cut);
            rest = next;
        } else {
            break;
        }
    }
    Some(format!("{base}/{rest}"))
}

/// The submodules a repository's `.gitmodules` lists, as `(path, address)` (relative addresses
/// resolved against `base`, its remote's address), its own settings first. Read as data.
fn submodules(
    top: &Path,
    repo: Option<&Repo>,
    base: Option<&str>,
) -> Vec<(String, Result<String, String>)> {
    let modules = std::fs::read_to_string(top.join(".gitmodules")).unwrap_or_default();
    let entries = config::parse(&modules);
    let mut found = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.section == "submodule" && entry.name == "url")
    {
        let Some(name) = &entry.subsection else {
            continue;
        };
        let path = entries
            .iter()
            .find(|other| other.subsection.as_deref() == Some(name) && other.name == "path")
            .map_or(name.as_str(), |other| other.value.as_str());
        let url = repo
            .and_then(|repo| repo.get("submodule", Some(name), "url"))
            .unwrap_or_else(|| entry.value.clone());
        let url = if url.starts_with("./") || url.starts_with("../") {
            base.and_then(|base| relative_url(base, &url)).ok_or_else(|| {
                format!(
                    "The submodule {name}'s address is relative, and its repository has no remote \
                     to resolve it against."
                )
            })
        } else {
            Ok(url)
        };
        found.push((path.to_owned(), url));
    }
    found
}

/// Every submodule address under `top`, in checked-out submodules too (`depth` levels).
fn all_submodule_urls(top: &Path, base: Option<&str>, depth: usize, into: &mut Vec<String>) {
    let repo = Repo::find(top).filter(|repo| repo.top == crate::paths::resolve(top));
    for (path, url) in submodules(top, repo.as_ref(), base) {
        let Ok(url) = url else { continue };
        if unsafe_url(&url) {
            continue;
        }
        if !into.contains(&url) {
            into.push(url.clone());
        }
        if depth > 0 {
            all_submodule_urls(&top.join(&path), Some(&url), depth - 1, into);
        }
    }
}

fn submodule_update(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &SUBMODULE, "git submodule update")?;
    let repo = cx.repo("submodule update")?;
    let origin = default_remote(&repo, false);
    let base = repo.get("remote", Some(&origin), "url");
    let mut urls = Vec::new();
    for (path, url) in submodules(&repo.top, Some(&repo), base.as_deref()) {
        if !parsed.positional.is_empty()
            && !parsed
                .positional
                .iter()
                .any(|given| given.trim_end_matches('/') == path)
        {
            continue;
        }
        let url = url?;
        if unsafe_url(&url) {
            return Err(format!(
                "Pistachio won't get the submodule {path} outside the sandbox: its address ({url}) \
                 is a local folder or a helper program."
            ));
        }
        urls.push((path, url));
    }
    if urls.is_empty() {
        return Err(
            "This repository has no submodules to get (.gitmodules lists none that match)."
                .to_owned(),
        );
    }
    let target = base
        .as_deref()
        .map_or_else(|| normalize_url(&urls[0].1), normalize_url);
    let names: Vec<&str> = urls.iter().map(|(path, _)| path.as_str()).collect();
    // The commits the superproject wants, for their LFS files.
    let paths: Vec<&str> = urls.iter().map(|(path, _)| path.as_str()).collect();
    let mut wanted = vec!["ls-files", "-s", "--"];
    wanted.extend(&paths);
    let gitlinks = cx.local(&wanted).unwrap_or_default();
    let depth = depth_options(&parsed);
    let mirrors = urls
        .iter()
        .map(|(path, url)| {
            let commit = gitlinks
                .lines()
                .find(|line| line.ends_with(&format!("\t{path}")))
                .and_then(|line| line.split_whitespace().nth(1))
                .map(str::to_owned);
            Get {
                depth: depth.clone(),
                whole: Some(depth.is_empty()),
                lfs: commit.map(|commit| (Vec::new(), vec![commit])),
                ..Get::new(url.clone())
            }
        })
        .collect();
    let mut plan = cx.plan(
        format!("Get the submodules {} of {target}", names.join(", ")),
        Class::Fetch,
        target,
        Job::Through {
            mirrors,
            repeat: parsed.has("--recursive"),
        },
    );
    plan.details = urls
        .iter()
        .map(|(path, url)| format!("{path}: {}", normalize_url(url)))
        .collect();
    if parsed.has("--recursive") {
        plan.warnings
            .push("And their own submodules, which aren't listed here.".to_owned());
    }
    Ok(plan)
}

fn lfs(cx: &Cx<'_>, command: &str, args: &[String]) -> Result<Plan, String> {
    let options = Options {
        flags: &[
            "--all",
            "-a",
            "--recent",
            "-r",
            "--prune",
            "-p",
            "--dry-run",
            "-d",
            "-q",
            "--quiet",
            "--object-id",
        ],
        valued: &["-I", "--include", "-X", "--exclude"],
    };
    let parsed = parse_args(args, &options, &format!("git lfs {command}"))?;
    let repo = cx.repo(&format!("lfs {command}"))?;
    let push = command == "push";
    let name = parsed
        .positional
        .first()
        .cloned()
        .unwrap_or_else(|| default_remote(&repo, push));
    let (_, url) = resolve_remote(&repo, &name, push)?;
    let target = normalize_url(&url);
    let revisions: Vec<String> = parsed.positional.iter().skip(1).cloned().collect();
    let all = parsed.has("--all") || parsed.has("-a");
    if !push {
        // The mirror gets the files the agent's command asks for; the command then copies them.
        let mut lfs_options: Vec<String> = parsed
            .flags
            .iter()
            .filter(|flag| matches!(flag.as_str(), "--all" | "-a" | "--recent" | "-r"))
            .cloned()
            .collect();
        for (option, value) in &parsed.values {
            lfs_options.push(option.clone());
            lfs_options.push(value.clone());
        }
        let revs = if revisions.is_empty() && !all {
            vec![
                cx.local(&["rev-parse", "--verify", "--quiet", "HEAD"])
                    .ok_or("`git lfs` needs a commit checked out.")?,
            ]
        } else {
            revisions
        };
        let get = Get {
            lfs: Some((lfs_options, revs)),
            ..Get::new(url)
        };
        return Ok(cx.plan(
            format!("Download LFS files from {target}"),
            Class::Fetch,
            target,
            Job::Through {
                mirrors: vec![get],
                repeat: false,
            },
        ));
    }
    let mut lfs_options: Vec<String> = parsed
        .flags
        .iter()
        .filter(|flag| matches!(flag.as_str(), "--dry-run" | "-d" | "-q" | "--quiet"))
        .cloned()
        .collect();
    let (refs, oids) = if parsed.has("--object-id") {
        lfs_options.push("--object-id".to_owned());
        (Vec::new(), revisions)
    } else {
        let mirror = cx.mirror(&url)?;
        let sources: Vec<String> = if all {
            cx.local(&["for-each-ref", "--format=%(refname)", "refs/heads"])
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        } else if revisions.is_empty() {
            vec![
                repo.branch()
                    .map(|branch| format!("refs/heads/{branch}"))
                    .ok_or("`git lfs push` needs a branch checked out, or one named.")?,
            ]
        } else {
            revisions
                .iter()
                .map(|name| {
                    if name.starts_with('-') {
                        return Err(format!("`{name}` isn't a branch or tag."));
                    }
                    cx.local(&["rev-parse", "--symbolic-full-name", name])
                        .filter(|full| full.starts_with("refs/"))
                        .ok_or_else(|| {
                            format!("`{name}` isn't a branch or tag in this repository.")
                        })
                })
                .collect::<Result<_, _>>()?
        };
        mirror.import(cx.project, cx.dir, &sources)?;
        let refs = sources
            .iter()
            .map(|source| format!("refs/pistachio/in/{}", source.trim_start_matches("refs/")))
            .collect();
        (refs, Vec::new())
    };
    Ok(cx.plan(
        format!("Upload LFS files to {target}"),
        Class::Push,
        target,
        Job::LfsPush {
            url,
            options: lfs_options,
            refs,
            oids,
        },
    ))
}

/// `path` relative to `project` for the question.
fn shown(path: &Path, project: &Path) -> String {
    let path: PathBuf = crate::paths::resolve(path);
    path.strip_prefix(crate::paths::resolve(project))
        .ok()
        .filter(|rest| !rest.as_os_str().is_empty())
        .map_or_else(
            || path.display().to_string(),
            |rest| rest.display().to_string(),
        )
}

/// Does `job` for `request` (allowed by the user), passing on git's output; the exit code.
pub fn execute(job: &Job, request: &Request, env: &Env) -> i32 {
    match run_job(job, request, env) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("[Pistachio] {error}");
            1
        }
    }
}

fn run_job(job: &Job, request: &Request, env: &Env) -> Result<i32, String> {
    let project = project_of(request);
    match job {
        Job::Through { mirrors, repeat } => through(mirrors, *repeat, request, &project, env),
        Job::List { url, args } => {
            let mirror = Mirror::open(env, &project, url)?;
            let mut command = mirror.git();
            command.arg("ls-remote").args(args).stdin(Stdio::null());
            Ok(status(command))
        }
        Job::Push(push) => push_from_mirror(push, request, &project, env),
        Job::LfsPush {
            url,
            options,
            refs,
            oids,
        } => {
            let mirror = Mirror::open(env, &project, url)?;
            mirror.lfs_server_ok()?;
            let oids = if oids.is_empty() {
                // What the remote lacks (the mirror knows what it has).
                mirror.fetch(&[])?;
                let mut command = mirror.git();
                command
                    .args(["lfs", "push", "--dry-run", "origin"])
                    .args(refs)
                    .stdin(Stdio::null())
                    .stderr(Stdio::inherit());
                let listed = command.output().map_err(|error| error.to_string())?;
                String::from_utf8_lossy(&listed.stdout)
                    .lines()
                    .filter_map(|line| line.strip_prefix("push "))
                    .filter_map(|rest| rest.split_whitespace().next())
                    .map(str::to_owned)
                    .collect()
            } else {
                oids.clone()
            };
            mirror.import_lfs(&project, &request.cwd, &oids)?;
            let mut command = mirror.git();
            command.args(["lfs", "push"]).args(options).arg("origin");
            if options.iter().any(|option| option == "--object-id") {
                command.args(&oids);
            } else {
                command.args(refs);
            }
            command.stdin(Stdio::null());
            Ok(status(command))
        }
    }
}

/// Runs `command`, its output passed on; the exit code.
fn status(mut command: std::process::Command) -> i32 {
    command
        .status()
        .map_or(127, |status| status.code().unwrap_or(1))
}

/// Gets the mirrors, then runs the agent's command in the sandbox reading them.
fn through(
    gets: &[Get],
    repeat: bool,
    request: &Request,
    project: &Path,
    env: &Env,
) -> Result<i32, String> {
    let mut mirrors: Vec<Mirror> = Vec::new();
    let mut done: Vec<String> = Vec::new();
    let agent_shallow = || {
        env.sandboxed_output(
            project,
            &request.cwd,
            &["rev-parse", "--is-shallow-repository"],
            None,
        )
        .is_some_and(|answer| answer == "true")
    };
    let mut pending: Vec<Get> = gets.to_vec();
    for round in 0..5 {
        for get in pending.drain(..) {
            if done.contains(&get.url) {
                continue;
            }
            let mirror = Mirror::open(env, project, &get.url)?;
            let whole = get.whole.unwrap_or_else(|| !agent_shallow());
            let options = if mirror.is_empty() || mirror.is_shallow() {
                if get.depth.iter().any(|option| option == "--unshallow")
                    || (whole && get.depth.is_empty())
                {
                    if mirror.is_shallow() {
                        vec!["--unshallow".to_owned()]
                    } else {
                        Vec::new()
                    }
                } else {
                    get.depth
                        .iter()
                        .filter(|option| !(mirror.is_empty() && option.starts_with("--deepen")))
                        .cloned()
                        .collect()
                }
            } else {
                Vec::new()
            };
            mirror.fetch(&options)?;
            // The mirror's HEAD is the remote's default branch (what a clone or a fetch of an
            // address gets).
            if get.head || mirror.resolve("refs/remotes/origin/HEAD").is_none() {
                let _ = mirror.git_output(&["remote", "set-head", "origin", "--auto"]);
            }
            if let Ok(head) = mirror.git_output(&["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
                && let Some(branch) = head.strip_prefix("refs/remotes/origin/")
            {
                let _ =
                    mirror.git_output(&["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")]);
            }
            if let Some((options, revs)) = &get.lfs {
                let explicit = options
                    .iter()
                    .any(|option| matches!(option.as_str(), "--all" | "-a" | "--recent" | "-r"));
                let wanted: Vec<String> = revs
                    .iter()
                    .filter(|rev| mirror.resolve(rev).is_some() && mirror.uses_lfs(rev))
                    .cloned()
                    .collect();
                if explicit || !wanted.is_empty() {
                    mirror.lfs_fetch(options, &wanted)?;
                }
            }
            done.push(get.url.clone());
            mirrors.push(mirror);
        }
        let mut command = env.sandboxed(
            project,
            &request.cwd,
            &mirrors
                .iter()
                .map(|mirror| mirror.dir.as_path())
                .collect::<Vec<_>>(),
        );
        for mirror in &mirrors {
            command.args(mirror.rewrite());
        }
        command.args(&request.args).stdin(Stdio::null());
        if !repeat {
            return Ok(status(command));
        }
        // Submodules: their own submodules show once they are there; again until none is new.
        let output = command.output().map_err(|error| error.to_string())?;
        let top = Repo::find(&request.cwd).map_or_else(|| request.cwd.clone(), |repo| repo.top);
        let base = Repo::find(&top).and_then(|repo| {
            let origin = default_remote(&repo, false);
            repo.get("remote", Some(&origin), "url")
        });
        let mut urls = Vec::new();
        all_submodule_urls(&top, base.as_deref(), 6, &mut urls);
        pending = urls
            .into_iter()
            .filter(|url| !done.contains(url))
            .map(|url| Get {
                whole: Some(true),
                ..Get::new(url)
            })
            .collect();
        if pending.is_empty() || round == 4 {
            use std::io::Write as _;
            let _ = std::io::stdout().write_all(&output.stdout);
            let _ = std::io::stderr().write_all(&output.stderr);
            return Ok(output.status.code().unwrap_or(1));
        }
    }
    Ok(1)
}

fn push_from_mirror(
    push: &Push,
    request: &Request,
    project: &Path,
    env: &Env,
) -> Result<i32, String> {
    let mirror = Mirror::open(env, project, &push.url)?;
    let mut command = mirror.git();
    command
        .arg("push")
        .args(&push.options)
        .arg("origin")
        .args(&push.specs)
        .stdin(Stdio::null());
    let code = status(command);
    let dry = push
        .options
        .iter()
        .any(|option| option == "-n" || option == "--dry-run");
    if code != 0 || dry {
        return Ok(code);
    }
    // The mirror and the agent's repository learn what the remote now has.
    let mut theirs = String::new();
    let mut ours = String::new();
    for (destination, object) in &push.updates {
        let branch = destination.strip_prefix("refs/heads/");
        match object {
            Some(object) => {
                let _ = writeln!(theirs, "update {destination} {object}");
                if let Some(branch) = branch {
                    let _ = writeln!(theirs, "update refs/remotes/origin/{branch} {object}");
                    if let Some(remote) = &push.remote {
                        let _ = writeln!(ours, "update refs/remotes/{remote}/{branch} {object}");
                    }
                }
            }
            None => {
                let _ = writeln!(theirs, "delete {destination}");
                if let Some(branch) = branch {
                    let _ = writeln!(theirs, "delete refs/remotes/origin/{branch}");
                    if let Some(remote) = &push.remote {
                        let _ = writeln!(ours, "delete refs/remotes/{remote}/{branch}");
                    }
                }
            }
        }
    }
    let mut update = mirror.git();
    update
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    if let Ok(mut child) = update.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            let _ = stdin.write_all(theirs.as_bytes());
        }
        let _ = child.wait();
    }
    if !ours.is_empty() {
        let _ = env.sandboxed_output(
            project,
            &request.cwd,
            &["update-ref", "--stdin"],
            Some(&ours),
        );
    }
    if let Some(remote) = &push.remote {
        for (local, destination) in &push.upstream {
            let _ = env.sandboxed_output(
                project,
                &request.cwd,
                &["config", &format!("branch.{local}.remote"), remote],
                None,
            );
            let _ = env.sandboxed_output(
                project,
                &request.cwd,
                &["config", &format!("branch.{local}.merge"), destination],
                None,
            );
            println!(
                "branch '{local}' set up to track '{remote}/{}'.",
                short(destination)
            );
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use pretty_assertions::assert_eq;

    const GIT_ENV: [(&str, &str); 6] = [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "a"),
        ("GIT_AUTHOR_EMAIL", "a@b"),
        ("GIT_COMMITTER_NAME", "a"),
        ("GIT_COMMITTER_EMAIL", "a@b"),
    ];

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .envs(GIT_ENV)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn git(dir: &Path, args: &[&str]) {
        git_out(dir, args);
    }

    /// A test setup: a remote (a bare repository standing in for GitHub at
    /// `git@github.com:a/app.git`), a clone of it in `work`, and the gate's env, whose sandbox
    /// is a stand-in that runs the command as it is.
    struct Setup {
        _dir: tempfile::TempDir,
        root: PathBuf,
        remote: PathBuf,
        work: PathBuf,
        env: Env,
    }

    impl Setup {
        fn plan(&self, args: &[&str]) -> Result<Option<Plan>, String> {
            plan(&request(&self.work, args), &self.env)
        }

        /// Plans and does `args` (as the gate would once allowed); the plan and exit code.
        fn run(&self, args: &[&str]) -> (Plan, i32) {
            self.run_in(&self.work, args)
        }

        fn run_in(&self, cwd: &Path, args: &[&str]) -> (Plan, i32) {
            let mut request = request(cwd, args);
            request.project = Some(self.root.join("work"));
            let plan = plan(&request, &self.env).expect("planned").expect("gated");
            let code = execute(plan.job.as_ref().expect("a job"), &request, &self.env);
            (plan, code)
        }
    }

    /// See [`Setup`]: `count` commits on `branch` that the remote doesn't have.
    fn repo(branch: &str, count: usize) -> Setup {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().canonicalize().expect("canonical");
        let remote = root.join("remote.git");
        let work = root.join("work");
        std::fs::create_dir_all(&work).expect("mkdir");
        git(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
        git(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        git(&work, &["remote", "set-head", "origin", "main"]);
        if branch != "main" {
            git(&work, &["checkout", "-q", "-b", branch]);
            git(&work, &["push", "-q", "origin", branch]);
        }
        for n in 0..count {
            git(
                &work,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &format!("change {n}"),
                ],
            );
        }
        // What the user's GitHub remote would look like.
        git(
            &work,
            &["remote", "set-url", "origin", "git@github.com:a/app.git"],
        );
        // The stand-in sandbox: `--sandbox-tool run … -- <program> <args>` runs the program.
        let exe = root.join("fake-pistachio");
        std::fs::write(
            &exe,
            "#!/bin/sh\nexport IN_TEST_SANDBOX=1\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        // The gate's git reaches "GitHub" (the local remote) under its address.
        let env = Env {
            exe,
            dir: root.join("sandbox"),
            extra: vec![
                ("protocol.file.allow".to_owned(), "always".to_owned()),
                (
                    format!("url.{}.insteadOf", remote.display()),
                    "git@github.com:a/app.git".to_owned(),
                ),
            ],
        };
        Setup {
            _dir: dir,
            root,
            remote,
            work,
            env,
        }
    }

    fn request(work: &Path, args: &[&str]) -> Request {
        Request {
            tool: "git".into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cwd: work.to_path_buf(),
            project: Some(work.to_path_buf()),
            stdin: None,
            account: None,
            approved: None,
        }
    }

    #[test]
    fn local_work_stays_in_the_sandbox() {
        let setup = repo("main", 0);
        for args in [
            &["status"][..],
            &["commit", "-m", "x"],
            &["log"],
            &["submodule", "status"],
            &["lfs", "install"],
            &["-c", "x=y", "status"],
        ] {
            assert_eq!(setup.plan(args), Ok(None), "{args:?}");
        }
    }

    #[test]
    fn a_push_says_what_goes_where() {
        let setup = repo("feature", 2);
        let push = setup.plan(&["push"]).expect("planned").expect("gated");
        assert_eq!(push.title, "Push 2 commits on feature to github.com/a/app");
        let subjects: Vec<&str> = push
            .details
            .iter()
            .map(|line| line.split_once(' ').map_or("", |(_, subject)| subject))
            .collect();
        assert_eq!(subjects, ["change 1", "change 0"]);
        assert_eq!(push.class, Class::Push);
        assert_eq!(push.target, "github.com/a/app");
        assert!(push.warnings.is_empty(), "{:?}", push.warnings);

        let new = setup
            .plan(&["push", "-u", "origin", "HEAD:topic"])
            .unwrap()
            .unwrap();
        assert_eq!(
            new.title,
            "Push feature to github.com/a/app as topic (new there)"
        );

        let main = setup
            .plan(&["push", "origin", "feature:main"])
            .unwrap()
            .unwrap();
        assert_eq!(main.class, Class::PushDefault);
        assert!(
            main.warnings[0].contains("default branch (main)"),
            "{:?}",
            main.warnings
        );

        let force = setup
            .plan(&["push", "--force-with-lease", "origin", "feature"])
            .unwrap()
            .unwrap();
        assert_eq!(force.class, Class::Rewrite);
        let plus = setup
            .plan(&["push", "origin", "+feature"])
            .unwrap()
            .unwrap();
        assert_eq!(plus.class, Class::Rewrite);

        let delete = setup
            .plan(&["push", "origin", "--delete", "old"])
            .unwrap()
            .unwrap();
        assert_eq!(delete.class, Class::Rewrite);
        assert_eq!(delete.title, "Delete old on github.com/a/app");
        let colon = setup.plan(&["push", "origin", ":old"]).unwrap().unwrap();
        assert_eq!(colon.class, Class::Rewrite);

        let tags = setup.plan(&["push", "--tags"]).unwrap().unwrap();
        assert_eq!(tags.title, "Push tags to github.com/a/app");
    }

    #[test]
    fn options_that_run_programs_are_refused() {
        let setup = repo("main", 1);
        for args in [
            &["push", "--receive-pack=evil", "origin"][..],
            &["-c", "core.sshCommand=evil", "push"],
            &["fetch", "--upload-pack=evil"],
            &["fetch", "--recurse-submodules"],
            &[
                "clone",
                "--config",
                "core.sshCommand=evil",
                "https://github.com/a/b",
            ],
            &["clone", "/Users/a/private"],
            &["clone", "ext::sh -c evil"],
            &["push", "ext::sh -c evil", "main"],
        ] {
            assert!(setup.plan(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn only_in_the_session_folder() {
        let setup = repo("main", 0);
        let mut outside = request(&setup.work, &["push"]);
        outside.cwd = setup.root.clone();
        assert!(plan(&outside, &setup.env).is_err());
        let escape = request(&setup.work, &["clone", "https://github.com/a/b", "../b"]);
        assert!(
            plan(&escape, &setup.env)
                .unwrap_err()
                .contains("session's folder")
        );
    }

    #[test]
    fn fetch_pull_clone_and_listing() {
        let setup = repo("main", 0);
        let fetch = setup.plan(&["fetch", "--prune"]).unwrap().unwrap();
        assert_eq!(
            (fetch.title.as_str(), fetch.class),
            ("Fetch from github.com/a/app", Class::Fetch)
        );
        let pull = setup.plan(&["pull", "--rebase"]).unwrap().unwrap();
        assert_eq!(pull.title, "Pull from github.com/a/app into main");
        let clone = setup
            .plan(&["clone", "--depth", "1", "https://github.com/b/lib.git"])
            .unwrap()
            .unwrap();
        assert_eq!(clone.title, "Clone github.com/b/lib into lib");
        assert_eq!(clone.target, "github.com/b/lib");
        let listing = setup
            .plan(&["ls-remote", "--tags", "https://gitlab.com/c/d"])
            .unwrap()
            .unwrap();
        assert_eq!(listing.target, "gitlab.com/c/d");
    }

    /// The agent's repository may set anything; git with the user's keys never reads it.
    #[test]
    fn the_repository_settings_never_reach_the_gates_git() {
        let setup = repo("main", 0);
        let pwned = setup.root.join("PWNED");
        // Only outside the (stand-in) sandbox does it count.
        let evil = format!(
            "[ -n \"$IN_TEST_SANDBOX\" ] || touch {}; false",
            pwned.display()
        );
        git(&setup.work, &["config", "core.sshCommand", &evil]);
        git(&setup.work, &["config", "core.fsmonitor", &evil]);
        git(
            &setup.work,
            &["config", "credential.helper", &format!("!{evil}")],
        );
        // A subfolder that is a bare repository with its own settings.
        git(&setup.work, &["init", "-q", "--bare", "nested"]);
        let nested = setup.work.join("nested");
        git(&nested, &["config", "core.sshCommand", &evil]);
        git(
            &nested,
            &["config", "remote.origin.url", "git@github.com:a/app.git"],
        );
        git_out(&setup.remote, &["rev-parse", "main"]);
        assert_eq!(setup.run(&["fetch"]).1, 0);
        assert_eq!(setup.run(&["ls-remote", "git@github.com:a/app.git"]).1, 0);
        assert_eq!(setup.run(&["push", "origin", "main:elsewhere"]).1, 0);
        let mut request = request(&nested, &["ls-remote"]);
        request.project = Some(setup.work.clone());
        // The gate reads the work tree's repository (the one with a `.git`), as data.
        let listing = plan(&request, &setup.env).unwrap().unwrap();
        assert_eq!(listing.target, "github.com/a/app");
        assert_eq!(
            execute(listing.job.as_ref().unwrap(), &request, &setup.env),
            0
        );
        assert!(!pwned.exists(), "a setting of the repository ran a program");
    }

    #[test]
    fn a_push_goes_from_the_mirror_and_the_repository_learns_it() {
        let setup = repo("feature", 2);
        let ours = git_out(&setup.work, &["rev-parse", "feature"]);
        let (_, code) = setup.run(&["push", "-u", "origin", "feature"]);
        assert_eq!(code, 0);
        assert_eq!(git_out(&setup.remote, &["rev-parse", "feature"]), ours);
        assert_eq!(git_out(&setup.work, &["rev-parse", "origin/feature"]), ours);
        assert_eq!(
            git_out(&setup.work, &["config", "branch.feature.merge"]),
            "refs/heads/feature"
        );
        // Nothing new: it still works.
        assert_eq!(setup.run(&["push"]).1, 0);
        // A new branch, and a delete.
        assert_eq!(setup.run(&["push", "origin", "feature:topic"]).1, 0);
        assert_eq!(git_out(&setup.remote, &["rev-parse", "topic"]), ours);
        assert_eq!(setup.run(&["push", "origin", "--delete", "topic"]).1, 0);
        assert!(
            Command::new("git")
                .args(["rev-parse", "--verify", "--quiet", "topic"])
                .current_dir(&setup.remote)
                .status()
                .is_ok_and(|status| !status.success())
        );
        // A rejected push (the remote moved on) fails, as git's would.
        let other = setup.root.join("other");
        git(
            &setup.root,
            &["clone", "-q", setup.remote.to_str().unwrap(), "other"],
        );
        git(&other, &["checkout", "-q", "feature"]);
        git(&other, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
        git(&other, &["push", "-q", "origin", "feature"]);
        git(
            &setup.work,
            &["commit", "-q", "--allow-empty", "-m", "ours"],
        );
        assert_ne!(setup.run(&["push"]).1, 0);
        // A lease on what the repository last saw refuses to replace their commit.
        assert_ne!(
            setup
                .run(&["push", "--force-with-lease", "origin", "feature"])
                .1,
            0
        );
        assert_eq!(setup.run(&["push", "--force", "origin", "feature"]).1, 0);
        assert_eq!(
            git_out(&setup.remote, &["rev-parse", "feature"]),
            git_out(&setup.work, &["rev-parse", "feature"])
        );
    }

    #[test]
    fn tags_go_along() {
        let setup = repo("main", 1);
        git(&setup.work, &["tag", "-a", "v1", "-m", "one"]);
        assert_eq!(setup.run(&["push", "--follow-tags"]).1, 0);
        assert_eq!(
            git_out(&setup.remote, &["rev-parse", "v1"]),
            git_out(&setup.work, &["rev-parse", "v1"])
        );
        git(&setup.work, &["tag", "v2"]);
        assert_eq!(setup.run(&["push", "--tags"]).1, 0);
        assert_eq!(
            git_out(&setup.remote, &["rev-parse", "v2"]),
            git_out(&setup.work, &["rev-parse", "v2"])
        );
    }

    #[test]
    fn fetch_pull_and_clone_come_through_the_mirror() {
        let setup = repo("main", 0);
        let other = setup.root.join("other");
        git(
            &setup.root,
            &["clone", "-q", setup.remote.to_str().unwrap(), "other"],
        );
        git(&other, &["commit", "-q", "--allow-empty", "-m", "news"]);
        git(&other, &["push", "-q", "origin", "main"]);
        let news = git_out(&other, &["rev-parse", "main"]);
        assert_eq!(setup.run(&["fetch"]).1, 0);
        assert_eq!(git_out(&setup.work, &["rev-parse", "origin/main"]), news);
        // The address stays the remote's.
        assert_eq!(
            git_out(&setup.work, &["remote", "get-url", "origin"]),
            "git@github.com:a/app.git"
        );
        git(&other, &["commit", "-q", "--allow-empty", "-m", "more"]);
        git(&other, &["push", "-q", "origin", "main"]);
        assert_eq!(setup.run(&["pull", "--ff-only"]).1, 0);
        assert_eq!(
            git_out(&setup.work, &["rev-parse", "HEAD"]),
            git_out(&other, &["rev-parse", "main"])
        );
        // A shallow clone, then all of it.
        let (_, code) = setup.run(&["clone", "--depth", "1", "git@github.com:a/app.git", "copy"]);
        assert_eq!(code, 0);
        let copy = setup.work.join("copy");
        assert_eq!(git_out(&copy, &["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(
            git_out(&copy, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "main"
        );
        assert_eq!(setup.run_in(&copy, &["fetch", "--unshallow"]).1, 0);
        assert_eq!(git_out(&copy, &["rev-list", "--count", "HEAD"]), "3");
        let (_, code) = setup.run(&["clone", "git@github.com:a/app.git", "full"]);
        assert_eq!(code, 0);
        assert_eq!(
            git_out(&setup.work.join("full"), &["rev-list", "--count", "HEAD"]),
            "3"
        );
    }

    #[test]
    fn submodules_are_listed_with_their_addresses() {
        let setup = repo("main", 0);
        std::fs::write(
            setup.work.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = vendor/lib\n\turl = ../lib.git\n[submodule \"x\"]\n\tpath = x\n\turl = https://gitlab.com/c/x\n",
        )
        .expect("write");
        let update = setup
            .plan(&["submodule", "update", "--init", "--recursive"])
            .unwrap()
            .unwrap();
        assert_eq!(
            update.title,
            "Get the submodules vendor/lib, x of github.com/a/app"
        );
        assert_eq!(
            update.details,
            ["vendor/lib: github.com/a/lib", "x: gitlab.com/c/x"]
        );
        assert_eq!(update.warnings.len(), 1);
        std::fs::write(
            setup.work.join(".gitmodules"),
            "[submodule \"e\"]\n\tpath = e\n\turl = /Users/a/secret\n",
        )
        .expect("write");
        assert!(setup.plan(&["submodule", "update", "--init"]).is_err());
    }

    #[test]
    fn submodules_come_through_their_mirrors() {
        let setup = repo("main", 0);
        // A library "on GitHub" with its own submodule.
        let make = |name: &str| {
            let bare = setup.root.join(format!("{name}.git"));
            let seed = setup.root.join(format!("{name}-seed"));
            git(
                &setup.root,
                &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
            );
            git(
                &setup.root,
                &["init", "-q", "-b", "main", seed.to_str().unwrap()],
            );
            std::fs::write(seed.join("file"), name).expect("write");
            (bare, seed)
        };
        let (deep, deep_seed) = make("deep");
        git(&deep_seed, &["add", "file"]);
        git(&deep_seed, &["commit", "-q", "-m", "deep"]);
        git(&deep_seed, &["push", "-q", deep.to_str().unwrap(), "main"]);
        let (lib, lib_seed) = make("lib");
        git(
            &lib_seed,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                deep.to_str().unwrap(),
                "deep",
            ],
        );
        git(
            &lib_seed,
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.deep.url",
                "git@github.com:a/deep.git",
            ],
        );
        git(&lib_seed, &["add", "-A"]);
        git(&lib_seed, &["commit", "-q", "-m", "lib"]);
        git(&lib_seed, &["push", "-q", lib.to_str().unwrap(), "main"]);
        git(
            &setup.work,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                lib.to_str().unwrap(),
                "lib",
            ],
        );
        git(
            &setup.work,
            &[
                "config",
                "-f",
                ".gitmodules",
                "submodule.lib.url",
                "../lib.git",
            ],
        );
        git(&setup.work, &["commit", "-q", "-am", "add lib"]);
        // A fresh clone of the superproject, without its submodules.
        let mut env = setup.env.clone();
        for name in ["lib", "deep"] {
            env.extra.push((
                format!(
                    "url.{}.insteadOf",
                    setup.root.join(format!("{name}.git")).display()
                ),
                format!("git@github.com:a/{name}.git"),
            ));
        }
        let fresh = setup.work.join("fresh");
        git(
            &setup.root,
            &[
                "clone",
                "-q",
                setup.work.to_str().unwrap(),
                fresh.to_str().unwrap(),
            ],
        );
        git(
            &fresh,
            &["remote", "set-url", "origin", "git@github.com:a/app.git"],
        );
        let mut request = request(&fresh, &["submodule", "update", "--init", "--recursive"]);
        request.project = Some(setup.work.clone());
        let update = plan(&request, &env).unwrap().unwrap();
        assert_eq!(update.details, ["lib: github.com/a/lib"]);
        assert_eq!(execute(update.job.as_ref().unwrap(), &request, &env), 0);
        assert_eq!(
            std::fs::read_to_string(fresh.join("lib/deep/file")).expect("the nested submodule"),
            "deep"
        );
    }

    /// git with git-lfs's filters (the tests' git has no settings of the user's).
    fn lfs_git(dir: &Path, args: &[&str]) {
        let mut all = vec![
            "-c",
            "filter.lfs.clean=git-lfs clean -- %f",
            "-c",
            "filter.lfs.smudge=git-lfs smudge -- %f",
            "-c",
            "filter.lfs.process=git-lfs filter-process",
            "-c",
            "filter.lfs.required=true",
        ];
        all.extend(args);
        git(dir, &all);
    }

    #[test]
    fn lfs_files_come_and_go_through_the_mirror() {
        if !super::super::relay::lfs_installed() {
            return;
        }
        let setup = repo("main", 0);
        // "GitHub" gets a file kept with LFS, from another clone.
        let other = setup.root.join("other");
        git(
            &setup.root,
            &["clone", "-q", setup.remote.to_str().unwrap(), "other"],
        );
        std::fs::write(
            other.join(".gitattributes"),
            "*.bin filter=lfs diff=lfs merge=lfs -text
",
        )
        .expect("write");
        std::fs::write(other.join("big.bin"), "their big file").expect("write");
        lfs_git(&other, &["add", "-A"]);
        lfs_git(&other, &["commit", "-q", "-m", "big"]);
        git(&other, &["push", "-q", "--no-verify", "origin", "main"]);
        let remote_url = format!("file://{}", setup.remote.display());
        git(&other, &["lfs", "push", "--all", &remote_url, "main"]);
        // Fetched without the file, then the file through the gate.
        let env = setup.env.clone();
        let pull = |args: &[&str]| {
            let mut request = request(&setup.work, args);
            request.project = Some(setup.work.clone());
            let plan = plan(&request, &env).unwrap().unwrap();
            execute(plan.job.as_ref().unwrap(), &request, &env)
        };
        assert_eq!(setup.run(&["fetch"]).1, 0);
        lfs_git(
            &setup.work,
            &[
                "-c",
                "filter.lfs.smudge=git-lfs smudge --skip -- %f",
                "-c",
                "filter.lfs.process=git-lfs filter-process --skip",
                "merge",
                "-q",
                "--ff-only",
                "origin/main",
            ],
        );
        assert_ne!(
            std::fs::read_to_string(setup.work.join("big.bin")).unwrap(),
            "their big file"
        );
        assert_eq!(
            pull(&[
                "-c",
                "filter.lfs.process=git-lfs filter-process",
                "-c",
                "filter.lfs.required=true",
                "lfs",
                "pull"
            ]),
            0
        );
        assert_eq!(
            std::fs::read_to_string(setup.work.join("big.bin")).unwrap(),
            "their big file"
        );
        // A new file goes up.
        std::fs::write(setup.work.join("mine.bin"), "my big file").expect("write");
        lfs_git(&setup.work, &["add", "mine.bin"]);
        lfs_git(&setup.work, &["commit", "-q", "-m", "mine"]);
        assert_eq!(pull(&["lfs", "push", "origin", "main"]), 0);
        let oid = {
            use sha2::Digest as _;
            super::super::relay::hex(&sha2::Sha256::digest(b"my big file"))
        };
        assert!(
            setup
                .remote
                .join(format!("lfs/objects/{}/{}/{oid}", &oid[..2], &oid[2..4]))
                .is_file(),
            "the file reached the remote"
        );
        // A repository that sends LFS files to another server: refused.
        git(&other, &["pull", "-q", "--no-rebase", "origin", "main"]);
        std::fs::write(
            other.join(".lfsconfig"),
            "[lfs]\n\turl = https://evil.example/lfs\n",
        )
        .expect("write");
        git(&other, &["add", ".lfsconfig"]);
        git(&other, &["commit", "-q", "-m", "elsewhere"]);
        git(&other, &["push", "-q", "--no-verify", "origin", "main"]);
        assert_ne!(setup.run(&["pull", "--no-rebase", "-q"]).1, 0);
        std::fs::write(setup.work.join("more.bin"), "more").expect("write");
        lfs_git(&setup.work, &["add", "more.bin"]);
        lfs_git(&setup.work, &["commit", "-q", "-m", "more"]);
        assert_ne!(pull(&["lfs", "push", "origin", "main"]), 0);
    }

    #[test]
    fn lfs_needs_a_named_branch() {
        let setup = repo("main", 0);
        let up = setup
            .plan(&["lfs", "push", "origin", "main"])
            .unwrap()
            .unwrap();
        assert_eq!(
            (up.title.as_str(), up.class),
            ("Upload LFS files to github.com/a/app", Class::Push)
        );
        assert!(setup.plan(&["lfs", "push", "origin", "nothing"]).is_err());
    }

    #[test]
    fn relative_submodule_addresses() {
        assert_eq!(
            relative_url("git@github.com:a/app.git", "../lib.git").as_deref(),
            Some("git@github.com:a/lib.git")
        );
        assert_eq!(
            relative_url("https://github.com/a/app", "../lib").as_deref(),
            Some("https://github.com/a/lib")
        );
        assert_eq!(
            relative_url("https://github.com/a/app", "./sub").as_deref(),
            Some("https://github.com/a/app/sub")
        );
    }
}
