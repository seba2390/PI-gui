//! git's commands that need the network and the user's keys: what each may be given, and what
//! the user is asked.

use std::path::{Path, PathBuf};

use super::config::{self, Repo, normalize_url, unsafe_url};
use super::{Class, Options, Plan, Request, inside, parse_args, project_of};

/// Options before the command that may come along (`git -C dir push`).
const GLOBAL_FLAGS: &[&str] = &[
    "--no-pager",
    "-P",
    "--no-optional-locks",
    "--no-replace-objects",
];

/// The plan for a `git` request, `None` when it is git's local work (run in the sandbox), or why
/// it is refused.
pub fn plan(request: &Request) -> Result<Option<Plan>, String> {
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
    if let Some(odd) = odd_globals.first() {
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
    let hooks = super::hooks_dir();
    let cx = Cx {
        request,
        dir: &dir,
        project: &project,
        hooks: &hooks,
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
    dir: &'a Path,
    project: &'a Path,
    hooks: &'a Path,
}

impl Cx<'_> {
    /// The repository the command is for, if its settings start no program.
    fn repo(&self, command: &str) -> Result<Repo, String> {
        let repo = Repo::find(self.dir).ok_or_else(|| {
            format!(
                "`git {command}`: {} is not in a git repository.",
                self.dir.display()
            )
        })?;
        let found = repo.unsafe_settings();
        if !found.is_empty() {
            return Err(format!(
                "Pistachio won't run `git {command}` outside the sandbox: this repository's git \
                 settings (.git/config) set {}, which could start programs with the user's keys. \
                 Tell the user; they can remove them (`git config --unset <key>`) or run the \
                 command themselves.",
                found.join(", ")
            ));
        }
        Ok(repo)
    }

    fn plan(&self, title: String, class: Class, target: String) -> Plan {
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
        }
    }

    fn git(&self, repo: &Repo, args: &[&str]) -> Option<String> {
        super::git_output(&repo.top, args, self.hooks)
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

/// A remote's name and address: `name` may be an address itself.
fn resolve_remote(repo: &Repo, name: &str, push: bool) -> Result<(String, String), String> {
    if name.contains(':') || name.contains('/') {
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
    Ok((name.to_owned(), url))
}

/// The branch a remote's `HEAD` points to (its default branch), if git knows it.
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

fn push(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &PUSH, "git push")?;
    let repo = cx.repo("push")?;
    let remote_arg = parsed.positional.first().cloned();
    let remote_name = remote_arg
        .clone()
        .unwrap_or_else(|| default_remote(&repo, true));
    let (remote, url) = resolve_remote(&repo, &remote_name, true)?;
    let target = normalize_url(&url);
    let current = repo.branch();
    let default = remote_default_branch(&repo, &remote);
    let is_default = |branch: &str| match &default {
        Some(default) => branch == default,
        None => matches!(branch, "main" | "master"),
    };
    let force = ["-f", "--force", "--force-with-lease", "--force-if-includes"]
        .iter()
        .any(|flag| {
            parsed
                .flags
                .iter()
                .any(|given| given == flag || given.starts_with(&format!("{flag}=")))
        });
    let delete = parsed.has("-d") || parsed.has("--delete");
    let all = parsed.has("--all") || parsed.has("--branches");
    let tags = parsed.has("--tags");
    let mut warnings = Vec::new();
    let mut rewrite = force || delete || parsed.has("--prune");
    // (local, remote) branch pairs.
    let mut pairs: Vec<(String, String)> = Vec::new();
    for spec in parsed.positional.iter().skip(1) {
        let (plus, spec) = match spec.strip_prefix('+') {
            Some(rest) => (true, rest),
            None => (false, spec.as_str()),
        };
        rewrite |= plus;
        let (source, destination) = match spec.split_once(':') {
            Some((source, destination)) => (source.to_owned(), destination.to_owned()),
            None => (spec.to_owned(), spec.to_owned()),
        };
        let short = |name: &str| {
            let name = if name == "HEAD" {
                current.clone().unwrap_or_else(|| name.to_owned())
            } else {
                name.to_owned()
            };
            name.strip_prefix("refs/heads/").unwrap_or(&name).to_owned()
        };
        if source.is_empty() {
            rewrite = true;
        }
        pairs.push((short(&source), short(&destination)));
    }
    if pairs.is_empty() && !all && !tags {
        let Some(branch) = current.clone() else {
            return Err(
                "`git push` without a branch needs a checked-out branch (HEAD is detached)."
                    .to_owned(),
            );
        };
        let upstream = repo
            .get("branch", Some(&branch), "merge")
            .and_then(|merge| merge.strip_prefix("refs/heads/").map(str::to_owned))
            .unwrap_or_else(|| branch.clone());
        pairs.push((branch, upstream));
    }
    let to_default = all || pairs.iter().any(|(_, destination)| is_default(destination));
    let class = if rewrite {
        Class::Rewrite
    } else if to_default {
        Class::PushDefault
    } else {
        Class::Push
    };
    let mut details = Vec::new();
    let title = if delete || pairs.iter().any(|(source, _)| source.is_empty()) {
        let names: Vec<&str> = pairs
            .iter()
            .map(|(_, destination)| destination.as_str())
            .collect();
        warnings.push("Deletes what is on the remote; it can't be undone from here.".to_owned());
        format!("Delete {} on {target}", names.join(", "))
    } else if all {
        format!("Push all branches to {target}")
    } else if pairs.is_empty() {
        format!("Push tags to {target}")
    } else if let [(source, destination)] = pairs.as_slice() {
        let range = format!("refs/remotes/{remote}/{destination}..{source}");
        let known = cx
            .git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/{remote}/{destination}"),
                ],
            )
            .is_some();
        if known {
            let count = cx
                .git(&repo, &["rev-list", "--count", &range])
                .unwrap_or_default();
            let count: usize = count.parse().unwrap_or(0);
            details.extend(
                cx.git(&repo, &["log", "--format=%s", "-n", "6", &range])
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned),
            );
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
        } else {
            format!("Push {source} to {target} as {destination} (new there)")
        }
    } else {
        let names: Vec<&str> = pairs.iter().map(|(source, _)| source.as_str()).collect();
        format!("Push {} to {target}", names.join(", "))
    };
    if tags && !pairs.is_empty() {
        details.push("And all tags.".to_owned());
    }
    if force {
        warnings.push(
            "Force push: replaces the remote's history, which can lose others' work.".to_owned(),
        );
    }
    if to_default && !delete {
        let default = default.clone().unwrap_or_else(|| "main".to_owned());
        warnings.push(format!("Goes straight to the default branch ({default})."));
    }
    if remote_arg
        .as_deref()
        .is_some_and(|name| name.contains(':') || name.contains('/'))
    {
        warnings.push("An address given directly, not one of the repository's remotes.".to_owned());
    }
    let mut plan = cx.plan(title, class, target);
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
            "Pistachio runs `git {command}` outside the sandbox without submodules; get them with \
             `git submodule update --init`."
        )),
    }
}

