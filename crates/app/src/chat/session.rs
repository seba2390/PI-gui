//! `PiSession`: one pi process and the live state derived from it.
//!
//! This entity is the only place the GUI talks to pi. It spawns `pi --mode rpc`,
//! pumps its events into the [`Transcript`] reducer, and exposes pi's commands as
//! methods. Views observe it and render; they never hold protocol state.

use pi_sessions::Mode;
use remote::HostId;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui_kit::{Context, EventEmitter, Task};
use pi_rpc::protocol::{
    ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest, ImageContent, Model, NotifyKind,
    SessionState, SessionStats, SlashCommand, StreamingBehavior, ThinkingLevel,
};
use pi_rpc::{ClientEvent, PiClient, PiProcess, RpcError, SpawnOptions};
use transcript::{NoticeKind, Transcript};

use crate::config::AppConfig;

/// Where the pi process is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifecycle {
    /// A placeholder with no pi process (the welcome screen before any session is open).
    Idle,
    Starting,
    Ready,
    Exited {
        message: String,
    },
}

/// What a view may want to react to.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Transcript items were appended or changed.
    TranscriptChanged,
    /// Model, thinking level, stats, title or lifecycle changed.
    MetaChanged,
    /// pi reported (or changed) the session file; the sidebar should refresh.
    SessionFileChanged,
    /// The agent finished all automatic work.
    Settled,
    /// An extension asks the user something; answer with [`PiSession::answer_dialog`].
    Dialog(ExtensionUiRequest),
    /// Put this text into the composer (dequeued messages, `set_editor_text`).
    SetEditorText(String),
    Notify {
        kind: NotifyKind,
        message: String,
    },
}

/// A prompt typed before pi finished starting.
struct PendingPrompt {
    text: String,
    images: Vec<ImageContent>,
}

pub struct PiSession {
    /// The SSH host pi runs on, or `None` for this computer.
    host: Option<HostId>,
    mode: Mode,
    cwd: PathBuf,
    resumed_from: Option<PathBuf>,
    process: Option<PiProcess>,
    client: Option<PiClient>,
    lifecycle: Lifecycle,
    transcript: Transcript,
    state: Option<SessionState>,
    models: Vec<Model>,
    thinking_levels: Vec<ThinkingLevel>,
    stats: Option<SessionStats>,
    commands: Vec<SlashCommand>,
    /// `setStatus` entries from extensions, shown in the status line.
    extension_status: BTreeMap<String, String>,
    /// `setWidget` blocks from extensions, shown above the composer.
    widgets: BTreeMap<String, Vec<String>>,
    pending: VecDeque<PendingPrompt>,
    /// `(provider, model id)` to switch to once pi has started (after adding a model).
    pending_model: Option<(String, String)>,
    _event_pump: Task<()>,
}

impl EventEmitter<SessionEvent> for PiSession {}

