//! Signing commits and tags with the user's key, whose files the sandbox hides. git in the
//! sandbox calls `gpg` (or `ssh-keygen` for SSH signing) as usual; those wrappers hand exactly
//! git's signing call to the gate, which runs the user's own program outside. Only a commit or a
//! tag is signed, in git's namespace; other uses of the programs stay in the sandbox.

use std::path::Path;

use super::{Class, Plan, Request};

/// The plan for a `gpg` or `ssh-keygen` request: git's signing call, else `None` (in the
/// sandbox).
pub fn plan(request: &Request) -> Result<Option<Plan>, String> {
    match request.tool.as_str() {
        "gpg" => gpg(request),
        "ssh-keygen" => ssh(request),
        _ => Ok(None),
    }
}

/// git's call: `gpg --status-fd=2 -bsau <key>`, with the commit or tag on standard input.
fn gpg(request: &Request) -> Result<Option<Plan>, String> {
    let [status, sign, key] = request.args.as_slice() else {
        return Ok(None);
    };
    if status != "--status-fd=2" || sign != "-bsau" || key.starts_with('-') {
        return Ok(None);
    }
    let payload = request.stdin.clone().unwrap_or_default();
    let what = describe(&payload)?;
    let program = user_setting("gpg.program").unwrap_or_else(|| "gpg".to_owned());
    Ok(Some(Plan {
        title: format!("Sign {what} with the key {key}"),
        details: Vec::new(),
        warnings: Vec::new(),
        class: Class::Sign,
        target: key.clone(),
        rememberable: true,
        program,
        args: request.args.clone(),
        cwd: request.cwd.clone(),
        stdin: Some(payload),
        accounts: Vec::new(),
        account: None,
    }))
}

/// git's call: `ssh-keygen -Y sign -n git -f <key> [-U] <file>`; the signature goes next to the
/// file, which is in a temporary folder.
fn ssh(request: &Request) -> Result<Option<Plan>, String> {
    let args = &request.args;
    if !args
        .windows(2)
        .any(|pair| pair[0] == "-Y" && pair[1] == "sign")
    {
        return Ok(None);
    }
    let mut namespace = None;
    let mut key = None;
    let mut file = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-Y" => {
                iter.next();
            }
            "-n" => namespace = iter.next().cloned(),
            "-f" => key = iter.next().cloned(),
            "-U" => {}
            other if other.starts_with('-') => {
                return Err(format!(
                    "Pistachio signs with SSH outside the sandbox only as git asks; `{other}` isn't part of that."
                ));
            }
            other => file = Some(other.to_owned()),
        }
    }
    let (Some(namespace), Some(key), Some(file)) = (namespace, key, file) else {
        return Err("Pistachio signs with SSH outside the sandbox only as git asks (-n git -f <key> <file>).".to_owned());
    };
    if namespace != "git" {
        return Err(format!(
            "Pistachio signs only for git, not for `{namespace}`."
        ));
    }
    let file_path = request.cwd.join(&file);
    if !in_temp(&file_path) {
        return Err("Pistachio signs only what git puts in a temporary file.".to_owned());
    }
    let payload = std::fs::read_to_string(&file_path)
        .map_err(|error| format!("Can't read what to sign ({file}): {error}"))?;
    let what = describe(&payload)?;
    // git writes a literal key (`key::…`) to a temporary file; show the key itself then.
    let key_path = request.cwd.join(&key);
    let shown_key = if in_temp(&key_path) {
        std::fs::read_to_string(&key_path)
            .ok()
            .map_or_else(|| key.clone(), |text| short_key(&text))
    } else {
        crate::paths::tilde(&key_path)
    };
    let program = user_setting("gpg.ssh.program").unwrap_or_else(|| "ssh-keygen".to_owned());
    Ok(Some(Plan {
        title: format!("Sign {what} with the SSH key {shown_key}"),
        details: Vec::new(),
        warnings: Vec::new(),
        class: Class::Sign,
        target: shown_key,
        rememberable: true,
        program,
        args: args.clone(),
        cwd: request.cwd.clone(),
        stdin: None,
        accounts: Vec::new(),
        account: None,
    }))
}

