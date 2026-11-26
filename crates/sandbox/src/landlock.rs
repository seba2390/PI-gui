//! Linux, where bubblewrap can't run: the helper restricts itself with Landlock, the kernel's
//! sandbox for unprivileged processes, and then becomes the command, which keeps the
//! restrictions. Landlock only allows (it can't deny a folder inside an allowed one), so a
//! folder holding a private or hidden path is allowed entry by entry, without it
//! ([`allowed_roots`]).
//! Network rules need Linux 6.7; older kernels only get the file rules.

use std::path::{Path, PathBuf};

/// `roots` with every hidden path taken out: a root that is hidden is dropped, and one that holds
/// a hidden path is replaced by its entries (again without hidden ones). Links among those
/// entries are followed by the kernel, so one leading into a hidden path is dropped too.
/// Entries made later in such a folder are not allowed.
pub fn allowed_roots(roots: &[PathBuf], hidden: &[PathBuf]) -> Vec<PathBuf> {
    let is_hidden = |path: &Path| hidden.iter().any(|hidden| path.starts_with(hidden));
    let holds_hidden = |path: &Path| {
        hidden
            .iter()
            .any(|hidden| hidden.starts_with(path) && hidden != path)
    };
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(root) = stack.pop() {
        let real = root.canonicalize().unwrap_or_else(|_| root.clone());
        if is_hidden(&root) || is_hidden(&real) {
            continue;
        }
        if holds_hidden(&root) || holds_hidden(&real) {
            if let Ok(entries) = std::fs::read_dir(&real) {
                stack.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else {
            out.push(root);
        }
    }
    out.sort();
    out
}

/// What Landlock enforces once applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enforced {
    /// Internet connections are blocked too (when the policy asked for it).
    pub network: bool,
}

/// Restricts this process (and whatever it runs) to `policy`.
#[cfg(target_os = "linux")]
pub fn apply(policy: &crate::Policy) -> Result<Enforced, String> {
    use landlock::{
        ABI, Access, AccessFs, AccessNet, LandlockStatus, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, path_beneath_rules,
    };

    let abi = ABI::V5;
    let error = |error: landlock::RulesetError| error.to_string();
    let block_network = policy.network == crate::Network::Off;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(error)?;
    if block_network {
        ruleset = ruleset
            .handle_access(AccessNet::from_all(abi))
            .map_err(error)?;
    }
    // Everything but the private folders, and what is open in them.
    let mut closed = policy.hidden.clone();
    closed.extend(policy.private.iter().cloned());
    let mut read = allowed_roots(&[PathBuf::from("/")], &closed);
    read.extend(allowed_roots(&policy.open_in_private(), &policy.hidden));
    let mut writable: Vec<PathBuf> = policy.writable.clone();
    writable.push(PathBuf::from("/dev"));
    let writable = allowed_roots(&writable, &policy.hidden);
    let status = ruleset
        .create()
        .map_err(error)?
        .add_rules(path_beneath_rules(&read, AccessFs::from_read(abi)))
        .map_err(error)?
        .add_rules(path_beneath_rules(&writable, AccessFs::from_all(abi)))
        .map_err(error)?
        .restrict_self()
        .map_err(error)?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err(
            "this Linux kernel has no Landlock (it needs Linux 5.13 or later, \
                    with Landlock turned on)"
                .to_owned(),
        );
    }
    let network = block_network
        && matches!(
            status.landlock,
            LandlockStatus::Available { effective_abi, .. } if effective_abi >= ABI::V4
        );
    Ok(Enforced { network })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_holding_a_hidden_path_is_allowed_entry_by_entry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().canonicalize().expect("canonical");
        for folder in [".ssh", ".config/gh", ".config/git", "app"] {
            std::fs::create_dir_all(home.join(folder)).expect("mkdir");
        }
        std::fs::write(home.join(".bashrc"), "").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join(".ssh"), home.join("keys")).expect("symlink");
        let hidden = [home.join(".ssh"), home.join(".config/gh")];
        let roots = allowed_roots(std::slice::from_ref(&home), &hidden);
        let mut expected = vec![
            home.join(".bashrc"),
            home.join(".config/git"),
            home.join("app"),
        ];
        expected.sort();
        assert_eq!(roots, expected);
        // A root inside a hidden folder is dropped; one without hidden paths stays whole.
        assert_eq!(
            allowed_roots(&[home.join(".ssh/inner"), home.join("app")], &hidden),
            vec![home.join("app")]
        );
    }
}
