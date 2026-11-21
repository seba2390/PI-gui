//! Composer input that pi's terminal handles itself rather than sending to the
//! model: built-in slash commands and `!`/`!!` shell commands.
//!
//! Built-in TUI commands are not available over RPC, so the GUI maps them onto
//! RPC calls. Extension commands, prompt templates and skills (`/skill:name`) are
//! sent to pi unchanged, exactly as the terminal does.

use gpui_kit::{App, ClipboardItem, Entity, Window};
use pi_rpc::protocol::ThinkingLevel;
use transcript::{BlockKind, ItemKind, NoticeKind};

use super::session::PiSession;
use crate::actions::NewChat;

pub enum CommandOutcome {
    /// Executed; clear the composer.
    Handled,
    /// Rejected (e.g. bad arguments); keep the text so the user can fix it.
    Keep,
    /// Ordinary prompt (or a pi-side command): send it to pi.
    NotACommand,
}

/// Built-in commands the GUI implements, with the help text shown in completion.
pub const SUPPORTED: &[(&str, &str)] = &[
    ("new", "Start a new chat"),
    (
        "compact",
        "Compact the conversation context (optional instructions)",
    ),
    ("name", "Name this chat"),
    ("model", "Switch model (provider/id or name)"),
    ("thinking", "Set the thinking level"),
    ("session", "Show session information"),
    ("copy", "Copy the last assistant reply"),
    ("quit", "Quit"),
];

/// One completion candidate for `/` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub description: String,
}

/// Completions for composer text that is a bare `/prefix` (no arguments yet):
/// GUI built-ins first, then pi's extension commands, prompt templates and skills.
pub fn suggestions(text: &str, pi_commands: &[pi_rpc::protocol::SlashCommand]) -> Vec<Suggestion> {
    let Some(prefix) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if prefix.contains(char::is_whitespace) {
        return Vec::new();
    }
    let prefix = prefix.to_lowercase();
    let builtins = SUPPORTED.iter().map(|(name, description)| Suggestion {
        name: (*name).to_owned(),
        description: (*description).to_owned(),
    });
    let from_pi = pi_commands.iter().map(|command| Suggestion {
        name: command.name.clone(),
        description: command
            .description
            .clone()
            .unwrap_or_else(|| command.source.clone()),
    });
    let mut matches: Vec<Suggestion> = builtins
        .chain(from_pi)
        .filter(|suggestion| suggestion.name.to_lowercase().starts_with(&prefix))
        .collect();
    matches.dedup_by(|a, b| a.name == b.name);
    matches.truncate(8);
    matches
}

/// Parsed form of composer text, independent of GPUI so it is unit-testable.
#[derive(Debug, PartialEq, Eq)]
enum Parsed<'a> {
    Shell {
        command: &'a str,
        exclude_from_context: bool,
    },
    Builtin {
        name: &'a str,
        args: &'a str,
    },
    Prompt,
}

const BUILTINS: &[&str] = &[
    "new",
    "compact",
    "name",
    "thinking",
    "model",
    "session",
    "copy",
    "clone",
    "quit",
    "exit",
    "settings",
    "scoped-models",
    "login",
    "logout",
    "llama",
    "resume",
    "tree",
    "fork",
    "import",
    "export",
    "share",
    "bug",
    "trust",
    "mcp",
    "reload",
    "hotkeys",
    "changelog",
];

fn parse(text: &str) -> Parsed<'_> {
    if let Some(command) = text.strip_prefix("!!") {
        return Parsed::Shell {
            command: command.trim(),
            exclude_from_context: true,
        };
    }
    if let Some(command) = text.strip_prefix('!') {
        return Parsed::Shell {
            command: command.trim(),
            exclude_from_context: false,
        };
    }
    if let Some(rest) = text.strip_prefix('/') {
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if BUILTINS.contains(&name) {
            return Parsed::Builtin {
                name,
                args: args.trim(),
            };
        }
    }
    Parsed::Prompt
}

pub fn handle(
    text: &str,
    session: &Entity<PiSession>,
    window: &mut Window,
    cx: &mut App,
) -> CommandOutcome {
    match parse(text) {
        Parsed::Prompt => CommandOutcome::NotACommand,
        Parsed::Shell { command: "", .. } => CommandOutcome::Keep,
        Parsed::Shell {
            command,
            exclude_from_context,
        } => {
            let command = command.to_owned();
            session.update(cx, |session, cx| {
                session.run_bash(command, exclude_from_context, cx);
            });
            CommandOutcome::Handled
        }
        Parsed::Builtin { name, args } => builtin(name, args, session, window, cx),
    }
}

fn notice(session: &Entity<PiSession>, kind: NoticeKind, text: String, cx: &mut App) {
    session.update(cx, |session, cx| session.push_notice(kind, text, cx));
}

