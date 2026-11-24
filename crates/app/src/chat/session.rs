//! `PiSession`: one pi process and the live state derived from it.
//!
//! This entity is the only place the GUI talks to pi. It spawns `pi --mode rpc`,
//! pumps its events into the [`Transcript`] reducer, and exposes pi's commands as
//! methods. Views observe it and render; they never hold protocol state.

pub use deepsearch::config::Mode as WebMode;
use deepsearch::effort::Effort;
use pi_sessions::Mode;
use remote::HostId;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::{Context, EventEmitter, Task};
use pi_rpc::protocol::{
    AgentMessage, CompactionReason, ContentBlock, Disposition, ExtensionUiAnswer,
    ExtensionUiMethod, ExtensionUiRequest, ImageContent, MessageContent, Model, NotifyKind,
    SessionState, SessionStats, SlashCommand, StreamingBehavior, ThinkingLevel,
};
use pi_rpc::{ClientEvent, PiClient, PiProcess, RpcError, SpawnOptions};
use transcript::{BashStatus, NoticeKind, RunStatus, Transcript};

use super::queued::QueuedPrompts;
use crate::config::{AppConfig, ChildEnv};

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
    /// Replace the composer's text (an extension's `set_editor_text`).
    SetEditorText(String),
    /// Give messages back to the composer: queued ones restored by Esc / Alt+Up, prompts
    /// that could not be sent, or a message to edit. `replace` swaps out the draft;
    /// otherwise the restored text goes before it, so nothing typed is lost.
    RestoreToEditor {
        text: String,
        images: Vec<ImageContent>,
        replace: bool,
    },
    Notify {
        kind: NotifyKind,
        message: String,
    },
    /// [`PiSession::is_busy`] changed.
    BusyChanged,
    /// pi switched to another session file in the same process (fork, clone): the transcript
    /// was replaced, not appended to.
    Reloaded,
}

/// What to do with the message a branch starts from (see [`PiSession::fork_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterFork {
    /// Put it in the composer to edit and send.
    Edit,
    /// Send it again as it was (retry).
    Resend,
    /// Leave it out: the branch ends before it.
    Keep,
}

/// How long events pi sent before an answer (or before it stopped) may take to be handled after
/// it.
const EVENT_GRACE: Duration = Duration::from_secs(1);

/// The images of the user message `from_end` messages before the last one (0 is the
/// newest), if its text is `text`; empty when it cannot be matched.
fn user_images(messages: &[AgentMessage], from_end: usize, text: &str) -> Vec<ImageContent> {
    let user = messages
        .iter()
        .rev()
        .filter_map(|message| match message {
            AgentMessage::User(user) => Some(user),
            _ => None,
        })
        .nth(from_end);
    match user {
        Some(user) if user.content.text().trim() == text.trim() => match &user.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Image(image) => Some(image.clone()),
                    _ => None,
                })
                .collect(),
            MessageContent::Text(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// pi's fork entry for the user message `from_end` places before the last one, if its text
/// matches. Counting from the end keeps the match right when pi's list holds more messages
/// than the transcript (history before a compaction).
fn fork_entry(
    messages: &[pi_rpc::protocol::ForkMessage],
    from_end: usize,
    text: &str,
) -> Option<String> {
    let index = messages.len().checked_sub(from_end + 1)?;
    let message = &messages[index];
    (message.text.trim() == text.trim()).then(|| message.entry_id.clone())
}

/// Runs `gh gist create` (secret) for `path`; returns pi's viewer link and the gist URL.
fn create_gist(path: &str, env: &ChildEnv) -> Result<(String, String), String> {
    let output = std::process::Command::new("gh")
        .args(["gist", "create", "--public=false", path])
        .envs(env.vars().iter().cloned())
        .output()
        .map_err(|_| {
            "Sharing needs the GitHub CLI (gh): https://cli.github.com, then `gh auth login`."
                .to_owned()
        })?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh could not create the gist: {}", error.trim()));
    }
    let gist = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let id = gist.rsplit('/').next().unwrap_or_default();
    if id.is_empty() {
        return Err(format!("Unexpected answer from gh: {gist}"));
    }
    let base = env
        .var("PI_SHARE_VIEWER_URL")
        .and_then(|url| url.into_string().ok())
        .unwrap_or_else(|| "https://pi.dev/session/".to_owned());
    Ok((format!("{base}#{id}"), gist))
}

/// A prompt waiting to be sent to pi.
struct PendingPrompt {
    text: String,
    images: Vec<ImageContent>,
    /// Queue it behind the current run (Alt+Enter) instead of steering it (Enter).
    follow_up: bool,
}

/// The prompt pi has not answered yet. It is kept to give it back if pi refuses it.
struct Sending {
    prompt: PendingPrompt,
    /// [`PiSession::runs_started`] when it was sent.
    runs_before: u64,
    /// Nothing was said in the conversation before it: it names it once pi accepts it.
    first: bool,
}

/// Compaction outside a run.
#[derive(Default)]
struct Compaction {
    /// `/compact` was sent and pi has not answered yet.
    sent: bool,
    /// pi compacts (`/compact`, or an extension asked for it) and refuses prompts meanwhile.
    running: bool,
    /// How many ended so far.
    ended: u64,
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
    /// All levels pi offers for the model; see [`Self::thinking_levels`].
    thinking_levels: Vec<ThinkingLevel>,
    stats: Option<SessionStats>,
    commands: Vec<SlashCommand>,
    /// `setStatus` entries from extensions, shown in the status line.
    extension_status: BTreeMap<String, String>,
    /// `setWidget` blocks from extensions, shown above the composer.
    widgets: BTreeMap<String, Vec<String>>,
    /// Prompts not yet sent: pi gets one at a time, each after the previous was accepted, so a
    /// second message never arrives while pi still thinks it is idle (it would start a second
    /// run or be refused as busy).
    outbox: VecDeque<PendingPrompt>,
    /// The prompt that was sent and that pi has not answered yet.
    sending: Option<Sending>,
    /// The prompts in pi's queue, for their images.
    queued: QueuedPrompts,
    /// `clear_queue` commands pi has not answered yet. The queue updates they cause don't
    /// mean that messages were delivered.
    clearing_queue: usize,
    /// pi said it started a run whose first event has not arrived yet (until then,
    /// `transcript.is_running()` is still false). Responses and events travel separately, so
    /// the run may also have started (and even ended) before the response is handled: see
    /// `runs_started`.
    awaiting_start: bool,
    /// `agent_start` events seen so far.
    runs_started: u64,
    compaction: Compaction,
    /// [`Self::is_busy`] as last reported with [`SessionEvent::BusyChanged`].
    reported_busy: bool,
    /// `(provider, model id)` to switch to once pi has started (after adding a model).
    pending_model: Option<(String, String)>,
    _event_pump: Task<()>,
    /// The resumed session's history as read from its file, shown until pi answers.
    preview: Preview,
    _preview_task: Task<()>,
    /// Where automatic naming is (see [`crate::naming`]).
    naming: Naming,
    _naming_task: Task<()>,
    /// Starts pi once the login shell's environment is known and, on a host, its settings for
    /// this computer's models are written.
    _start_task: Task<()>,
}

/// Automatic naming looks at the first prompt only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// pi accepted no prompt yet.
    NotYet,
    /// Started, skipped or not wanted; later prompts never name the conversation.
    Considered,
}

/// Whether the transcript shows a preview read from the session file (see
/// [`pi_sessions::branch`]) and whether pi's own history has arrived since.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Preview {
    #[default]
    None,
    Shown,
    /// pi's history arrived: a late preview must not replace it.
    Superseded,
}

impl EventEmitter<SessionEvent> for PiSession {}

