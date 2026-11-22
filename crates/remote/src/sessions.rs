//! pi's session files on a host, for the sidebar: listed over the shared connection, with
//! only new or changed files read again, and moved to the host's trash on request.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use pi_sessions::SessionSummary;

use crate::RemoteError;
use crate::ssh::{Ssh, sh_quote};

/// Prints `<mtime> <size> <path>` for every `<dir>/*/*.jsonl` (GNU or BSD `stat`).
const STAT_SCRIPT: &str = r#"d=$1
[ -d "$d" ] || exit 0
if stat -c %Y / >/dev/null 2>&1; then
  find "$d" -mindepth 2 -maxdepth 2 -type f -name '*.jsonl' -exec stat -c '%Y %s %n' {} +
else
  find "$d" -mindepth 2 -maxdepth 2 -type f -name '*.jsonl' -exec stat -f '%m %z %N' {} +
fi
exit 0"#;

/// For each file argument: `PISTACHIO_FILE`, its path, its first line, its last `session_info`
/// line and its first user-message line (cut to a few kB), all NUL-terminated.
const READ_SCRIPT: &str = r#"for f; do
  printf 'PISTACHIO_FILE\000%s\000' "$f"
  head -n 1 "$f" 2>/dev/null | cut -c 1-4000; printf '\000'
  grep '^{"type":"session_info"' "$f" 2>/dev/null | tail -n 1 | cut -c 1-4000; printf '\000'
  grep -m 1 '"role":"user"' "$f" 2>/dev/null | cut -c 1-8000; printf '\000'
done
exit 0"#;

/// Moves `$1` to the trash: `gio trash`, `trash-put`, `~/.Trash` on macOS, else the
/// freedesktop trash in `~/.local/share/Trash` (restorable from file managers).
const TRASH_SCRIPT: &str = r#"f=$1
[ -e "$f" ] || { printf 'No such file: %s\n' "$f" >&2; exit 1; }
if command -v gio >/dev/null 2>&1 && gio trash -- "$f" 2>/dev/null; then exit 0; fi
if command -v trash-put >/dev/null 2>&1 && trash-put -- "$f" 2>/dev/null; then exit 0; fi
b=${f##*/}
if [ "$(uname -s)" = Darwin ]; then
  n=$b; i=1
  while [ -e "$HOME/.Trash/$n" ]; do n="$b $i"; i=$((i + 1)); done
  mkdir -p "$HOME/.Trash" && mv -- "$f" "$HOME/.Trash/$n" && exit 0
fi
t=${XDG_DATA_HOME:-$HOME/.local/share}/Trash
mkdir -p "$t/files" "$t/info" || exit 1
n=$b; i=1
while [ -e "$t/files/$n" ] || [ -e "$t/info/$n.trashinfo" ]; do n="$b.$i"; i=$((i + 1)); done
printf '[Trash Info]\nPath=%s\nDeletionDate=%s\n' "$f" "$(date +%Y-%m-%dT%H:%M:%S)" > "$t/info/$n.trashinfo"
mv -- "$f" "$t/files/$n""#;

/// Files read per round trip (keeps the command line short).
const READ_BATCH: usize = 150;

/// What is known about one remote session file.
#[derive(Debug, Clone)]
struct Known {
    mtime: i64,
    size: u64,
    /// `None`: not a session file (remembered so it is not read again).
    summary: Option<SessionSummary>,
}

/// The session files of one host, remembered between refreshes.
#[derive(Debug, Default, Clone)]
pub struct SessionIndex {
    known: HashMap<String, Known>,
}

impl SessionIndex {
    /// Lists the sessions in `dir` (pi's sessions folder on the host), newest first. One round
    /// trip lists sizes and times; only files that are new or changed since the last refresh
    /// are read (just their first line, last title and first prompt).
    pub fn refresh(&mut self, ssh: &Ssh, dir: &str) -> Result<Vec<SessionSummary>, RemoteError> {
        let listing = ssh.exec(&format!("set -- {}\n{STAT_SCRIPT}", sh_quote(dir)))?;
        let stats = parse_stats(&listing);
        let changed: Vec<&(i64, u64, String)> = stats
            .iter()
            .filter(|(mtime, size, path)| {
                self.known
                    .get(path)
                    .is_none_or(|known| known.mtime != *mtime || known.size != *size)
            })
            .collect();
        let mut fresh: HashMap<String, Known> = HashMap::new();
        for batch in changed.chunks(READ_BATCH) {
            let mut script = String::from("set --");
            for (_, _, path) in batch {
                script.push(' ');
                script.push_str(&sh_quote(path));
            }
            script.push('\n');
            script.push_str(READ_SCRIPT);
            let output = ssh.exec_bytes(&script, Duration::from_secs(60))?;
            let by_path: HashMap<&str, &(i64, u64, String)> =
                batch.iter().map(|stat| (stat.2.as_str(), *stat)).collect();
            for (path, header, info, user) in parse_reads(&output.stdout) {
                let Some((mtime, size, _)) = by_path.get(path.as_str()) else {
                    continue;
                };
                let modified = time_of(*mtime);
                let summary = pi_sessions::summary_from_lines(
                    Path::new(&path),
                    modified,
                    &header,
                    Some(info.as_str()).filter(|line| !line.trim().is_empty()),
                    Some(user.as_str()).filter(|line| !line.trim().is_empty()),
                );
                fresh.insert(
                    path,
                    Known {
                        mtime: *mtime,
                        size: *size,
                        summary,
                    },
                );
            }
        }
        // Keep what still exists (unchanged entries) plus what was just read.
        let mut known: HashMap<String, Known> = HashMap::with_capacity(stats.len());
        for (_, _, path) in &stats {
            if let Some(entry) = fresh.remove(path).or_else(|| self.known.remove(path)) {
                known.insert(path.clone(), entry);
            }
        }
        self.known = known;
        let mut sessions: Vec<SessionSummary> = self
            .known
            .values()
            .filter_map(|known| known.summary.clone())
            .collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.modified));
        Ok(sessions)
    }

    /// Forgets one file (e.g. after trashing it) so the next refresh does not list it from
    /// memory.
    pub fn forget(&mut self, path: &str) {
        self.known.remove(path);
    }
}

