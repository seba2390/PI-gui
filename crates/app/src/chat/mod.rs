//! The chat column: transcript above, composer below.

mod commands;
pub mod composer;
mod dialogs;
mod rows;
pub mod session;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::{BlockKind, ItemId, ItemKind};

use composer::Composer;
pub use composer::ComposerEvent;
use session::{PiSession, SessionEvent};

use crate::theme::palette;

actions!(chat, [ToggleThinking, ToggleToolOutput]);

const CHAT_CONTEXT: &str = "Chat";
/// Readable line length, like other agent UIs.
pub const CONTENT_MAX_WIDTH: Pixels = px(780.);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-t", ToggleThinking, Some(CHAT_CONTEXT)),
        KeyBinding::new("ctrl-o", ToggleToolOutput, Some(CHAT_CONTEXT)),
    ]);
    composer::init(cx);
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
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// The session's file, title or state changed; the sidebar may need a refresh.
    SessionChanged,
    /// The user picked another working folder for a new chat.
    ChangeFolder(PathBuf),
}

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
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    pub fn new(
        cwd: PathBuf,
        session_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|cx| PiSession::new(cwd, session_file, cx));
        let composer = cx.new(|cx| Composer::new(session.clone(), window, cx));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        let subscriptions = vec![
            cx.subscribe_in(&session, window, Self::on_session_event),
            cx.subscribe(&composer, |_, _, event: &ComposerEvent, cx| match event {
                ComposerEvent::ChangeFolder(folder) => {
                    cx.emit(ChatEvent::ChangeFolder(folder.clone()));
                }
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
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
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    pub fn session(&self) -> &Entity<PiSession> {
        &self.session
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
            SessionEvent::MetaChanged => cx.notify(),
            SessionEvent::SessionFileChanged | SessionEvent::Settled => {
                self.sync(cx);
                cx.emit(ChatEvent::SessionChanged);
            }
            SessionEvent::Dialog(request) => {
                dialogs::open(request.clone(), self.session.clone(), window, cx);
            }
            SessionEvent::SetEditorText(text) => {
                let text = text.clone();
                self.composer
                    .update(cx, |composer, cx| composer.set_text(&text, window, cx));
            }
            SessionEvent::Notify { kind, message } => {
                use gpui_kit::component::notification::Notification;
                let notification = match kind {
                    pi_rpc::protocol::NotifyKind::Info => Notification::info(message.clone()),
                    pi_rpc::protocol::NotifyKind::Warning => Notification::warning(message.clone()),
                    pi_rpc::protocol::NotifyKind::Error => Notification::error(message.clone()),
                };
                window.push_notification(notification, cx);
            }
        }
    }

    /// Brings markdown states and the virtual list in line with the transcript.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let transcript = session.transcript();
        let running = session.is_running();
        let items = transcript.items();

        // 1. Collect changed rows and the markdown sources inside them.
        let mut changed_rows = Vec::new();
        // Only the new suffix of a streamed block is copied (a full copy per delta would make
        // long answers quadratic); `None` source_len means a full (re)load.
        let mut sources: Vec<(BlockKey, Option<usize>, String)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if self.row_versions.get(index) == Some(&item.version) {
                continue;
            }
            changed_rows.push(index);
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

        // 2. Update retained markdown, streaming appends incrementally.
        for (key, appended_at, text) in sources {
            match (self.markdown.get_mut(&key), appended_at) {
                (Some(markdown), Some(len)) => {
                    markdown
                        .state
                        .update(cx, |state, cx| state.push_str(&text, cx));
                    markdown.source_len = len + text.len();
                }
                (Some(markdown), None) => {
                    markdown
                        .state
                        .update(cx, |state, cx| state.set_text(&text, cx));
                    markdown.source_len = text.len();
                }
                (None, _) => {
                    let state = cx.new(|cx| TextViewState::markdown(&text, cx).selectable(true));
                    self.markdown.insert(
                        key,
                        Markdown {
                            state,
                            source_len: text.len(),
                        },
                    );
                }
            }
        }

        // 3. Splice the virtual list: new items go before the trailing working row.
        let old_items = self.row_versions.len();
        let old_rows = old_items + usize::from(self.working_row);
        let new_items = new_versions.len();
        let new_rows = new_items + usize::from(running);
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
        self.row_versions = new_versions;
        self.working_row = running;

        // 4. Tick once a second while working so timers advance.
        if running && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let keep = this
                        .update(cx, |this, cx| {
                            let running = this.session.read(cx).is_running();
                            if running {
                                cx.notify();
                            } else {
                                this.ticker = None;
                            }
                            running
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            }));
        }
        cx.notify();
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
        self.expand_tools = !self.expand_tools;
        self.toggled.clear();
        self.scroller
            .update(cx, |scroller, cx| scroller.remeasure(cx));
    }

    pub(crate) fn markdown(&self, key: BlockKey) -> Option<&Entity<TextViewState>> {
        self.markdown.get(&key).map(|markdown| &markdown.state)
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let home = cx.global::<crate::config::AppConfig>().home.clone();
        let folder = crate::ui::short_path(session.cwd(), home.as_deref());
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
                    .child("What should we build?"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(folder),
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

impl Focusable for ChatView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.session.read(cx).is_empty() && !self.session.read(cx).is_running();
        let weak = cx.weak_entity();
        let background = cx.theme().background;

        v_flex()
            .key_context(CHAT_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_all_thinking))
            .on_action(cx.listener(Self::toggle_all_tools))
            .size_full()
            .min_h_0()
            .bg(background)
            .child(if is_empty {
                self.render_empty(cx).into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        MessageScroller::new(
                            "transcript",
                            self.scroller.clone(),
                            move |index, window, cx| rows::render_row(&weak, index, window, cx),
                        )
                        // Rows own their spacing (see rows.rs); drop the default 32px gap.
                        .with_row_style(StyleRefinement::default().pb(px(0.)))
                        .with_bottom_fade(background)
                        .size_full(),
                    )
                    .into_any_element()
            })
            .child(
                h_flex().w_full().justify_center().px_4().pb_4().child(
                    div()
                        .w_full()
                        .max_w(CONTENT_MAX_WIDTH + px(32.))
                        .child(self.composer.clone()),
                ),
            )
    }
}

/// Renders a markdown view for `state` (helper so rows stay declarative).
pub(crate) fn markdown_view(state: &Entity<TextViewState>) -> TextView {
    TextView::new(state).selectable(true)
}
