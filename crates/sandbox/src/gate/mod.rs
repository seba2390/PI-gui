//! The gate: what sandboxed commands may have done outside the sandbox, with the user's keys,
//! once the user says yes (Settings → Sandboxing & Security → Git and GitHub).
//!
//! In the sandbox, `git`, `gh`, `gpg` and `ssh-keygen` are small wrappers ([`crate::cli`]'s
//! `shim`). Most of what they are asked runs in the sandbox as usual. What needs the user's keys
//! goes to the gate instead, over a socket the sandbox extension listens on:
//!
//! - git's network commands: `clone`, `fetch`, `pull`, `push` (with tags, deletes and force
//!   pushes, which are pointed out), `ls-remote`, `submodule update` and git-lfs's `fetch`,
//!   `pull` and `push` ([`git`]);
//! - GitHub's `gh` for a repository: reading pull requests, issues and runs, and opening or
//!   changing them ([`gh`]);
//! - signing a commit or tag with the user's GPG or SSH key ([`sign`]).
//!
//! The gate reads the request itself ([`plan`]): only these commands and a list of their options,
//! in repositories whose settings start no program ([`config`]), with hooks off. It says exactly
//! what will happen (the commits, the branch, the remote's real address), and the extension asks
//! the user: once, always for this project and remote ([`Rule`]), or not. The keys never enter
//! the sandbox: only what the command prints comes back.

pub mod config;
pub mod gh;
pub mod git;
pub mod relay;
pub mod shim;
pub mod sign;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// A wrapper's request, as the extension passes it on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// `git`, `gh`, `gpg` or `ssh-keygen`.
    pub tool: String,
    pub args: Vec<String>,
    /// Where the command ran.
    pub cwd: PathBuf,
    /// The session's folder (set by the extension, not the sandbox).
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// What the command was given on standard input (what to sign).
    #[serde(default)]
    pub stdin: Option<String>,
    /// The `gh` account the user chose (set by the extension).
    #[serde(default)]
    pub account: Option<String>,
    /// The plan the user allowed (set by the extension): what runs must still be that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<Box<Plan>>,
}

/// What kind of thing a request does; the user allows each kind separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Getting from a remote: clone, fetch, pull, `ls-remote`, submodules, LFS downloads.
    Fetch,
    /// Pushing a branch other than the default one, or tags.
    Push,
    /// Pushing to the remote's default branch (`main`).
    PushDefault,
    /// Force pushes and deleting what is on the remote.
    Rewrite,
    /// Signing commits and tags with the user's key.
    Sign,
    /// Reading from GitHub: pull requests, issues, runs.
    GhRead,
    /// Changing things on GitHub: opening, commenting on, closing or merging pull requests and
    /// issues, creating and deleting repositories (those two never without asking).
    GhWrite,
    /// Reaching a website from a command (asked by the proxy, [`crate::proxy`]).
    Web,
}

impl Class {
    pub const ALL: [Self; 8] = [
        Self::Fetch,
        Self::Push,
        Self::PushDefault,
        Self::Rewrite,
        Self::Sign,
        Self::GhRead,
        Self::GhWrite,
        Self::Web,
    ];

    /// What the user allows, as the settings say it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch and pull",
            Self::Push => "Push",
            Self::PushDefault => "Push to the default branch",
            Self::Rewrite => "Force push and delete",
            Self::Sign => "Sign",
            Self::GhRead => "Read on GitHub",
            Self::GhWrite => "Change on GitHub",
            Self::Web => "Reach the website",
        }
    }
}

