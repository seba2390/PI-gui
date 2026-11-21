//! Error type of the crate, and classification of `ssh`'s stderr into user-facing failures.

use thiserror::Error;

/// Why talking to a remote host failed. The `Display` text is meant to be shown to the user.
#[derive(Debug, Error)]
pub enum RemoteError {
    /// The host's key is not in `known_hosts` yet. Show the fingerprint
    /// ([`Ssh::host_key_fingerprint`](crate::Ssh::host_key_fingerprint)) and, once the user
    /// trusts it, retry with [`Ssh::connect(true)`](crate::Ssh::connect).
    #[error("This host's key is not trusted yet. Check its fingerprint before connecting.")]
    HostKeyUnknown,

    /// The host presented a different key than the one recorded in `known_hosts`.
    #[error(
        "The host key has changed since you last connected. Someone may be intercepting the \
         connection, or the server was reinstalled. If the change is expected, remove the old \
         key with `ssh-keygen -R <host>` and try again."
    )]
    HostKeyChanged,

    /// The server refused every key on offer. Batch mode rules out password and passphrase
    /// prompts, so the key must be usable without one. Holds ssh's own message.
    #[error(
        "Authentication failed. Use a key without a passphrase, or add it to ssh-agent \
         (ssh-add --apple-use-keychain <key>)."
    )]
    AuthFailed(String),

    /// The host could not be reached: unknown name, refused, timed out, no route, or the
    /// connection dropped. Holds ssh's own message.
    #[error("Could not reach the host: {0}")]
    Unreachable(String),

    /// Anything else, e.g. a remote command that failed. Holds the (cleaned) stderr.
    #[error("{0}")]
    Failed(String),

    /// `ssh` (or a helper such as `ssh-keyscan`) could not be run at all.
    #[error("Could not run ssh: {0}")]
    Io(#[from] std::io::Error),
}

/// Turns the stderr of a failed `ssh` invocation into a [`RemoteError`].
///
/// Pure; matches the messages OpenSSH prints for host-key, authentication and network
/// failures, and falls back to [`RemoteError::Failed`] with the noise-free stderr.
pub fn classify(stderr: &str) -> RemoteError {
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || stderr.contains("POSSIBLE DNS SPOOFING DETECTED")
        || (stderr.contains("Host key for") && stderr.contains("has changed"))
    {
        return RemoteError::HostKeyChanged;
    }
    if stderr.contains("Host key verification failed") {
        return RemoteError::HostKeyUnknown;
    }

    let lines = meaningful_lines(stderr);
    let find = |needles: &[&str]| {
        lines
            .iter()
            .find(|line| needles.iter().any(|needle| line.contains(needle)))
            .map(|line| strip_ssh_prefix(line).to_owned())
    };

    if let Some(line) = find(&[
        "Permission denied (",
        "Too many authentication failures",
        "No more authentication methods",
    ]) {
        return RemoteError::AuthFailed(line);
    }
    if let Some(line) = find(&[
        "Could not resolve hostname",
        "Name or service not known",
        "Temporary failure in name resolution",
        "nodename nor servname",
        "Connection refused",
        "Operation timed out",
        "Connection timed out",
        "No route to host",
        "Network is unreachable",
        "Host is down",
        "Connection reset by",
        "Connection closed by",
        "closed by remote host",
        "kex_exchange_identification",
        "ssh_exchange_identification",
        "not responding",
        "Broken pipe",
    ]) {
        return RemoteError::Unreachable(line);
    }

    if lines.is_empty() {
        RemoteError::Failed("ssh exited without an error message".to_owned())
    } else {
        RemoteError::Failed(lines.join("\n"))
    }
}

/// stderr lines minus blank lines and informational chatter ssh prints even on success.
pub(crate) fn meaningful_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("** ")
                && !line.starts_with("Warning: Permanently added")
                && !line.starts_with("Pseudo-terminal will not be allocated")
        })
        .collect()
}

fn strip_ssh_prefix(line: &str) -> &str {
    line.strip_prefix("ssh: ").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn unknown_host_key() {
        assert!(matches!(
            classify("Host key verification failed.\n"),
            RemoteError::HostKeyUnknown
        ));
        let strict = "No ED25519 host key is known for dgx.example.com and you have requested \
                      strict checking.\nHost key verification failed.\n";
        assert!(matches!(classify(strict), RemoteError::HostKeyUnknown));
    }

    #[test]
    fn changed_host_key() {
        let stderr = "\
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!
The fingerprint for the ED25519 key sent by the remote host is
SHA256:voqbH8vJ0HAh163ILLUcx2s0xPOE39i5E18fofd1oNA.
Offending ED25519 key in /Users/me/.ssh/known_hosts:12
Host key for dgx.example.com has changed and you have requested strict checking.
Host key verification failed.
";
        assert!(matches!(classify(stderr), RemoteError::HostKeyChanged));
    }

    #[test]
    fn auth_failure() {
        let error = classify("me@dgx.example.com: Permission denied (publickey,password).\n");
        let RemoteError::AuthFailed(detail) = &error else {
            panic!("expected AuthFailed, got {error:?}");
        };
        assert_eq!(
            detail,
            "me@dgx.example.com: Permission denied (publickey,password)."
        );
        assert!(error.to_string().starts_with("Authentication failed."));

        assert!(matches!(
            classify(
                "Received disconnect from 10.0.0.1 port 22:2: Too many authentication failures"
            ),
            RemoteError::AuthFailed(_)
        ));
    }

    #[test]
    fn unreachable() {
        for (stderr, expected) in [
            (
                "ssh: Could not resolve hostname nope.invalid: nodename nor servname provided, \
                 or not known\n",
                "Could not resolve hostname nope.invalid: nodename nor servname provided, or not \
                 known",
            ),
            (
                "ssh: Could not resolve hostname nope: Name or service not known\n",
                "Could not resolve hostname nope: Name or service not known",
            ),
            (
                "ssh: connect to host 127.0.0.1 port 1: Connection refused\n",
                "connect to host 127.0.0.1 port 1: Connection refused",
            ),
            (
                "ssh: connect to host 10.255.255.1 port 22: Operation timed out\n",
                "connect to host 10.255.255.1 port 22: Operation timed out",
            ),
            (
                "ssh: connect to host 10.0.0.9 port 22: No route to host\n",
                "connect to host 10.0.0.9 port 22: No route to host",
            ),
            (
                "kex_exchange_identification: Connection closed by remote host\n\
                 Connection closed by UNKNOWN port 65535\n",
                "kex_exchange_identification: Connection closed by remote host",
            ),
            (
                "Timeout, server dgx.example.com not responding.\n",
                "Timeout, server dgx.example.com not responding.",
            ),
        ] {
            match classify(stderr) {
                RemoteError::Unreachable(detail) => assert_eq!(detail, expected),
                other => panic!("{stderr:?} classified as {other:?}"),
            }
        }
    }

    #[test]
    fn other_failures_keep_meaningful_stderr() {
        let stderr = "** WARNING: connection is not using a post-quantum key exchange algorithm.\n\
                      ** This session may be vulnerable to \"store now, decrypt later\" attacks.\n\
                      Warning: Permanently added 'dgx' (ED25519) to the list of known hosts.\n\
                      bash: line 1: pi: command not found\n";
        match classify(stderr) {
            RemoteError::Failed(detail) => {
                assert_eq!(detail, "bash: line 1: pi: command not found");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(classify(""), RemoteError::Failed(_)));
    }
}
