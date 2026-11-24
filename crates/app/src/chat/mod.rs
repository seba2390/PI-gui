//! The chat column: transcript above, composer below.

mod commands;
pub mod composer;
mod context_panel;
pub mod dialogs;
mod file_links;
mod file_picker;
mod find;
mod images;
mod math;
pub(crate) mod mention;
pub mod models_dialog;
mod queued;
mod rows;
pub mod session;
#[cfg(all(test, unix))]
mod session_tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::{BlockKind, ItemId, ItemKind};

use composer::Composer;
pub use composer::ComposerEvent;
use session::{PiSession, SessionEvent};

use crate::theme::palette;
use crate::ui::{MessageScroller, MessageScrollerState};

actions!(chat, [ToggleThinking, ToggleToolOutput]);

const CHAT_CONTEXT: &str = "Chat";
/// Answers up to this size are searched for file names right away; more (a long session
/// opening) in the background.
const PARSE_INLINE_BYTES: usize = 64 * 1024;
/// Readable line length, like other agent UIs.
pub const CONTENT_MAX_WIDTH: Pixels = px(780.);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-t", ToggleThinking, Some(CHAT_CONTEXT)),
        KeyBinding::new("ctrl-o", ToggleToolOutput, Some(CHAT_CONTEXT)),
    ]);
    composer::init(cx);
    find::init(cx);
}

/// Identifies one block of one transcript item, for expansion and markdown state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub item: ItemId,
    pub block: usize,
}

/// Retained markdown state for one streamed text block.
struct Markdown {
    state: Entity<TextViewState>,
    source_len: usize,
    /// What may name a file in this text (parsed once it is finished).
    file_candidates: Option<Vec<file_links::Candidate>>,
    /// The file links drawn now (their URLs), so a block is only re-parsed for display when
    /// they change.
    file_links: Vec<String>,
    /// Remeasures the block's row when a parse lands (see [`ChatView::new_markdown`]).
    _parsed: Subscription,
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// The session's file, title or state changed; the sidebar may need a refresh.
    SessionChanged,
    /// Folder / device choices from the composer, handled by the window.
    Composer(ComposerEvent),
    /// pi ended (crashed, or the SSH connection dropped): start it again (connecting first
    /// for a remote session) and resume the session.
    Reconnect,
    /// The session started or stopped having work in flight ([`PiSession::is_busy`]).
    BusyChanged,
    /// An extension asked something while this chat was in the background; the question
    /// waits until the chat is shown ([`ChatView::needs_attention`]).
    NeedsAttention,
    /// pi stopped (crashed or disconnected) while this chat was in the background.
    Stopped,
}

#[allow(clippy::struct_excessive_bools)] // independent view flags
pub struct ChatView {
    session: Entity<PiSession>,
    composer: Entity<Composer>,
    scroller: Entity<MessageScrollerState>,
    /// Item version last synced into the scroller, per row.
    row_versions: Vec<u64>,
    /// Whether the trailing "Working…" row is present.
    working_row: bool,
    markdown: HashMap<BlockKey, Markdown>,
    /// Blocks the user toggled away from the default (collapsed) state.
    toggled: HashSet<BlockKey>,
    expand_thinking: bool,
    expand_tools: bool,
    focus_handle: FocusHandle,
    ticker: Option<Task<()>>,
    /// The host was re-checked after this remote session ended.
    checked_host: bool,
    /// Shown in the window (false while it works in the background).
    visible: bool,
    /// How each transcript item is shown: on its own, as an "Explored" group, or folded
    /// into one.
    roles: Vec<transcript::explore::RowRole>,
    /// Each item's exploration calls, by item version (classifying bash commands is not
    /// repeated for unchanged items).
    exploration: Vec<(ItemId, u64, Option<Vec<usize>>)>,
    /// Extension questions asked while in the background, opened when the chat is shown so
    /// they are never answered for the wrong chat.
    /// Questions asked while hidden, with when they arrived.
    held_dialogs: Vec<(pi_rpc::protocol::ExtensionUiRequest, std::time::Instant)>,
    /// pi stopped while the chat was hidden; shown so the user sees why (and any prompts
    /// returned to the composer) before the chat is let go.
    stopped_unseen: bool,
    /// The π field behind a new, empty conversation; it fades out when the first message is
    /// sent and is then dropped.
    pi_field: Option<Entity<crate::ui::PiField>>,
    /// Syntax colors of edit diffs and read files in tool rows, so re-rendering (scrolling,
    /// streaming) never parses the same code twice. Filled while rendering, hence the cell.
    highlights: std::cell::RefCell<crate::code_highlight::HighlightCache>,
    /// The find bar (⌘F), while open.
    find: Option<find::Find>,
    /// The transcript row under the pointer.
    hovered_row: Option<usize>,
    /// The "Worked for" item of the turn the pointer is in: its hover actions show while the
    /// pointer is anywhere in that turn's answer, not only over the "Worked for" row.
    hovered_turn: Option<ItemId>,
    /// The session folder's files, to link the files answers mention (see [`file_links`]).
    files: file_links::FileIndex,
    /// Answer blocks whose file links are not drawn yet (they still stream, or changed).
    unlinked: HashSet<BlockKey>,
    /// Parses a long session's answers for file names in the background.
    _parse_files: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// Opens a file of the session's folder in the Files tab, at a line (a file link in an
/// answer, or a tool row's file).
#[derive(Action, Clone, PartialEq)]
#[action(namespace = chat, no_json)]
pub struct OpenFileLink {
    /// Relative to the session's folder, or absolute.
    pub path: String,
    pub line: Option<u32>,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    pub fn new(
        cwd: PathBuf,
        session_file: Option<PathBuf>,
        host: Option<remote::HostId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Only a brand-new conversation gets the π field: a resumed one is briefly empty
        // while its history loads, and the field would flash.
        let new = session_file.is_none();
        let session = cx.new(|cx| PiSession::new(cwd, session_file, host, cx));
        let mut this = Self::with_session(session, window, cx);
        if new {
            this.pi_field = Some(cx.new(|_| crate::ui::PiField::new()));
        }
        this
    }

