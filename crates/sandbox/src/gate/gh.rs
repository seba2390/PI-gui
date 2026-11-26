//! GitHub's `gh` for one repository: reading pull requests, issues and runs, and opening,
//! commenting on, closing or merging them, and creating a repository. Its login stays outside
//! the sandbox; `gh api`, logins, secrets and the like are not run.
//!
//! With several accounts on a host (a personal one and a company one), the user picks which one
//! each command acts as; the gate runs `gh` with that account's token for that command only
//! (`GH_TOKEN`), without switching the active account. A repository created for an owner is
//! offered only as accounts that are that owner or members of it.

use std::path::Path;

use super::config::{Repo, normalize_url};
use super::{Class, Plan, Request, inside, project_of};

/// What `gh <group> <command>` may do: read, or change things on GitHub.
fn class_of(group: &str, command: &str) -> Option<Class> {
    let read = matches!(
        (group, command),
        ("pr", "view" | "list" | "status" | "checks" | "diff")
            | ("issue", "view" | "list" | "status")
            | ("run", "list" | "view" | "watch")
            | ("repo", "view")
            | ("release" | "workflow", "list" | "view")
            | ("label", "list")
    );
    let write = matches!(
        (group, command),
        (
            "pr",
            "create" | "comment" | "edit" | "ready" | "close" | "reopen" | "merge" | "review"
        ) | ("issue", "create" | "comment" | "edit" | "close" | "reopen")
            | ("repo", "create")
            | ("run", "rerun" | "cancel")
            | ("workflow", "run")
    );
    if read {
        Some(Class::GhRead)
    } else if write {
        Some(Class::GhWrite)
    } else {
        None
    }
}

/// Options whose value is a file `gh` reads (and would send to GitHub).
const FILE_OPTIONS: &[&str] = &["--body-file", "-F"];

/// The accounts `gh` is logged in to, and the organisations each belongs to.
pub trait Accounts {
    /// The logins on `host`, the active one first.
    fn logins(&self, host: &str) -> Vec<String>;
    /// The organisations `login` on `host` is a member of.
    fn orgs(&self, host: &str, login: &str) -> Vec<String>;
}

/// The accounts of the `gh` on this computer.
pub struct Installed;

impl Accounts for Installed {
    fn logins(&self, host: &str) -> Vec<String> {
        let Ok(output) = gh_command()
            .args(["auth", "status", "--json", "hosts", "--hostname", host])
            .output()
        else {
            return Vec::new();
        };
        let Ok(status) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
            return Vec::new();
        };
        let mut accounts: Vec<(bool, String)> =
            status["hosts"][host].as_array().map_or_default(|list| {
                list.iter()
                    .filter(|account| account["state"].as_str() == Some("success"))
                    .filter_map(|account| {
                        let login = account["login"].as_str()?.to_owned();
                        Some((account["active"].as_bool() != Some(true), login))
                    })
                    .collect()
            });
        accounts.sort();
        accounts.into_iter().map(|(_, login)| login).collect()
    }

    fn orgs(&self, host: &str, login: &str) -> Vec<String> {
        let Some(token) = token(host, login) else {
            return Vec::new();
        };
        let mut command = gh_command();
        command.args(["api", "--hostname", host, "user/orgs", "--jq", ".[].login"]);
        if host == "github.com" {
            command.env("GH_TOKEN", token);
        } else {
            command.env("GH_ENTERPRISE_TOKEN", token);
        }
        command.output().map_or_else(
            |_| Vec::new(),
            |output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            },
        )
    }
}

/// `gh` outside any repository, without prompts.
fn gh_command() -> std::process::Command {
    let mut command = std::process::Command::new("gh");
    command
        .current_dir(std::env::temp_dir())
        .env("GH_PROMPT_DISABLED", "1")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .stdin(std::process::Stdio::null());
    command
}

/// The token of `login` on `host`, from gh's own store.
pub fn token(host: &str, login: &str) -> Option<String> {
    let output = gh_command()
        .args(["auth", "token", "--hostname", host, "--user", login])
        .output()
        .ok()?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !token.is_empty()).then_some(token)
}

/// The plan for a `gh` request, `None` for help and version (run in the sandbox), or why it is
/// refused.
pub fn plan(request: &Request) -> Result<Option<Plan>, String> {
    plan_with(request, &Installed)
}