/// What the user allowed without asking: in a project, for a remote or key, these kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// The project folder (`~/…` in the home folder).
    pub project: String,
    /// The remote's address as [`config::normalize_url`] gives it, or the signing key.
    pub target: String,
    pub allow: Vec<Class>,
    /// For GitHub (`gh`): the account it acts as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// What the gate will do for a request, once the user agrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// One line: "Push 3 commits on 0.5.5 to github.com/a/b".
    pub title: String,
    /// What it is about: the commits, the addresses.
    pub details: Vec<String>,
    /// What needs care: a force push, the default branch.
    pub warnings: Vec<String>,
    pub class: Class,
    /// The remote, repository or key ([`Rule::target`]).
    pub target: String,
    /// Whether "always for this project" can be offered (not for one-offs like creating a
    /// repository).
    pub rememberable: bool,
    /// What runs, outside the sandbox.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub stdin: Option<String>,
    /// For `gh`: the accounts it may act as (the active one first); the user picks one.
    #[serde(default)]
    pub accounts: Vec<String>,
    /// The account a rule chose.
    #[serde(default)]
    pub account: Option<String>,
    /// For git: how it is done through the gate's mirrors ([`relay`]).
    #[serde(skip)]
    pub job: Option<git::Job>,
    /// What follows in the sandbox once the command succeeded.
    #[serde(skip)]
    pub after: Option<After>,
}

/// What follows a command that succeeded, in the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After {
    /// `gh repo create --source`: the new repository becomes the folder's remote `remote`.
    /// `name` is `owner/name`, or `name` for the account that created it.
    AddRemote {
        dir: PathBuf,
        remote: String,
        name: String,
    },
}

impl Plan {
    /// Whether `other` says the same to the user (what they allowed).
    pub fn same_question(&self, other: &Self) -> bool {
        self.title == other.title
            && self.details == other.details
            && self.warnings == other.warnings
            && self.class == other.class
            && self.target == other.target
    }
}

/// The gate's answer to a wrapper: run it in the sandbox as usual, ask the user, or refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Decision {
    /// Not something the gate does: the wrapper runs the real program in the sandbox.
    Local,
    /// Needs the user's keys; `approved` when a rule already allows it.
    Gate { plan: Box<Plan>, approved: bool },
    /// Needs the user's keys but can't be done safely; why, for the model and the user.
    Refused { reason: String },
    /// Answered by the gate itself, without asking (`gh auth status`): what to print.
    Answer { text: String },
}

/// What `request` needs, with `settings`' rules.
pub fn decide(request: &Request, settings: &Settings, home: Option<&Path>) -> Decision {
    decide_with(request, settings, home, &relay::Env::current())
}

/// [`decide`], with the gate's git in `env`.
pub fn decide_with(
    request: &Request,
    settings: &Settings,
    home: Option<&Path>,
    env: &relay::Env,
) -> Decision {
    if request.tool == "gh" && gh::is_auth_status(&request.args) {
        return Decision::Answer {
            text: gh::auth_status(&gh::Installed, git_identity(request, env)),
        };
    }
    let planned = match request.tool.as_str() {
        "git" => git::plan(request, env),
        "gh" => gh::plan(request, env),
        "gpg" | "ssh-keygen" => sign::plan(request),
        "site" => Ok(site_plan(request)),
        _ => Ok(None),
    };
    match planned {
        Ok(None) => Decision::Local,
        Ok(Some(mut plan)) => {
            let rule = matching_rule(&plan, request, settings, home);
            let approved = rule.is_some();
            if let Some(rule) = rule {
                plan.account.clone_from(&rule.account);
            }
            Decision::Gate {
                plan: Box::new(plan),
                approved,
            }
        }
        Err(reason) => Decision::Refused { reason },
    }
}

/// The `user.name` and `user.email` git uses in the request's folder (the session's folder when it
/// ran elsewhere), read outside the sandbox.
fn git_identity(request: &Request, env: &relay::Env) -> (Option<String>, Option<String>) {
    let project = project_of(request);
    let folder = if inside(&request.cwd, &project) {
        request.cwd.clone()
    } else {
        project
    };
    // Read in the sandbox: the repository's settings are the agent's.
    let get = |key: &str| {
        env.sandboxed_output(
            &project_of(request),
            &folder,
            &["config", "--get", key],
            None,
        )
        .filter(|value| !value.is_empty())
    };
    (get("user.name"), get("user.email"))
}