impl PiSession {
    /// Starts pi in `cwd`, resuming `session_file` when given; on `host` (a saved SSH host)
    /// pi runs on that machine and `cwd` / `session_file` are paths there.
    pub fn new(
        cwd: PathBuf,
        session_file: Option<PathBuf>,
        host: Option<HostId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = cx.global::<AppConfig>().clone();
        let saved = saved_host(host.as_ref(), cx);
        // pi on a host uses this computer's models once the host's settings for them are
        // written; when they must be written first, pi starts after that.
        let hosts = cx
            .try_global::<crate::hosts::HostsHandle>()
            .map(|hosts| hosts.0.clone());
        let remote_env = match (&host, hosts) {
            (Some(id), Some(hosts)) => hosts.update(cx, |hosts, cx| hosts.session_env(id, cx)),
            _ => Ok(Vec::new()),
        };
        // Remote sessions are always Code (see `spawn_options`).
        let mode = match saved {
            Some(_) => Mode::Code,
            None => config.mode_of(&cwd),
        };
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
            outbox: VecDeque::new(),
            sending: None,
            queued: QueuedPrompts::default(),
            clearing_queue: 0,
            awaiting_start: false,
            runs_started: 0,
            compaction: Compaction::default(),
            reported_busy: false,
            pending_model: None,
            _event_pump: Task::ready(()),
            preview: Preview::None,
            _preview_task: Task::ready(()),
            naming: Naming::NotYet,
            _naming_task: Task::ready(()),
            _start_task: Task::ready(()),
        };
        this.show_preview(cx);
        match remote_env {
            Ok(env) if config.child_env.ready().is_some() => this.start(&env, cx),
            remote_env => {
                this._start_task = cx.spawn(async move |this, cx| {
                    let env = match remote_env {
                        Ok(env) => env,
                        Err(link) => link.await,
                    };
                    config.child_env.wait().await;
                    let _ = this.update(cx, |this, cx| {
                        this.start(&env, cx);
                        cx.emit(SessionEvent::MetaChanged);
                    });
                });
            }
        }
        this
    }

    /// Starts pi (on a host with `remote_env`) with the login shell's environment, which must
    /// be ready.
    fn start(&mut self, remote_env: &[(String, String)], cx: &mut Context<Self>) {
        let config = cx.global::<AppConfig>().clone();
        let saved = saved_host(self.host.as_ref(), cx);
        let (options, _) = spawn_options(
            &config,
            &self.cwd,
            self.resumed_from.as_deref(),
            saved.as_ref(),
            remote_env,
        );
        self.start_process(&options, cx);
    }

    /// Starts pi with `options` and follows its events.
    fn start_process(&mut self, options: &SpawnOptions, cx: &mut Context<Self>) {
        let this = self;
        match PiProcess::spawn(options) {
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
                            this.report_busy(cx);
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
                let message = format!(
                    "Could not start `{}`: {error}. Is pi installed and on your PATH? \
                     (Set PISTACHIO_PI_BIN to override.)",
                    options.program.display()
                );
                // An empty chat shows the message itself; a preview shown meanwhile hides it.
                this._preview_task = Task::ready(());
                if !this.transcript.items().is_empty() {
                    this.push_notice(NoticeKind::Error, message.clone(), cx);
                }
                this.lifecycle = Lifecycle::Exited { message };
                this.return_outbox(cx);
                cx.notify();
            }
        }
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
            outbox: VecDeque::new(),
            sending: None,
            queued: QueuedPrompts::default(),
            clearing_queue: 0,
            awaiting_start: false,
            runs_started: 0,
            compaction: Compaction::default(),
            reported_busy: false,
            pending_model: None,
            _event_pump: Task::ready(()),
            preview: Preview::None,
            _preview_task: Task::ready(()),
            naming: Naming::NotYet,
            _naming_task: Task::ready(()),
            _start_task: Task::ready(()),
        }
    }

    /// A placeholder of a session on `host`.
    #[cfg(test)]
    pub fn idle_on_host(host: HostId) -> Self {
        Self {
            host: Some(host),
            ..Self::idle(PathBuf::from("/srv"), Mode::Code)
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

    /// Whether a shell command (`!cmd`) is running.
    pub fn is_bash_running(&self) -> bool {
        self.transcript.is_bash_running()
    }

    /// Whether the conversation is being compacted, in a run or between runs.
    pub fn is_compacting(&self) -> bool {
        self.holds_prompts() || *self.transcript.status() == RunStatus::Compacting
    }

    /// Whether the transcript ends with the working row.
    pub fn shows_working(&self) -> bool {
        self.is_running() || self.is_compacting()
    }

    /// What the working row says.
    pub fn working_status(&self) -> RunStatus {
        if self.is_compacting() {
            RunStatus::Compacting
        } else {
            self.transcript.status().clone()
        }
    }

    /// Whether something visible goes on that Esc and the stop button end.
    pub fn can_stop(&self) -> bool {
        self.shows_working() || self.is_bash_running()
    }

    /// Whether this session has work in flight: a run, a shell command, compaction, prompts
    /// waiting to be sent, or a prompt pi accepted but has not started showing yet. A busy chat
    /// must stay alive when the user switches away, or its messages are lost.
    pub fn is_busy(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Starting | Lifecycle::Ready)
            && (self.agent_busy() || self.is_bash_running())
    }

    /// [`Self::is_busy`] without shell commands.
    fn agent_busy(&self) -> bool {
        self.shows_working()
            || self.sending.is_some()
            || self.awaiting_start
            || !self.outbox.is_empty()
    }

    pub fn model(&self) -> Option<&Model> {
        self.state.as_ref().and_then(|state| state.model.as_ref())
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// The thinking level, as the one of [`Self::thinking_levels`] that does the same.
    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        let level = self
            .transcript
            .thinking_level()
            .or_else(|| self.state.as_ref().map(|state| state.thinking_level))?;
        Some(self.shown_thinking_level(level))
    }

    /// The one of [`Self::thinking_levels`] that does the same as `level`.
    pub fn shown_thinking_level(&self, level: ThinkingLevel) -> ThinkingLevel {
        match self.model() {
            Some(model) => model.shown_thinking_level(level, &self.thinking_levels),
            None => level,
        }
    }

    /// The thinking levels that differ on the model.
    pub fn thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self.model() {
            Some(model) => model.distinct_thinking_levels(&self.thinking_levels),
            None => self.thinking_levels.clone(),
        }
    }

    /// All levels pi accepts, also those that do the same as another on the model.
    pub fn accepted_thinking_levels(&self) -> &[ThinkingLevel] {
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

    /// How this chat uses the web (as its pi extension reports).
    pub fn web_mode(&self) -> WebMode {
        self.extension_status
            .get(deepsearch::STATUS_KEY)
            .and_then(|text| WebMode::parse(text))
            .unwrap_or_default()
    }

    /// How much effort this chat's deep search makes.
    pub fn web_effort(&self) -> Effort {
        self.extension_status
            .get(deepsearch::EFFORT_KEY)
            .and_then(|text| Effort::parse(text))
            .unwrap_or_default()
    }

    /// What deep search is doing right now.
    pub fn web_progress(&self) -> Option<&str> {
        self.extension_status
            .get(deepsearch::PROGRESS_KEY)
            .map(String::as_str)
    }

    /// Whether this chat's pi loaded web search (it offers the `/websearch` command).
    pub fn web_search_available(&self) -> bool {
        self.mode == Mode::Chat
            && self
                .commands
                .iter()
                .any(|command| command.name == deepsearch::COMMAND)
    }

    /// Sets this chat's web mode through the extension's `/websearch` command; the extension
    /// remembers it in the session and confirms it with its status.
    pub fn set_web_mode(&mut self, mode: WebMode, cx: &mut Context<Self>) {
        if !self.web_search_available() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        // Shown right away; the extension confirms it with its status, and a failed command
        // puts the previous mode back.
        let previous = self.web_mode();
        self.show_web_mode(mode, cx);
        if mode != WebMode::Off {
            crate::search_server::start_if_used(cx);
        }
        let command = format!("/{} {}", deepsearch::COMMAND, mode.as_str());
        cx.spawn(async move |this, cx| {
            let result = client
                .prompt(command, Vec::new(), Some(StreamingBehavior::FollowUp))
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Disposition::Handled) => {}
                Ok(_) => {
                    // pi did not run the command (the extension is gone): it went to the model.
                    this.show_web_mode(previous, cx);
                    this.push_notice(
                        NoticeKind::Error,
                        "Web search is not available in this chat.",
                        cx,
                    );
                }
                Err(error) => {
                    this.show_web_mode(previous, cx);
                    this.report(&error, cx);
                }
            });
        })
        .detach();
    }

    /// Sets the effort of this chat's deep search, from the next message on.
    pub fn set_web_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        if !self.web_search_available() || self.web_effort() == effort {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        // Shown right away, like the mode.
        let previous = self.web_effort();
        self.show_web_effort(effort, cx);
        let command = format!("/{} effort {}", deepsearch::COMMAND, effort.as_str());
        cx.spawn(async move |this, cx| {
            let result = client
                .prompt(command, Vec::new(), Some(StreamingBehavior::FollowUp))
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.show_web_effort(previous, cx);
                    this.report(&error, cx);
                }
            });
        })
        .detach();
    }

    fn show_web_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        self.extension_status
            .insert(deepsearch::EFFORT_KEY.into(), effort.as_str().into());
        cx.emit(SessionEvent::MetaChanged);
        cx.notify();
    }

    /// As if pi had loaded web search and set this.
    #[cfg(test)]
    pub fn set_web_for_tests(&mut self, mode: WebMode, effort: Effort, cx: &mut Context<Self>) {
        self.commands = vec![
            serde_json::from_value(serde_json::json!({
                "name": deepsearch::COMMAND, "source": "extension",
            }))
            .unwrap(),
        ];
        self.show_web_mode(mode, cx);
        self.show_web_effort(effort, cx);
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
        self.transcript.items().is_empty() && self.outbox.is_empty()
    }

    /// Whether nothing was sent in this conversation yet. pi writes a session's file with its
    /// first message, so a new one is not in the sidebar and has no file to resume.
    pub fn is_new(&self) -> bool {
        // A branch that starts before the first message is new too, unlike what it came from.
        let resumed = self.resumed_from.is_some() && self.session_file() == self.resumed_from;
        !resumed
            && !self.is_idle()
            && !self.is_busy()
            && self.outbox.is_empty()
            && self.transcript.items().iter().all(|item| {
                use transcript::ItemKind::{Bash, Notice};
                matches!(item.kind, Bash { .. } | Notice(_))
            })
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
        // Only a visible run can be steered; anything sent before it shows (typed while pi
        // starts, or right after another message) waits its turn as a follow-up.
        let follow_up = follow_up || !self.is_running();
        self.outbox.push_back(PendingPrompt {
            text,
            images,
            follow_up,
        });
        self.flush_outbox(cx);
        self.report_busy(cx);
        cx.notify();
    }

    /// Whether nothing was said in this conversation yet: a new session, no prompt shown.
    fn is_first_prompt(&self) -> bool {
        self.resumed_from.is_none()
            && !self
                .transcript
                .items()
                .iter()
                .any(|item| matches!(item.kind, transcript::ItemKind::User(_)))
    }

    /// Names the conversation after the prompt pi just accepted, if it is the first one.
    /// Later prompts never do.
    fn consider_naming(&mut self, sent: &Sending, cx: &mut Context<Self>) {
        if self.naming == Naming::NotYet {
            self.naming = Naming::Considered;
            if sent.first {
                self.start_naming(&sent.prompt.text, cx);
            }
        }
    }

    /// Starts naming the conversation after its first `message`, if the user turned automatic
    /// names on (see [`crate::naming`]). Naming runs beside the conversation, so a slow namer
    /// never holds it up.
    fn start_naming(&mut self, message: &str, cx: &mut Context<Self>) {
        let Some(settings) = cx.try_global::<crate::settings::AppSettings>() else {
            return;
        };
        if !settings.auto_names || self.title().is_some() || !titles::worth_naming(message) {
            return;
        }
        let namer = settings.namer;
        if let Some(title) = titles::as_is(message) {
            // Already what the row shows: saved, but not typed out again.
            self.apply_name(title, false, cx);
            return;
        }
        match namer {
            crate::naming::Namer::Local => match crate::naming::local_job(message, cx) {
                Some(job) => self.run_naming(job, cx),
                None => log::info!("no local naming model is installed; not naming"),
            },
            crate::naming::Namer::Conversation => {
                if let Some(job) = self.pi_naming_job(message.to_owned(), cx) {
                    self.run_naming(job, cx);
                }
            }
        }
    }

    /// Naming with the conversation's model, through a separate `pi --print` started the
    /// way this conversation's pi was (on its SSH host for a remote conversation).
    fn pi_naming_job(&self, message: String, cx: &gpui_kit::App) -> Option<crate::naming::Job> {
        let model = self.model()?;
        let host = saved_host(self.host.as_ref(), cx);
        if self.host.is_some() && host.is_none() {
            return None;
        }
        let config = cx.try_global::<AppConfig>()?;
        Some(crate::naming::Job::Pi {
            launch: Box::new(crate::naming::PiLaunch::new(config, self.cwd.clone(), host)),
            provider: model.provider.clone(),
            model: model.id.clone(),
            message,
        })
    }

    fn run_naming(&mut self, job: crate::naming::Job, cx: &mut Context<Self>) {
        self._naming_task = cx.spawn(async move |this, cx| {
            if let Some(title) = crate::naming::run(job).await {
                let _ = this.update(cx, |this, cx| this.apply_name(title, true, cx));
            }
        });
    }

    /// Saves an automatic name through pi, unless the conversation got a name meanwhile
    /// (Rename, `/name`): those are never replaced. `announce` has the sidebar type it out.
    fn apply_name(&mut self, title: String, announce: bool, cx: &mut Context<Self>) {
        if self.title().is_some() || self.client.is_none() {
            return;
        }
        self.rename(title.clone(), cx);
        if announce {
            crate::naming::announce(self.host.clone(), self.session_file(), title, cx);
        }
    }

    /// Whether prompts wait for a compaction between runs to end: pi refuses them while it
    /// lasts, and one sent just before it would run into it.
    fn holds_prompts(&self) -> bool {
        self.compaction.sent || self.compaction.running
    }

    /// Sends the next waiting prompt once pi is ready and has answered the previous one.
    fn flush_outbox(&mut self, cx: &mut Context<Self>) {
        if self.sending.is_some() || self.lifecycle != Lifecycle::Ready || self.holds_prompts() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(prompt) = self.outbox.pop_front() else {
            return;
        };
        // pi ignores the behaviour while idle and queues the prompt while a run is active.
        let behavior = Some(if prompt.follow_up {
            StreamingBehavior::FollowUp
        } else {
            StreamingBehavior::Steer
        });
        if self.mode == Mode::Chat && self.web_mode() != WebMode::Off {
            crate::search_server::start_if_used(cx);
        }
        let (text, images) = (prompt.text.clone(), prompt.images.clone());
        self.queued.sending(&text, prompt.follow_up);
        self.sending = Some(Sending {
            prompt,
            runs_before: self.runs_started,
            first: self.is_first_prompt(),
        });
        cx.spawn(async move |this, cx| {
            let result = client.prompt(text, images, behavior).await;
            if matches!(result, Err(RpcError::Disconnected)) {
                // Events pi sent before it stopped may still wait to be handled, and they say
                // whether the prompt's run started. The disconnect comes after them and gives
                // the prompt back; this is for when it doesn't come.
                cx.background_executor().timer(EVENT_GRACE).await;
            }
            let _ = this.update(cx, |this, cx| this.prompt_answered(result, cx));
        })
        .detach();
    }

    /// pi accepted or refused the prompt being sent.
    fn prompt_answered(&mut self, result: Result<Disposition, RpcError>, cx: &mut Context<Self>) {
        // pi stopped, and the prompt was given back already.
        let Some(sent) = self.sending.take() else {
            return;
        };
        if !matches!(result, Ok(Disposition::Queued)) {
            self.queued.not_queued();
        }
        match result {
            Ok(Disposition::Started) => {
                // Unless its run already showed up: then the transcript tracks it.
                self.awaiting_start = self.runs_started == sent.runs_before;
                self.consider_naming(&sent, cx);
            }
            // Joins the active run, which the transcript tracks.
            Ok(Disposition::Queued) => {
                self.consider_naming(&sent, cx);
                self.queued.accepted(sent.prompt.images);
            }
            // A command: nothing was said.
            Ok(Disposition::Handled) => {}
            // pi took the prompt, but its answer could not be read.
            Err(error @ RpcError::Decode { .. }) => self.report(&error, cx),
            Err(error) => {
                // The ones behind it go back too: sent now, they would be refused as well, or
                // be answered without it.
                self.outbox.push_front(sent.prompt);
                self.return_outbox(cx);
                self.report(&error, cx);
            }
        }
        self.flush_outbox(cx);
        self.report_busy(cx);
        cx.notify();
    }

    /// Emits [`SessionEvent::BusyChanged`] when [`Self::is_busy`] changed since last time.
    fn report_busy(&mut self, cx: &mut Context<Self>) {
        let busy = self.is_busy();
        if busy != self.reported_busy {
            self.reported_busy = busy;
            cx.emit(SessionEvent::BusyChanged);
        }
    }

    /// Removes the prompts waiting to be sent, returning their texts and images.
    fn take_outbox(&mut self, cx: &mut Context<Self>) -> (Vec<String>, Vec<ImageContent>) {
        if self.outbox.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut texts = Vec::new();
        let mut images = Vec::new();
        for prompt in self.outbox.drain(..) {
            if !prompt.text.is_empty() {
                texts.push(prompt.text);
            }
            images.extend(prompt.images);
        }
        self.report_busy(cx);
        cx.notify();
        (texts, images)
    }

    /// Gives prompts that can no longer be sent (pi exited, or refused one), or should not be
    /// (Esc), back to the composer.
    fn return_outbox(&mut self, cx: &mut Context<Self>) {
        let (texts, images) = self.take_outbox(cx);
        restore(&texts, images, cx);
    }

    /// Prompts typed but not sent to pi yet (pi is starting, or busy with the previous one).
    pub fn pending_prompts(&self) -> impl Iterator<Item = (&str, usize, bool)> {
        self.outbox
            .iter()
            .map(|prompt| (prompt.text.as_str(), prompt.images.len(), prompt.follow_up))
    }

    /// Cancels the `index`th prompt waiting to be sent and returns it to the composer.
    pub fn cancel_pending(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(prompt) = self.outbox.remove(index) {
            cx.emit(SessionEvent::RestoreToEditor {
                text: prompt.text,
                images: prompt.images,
                replace: false,
            });
            self.report_busy(cx);
            cx.notify();
        }
    }

    /// Esc: restore queued messages (pi's queue and prompts not sent yet) to the editor,
    /// then abort the run. While the agent rests, stop the shell commands instead (pi's
    /// terminal has the same order).
    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        if !self.agent_busy() && self.is_bash_running() {
            self.run(cx, async move |client| client.abort_bash().await);
            return;
        }
        // Prompts still waiting here would otherwise be sent right after the abort. They are
        // back in the editor before anything can let this chat go (it may not be busy now);
        // pi's queue goes before them.
        self.return_outbox(cx);
        let Some(client) = self.client.clone() else {
            return;
        };
        let queue = self.take_queue(client.clone(), cx);
        cx.spawn(async move |this, cx| {
            let (texts, images) = queue.await;
            let _ = this.update(cx, |_, cx| restore(&texts, images, cx));
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
        let queue = self.take_queue(client, cx);
        cx.spawn(async move |this, cx| {
            let (texts, images) = queue.await;
            let _ = this.update(cx, |_, cx| restore(&texts, images, cx));
        })
        .detach();
    }

    /// Takes the messages out of pi's queue: their texts and images.
    fn take_queue(
        &mut self,
        client: PiClient,
        cx: &mut Context<Self>,
    ) -> Task<(Vec<String>, Vec<ImageContent>)> {
        self.clearing_queue += 1;
        cx.spawn(async move |this, cx| {
            let cleared = client.clear_queue().await;
            this.update(cx, |this, _| {
                this.clearing_queue -= 1;
                let Ok(cleared) = cleared else {
                    return (Vec::new(), Vec::new());
                };
                let images = this.queued.take(&cleared.steering, &cleared.follow_up);
                let texts = cleared.steering.into_iter().chain(cleared.follow_up);
                // One with only images has no text.
                (texts.filter(|text| !text.is_empty()).collect(), images)
            })
            .unwrap_or_default()
        })
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
        let levels = self.thinking_levels();
        let (Some(current), false) = (self.thinking_level(), levels.is_empty()) else {
            return;
        };
        let index = levels
            .iter()
            .position(|level| *level == current)
            .unwrap_or(0);
        let next = levels[(index + 1) % levels.len()];
        self.set_thinking_level(next, cx);
    }

    /// Compacts the conversation (`/compact`), aborting a run like pi's terminal does.
    /// Prompts submitted meanwhile wait.
    pub fn compact(&mut self, instructions: Option<String>, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        if self.holds_prompts() {
            let text = "The conversation is already being compacted.";
            self.push_notice(NoticeKind::Info, text, cx);
            return;
        }
        self.compaction.sent = true;
        let ended = self.compaction.ended;
        cx.spawn(async move |this, cx| {
            let result = client.compact(instructions).await;
            let _ = this.update(cx, |this, cx| {
                this.compaction.sent = false;
                this.compaction.running = false;
                this.flush_outbox(cx);
                this.transcript_changed(cx);
            });
            match result {
                // A lost connection says so itself.
                Ok(()) | Err(RpcError::Disconnected) => return,
                Err(RpcError::Rejected { .. }) => {
                    // pi tells how a compaction ended with an event, which may still wait to
                    // be handled.
                    cx.background_executor().timer(EVENT_GRACE).await;
                }
                Err(_) => {}
            }
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result
                    && this.compaction.ended == ended
                {
                    this.report(&error, cx);
                }
            });
        })
        .detach();
        self.transcript_changed(cx);
    }

    pub fn rename(&mut self, name: String, cx: &mut Context<Self>) {
        self.transcript.set_session_name(Some(name.clone()));
        cx.emit(SessionEvent::SessionFileChanged);
        self.run(cx, async move |client| client.set_session_name(name).await);
    }

    /// [`Self::rename`] for a pi that may still be starting; tells when pi has saved the name.
    pub fn rename_when_ready(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let (changed, changes) = async_channel::bounded(1);
        let observed = cx.observe_self(move |_, _| {
            let _ = changed.try_send(());
        });
        cx.spawn(async move |this, cx| {
            let _observed = observed;
            let gone = |_| "pi was stopped.".to_owned();
            let client = loop {
                let client = this
                    .update(cx, |this, _| match &this.lifecycle {
                        Lifecycle::Ready => Ok(this.client.clone()),
                        Lifecycle::Exited { message } => Err(message.clone()),
                        Lifecycle::Idle | Lifecycle::Starting => Ok(None),
                    })
                    .map_err(gone)??;
                match client {
                    Some(client) => break client,
                    None => changes.recv().await.map_err(|error| error.to_string())?,
                }
            };
            client
                .set_session_name(name.clone())
                .await
                .map_err(|error| match error {
                    RpcError::Rejected { message, .. } => message,
                    other => other.to_string(),
                })?;
            this.update(cx, |this, cx| {
                this.transcript.set_session_name(Some(name));
                cx.emit(SessionEvent::SessionFileChanged);
                cx.notify();
            })
            .map_err(gone)
        })
    }

    /// `!cmd` / `!!cmd`: run a shell command through pi (added to context unless `!!`). Its
    /// row shows its output as it comes.
    pub fn run_bash(
        &mut self,
        command: String,
        exclude_from_context: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        let id = client.next_id();
        self.transcript
            .start_bash(&id, command.clone(), Instant::now());
        cx.spawn(async move |this, cx| {
            let result = client.bash(id.clone(), command, exclude_from_context).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => this.transcript.finish_bash(&id, result),
                    // The disconnect says why.
                    Err(RpcError::Disconnected) => {
                        this.transcript.stop_bash(&id, BashStatus::Cancelled);
                    }
                    Err(error) => {
                        this.transcript.stop_bash(&id, BashStatus::Failed);
                        this.report(&error, cx);
                    }
                }
                this.transcript_changed(cx);
            });
        })
        .detach();
        self.transcript_changed(cx);
    }

    fn transcript_changed(&mut self, cx: &mut Context<Self>) {
        self.report_busy(cx);
        cx.emit(SessionEvent::TranscriptChanged);
        cx.notify();
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

    // ---- history ---------------------------------------------------------------

    /// Branches the conversation at a user message: pi continues in a new session file that
    /// holds everything before that message (the original stays as it was). `from_end` counts
    /// the transcript's user messages from the last one (0); `text` is that message, to check
    /// the match. Then `after` decides what happens with the message.
    pub fn fork_at(
        &mut self,
        from_end: usize,
        text: String,
        after: AfterFork,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if self.is_busy() {
            self.push_notice(
                NoticeKind::Info,
                "Wait for the current response to finish before branching.",
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = async {
                let messages = client.get_fork_messages().await?;
                let Some(entry_id) = fork_entry(&messages, from_end, &text) else {
                    return Ok(None);
                };
                // The transcript keeps only an image count; Edit and Retry need the images.
                let images = if after == AfterFork::Keep {
                    Vec::new()
                } else {
                    client
                        .get_messages()
                        .await
                        .map(|messages| user_images(&messages, from_end, &text))
                        .unwrap_or_default()
                };
                Ok(client.fork(entry_id).await?.map(|text| (text, images)))
            }
            .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Some((text, images))) => this.reload(Some((text, images, after)), cx),
                Ok(None) => this.push_notice(
                    NoticeKind::Warning,
                    "pi could not branch from this message (it may be before a compaction).",
                    cx,
                ),
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// pi's user messages that a branch can start from, oldest first (for `/fork`).
    pub fn fork_messages(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<pi_rpc::protocol::ForkMessage>, RpcError>> {
        let client = self.client.clone();
        cx.background_executor().spawn(async move {
            match client {
                Some(client) => client.get_fork_messages().await,
                None => Err(RpcError::Disconnected),
            }
        })
    }

    /// Branches at pi's fork entry `entry_id` (from [`Self::fork_messages`]).
    pub fn fork_entry_id(&mut self, entry_id: String, after: AfterFork, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.fork(entry_id).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Some(text)) => this.reload(Some((text, Vec::new(), after)), cx),
                Ok(None) => {}
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Continues in a copy of the whole conversation (a new session file).
    pub fn clone_session(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        // Replacing the transcript mid-run would lose track of the run still streaming.
        if self.is_busy() {
            self.push_notice(
                NoticeKind::Info,
                "Wait for the current response to finish before cloning.",
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = client.clone_session().await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(true) => this.reload(None, cx),
                Ok(false) => {}
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Exports the conversation as HTML (to `path`, or next to the session) and opens it.
    pub fn export_html(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.export_html(path).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.push_notice(NoticeKind::Info, format!("Exported to {path}"), cx);
                    if this.host.is_none() {
                        cx.open_with_system(Path::new(&path));
                    }
                }
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Uploads the conversation (as HTML) to a secret GitHub gist with `gh`, like pi's
    /// terminal `/share`, and copies the viewer link.
    pub fn share(&mut self, cx: &mut Context<Self>) {
        if self.host.is_some() {
            self.push_notice(
                NoticeKind::Info,
                "Sharing works for sessions on this computer; use /export on the host instead.",
                cx,
            );
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let env = cx.global::<AppConfig>().child_env.clone();
        let dir = std::env::temp_dir().join(format!("pistachio-share-{}", std::process::id()));
        let file = dir.join("session.html");
        self.push_notice(NoticeKind::Info, "Creating a secret gist…", cx);
        cx.spawn(async move |this, cx| {
            let _ = std::fs::create_dir_all(&dir);
            let result = match client
                .export_html(Some(file.to_string_lossy().into_owned()))
                .await
            {
                Ok(path) => {
                    cx.background_executor()
                        .spawn(async move { create_gist(&path, &env) })
                        .await
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = std::fs::remove_dir_all(&dir);
            let _ = this.update(cx, |this, cx| match result {
                Ok((viewer, gist)) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(viewer.clone()));
                    this.push_notice(
                        NoticeKind::Info,
                        format!("Shared (link copied): {viewer}\nGist: {gist}"),
                        cx,
                    );
                }
                Err(error) => this.push_notice(NoticeKind::Error, error, cx),
            });
        })
        .detach();
    }

    /// Reads the state and history again after pi switched session files in this process,
    /// then handles the message a fork started from.
    fn reload(
        &mut self,
        forked: Option<(String, Vec<ImageContent>, AfterFork)>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let state = client.get_state().await;
            let messages = client.get_messages().await;
            let stats = client.get_session_stats().await.ok();
            let _ = this.update(cx, |this, cx| {
                if let Ok(state) = state {
                    this.state = Some(state);
                }
                match messages {
                    Ok(messages) => {
                        this.transcript = Transcript::from_messages(&messages);
                        let name = this.state.as_ref().and_then(|s| s.session_name.clone());
                        this.transcript.set_session_name(name);
                    }
                    Err(error) => this.report(&error, cx),
                }
                this.stats = stats;
                // Another session in pi: the old one's queue and compaction are over.
                this.queued.clear();
                this.compaction.running = false;
                cx.emit(SessionEvent::Reloaded);
                cx.emit(SessionEvent::SessionFileChanged);
                cx.emit(SessionEvent::MetaChanged);
                match forked {
                    Some((text, images, AfterFork::Edit)) => {
                        cx.emit(SessionEvent::RestoreToEditor {
                            text,
                            images,
                            replace: true,
                        });
                    }
                    Some((text, images, AfterFork::Resend)) => this.submit(text, images, false, cx),
                    Some((_, _, AfterFork::Keep)) | None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_web_mode(&mut self, mode: WebMode, cx: &mut Context<Self>) {
        if mode == WebMode::Off {
            self.extension_status.remove(deepsearch::STATUS_KEY);
        } else {
            self.extension_status
                .insert(deepsearch::STATUS_KEY.into(), mode.as_str().into());
        }
        cx.emit(SessionEvent::MetaChanged);
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

    /// Shows a resumed local session's history right away, read from its file on the
    /// background executor, instead of waiting a second or so for pi to start and send it.
    /// pi's history replaces the preview when it arrives, unless they are the same.
    fn show_preview(&mut self, cx: &mut Context<Self>) {
        let (None, Some(file)) = (&self.host, self.resumed_from.clone()) else {
            return;
        };
        self._preview_task = cx.spawn(async move |this, cx| {
            let messages = cx
                .background_executor()
                .spawn(async move { read_preview(&file) })
                .await;
            let Some(messages) = messages else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.preview != Preview::None {
                    return;
                }
                let notices = this.notices();
                this.transcript = Transcript::from_messages(&messages);
                for notice in notices {
                    this.transcript.push_notice(notice.kind, notice.text);
                }
                this.preview = Preview::Shown;
                cx.emit(SessionEvent::Reloaded);
                cx.notify();
            });
        });
    }

    /// Notices in the transcript (e.g. an extension that failed to load while pi started).
    fn notices(&self) -> Vec<transcript::Notice> {
        self.transcript
            .items()
            .iter()
            .filter_map(|item| match &item.kind {
                transcript::ItemKind::Notice(notice) => Some(notice.clone()),
                _ => None,
            })
            .collect()
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
                        this.return_outbox(cx);
                    }
                }
                this.models = models;
                this.thinking_levels = levels;
                this.stats = stats;
                this.commands = commands;
                let mut replaced = false;
                if let Some(messages) = messages {
                    let name = this.transcript.session_name().map(str::to_owned);
                    // Notices pi sent while starting (e.g. an extension that failed to load)
                    // stay visible after the history.
                    let notices = this.notices();
                    let mut fresh = Transcript::from_messages(&messages);
                    for notice in notices {
                        fresh.push_notice(notice.kind, notice.text);
                    }
                    // The preview from the file is usually exactly this: keep it, so the rows
                    // (and the scroll position) stay as they are.
                    let same =
                        this.preview == Preview::Shown && fresh.items() == this.transcript.items();
                    if !same {
                        this.transcript = fresh;
                        replaced = true;
                    }
                    this.preview = Preview::Superseded;
                    this.transcript.set_session_name(
                        name.or_else(|| this.state.as_ref().and_then(|s| s.session_name.clone())),
                    );
                }
                this.flush_outbox(cx);
                this.report_busy(cx);
                if this.lifecycle == Lifecycle::Ready {
                    this.apply_pending_model(cx);
                }
                // A replaced transcript restarts item ids: rebuild every row.
                cx.emit(if replaced {
                    SessionEvent::Reloaded
                } else {
                    SessionEvent::TranscriptChanged
                });
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
                    pi_rpc::protocol::Event::AgentStart => {
                        self.runs_started += 1;
                        self.awaiting_start = false;
                        self.compaction.running = false;
                    }
                    pi_rpc::protocol::Event::QueueUpdate {
                        steering,
                        follow_up,
                    } if self.clearing_queue == 0 => self.queued.update(&steering, &follow_up),
                    pi_rpc::protocol::Event::AgentSettled { .. } => {
                        self.awaiting_start = false;
                        if self.clearing_queue == 0 && self.transcript.queue().is_empty() {
                            self.queued.clear();
                        }
                        self.refresh_stats(cx);
                        cx.emit(SessionEvent::Settled);
                    }
                    pi_rpc::protocol::Event::CompactionStart {
                        reason: CompactionReason::Manual,
                    } => self.compaction.running = true,
                    pi_rpc::protocol::Event::CompactionEnd {
                        reason: CompactionReason::Manual,
                        ..
                    } => {
                        self.compaction.running = false;
                        self.compaction.ended += 1;
                        self.flush_outbox(cx);
                        self.refresh_stats(cx);
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
                if let Some(sent) = self.sending.take()
                    && self.runs_started == sent.runs_before
                {
                    // pi stopped before it took the prompt.
                    self.outbox.push_front(sent.prompt);
                }
                self.awaiting_start = false;
                self.compaction.sent = false;
                self.compaction.running = false;
                self.queued = QueuedPrompts::default();
                self.return_outbox(cx);
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

/// Gives messages back to the composer, if there are any.
fn restore(texts: &[String], images: Vec<ImageContent>, cx: &mut Context<PiSession>) {
    if !texts.is_empty() || !images.is_empty() {
        cx.emit(SessionEvent::RestoreToEditor {
            text: texts.join("\n\n"),
            images,
            replace: false,
        });
    }
}

/// The saved SSH host `host` with the `pi` to run there.
fn saved_host(host: Option<&HostId>, cx: &gpui_kit::App) -> Option<remote::SavedHost> {
    let id = host?;
    let hosts = cx.try_global::<crate::hosts::HostsHandle>()?.0.read(cx);
    let mut saved = hosts.get(id).cloned()?;
    // Prefer the exact path the probe found (conda / nvm installs may only be on an
    // interactive shell's PATH; pistachio's own install is not on PATH at all).
    saved.pi_command = hosts.pi_command(id);
    Some(saved)
}

/// How to launch pi for a session in `cwd`. Chat sessions get only the web tools
/// (`--tools web_search,web_fetch`, which the extension keeps off until the chat turns web
/// search on), no MCP, and no built-in tools: a chat can never read or change files. An
/// explicit `--tools` list wins over the tool set pi recorded in the session, also for chats
/// started with `--no-tools` before web search existed. If the extension cannot be written,
/// chats fall back to `--no-tools`.
///
/// On a remote `host`, pi runs there through the host's shared ssh connection, in `cwd` on the
/// remote, via the login shell (see [`remote::Ssh::command`]); remote sessions are always Code.
/// The active branch's messages from a session file, or `None` when pi has to rebuild them
/// (see [`pi_sessions::branch`]) or the file can't be read.
fn read_preview(file: &Path) -> Option<Vec<AgentMessage>> {
    let text = std::fs::read_to_string(file).ok()?;
    pi_sessions::branch::active_messages(&text)?
        .into_iter()
        .map(|message| serde_json::from_value(message).ok())
        .collect()
}

fn spawn_options(
    config: &AppConfig,
    cwd: &Path,
    session_file: Option<&Path>,
    host: Option<&remote::SavedHost>,
    remote_env: &[(String, String)],
) -> (SpawnOptions, Mode) {
    let mut options = SpawnOptions::new(cwd);
    options.program.clone_from(&config.pi_program);
    config
        .child_env
        .ready()
        .unwrap_or_default()
        .clone_into(&mut options.env);
    options.session = session_file.map(Path::to_path_buf);
    let Some(host) = host else {
        let mode = config.mode_of(cwd);
        if mode == Mode::Chat {
            chat_args(config, &mut options);
        } else {
            builtin_skill_args(config, &mut options);
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
    let (program, args) = ssh.command_with_env(Some(&cwd.to_string_lossy()), remote_env, &argv);
    options.launcher = Some(pi_rpc::Launcher { program, args });
    // The local ssh client just needs some existing folder to run in.
    options.cwd = config.home.clone().unwrap_or_else(std::env::temp_dir);
    (options, Mode::Code)
}

/// `--skill` for each skill that comes with the app (see [`crate::builtin_skills`]). Chats
/// leave them out: they have no tools to act on them.
fn builtin_skill_args(config: &AppConfig, options: &mut SpawnOptions) {
    match crate::builtin_skills::install(&config.builtin_skills_dir) {
        Ok(folders) => {
            for folder in folders {
                options.extra_args.extend(["--skill".into(), folder.into()]);
            }
        }
        Err(error) => log::warn!(
            "the built-in skills are unavailable: could not write them to {}: {error}",
            config.builtin_skills_dir.display()
        ),
    }
}

/// pi's arguments and environment for a chat: the web tools only (see [`spawn_options`]).
fn chat_args(config: &AppConfig, options: &mut SpawnOptions) {
    let extension = match (
        &config.exe,
        deepsearch::install_extension(&config.deepsearch_dir),
    ) {
        (Some(exe), Ok(extension)) => Some((exe, extension)),
        (None, _) => {
            log::warn!("web search is unavailable: the app's own executable could not be found");
            None
        }
        (_, Err(error)) => {
            log::warn!(
                "web search is unavailable: could not write its extension to {}: {error}",
                config.deepsearch_dir.display()
            );
            None
        }
    };
    let Some((exe, extension)) = extension else {
        options.extra_args.push("--no-tools".into());
        return;
    };
    options.extra_args.extend([
        "--tools".into(),
        deepsearch::TOOL_NAMES.join(",").into(),
        "--no-mcp".into(),
        "--extension".into(),
        extension.into(),
    ]);
    options
        .env
        .push((deepsearch::BIN_ENV.into(), exe.clone().into()));
    options.env.push((
        deepsearch::config::DIR_ENV.into(),
        config.deepsearch_dir.clone().into(),
    ));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn fork_entries_match_from_the_end() {
        let message = |id: &str, text: &str| pi_rpc::protocol::ForkMessage {
            entry_id: id.into(),
            text: text.into(),
        };
        // pi still lists a prompt from before a compaction that the transcript no longer has.
        let messages = [
            message("e0", "old"),
            message("e1", "first"),
            message("e2", "second"),
        ];
        assert_eq!(
            super::fork_entry(&messages, 0, "second").as_deref(),
            Some("e2")
        );
        assert_eq!(
            super::fork_entry(&messages, 1, " first\n").as_deref(),
            Some("e1")
        );
        assert_eq!(super::fork_entry(&messages, 1, "other"), None);
        assert_eq!(super::fork_entry(&messages, 3, "x"), None);
    }

    #[test]
    fn chat_sessions_launch_with_only_deep_search_tools() {
        let config = AppConfig::for_tests("pi".into());
        let (chat, mode) = super::spawn_options(&config, &config.chat_dir.clone(), None, None, &[]);
        assert_eq!(mode, super::Mode::Chat);
        let extension = config.deepsearch_dir.join("extension.ts");
        assert_eq!(
            chat.extra_args,
            [
                "--tools".into(),
                "web_search,web_fetch".into(),
                "--no-mcp".into(),
                "--extension".into(),
                std::ffi::OsString::from(&extension),
            ]
        );
        assert!(extension.exists());
        assert!(chat.env.contains(&(
            deepsearch::BIN_ENV.into(),
            std::ffi::OsString::from("/app/pistachio")
        )));

        let project = std::env::temp_dir().join("some-project");
        let session = project.join("s.jsonl");
        let (code, mode) = super::spawn_options(&config, &project, Some(&session), None, &[]);
        assert_eq!(mode, super::Mode::Code);
        assert_eq!(
            code.extra_args,
            [
                "--skill".into(),
                std::ffi::OsString::from(config.builtin_skills_dir.join("add-skill")),
            ]
        );
        assert!(
            config
                .builtin_skills_dir
                .join("add-skill/SKILL.md")
                .exists()
        );
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
        let (options, mode) = super::spawn_options(&config, folder, None, Some(&host), &[]);
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
    /// `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored real_pi_over_ssh`
    /// The preview read from a session file shows exactly what pi's own history does. Runs
    /// real pi on every `.jsonl` in `PISTACHIO_PREVIEW_CHECK` (use copies: pi may migrate a
    /// file it opens).
    #[test]
    #[ignore = "needs pi and session files"]
    fn preview_matches_pi() {
        use futures::executor::block_on;
        let Ok(dir) = std::env::var("PISTACHIO_PREVIEW_CHECK") else {
            return;
        };
        let (mut same, mut left_to_pi) = (0, 0);
        for entry in std::fs::read_dir(&dir).expect("readable folder") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Some(preview) = super::read_preview(&path) else {
                left_to_pi += 1;
                continue;
            };
            let mut options = pi_rpc::SpawnOptions::new(std::env::temp_dir());
            options.session = Some(path.clone());
            let (process, _events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi");
            let messages = block_on(process.client().get_messages()).expect("get_messages");
            let ours = transcript::Transcript::from_messages(&preview);
            let theirs = transcript::Transcript::from_messages(&messages);
            assert_eq!(
                ours.items(),
                theirs.items(),
                "preview differs from pi for {}",
                path.display()
            );
            same += 1;
        }
        eprintln!("{same} previews match pi; {left_to_pi} left to pi");
        assert!(same > 0, "no session was checked");
    }

    #[test]
    #[ignore = "runs pi on PISTACHIO_SSH_TEST_HOST"]
    fn real_pi_over_ssh() {
        use futures::executor::block_on;
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
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
                "d=$(mktemp -d /tmp/pistachio-e2e.XXXXXX) && cd \"$d\" && mkdir -p proj agent && \
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
        let (options, mode) = super::spawn_options(
            &config,
            std::path::Path::new(&folder),
            None,
            Some(&host),
            &[],
        );
        assert_eq!(mode, super::Mode::Code);
        let (process, events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi over ssh");
        let client = process.client().clone();
        block_on(client.get_state()).expect("get_state");
        let result = block_on(client.bash(
            client.next_id(),
            "printf e2e > made-by-pi.txt && pwd -P".into(),
            false,
        ))
        .expect("bash through pi");
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
            &[],
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

    /// pi on an SSH host uses a model configured only on this computer, through the relay:
    /// a fake OpenAI-compatible server here (on loopback, unreachable from the host) answers,
    /// and gets the real key and header that only this computer has.
    /// `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored models_relay_over_ssh`
    #[cfg(unix)]
    #[test]
    #[ignore = "runs pi on PISTACHIO_SSH_TEST_HOST"]
    fn models_relay_over_ssh() {
        use std::io::{BufRead as _, BufReader, Read as _, Write as _};
        use std::sync::{Arc, Mutex};

        use futures::executor::block_on;
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
            return;
        };

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut head = Vec::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        head.push(line.trim().to_owned());
                    }
                    let header = |name: &str| {
                        head.iter()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case(name)
                                    .then(|| value.trim().to_owned())
                            })
                            .unwrap_or_default()
                    };
                    let length: usize = header("content-length").parse().unwrap_or(0);
                    let mut body = vec![0; length];
                    let _ = reader.read_exact(&mut body);
                    log.lock().unwrap().push(format!(
                        "{} auth={} extra={}",
                        head.first().cloned().unwrap_or_default(),
                        header("authorization"),
                        header("x-extra")
                    ));
                    let mut out = reader.into_inner();
                    // A careless server that writes the credentials it got into its answer.
                    if head
                        .first()
                        .is_some_and(|line| line.starts_with("GET /v1/echo"))
                    {
                        let body = format!(
                            "{{\"got\":\"{}\",\"extra\":\"{}\"}}",
                            header("authorization"),
                            header("x-extra")
                        );
                        let _ = write!(
                            out,
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        return;
                    }
                    let _ = out.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                    );
                    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
                        let event = serde_json::json!({
                            "id": "e2e", "object": "chat.completion.chunk", "created": 0,
                            "model": "fake-1",
                            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
                        });
                        format!("data: {event}\n\n")
                    };
                    let mut text = chunk(
                        serde_json::json!({ "role": "assistant", "content": "Hello from" }),
                        serde_json::Value::Null,
                    );
                    text += &chunk(
                        serde_json::json!({ "content": " this computer." }),
                        serde_json::Value::Null,
                    );
                    text += &chunk(serde_json::json!({}), "stop".into());
                    text += "data: [DONE]\n\n";
                    let _ = out.write_all(text.as_bytes());
                });
            }
        });

        // This computer's pi settings: one model on the fake server, with a key and a header
        // that must never reach the host.
        let local = tempfile::tempdir().unwrap();
        std::fs::write(
            local.path().join("models.json"),
            serde_json::json!({ "providers": { "fake": {
                "baseUrl": format!("http://127.0.0.1:{port}/v1"),
                "api": "openai-completions",
                "apiKey": "real-secret-key",
                "headers": { "x-extra": "real-header" },
                "models": [{ "id": "fake-1", "name": "Fake One" }],
            }}})
            .to_string(),
        )
        .unwrap();
        std::fs::write(local.path().join("auth.json"), "{}").unwrap();
        let launch = crate::relay::Launch {
            program: crate::config::default_pi_program(),
            env: vec![(
                std::ffi::OsString::from("PI_CODING_AGENT_DIR"),
                local.path().as_os_str().to_owned(),
            )]
            .into(),
            dir: local.path().join("relay"),
        };

        let ssh = remote::Ssh::new(remote::Target::Alias {
            alias: alias.clone(),
        });
        ssh.connect(false).expect("connect");
        let probe = ssh.probe("pi").expect("probe");
        let host_id = remote::HostId("e2e-relay".into());
        let token = crate::relay::token(&host_id);
        let link = crate::relay::link(&ssh, &host_id, &probe, None, &launch).expect("link");
        eprintln!("linked on port {}: {:?}", link.port, link.env);
        let agent = &link.env[0].1;
        let models = String::from_utf8(
            ssh.read_file(&format!("{agent}/models.json"))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            models.contains(&format!("/p/fake/http/127.0.0.1:{port}/v1")),
            "{models}"
        );
        assert!(
            !models.contains("real-"),
            "no credential on the host: {models}"
        );

        // Linking again on the same connection keeps the port.
        let again = crate::relay::link(&ssh, &host_id, &probe, Some(&link), &launch).unwrap();
        assert_eq!(again.port, link.port);

        // pi there, with its sessions in a scratch folder.
        let scratch = ssh
            .exec("mktemp -d /tmp/pistachio-relay.XXXXXX")
            .unwrap()
            .trim()
            .to_owned();
        // With the relay port, so pi starts only once that answers.
        let env = vec![
            link.env[0].clone(),
            ("PI_CODING_AGENT_SESSION_DIR".to_owned(), scratch.clone()),
            link.env[2].clone(),
        ];
        let host = remote::SavedHost {
            id: host_id,
            label: alias,
            target: ssh.target().clone(),
            pi_command: probe.pi_path.clone(),
            recent_folders: Vec::new(),
        };
        let config = AppConfig::for_tests("pi".into());
        let (options, _) = super::spawn_options(
            &config,
            std::path::Path::new(&scratch),
            None,
            Some(&host),
            &env,
        );
        let (process, events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi over ssh");
        let client = process.client().clone();
        let available = block_on(client.get_available_models()).expect("models");
        eprintln!(
            "available on the host: {:?}",
            available
                .iter()
                .map(|m| m.qualified_id())
                .collect::<Vec<_>>()
        );
        assert!(available.iter().any(|model| model.id == "fake-1"));
        block_on(client.request(pi_rpc::protocol::Command::SetModel {
            provider: "fake".into(),
            model_id: "fake-1".into(),
        }))
        .expect("set model");
        block_on(client.prompt("hi".into(), Vec::new(), None)).expect("prompt accepted");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            match block_on(events.recv()) {
                Ok(pi_rpc::ClientEvent::Event(event))
                    if matches!(*event, pi_rpc::protocol::Event::AgentSettled { .. }) =>
                {
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let messages = block_on(client.get_messages()).expect("messages");
        drop(process);
        // Without the host's token, or for the relay's own models list, the host gets nothing.
        let refused = ssh
            .exec(&format!(
                "command -v curl >/dev/null || exit 0; \
                 curl -s -m 5 -X POST -H 'authorization: Bearer pistachio-{}-{}' -d '{{}}' \
                   http://127.0.0.1:{}/p/fake/http/127.0.0.1:{port}/v1/chat/completions; echo; \
                 curl -s -m 5 http://127.0.0.1:{}/_pistachio/models",
                "0".repeat(16),
                "0".repeat(64),
                link.port,
                link.port
            ))
            .unwrap();
        eprintln!("without the token: {refused}");
        // An answer that is not a stream never carries this computer's credentials back.
        let echoed = ssh
            .exec(&format!(
                "command -v curl >/dev/null || exit 0; \
                 curl -s -m 5 -H 'authorization: Bearer {token}' \
                   http://127.0.0.1:{}/p/fake/http/127.0.0.1:{port}/v1/echo",
                link.port
            ))
            .unwrap();
        eprintln!("echoed: {echoed}");
        assert!(
            echoed.is_empty() || (echoed.contains("[redacted]") && !echoed.contains("real-")),
            "{echoed}"
        );
        assert!(
            refused.is_empty()
                || (refused.contains("did not come from a Pistachio session")
                    && refused.contains("forbidden")),
            "{refused}"
        );
        let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&scratch)));
        let transcript = serde_json::to_string(&messages).unwrap();
        let seen = seen.lock().unwrap().clone();
        eprintln!("the fake server saw: {seen:?}");
        assert!(
            transcript.contains("Hello from this computer."),
            "{transcript}"
        );
        assert!(
            seen.iter()
                .any(|request| request.starts_with("POST /v1/chat/completions")
                    && request.ends_with("auth=Bearer real-secret-key extra=real-header")),
            "{seen:?}"
        );
        crate::relay::stop();
    }

    use gpui_kit::{AppContext as _, TestAppContext};
    use transcript::{BlockKind, ItemKind};

    use super::{Lifecycle, PiSession, SessionEvent};
    use crate::config::AppConfig;

    /// The "Add model" flow against real pi (`pi` on PATH): pi starts without the model, the
    /// model is added to models.json the way the dialog does it, pi is restarted (as the
    /// window does after saving), and the new model gets selected.
    /// `cargo test -p pistachio -- --ignored added_model_is_loaded_and_selected`
    #[cfg(unix)]
    #[gpui_kit::test]
    #[ignore = "needs pi on PATH"]
    async fn added_model_is_loaded_and_selected(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let agent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut config = AppConfig::for_tests("pi".into());
        config.child_env = vec![("PI_CODING_AGENT_DIR".into(), agent.path().into())].into();
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
                .all(|m| m.provider != "pistachio-test")
        }));

        let mut file = pi_config::ModelsFile::parse("").unwrap();
        let provider = pi_config::NewProvider {
            id: "pistachio-test".into(),
            name: "Pistachio test".into(),
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
        file.add_model("pistachio-test", Some(&provider), &model)
            .unwrap();
        std::fs::write(agent.path().join("models.json"), file.to_text()).unwrap();
        drop(before);

        let after = cx.new(|cx| PiSession::new(project.path().into(), None, None, cx));
        after.update(cx, |session, cx| {
            session.select_model_when_ready("pistachio-test".into(), "added-model".into(), cx);
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
            if current.as_deref() == Some("pistachio-test/added-model") {
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
    /// Messages typed while pi starts are all sent, in order, one after another, and the
    /// session counts as busy from the first keystroke until the last run settles.
    #[gpui_kit::test]
    async fn prompts_typed_during_startup_are_all_sent(cx: &mut TestAppContext) {
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
        session.update(cx, |session, cx| {
            assert_eq!(session.lifecycle(), &Lifecycle::Starting);
            session.submit("first".into(), Vec::new(), false, cx);
            session.submit("second".into(), Vec::new(), false, cx);
            assert!(session.is_busy(), "waiting messages keep the chat busy");
        });

        let mut settled = 0;
        while settled < 2 || cx.read(|cx| session.read(cx).is_busy()) {
            if let SessionEvent::Settled = events.recv().await.expect("session alive") {
                settled += 1;
            }
        }
        cx.read(|cx| {
            let prompts: Vec<String> = session
                .read(cx)
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::User(user) => Some(user.text.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(prompts, ["first", "second"]);
        });
    }

    /// While the login shell is still being asked for its environment, a new session opens at
    /// once, shows its history and takes prompts; pi starts with the environment when it is
    /// there.
    #[cfg(unix)]
    #[gpui_kit::test]
    async fn pi_starts_once_the_environment_is_known(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        let script = dir.path().join("pi");
        let seen = dir.path().join("seen");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$FAKE_API_KEY\" > '{}'\nexec '{}' \"$@\"\n",
                seen.display(),
                fake.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let mut config = AppConfig::for_tests(script);
        config.child_env = crate::config::ChildEnv::resolve(move || {
            let _ = held.recv();
            vec![("FAKE_API_KEY".into(), "from the shell".into())]
        });
        cx.update(|cx| cx.set_global(config));

        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        session.update(cx, |session, cx| {
            session.submit("early".into(), Vec::new(), false, cx);
            assert!(session.is_busy());
        });
        cx.run_until_parked();
        assert!(!seen.exists(), "pi must not start yet");
        cx.read(|cx| assert_eq!(session.read(cx).lifecycle(), &Lifecycle::Starting));

        release.send(()).unwrap();
        while !matches!(
            events.recv().await.expect("session alive"),
            SessionEvent::Settled
        ) {}
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().trim(),
            "from the shell"
        );
        cx.read(|cx| assert_eq!(session.read(cx).lifecycle(), &Lifecycle::Ready));
    }

    /// Prompts typed while pi could not start yet go back to the composer when it can't.
    #[gpui_kit::test]
    async fn prompts_come_back_when_a_delayed_pi_cannot_start(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let mut config = AppConfig::for_tests("/nonexistent/pi".into());
        config.child_env = crate::config::ChildEnv::resolve(move || {
            let _ = held.recv();
            Vec::new()
        });
        cx.update(|cx| cx.set_global(config));
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        session.update(cx, |session, cx| {
            session.submit("early".into(), Vec::new(), false, cx);
        });
        release.send(()).unwrap();
        loop {
            if let SessionEvent::RestoreToEditor { text, .. } =
                events.recv().await.expect("session alive")
            {
                assert_eq!(text, "early");
                break;
            }
        }
        cx.read(|cx| {
            let session = session.read(cx);
            assert!(matches!(session.lifecycle(), Lifecycle::Exited { .. }));
            assert!(!session.is_busy());
        });
    }

    /// Automatic names with the conversation's own model: the name comes from a separate
    /// `pi --print --no-session` (a stand-in script here), is saved through pi, and the
    /// conversation itself only ever sees the user's prompt.
    #[cfg(unix)]
    #[gpui_kit::test]
    async fn first_prompt_is_named_by_a_separate_pi(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        let stdin = dir.path().join("naming-stdin");
        let script = dir.path().join("pi");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \" $* \" in *\" --print \"*)\n  \
                 echo \"$*\" > '{args}'; cat > '{stdin}'; echo 'Title: \"CI build failures.\"'; \
                 exit 0;;\nesac\nexec '{fake}' \"$@\"\n",
                args = dir.path().join("naming-args").display(),
                stdin = stdin.display(),
                fake = fake.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests(script.clone()));
            cx.set_global(crate::settings::AppSettings {
                auto_names: true,
                ..crate::settings::AppSettings::default()
            });
        });
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        // Typed while pi starts: naming waits for the model, the prompt for pi.
        let message = "Please look at why the build fails on CI since this morning";
        session.update(cx, |session, cx| {
            session.submit(message.into(), Vec::new(), false, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while cx.read(|cx| session.read(cx).title().is_none()) {
            assert!(std::time::Instant::now() < deadline, "no name arrived");
            let _ = events.recv().await;
        }
        cx.read(|cx| {
            let session = session.read(cx);
            assert_eq!(session.title().as_deref(), Some("CI build failures"));
            let prompts: Vec<&str> = session
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::User(user) => Some(user.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(prompts.len() <= 1 && prompts.iter().all(|p| *p == message));
        });
        let args = std::fs::read_to_string(dir.path().join("naming-args")).unwrap();
        assert!(
            args.contains("--no-session") && args.contains("fake-1"),
            "{args}"
        );
        assert!(std::fs::read_to_string(&stdin).unwrap().contains(message));
    }

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