/// [`plan`], with the accounts from `accounts`.
pub fn plan_with(request: &Request, accounts: &dyn Accounts) -> Result<Option<Plan>, String> {
    let args = &request.args;
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version"))
        || matches!(args[0].as_str(), "help" | "version" | "completion")
    {
        return Ok(None);
    }
    let mut repo_option = None;
    let mut words = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-R" || arg == "--repo" {
            repo_option = iter.next().cloned();
        } else if let Some(value) = arg.strip_prefix("--repo=") {
            repo_option = Some(value.to_owned());
        } else if !arg.starts_with('-') && words.len() < 2 {
            words.push(arg.clone());
        } else if FILE_OPTIONS.contains(&arg.as_str())
            || FILE_OPTIONS
                .iter()
                .any(|option| arg.starts_with(&format!("{option}=")))
        {
            let value = match arg.split_once('=') {
                Some((_, value)) => Some(value.to_owned()),
                None => iter.next().cloned(),
            };
            check_file(value.as_deref(), request)?;
        }
    }
    let (group, command) =
        match words.as_slice() {
            [group, command, ..] => (group.as_str(), command.as_str()),
            _ => return Err(
                "Pistachio runs `gh` outside the sandbox only for a command like `gh pr create`."
                    .to_owned(),
            ),
        };
    let Some(class) = class_of(group, command) else {
        return Err(format!(
            "Pistachio doesn't run `gh {group} {command}` outside the sandbox. It runs reading and \
             changing pull requests, issues, runs and releases, and creating a repository; ask the \
             user for anything else."
        ));
    };
    let project = project_of(request);
    if !inside(&request.cwd, &project) {
        return Err(format!(
            "Pistachio runs `gh` outside the sandbox only in the session's folder ({}).",
            project.display()
        ));
    }
    if (group, command) == ("repo", "create") {
        return repo_create(request, &project, accounts).map(Some);
    }
    // gh reads the repository's remotes with git, so its settings must be safe too.
    let repo = Repo::find(&request.cwd);
    if let Some(repo) = &repo {
        let found = repo.unsafe_settings();
        if !found.is_empty() {
            return Err(format!(
                "Pistachio won't run `gh` outside the sandbox: this repository's git settings set {}.",
                found.join(", ")
            ));
        }
    }
    let target = match repo_option {
        Some(name) if name.matches('/').count() >= 2 => name.to_lowercase(),
        Some(name) => format!("github.com/{name}"),
        None => repo
            .as_ref()
            .and_then(|repo| {
                repo.get("remote", Some("upstream"), "url")
                    .or_else(|| repo.get("remote", Some("origin"), "url"))
            })
            .map(|url| normalize_url(&url))
            .ok_or_else(|| {
                "`gh` needs a GitHub repository: run it in a clone, or name one with -R owner/name."
                    .to_owned()
            })?,
    };
    let title = match (group, command) {
        ("pr", "create") => format!("Open a pull request on {target}"),
        ("pr", "merge") => format!("Merge a pull request on {target}"),
        ("pr" | "issue", "comment") => format!("Comment on {target}"),
        ("pr", "close") => format!("Close a pull request on {target}"),
        ("issue", "create") => format!("Open an issue on {target}"),
        ("issue", "close") => format!("Close an issue on {target}"),
        ("run", "rerun") => format!("Run a workflow again on {target}"),
        ("workflow", "run") => format!("Start a workflow on {target}"),
        _ if class == Class::GhRead => format!("Read {group}s on {target} (gh {group} {command})"),
        _ => format!("gh {group} {command} on {target}"),
    };
    let mut plan = gh_plan(request, title, class, target);
    if let Some(title) = value_of(args, &["--title", "-t"]) {
        plan.details.push(format!("Title: {title}"));
    }
    if (group, command) == ("pr", "merge") {
        plan.warnings
            .push("Merges into the pull request's base branch on GitHub.".to_owned());
    }
    let host = plan
        .target
        .split('/')
        .next()
        .unwrap_or("github.com")
        .to_owned();
    plan.accounts = logged_in(accounts, &host)?;
    name_the_account(&mut plan);
    Ok(plan.into())
}

/// The accounts on `host`, or why there are none.
fn logged_in(accounts: &dyn Accounts, host: &str) -> Result<Vec<String>, String> {
    let logins = accounts.logins(host);
    if logins.is_empty() {
        return Err(format!(
            "gh isn't logged in to {host} on this computer; ask the user to run `gh auth login`."
        ));
    }
    Ok(logins)
}

