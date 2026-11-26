//! UI-agnostic presentation of tool calls, mirroring how pi's TUI labels them
//! (`read path:1-20`, `$ command`, `grep /pattern/ in path`, …).

use std::path::Path;

use serde_json::Value;

use crate::model::ToolBlock;

/// The one-line header of a tool call: a bold verb and a muted target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolHeader {
    pub title: String,
    pub detail: String,
}

/// Paths used to shorten displayed file paths.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathContext<'a> {
    pub cwd: Option<&'a Path>,
    pub home: Option<&'a Path>,
}

impl PathContext<'_> {
    /// Relative to the working folder when inside it, else `~`-shortened.
    pub fn display(&self, raw: &str) -> String {
        let path = Path::new(raw);
        if let Some(relative) = self.cwd.and_then(|cwd| path.strip_prefix(cwd).ok()) {
            let relative = relative.to_string_lossy();
            return if relative.is_empty() {
                ".".to_owned()
            } else {
                relative.into_owned()
            };
        }
        if let Some(relative) = self.home.and_then(|home| path.strip_prefix(home).ok()) {
            return format!("~/{}", relative.to_string_lossy());
        }
        raw.to_owned()
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn path_arg(args: &Value) -> Option<&str> {
    arg(args, "file_path").or_else(|| arg(args, "path"))
}

/// The file a finished `read`, `edit` or `write` call is about (as the model gave it), and
/// the line a `read` started at.
pub fn file_target(tool: &ToolBlock) -> Option<(&str, Option<u32>)> {
    if !matches!(tool.name.as_str(), "read" | "edit" | "write") || tool.args.is_null() {
        return None;
    }
    let path = path_arg(&tool.args).filter(|path| !path.is_empty())?;
    let line = if tool.name == "read" {
        tool.args
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|line| u32::try_from(line).ok())
    } else {
        None
    };
    Some((path, line))
}

pub fn header(tool: &ToolBlock, paths: PathContext<'_>) -> ToolHeader {
    let args = if tool.args.is_null() {
        // While arguments stream, try the partial JSON; it is often parseable.
        &serde_json::from_str(&tool.args_partial).unwrap_or(Value::Null)
    } else {
        &tool.args
    };
    let path =
        |fallback: &str| path_arg(args).map_or_else(|| fallback.to_owned(), |p| paths.display(p));
    // Titles are past tense, like the labels of recognized shell commands ("Listed",
    // "Ran"); "Read" reads both ways. Grep, Find and Bash are tool names.
    let (title, detail) = match tool.name.as_str() {
        "read" => ("Read", format!("{}{}", path(""), line_range(args))),
        "bash" => {
            // Labelled once the command is complete; raw while it streams or when unsure.
            let label = (!tool.args.is_null())
                .then(|| arg(args, "command").and_then(crate::command::summarize))
                .flatten();
            if let Some(summary) = label {
                return ToolHeader {
                    title: summary.title(),
                    detail: summary.detail(),
                };
            }
            ("Bash", first_line(arg(args, "command").unwrap_or("")))
        }
        "edit" => ("Edited", path("")),
        "write" => ("Wrote", path("")),
        "ls" => ("Listed", path(".")),
        "grep" => {
            let detail = format!("/{}/ in {}", arg(args, "pattern").unwrap_or(""), path("."));
            match arg(args, "glob") {
                Some(glob) => ("Grep", format!("{detail} ({glob})")),
                None => ("Grep", detail),
            }
        }
        "find" => (
            "Find",
            format!("{} in {}", arg(args, "pattern").unwrap_or(""), path(".")),
        ),
        // Deep search (pistachio's chat extension).
        "web_search" => ("Searched", arg(args, "query").unwrap_or("").to_owned()),
        "web_fetch" => ("Read", short_url(arg(args, "url").unwrap_or(""))),
        // The chats' python tool (Monty): its last line, usually the expression computed.
        "python" => ("Computed", last_line(arg(args, "code").unwrap_or(""))),
        other => {
            return ToolHeader {
                title: other.to_owned(),
                detail: compact_args(args),
            };
        }
    };
    ToolHeader {
        title: title.to_owned(),
        detail,
    }
}

