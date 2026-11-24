//! Composer input that pi's terminal handles itself rather than sending to the
//! model: built-in slash commands and `!`/`!!` shell commands.
//!
//! Built-in TUI commands are not available over RPC, so the GUI maps them onto
//! RPC calls. Extension commands, prompt templates and skills (`/skill:name`) are
//! sent to pi unchanged, exactly as the terminal does.

use gpui_kit::{App, ClipboardItem, Entity, Window};
use pi_rpc::protocol::{Model, ThinkingLevel};
use transcript::{BlockKind, ItemKind, NoticeKind};

use super::dialogs::{PickerItem, open_picker};
use super::session::{AfterFork, PiSession};
use crate::actions::{NewChat, ResumeSession};

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
    (
        "fork",
        "Branch from an earlier message (opens in a new session)",
    ),
    ("clone", "Continue in a copy of this conversation"),
    ("resume", "Open an earlier session"),
    ("export", "Export the conversation as HTML (optional path)"),
    (
        "share",
        "Share the conversation as a secret GitHub gist (gh)",
    ),
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

/// What `/model` found for the text after it.
#[derive(Debug, PartialEq)]
enum ModelMatch<'a> {
    One(&'a Model),
    /// Several models fit equally well; the user has to say which.
    Several(Vec<&'a Model>),
    None,
}

/// Finds the model `text` means: its qualified id (`provider/id`), its id or its display name,
/// else the only one that starts with the text, else the only one that contains it. An id (or
/// name) that several providers offer means the current provider's, if that has exactly one.
fn find_model<'a>(text: &str, models: &'a [Model], current: Option<&Model>) -> ModelMatch<'a> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return ModelMatch::None;
    }
    let find = |fits: &dyn Fn(&str) -> bool, fields: &dyn Fn(&Model) -> Vec<String>| {
        models
            .iter()
            .filter(|model| {
                fields(model)
                    .iter()
                    .any(|field| fits(&field.to_lowercase()))
            })
            .collect::<Vec<_>>()
    };
    let all = |model: &Model| vec![model.qualified_id(), model.id.clone(), model.name.clone()];
    let exact = |field: &str| field == text;
    for fields in [
        &Model::qualified_id as &dyn Fn(&Model) -> String,
        &|model: &Model| model.id.clone(),
        &|model: &Model| model.name.clone(),
    ] {
        let found = find(&exact, &|model| vec![fields(model)]);
        let own: Vec<_> = found
            .iter()
            .copied()
            .filter(|model| current.is_some_and(|current| current.provider == model.provider))
            .collect();
        match (found.as_slice(), own.as_slice()) {
            ([], _) => {}
            ([model], _) | (_, [model]) => return ModelMatch::One(model),
            _ => return ModelMatch::Several(found),
        }
    }
    let starting = find(&|field| field.starts_with(&text), &all);
    let found = if starting.is_empty() {
        find(&|field| field.contains(&text), &all)
    } else {
        starting
    };
    match found.as_slice() {
        [] => ModelMatch::None,
        [model] => ModelMatch::One(model),
        _ => ModelMatch::Several(found),
    }
}

/// How many models an ambiguous `/model` lists.
const MODELS_LISTED: usize = 8;