    /// A placeholder with no pi process behind the welcome screen (see
    /// [`PiSession::idle`]).
    pub fn idle(
        cwd: PathBuf,
        mode: pi_sessions::Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|_| PiSession::idle(cwd, mode));
        Self::with_session(session, window, cx)
    }

    fn with_session(
        session: Entity<PiSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| Composer::new(session.clone(), window, cx));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        let subscriptions = vec![
            // The speech model download banner.
            cx.observe(&crate::stt::models::models(cx), |_, _, cx| cx.notify()),
            // The reconnect banner shows the host's live status.
            cx.observe(&crate::hosts::hosts(cx), |this, _, cx| {
                if this.session.read(cx).host().is_some() {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&session, window, Self::on_session_event),
            cx.subscribe(&composer, |_, _, event: &ComposerEvent, cx| {
                cx.emit(ChatEvent::Composer(event.clone()));
            }),
            cx.observe(&session, |this, _, cx| {
                this.fade_pi_field_once_started(cx);
                cx.notify();
            }),
            // The folder's file list (the attach picker's index): link the files answers name.
            cx.observe(&composer.read(cx).file_picker(), |this, picker, cx| {
                let files = picker.read(cx).delegate().files();
                if !this.files.is_for(&files) {
                    this.files = file_links::FileIndex::new(files);
                    this.link_files(true, cx);
                }
            }),
        ];

        let mut this = Self {
            session,
            composer,
            scroller,
            row_versions: Vec::new(),
            working_row: false,
            markdown: HashMap::new(),
            toggled: HashSet::new(),
            expand_thinking: false,
            expand_tools: false,
            focus_handle: cx.focus_handle(),
            ticker: None,
            checked_host: false,
            visible: true,
            roles: Vec::new(),
            exploration: Vec::new(),
            held_dialogs: Vec::new(),
            stopped_unseen: false,
            pi_field: None,
            highlights: std::cell::RefCell::default(),
            find: None,
            hovered_row: None,
            hovered_turn: None,
            files: file_links::FileIndex::default(),
            unlinked: HashSet::new(),
            _parse_files: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    pub fn session(&self) -> &Entity<PiSession> {
        &self.session
    }

    /// Starts fading the π field as soon as the conversation starts: a message was sent (it
    /// may still be on its way to pi) or the transcript has something in it.
    fn fade_pi_field_once_started(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.pi_field else { return };
        let session = self.session.read(cx);
        if session.is_busy() || !session.is_empty() {
            field.update(cx, |field, cx| field.fade_out(cx));
        }
    }

    /// Whether an extension question is waiting for this chat to be shown.
    pub fn needs_attention(&self) -> bool {
        !self.held_dialogs.is_empty() || self.stopped_unseen
    }

    /// Marks the chat shown or in the background; showing it opens the questions it held.
    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible {
            self.stopped_unseen = false;
            // pi gives up on a question after its timeout, and a stopped pi takes no answers.
            let stopped = matches!(
                self.session.read(cx).lifecycle(),
                session::Lifecycle::Exited { .. }
            );
            for (request, asked) in std::mem::take(&mut self.held_dialogs) {
                let expired = request
                    .timeout_ms()
                    .is_some_and(|ms| asked.elapsed().as_millis() >= u128::from(ms));
                if !stopped && !expired {
                    dialogs::open(request, self.session.clone(), window, cx);
                }
            }
        }
    }

    /// Rebuilds every row: the transcript was replaced (a fork or clone), not appended to.
    fn reset_rows(&mut self, cx: &mut Context<Self>) {
        self.markdown.clear();
        self.unlinked.clear();
        self.roles.clear();
        self.exploration.clear();
        self.toggled.clear();
        self.row_versions.clear();
        self.working_row = false;
        // Row numbers now point at other items; the row under the pointer reports itself again.
        self.hovered_row = None;
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(0, cx));
        self.sync(cx);
    }

    /// Opens the picture viewer on the conversation's pictures, at `start`
    /// (`PISTACHIO_OPEN=image-viewer:<n>`, for layout checks).
    /// Returns whether there were pictures to show.
    pub fn open_images(
        chat: &Entity<Self>,
        start: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let session = chat.read(cx).session.clone();
        let all = images::all_images(session.read(cx).transcript().items());
        if all.is_empty() {
            return false;
        }
        let start = start.min(all.len() - 1);
        images::open_viewer(all.into(), start, window, cx);
        true
    }

    /// The composer, e.g. to insert an `@path` mention from the Files tab.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The handle that focuses the composer input.
    pub fn composer_focus(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }

    fn on_session_event(
        &mut self,
        _: &Entity<PiSession>,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::TranscriptChanged => self.sync(cx),
            SessionEvent::MetaChanged => {
                let stopped = matches!(
                    self.session.read(cx).lifecycle(),
                    session::Lifecycle::Exited { .. }
                );
                if stopped && !self.visible && !self.stopped_unseen {
                    self.stopped_unseen = true;
                    cx.emit(ChatEvent::Stopped);
                }
                let session = self.session.read(cx);
                if let (Some(host), session::Lifecycle::Exited { .. }) =
                    (session.host().cloned(), session.lifecycle())
                    && !self.checked_host
                {
                    self.checked_host = true;
                    crate::hosts::hosts(cx)
                        .update(cx, |hosts, cx| hosts.check_connection(&host, cx));
                }
                cx.notify();
            }
            SessionEvent::SessionFileChanged | SessionEvent::Settled => {
                self.sync(cx);
                cx.emit(ChatEvent::SessionChanged);
            }
            SessionEvent::Dialog(request) => {
                if self.visible {
                    dialogs::open(request.clone(), self.session.clone(), window, cx);
                } else {
                    self.held_dialogs
                        .push((request.clone(), std::time::Instant::now()));
                    cx.emit(ChatEvent::NeedsAttention);
                }
            }
            SessionEvent::BusyChanged => cx.emit(ChatEvent::BusyChanged),
            SessionEvent::Reloaded => self.reset_rows(cx),
            SessionEvent::SetEditorText(text) => {
                let text = text.clone();
                self.composer
                    .update(cx, |composer, cx| composer.set_text(&text, window, cx));
            }
            SessionEvent::RestoreToEditor {
                text,
                images,
                replace,
            } => {
                let (text, images, replace) = (text.clone(), images.clone(), *replace);
                self.composer.update(cx, |composer, cx| {
                    composer.restore(&text, images, replace, window, cx);
                });
            }
            SessionEvent::Notify { kind, message } => {
                use gpui_kit::component::notification::Notification;
                // From a chat in the background: say which one.
                let message = if self.visible {
                    message.clone()
                } else {
                    let title = self
                        .session
                        .read(cx)
                        .display_title()
                        .unwrap_or_else(|| "A background chat".to_owned());
                    format!("{title}: {message}")
                };
                let notification = match kind {
                    pi_rpc::protocol::NotifyKind::Info => Notification::info(message),
                    pi_rpc::protocol::NotifyKind::Warning => Notification::warning(message),
                    pi_rpc::protocol::NotifyKind::Error => Notification::error(message),
                };
                window.push_notification(notification, cx);
            }
        }
    }