/// What kind of step a tool call is, for the small icon at the start of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Reads a file.
    Read,
    /// Creates, changes, moves or removes files.
    Write,
    /// Lists folders.
    List,
    /// Searches or checks (grep, find, `which`, versions).
    Search,
    /// Fetches or searches the web.
    Web,
    /// Installs packages.
    Install,
    /// Git.
    Git,
    /// Runs a command or script.
    Shell,
    /// Anything else (extension and MCP tools).
    Other,
}

/// The kind of `tool`: by tool name, and for shell commands by what the command does (the
/// first step of a recognized command, as its title shows).
pub fn kind(tool: &ToolBlock) -> ToolKind {
    match tool.name.as_str() {
        "read" => ToolKind::Read,
        "write" | "edit" => ToolKind::Write,
        "ls" => ToolKind::List,
        "grep" | "find" => ToolKind::Search,
        "web_search" | "web_fetch" => ToolKind::Web,
        "python" => ToolKind::Shell,
        "bash" => arg(&tool.args, "command")
            .and_then(crate::command::summarize)
            .and_then(|summary| summary.actions.first().map(action_kind))
            .unwrap_or(ToolKind::Shell),
        _ => ToolKind::Other,
    }
}

fn action_kind(action: &crate::command::Action) -> ToolKind {
    use crate::command::Action;
    match action {
        Action::Read(_) => ToolKind::Read,
        Action::List(_) => ToolKind::List,
        Action::Find { .. } | Action::Search { .. } | Action::Check(_) => ToolKind::Search,
        Action::GitRead(_) | Action::Git(_) | Action::InitRepo => ToolKind::Git,
        Action::Install { .. } => ToolKind::Install,
        Action::CreateProject(_)
        | Action::MakeDir(_)
        | Action::Create(_)
        | Action::Write(_)
        | Action::Edit(_)
        | Action::Remove(_)
        | Action::Move(..)
        | Action::Copy(..) => ToolKind::Write,
        Action::Fetch(_) => ToolKind::Web,
        Action::Remote {
            inner: Some(inner), ..
        } => inner.actions.first().map_or(ToolKind::Shell, action_kind),
        _ => ToolKind::Shell,
    }
}

/// A URL without its scheme and `www.`, at most 80 characters.
fn short_url(url: &str) -> String {
    let short = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_end_matches('/');
    if short.chars().count() > 80 {
        format!("{}…", short.chars().take(79).collect::<String>())
    } else {
        short.to_owned()
    }
}