/// With one account, the question says which (with several, each answer does).
fn name_the_account(plan: &mut Plan) {
    if let [account] = plan.accounts.as_slice() {
        plan.title = format!("{} as {account}", plan.title);
    }
}

fn gh_plan(request: &Request, title: String, class: Class, target: String) -> Plan {
    Plan {
        title,
        details: Vec::new(),
        warnings: Vec::new(),
        class,
        target,
        rememberable: true,
        program: "gh".to_owned(),
        args: request.args.clone(),
        cwd: request.cwd.clone(),
        stdin: None,
        accounts: Vec::new(),
        account: None,
    }
}

/// `gh repo create`: a new repository, asked about each time, as an account that may create it
/// for its owner.
fn repo_create(request: &Request, project: &Path, accounts: &dyn Accounts) -> Result<Plan, String> {
    let args = &request.args;
    if let Some(option) = args.iter().find(|arg| {
        matches!(arg.as_str(), "--push" | "--clone" | "-c")
            || arg.starts_with("--push=")
            || arg.starts_with("--clone=")
    }) {
        return Err(format!(
            "Pistachio runs `gh repo create` outside the sandbox without `{option}`: create it, then \
             `git push` (which asks the user separately)."
        ));
    }
    if let Some(source) = value_of(args, &["--source", "-s"])
        && !inside(&request.cwd.join(&source), project)
    {
        return Err(
            "`gh repo create --source` must be the session's folder or inside it.".to_owned(),
        );
    }
    let name = args
        .iter()
        .skip_while(|arg| *arg != "create")
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .cloned()
        .unwrap_or_else(|| {
            project.file_name().map_or_else(
                || "repository".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        });
    let visibility = if args.iter().any(|arg| arg == "--public") {
        "public"
    } else if args.iter().any(|arg| arg == "--internal") {
        "internal"
    } else {
        "private"
    };
    let target = format!("github.com/{name}");
    let mut plan = gh_plan(
        request,
        format!("Create the {visibility} repository {name} on GitHub"),
        Class::GhWrite,
        target,
    );
    plan.rememberable = false;
    if visibility == "public" {
        plan.warnings.push("Public: anyone can see it.".to_owned());
    }
    if let Some(remote) = value_of(args, &["--remote", "-r"]) {
        plan.details
            .push(format!("Adds it as the remote {remote}."));
    }
    // Who may create it: for `owner/name`, that owner or its members; else any account (the
    // repository is then that account's).
    let logins = logged_in(accounts, "github.com")?;
    plan.accounts = match name.split_once('/') {
        Some((owner, _)) => {
            let owner = owner.to_lowercase();
            let allowed: Vec<String> = logins
                .iter()
                .filter(|login| {
                    login.to_lowercase() == owner
                        || accounts
                            .orgs("github.com", login)
                            .iter()
                            .any(|org| org.to_lowercase() == owner)
                })
                .cloned()
                .collect();
            if allowed.is_empty() {
                return Err(format!(
                    "None of the user's GitHub accounts ({}) can create repositories in {owner}: \
                     none of them is {owner} or a member of it. Ask the user.",
                    logins.join(", ")
                ));
            }
            allowed
        }
        None => {
            for login in &logins {
                plan.details
                    .push(format!("As {login}: github.com/{login}/{name}"));
            }
            logins
        }
    };
    name_the_account(&mut plan);
    Ok(plan)
}

/// A file `gh` would read and send: only one in the session's folder.
fn check_file(value: Option<&str>, request: &Request) -> Result<(), String> {
    let project = project_of(request);
    match value {
        Some(path) if path != "-" && inside(&request.cwd.join(path), &project) => Ok(()),
        _ => Err(format!(
            "Pistachio runs `gh` outside the sandbox only with a body file in the session's folder \
             ({}); write the text there, or give it with --body.",
            project.display()
        )),
    }
}

/// The value of the first of `options` given.
fn value_of(args: &[String], options: &[&str]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        for option in options {
            if arg == option {
                return iter.next().cloned();
            }
            if let Some(value) = arg.strip_prefix(&format!("{option}=")) {
                return Some(value.to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Two accounts; the company one is a member of `acme`.
    struct Fake;

    impl Accounts for Fake {
        fn logins(&self, _host: &str) -> Vec<String> {
            vec!["me".into(), "me-at-work".into()]
        }
        fn orgs(&self, _host: &str, login: &str) -> Vec<String> {
            if login == "me-at-work" {
                vec!["Acme".into()]
            } else {
                Vec::new()
            }
        }
    }

    /// One account.
    struct Single;

    impl Accounts for Single {
        fn logins(&self, _host: &str) -> Vec<String> {
            vec!["me".into()]
        }
        fn orgs(&self, _host: &str, _login: &str) -> Vec<String> {
            Vec::new()
        }
    }

    fn plan(request: &Request) -> Result<Option<Plan>, String> {
        plan_with(request, &Fake)
    }

    fn request(cwd: &Path, args: &[&str]) -> Request {
        Request {
            tool: "gh".into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cwd: cwd.to_path_buf(),
            project: Some(cwd.to_path_buf()),
            stdin: None,
            account: None,
        }
    }

    fn work() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let work = dir.path().canonicalize().expect("canonical");
        std::fs::create_dir_all(work.join(".git")).expect("mkdir");
        std::fs::write(work.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write");
        std::fs::write(
            work.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:a/app.git\n",
        )
        .expect("write");
        (dir, work)
    }

    #[test]
    fn reading_and_changing_on_github() {
        let (_dir, work) = work();
        let view = plan(&request(&work, &["pr", "view", "12"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (view.class, view.target.as_str()),
            (Class::GhRead, "github.com/a/app")
        );
        let create = plan(&request(
            &work,
            &["pr", "create", "--title", "Fix it", "--body", "x"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(create.title, "Open a pull request on github.com/a/app");
        assert_eq!(create.details, ["Title: Fix it"]);
        assert_eq!(create.class, Class::GhWrite);
        let other = plan(&request(&work, &["-R", "b/lib", "issue", "list"]))
            .unwrap()
            .unwrap();
        assert_eq!(other.target, "github.com/b/lib");
        assert_eq!(plan(&request(&work, &["--version"])), Ok(None));
    }

    #[test]
    fn logins_api_and_files_elsewhere_are_refused() {
        let (_dir, work) = work();
        for args in [
            &["auth", "token"][..],
            &["api", "user"],
            &["secret", "list"],
            &["pr", "create", "--body-file", "/Users/a/.ssh/id_rsa"],
            &["issue", "create", "-F", "-"],
            &["repo", "create", "x", "--push"],
            &["repo", "create", "x", "--clone"],
        ] {
            assert!(plan(&request(&work, args)).is_err(), "{args:?}");
        }
        assert!(
            plan(&request(
                &work,
                &["pr", "create", "--body-file", "notes.md"]
            ))
            .is_ok()
        );
    }

    #[test]
    fn creating_a_repository_is_asked_each_time() {
        let (_dir, work) = work();
        let create = plan(&request(
            &work,
            &[
                "repo",
                "create",
                "tool",
                "--public",
                "--source=.",
                "--remote=origin",
            ],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(create.title, "Create the public repository tool on GitHub");
        assert!(!create.rememberable);
        assert_eq!(create.warnings, ["Public: anyone can see it."]);
        assert_eq!(create.accounts, ["me", "me-at-work"]);
        assert!(
            create
                .details
                .contains(&"As me-at-work: github.com/me-at-work/tool".to_owned())
        );
    }

    #[test]
    fn a_company_repository_only_as_a_member() {
        let (_dir, work) = work();
        let create = plan(&request(
            &work,
            &["repo", "create", "acme/tool", "--private"],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(create.accounts, ["me-at-work"]);
        assert_eq!(
            create.title,
            "Create the private repository acme/tool on GitHub as me-at-work"
        );
        let refused = plan(&request(
            &work,
            &["repo", "create", "other-co/tool", "--private"],
        ))
        .unwrap_err();
        assert!(refused.contains("me, me-at-work"), "{refused}");
    }

    #[test]
    fn the_account_is_named_or_chosen() {
        let (_dir, work) = work();
        let both = plan(&request(&work, &["pr", "list"])).unwrap().unwrap();
        assert_eq!(both.accounts, ["me", "me-at-work"]);
        assert_eq!(both.title, "Read prs on github.com/a/app (gh pr list)");
        let one = plan_with(&request(&work, &["pr", "list"]), &Single)
            .unwrap()
            .unwrap();
        assert_eq!(one.title, "Read prs on github.com/a/app (gh pr list) as me");
    }
}