/// A command reaching a website the user hasn't allowed (`args`: the host and port, from the
/// proxy): nothing runs, the answer is the point. "Always" is for the site in this project.
fn site_plan(request: &Request) -> Option<Plan> {
    let host = request.args.first()?.to_lowercase();
    let port = request.args.get(1).map_or("443", String::as_str);
    let shown = if matches!(port, "443" | "80") {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    Some(Plan {
        title: format!("Let commands reach {shown}"),
        details: vec![
            "For this conversation: downloads, installs and requests to this site.".to_owned(),
            "Other sites are asked about separately.".to_owned(),
        ],
        warnings: Vec::new(),
        class: Class::Web,
        target: host,
        rememberable: true,
        program: String::new(),
        args: Vec::new(),
        cwd: request.cwd.clone(),
        stdin: None,
        accounts: Vec::new(),
        account: None,
        job: None,
        after: None,
    })
}

/// The project a request belongs to: the session's folder, else where it ran.
fn project_of(request: &Request) -> PathBuf {
    let project = request
        .project
        .clone()
        .unwrap_or_else(|| request.cwd.clone());
    crate::paths::resolve(&project)
}

/// The rule that allows `plan` without asking, if any. For `gh` it must name an account that
/// is still logged in.
pub fn matching_rule<'a>(
    plan: &Plan,
    request: &Request,
    settings: &'a Settings,
    home: Option<&Path>,
) -> Option<&'a Rule> {
    let project = project_of(request);
    if !plan.rememberable {
        return None;
    }
    settings.gate.iter().find(|rule| {
        let folder = crate::paths::resolve(&crate::paths::expand(&rule.project, home));
        let account_ok = plan.accounts.is_empty()
            || rule
                .account
                .as_ref()
                .is_some_and(|account| plan.accounts.contains(account));
        project == folder
            && rule.target == plan.target
            && rule.allow.contains(&plan.class)
            && account_ok
    })
}

/// Whether a rule allows `plan` without asking.
pub fn allowed_by_rules(
    plan: &Plan,
    request: &Request,
    settings: &Settings,
    home: Option<&Path>,
) -> bool {
    matching_rule(plan, request, settings, home).is_some()
}

/// Adds `class` for `target` in `project` (as `account`, for `gh`) to the rules.
pub fn allow(
    settings: &mut Settings,
    project: &Path,
    target: &str,
    class: Class,
    account: Option<&str>,
) {
    let project = crate::paths::tilde(project);
    match settings.gate.iter_mut().find(|rule| {
        rule.project == project && rule.target == target && rule.account.as_deref() == account
    }) {
        Some(rule) => {
            if !rule.allow.contains(&class) {
                rule.allow.push(class);
            }
        }
        None => settings.gate.push(Rule {
            project,
            target: target.to_owned(),
            allow: vec![class],
            account: account.map(str::to_owned),
        }),
    }
}

/// The environment of what the gate runs: hooks off (from `hooks`, an empty folder), no program
/// a repository could name, no prompts on a terminal nobody sees, then `extra` settings. As
/// environment variables, so the git that `gh` or git-lfs start keeps to them too.
pub fn hardened_env(hooks: &Path, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut settings = vec![
        ("core.hooksPath", hooks.to_string_lossy().into_owned()),
        ("core.fsmonitor", "false".to_owned()),
        ("protocol.ext.allow", "never".to_owned()),
        ("protocol.file.allow", "never".to_owned()),
        ("submodule.recurse", "false".to_owned()),
        // A folder that is a bare repository is not taken for one (only with `GIT_DIR`).
        ("safe.bareRepository", "explicit".to_owned()),
    ];
    settings.extend(
        extra
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone())),
    );
    let mut env = vec![("GIT_CONFIG_COUNT".to_owned(), settings.len().to_string())];
    for (index, (key, value)) in settings.into_iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{index}"), key.to_owned()));
        env.push((format!("GIT_CONFIG_VALUE_{index}"), value));
    }
    env.extend(
        [
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_PROTOCOL_FROM_USER", "0"),
            ("GH_PROMPT_DISABLED", "1"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned())),
    );
    env
}

