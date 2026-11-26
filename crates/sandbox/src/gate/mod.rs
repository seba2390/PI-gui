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
    /// issues, creating repositories.
    GhWrite,
}

impl Class {
    pub const ALL: [Self; 7] = [
        Self::Fetch,
        Self::Push,
        Self::PushDefault,
        Self::Rewrite,
        Self::Sign,
        Self::GhRead,
        Self::GhWrite,
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
}

/// What `request` needs, with `settings`' rules.
pub fn decide(request: &Request, settings: &Settings, home: Option<&Path>) -> Decision {
    let planned = match request.tool.as_str() {
        "git" => git::plan(request),
        "gh" => gh::plan(request),
        "gpg" | "ssh-keygen" => sign::plan(request),
        _ => Ok(None),
    };
    match planned {
        Ok(None) => Decision::Local,
        Ok(Some(plan)) => {
            let approved = allowed_by_rules(&plan, request, settings, home);
            Decision::Gate {
                plan: Box::new(plan),
                approved,
            }
        }
        Err(reason) => Decision::Refused { reason },
    }
}

/// The project a request belongs to: the session's folder, else where it ran.
fn project_of(request: &Request) -> PathBuf {
    let project = request
        .project
        .clone()
        .unwrap_or_else(|| request.cwd.clone());
    crate::paths::resolve(&project)
}

/// Whether a rule allows `plan` without asking.
pub fn allowed_by_rules(
    plan: &Plan,
    request: &Request,
    settings: &Settings,
    home: Option<&Path>,
) -> bool {
    let project = project_of(request);
    plan.rememberable
        && settings.gate.iter().any(|rule| {
            let folder = crate::paths::resolve(&crate::paths::expand(&rule.project, home));
            project == folder && rule.target == plan.target && rule.allow.contains(&plan.class)
        })
}

/// Adds `class` for `target` in `project` to the rules.
pub fn allow(settings: &mut Settings, project: &Path, target: &str, class: Class) {
    let project = crate::paths::tilde(project);
    match settings
        .gate
        .iter_mut()
        .find(|rule| rule.project == project && rule.target == target)
    {
        Some(rule) => {
            if !rule.allow.contains(&class) {
                rule.allow.push(class);
            }
        }
        None => settings.gate.push(Rule {
            project,
            target: target.to_owned(),
            allow: vec![class],
        }),
    }
}

/// The environment of what the gate runs: hooks off (from `hooks`, an empty folder), no program
/// a repository could name, no prompts on a terminal nobody sees. As environment variables,
/// so the git that `gh` or git-lfs start keeps to them too.
pub fn hardened_env(hooks: &Path) -> Vec<(String, String)> {
    let settings = [
        ("core.hooksPath", hooks.to_string_lossy().into_owned()),
        ("core.fsmonitor", "false".to_owned()),
        ("protocol.ext.allow", "never".to_owned()),
        ("protocol.file.allow", "never".to_owned()),
        ("submodule.recurse", "false".to_owned()),
    ];
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

/// The command for `plan`, outside the sandbox, with [`hardened_env`]. Git's own variables from
/// this process's environment are dropped (a repository or folder they name would be obeyed).
pub fn command(plan: &Plan, hooks: &Path) -> Command {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.cwd);
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if name.starts_with("GIT_") && name != "GIT_SSH_COMMAND" && name != "GIT_SSH" {
            command.env_remove(name.as_ref());
        }
    }
    command.envs(hardened_env(hooks));
    command
}

/// Output of a quick git command outside the sandbox (counting commits for the question), or
/// `None`.
pub(crate) fn git_output(repo: &Path, args: &[&str], hooks: &Path) -> Option<String> {
    let plan = Plan {
        title: String::new(),
        details: Vec::new(),
        warnings: Vec::new(),
        class: Class::Fetch,
        target: String::new(),
        rememberable: false,
        program: "git".to_owned(),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        cwd: repo.to_path_buf(),
        stdin: None,
    };
    let output = command(&plan, hooks)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// An empty folder for `core.hooksPath`, so no hook runs.
pub fn hooks_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pistachio-no-hooks-{}", whoami()));
    let _ = std::fs::create_dir_all(&dir);
    // Anything put there is not ours.
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    dir
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
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
        allow(&mut settings, &project, "github.com/a/b", Class::Push);
        allow(&mut settings, &project, "github.com/a/b", Class::Push);
        allow(&mut settings, &project, "github.com/a/b", Class::Fetch);
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
        };
        let request = Request {
            tool: "git".into(),
            args: Vec::new(),
            cwd: project.join("src"),
            project: Some(project.clone()),
            stdin: None,
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
        let env = hardened_env(Path::new("/tmp/none"));
        let get = |key: &str| {
            env.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("GIT_CONFIG_KEY_0"), Some("core.hooksPath"));
        assert_eq!(get("GIT_CONFIG_VALUE_0"), Some("/tmp/none"));
        assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(get("GIT_CONFIG_COUNT"), Some("5"));
    }
}