    /// Brings markdown states and the virtual list in line with the transcript.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let transcript = session.transcript();
        let working = session.shows_working();
        let ticking = session.can_stop();
        let items = transcript.items();

        // 1. Collect changed rows and the markdown sources inside them.
        let mut changed_rows = Vec::new();
        // Only the new suffix of a streamed block is copied (a full copy per delta would make
        // long answers quadratic); `None` source_len means a full (re)load.
        let mut sources: Vec<(BlockKey, Option<usize>, String)> = Vec::new();
        // User messages: shown as typed, but through a text view so they can be selected.
        let mut user_texts: Vec<(BlockKey, String)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if self.row_versions.get(index) == Some(&item.version) {
                continue;
            }
            changed_rows.push(index);
            if let ItemKind::User(user) = &item.kind {
                let key = BlockKey {
                    item: item.id,
                    block: 0,
                };
                if self.markdown.get(&key).map(|markdown| markdown.source_len)
                    != Some(user.text.len())
                {
                    user_texts.push((key, user.text.clone()));
                }
            }
            if let ItemKind::Assistant(assistant) = &item.kind {
                for (block_index, block) in assistant.blocks.iter().enumerate() {
                    if let BlockKind::Text(text) = &block.kind {
                        let key = BlockKey {
                            item: item.id,
                            block: block_index,
                        };
                        let text = text.text.as_str();
                        match self.markdown.get(&key).map(|markdown| markdown.source_len) {
                            Some(len) if len == text.len() => {}
                            Some(len) if len < text.len() && text.is_char_boundary(len) => {
                                sources.push((key, Some(len), text[len..].to_owned()));
                            }
                            _ => sources.push((key, None, text.to_owned())),
                        }
                    }
                }
            }
        }
        let new_versions: Vec<u64> = items.iter().map(|item| item.version).collect();

        // 1b. Fold runs of exploration calls into groups. A row whose role changed, or a group
        //    that one of its folded items changed, is measured again.
        self.exploration.truncate(items.len());
        for (index, item) in items.iter().enumerate() {
            let fresh = || {
                (
                    item.id,
                    item.version,
                    transcript::explore::exploration_tools(item),
                )
            };
            match self.exploration.get_mut(index) {
                Some((id, version, _)) if *id == item.id && *version == item.version => {}
                Some(entry) => *entry = fresh(),
                None => self.exploration.push(fresh()),
            }
        }
        let members: Vec<Option<Vec<usize>>> = self
            .exploration
            .iter()
            .map(|(_, _, tools)| tools.clone())
            .collect();
        let roles = transcript::explore::group_rows(&members);
        for (index, role) in roles.iter().enumerate() {
            let regrouped = self.roles.get(index) != Some(role);
            let member_changed = match role {
                transcript::explore::RowRole::Group(group) => changed_rows
                    .iter()
                    .any(|row| (index + 1..=group.last).contains(row)),
                _ => false,
            };
            if (regrouped || member_changed) && !changed_rows.contains(&index) {
                changed_rows.push(index);
            }
        }
        self.roles = roles;

        // 2. Update retained markdown, streaming appends incrementally.
        for (key, appended_at, text) in sources {
            self.unlinked.insert(key);
            match (self.markdown.get_mut(&key), appended_at) {
                (Some(markdown), Some(len)) => {
                    markdown
                        .state
                        .update(cx, |state, cx| state.push_str(&text, cx));
                    markdown.source_len = len + text.len();
                    markdown.file_candidates = None;
                }
                (Some(markdown), None) => {
                    markdown
                        .state
                        .update(cx, |state, cx| state.set_text(&text, cx));
                    markdown.source_len = text.len();
                    markdown.file_candidates = None;
                    markdown.file_links.clear();
                }
                (None, _) => {
                    let markdown = Self::new_markdown(key, &text, text.len(), cx);
                    self.markdown.insert(key, markdown);
                }
            }
        }

        for (key, text) in user_texts {
            let source = literal_markdown(&text);
            match self.markdown.get_mut(&key) {
                Some(markdown) => {
                    markdown
                        .state
                        .update(cx, |state, cx| state.set_text(&source, cx));
                    markdown.source_len = text.len();
                }
                None => {
                    let markdown = Self::new_markdown(key, &source, text.len(), cx);
                    self.markdown.insert(key, markdown);
                }
            }
        }

        // 2b. Link the files finished answers mention. When a turn ends, list the folder again
        //     first: it may have made the files it talks about.
        if self.working_row && !working {
            self.composer
                .update(cx, |composer, cx| composer.refresh_file_index(cx));
        }
        self.link_files(false, cx);

        // 3. Splice the virtual list: new items go before the trailing working row.
        let old_items = self.row_versions.len();
        let old_rows = old_items + usize::from(self.working_row);
        let new_items = new_versions.len();
        let new_rows = new_items + usize::from(working);
        self.scroller.update(cx, |scroller, cx| {
            if new_items < old_items {
                // A reset (history reload): rebuild the list.
                scroller.reset(new_rows, cx);
                return;
            }
            let inserted = new_rows - old_items;
            scroller.splice(old_items..old_rows, inserted, cx);
            for index in changed_rows
                .iter()
                .copied()
                .filter(|index| *index < old_items)
            {
                scroller.remeasure_items(index..index + 1, cx);
            }
        });
        if new_items < old_items {
            self.hovered_row = None;
        }
        self.row_versions = new_versions;
        self.working_row = working;
        // A turn under the pointer may just have ended (or the transcript was replaced).
        self.update_hovered_turn(cx);

        // 4. While working, repaint at the working π's frame rate so it pulses smoothly and
        //    the timers advance; nothing ticks while idle.
        if ticking && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(crate::ui::spinner::FRAME)
                        .await;
                    let keep = this
                        .update(cx, |this, cx| {
                            let ticking = this.session.read(cx).can_stop();
                            if ticking {
                                cx.notify();
                            } else {
                                this.ticker = None;
                            }
                            ticking
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
        // Matches move with the transcript (new text, a reload).
        self.refresh_find(cx);
        cx.notify();
    }

    /// A selectable markdown view of `source` for block `key`; `source_len` is the length of
    /// the transcript text it shows (kept to stream appends in).
    fn new_markdown(
        key: BlockKey,
        source: &str,
        source_len: usize,
        cx: &mut Context<Self>,
    ) -> Markdown {
        let state = cx.new(|cx| TextViewState::markdown(source, cx).selectable(true));
        // The scroller measures every row once, those above the viewport included, so its
        // scrollbar is right from the first frame. gpui-kit parses a text over 4 KB in the
        // background, so such a row is first measured before its parse lands and would
        // keep that height until scrolled into view. Remeasure it when a parse lands: the
        // source of the parsed text changes then, and only then (selection and find
        // highlights notify too).
        let mut parsed = state.read(cx).rendered_text().source().len();
        let observer = cx.observe(&state, move |this, state, cx| {
            let len = state.read(cx).rendered_text().source().len();
            if len != parsed {
                parsed = len;
                this.remeasure_item(key.item, cx);
            }
        });
        Markdown {
            state,
            source_len,
            file_candidates: None,
            file_links: Vec::new(),
            _parsed: observer,
        }
    }

    /// Draws links on the files that finished answer blocks name: those in
    /// [`Self::unlinked`], or every block with `all` (the file list changed). Only blocks
    /// whose links changed are drawn again.
    fn link_files(&mut self, all: bool, cx: &mut Context<Self>) {
        if !all && self.unlinked.is_empty() {
            return;
        }
        let session = self.session.read(cx);
        if !session.has_folder_tools() {
            self.unlinked.clear();
            return;
        }
        let running = session.is_running();
        let root = session.cwd().to_path_buf();
        let items = session.transcript().items();
        // Parsing runs at roughly 10 MB/s: a long session's answers are parsed in the
        // background, then linked.
        let unparsed: Vec<(BlockKey, String)> = items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Assistant(assistant) => Some((item.id, assistant)),
                _ => None,
            })
            .flat_map(|(id, assistant)| {
                assistant
                    .blocks
                    .iter()
                    .enumerate()
                    .filter_map(move |(block, kind)| match &kind.kind {
                        BlockKind::Text(text) => Some((BlockKey { item: id, block }, text)),
                        _ => None,
                    })
            })
            .filter(|(key, text)| {
                (all || self.unlinked.contains(key))
                    && self.markdown.get(key).is_some_and(|markdown| {
                        markdown.file_candidates.is_none() && markdown.source_len == text.text.len()
                    })
            })
            .map(|(key, text)| (key, text.text.clone()))
            .collect();
        if unparsed.iter().map(|(_, text)| text.len()).sum::<usize>() > PARSE_INLINE_BYTES {
            self._parse_files = cx.spawn(async move |this, cx| {
                let parsed = cx
                    .background_executor()
                    .spawn(async move {
                        unparsed
                            .into_iter()
                            .map(|(key, text)| (key, text.len(), file_links::candidates(&text)))
                            .collect::<Vec<_>>()
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    for (key, len, candidates) in parsed {
                        if let Some(markdown) = this.markdown.get_mut(&key)
                            && markdown.source_len == len
                        {
                            markdown.file_candidates = Some(candidates);
                        }
                    }
                    this.link_files(all, cx);
                });
            });
            return;
        }
        let mut redraw = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let ItemKind::Assistant(assistant) = &item.kind else {
                continue;
            };
            for (block_index, block) in assistant.blocks.iter().enumerate() {
                let BlockKind::Text(text) = &block.kind else {
                    continue;
                };
                let key = BlockKey {
                    item: item.id,
                    block: block_index,
                };
                if !all && !self.unlinked.contains(&key) {
                    continue;
                }
                // The text still streaming in.
                if running && index + 1 == items.len() && block_index + 1 == assistant.blocks.len()
                {
                    continue;
                }
                let Some(markdown) = self.markdown.get_mut(&key) else {
                    continue;
                };
                if markdown.source_len != text.text.len() {
                    continue;
                }
                self.unlinked.remove(&key);
                let candidates = markdown
                    .file_candidates
                    .get_or_insert_with(|| file_links::candidates(&text.text));
                let links: Vec<(&file_links::Candidate, &str)> = candidates
                    .iter()
                    .filter_map(|candidate| {
                        Some((candidate, self.files.resolve(&candidate.path, &root)?))
                    })
                    .collect();
                let urls: Vec<String> = links
                    .iter()
                    .map(|(candidate, file)| file_links::url(file, candidate.line))
                    .collect();
                if urls != markdown.file_links {
                    redraw.push((markdown.state.clone(), file_links::link(&text.text, &links)));
                    markdown.file_links = urls;
                }
            }
        }
        for (state, text) in redraw {
            state.update(cx, |state, cx| state.set_text(&text, cx));
        }
    }

    /// Measures the row of transcript item `item` again (its height changed).
    fn remeasure_item(&mut self, item: ItemId, cx: &mut Context<Self>) {
        let row = self
            .session
            .read(cx)
            .transcript()
            .items()
            .iter()
            .position(|candidate| candidate.id == item);
        if let Some(row) = row {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(row..row + 1, cx);
            });
        }
    }

    /// The "Worked for" item of the turn under the pointer (see [`Self::set_row_hovered`]).
    pub(crate) fn hovered_turn(&self) -> Option<ItemId> {
        self.hovered_turn
    }

    /// The pointer entered (or left) transcript row `row`. Rows report enter and leave in
    /// either order when the pointer crosses between them, so a leave only counts for the
    /// row last entered.
    pub(crate) fn set_row_hovered(&mut self, row: usize, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hovered_row = Some(row);
        } else if self.hovered_row == Some(row) {
            self.hovered_row = None;
        } else {
            return;
        }
        self.update_hovered_turn(cx);
    }

    /// Recomputes the turn under the pointer, re-rendering only when it changed (not as the
    /// pointer crosses rows of the same turn).
    fn update_hovered_turn(&mut self, cx: &mut Context<Self>) {
        let items = self.session.read(cx).transcript().items();
        let turn = self
            .hovered_row
            .and_then(|row| rows::turn_end(items, row))
            .map(|end| items[end].id);
        if turn != self.hovered_turn {
            self.hovered_turn = turn;
            cx.notify();
        }
    }

    pub(crate) fn is_expanded(&self, key: BlockKey, is_thinking: bool) -> bool {
        let default = if is_thinking {
            self.expand_thinking
        } else {
            self.expand_tools
        };
        default != self.toggled.contains(&key)
    }

    pub(crate) fn toggle_block(&mut self, key: BlockKey, row: usize, cx: &mut Context<Self>) {
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(row..row + 1, cx);
        });
        cx.notify();
    }

    fn toggle_all_thinking(&mut self, _: &ToggleThinking, _: &mut Window, cx: &mut Context<Self>) {
        self.expand_thinking = !self.expand_thinking;
        self.toggled.clear();
        self.scroller
            .update(cx, |scroller, cx| scroller.remeasure(cx));
    }

    fn toggle_all_tools(&mut self, _: &ToggleToolOutput, _: &mut Window, cx: &mut Context<Self>) {
        self.set_expand_tools(!self.expand_tools, cx);
    }

    /// Expands (or collapses) every tool call's output, like Ctrl+O.
    pub fn set_expand_tools(&mut self, expand: bool, cx: &mut Context<Self>) {
        self.expand_tools = expand;
        self.toggled.clear();
        self.scroller
            .update(cx, |scroller, cx| scroller.remeasure(cx));
    }

    pub(crate) fn role(&self, row: usize) -> Option<&transcript::explore::RowRole> {
        self.roles.get(row)
    }

    pub(crate) fn markdown(&self, key: BlockKey) -> Option<&Entity<TextViewState>> {
        self.markdown.get(&key).map(|markdown| &markdown.state)
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let home = cx.global::<crate::config::AppConfig>().home.clone();
        let chat = session.mode() == pi_sessions::Mode::Chat;
        let (headline, subtitle) = if chat {
            (
                "How can I help?".to_owned(),
                "Chat · no access to your files".to_owned(),
            )
        } else {
            (
                "What should we build?".to_owned(),
                crate::ui::short_path(session.cwd(), home.as_deref()),
            )
        };
        let accent = palette(cx).accent;
        let message = match session.lifecycle() {
            session::Lifecycle::Exited { message } => Some(message.clone()),
            _ => None,
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                component::Icon::from(crate::ui::AppIcon::PiLogo)
                    .size_10()
                    .text_color(accent),
            )
            .child(
                div()
                    .text_xl()
                    .text_color(cx.theme().foreground)
                    .child(headline),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle),
            )
            .when_some(message, |this, message| {
                this.child(
                    div()
                        .max_w(px(560.))
                        .mt_2()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(palette(cx).error)
                        .text_sm()
                        .text_color(palette(cx).error)
                        .child(message),
                )
            })
    }
}

