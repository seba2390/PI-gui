//! The helper: `<app> --sandbox-tool <command>`, started by the pi extension and the app.
//!
//! - `run --cwd <folder> [--force] -- <program> <args…>` becomes `<program>` in the sandbox for
//!   `<folder>` (or without one when the sandbox is off for it; `--force` ignores that). The
//!   extension runs every `bash` command this way.
//! - `policy --cwd <folder>` prints the rules for that folder as JSON ([`PolicyReply`]); the
//!   extension checks the file tools' paths against them.
//! - `status` checks again which sandbox works here and prints it ([`Status`]).
//! - `test` tries what the sandbox should block and prints what happened ([`TestReport`]).
//!
//! The settings come from `$PISTACHIO_SANDBOX_DIR` (see [`crate::default_dir`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend, Status};
use crate::policy::{Context, Policy};
use crate::settings::{Network, Settings};

pub const FLAG: &str = "--sandbox-tool";
pub(crate) const LANDLOCK_PROBE: &str = "landlock-probe";

/// The rules for a folder, as the extension reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyReply {
    /// Whether commands there run in a sandbox: it is on for the folder and works here.
    pub sandboxed: bool,
    /// Whether file tools are checked: the sandbox is on for the folder (also when no sandbox
    /// works here, the file tools still keep to the rules).
    pub enforced: bool,
    pub backend: Option<Backend>,
    pub project: Option<PathBuf>,
    /// Hidden as a whole except what is readable or writable in them (the home folder).
    pub private: Vec<PathBuf>,
    pub readable: Vec<PathBuf>,
    pub writable: Vec<PathBuf>,
    /// Hidden wherever they are.
    pub hidden: Vec<PathBuf>,
    pub network: Network,
}

/// One thing the test tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    /// `None`: not tried.
    pub passed: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReport {
    pub status: Status,
    pub checks: Vec<Check>,
}

/// Runs the helper when the process was started with [`FLAG`], returning its exit code.
pub fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(FLAG.as_ref()) {
        return None;
    }
    let command = args.next().map(|arg| arg.to_string_lossy().into_owned());
    let rest: Vec<OsString> = args.collect();
    let dir = crate::default_dir();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pistachio"));
    Some(match command.as_deref() {
        Some("run") => run(&dir, &exe, &rest),
        Some("policy") => print_json(&policy_reply(&dir, &exe, &cwd_arg(&rest))),
        Some("status") => print_json(&backend::detect(&dir, &exe)),
        Some("test") => print_json(&test(&dir, &exe)),
        Some(LANDLOCK_PROBE) => landlock_probe(),
        _ => {
            eprintln!("usage: {FLAG} run|policy|status|test");
            2
        }
    })
}

fn print_json(value: &impl Serialize) -> i32 {
    match serde_json::to_string(value) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// `--cwd <folder>` among `args`, else the current folder.
fn cwd_arg(args: &[OsString]) -> PathBuf {
    args.iter()
        .position(|arg| arg == "--cwd")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The rules for `cwd` with the saved settings.
pub fn policy_reply(dir: &Path, exe: &Path, cwd: &Path) -> PolicyReply {
    let settings = Settings::load(dir);
    let policy = Policy::for_project(&settings, cwd, &Context::current());
    let backend = policy
        .as_ref()
        .and_then(|_| backend::current(dir, exe).backend);
    match policy {
        Some(policy) => PolicyReply {
            sandboxed: backend.is_some(),
            enforced: true,
            backend,
            project: Some(policy.project),
            private: policy.private,
            readable: policy.readable,
            writable: policy.writable,
            hidden: policy.hidden,
            network: policy.network,
        },
        None => PolicyReply {
            sandboxed: false,
            enforced: false,
            backend: None,
            project: None,
            private: Vec::new(),
            readable: Vec::new(),
            writable: Vec::new(),
            hidden: Vec::new(),
            network: settings.network,
        },
    }
}

/// `run --cwd <folder> [--force] -- <program> <args…>`.
fn run(dir: &Path, exe: &Path, args: &[OsString]) -> i32 {
    let Some(split) = args.iter().position(|arg| arg == "--") else {
        eprintln!("usage: {FLAG} run --cwd <folder> [--force] -- <program> <args…>");
        return 2;
    };
    let (options, argv) = (&args[..split], &args[split + 1..]);
    let Some((program, program_args)) = argv.split_first() else {
        eprintln!("{FLAG} run: no program to run");
        return 2;
    };
    let cwd = cwd_arg(options);
    let mut settings = Settings::load(dir);
    if options.iter().any(|arg| arg == "--force") {
        settings.enabled = true;
        settings.unsandboxed.clear();
    }
    let policy = Policy::for_project(&settings, &cwd, &Context::current());
    let mut command = match &policy {
        None => plain(program, program_args),
        Some(policy) => match sandboxed(policy, dir, exe, program, program_args) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("Pistachio's sandbox: {error}");
                return 126;
            }
        },
    };
    command.current_dir(&cwd);
    if policy.is_some() {
        for (name, _) in std::env::vars_os() {
            if crate::secrets::is_secret(&name.to_string_lossy()) {
                command.env_remove(name);
            }
        }
    }
    exec(command, program)
}

fn plain(program: &OsString, args: &[OsString]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    command
}