/// "the commit Fix the parser" or "the tag v1.2", from git's object text; anything else is
/// refused (the key signs commits and tags, not whatever a command hands it).
fn describe(payload: &str) -> Result<String, String> {
    let (headers, message) = payload.split_once("\n\n").unwrap_or((payload, ""));
    let subject = message.lines().next().unwrap_or_default().trim();
    if headers.starts_with("tree ") {
        Ok(format!("the commit \u{201c}{subject}\u{201d}"))
    } else if headers.starts_with("object ") {
        let tag = headers
            .lines()
            .find_map(|line| line.strip_prefix("tag "))
            .unwrap_or("a tag");
        Ok(format!("the tag {tag}"))
    } else {
        Err("Pistachio signs only git commits and tags with the user's key.".to_owned())
    }
}

/// The start and end of a public key, enough to tell it apart.
fn short_key(text: &str) -> String {
    let mut parts = text.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    let tail: String = body
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{kind} …{tail}")
}

/// Whether `path` is in a temporary folder.
fn in_temp(path: &Path) -> bool {
    let path = crate::paths::resolve(path);
    crate::policy::Context::current()
        .temp
        .iter()
        .any(|temp| path.starts_with(crate::paths::resolve(temp)))
}

/// The user's own git setting (never the repository's).
fn user_setting(key: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", "--get", key])
        .env_remove("GIT_DIR")
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const COMMIT: &str = "tree 1234\nparent 5678\nauthor a <a@b> 1 +0000\ncommitter a <a@b> 1 +0000\n\nFix the parser\n\nMore.\n";

    fn request(tool: &str, args: &[&str], stdin: Option<&str>) -> Request {
        Request {
            tool: tool.into(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            cwd: PathBuf::from("/work/app"),
            project: Some(PathBuf::from("/work/app")),
            stdin: stdin.map(str::to_owned),
            account: None,
        }
    }

    #[test]
    fn gits_gpg_call_is_signed_outside() {
        let plan = gpg(&request(
            "gpg",
            &["--status-fd=2", "-bsau", "ABCD1234"],
            Some(COMMIT),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.title,
            "Sign the commit \u{201c}Fix the parser\u{201d} with the key ABCD1234"
        );
        assert_eq!(
            (plan.class, plan.target.as_str()),
            (Class::Sign, "ABCD1234")
        );
        assert_eq!(plan.stdin.as_deref(), Some(COMMIT));
        // Other uses of gpg stay in the sandbox; signing other things is refused.
        assert_eq!(gpg(&request("gpg", &["--verify", "x.sig"], None)), Ok(None));
        assert!(
            gpg(&request(
                "gpg",
                &["--status-fd=2", "-bsau", "K"],
                Some("hello")
            ))
            .is_err()
        );
    }

    #[test]
    fn ssh_signing_only_for_git_in_a_temporary_file() {
        let temp = std::env::temp_dir().join(format!("pistachio-sign-test-{}", std::process::id()));
        std::fs::create_dir_all(&temp).expect("mkdir");
        let file = temp.join("commit");
        std::fs::write(&file, COMMIT).expect("write");
        let key = temp.join("key");
        std::fs::write(&key, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabcdefgh a@b\n").expect("write");
        let args = [
            "-Y",
            "sign",
            "-n",
            "git",
            "-f",
            key.to_str().unwrap(),
            "-U",
            file.to_str().unwrap(),
        ];
        let plan = ssh(&request("ssh-keygen", &args, None)).unwrap().unwrap();
        assert_eq!(
            plan.title,
            "Sign the commit \u{201c}Fix the parser\u{201d} with the SSH key ssh-ed25519 …abcdefgh"
        );
        let other = [
            "-Y",
            "sign",
            "-n",
            "file",
            "-f",
            key.to_str().unwrap(),
            file.to_str().unwrap(),
        ];
        assert!(ssh(&request("ssh-keygen", &other, None)).is_err());
        assert_eq!(
            ssh(&request("ssh-keygen", &["-l", "-f", "x"], None)),
            Ok(None)
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