/// Does `after` in the sandbox, once the command succeeded.
pub fn after(after: &After, plan: &Plan, request: &Request, env: &relay::Env) {
    match after {
        After::AddRemote { dir, remote, name } => {
            let full = if name.contains('/') {
                name.clone()
            } else {
                let account = request
                    .account
                    .clone()
                    .or_else(|| plan.account.clone())
                    .or_else(|| plan.accounts.first().cloned())
                    .unwrap_or_default();
                format!("{account}/{name}")
            };
            let ssh = env
                .outside("gh")
                .args(["config", "get", "git_protocol", "-h", "github.com"])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "ssh");
            let url = if ssh {
                format!("git@github.com:{full}.git")
            } else {
                format!("https://github.com/{full}.git")
            };
            match env.sandboxed_output(
                &project_of(request),
                dir,
                &["remote", "add", remote, &url],
                None,
            ) {
                Some(_) => println!("Added the remote {remote}: {url}"),
                None => eprintln!(
                    "[Pistachio] The repository was created, but adding the remote {remote} failed; \
                     run `git remote add {remote} {url}`."
                ),
            }
        }
    }
}

/// The command for `plan` (`gh`, signing), outside the sandbox, with [`hardened_env`] and none of
/// this process's git variables. git's own requests go through [`git::execute`] instead.
pub fn command(plan: &Plan, env: &relay::Env) -> Command {
    let mut command = env.outside(&plan.program);
    command.args(&plan.args).current_dir(&plan.cwd);
    command
}

/// Options of a command: which take a value, which may stand alone.
pub(crate) struct Options<'a> {
    /// Options that stand alone (`--tags`, `-u`).
    pub flags: &'a [&'a str],
    /// Options that take a value (`--depth 1` or `--depth=1`).
    pub valued: &'a [&'a str],
}

/// An argument list read with `options`: what was given and the rest, in order.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub flags: Vec<String>,
    pub values: Vec<(String, String)>,
    pub positional: Vec<String>,
}

impl Parsed {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|given| given == flag)
    }

    pub fn value(&self, option: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == option)
            .map(|(_, value)| value.as_str())
    }
}

/// Reads `args` with `options`; an option not among them is refused (naming `what`).
pub(crate) fn parse_args(
    args: &[String],
    options: &Options<'_>,
    what: &str,
) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut iter = args.iter();
    let mut only_positional = false;
    while let Some(arg) = iter.next() {
        if only_positional || !arg.starts_with('-') || arg == "-" {
            parsed.positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        if let Some((name, value)) = arg.split_once('=')
            && options.valued.contains(&name)
        {
            parsed.values.push((name.to_owned(), value.to_owned()));
            continue;
        }
        if options.valued.contains(&arg.as_str()) {
            let Some(value) = iter.next() else {
                return Err(format!("`{what} {arg}` needs a value."));
            };
            parsed.values.push((arg.clone(), value.clone()));
            continue;
        }
        if options.flags.contains(&arg.as_str())
            || arg
                .split_once('=')
                .is_some_and(|(name, _)| options.flags.contains(&name))
        {
            parsed.flags.push(arg.clone());
            continue;
        }
        return Err(format!(
            "Pistachio doesn't run `{what} {arg}` outside the sandbox. Use the command without \
             that option, or ask the user to run it."
        ));
    }
    Ok(parsed)
}

