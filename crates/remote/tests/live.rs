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
    let listing = ssh.list_dirs("~", false).unwrap();
    eprintln!(
        "list ~: {:?} {} ({} dirs, parent {:?})",
        started.elapsed(),
        listing.path,
        listing.dirs.len(),
        listing.parent
    );
    assert_eq!(listing.path, probe.home);
    let root = ssh.list_dirs("/", false).unwrap();
    assert_eq!(root.parent, None);
    assert!(root.dirs.iter().any(|dir| dir == "usr"), "{root:?}");
    let missing = ssh.list_dirs("/no/such/folder 'x'", false).unwrap_err();
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

/// Installs pi on the test host (into `~/.pi-gui/server`, like the app does when pi is
/// missing) and checks that the probe finds it. Writes to the host, so it is opt-in:
/// `PI_GUI_SSH_TEST_HOST=<alias> cargo test -p remote --test live -- --ignored live_install`.
#[test]
#[ignore = "installs pi on PI_GUI_SSH_TEST_HOST"]
fn live_install() {
    let Some(ssh) = test_host() else { return };
    ssh.connect(false).unwrap();
    let started = Instant::now();
    let pi = ssh
        .install_pi(|step| eprintln!("step: {step} ({:?})", started.elapsed()))
        .unwrap();
    eprintln!("installed {pi} in {:?}", started.elapsed());
    let probe = ssh.probe("pi").unwrap();
    eprintln!("probe: {probe:?}");
    assert_eq!(probe.pi_path.as_deref(), Some(pi.as_str()));
    assert!(probe.pi_version.is_some());
}

/// The diff / files panels, the file index, sessions, uploads and trash over a real host,
/// in a scratch folder there (removed afterwards).
#[test]
#[ignore = "needs PI_GUI_SSH_TEST_HOST and network access"]
fn live_workspace_and_sessions() {
    use std::path::Path;
    use std::sync::Arc;
    use workspace::Location;

    let Some(ssh) = test_host() else { return };
    ssh.connect(false).unwrap();
    let root = ssh
        .exec("d=$(mktemp -d /tmp/pi-gui-live.XXXXXX) && cd \"$d\" && git init -q && printf 'one\\n' > a.txt && git add . && git -c user.name=t -c user.email=t@t commit -qm init && printf 'one\\ntwo\\n' > a.txt && mkdir -p 'src dir' ignored && printf 'x\\n' > 'src dir/new file.rs' && printf 'ignored/\\n' > .gitignore && printf 'secret' > ignored/s && pwd -P")
        .unwrap()
        .trim()
        .to_owned();
    eprintln!("scratch repo: {root}");
    let result = std::panic::catch_unwind(|| {
        let location = Location::Remote(Arc::new(ssh.clone()));
        let root_path = Path::new(&root);
        let started = Instant::now();
        let status = workspace::git::status_at(&location, root_path)
            .unwrap()
            .unwrap();
        eprintln!("status {:?}: {:?}", started.elapsed(), status.files);
        let paths: Vec<String> = status
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(paths, [".gitignore", "a.txt", "src dir/new file.rs"]);
        let a = &status.files[1];
        assert_eq!((a.additions, a.deletions), (Some(1), Some(0)));
        let diff = workspace::git::change_diff_at(&location, &status, a).unwrap();
        assert!(diff.contains("+two"), "{diff}");

        let started = Instant::now();
        let entries =
            workspace::files::list_dir_at(&location, root_path, root_path, false).unwrap();
        eprintln!("list {:?}", started.elapsed());
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src dir", "a.txt"]);
        let (mut files, cut) = workspace::files::list_files_at(&location, root_path, 100);
        files.sort();
        assert!(!cut);
        assert_eq!(files, ["a.txt", "src dir/new file.rs"]);

        // Sessions: a fake session file in a scratch sessions folder.
        let sessions = format!("{root}/sessions");
        let file = format!("{sessions}/--srv--/2026_x.jsonl");
        ssh.upload(
            &file,
            concat!(
                r#"{"type":"session","version":3,"id":"x","timestamp":"t","cwd":"/srv"}"#,
                "\n",
                r#"{"type":"message","id":"1","parentId":null,"timestamp":"t","message":{"role":"user","content":"hello remote"}}"#,
                "\n"
            )
            .as_bytes()
            .to_vec(),
        )
        .unwrap();
        // Reading files and sending private input (how the models dialog works remotely).
        let read = ssh.read_file(&file).unwrap().expect("the uploaded file");
        assert!(String::from_utf8(read).unwrap().contains("hello remote"));
        assert_eq!(
            ssh.read_file(&format!("{root}/missing.json")).unwrap(),
            None
        );
        let echoed = ssh
            .exec_with_input("cat", b"secret header".to_vec())
            .unwrap();
        assert_eq!(echoed, b"secret header");

        let mut index = remote::SessionIndex::default();
        let listed = index.refresh(&ssh, &sessions).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title(), "hello remote");
        let again = index.refresh(&ssh, &sessions).unwrap();
        assert_eq!(again, listed);
        ssh.trash(&file).unwrap();
        assert!(!ssh.exists(&file).unwrap());
        assert_eq!(index.refresh(&ssh, &sessions).unwrap(), Vec::new());
    });
    // The trashed fake session went to the host's trash; remove the scratch repo itself.
    let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&root)));
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
