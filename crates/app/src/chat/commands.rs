//! Composer input that pi's terminal handles itself rather than sending to the
//! model: built-in slash commands and `!`/`!!` shell commands.
//!
//! Built-in TUI commands are not available over RPC, so the GUI maps them onto
//! RPC calls. Extension commands, prompt templates and skills (`/skill:name`) are
//! sent to pi unchanged, exactly as the terminal does.

use std::collections::HashSet;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, ClipboardItem, Entity, Window};
use pi_rpc::protocol::{Model, ThinkingLevel};
use transcript::{ItemKind, NoticeKind};

use super::dialogs::{PickerItem, open_picker};
use super::session::{AfterFork, Lifecycle, PiSession};
use crate::actions::{
    NewChat, OpenLogin, OpenSettings, OpenSideChat, ReloadSession, ResumeSession,
};

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
    (
        "side",
        "Open a side chat with everything here so far, next to this one",
    ),
    ("resume", "Open an earlier session"),
    ("export", "Export the conversation as HTML (optional path)"),
    (
        "share",
        "Share the conversation as a secret GitHub gist (gh)",
    ),
    ("settings", "Open Settings"),
    (
        "login",
        "Sign in to a provider in pi's terminal (optional provider)",
    ),
    (
        "logout",
        "Sign out of a provider in pi's terminal (optional provider)",
    ),
    (
        "reload",
        "Restart pi on this conversation (new sign-ins, extensions, skills)",
    ),
    ("quit", "Quit"),
];

/// Built-ins without arguments: Enter on their row runs them right away, as in pi's terminal.
/// `/share` (it uploads) and `/quit` are left out, so a half-typed name never runs them.
const RUN_AT_ONCE: &[&str] = &[
    "new", "session", "copy", "fork", "clone", "side", "resume", "settings", "reload",
];

/// One completion candidate for `/` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub description: String,
}

impl Suggestion {
    /// The composer text that names this command.
    pub fn text(&self) -> String {
        format!("/{}", self.name)
    }

    /// Whether Enter on this row (with `typed` in the composer) runs the command instead of
    /// only completing it: it takes no arguments, and what was typed is the start of its name.
    pub fn runs_at_once(&self, typed: &str) -> bool {
        RUN_AT_ONCE.contains(&self.name.as_str()) && self.text().starts_with(typed)
    }
}

/// Completions for composer text that is a bare `/query` (no arguments yet): the command of
/// exactly that name, then names that start with the query, then names that contain it. Within
/// each, GUI built-ins come first, then pi's extension commands, prompt templates and skills in
/// pi's order.
pub fn suggestions(text: &str, pi_commands: &[pi_rpc::protocol::SlashCommand]) -> Vec<Suggestion> {
    let Some(query) = text.strip_prefix('/') else {
        return Vec::new();
    };
    // A built-in without a row (`/tree`) is complete as typed, whatever else contains its name.
    let has_row = SUPPORTED.iter().any(|(name, _)| *name == query);
    if query.contains(char::is_whitespace) || (BUILTINS.contains(&query) && !has_row) {
        return Vec::new();
    }
    let lowercase = query.to_lowercase();
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
    let mut seen = HashSet::new();
    let (mut exact, mut starting, mut containing) = (Vec::new(), Vec::new(), Vec::new());
    for suggestion in builtins.chain(from_pi) {
        if !seen.insert(suggestion.name.clone()) {
            continue;
        }
        let name = suggestion.name.to_lowercase();
        if suggestion.name == query {
            exact.push(suggestion);
        } else if name.starts_with(&lowercase) {
            starting.push(suggestion);
        } else if name.contains(&lowercase) {
            containing.push(suggestion);
        }
    }
    [exact, starting, containing].concat()
}

/// What the user did with the `/` list: the row they moved to, and whether they closed it.
#[derive(Debug, Default)]
pub struct SuggestionList {
    /// The row moved to with ↑ / ↓, by name; the first row until then.
    selected: Option<String>,
    /// The text Esc closed the list for; it stays closed until the text changes.
    closed_for: Option<String>,
}

impl SuggestionList {
    /// The list's rows for `text`; none after Esc.
    pub fn rows(
        &self,
        text: &str,
        pi_commands: &[pi_rpc::protocol::SlashCommand],
    ) -> Vec<Suggestion> {
        if self.closed_for.as_deref() == Some(text) {
            return Vec::new();
        }
        suggestions(text, pi_commands)
    }