impl ChatView {
    /// For a session on an SSH host that ended: what happened and a Reconnect button.
    fn render_reconnect(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session.read(cx);
        if !matches!(session.lifecycle(), session::Lifecycle::Exited { .. }) {
            return None;
        }
        let Some(host) = session.host() else {
            return Some(self.render_restart(session.session_file().is_some(), cx));
        };
        let hosts = crate::hosts::hosts(cx);
        let hosts = hosts.read(cx);
        let label = hosts.label(host);
        let status = hosts.status(host);
        let resumable = session.session_file().is_some();
        let text = if status.is_connected() {
            format!("pi stopped on {label}.")
        } else if hosts.is_turned_off(host) {
            format!("The connection to {label} is turned off.")
        } else {
            format!("The connection to {label} was lost.")
        };
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        Some(
            h_flex()
                .w_full()
                .gap_2()
                .mb_2()
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .child(crate::remote_ui::status_dot(Some(&status), cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(muted)
                        .child(if resumable {
                            format!("{text} Reconnect to continue this session.")
                        } else {
                            text
                        }),
                )
                .child(
                    component::button::Button::new("reconnect")
                        .small()
                        .label(if status.is_busy() {
                            "Connecting…"
                        } else {
                            "Reconnect"
                        })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::Reconnect))),
                )
                .into_any_element(),
        )
    }
}