fn builtin(
    name: &str,
    args: &str,
    session: &Entity<PiSession>,
    window: &mut Window,
    cx: &mut App,
) -> CommandOutcome {
    match name {
        "new" => window.dispatch_action(Box::new(NewChat), cx),
        "quit" | "exit" => cx.quit(),
        "compact" => {
            let instructions = (!args.is_empty()).then(|| args.to_owned());
            session.update(cx, |session, cx| session.compact(instructions, cx));
        }
        "name" if args.is_empty() => {
            let name = session
                .read(cx)
                .title()
                .unwrap_or_else(|| "(unnamed)".to_owned());
            notice(
                session,
                NoticeKind::Info,
                format!("Session name: {name}"),
                cx,
            );
        }
        "name" => {
            let name = args.to_owned();
            session.update(cx, |session, cx| session.rename(name, cx));
        }
        "thinking" => {
            let level = ThinkingLevel::ALL
                .into_iter()
                .find(|level| level.as_str() == args);
            match level {
                Some(level) => {
                    session.update(cx, |session, cx| session.set_thinking_level(level, cx));
                }
                None => {
                    let available = session
                        .read(cx)
                        .thinking_levels()
                        .iter()
                        .map(|level| level.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    notice(
                        session,
                        NoticeKind::Warning,
                        format!("Usage: /thinking <{available}>"),
                        cx,
                    );
                    return CommandOutcome::Keep;
                }
            }
        }
        "model" => {
            let needle = args.to_lowercase();
            let model = session
                .read(cx)
                .models()
                .iter()
                .find(|model| {
                    !needle.is_empty()
                        && (model.qualified_id().to_lowercase() == needle
                            || model.id.to_lowercase().contains(&needle)
                            || model.name.to_lowercase().contains(&needle))
                })
                .cloned();
            match model {
                Some(model) => session.update(cx, |session, cx| session.set_model(&model, cx)),
                None => {
                    notice(
                        session,
                        NoticeKind::Warning,
                        "Usage: /model <provider/id or name> — or use the model picker.".into(),
                        cx,
                    );
                    return CommandOutcome::Keep;
                }
            }
        }
        "session" => {
            let text = session_summary(session.read(cx));
            notice(session, NoticeKind::Info, text, cx);
        }
        "copy" => {
            let text = last_assistant_text(session.read(cx));
            match text {
                Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                None => notice(
                    session,
                    NoticeKind::Warning,
                    "No assistant message to copy.".into(),
                    cx,
                ),
            }
        }
        other => notice(
            session,
            NoticeKind::Warning,
            format!("/{other} is a pi terminal command that the GUI doesn't support yet."),
            cx,
        ),
    }
    CommandOutcome::Handled
}

fn session_summary(session: &PiSession) -> String {
    let mut lines = Vec::new();
    if let Some(file) = session.session_file() {
        lines.push(format!("File: {}", file.display()));
    }
    lines.push(format!("Folder: {}", session.cwd().display()));
    if let Some(model) = session.model() {
        lines.push(format!("Model: {}", model.qualified_id()));
    }
    if let Some((used, window)) = session.context_usage() {
        lines.push(format!(
            "Context: {} / {} tokens",
            transcript::format_tokens(used),
            transcript::format_tokens(window)
        ));
    }
    if let Some(cost) = session.total_cost() {
        lines.push(format!("Cost: ${cost:.4}"));
    }
    lines.join("\n")
}

fn last_assistant_text(session: &PiSession) -> Option<String> {
    session
        .transcript()
        .items()
        .iter()
        .rev()
        .find_map(|item| match &item.kind {
            ItemKind::Assistant(assistant) => {
                let text = assistant
                    .blocks
                    .iter()
                    .filter_map(|block| match &block.kind {
                        BlockKind::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (!text.trim().is_empty()).then_some(text)
            }
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_builtins_and_prompts() {
        assert_eq!(
            parse("!ls -la"),
            Parsed::Shell {
                command: "ls -la",
                exclude_from_context: false
            }
        );
        assert_eq!(
            parse("!! git status"),
            Parsed::Shell {
                command: "git status",
                exclude_from_context: true
            }
        );
        assert_eq!(
            parse("/compact keep the API notes"),
            Parsed::Builtin {
                name: "compact",
                args: "keep the API notes"
            }
        );
        assert_eq!(
            parse("/new"),
            Parsed::Builtin {
                name: "new",
                args: ""
            }
        );
        // Extension commands, templates and skills go to pi untouched.
        assert_eq!(parse("/skill:review src"), Parsed::Prompt);
        assert_eq!(parse("/fix-tests"), Parsed::Prompt);
        assert_eq!(parse("hello /new"), Parsed::Prompt);
    }

    #[test]
    fn suggests_builtins_then_pi_commands() {
        let pi = vec![pi_rpc::protocol::SlashCommand {
            name: "skill:commit".into(),
            description: Some("Write a commit".into()),
            source: "skill".into(),
        }];
        let names = |text| {
            suggestions(text, &pi)
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("/co"), vec!["compact", "copy"]);
        assert_eq!(names("/sk"), vec!["skill:commit"]);
        assert_eq!(names("/compact now"), Vec::<&str>::new());
        assert_eq!(names("hello"), Vec::<&str>::new());
        assert_eq!(names("/").len(), 8);
    }
}