/// The notice for an ambiguous `/model`: the models it could mean, as the ids that select them.
fn several_models(text: &str, models: &[&Model]) -> String {
    let mut lines = vec![format!(
        "Several models match “{}”. Type more of the name, or one of:",
        text.trim()
    )];
    lines.extend(
        models
            .iter()
            .take(MODELS_LISTED)
            .map(|model| model.qualified_id()),
    );
    if models.len() > MODELS_LISTED {
        lines.push(format!("… and {} more", models.len() - MODELS_LISTED));
    }
    lines.join("\n")
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
        Parsed::Shell { .. } if session.read(cx).mode() == pi_sessions::Mode::Chat => {
            notice(
                session,
                NoticeKind::Info,
                "Shell commands are not available in chat mode. Switch to Code to work in a folder."
                    .to_owned(),
                cx,
            );
            CommandOutcome::Keep
        }
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
        "thinking" => return set_thinking(args, session, cx),
        "model" => return set_model(args, session, cx),
        "session" => {
            let text = session_summary(session.read(cx));
            notice(session, NoticeKind::Info, text, cx);
        }
        "fork" => pick_fork(session, window, cx),
        "clone" => session.update(cx, |session, cx| session.clone_session(cx)),
        "resume" => window.dispatch_action(Box::new(ResumeSession), cx),
        "export" => {
            let path = (!args.is_empty()).then(|| args.to_owned());
            session.update(cx, |session, cx| session.export_html(path, cx));
        }
        "share" => session.update(cx, |session, cx| session.share(cx)),
        "tree" => notice(
            session,
            NoticeKind::Info,
            "Branches open as their own sessions in the GUI: use /fork, or Edit, Retry and \
             Branch from here on a message. Moving within one session's tree is only \
             available in pi's terminal."
                .into(),
            cx,
        ),
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

/// `/thinking <level>`: one of the levels the model offers (any of pi's while those are not
/// known yet).
fn set_thinking(args: &str, session: &Entity<PiSession>, cx: &mut App) -> CommandOutcome {
    let offered = match session.read(cx).thinking_levels() {
        [] => ThinkingLevel::ALL.to_vec(),
        levels => levels.to_vec(),
    };
    if let Some(level) = offered
        .iter()
        .copied()
        .find(|level| level.as_str().eq_ignore_ascii_case(args))
    {
        session.update(cx, |session, cx| session.set_thinking_level(level, cx));
        return CommandOutcome::Handled;
    }
    let offered: Vec<_> = offered.iter().map(|level| level.as_str()).collect();
    notice(
        session,
        NoticeKind::Warning,
        format!("Usage: /thinking <{}>", offered.join(", ")),
        cx,
    );
    CommandOutcome::Keep
}

/// `/model <text>`: switches to the model it means, or says why it can't tell.
fn set_model(args: &str, session: &Entity<PiSession>, cx: &mut App) -> CommandOutcome {
    let found = {
        let session = session.read(cx);
        match find_model(args, session.models(), session.model()) {
            ModelMatch::One(model) => Ok(model.clone()),
            ModelMatch::Several(models) => Err(several_models(args, &models)),
            ModelMatch::None if args.is_empty() => Err(MODEL_USAGE.to_owned()),
            ModelMatch::None => Err(format!("No model matches “{args}”. {MODEL_USAGE}")),
        }
    };
    match found {
        Ok(model) => {
            session.update(cx, |session, cx| session.set_model(&model, cx));
            CommandOutcome::Handled
        }
        Err(text) => {
            notice(session, NoticeKind::Warning, text, cx);
            CommandOutcome::Keep
        }
    }
}

const MODEL_USAGE: &str = "Usage: /model <provider/id or name> — or use the model picker.";

/// `/fork`: pick one of pi's user messages; the branch starts before it, with its text in
/// the composer to edit and send.
fn pick_fork(session: &Entity<PiSession>, window: &mut Window, cx: &mut App) {
    if session.read(cx).is_busy() {
        notice(
            session,
            NoticeKind::Info,
            "Wait for the current response to finish before branching.".into(),
            cx,
        );
        return;
    }
    let task = session.update(cx, |session, cx| session.fork_messages(cx));
    let session = session.clone();
    window
        .spawn(cx, async move |cx| {
            let messages = task.await;
            let _ = cx.update(|window, cx| match messages {
                Ok(messages) if messages.is_empty() => notice(
                    &session,
                    NoticeKind::Info,
                    "There is no earlier message to branch from.".into(),
                    cx,
                ),
                Ok(messages) => {
                    // Newest first, as in pi's terminal picker.
                    let messages: Vec<_> = messages.into_iter().rev().collect();
                    let items = messages
                        .iter()
                        .map(|message| PickerItem {
                            label: first_line(&message.text),
                            detail: None,
                        })
                        .collect();
                    open_picker(
                        "Branch from a message",
                        items,
                        move |index, _, cx| {
                            let entry = messages[index].entry_id.clone();
                            session.update(cx, |session, cx| {
                                session.fork_entry_id(entry, AfterFork::Edit, cx);
                            });
                        },
                        window,
                        cx,
                    );
                }
                Err(error) => notice(&session, NoticeKind::Error, error.to_string(), cx),
            });
        })
        .detach();
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    line.chars().take(120).collect()
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
            source_info: None,
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

    fn model(provider: &str, id: &str, name: &str) -> Model {
        serde_json::from_value(serde_json::json!({
            "provider": provider, "id": id, "name": name,
        }))
        .unwrap()
    }

    #[test]
    fn finds_the_model_that_is_meant() {
        let models = [
            model("openai", "gpt-5-mini", "GPT-5 mini"),
            model("openai", "gpt-5", "GPT-5"),
            model("openrouter", "openai/gpt-5-nano", "GPT-5 nano"),
            model("anthropic", "claude-opus-5", "Claude Opus 5"),
        ];
        let found = |text: &str| match find_model(text, &models, None) {
            ModelMatch::One(model) => model.qualified_id(),
            ModelMatch::Several(models) => format!("{} models", models.len()),
            ModelMatch::None => "none".to_owned(),
        };
        // An exact id or name wins over one that only starts with it.
        assert_eq!(found("gpt-5"), "openai/gpt-5");
        assert_eq!(found("GPT-5"), "openai/gpt-5");
        assert_eq!(found("openai/gpt-5"), "openai/gpt-5");
        assert_eq!(found(" claude opus 5 "), "anthropic/claude-opus-5");
        // One prefix wins over models that contain the text elsewhere.
        assert_eq!(found("openai/gpt-5-n"), "openrouter/openai/gpt-5-nano");
        assert_eq!(found("gpt-5-m"), "openai/gpt-5-mini");
        assert_eq!(found("opus"), "anthropic/claude-opus-5");
        // Several: no guessing.
        assert_eq!(found("gpt"), "3 models");
        assert_eq!(found("mystery"), "none");
        assert_eq!(found(""), "none");
    }

    #[test]
    fn the_same_id_in_several_providers_is_ambiguous() {
        let models = [
            model("openai", "gpt-5", "GPT-5"),
            model("azure", "gpt-5", "GPT-5"),
            model("anthropic", "claude-opus-5", "Claude Opus 5"),
        ];
        let ModelMatch::Several(found) = find_model("gpt-5", &models, Some(&models[2])) else {
            panic!("ambiguous");
        };
        assert_eq!(
            several_models("gpt-5", &found),
            "Several models match “gpt-5”. Type more of the name, or one of:\n\
             openai/gpt-5\nazure/gpt-5"
        );
        // The current provider's is meant, when it has one.
        assert_eq!(
            find_model("gpt-5", &models, Some(&models[1])),
            ModelMatch::One(&models[1])
        );
        assert_eq!(
            find_model("azure/gpt-5", &models, Some(&models[0])),
            ModelMatch::One(&models[1])
        );
    }

    #[test]
    fn a_long_list_of_models_is_cut() {
        let models: Vec<Model> = (0..12)
            .map(|index| model("p", &format!("m{index}"), ""))
            .collect();
        let ModelMatch::Several(found) = find_model("m", &models, None) else {
            panic!("ambiguous");
        };
        let text = several_models("m", &found);
        assert_eq!(text.lines().count(), 1 + MODELS_LISTED + 1);
        assert!(text.ends_with("… and 4 more"), "{text}");
    }
}
