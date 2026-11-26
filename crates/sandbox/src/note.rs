//! What a tool result says when the sandbox blocked it: the extension appends a note to a
//! failed command's output ([`NOTE`]), and the file tools' refusals name the sandbox. The app
//! shows such results with what was blocked and a way to allow it.

use std::path::PathBuf;

/// The start of the note the extension appends to a command that failed on the sandbox (keep
/// in step with `extension.ts`).
pub const NOTE: &str = "\n\n[Pistachio] This command ran in a sandbox";

/// Whether a tool's output says the sandbox blocked it.
pub fn is_blocked(output: &str) -> bool {
    output.contains(NOTE.trim_start()) || output.contains("Pistachio's sandbox")
}

/// The output without the note for the model.
pub fn without_note(output: &str) -> &str {
    output.find(NOTE).map_or(output, |at| &output[..at])
}

/// The absolute paths a command's error messages say it was not allowed to use, in order.
pub fn blocked_paths(output: &str) -> Vec<PathBuf> {
    const REASONS: &[&str] = &[
        ": Operation not permitted",
        ": Read-only file system",
        ": Permission denied",
    ];
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in without_note(output).lines() {
        let Some(before) = REASONS
            .iter()
            .find_map(|reason| line.find(reason).map(|at| &line[..at]))
        else {
            continue;
        };
        // `cannot touch '/x/y'` (GNU) or `touch: /x/y` (BSD, bash).
        let path = match before.strip_suffix('\'') {
            Some(quoted) => quoted.rfind('\'').map(|at| &quoted[at + 1..]),
            None => Some(before.rsplit(": ").next().unwrap_or(before)),
        };
        if let Some(path) = path.map(str::trim).filter(|path| path.starts_with('/')) {
            let path = PathBuf::from(path);
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_paths_in_common_error_messages() {
        let output = "touch: /Users/ana/out.txt: Operation not permitted\n\
                      /bin/bash: /Users/ana/log: Operation not permitted\n\
                      mkdir: cannot create directory '/home/ana/x y': Read-only file system\n\
                      bash: line 1: /etc/hosts: Permission denied\n\
                      cat: relative.txt: Operation not permitted\n\
                      touch: /Users/ana/out.txt: Operation not permitted";
        assert_eq!(
            blocked_paths(output),
            [
                "/Users/ana/out.txt",
                "/Users/ana/log",
                "/home/ana/x y",
                "/etc/hosts"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn the_note_is_recognised_and_left_out() {
        let output = format!("touch: /x: Operation not permitted{NOTE}: it can write only in /p.");
        assert!(is_blocked(&output));
        assert_eq!(without_note(&output), "touch: /x: Operation not permitted");
        assert!(is_blocked(
            "~/.ssh/id is hidden by Pistachio's sandbox (/Users/a/.ssh)."
        ));
        assert!(!is_blocked("touch: /x: Operation not permitted"));
    }
}
