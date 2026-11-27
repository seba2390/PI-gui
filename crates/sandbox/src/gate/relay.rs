//! Where git runs with the user's keys: a mirror of the remote the gate keeps for each project and
//! remote, outside the sandbox and out of its reach. git outside the sandbox never works in the
//! agent's repository, whose settings the agent writes; only in a mirror, with settings the gate
//! wrote.
//!
//! - Getting (fetch, pull, clone, submodules, LFS downloads): the gate fetches into the mirror,
//!   then the agent's command runs in the sandbox with the remote's address pointed at the mirror
//!   (`url.<mirror>.insteadOf`), which the sandbox may read for that command.
//! - Sending (push, LFS uploads): the sandbox writes the commits as a bundle (a file of git
//!   objects, plain data) to the gate, which adds them to the mirror and pushes from there.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest as _, Sha256};

use super::config::{normalize_url, unsafe_url};

/// What the gate's git needs to know: where things are.
#[derive(Debug, Clone)]
pub struct Env {
    /// This program, which starts sandboxed commands (`--sandbox-tool run`).
    pub exe: PathBuf,
    /// The sandbox's folder (its settings); the mirrors are in it.
    pub dir: PathBuf,
    /// More settings for the gate's own git, after the hardened ones (tests: local remotes).
    pub extra: Vec<(String, String)>,
}

impl Env {
    pub fn current() -> Self {
        Self {
            exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pistachio")),
            dir: crate::default_dir(),
            extra: Vec::new(),
        }
    }

    fn mirrors(&self) -> PathBuf {
        self.dir.join("gate-repos")
    }

    /// An empty folder for `core.hooksPath`, out of the sandbox's reach, so no hook runs.
    pub fn hooks(&self) -> PathBuf {
        let dir = self.dir.join("gate-hooks");
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        dir
    }

    /// An empty folder outside any repository, for `gh`.
    pub fn nowhere(&self) -> PathBuf {
        let dir = self.dir.join("gate-empty");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// git (or `program`) outside the sandbox: none of this process's git variables, the
    /// hardened settings, no repository found by looking around.
    pub fn outside(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("GIT_") && name != "GIT_SSH_COMMAND" && name != "GIT_SSH" {
                command.env_remove(name.as_ref());
            }
        }
        command.envs(super::hardened_env(&self.hooks(), &self.extra));
        let nowhere = self.nowhere();
        command
            .env(
                "GIT_CEILING_DIRECTORIES",
                nowhere.parent().unwrap_or(&nowhere),
            )
            .current_dir(&nowhere);
        command
    }

    /// git in the sandbox for `project`, in `cwd`, able to read `mirrors` (and only for this
    /// command): `--sandbox-tool run --cwd <project> --read <mirror>… -- git -C <cwd> …`.
    pub fn sandboxed(&self, project: &Path, cwd: &Path, mirrors: &[&Path]) -> Command {
        let mut command = Command::new(&self.exe);
        command
            .arg(crate::cli::FLAG)
            .arg("run")
            .arg("--cwd")
            .arg(project);
        for mirror in mirrors {
            command.arg("--read").arg(mirror);
        }
        let git = super::shim::real_program("git", &super::shim::dir(&self.dir))
            .unwrap_or_else(|| PathBuf::from("git"));
        command
            .arg("--")
            .arg(git)
            .arg("-c")
            .arg("protocol.file.allow=always")
            .arg("-C")
            .arg(cwd);
        command.env_remove(super::shim::SOCKET_ENV);
        command
    }

    /// The output of git in the sandbox (`None` when it fails), with `input` on standard input.
    pub fn sandboxed_output(
        &self,
        project: &Path,
        cwd: &Path,
        args: &[&str],
        input: Option<&str>,
    ) -> Option<String> {
        let mut command = self.sandboxed(project, cwd, &[]);
        command.args(args);
        output(command, input).ok()
    }
}

/// The output of `command`, or what it said on failure.
fn output(mut command: Command, input: Option<&str>) -> Result<String, String> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let text = text.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// The gate's copy of one remote for one project: a bare repository whose `refs/heads` and
/// `refs/tags` are the remote's (as of the last fetch). What the agent sends waits in
/// `refs/pistachio/`. Locked while open.
pub struct Mirror {
    pub dir: PathBuf,
    pub url: String,
    env: Env,
    _lock: File,
}

