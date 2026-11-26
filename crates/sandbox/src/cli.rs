//! The helper: `<app> --sandbox-tool <command>`, started by the pi extension and the app.
//!
//! - `run --cwd <folder> [--force] -- <program> <args…>` becomes `<program>` in the sandbox for
//!   `<folder>` (or without one when the sandbox is off for it; `--force` ignores that). The
//!   extension runs every `bash` command this way.
//! - `policy --cwd <folder>` prints the rules for that folder as JSON ([`PolicyReply`]); the
//!   extension checks the file tools' paths against them.
//! - `status` checks again which sandbox works here and prints it ([`Status`]).
//! - `proxy` runs the proxy of a limited network by itself (the app runs it otherwise).
//!
//! The settings come from `$PISTACHIO_SANDBOX_DIR` (see [`crate::default_dir`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::backend::{self, Backend};
use crate::policy::{Context, Policy};
use crate::settings::{Network, Settings};

pub const FLAG: &str = "--sandbox-tool";
pub(crate) const LANDLOCK_PROBE: &str = "landlock-probe";
/// Inside bubblewrap with a limited network: `bridge <socket> <port> -- <program> <args…>`.
const BRIDGE: &str = "bridge";

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
        Some(LANDLOCK_PROBE) => landlock_probe(),
        Some(BRIDGE) => bridge(&rest),
        Some("proxy") => match crate::proxy::start(&dir) {
            // The app runs it; this runs it alone, to try it.
            Ok(endpoint) => {
                println!("{}", endpoint.url());
                loop {
                    std::thread::park();
                }
            }
            Err(error) => {
                eprintln!("the proxy could not start: {error}");
                1
            }
        },
        _ => {
            eprintln!("usage: {FLAG} run|policy|status");
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
    let mut policy = Policy::for_project(&settings, &cwd, &Context::current());
    // A limited network goes through the app's proxy; without one (the app is not running)
    // there is none.
    let mut proxy = None;
    if let Some(policy) = policy
        .as_mut()
        .filter(|policy| policy.network == Network::Custom)
    {
        match crate::proxy::endpoint(dir) {
            Some(endpoint) => {
                policy.proxy = Some(endpoint.port);
                proxy = Some(endpoint);
            }
            None => policy.network = Network::Off,
        }
    }
    let mut command = match &policy {
        None => plain(program, program_args),
        Some(policy) => match sandboxed(policy, proxy.as_ref(), dir, exe, program, program_args) {
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
    if let Some(endpoint) = &proxy {
        command.envs(endpoint.env());
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
    proxy: Option<&crate::proxy::Endpoint>,
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
            // A limited network: the command has a network of its own, and this program, mounted
            // in, passes its connections on to the proxy's socket.
            let bridge = match (policy.network, policy.proxy, proxy) {
                (
                    Network::Custom,
                    Some(port),
                    Some(crate::proxy::Endpoint {
                        socket: Some(socket),
                        ..
                    }),
                ) => Some((socket.clone(), port)),
                _ => None,
            };
            match bridge {
                Some((socket, port)) => {
                    let extra = [(socket.clone(), true), (exe.to_path_buf(), false)];
                    command
                        .args(crate::bwrap::args(policy, &extra))
                        .arg(exe)
                        .args([FLAG, BRIDGE])
                        .arg(&socket)
                        .arg(port.to_string())
                        .arg("--")
                        .arg(program)
                        .args(args);
                }
                None => {
                    command
                        .args(crate::bwrap::args(policy, &[]))
                        .arg(program)
                        .args(args);
                }
            }
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
            proxy: None,
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

/// `bridge <socket> <port> -- <program> <args…>` (see [`crate::proxy::bridge`]).
fn bridge(args: &[OsString]) -> i32 {
    #[cfg(unix)]
    {
        let split = args.iter().position(|arg| arg == "--");
        let (Some(socket), Some(port), Some(split)) = (args.first(), args.get(1), split) else {
            eprintln!("usage: {FLAG} {BRIDGE} <socket> <port> -- <program> <args…>");
            return 2;
        };
        let Some(port) = port.to_str().and_then(|port| port.parse().ok()) else {
            eprintln!("{FLAG} {BRIDGE}: not a port");
            return 2;
        };
        match args[split + 1..].split_first() {
            Some((program, rest)) => crate::proxy::bridge(Path::new(socket), port, program, rest),
            None => 2,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        2
    }
}