    /// Index of the highlighted row.
    pub fn selected(&self, rows: &[Suggestion]) -> usize {
        self.selected
            .as_ref()
            .and_then(|name| rows.iter().position(|row| row.name == *name))
            .unwrap_or(0)
    }

    /// ↑ / ↓: moves the highlight one row, stopping at the ends. Returns its new index.
    pub fn move_selection(&mut self, rows: &[Suggestion], down: bool) -> usize {
        let current = self.selected(rows);
        let index = if down {
            (current + 1).min(rows.len().saturating_sub(1))
        } else {
            current.saturating_sub(1)
        };
        self.selected = rows.get(index).map(|row| row.name.clone());
        index
    }

    /// Esc: closes the list until the text changes.
    pub fn close(&mut self, text: &str) {
        self.closed_for = Some(text.to_owned());
    }

    /// The text changed: the highlight stays on its row while the list refines, unless the
    /// text now names a command (Enter then runs that one).
    pub fn text_changed(&mut self, text: &str, rows: &[Suggestion]) {
        if self.closed_for.as_deref() != Some(text) {
            self.closed_for = None;
        }
        let named = rows.first().is_some_and(|row| row.text() == text);
        if named
            || !rows
                .iter()
                .any(|row| Some(&row.name) == self.selected.as_ref())
        {
            self.selected = None;
        }
    }
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
    "side",
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
        Parsed::Shell { .. } if *session.read(cx).lifecycle() != Lifecycle::Ready => {
            let message = match session.read(cx).lifecycle() {
                Lifecycle::Exited { .. } => {
                    "pi is not running, so the command was kept. Reconnect or start a new session."
                }
                _ => "pi is still starting, so the command was kept. Run it again in a moment.",
            };
            window.push_notification(message, cx);
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
        "side" => window.dispatch_action(Box::new(OpenSideChat), cx),
        "resume" => window.dispatch_action(Box::new(ResumeSession), cx),
        "settings" => window.dispatch_action(Box::new(OpenSettings), cx),
        "reload" => window.dispatch_action(Box::new(ReloadSession), cx),
        "login" | "logout" => return login(name == "logout", args, session, window, cx),
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

/// `/login` and `/logout`, with or without a provider: in a terminal next to a Code session; a
/// chat has none, so it says where to do it.
fn login(
    logout: bool,
    args: &str,
    session: &Entity<PiSession>,
    window: &mut Window,
    cx: &mut App,
) -> CommandOutcome {
    let valid = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    if args.starts_with('-') || !args.chars().all(valid) {
        let name = if logout { "logout" } else { "login" };
        notice(
            session,
            NoticeKind::Warning,
            format!("Usage: /{name}, or /{name} <provider> (for example /{name} anthropic)"),
            cx,
        );
        return CommandOutcome::Keep;
    }
    if session.read(cx).has_folder_tools() {
        let provider = (!args.is_empty()).then(|| args.to_owned());
        window.dispatch_action(Box::new(OpenLogin { logout, provider }), cx);
    } else {
        notice(session, NoticeKind::Info, CHAT_LOGIN.to_owned(), cx);
    }
    CommandOutcome::Handled
}

const CHAT_LOGIN: &str = "Signing in (and out) happens in pi's terminal, which chats don't have. \
    Switch to Code and use /login in a session there, or run `pi` in a terminal and type /login. \
    For an API key or a model server of your own, choose “Add model…” in the model menu. \
    Afterwards, /reload makes this chat use it.";

/// `/thinking <level>`: one of the levels pi accepts for the model (any of pi's while those are
/// not known yet). The usage names the ones that differ, like the thinking menu.
fn set_thinking(args: &str, session: &Entity<PiSession>, cx: &mut App) -> CommandOutcome {
    let (accepted, shown) = {
        let session = session.read(cx);
        match session.accepted_thinking_levels() {
            [] => (ThinkingLevel::ALL.to_vec(), ThinkingLevel::ALL.to_vec()),
            levels => (levels.to_vec(), session.thinking_levels()),
        }
    };
    if let Some(level) = accepted
        .into_iter()
        .find(|level| level.as_str().eq_ignore_ascii_case(args))
    {
        session.update(cx, |session, cx| session.set_thinking_level(level, cx));
        return CommandOutcome::Handled;
    }
    let shown: Vec<_> = shown.iter().map(|level| level.as_str()).collect();
    notice(
        session,
        NoticeKind::Warning,
        format!("Usage: /thinking <{}>", shown.join(", ")),
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
            ItemKind::Assistant(assistant) => assistant.answer().filter(|text| !text.is_empty()),
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
        assert_eq!(names("/co"), vec!["compact", "copy", "skill:commit"]);
        assert_eq!(names("/sk"), vec!["skill:commit"]);
        assert_eq!(names("/compact now"), Vec::<&str>::new());
        assert_eq!(names("hello"), Vec::<&str>::new());
        assert_eq!(names("/path/like/this"), Vec::<&str>::new());
        // Every command is listed, not only the first few.
        assert_eq!(names("/").len(), SUPPORTED.len() + 1);
    }

    fn command(name: &str) -> pi_rpc::protocol::SlashCommand {
        pi_rpc::protocol::SlashCommand {
            name: name.into(),
            description: None,
            source: "skill".into(),
            source_info: None,
        }
    }

    fn names(rows: &[Suggestion]) -> Vec<&str> {
        rows.iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn ranks_exact_then_prefix_then_substring() {
        let pi = [
            command("skill:add-skill"),
            command("readd"),
            command("add-tests"),
            command("skill:add"),
            command("add"),
        ];
        assert_eq!(
            names(&suggestions("/add", &pi)),
            ["add", "add-tests", "skill:add-skill", "readd", "skill:add"]
        );
        assert_eq!(
            names(&suggestions("/SKILL:add", &pi)),
            ["skill:add-skill", "skill:add"]
        );
    }

    #[test]
    fn lists_each_name_once() {
        let pi = [command("fix"), command("model"), command("fix")];
        let rows = suggestions("/", &pi);
        assert_eq!(
            names(&rows).iter().filter(|name| **name == "fix").count(),
            1
        );
        let model: Vec<_> = rows.iter().filter(|row| row.name == "model").collect();
        assert_eq!(model.len(), 1);
        assert_eq!(model[0].description, "Switch model (provider/id or name)");
    }

    #[test]
    fn a_builtin_without_a_row_is_complete() {
        let pi = [command("skill:tree-view")];
        assert_eq!(suggestions("/tree", &pi), []);
        assert_eq!(names(&suggestions("/tre", &pi)), ["skill:tree-view"]);
    }

    #[test]
    fn only_typed_commands_without_arguments_run_at_once() {
        let row = |name: &str| Suggestion {
            name: name.into(),
            description: String::new(),
        };
        assert!(row("resume").runs_at_once("/re"));
        assert!(!row("resume").runs_at_once("/sum"));
        assert!(!row("compact").runs_at_once("/comp"));
        assert!(!row("quit").runs_at_once("/q"));
        assert!(!row("share").runs_at_once("/sha"));
        assert!(!row("skill:new").runs_at_once("/skill"));
    }

    #[test]
    fn the_highlight_moves_and_follows_its_row() {
        let mut list = SuggestionList::default();
        let rows = list.rows("/co", &[]);
        assert_eq!(names(&rows), ["compact", "copy"]);
        assert_eq!(list.selected(&rows), 0);
        assert_eq!(list.move_selection(&rows, false), 0);
        assert_eq!(list.move_selection(&rows, true), 1);
        assert_eq!(list.move_selection(&rows, true), 1);
        // Widening the list keeps the highlight on `copy`.
        let rows = list.rows("/c", &[]);
        list.text_changed("/c", &rows);
        assert_eq!(rows[list.selected(&rows)].name, "copy");
        // A row that is gone starts over.
        let rows = list.rows("/com", &[]);
        list.text_changed("/com", &rows);
        let rows = list.rows("/c", &[]);
        list.text_changed("/c", &rows);
        assert_eq!(list.selected(&rows), 0);
    }

    #[test]
    fn a_fully_typed_name_takes_the_highlight() {
        let pi = [command("fix-all"), command("fix")];
        let mut list = SuggestionList::default();
        let rows = list.rows("/fi", &pi);
        list.text_changed("/fi", &rows);
        assert_eq!(rows[list.selected(&rows)].name, "fix-all");
        list.move_selection(&rows, true);
        list.move_selection(&rows, false);
        let rows = list.rows("/fix", &pi);
        list.text_changed("/fix", &rows);
        assert_eq!(rows[list.selected(&rows)].name, "fix");
    }

    #[test]
    fn esc_closes_the_list_until_the_text_changes() {
        let mut list = SuggestionList::default();
        list.close("/co");
        assert_eq!(list.rows("/co", &[]), []);
        let rows = list.rows("/com", &[]);
        assert_eq!(names(&rows), ["compact"]);
        list.text_changed("/com", &rows);
        assert_eq!(names(&list.rows("/co", &[])), ["compact", "copy"]);
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