impl Mirror {
    /// The mirror of `url` for `project` (made the first time).
    pub fn open(env: &Env, project: &Path, url: &str) -> Result<Self, String> {
        if unsafe_url(url) {
            return Err(format!(
                "Pistachio won't use `{url}` outside the sandbox: only addresses of remote servers \
                 (https, ssh), not local folders or helper programs."
            ));
        }
        let root = env.mirrors();
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let mut hash = Sha256::new();
        hash.update(project.to_string_lossy().as_bytes());
        hash.update(b"\n");
        hash.update(url.as_bytes());
        let key = hex(&hash.finalize()[..10]);
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(format!("{key}.lock")))
            .map_err(|error| error.to_string())?;
        lock.lock().map_err(|error| error.to_string())?;
        let dir = root.join(format!("{key}.git"));
        let mirror = Self {
            dir,
            url: url.to_owned(),
            env: env.clone(),
            _lock: lock,
        };
        if !mirror.dir.join("HEAD").is_file() {
            let mut init = env.outside("git");
            init.args(["init", "-q", "--bare"]).arg(&mirror.dir);
            output(init, None).map_err(|error| format!("git init: {error}"))?;
            let _ = std::fs::write(
                mirror.dir.join("description"),
                format!("Pistachio's copy of {url} for {}\n", project.display()),
            );
        }
        // Written each time: only the gate writes here.
        let settings: &[&[&str]] = &[
            &["config", "remote.origin.url", url],
            &[
                "config",
                "--replace-all",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/tags/*:refs/tags/*",
            ],
            // What git-lfs takes for the remote's branches.
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
            &["config", "uploadpack.allowFilter", "true"],
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
            &["config", "gc.auto", "0"],
        ];
        for args in settings {
            mirror.git_output(args)?;
        }
        Ok(mirror)
    }

    /// git in the mirror, outside the sandbox.
    pub fn git(&self) -> Command {
        let mut command = self.env.outside("git");
        command.env("GIT_DIR", &self.dir);
        command
    }

    pub fn git_output(&self, args: &[&str]) -> Result<String, String> {
        let mut command = self.git();
        command.args(args);
        output(command, None).map_err(|error| format!("git {}: {error}", args.join(" ")))
    }

    /// `-c url.file://<mirror>.insteadOf=<url>`: the remote's address read as the mirror.
    pub fn rewrite(&self) -> [String; 2] {
        [
            "-c".to_owned(),
            format!("url.file://{}.insteadOf={}", self.dir.display(), self.url),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.git_output(&["for-each-ref", "--count=1", "refs/heads", "refs/tags"])
            .is_ok_and(|refs| refs.is_empty())
    }

    pub fn is_shallow(&self) -> bool {
        self.dir.join("shallow").is_file()
    }

    /// Gets what the remote has, with the user's keys; git's messages go to standard error.
    pub fn fetch(&self, options: &[String]) -> Result<(), String> {
        let mut command = self.git();
        command
            .args(["fetch", "--prune", "--no-write-fetch-head"])
            .args(options)
            .arg("origin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command.output().map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Getting {} failed: {}",
                self.url,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// The commit or object `rev` names in the mirror.
    pub fn resolve(&self, rev: &str) -> Option<String> {
        self.git_output(&["rev-parse", "--verify", "--quiet", rev])
            .ok()
            .filter(|sha| !sha.is_empty())
    }

    pub fn has(&self, sha: &str) -> bool {
        self.git_output(&["cat-file", "-e", sha]).is_ok()
    }

    /// Whether the files at `rev` are kept with git-lfs (its `.gitattributes` says so).
    pub fn uses_lfs(&self, rev: &str) -> bool {
        self.git_output(&["show", &format!("{rev}:.gitattributes")])
            .is_ok_and(|text| text.contains("filter=lfs"))
    }

    /// That git-lfs sends files only to the remote's own server (a `.lfsconfig` in the
    /// repository may name another).
    pub fn lfs_server_ok(&self) -> Result<(), String> {
        let host = |address: &str| {
            normalize_url(address)
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        let env = self.git_output(&["lfs", "env"]).unwrap_or_default();
        let theirs = host(&self.url);
        for line in env.lines() {
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            if !name.starts_with("Endpoint") {
                continue;
            }
            let address = value.split_whitespace().next().unwrap_or_default();
            if !address.is_empty() && host(address) != theirs {
                return Err(format!(
                    "Pistachio won't use git-lfs here: the repository sends LFS files to \
                     {address}, not to the remote's server ({theirs})."
                ));
            }
        }
        Ok(())
    }

    /// Downloads the LFS files of `revs` into the mirror (if git-lfs is installed).
    pub fn lfs_fetch(&self, options: &[String], revs: &[String]) -> Result<(), String> {
        if !lfs_installed() {
            return Ok(());
        }
        self.lfs_server_ok()?;
        let mut command = self.git();
        command
            .args(["lfs", "fetch"])
            .args(options)
            .arg("origin")
            .args(revs)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        let status = command.status().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Getting the LFS files from {} failed.", self.url))
        }
    }

    /// Adds `sources` (refs in the agent's repository, as `git rev-parse` names them) to the
    /// mirror, from a bundle the sandbox writes: what the mirror already has is left out. Returns
    /// each source's object.
    pub fn import(
        &self,
        project: &Path,
        cwd: &Path,
        sources: &[String],
    ) -> Result<Vec<String>, String> {
        let env = &self.env;
        let mut args = vec!["rev-parse", "--verify", "--end-of-options"];
        let mut shas = Vec::new();
        for source in sources {
            args.truncate(3);
            args.push(source);
            let sha = env
                .sandboxed_output(project, cwd, &args, None)
                .filter(|sha| sha.len() >= 40)
                .ok_or_else(|| format!("`{source}` isn't something in this repository."))?;
            shas.push(sha);
        }
        if shas.iter().all(|sha| self.has(sha)) {
            return Ok(shas);
        }
        // What the mirror has and the repository too: the bundle starts there.
        let known = self.git_output(&[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/heads",
            "refs/tags",
            "refs/pistachio",
        ])?;
        let checked = env
            .sandboxed_output(
                project,
                cwd,
                &["cat-file", "--batch-check=%(objectname) %(objecttype)"],
                Some(&format!("{known}\n")),
            )
            .unwrap_or_default();
        let mut bundle = env.sandboxed(project, cwd, &[]);
        bundle.args(["bundle", "create", "-"]).args(sources);
        let exclude: Vec<String> = checked
            .lines()
            .filter_map(|line| line.split_once(' '))
            .filter(|(_, kind)| matches!(*kind, "commit" | "tag"))
            .map(|(sha, _)| format!("^{sha}"))
            .collect();
        bundle.args(&exclude);
        let file = self
            .dir
            .join(format!("incoming-{}.bundle", std::process::id()));
        let result = self.import_bundle(bundle, &file, &shas);
        let _ = std::fs::remove_file(&file);
        result?;
        Ok(shas)
    }

    fn import_bundle(
        &self,
        mut bundle: Command,
        file: &Path,
        shas: &[String],
    ) -> Result<(), String> {
        let mut out = File::create(file).map_err(|error| error.to_string())?;
        let mut child = bundle
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let mut stdout = child.stdout.take().ok_or("no output")?;
        std::io::copy(&mut stdout, &mut out).map_err(|error| error.to_string())?;
        let mut errors = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut errors);
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!(
                "The commits couldn't be packed in the sandbox: {}",
                errors.trim()
            ));
        }
        let file = file.to_string_lossy();
        self.git_output(&[
            // The bundle is a file the gate wrote (git counts reading it as the `file` transport).
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "-q",
            "--no-write-fetch-head",
            &file,
            "+refs/*:refs/pistachio/in/*",
        ])?;
        for sha in shas {
            if !self.has(sha) {
                return Err(format!("The sandbox's bundle lacks {sha}."));
            }
        }
        Ok(())
    }

    /// Copies the LFS files `oids` from the agent's repository (read in the sandbox, checked
    /// against their names) into the mirror.
    pub fn import_lfs(&self, project: &Path, cwd: &Path, oids: &[String]) -> Result<(), String> {
        let env = &self.env;
        let common = env
            .sandboxed_output(
                project,
                cwd,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                None,
            )
            .ok_or("This isn't a git repository.")?;
        for oid in oids {
            if oid.len() != 64 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("`{oid}` isn't an LFS object id."));
            }
            let relative = format!("lfs/objects/{}/{}/{oid}", &oid[..2], &oid[2..4]);
            let into = self.dir.join(&relative);
            if into.is_file() {
                continue;
            }
            let mut cat = Command::new(&env.exe);
            cat.arg(crate::cli::FLAG)
                .args(["run", "--cwd"])
                .arg(project)
                .args(["--", "cat"])
                .arg(Path::new(&common).join(&relative))
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            let output = cat.output().map_err(|error| error.to_string())?;
            let found = hex(&Sha256::digest(&output.stdout));
            if !output.status.success() || found != *oid {
                return Err(format!(
                    "The LFS file {oid} isn't in this repository (run `git lfs fetch` first)."
                ));
            }
            if let Some(parent) = into.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let temp = into.with_extension("part");
            std::fs::write(&temp, &output.stdout).map_err(|error| error.to_string())?;
            std::fs::rename(&temp, &into).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// `bytes` in lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Whether git-lfs is installed.
pub fn lfs_installed() -> bool {
    Command::new("git-lfs")
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// An [`Env`] for tests: its sandbox is a stand-in in `root` that runs the command as it is.
#[cfg(test)]
pub fn test_env(root: &Path) -> Env {
    let exe = root.join("fake-pistachio");
    std::fs::write(
        &exe,
        "#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    Env {
        exe,
        dir: root.join("sandbox"),
        extra: Vec::new(),
    }
}