impl Ssh {
    /// Moves a file on the host to its trash (see [`TRASH_SCRIPT`]).
    pub fn trash(&self, path: &str) -> Result<(), RemoteError> {
        self.exec(&format!("set -- {}\n{TRASH_SCRIPT}", sh_quote(path)))
            .map(drop)
    }
}

fn time_of(mtime: i64) -> SystemTime {
    u64::try_from(mtime).map_or(SystemTime::UNIX_EPOCH, |secs| {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    })
}

/// Parses [`STAT_SCRIPT`] lines.
fn parse_stats(text: &str) -> Vec<(i64, u64, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let mtime = parts.next()?.parse().ok()?;
            let size = parts.next()?.parse().ok()?;
            let path = parts.next()?.to_owned();
            path.starts_with('/').then_some((mtime, size, path))
        })
        .collect()
}

/// Parses [`READ_SCRIPT`] output into `(path, header, info, user)`.
fn parse_reads(stdout: &[u8]) -> Vec<(String, String, String, String)> {
    let fields: Vec<String> = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).trim().to_owned())
        .collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 4 < fields.len() {
        if fields[index] != "PISTACHIO_FILE" {
            index += 1;
            continue;
        }
        out.push((
            fields[index + 1].clone(),
            fields[index + 2].clone(),
            fields[index + 3].clone(),
            fields[index + 4].clone(),
        ));
        index += 5;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(script: &str, args: &[&str], home: &Path) -> (String, bool) {
        let mut full = String::from("set --");
        for arg in args {
            full.push(' ');
            full.push_str(&sh_quote(arg));
        }
        full.push('\n');
        full.push_str(script);
        let out = Command::new("/bin/sh")
            .args(["-c", &full])
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.success(),
        )
    }

    #[test]
    fn scripts_list_and_read_sessions_locally() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let group = root_path.join("--srv-app--");
        std::fs::create_dir_all(&group).unwrap();
        let file = group.join("2026_x.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"type":"session","version":3,"id":"x","timestamp":"t","cwd":"/srv/app"}"#,
                "\n",
                r#"{"type":"message","id":"1","parentId":null,"timestamp":"t","message":{"role":"user","content":"hi there"}}"#,
                "\n",
                r#"{"type":"session_info","id":"2","parentId":"1","timestamp":"t","name":"Old"}"#,
                "\n",
                r#"{"type":"session_info","id":"3","parentId":"2","timestamp":"t","name":"Newest"}"#,
                "\n"
            ),
        )
        .unwrap();
        std::fs::write(group.join("notes.txt"), "x").unwrap();

        let (listing, ok) = run(STAT_SCRIPT, &[root_path.to_str().unwrap()], &root_path);
        assert!(ok);
        let stats = parse_stats(&listing);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].2, file.to_str().unwrap());
        assert!(stats[0].1 > 0);

        let mut full = String::from("set -- ");
        full.push_str(&sh_quote(file.to_str().unwrap()));
        full.push('\n');
        full.push_str(READ_SCRIPT);
        let out = Command::new("/bin/sh")
            .args(["-c", &full])
            .output()
            .unwrap();
        let reads = parse_reads(&out.stdout);
        assert_eq!(reads.len(), 1);
        let (path, header, info, user) = &reads[0];
        let summary = pi_sessions::summary_from_lines(
            Path::new(path),
            time_of(stats[0].0),
            header,
            Some(info),
            Some(user),
        )
        .unwrap();
        assert_eq!(summary.title(), "Newest");
        assert_eq!(summary.first_prompt.as_deref(), Some("hi there"));

        // A missing sessions folder lists nothing.
        let (listing, ok) = run(STAT_SCRIPT, &["/no/such/dir"], &root_path);
        assert!(ok);
        assert_eq!(listing, "");
    }

    #[test]
    fn trash_script_uses_a_restorable_trash() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let file = home_path.join("s.jsonl");
        std::fs::write(&file, "x").unwrap();
        // Keep gio / trash-put out of the way so the built-in fallback is what runs.
        let script = format!("PATH=/usr/bin:/bin\n{TRASH_SCRIPT}");
        let (_, ok) = run(&script, &[file.to_str().unwrap()], &home_path);
        assert!(ok);
        assert!(!file.exists());
        let (_, ok) = run(&script, &[file.to_str().unwrap()], &home_path);
        assert!(!ok, "trashing a missing file fails");
    }

    #[test]
    fn parses_stat_lines() {
        assert_eq!(
            parse_stats("1700000000 12 /a b/c.jsonl\njunk\n5 x /y\n"),
            vec![(1_700_000_000, 12, "/a b/c.jsonl".to_owned())]
        );
    }
}