impl ChatView {
    /// A local pi that stopped (crashed or failed to start): offer to start it again.
    fn render_restart(&self, resumable: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        h_flex()
            .w_full()
            .gap_2()
            .mb_2()
            .px_3()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(muted)
                    .child(if resumable {
                        "pi stopped. Restart it to continue this session."
                    } else {
                        "pi stopped."
                    }),
            )
            .child(
                component::button::Button::new("restart-pi")
                    .small()
                    .label("Restart pi")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::Reconnect))),
            )
            .into_any_element()
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("chat");
        let is_empty = self.session.read(cx).is_empty() && !self.session.read(cx).shows_working();
        let weak = cx.weak_entity();
        let background = cx.theme().background;
        // Once faded out, the field is let go (it costs nothing either way while still).
        if self
            .pi_field
            .as_ref()
            .is_some_and(|field| field.read(cx).is_faded())
        {
            self.pi_field = None;
        }
        let pi_field = self.pi_field.clone();

        let find_bar = self.render_find(cx);
        let root = self.find_actions(v_flex(), cx);
        root.relative()
            .key_context(CHAT_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_all_thinking))
            .on_action(cx.listener(Self::toggle_all_tools))
            .size_full()
            .min_h_0()
            .bg(background)
            .children(crate::stt::ui::download_banner(cx))
            // Behind everything else (painted first), across the whole chat.
            .when_some(pi_field, |this, field| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .child(field.cached(StyleRefinement::default().size_full())),
                )
            })
            .child(if is_empty {
                self.render_empty(cx).into_any_element()
            } else {
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        MessageScroller::new(
                            "transcript",
                            self.scroller.clone(),
                            move |index, window, cx| {
                                let _timer = crate::perf::render("chat_row");
                                rows::render_row(&weak, index, window, cx)
                            },
                        )
                        // Rows own their spacing (see rows.rs); drop the default 32px gap.
                        .with_row_style(StyleRefinement::default().pb(px(0.)))
                        .with_bottom_fade(background)
                        .size_full(),
                    )
                    .children(find_bar)
                    .into_any_element()
            })
            .child(
                h_flex().w_full().justify_center().px_4().pb_4().child(
                    div()
                        .w_full()
                        .max_w(CONTENT_MAX_WIDTH + px(32.))
                        .children(self.render_reconnect(cx))
                        .child(self.composer.clone()),
                ),
            )
    }
}