/// First non-empty line, with `…` when more lines follow (multi-line commands).
/// The last line of `text` that has something on it, trimmed.
fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn first_line(text: &str) -> String {
    let mut lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default().trim_start().to_owned();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

fn line_range(args: &Value) -> String {
    let offset = args.get("offset").and_then(Value::as_u64);
    let limit = args.get("limit").and_then(Value::as_u64);
    match (offset, limit) {
        (None, None) => String::new(),
        (start, None) => format!(":{}", start.unwrap_or(1)),
        (start, Some(limit)) => {
            let start = start.unwrap_or(1);
            format!(":{start}-{}", start + limit.saturating_sub(1))
        }
    }
}

fn compact_args(args: &Value) -> String {
    const MAX: usize = 120;
    let text = match args {
        Value::Null => String::new(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() > MAX {
        format!("{}…", single_line.chars().take(MAX).collect::<String>())
    } else {
        single_line
    }
}

/// A short result line shown under a collapsed tool call (`⎿ Read 42 lines`).
pub fn result_summary(tool: &ToolBlock) -> Option<String> {
    let lines = |text: &str| text.trim_end().lines().count();
    match tool.name.as_str() {
        // A picture comes back with a one-line note; the picture is what was read.
        "read" if !tool.images.is_empty() => Some(match tool.images.len() {
            1 => "Read an image".to_owned(),
            n => format!("Read {n} images"),
        }),
        "read" => Some(plural(lines(&tool.output), "line", "Read")),
        "write" => {
            arg(&tool.args, "content").map(|content| plural(lines(content), "line", "Wrote"))
        }
        "edit" => {
            let diff = diff_lines(tool)?;
            let added = diff
                .iter()
                .filter(|line| line.kind == DiffKind::Added)
                .count();
            let removed = diff
                .iter()
                .filter(|line| line.kind == DiffKind::Removed)
                .count();
            Some(format!(
                "Added {}, removed {}",
                count(added, "line"),
                count(removed, "line")
            ))
        }
        "grep" | "find" | "ls" => {
            let found = lines(&tool.output);
            Some(if found == 0 {
                "No results".to_owned()
            } else {
                count(found, "result")
            })
        }
        // What it printed or gave back: its last line, the result. A failure shows its error.
        "python" if tool.status == crate::ToolStatus::Failed => Some(last_line(&tool.output)),
        "python" => {
            let result = last_line(&tool.output);
            (!result.is_empty()).then(|| {
                const MAX: usize = 80;
                if result.chars().count() > MAX {
                    format!("= {}…", result.chars().take(MAX).collect::<String>())
                } else {
                    format!("= {result}")
                }
            })
        }
        "web_search" => {
            let results = tool.details.as_ref()?.get("results")?.as_array()?.len();
            Some(if results == 0 {
                "No results".to_owned()
            } else {
                count(results, "result")
            })
        }
        "web_fetch" => {
            let details = tool.details.as_ref()?;
            let title = details.get("title")?.as_str()?;
            let shown = details.get("sections_shown").and_then(Value::as_u64);
            let total = details.get("sections_total").and_then(Value::as_u64);
            Some(match (shown, total) {
                (Some(shown), Some(total)) if shown < total => {
                    format!("{title} ({shown} of {total} sections)")
                }
                _ => title.to_owned(),
            })
        }
        _ => None,
    }
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

fn plural(n: usize, noun: &str, verb: &str) -> String {
    format!("{verb} {}", count(n, noun))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
    /// `...` between hunks.
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub line_number: Option<u32>,
    pub text: String,
}

/// Parses the numbered diff pi's `edit` tool returns in `details.diff`
/// (lines like `+12     code`, `-12 code`, ` 13 code`, `      ...`).
pub fn diff_lines(tool: &ToolBlock) -> Option<Vec<DiffLine>> {
    let diff = tool.details.as_ref()?.get("diff")?.as_str()?;
    Some(diff.lines().map(parse_diff_line).collect())
}

fn parse_diff_line(line: &str) -> DiffLine {
    let mut chars = line.chars();
    let kind = match chars.next() {
        Some('+') => DiffKind::Added,
        Some('-') => DiffKind::Removed,
        _ => DiffKind::Context,
    };
    let rest = chars.as_str().trim_start();
    if rest == "..." || rest.is_empty() && kind == DiffKind::Context {
        return DiffLine {
            kind: DiffKind::Gap,
            line_number: None,
            text: String::new(),
        };
    }
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let line_number = rest[..digits].parse().ok();
    let text = rest[digits..].strip_prefix(' ').unwrap_or(&rest[digits..]);
    DiffLine {
        kind,
        line_number,
        text: text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolStatus;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn tool(name: &str, args: Value) -> ToolBlock {
        ToolBlock {
            call_id: "c".into(),
            name: name.into(),
            args,
            args_partial: String::new(),
            status: ToolStatus::Done,
            output: String::new(),
            details: None,
            duration_ms: None,
            images: Vec::new(),
            nested: Vec::new(),
        }
    }

    #[test]
    fn kinds_follow_the_tool_and_the_command() {
        let bash = |command: &str| tool("bash", json!({ "command": command }));
        let cases = [
            (tool("read", json!({"path":"a.rs"})), ToolKind::Read),
            (tool("edit", json!({"path":"a.rs"})), ToolKind::Write),
            (tool("write", json!({"path":"a.rs"})), ToolKind::Write),
            (tool("ls", json!({})), ToolKind::List),
            (tool("grep", json!({"pattern":"x"})), ToolKind::Search),
            (
                tool("web_fetch", json!({"url":"https://x.dev"})),
                ToolKind::Web,
            ),
            (tool("mcp__x", json!({})), ToolKind::Other),
            (tool("python", json!({"code":"1 + 1"})), ToolKind::Shell),
            (bash("ls -la"), ToolKind::List),
            (bash("cat Cargo.toml"), ToolKind::Read),
            (bash("rg foo src"), ToolKind::Search),
            (bash("mkdir -p js"), ToolKind::Write),
            (bash("git status"), ToolKind::Git),
            (bash("npm install"), ToolKind::Install),
            (bash("frobnicate --hard"), ToolKind::Shell),
        ];
        for (tool, expected) in cases {
            assert_eq!(kind(&tool), expected, "{} {}", tool.name, tool.args);
        }
    }

    #[test]
    fn headers_mirror_pi_tui() {
        let paths = PathContext {
            cwd: Some(Path::new("/w")),
            home: Some(Path::new("/h")),
        };
        let cases = [
            (
                tool("read", json!({"path":"/w/src/a.rs","offset":10,"limit":5})),
                "Read",
                "src/a.rs:10-14",
            ),
            (tool("bash", json!({"command":"ls -la"})), "Listed", "files"),
            (
                tool("python", json!({"code":"import math\nmath.log2(443)\n"})),
                "Computed",
                "math.log2(443)",
            ),
            (
                tool("bash", json!({"command":"frobnicate -x"})),
                "Bash",
                "frobnicate -x",
            ),
            (
                tool("edit", json!({"path":"/h/x.txt"})),
                "Edited",
                "~/x.txt",
            ),
            (
                tool("grep", json!({"pattern":"fn main","glob":"*.rs"})),
                "Grep",
                "/fn main/ in . (*.rs)",
            ),
            (tool("ls", json!({})), "Listed", "."),
            (
                tool("web_search", json!({"query":"dgx spark price"})),
                "Searched",
                "dgx spark price",
            ),
            (
                tool("web_fetch", json!({"url":"https://www.example.com/a/b/"})),
                "Read",
                "example.com/a/b",
            ),
            (tool("mcp__x", json!({"q":"hi"})), "mcp__x", "q: hi"),
        ];
        for (tool, title, detail) in cases {
            assert_eq!(
                header(&tool, paths),
                ToolHeader {
                    title: title.into(),
                    detail: detail.into()
                }
            );
        }
    }

    #[test]
    fn summarizes_deep_search_results() {
        let mut search = tool("web_search", json!({"query":"q"}));
        search.details = Some(json!({"results": [{"url":"a"}, {"url":"b"}]}));
        assert_eq!(result_summary(&search).unwrap(), "2 results");
        let mut fetch = tool("web_fetch", json!({"url":"https://e.com"}));
        fetch.details = Some(json!({"title":"Docs","sections_shown":3,"sections_total":9}));
        assert_eq!(result_summary(&fetch).unwrap(), "Docs (3 of 9 sections)");
    }

    #[test]
    fn multi_line_bash_header_shows_first_line() {
        let script = tool(
            "bash",
            json!({"command":"\nfrob > a.py << 'EOF'\nprint(1)\nEOF\npython3 a.py"}),
        );
        assert_eq!(
            header(&script, PathContext::default()).detail,
            "frob > a.py << 'EOF' …"
        );
        // Understood commands are labelled instead.
        let labelled = tool(
            "bash",
            json!({"command":"\ncat > a.py << 'EOF'\nprint(1)\nEOF\npython3 a.py"}),
        );
        let header = header(&labelled, PathContext::default());
        assert_eq!(
            (header.title.as_str(), header.detail.as_str()),
            ("Wrote", "a.py, ran a.py")
        );
    }

    #[test]
    fn header_uses_partial_args_while_streaming() {
        let mut streaming = tool("bash", Value::Null);
        streaming.args_partial = r#"{"command":"cargo test"}"#.into();
        assert_eq!(
            header(&streaming, PathContext::default()).detail,
            "cargo test"
        );
    }

    #[test]
    fn parses_pi_edit_diff_and_summarizes() {
        let mut edit = tool("edit", json!({"path":"hello.py"}));
        edit.details = Some(json!({
            "diff": " 1 def greet(name):\n-2     return f\"Hello, {name}\"\n+2     return f\"Hello, {name}!\"\n 3 \n      ..."
        }));
        let lines = diff_lines(&edit).unwrap();
        assert_eq!(
            lines[1],
            DiffLine {
                kind: DiffKind::Removed,
                line_number: Some(2),
                text: "    return f\"Hello, {name}\"".into()
            }
        );
        assert_eq!(lines[3].kind, DiffKind::Context);
        assert_eq!(lines[4].kind, DiffKind::Gap);
        assert_eq!(
            result_summary(&edit).unwrap(),
            "Added 1 line, removed 1 line"
        );
    }
}