/// Whether `path` is the project or inside it.
pub(crate) fn inside(path: &Path, project: &Path) -> bool {
    crate::paths::resolve(path).starts_with(crate::paths::resolve(project))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_allow_a_kind_for_a_project_and_target() {
        let mut settings = Settings::default();
        let project = PathBuf::from("/work/app");
        allow(&mut settings, &project, "github.com/a/b", Class::Push, None);
        allow(&mut settings, &project, "github.com/a/b", Class::Push, None);
        allow(
            &mut settings,
            &project,
            "github.com/a/b",
            Class::Fetch,
            None,
        );
        assert_eq!(settings.gate.len(), 1);
        assert_eq!(settings.gate[0].allow, [Class::Push, Class::Fetch]);
        let plan = Plan {
            title: String::new(),
            details: Vec::new(),
            warnings: Vec::new(),
            class: Class::Push,
            target: "github.com/a/b".into(),
            rememberable: true,
            program: "git".into(),
            args: Vec::new(),
            cwd: project.clone(),
            stdin: None,
            accounts: Vec::new(),
            account: None,
            job: None,
            after: None,
        };
        let request = Request {
            tool: "git".into(),
            args: Vec::new(),
            cwd: project.join("src"),
            project: Some(project.clone()),
            stdin: None,
            account: None,
            approved: None,
        };
        assert!(allowed_by_rules(&plan, &request, &settings, None));
        let other = Plan {
            target: "github.com/evil/b".into(),
            ..plan.clone()
        };
        assert!(!allowed_by_rules(&other, &request, &settings, None));
        let rewrite = Plan {
            class: Class::Rewrite,
            ..plan.clone()
        };
        assert!(!allowed_by_rules(&rewrite, &request, &settings, None));
        let elsewhere = Request {
            project: Some(PathBuf::from("/work/other")),
            ..request.clone()
        };
        assert!(!allowed_by_rules(&plan, &elsewhere, &settings, None));
        let once = Plan {
            rememberable: false,
            ..plan
        };
        assert!(!allowed_by_rules(&once, &request, &settings, None));
    }

    #[test]
    fn github_rules_keep_their_account() {
        let mut settings = Settings::default();
        let project = PathBuf::from("/work/app");
        allow(
            &mut settings,
            &project,
            "github.com/acme/app",
            Class::GhWrite,
            Some("me-at-work"),
        );
        let request = Request {
            tool: "gh".into(),
            args: Vec::new(),
            cwd: project.clone(),
            project: Some(project.clone()),
            stdin: None,
            account: None,
            approved: None,
        };
        let plan = Plan {
            title: String::new(),
            details: Vec::new(),
            warnings: Vec::new(),
            class: Class::GhWrite,
            target: "github.com/acme/app".into(),
            rememberable: true,
            program: "gh".into(),
            args: Vec::new(),
            cwd: project.clone(),
            stdin: None,
            accounts: vec!["me".into(), "me-at-work".into()],
            account: None,
            job: None,
            after: None,
        };
        let rule = matching_rule(&plan, &request, &settings, None).expect("allowed");
        assert_eq!(rule.account.as_deref(), Some("me-at-work"));
        // That account logged out: asked again.
        let gone = Plan {
            accounts: vec!["me".into()],
            ..plan
        };
        assert!(matching_rule(&gone, &request, &settings, None).is_none());
    }

    #[test]
    fn options_outside_the_list_are_refused() {
        let options = Options {
            flags: &["--tags", "-u"],
            valued: &["--depth"],
        };
        let args = |list: &[&str]| list.iter().map(|&arg| arg.to_owned()).collect::<Vec<_>>();
        let parsed = parse_args(
            &args(&["-u", "origin", "--depth=3", "--", "-x"]),
            &options,
            "git push",
        )
        .expect("allowed");
        assert!(parsed.has("-u"));
        assert_eq!(parsed.value("--depth"), Some("3"));
        assert_eq!(parsed.positional, ["origin", "-x"]);
        let refused =
            parse_args(&args(&["--receive-pack=evil"]), &options, "git push").unwrap_err();
        assert!(refused.contains("--receive-pack"), "{refused}");
        assert!(parse_args(&args(&["--depth"]), &options, "git fetch").is_err());
    }

    #[test]
    fn the_environment_turns_hooks_and_prompts_off() {
        let env = hardened_env(Path::new("/tmp/none"), &[]);
        let get = |key: &str| {
            env.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("GIT_CONFIG_KEY_0"), Some("core.hooksPath"));
        assert_eq!(get("GIT_CONFIG_VALUE_0"), Some("/tmp/none"));
        assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(get("GIT_CONFIG_COUNT"), Some("6"));
        assert_eq!(get("GIT_CONFIG_KEY_5"), Some("safe.bareRepository"));
        assert_eq!(get("GIT_CONFIG_VALUE_5"), Some("explicit"));
    }
}