/// Markdown that renders `text` exactly as typed: punctuation is escaped (no emphasis, links,
/// headings or math), line breaks are kept, and leading indentation stays text instead of
/// becoming a code block. Copying the rendered text gives the original back.
fn literal_markdown(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let body = line.trim_start();
        for c in line[..line.len() - body.len()].chars() {
            out.push_str(if c == '\t' {
                "&#32;&#32;&#32;&#32;"
            } else {
                "&#32;"
            });
        }
        for c in body.trim_end().chars() {
            if c.is_ascii_punctuation() {
                out.push('\\');
            }
            out.push(c);
        }
        // A trailing backslash is a hard line break, unless the paragraph ends here.
        let next_is_text = lines
            .get(index + 1)
            .is_some_and(|next| !next.trim().is_empty());
        out.push_str(if next_is_text { "\\\n" } else { "\n" });
    }
    out
}

/// Renders a markdown view for `state` (helper so rows stay declarative). Fenced code blocks
/// get their language label and a copy button in the top-right corner.
pub(crate) fn markdown_view(state: &Entity<TextViewState>) -> TextView {
    TextView::new(state)
        .selectable(true)
        .markdown_extensions(file_links::markdown_extensions())
        .on_link_click(|url, _, window, cx| match file_links::parse_url(url) {
            Some((path, line)) => window.dispatch_action(Box::new(OpenFileLink { path, line }), cx),
            None => cx.open_url(url),
        })
        .code_block_actions(|block, _, cx| {
            let muted = cx.theme().muted_foreground;
            h_flex()
                .gap_1()
                .items_center()
                .when_some(
                    block.lang().filter(|lang| !lang.is_empty()),
                    |this, lang| this.child(div().pl_1().text_xs().text_color(muted).child(lang)),
                )
                .child(crate::ui::CopyButton::new("copy", block.code()).tooltip("Copy code"))
        })
}