fn fetch(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &FETCH, "git fetch")?;
    no_submodules(&parsed, "fetch")?;
    let repo = cx.repo("fetch")?;
    let target = if parsed.has("--all") {
        let mut urls: Vec<String> = repo
            .entries()
            .into_iter()
            .filter(|entry| entry.section == "remote" && entry.name == "url")
            .map(|entry| normalize_url(&entry.value))
            .collect();
        urls.sort();
        urls.dedup();
        urls.join(", ")
    } else {
        let name = parsed
            .positional
            .first()
            .cloned()
            .unwrap_or_else(|| default_remote(&repo, false));
        normalize_url(&resolve_remote(&repo, &name, false)?.1)
    };
    Ok(cx.plan(format!("Fetch from {target}"), Class::Fetch, target))
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

/// Merge strategies git has itself (another name runs a program called `git-merge-<name>`).
const STRATEGIES: &[&str] = &["ort", "recursive", "resolve", "octopus", "ours", "subtree"];

fn pull(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &PULL, "git pull")?;
    no_submodules(&parsed, "pull")?;
    for option in ["-s", "--strategy"] {
        if let Some(strategy) = parsed.value(option)
            && !STRATEGIES.contains(&strategy)
        {
            return Err(format!(
                "Pistachio runs `git pull` outside the sandbox only with git's own merge strategies, not `{strategy}`."
            ));
        }
    }
    let repo = cx.repo("pull")?;
    let name = parsed
        .positional
        .first()
        .cloned()
        .unwrap_or_else(|| default_remote(&repo, false));
    let target = normalize_url(&resolve_remote(&repo, &name, false)?.1);
    let into = repo.branch().unwrap_or_else(|| "HEAD".to_owned());
    Ok(cx.plan(
        format!("Pull from {target} into {into}"),
        Class::Fetch,
        target,
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
    let mut plan = cx.plan(
        format!("Clone {target} into {}", shown(&destination, cx.project)),
        Class::Fetch,
        target,
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
        (Some(name), _) if name.contains(':') || name.contains('/') => {
            if unsafe_url(name) {
                return Err(format!("Pistachio won't use `{name}` outside the sandbox."));
            }
            name.clone()
        }
        (name, Some(_)) => {
            let repo = cx.repo("ls-remote")?;
            let name = name.clone().unwrap_or_else(|| default_remote(&repo, false));
            resolve_remote(&repo, &name, false)?.1
        }
        (_, None) => {
            return Err("`git ls-remote` needs an address outside a repository.".to_owned());
        }
    };
    let target = normalize_url(&url);
    Ok(cx.plan(
        format!("List the branches and tags of {target}"),
        Class::Fetch,
        target,
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

fn submodule_update(cx: &Cx<'_>, args: &[String]) -> Result<Plan, String> {
    let parsed = parse_args(args, &SUBMODULE, "git submodule update")?;
    let repo = cx.repo("submodule update")?;
    let modules = std::fs::read_to_string(repo.top.join(".gitmodules")).unwrap_or_default();
    let entries = config::parse(&modules);
    let origin = default_remote(&repo, false);
    let base = repo.get("remote", Some(&origin), "url");
    let mut urls = Vec::new();
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
        if !parsed.positional.is_empty()
            && !parsed
                .positional
                .iter()
                .any(|given| given.trim_end_matches('/') == path)
        {
            continue;
        }
        // The repository's own setting wins over .gitmodules (it was checked above).
        let url = repo
            .get("submodule", Some(name), "url")
            .unwrap_or_else(|| entry.value.clone());
        let url = if url.starts_with("./") || url.starts_with("../") {
            base.as_deref()
                .and_then(|base| relative_url(base, &url))
                .ok_or_else(|| format!("The submodule {name}'s address is relative, and this repository has no remote to resolve it against."))?
        } else {
            url
        };
        if unsafe_url(&url) {
            return Err(format!(
                "Pistachio won't get the submodule {name} outside the sandbox: its address ({url}) \
                 is a local folder or a helper program."
            ));
        }
        urls.push((path.to_owned(), url));
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
    let mut plan = cx.plan(
        format!("Get the submodules {} of {target}", names.join(", ")),
        Class::Fetch,
        target,
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
            "-q",
            "--quiet",
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
    let (remote, url) = resolve_remote(&repo, &name, push)?;
    let target = normalize_url(&url);
    // An LFS server named in the repository: it gets the files (and asks for the user's login).
    let lfs_config =
        config::parse(&std::fs::read_to_string(repo.top.join(".lfsconfig")).unwrap_or_default());
    let host = |address: &str| {
        normalize_url(address)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    let elsewhere = repo
        .entries()
        .into_iter()
        .chain(lfs_config)
        .filter(|entry| {
            (entry.section == "lfs" && matches!(entry.name.as_str(), "url" | "pushurl"))
                || (entry.section == "remote"
                    && entry.subsection.as_deref() == Some(remote.as_str())
                    && matches!(entry.name.as_str(), "lfsurl" | "lfspushurl"))
        })
        .map(|entry| entry.value)
        .find(|address| host(address) != host(&url));
    if let Some(address) = elsewhere {
        return Err(format!(
            "Pistachio won't run `git lfs {command}` outside the sandbox: the repository sends LFS \
             files to {address}, not to the remote's server."
        ));
    }
    let title = if push {
        format!("Upload LFS files to {target}")
    } else {
        format!("Download LFS files from {target}")
    };
    Ok(cx.plan(title, if push { Class::Push } else { Class::Fetch }, target))
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use pretty_assertions::assert_eq;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "a")
            .env("GIT_AUTHOR_EMAIL", "a@b")
            .env("GIT_COMMITTER_NAME", "a")
            .env("GIT_COMMITTER_EMAIL", "a@b")
            .output()
            .expect("git");
        assert!(
            status.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// A repository with a remote (a bare one standing in for GitHub) that knows `main`, and
    /// `count` commits on `branch` that it doesn't.
    fn repo(branch: &str, count: usize) -> (tempfile::TempDir, PathBuf) {
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
        git(&work, &["push", "-q", "origin", "main"]);
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
        (dir, work)
    }

    fn request(work: &Path, args: &[&str]) -> Request {
        Request {
            tool: "git".into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cwd: work.to_path_buf(),
            project: Some(work.to_path_buf()),
            stdin: None,
        }
    }

    #[test]
    fn local_work_stays_in_the_sandbox() {
        let (_dir, work) = repo("main", 0);
        for args in [
            &["status"][..],
            &["commit", "-m", "x"],
            &["log"],
            &["submodule", "status"],
            &["lfs", "install"],
            &["-c", "x=y", "status"],
        ] {
            assert_eq!(plan(&request(&work, args)), Ok(None), "{args:?}");
        }
    }

    #[test]
    fn a_push_says_what_goes_where() {
        let (_dir, work) = repo("feature", 2);
        let push = plan(&request(&work, &["push"]))
            .expect("planned")
            .expect("gated");
        assert_eq!(push.title, "Push 2 commits on feature to github.com/a/app");
        assert_eq!(push.details, ["change 1", "change 0"]);
        assert_eq!(push.class, Class::Push);
        assert_eq!(push.target, "github.com/a/app");
        assert!(push.warnings.is_empty(), "{:?}", push.warnings);

        let new = plan(&request(&work, &["push", "-u", "origin", "HEAD:topic"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            new.title,
            "Push feature to github.com/a/app as topic (new there)"
        );

        let main = plan(&request(&work, &["push", "origin", "feature:main"]))
            .unwrap()
            .unwrap();
        assert_eq!(main.class, Class::PushDefault);
        assert!(main.warnings[0].contains("default branch (main)"));

        let force = plan(&request(
            &work,
            &["push", "--force-with-lease", "origin", "feature"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(force.class, Class::Rewrite);
        let plus = plan(&request(&work, &["push", "origin", "+feature"]))
            .unwrap()
            .unwrap();
        assert_eq!(plus.class, Class::Rewrite);

        let delete = plan(&request(&work, &["push", "origin", "--delete", "old"]))
            .unwrap()
            .unwrap();
        assert_eq!(delete.class, Class::Rewrite);
        assert_eq!(delete.title, "Delete old on github.com/a/app");
        let colon = plan(&request(&work, &["push", "origin", ":old"]))
            .unwrap()
            .unwrap();
        assert_eq!(colon.class, Class::Rewrite);

        let tags = plan(&request(&work, &["push", "--tags"])).unwrap().unwrap();
        assert_eq!(tags.title, "Push tags to github.com/a/app");
    }

    #[test]
    fn options_and_settings_that_run_programs_are_refused() {
        let (_dir, work) = repo("main", 1);
        for args in [
            &["push", "--receive-pack=evil", "origin"][..],
            &["push", "--exec=evil"],
            &["-c", "core.sshCommand=evil", "push"],
            &["fetch", "--upload-pack=evil"],
            &["fetch", "--recurse-submodules"],
            &["pull", "-s", "evil"],
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
            assert!(plan(&request(&work, args)).is_err(), "{args:?}");
        }
        git(&work, &["config", "core.sshCommand", "evil"]);
        let refused = plan(&request(&work, &["push"])).unwrap_err();
        assert!(refused.contains("core.sshcommand"), "{refused}");
        // Its local work still runs (in the sandbox).
        assert_eq!(plan(&request(&work, &["status"])), Ok(None));
    }

    #[test]
    fn only_in_the_session_folder() {
        let (dir, work) = repo("main", 0);
        let mut outside = request(&work, &["push"]);
        outside.cwd = dir.path().to_path_buf();
        assert!(plan(&outside).is_err());
        let escape = request(&work, &["clone", "https://github.com/a/b", "../b"]);
        assert!(plan(&escape).unwrap_err().contains("session's folder"));
    }

    #[test]
    fn fetch_pull_clone_and_listing() {
        let (_dir, work) = repo("main", 0);
        let fetch = plan(&request(&work, &["fetch", "--prune"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (fetch.title.as_str(), fetch.class),
            ("Fetch from github.com/a/app", Class::Fetch)
        );
        let pull = plan(&request(&work, &["pull", "--rebase"]))
            .unwrap()
            .unwrap();
        assert_eq!(pull.title, "Pull from github.com/a/app into main");
        let clone = plan(&request(
            &work,
            &["clone", "--depth", "1", "https://github.com/b/lib.git"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(clone.title, "Clone github.com/b/lib into lib");
        assert_eq!(clone.target, "github.com/b/lib");
        let listing = plan(&request(
            &work,
            &["ls-remote", "--tags", "https://gitlab.com/c/d"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(listing.target, "gitlab.com/c/d");
    }

    #[test]
    fn submodules_are_listed_with_their_addresses() {
        let (_dir, work) = repo("main", 0);
        std::fs::write(
            work.join(".gitmodules"),
            "[submodule \"lib\"]\n\tpath = vendor/lib\n\turl = ../lib.git\n[submodule \"x\"]\n\tpath = x\n\turl = https://gitlab.com/c/x\n",
        )
        .expect("write");
        let update = plan(&request(
            &work,
            &["submodule", "update", "--init", "--recursive"],
        ))
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
            work.join(".gitmodules"),
            "[submodule \"e\"]\n\tpath = e\n\turl = /Users/a/secret\n",
        )
        .expect("write");
        assert!(plan(&request(&work, &["submodule", "update", "--init"])).is_err());
    }

    #[test]
    fn lfs_sends_files_only_to_the_remote_server() {
        let (_dir, work) = repo("main", 0);
        let up = plan(&request(&work, &["lfs", "push", "origin", "main"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (up.title.as_str(), up.class),
            ("Upload LFS files to github.com/a/app", Class::Push)
        );
        std::fs::write(
            work.join(".lfsconfig"),
            "[lfs]\n\turl = https://evil.example/lfs\n",
        )
        .expect("write");
        assert!(
            plan(&request(&work, &["lfs", "push", "origin", "main"]))
                .unwrap_err()
                .contains("evil.example")
        );
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