impl PiSession {
    /// Starts pi in `cwd`, resuming `session_file` when given.
    /// Starts pi in `cwd`, resuming `session_file` when given; on `host` (a saved SSH host)
    /// pi runs on that machine and `cwd` / `session_file` are paths there.
    pub fn new(
        cwd: PathBuf,
        session_file: Option<PathBuf>,
        host: Option<HostId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = cx.global::<AppConfig>().clone();
        let saved = host.as_ref().and_then(|id| {
            let hosts = cx.try_global::<crate::hosts::HostsHandle>()?.0.read(cx);
            let mut saved = hosts.get(id).cloned()?;
            // Prefer the exact path the probe found (conda / nvm installs may only be on an
            // interactive shell's PATH; pi-gui's own install is not on PATH at all).
            saved.pi_command = hosts.pi_command(id);
            Some(saved)
        });
        let (options, mode) = spawn_options(&config, &cwd, session_file.as_deref(), saved.as_ref());
        if mode == Mode::Chat
            && let Err(error) = std::fs::create_dir_all(&cwd)
        {
            log::warn!(
                "could not create the chat folder {}: {error}",
                cwd.display()
            );
        }

        let mut this = Self {
            host,
            mode,
            cwd,
            resumed_from: session_file,
            process: None,
            client: None,
            lifecycle: Lifecycle::Starting,
            transcript: Transcript::new(),
            state: None,
            models: Vec::new(),
            thinking_levels: Vec::new(),
            stats: None,
            commands: Vec::new(),
            extension_status: BTreeMap::new(),
            widgets: BTreeMap::new(),
            pending: VecDeque::new(),
            pending_model: None,
            _event_pump: Task::ready(()),
        };

        match PiProcess::spawn(&options) {
            Ok((process, events)) => {
                log::info!(
                    "started pi (pid {}) in {}",
                    process.pid(),
                    this.cwd.display()
                );
                this.client = Some(process.client().clone());
                this.process = Some(process);
                this._event_pump = cx.spawn(async move |this, cx| {
                    while let Ok(first) = events.recv().await {
                        // Drain what is already buffered so a burst of deltas costs one update.
                        let mut batch = vec![first];
                        while batch.len() < 512 {
                            match events.try_recv() {
                                Ok(event) => batch.push(event),
                                Err(_) => break,
                            }
                        }
                        let alive = this.update(cx, |this, cx| {
                            for event in batch {
                                this.handle(event, cx);
                            }
                            cx.emit(SessionEvent::TranscriptChanged);
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                });
                this.initialize(cx);
            }
            Err(error) => {
                this.lifecycle = Lifecycle::Exited {
                    message: format!(
                        "Could not start `{}`: {error}. Is pi installed and on your PATH? \
                         (Set PI_GUI_PI_BIN to override.)",
                        options.program.display()
                    ),
                };
            }
        }
        this
    }

    /// A placeholder without a pi process: nothing runs, nothing is read from disk. Shown
    /// (behind the welcome screen) until the user opens or starts a session.
    pub fn idle(cwd: PathBuf, mode: Mode) -> Self {
        Self {
            host: None,
            mode,
            cwd,
            resumed_from: None,
            process: None,
            client: None,
            lifecycle: Lifecycle::Idle,
            transcript: Transcript::new(),
            state: None,
            models: Vec::new(),
            thinking_levels: Vec::new(),
            stats: None,
            commands: Vec::new(),
            extension_status: BTreeMap::new(),
            widgets: BTreeMap::new(),
            pending: VecDeque::new(),
            pending_model: None,
            _event_pump: Task::ready(()),
        }
    }

    pub fn is_idle(&self) -> bool {
        self.lifecycle == Lifecycle::Idle
    }

    // ---- reads ---------------------------------------------------------------

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The SSH host this session runs on (`None`: this computer).
    pub fn host(&self) -> Option<&HostId> {
        self.host.as_ref()
    }

    /// A Code session on this computer (its folder is a local path).
    pub fn is_local_code(&self) -> bool {
        self.host.is_none() && self.mode == Mode::Code
    }

    /// A Code session, here or on an SSH host: it has a project folder, so the terminal,
    /// diff and files panels and the file picker apply.
    pub fn has_folder_tools(&self) -> bool {
        self.mode == Mode::Code
    }

    /// Chat sessions have no tools and no project folder.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn is_running(&self) -> bool {
        self.transcript.is_running()
    }

    pub fn model(&self) -> Option<&Model> {
        self.state.as_ref().and_then(|state| state.model.as_ref())
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }

    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        self.transcript
            .thinking_level()
            .or_else(|| self.state.as_ref().map(|state| state.thinking_level))
    }

    pub fn thinking_levels(&self) -> &[ThinkingLevel] {
        &self.thinking_levels
    }

    pub fn commands(&self) -> &[SlashCommand] {
        &self.commands
    }

    pub fn widgets(&self) -> &BTreeMap<String, Vec<String>> {
        &self.widgets
    }

    pub fn extension_status(&self) -> &BTreeMap<String, String> {
        &self.extension_status
    }

    pub fn session_file(&self) -> Option<PathBuf> {
        self.state
            .as_ref()
            .and_then(|state| state.session_file.as_ref().map(PathBuf::from))
            .or_else(|| self.resumed_from.clone())
    }

    pub fn title(&self) -> Option<String> {
        self.transcript
            .session_name()
            .map(str::to_owned)
            .or_else(|| {
                self.state
                    .as_ref()
                    .and_then(|state| state.session_name.clone())
            })
            .filter(|name| !name.is_empty() && name != "New thread")
    }

    /// The name to show: pi's session name, else the first prompt's first line.
    pub fn display_title(&self) -> Option<String> {
        self.title().or_else(|| {
            self.transcript
                .items()
                .iter()
                .find_map(|item| match &item.kind {
                    transcript::ItemKind::User(user) => user
                        .text
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .map(|line| line.chars().take(80).collect()),
                    _ => None,
                })
        })
    }

    pub fn is_empty(&self) -> bool {
        self.transcript.items().is_empty() && self.pending.is_empty()
    }

    /// Context-window usage as `(used tokens, window)`, live while streaming.
    pub fn context_usage(&self) -> Option<(u64, u64)> {
        let window = self
            .stats
            .as_ref()
            .and_then(|stats| stats.context_usage.as_ref())
            .map(|usage| usage.context_window)
            .filter(|window| *window > 0)
            .or_else(|| self.model().map(|model| model.context_window))?;
        let live = self
            .is_running()
            .then(|| self.transcript.last_usage())
            .flatten();
        let used = match live {
            Some(usage) => usage.total_tokens,
            None => self
                .stats
                .as_ref()
                .and_then(|stats| stats.context_usage.as_ref())
                .and_then(|usage| usage.tokens)
                .unwrap_or(0),
        };
        Some((used, window))
    }

    pub fn stats(&self) -> Option<&SessionStats> {
        self.stats.as_ref()
    }

    pub fn total_cost(&self) -> Option<f64> {
        self.stats.as_ref().map(|stats| stats.cost)
    }

    // ---- commands ------------------------------------------------------------

    /// Submits the composer text. While the agent works, Enter steers and
    /// Alt+Enter queues a follow-up — exactly like pi's terminal.
    pub fn submit(
        &mut self,
        text: String,
        images: Vec<ImageContent>,
        follow_up: bool,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle == Lifecycle::Starting {
            self.pending.push_back(PendingPrompt { text, images });
            cx.notify();
            return;
        }
        let behavior = self.is_running().then_some(if follow_up {
            StreamingBehavior::FollowUp
        } else {
            StreamingBehavior::Steer
        });
        self.run(cx, async move |client| {
            client.prompt(text, images, behavior).await.map(drop)
        });
    }

    /// Esc: restore queued messages to the editor, then abort the run.
    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let restored = client.clear_queue().await.unwrap_or_default();
            if !restored.is_empty() {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(SessionEvent::SetEditorText(restored.join("\n\n")));
                });
            }
            if let Err(error) = client.abort().await {
                let _ = this.update(cx, |this, cx| this.report(&error, cx));
            }
        })
        .detach();
    }

    /// Alt+Up: move queued messages back into the editor without aborting.
    pub fn dequeue(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Ok(restored) = client.clear_queue().await
                && !restored.is_empty()
            {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(SessionEvent::SetEditorText(restored.join("\n\n")));
                });
            }
        })
        .detach();
    }

    pub fn set_model(&mut self, model: &Model, cx: &mut Context<Self>) {
        let (provider, id) = (model.provider.clone(), model.id.clone());
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.set_model(provider, id).await;
            let levels = client.get_available_thinking_levels().await;
            let state = client.get_state().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        if let Ok(levels) = levels {
                            this.thinking_levels = levels;
                        }
                        if let Ok(state) = state {
                            this.state = Some(state);
                        }
                    }
                    Err(error) => this.report(&error, cx),
                }
                this.refresh_stats(cx);
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Switches to `provider/model_id` as soon as pi has started and listed its models (right
    /// away if it already has).
    pub fn select_model_when_ready(
        &mut self,
        provider: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_model = Some((provider, model_id));
        if self.lifecycle == Lifecycle::Ready {
            self.apply_pending_model(cx);
        }
    }

    fn apply_pending_model(&mut self, cx: &mut Context<Self>) {
        let Some((provider, id)) = self.pending_model.take() else {
            return;
        };
        match self
            .models
            .iter()
            .find(|model| model.provider == provider && model.id == id)
            .cloned()
        {
            Some(model) => self.set_model(&model, cx),
            None => self.push_notice(
                NoticeKind::Error,
                format!(
                    "pi did not load {provider}/{id}. Check the provider's base URL and API key \
                     (pi lists only models it can authenticate)."
                ),
                cx,
            ),
        }
    }

    pub fn set_thinking_level(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.state {
            state.thinking_level = level;
        }
        cx.notify();
        self.run(cx, async move |client| {
            client.set_thinking_level(level).await
        });
    }

    /// Shift+Tab: next available thinking level.
    pub fn cycle_thinking_level(&mut self, cx: &mut Context<Self>) {
        let (Some(current), false) = (self.thinking_level(), self.thinking_levels.is_empty())
        else {
            return;
        };
        let index = self
            .thinking_levels
            .iter()
            .position(|level| *level == current)
            .unwrap_or(0);
        let next = self.thinking_levels[(index + 1) % self.thinking_levels.len()];
        self.set_thinking_level(next, cx);
    }

    pub fn compact(&mut self, instructions: Option<String>, cx: &mut Context<Self>) {
        self.run(cx, async move |client| client.compact(instructions).await);
    }

    pub fn rename(&mut self, name: String, cx: &mut Context<Self>) {
        self.transcript.set_session_name(Some(name.clone()));
        cx.emit(SessionEvent::SessionFileChanged);
        self.run(cx, async move |client| client.set_session_name(name).await);
    }

    /// `!cmd` / `!!cmd`: run a shell command through pi (added to context unless `!!`).
    pub fn run_bash(
        &mut self,
        command: String,
        exclude_from_context: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client
                .request(pi_rpc::protocol::Command::Bash {
                    command: command.clone(),
                    exclude_from_context,
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(response) => {
                        let data: pi_rpc::protocol::BashResult =
                            serde_json::from_value(response.data.unwrap_or_default())
                                .unwrap_or_default();
                        this.transcript
                            .push_bash(command, data.output, data.exit_code);
                    }
                    Err(error) => this.report(&error, cx),
                }
                cx.emit(SessionEvent::TranscriptChanged);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn answer_dialog(&mut self, id: &str, answer: &ExtensionUiAnswer) {
        if let Some(client) = &self.client
            && let Err(error) = client.answer_extension_ui(id, answer)
        {
            log::warn!("could not answer extension dialog: {error}");
        }
    }

    pub fn push_notice(
        &mut self,
        kind: NoticeKind,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.transcript.push_notice(kind, text);
        cx.emit(SessionEvent::TranscriptChanged);
        cx.notify();
    }

    // ---- internals -----------------------------------------------------------

    /// Runs a command, surfacing failures in the transcript.
    fn run<F>(&mut self, cx: &mut Context<Self>, command: F)
    where
        F: AsyncFnOnce(PiClient) -> Result<(), RpcError> + 'static,
    {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Err(error) = command(client).await {
                let _ = this.update(cx, |this, cx| this.report(&error, cx));
            }
        })
        .detach();
    }

    fn report(&mut self, error: &RpcError, cx: &mut Context<Self>) {
        log::warn!("pi command failed: {error}");
        let text = match error {
            RpcError::Rejected { message, .. } => message.clone(),
            other => other.to_string(),
        };
        self.push_notice(NoticeKind::Error, text, cx);
    }

    fn initialize(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let resumed = self.resumed_from.is_some();
        cx.spawn(async move |this, cx| {
            let state = client.get_state().await;
            let models = client.get_available_models().await.unwrap_or_default();
            let levels = client
                .get_available_thinking_levels()
                .await
                .unwrap_or_default();
            let messages = if resumed {
                client.get_messages().await.ok()
            } else {
                None
            };
            let stats = client.get_session_stats().await.ok();
            let commands = client.get_commands().await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                match state {
                    Ok(state) => {
                        this.state = Some(state);
                        this.lifecycle = Lifecycle::Ready;
                    }
                    Err(error) => {
                        this.lifecycle = Lifecycle::Exited {
                            message: this.exit_message(&error),
                        };
                    }
                }
                this.models = models;
                this.thinking_levels = levels;
                this.stats = stats;
                this.commands = commands;
                if let Some(messages) = messages {
                    let name = this.transcript.session_name().map(str::to_owned);
                    this.transcript = Transcript::from_messages(&messages);
                    this.transcript.set_session_name(
                        name.or_else(|| this.state.as_ref().and_then(|s| s.session_name.clone())),
                    );
                }
                for prompt in std::mem::take(&mut this.pending) {
                    this.submit(prompt.text, prompt.images, false, cx);
                }
                if this.lifecycle == Lifecycle::Ready {
                    this.apply_pending_model(cx);
                }
                cx.emit(SessionEvent::TranscriptChanged);
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn exit_message(&self, error: &RpcError) -> String {
        let tail = self
            .process
            .as_ref()
            .map(PiProcess::stderr_tail)
            .unwrap_or_default();
        let detail = tail
            .iter()
            .rev()
            .take(6)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if detail.is_empty() {
            format!("pi stopped: {error}")
        } else {
            format!("pi stopped: {error}\n{detail}")
        }
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let stats = client.get_session_stats().await;
            let state = client.get_state().await;
            let _ = this.update(cx, |this, cx| {
                let old_file = this.session_file();
                if let Ok(stats) = stats {
                    this.stats = Some(stats);
                }
                if let Ok(state) = state {
                    this.state = Some(state);
                }
                if this.session_file() != old_file {
                    cx.emit(SessionEvent::SessionFileChanged);
                }
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn handle(&mut self, event: ClientEvent, cx: &mut Context<Self>) {
        match event {
            ClientEvent::Event(event) => {
                self.transcript.apply(&event, Instant::now());
                match *event {
                    pi_rpc::protocol::Event::AgentSettled { .. } => {
                        self.refresh_stats(cx);
                        cx.emit(SessionEvent::Settled);
                    }
                    pi_rpc::protocol::Event::TurnEnd { .. }
                    | pi_rpc::protocol::Event::CompactionEnd { .. } => self.refresh_stats(cx),
                    pi_rpc::protocol::Event::SessionInfoChanged { .. } => {
                        cx.emit(SessionEvent::SessionFileChanged);
                        cx.emit(SessionEvent::MetaChanged);
                    }
                    _ => {}
                }
            }
            ClientEvent::ExtensionUi(request) => self.handle_extension_ui(request, cx),
            ClientEvent::ProtocolError { line, error } => {
                log::warn!("undecodable pi record ({error}): {line}");
            }
            ClientEvent::Disconnected => {
                let message = self.exit_message(&RpcError::Disconnected);
                self.transcript.force_settle(Instant::now());
                if self.lifecycle == Lifecycle::Ready {
                    self.transcript
                        .push_notice(NoticeKind::Error, message.clone());
                }
                self.lifecycle = Lifecycle::Exited { message };
                self.client = None;
                cx.emit(SessionEvent::MetaChanged);
            }
        }
    }

    fn handle_extension_ui(&mut self, request: ExtensionUiRequest, cx: &mut Context<Self>) {
        match &request.method {
            ExtensionUiMethod::Notify {
                message,
                notify_type,
            } => {
                cx.emit(SessionEvent::Notify {
                    kind: *notify_type,
                    message: message.clone(),
                });
            }
            ExtensionUiMethod::SetStatus {
                status_key,
                status_text,
            } => match status_text {
                Some(text) => drop(
                    self.extension_status
                        .insert(status_key.clone(), text.clone()),
                ),
                None => drop(self.extension_status.remove(status_key)),
            },
            ExtensionUiMethod::SetWidget {
                widget_key,
                widget_lines,
                ..
            } => match widget_lines {
                Some(lines) => drop(self.widgets.insert(widget_key.clone(), lines.clone())),
                None => drop(self.widgets.remove(widget_key)),
            },
            ExtensionUiMethod::SetEditorText { text } => {
                cx.emit(SessionEvent::SetEditorText(text.clone()));
            }
            ExtensionUiMethod::SetTitle { .. } | ExtensionUiMethod::Unknown => {}
            ExtensionUiMethod::Select { .. }
            | ExtensionUiMethod::Confirm { .. }
            | ExtensionUiMethod::Input { .. }
            | ExtensionUiMethod::Editor { .. } => cx.emit(SessionEvent::Dialog(request)),
        }
        cx.emit(SessionEvent::MetaChanged);
    }
}

/// How to launch pi for a session in `cwd`. Chat sessions get `--no-tools`: pi records the
/// empty tool set in the session and restores it on resume (even from the terminal), so a chat
/// can never read or change files; passing it again on resume is belt and braces.
///
/// On a remote `host`, pi runs there through the host's shared ssh connection, in `cwd` on the
/// remote, via the login shell (see [`remote::Ssh::command`]); remote sessions are always Code.
fn spawn_options(
    config: &AppConfig,
    cwd: &Path,
    session_file: Option<&Path>,
    host: Option<&remote::SavedHost>,
) -> (SpawnOptions, Mode) {
    let mut options = SpawnOptions::new(cwd);
    options.program.clone_from(&config.pi_program);
    options.env.clone_from(&config.child_env);
    options.session = session_file.map(Path::to_path_buf);
    let Some(host) = host else {
        let mode = config.mode_of(cwd);
        if mode == Mode::Chat {
            options.extra_args.push("--no-tools".into());
        }
        return (options, mode);
    };
    let argv: Vec<String> = std::iter::once(host.pi_command().to_owned())
        .chain(
            options
                .args()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        )
        .collect();
    let ssh = remote::Ssh::new(host.target.clone());
    let (program, args) = ssh.command(Some(&cwd.to_string_lossy()), &argv);
    options.launcher = Some(pi_rpc::Launcher { program, args });
    // The local ssh client just needs some existing folder to run in.
    options.cwd = config.home.clone().unwrap_or_else(std::env::temp_dir);
    (options, Mode::Code)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn chat_sessions_launch_without_tools() {
        let config = AppConfig::for_tests("pi".into());
        let (chat, mode) = super::spawn_options(&config, &config.chat_dir.clone(), None, None);
        assert_eq!(mode, super::Mode::Chat);
        assert_eq!(
            chat.extra_args,
            vec![std::ffi::OsString::from("--no-tools")]
        );

        let project = std::env::temp_dir().join("some-project");
        let session = project.join("s.jsonl");
        let (code, mode) = super::spawn_options(&config, &project, Some(&session), None);
        assert_eq!(mode, super::Mode::Code);
        assert_eq!(code.extra_args, [] as [std::ffi::OsString; 0]);
        assert_eq!(code.session.as_deref(), Some(session.as_path()));
    }

    #[test]
    fn remote_sessions_launch_pi_through_ssh() {
        let config = AppConfig::for_tests("pi".into());
        let host = remote::SavedHost {
            id: remote::HostId("box".into()),
            label: "Box".into(),
            target: remote::Target::Alias {
                alias: "box".into(),
            },
            pi_command: Some("/opt/pi/bin/pi".into()),
            recent_folders: Vec::new(),
        };
        let folder = std::path::Path::new("/srv/my app");
        let (options, mode) = super::spawn_options(&config, folder, None, Some(&host));
        assert_eq!(mode, super::Mode::Code);
        let launcher = options.launcher.expect("remote sessions use a launcher");
        assert_eq!(launcher.program, PathBuf::from("ssh"));
        let args: Vec<String> = launcher
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(args.iter().any(|arg| arg == "box"), "{joined}");
        assert!(
            joined.contains("/opt/pi/bin/pi")
                && joined.contains("--mode")
                && joined.contains("rpc"),
            "{joined}"
        );
        assert!(
            joined.contains("'/srv/my app'"),
            "remote folder is quoted: {joined}"
        );
        // The local ssh client runs in an existing local folder, not the remote path.
        assert_ne!(options.cwd, folder);
    }

    /// End to end on a real SSH host with real pi (e.g. after `remote`'s `live_install`):
    /// pi is started through the same launch path as a remote session, runs a shell command
    /// on the host through pi's RPC, its session file is listed from the host, and the
    /// session resumes with its history. No model is needed.
    /// `PI_GUI_SSH_TEST_HOST=<alias> cargo test -p pi-gui -- --ignored real_pi_over_ssh`
    #[test]
    #[ignore = "runs pi on PI_GUI_SSH_TEST_HOST"]
    fn real_pi_over_ssh() {
        use futures::executor::block_on;
        let Ok(alias) = std::env::var("PI_GUI_SSH_TEST_HOST") else {
            return;
        };
        let ssh = remote::Ssh::new(remote::Target::Alias {
            alias: alias.clone(),
        });
        ssh.connect(false).expect("connect");
        let probe = ssh.probe("pi").expect("probe");
        let pi = probe.pi_path.expect("pi is installed on the host");
        // A scratch folder with a project, a pi settings folder whose only model is a dummy
        // that refuses connections (so pi records an error reply without any real model or
        // key), and a custom pi command (a wrapper) that points pi at those settings.
        let scratch = ssh
            .exec(&format!(
                "d=$(mktemp -d /tmp/pi-gui-e2e.XXXXXX) && cd \"$d\" && mkdir -p proj agent && \
                 (cd proj && git init -q) && \
                 printf '%s' '{{\"providers\":{{\"dummy\":{{\"baseUrl\":\"http://127.0.0.1:9/v1\",\"api\":\"openai-completions\",\"apiKey\":\"none\",\"models\":[{{\"id\":\"none\",\"name\":\"None\"}}]}}}}}}' > agent/models.json && \
                 printf '%s' '{{\"defaultProvider\":\"dummy\",\"defaultModel\":\"none\"}}' > agent/settings.json && \
                 printf '#!/bin/sh\\nPI_CODING_AGENT_DIR=%s/agent\\nexport PI_CODING_AGENT_DIR\\nexec %s \"$@\"\\n' \"$d\" {} > pi && chmod 755 pi && pwd -P",
                remote::sh_quote(&pi)
            ))
            .expect("scratch folder")
            .trim()
            .to_owned();
        let folder = format!("{scratch}/proj");
        let host = remote::SavedHost {
            id: remote::HostId("e2e".into()),
            label: alias,
            target: ssh.target().clone(),
            pi_command: Some(format!("{scratch}/pi")),
            recent_folders: Vec::new(),
        };
        let config = AppConfig::for_tests("pi".into());
        let (options, mode) =
            super::spawn_options(&config, std::path::Path::new(&folder), None, Some(&host));
        assert_eq!(mode, super::Mode::Code);
        let (process, events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi over ssh");
        let client = process.client().clone();
        let state = block_on(client.get_state()).expect("get_state");
        eprintln!(
            "remote pi state: session {:?} file {:?}",
            state.session_id, state.session_file
        );
        let response = block_on(client.request(pi_rpc::protocol::Command::Bash {
            command: "printf e2e > made-by-pi.txt && pwd -P".into(),
            exclude_from_context: false,
        }))
        .expect("bash through pi");
        let result: pi_rpc::protocol::BashResult =
            serde_json::from_value(response.data.unwrap_or_default()).unwrap_or_default();
        eprintln!("bash: {:?} exit {:?}", result.output, result.exit_code);
        assert_eq!(result.output.trim(), folder);
        assert!(ssh.exists(&format!("{folder}/made-by-pi.txt")).unwrap());

        // A prompt: the dummy model fails, and pi records the turn (and writes its file).
        block_on(client.prompt("hello from the e2e test".into(), Vec::new(), None))
            .expect("prompt accepted");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut settled = false;
        while std::time::Instant::now() < deadline {
            match block_on(events.recv()) {
                Ok(pi_rpc::ClientEvent::Event(event)) => {
                    if matches!(*event, pi_rpc::protocol::Event::AgentSettled { .. }) {
                        settled = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(settled, "the turn settles");
        let file = block_on(client.get_state())
            .expect("state")
            .session_file
            .expect("pi reports its session file");
        drop(process);

        // The sidebar's listing of the host finds it (pi writes it once there is history).
        let mut index = remote::SessionIndex::default();
        let sessions = index
            .refresh(&ssh, &format!("{scratch}/agent/sessions"))
            .expect("list sessions");
        eprintln!("sessions on the host: {}", sessions.len());
        let listed = sessions
            .iter()
            .find(|session| session.path == std::path::Path::new(&file));
        eprintln!("listed: {listed:?}");

        // Resume it: the history comes back.
        let (options, _) = super::spawn_options(
            &config,
            std::path::Path::new(&folder),
            Some(std::path::Path::new(&file)),
            Some(&host),
        );
        let (process, _events) = pi_rpc::PiProcess::spawn(&options).expect("resume");
        let messages = block_on(process.client().get_messages()).expect("messages");
        eprintln!("resumed with {} messages", messages.len());
        drop(process);
        let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&scratch)));
        let listed = listed.expect("the session is listed from the host");
        assert_eq!(listed.title(), "hello from the e2e test");
        assert_eq!(listed.cwd, std::path::Path::new(&folder));
        assert!(!messages.is_empty(), "the resumed session has its history");
    }

    use gpui_kit::{AppContext as _, TestAppContext};
    use transcript::{BlockKind, ItemKind};

    use super::{Lifecycle, PiSession, SessionEvent};
    use crate::config::AppConfig;

    /// The "Add model" flow against real pi (`pi` on PATH): pi starts without the model, the
    /// model is added to models.json the way the dialog does it, pi is restarted (as the
    /// window does after saving), and the new model gets selected.
    /// `cargo test -p pi-gui -- --ignored added_model_is_loaded_and_selected`
    #[cfg(unix)]
    #[gpui_kit::test]
    #[ignore = "needs pi on PATH"]
    async fn added_model_is_loaded_and_selected(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let agent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut config = AppConfig::for_tests("pi".into());
        config.child_env = vec![("PI_CODING_AGENT_DIR".into(), agent.path().into())];
        cx.update(|cx| cx.set_global(config));

        let wait_ready = async |session: &gpui_kit::Entity<PiSession>, cx: &mut TestAppContext| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while cx.read(|cx| session.read(cx).lifecycle() == &Lifecycle::Starting) {
                assert!(std::time::Instant::now() < deadline, "pi did not start");
                cx.background_executor
                    .timer(std::time::Duration::from_millis(50))
                    .await;
            }
        };
        let before = cx.new(|cx| PiSession::new(project.path().into(), None, None, cx));
        wait_ready(&before, cx).await;
        assert!(cx.read(|cx| {
            before
                .read(cx)
                .models()
                .iter()
                .all(|m| m.provider != "pigui-test")
        }));

        let mut file = pi_config::ModelsFile::parse("").unwrap();
        let provider = pi_config::NewProvider {
            id: "pigui-test".into(),
            name: "PI-gui test".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            api: "openai-completions".into(),
            api_key: "none".into(),
        };
        let model = pi_config::ModelSpec {
            id: "added-model".into(),
            name: "Added model".into(),
            context_window: Some(32_000),
            ..pi_config::ModelSpec::default()
        };
        file.add_model("pigui-test", Some(&provider), &model)
            .unwrap();
        std::fs::write(agent.path().join("models.json"), file.to_text()).unwrap();
        drop(before);

        let after = cx.new(|cx| PiSession::new(project.path().into(), None, None, cx));
        after.update(cx, |session, cx| {
            session.select_model_when_ready("pigui-test".into(), "added-model".into(), cx);
        });
        wait_ready(&after, cx).await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let current = cx.read(|cx| {
                after
                    .read(cx)
                    .model()
                    .map(pi_rpc::protocol::Model::qualified_id)
            });
            if current.as_deref() == Some("pigui-test/added-model") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "selected: {current:?}"
            );
            cx.background_executor
                .timer(std::time::Duration::from_millis(50))
                .await;
        }
        cx.read(|cx| {
            let session = after.read(cx);
            assert_eq!(session.model().unwrap().display_name(), "Added model");
            assert_eq!(session.model().unwrap().context_window, 32_000);
        });
    }

    /// Drives a real `PiSession` against the scripted fake pi from `pi-rpc`'s tests.
    #[cfg(unix)]
    #[gpui_kit::test]
    async fn prompt_round_trip_with_fake_pi(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests(fake));
        });
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });

        // Startup handshake: get_state etc. complete and the session is ready.
        while cx.read(|cx| session.read(cx).lifecycle() != &Lifecycle::Ready) {
            events.recv().await.expect("session alive");
        }
        assert_eq!(
            cx.read(|cx| session
                .read(cx)
                .model()
                .map(pi_rpc::protocol::Model::qualified_id)),
            Some("test/fake-1".to_owned())
        );

        session.update(cx, |session, cx| {
            session.submit("hi".into(), Vec::new(), false, cx);
        });
        while !matches!(
            events.recv().await.expect("session alive"),
            SessionEvent::Settled
        ) {}

        cx.read(|cx| {
            let session = session.read(cx);
            let items = session.transcript().items();
            assert!(matches!(&items[0].kind, ItemKind::User(user) if user.text == "hi"));
            let ItemKind::Assistant(assistant) = &items[1].kind else { panic!("{items:?}") };
            assert!(matches!(&assistant.blocks[0].kind, BlockKind::Text(text) if text.text == "Hello \u{2028} world"));
            assert!(matches!(items.last().map(|item| &item.kind), Some(ItemKind::TurnSummary(_))));
            assert!(!session.is_running());
            assert_eq!(session.display_title().as_deref(), Some("hi"));
        });
    }
}
