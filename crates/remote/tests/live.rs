//! Read-only end-to-end checks against a real host. Ignored by default; run with
//! `PI_GUI_SSH_TEST_HOST=<ssh config alias> cargo test -p remote -- --ignored --nocapture`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

use remote::{RemoteError, Ssh, Target, discover_hosts, resolve};

fn test_host() -> Option<Ssh> {
    let alias = std::env::var("PI_GUI_SSH_TEST_HOST").ok()?;
    Some(Ssh::new(Target::Alias { alias }))
}

#[test]
#[ignore = "needs PI_GUI_SSH_TEST_HOST and network access"]
fn live_host_round_trip() {
    let Some(ssh) = test_host() else {
        eprintln!("PI_GUI_SSH_TEST_HOST not set; skipping");
        return;
    };
    let home = dirs::home_dir().unwrap();
    let aliases: Vec<String> = discover_hosts(&home)
        .into_iter()
        .map(|host| host.alias)
        .collect();
    eprintln!("config hosts: {aliases:?}");
    eprintln!("resolved: {:?}", resolve(ssh.target()).unwrap());
    eprintln!("control dir: {}", ssh.control_dir().display());

    ssh.disconnect();
    assert!(!ssh.is_connected());
    let started = Instant::now();
    ssh.connect(false).unwrap();
    eprintln!("connect: {:?}", started.elapsed());
    assert!(ssh.is_connected());

    let result = std::panic::catch_unwind(|| checks(&ssh));
    ssh.disconnect();
    assert!(!ssh.is_connected());
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn checks(ssh: &Ssh) {
    let started = Instant::now();
    assert_eq!(ssh.exec("echo hello").unwrap(), "hello\n");
    eprintln!("exec: {:?}", started.elapsed());

    let started = Instant::now();
    let probe = ssh.probe("pi").unwrap();
    eprintln!("probe: {:?} {probe:?}", started.elapsed());
    assert!(probe.home.starts_with('/'));
    let probe_sh = ssh.probe("sh").unwrap();
    eprintln!("probe sh: {probe_sh:?}");
    assert!(probe_sh.pi_path.is_some());

    let started = Instant::now();
    let listing = ssh.list_dirs("~").unwrap();
    eprintln!(
        "list ~: {:?} {} ({} dirs, parent {:?})",
        started.elapsed(),
        listing.path,
        listing.dirs.len(),
        listing.parent
    );
    assert_eq!(listing.path, probe.home);
    let root = ssh.list_dirs("/").unwrap();
    assert_eq!(root.parent, None);
    assert!(root.dirs.iter().any(|dir| dir == "usr"), "{root:?}");
    let missing = ssh.list_dirs("/no/such/folder 'x'").unwrap_err();
    eprintln!("missing folder: {missing}");
    assert!(matches!(missing, RemoteError::Failed(_)));

    let fails = ssh.exec("echo oops >&2; exit 3").unwrap_err();
    assert_eq!(fails.to_string(), "oops");

    // Quoting through ssh, the remote user's shell and the login shell.
    let argv: Vec<String> = [
        "printf",
        "[%s]\\n",
        "a b",
        "it's",
        "$HOME",
        "",
        "`id`; echo pwned",
        "*",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let (program, args) = ssh.command(Some("/tmp"), &argv);
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    // Login-shell profiles may print banners first; the command's output comes last.
    assert!(
        stdout.ends_with("[a b]\n[it's]\n[$HOME]\n[]\n[`id`; echo pwned]\n[*]\n"),
        "{stdout}"
    );

    let fingerprint = ssh.host_key_fingerprint().unwrap();
    eprintln!("fingerprint:\n{fingerprint}");
    assert!(fingerprint.contains("SHA256:"));
}