#[cfg(test)]
mod tests {
    use super::literal_markdown;
    use pretty_assertions::assert_eq;

    #[test]
    fn user_text_renders_literally() {
        assert_eq!(
            literal_markdown("tell me something funny"),
            "tell me something funny\n"
        );
        assert_eq!(
            literal_markdown("# not a *heading* [x](y) $5\n    indented\n\nnext"),
            "\\# not a \\*heading\\* \\[x\\]\\(y\\) \\$5\\\n&#32;&#32;&#32;&#32;indented\n\nnext\n"
        );
    }
    /// The text a chat view shows for `source`, with the chat's own Markdown extensions.
    fn shown(source: &str, cx: &mut gpui_kit::TestAppContext) -> String {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::text::TextViewState;

        struct Host(gpui_kit::Entity<TextViewState>);
        impl gpui_kit::Render for Host {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                super::markdown_view(&self.0)
            }
        }

        cx.update(gpui_kit::component::init);
        let state = cx.update(|cx| cx.new(|cx| TextViewState::markdown(source, cx)));
        let host = state.clone();
        let window = cx.add_window(move |_, _| Host(host));
        // The view applies the extensions when it draws, then parses again.
        for _ in 0..2 {
            window.update(cx, |_, window, _| window.refresh()).unwrap();
            cx.run_until_parked();
        }
        state.read_with(cx, |state, _| {
            state.rendered_text().as_str().trim().to_owned()
        })
    }

    /// Formulas are drawn (their text is their source, for copying); prices around them stay
    /// prose with their emphasis. Without the math plugin, `*y*` would be emphasis.
    #[gpui_kit::test]
    fn formulas_and_prices_together(cx: &mut gpui_kit::TestAppContext) {
        assert_eq!(
            shown(
                "Since $x*y*z$ holds, **$4.0 trillion** and \\(x^2\\) cost $5 and $10.",
                cx
            ),
            "Since $x*y*z$ holds, $4.0 trillion and \\(x^2\\) cost $5 and $10."
        );
        // The same text without the plugin (a plain view): no formulas.
        let plain = {
            use gpui_kit::AppContext as _;
            let state = cx.update(|cx| {
                cx.new(|cx| {
                    gpui_kit::component::text::TextViewState::markdown("Since $x*y*z$ holds", cx)
                })
            });
            cx.run_until_parked();
            state.read_with(cx, |state, _| {
                state.rendered_text().as_str().trim().to_owned()
            })
        };
        assert_eq!(plain, "Since $xyz$ holds");
        // A formula KaTeX can't read shows as written.
        assert_eq!(shown("bad $\\frac{a}{$ here", cx), "bad $\\frac{a}{$ here");
        // Code keeps its dollars.
        assert_eq!(shown("run `echo $HOME $1`", cx), "run echo $HOME $1");
    }

    /// Dollar amounts are prose, not math, also in a plain view without the math plugin:
    /// reading "$4.0 trillion** — about **$" as one formula broke the bold around it.
    #[gpui_kit::test]
    fn dollar_amounts_keep_their_emphasis(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::component::text::TextViewState;

        cx.update(gpui_kit::component::init);
        let source = "Roughly **$4.0 trillion** — about **$4.02T** per StockAnalysis, while \
                      Capital.com shows **$3.99T**.";
        let state = cx.update(|cx| cx.new(|cx| TextViewState::markdown(source, cx)));
        cx.run_until_parked();
        let text = state.read_with(cx, |state, _| state.rendered_text().as_str().to_owned());
        assert_eq!(
            text.trim(),
            "Roughly $4.0 trillion — about $4.02T per StockAnalysis, while Capital.com shows \
             $3.99T."
        );
    }
}
