//! `request_user_help`: the model hands the user a step the sandbox blocks (Settings → Tools →
//! Code). The extension asks with a `select` whose title is [`MARK`] and a [`Request`] as JSON,
//! and the answers [`DONE`] and [`DISMISS`]; the app shows it as a pill above the composer. The
//! tool runs nothing itself: the user does the step, or doesn't, and the model gets the answer.

use serde::Deserialize;

/// The tool's name.
pub const TOOL: &str = "request_user_help";
/// Set to `off` when the user switched the tool off; on otherwise.
pub const ENV: &str = "PISTACHIO_HELP_TOOL";
/// How a help request's `select` title starts.
pub const MARK: &str = "pistachio-help:";
/// The user did what was asked.
pub const DONE: &str = "Done";
/// The user won't.
pub const DISMISS: &str = "Dismiss";

/// What the model asks the user to do, and what the sandbox blocked before it asked.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Request {
    /// A few words: what the user should do.
    pub title: String,
    /// The model's explanation.
    #[serde(default)]
    pub reason: String,
    /// A command the user may run themselves, if the model has one.
    #[serde(default)]
    pub suggested_command: Option<String>,
    /// The command the sandbox blocked last in this run, as the extension saw it.
    #[serde(default)]
    pub blocked: Option<Blocked>,
}

/// A command the sandbox blocked: what ran, where, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Blocked {
    pub command: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub error: String,
}

impl Request {
    /// The request in a `select` title, if it is one.
    pub fn parse(title: &str) -> Option<Self> {
        let mut request: Self = serde_json::from_str(title.strip_prefix(MARK)?).ok()?;
        request.title = request.title.trim().to_owned();
        if request.title.is_empty() {
            return None;
        }
        request.reason = request.reason.trim().to_owned();
        request.suggested_command = request
            .suggested_command
            .map(|command| command.trim().to_owned())
            .filter(|command| !command.is_empty());
        request.blocked = request
            .blocked
            .filter(|blocked| !blocked.command.trim().is_empty());
        Some(request)
    }

    /// What Copy command copies: the suggested command, else the blocked one.
    pub fn command_to_copy(&self) -> Option<&str> {
        self.suggested_command.as_deref().or_else(|| {
            self.blocked
                .as_ref()
                .map(|blocked| blocked.command.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_request_and_ignores_other_questions() {
        let title = format!(
            r#"{MARK}{{"title":" Install Xcode tools ","reason":"xcode-select is blocked.","suggested_command":"xcode-select --install","blocked":{{"command":"xcode-select --install","cwd":"/p","error":"Operation not permitted"}}}}"#
        );
        let request = Request::parse(&title).expect("a request");
        assert_eq!(request.title, "Install Xcode tools");
        assert_eq!(request.reason, "xcode-select is blocked.");
        assert_eq!(request.command_to_copy(), Some("xcode-select --install"));
        assert_eq!(request.blocked.expect("blocked").cwd, "/p");
        assert_eq!(Request::parse("Push 1 commit to github.com/a/b"), None);
        assert_eq!(Request::parse(&format!("{MARK}not json")), None);
        assert_eq!(Request::parse(&format!(r#"{MARK}{{"title":"  "}}"#)), None);
    }

    #[test]
    fn copies_the_suggested_command_else_the_blocked_one() {
        let parse = |json: &str| Request::parse(&format!("{MARK}{json}")).expect("a request");
        let blocked = parse(
            r#"{"title":"t","suggested_command":" ","blocked":{"command":"brew install jq"}}"#,
        );
        assert_eq!(blocked.suggested_command, None);
        assert_eq!(blocked.command_to_copy(), Some("brew install jq"));
        let nothing = parse(r#"{"title":"t","blocked":{"command":""}}"#);
        assert_eq!(nothing.blocked, None);
        assert_eq!(nothing.command_to_copy(), None);
    }
}