/// The command in the sandbox this computer has. Without one it runs as it is (the app says
/// that sessions are not sandboxed). With Landlock, this process restricts itself first.
fn sandboxed(
    policy: &Policy,
    dir: &Path,
    exe: &Path,
    program: &OsString,
    args: &[OsString],
) -> Result<Command, String> {
    match backend::current(dir, exe).backend {
        Some(Backend::Seatbelt) => {
            let mut command = Command::new(crate::seatbelt::SANDBOX_EXEC);
            command
                .arg("-p")
                .arg(crate::seatbelt::profile(policy))
                .arg(program)
                .args(args);
            Ok(command)
        }
        Some(Backend::Bubblewrap { path }) => {
            let mut command = Command::new(path);
            command
                .args(crate::bwrap::args(policy))
                .arg(program)
                .args(args);
            Ok(command)
        }
        Some(Backend::Landlock) => {
            restrict_self(policy)?;
            Ok(plain(program, args))
        }
        None => Ok(plain(program, args)),
    }
}

#[cfg(target_os = "linux")]
fn restrict_self(policy: &Policy) -> Result<(), String> {
    crate::landlock::apply(policy).map(|_| ())
}

#[cfg(not(target_os = "linux"))]
fn restrict_self(_policy: &Policy) -> Result<(), String> {
    Err("Landlock exists only on Linux".to_owned())
}

#[cfg(unix)]
fn exec(mut command: Command, program: &OsString) -> i32 {
    use std::os::unix::process::CommandExt as _;
    let error = command.exec();
    eprintln!("{}: {error}", Path::new(program).display());
    127
}

#[cfg(not(unix))]
fn exec(mut command: Command, program: &OsString) -> i32 {
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("{}: {error}", Path::new(program).display());
            127
        }
    }
}

/// Applies Landlock to this process with a policy that hides nothing and blocks the network,
/// and says what was enforced: `{"network": bool}` or `{"error": …}`.
fn landlock_probe() -> i32 {
    #[cfg(target_os = "linux")]
    {
        let probe = Policy {
            project: std::env::temp_dir(),
            private: Vec::new(),
            readable: Vec::new(),
            writable: vec![std::env::temp_dir()],
            hidden: Vec::new(),
            network: Network::Off,
        };
        match crate::landlock::apply(&probe) {
            Ok(enforced) => {
                println!("{}", serde_json::json!({ "network": enforced.network }));
                0
            }
            Err(error) => {
                println!("{}", serde_json::json!({ "error": error }));
                1
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!(
            "{}",
            serde_json::json!({ "error": "Landlock exists only on Linux" })
        );
        1
    }
}

/// Tries, in a fresh folder standing in for a project, what the sandbox should allow and block
/// with the saved settings (as if it were on).
fn test(dir: &Path, exe: &Path) -> TestReport {
    let status = backend::detect(dir, exe);
    let mut checks = Vec::new();
    if status.backend.is_none() || cfg!(windows) {
        return TestReport { status, checks };
    }
    let project =
        std::env::temp_dir().join(format!("pistachio-sandbox-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&project);
    let mut settings = Settings::load(dir);
    settings.enabled = true;
    let policy = Policy::for_project(&settings, &project, &Context::current());
    let run_in = |program: &str, script: &str| -> bool {
        Command::new(exe)
            .arg(FLAG)
            .arg("run")
            .arg("--cwd")
            .arg(&project)
            .args(["--force", "--", program, "-c", script])
            .env(crate::DIR_ENV, dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let shell = |script: &str| run_in("/bin/sh", script);

    let inside = project.join("written-by-the-test");
    let wrote_inside = shell(&format!("echo ok > {}", sh_quote(&inside))) && inside.exists();
    checks.push(Check {
        name: "Writing inside the project".into(),
        passed: Some(wrote_inside),
        detail: if wrote_inside {
            "Allowed, as it should be.".into()
        } else {
            "Blocked: commands could not change the project.".into()
        },
    });

    let outside = crate::paths::home()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(format!(".pistachio-sandbox-test-{}", std::process::id()));
    let _ = shell(&format!("echo no > {}", sh_quote(&outside)));
    let wrote_outside = outside.exists();
    let _ = std::fs::remove_file(&outside);
    checks.push(Check {
        name: "Writing outside the project".into(),
        passed: Some(!wrote_outside),
        detail: if wrote_outside {
            format!("Allowed: a command could write {}.", outside.display())
        } else {
            "Blocked.".into()
        },
    });

    // Something in the home folder the project may not see: keys if there are any, else
    // another folder there.
    let private = policy.as_ref().and_then(|policy| {
        let home_entries = policy.private.iter().flat_map(|private| {
            std::fs::read_dir(private)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
        });
        policy
            .hidden
            .iter()
            .filter(|path| path.exists())
            .cloned()
            .chain(home_entries.filter(|path| !policy.may_read(&crate::paths::resolve(path))))
            .next()
    });
    checks.push(match private {
        Some(path) => {
            let what = if path.is_dir() { "ls" } else { "cat" };
            let read = shell(&format!("{what} {} > /dev/null", sh_quote(&path)));
            Check {
                name: "Reading your other files".into(),
                passed: Some(!read),
                detail: if read {
                    format!("Allowed: a command could read {}.", path.display())
                } else {
                    format!("Blocked ({}).", path.display())
                },
            }
        }
        None => Check {
            name: "Reading your other files".into(),
            passed: None,
            detail: "There is nothing in the home folder to try.".into(),
        },
    });

    checks.push(match settings.network {
        Network::On => Check {
            name: "Network".into(),
            passed: None,
            detail: "Allowed, as set.".into(),
        },
        Network::Off => {
            // bash has /dev/tcp; `sh` may not.
            let connected = run_in("/bin/bash", "exec 3<>/dev/tcp/1.1.1.1/443");
            Check {
                name: "Network".into(),
                passed: Some(!connected),
                detail: if connected {
                    "Allowed: a command could connect to the Internet.".into()
                } else if status.network {
                    "Blocked.".into()
                } else {
                    "No connection, but this system can't block the network: it may just be offline."
                        .into()
                },
            }
        }
    });
    let _ = std::fs::remove_dir_all(&project);
    TestReport { status, checks }
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}
