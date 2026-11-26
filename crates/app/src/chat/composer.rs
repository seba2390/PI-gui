//! The composer: multi-line input plus the bottom toolbar (attachments, working
//! folder, model, thinking level, context-window wheel, send/stop).
//!
//! Key behaviour mirrors pi's terminal: Enter submits (steers while the agent
//! works), Shift+Enter inserts a newline, Alt+Enter queues a follow-up, Esc
//! interrupts, Alt+Up restores queued messages, Shift+Tab cycles thinking.

use pi_sessions::Mode;
use remote::HostId;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use deepsearch::effort::Effort;
use documents::attached;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Escape, IndentInline, InputEvent, MoveDown, MoveUp, Textarea, TextareaState,
};
use gpui_kit::component::list::{List, ListState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::speech::{SpeechEvent, SpeechState, SpeechStatus, SpeechWaveform};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::ImageContent;
use pi_sessions::prompt;

use super::attachment_card::{self, Card, Preview};
use super::commands::{self, CommandOutcome, Suggestion, SuggestionList};
use super::convert;
use super::document::{AttachedDocument, Reading, Room};
use super::file_picker::{self, FilePicker, Pick};
use super::images;
use super::mention::{self, History};
use super::pdf_viewer;
use super::session::{Lifecycle, PiSession, WebMode};
use super::text_viewer;
use crate::config::{AppConfig, RecentFolders};
use crate::theme::palette;
use crate::ui::{AppIcon, FadingText, QuietButton as _, described_item, spinner};
use gpui_kit::component::WindowExt as _;

actions!(
    composer,
    [SendFollowUp, Interrupt, Dequeue, CycleThinking, Complete]
);

const COMPOSER_CONTEXT: &str = "Composer";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-enter", SendFollowUp, Some(COMPOSER_CONTEXT)),
        KeyBinding::new("escape", Interrupt, Some(COMPOSER_CONTEXT)),
        KeyBinding::new("alt-up", Dequeue, Some(COMPOSER_CONTEXT)),
        KeyBinding::new("shift-tab", CycleThinking, Some(COMPOSER_CONTEXT)),
        KeyBinding::new("tab", Complete, Some(COMPOSER_CONTEXT)),
    ]);
}

#[derive(Debug, Clone)]
pub enum ComposerEvent {
    ChangeFolder(PathBuf),
    /// Run Code sessions on this computer (`None`) or a saved SSH host.
    ChooseHost(Option<HostId>),
    /// Open the "Add SSH connection" dialog.
    AddHost,
    /// Pick another folder on this session's SSH host.
    BrowseRemote(HostId),
    /// Open "Manage SSH connections".
    ManageHosts,
    /// Open the Models dialog.
    Models,
}

/// A file attached to the next prompt.
struct Attachment {
    /// Its place among the attachments and those still being read.
    order: u64,
    path: PathBuf,
    name: SharedString,
    preview: Preview,
    /// Images are sent as image content; other files as `@path` references,
    /// which is how pi's terminal refers to files.
    image: Option<ImageContent>,
    /// In chat mode the model has no `read` tool, so the text of files and documents goes along
    /// with the prompt.
    inline: Option<InlineText>,
    /// What the app knows a file whose text goes along by ([`documents::attached`]).
    id: Option<String>,
}

/// An attachment's contents for chat mode, or why it could not be included.
enum InlineText {
    Document(AttachedDocument),
    /// The block of a message that came back, as it was sent.
    Sent(String),
    Skipped(String),
}

impl InlineText {
    fn skipped(reason: &str) -> Self {
        Self::Skipped(reason.to_owned())
    }
}

/// A local file being read to attach, which the composer may be emptied before.
struct Loading {
    order: u64,
    path: PathBuf,
    preview: Preview,
    /// Dropping it stops the reading of a document, which can take a while.
    _document: Option<Reading>,
}

/// What is typed and attached in a composer but not sent.
pub struct Draft {
    text: String,
    attachments: Vec<Attachment>,
    /// Files whose attachments were still being read.
    unread: Vec<PathBuf>,
    /// The SSH host of the session it was written in.
    host: Option<HostId>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.attachments.is_empty() && self.unread.is_empty()
    }

    /// Adds a later draft of the same session after this one.
    pub fn append(&mut self, later: Self) {
        self.text = merge_restored(&self.text, &later.text, false);
        self.attachments.extend(later.attachments);
        self.unread.extend(later.unread);
    }

    #[cfg(test)]
    pub fn for_tests(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            attachments: Vec::new(),
            unread: Vec::new(),
            host: None,
        }
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }
}

pub struct Composer {
    session: Entity<PiSession>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    /// Attachments still being read.
    loading: Vec<Loading>,
    next_attachment: u64,
    /// Repaints the spinners while something is being read.
    spinning: Option<Task<()>>,
    /// The attach popover's file list (searchable, indexed in the background).
    picker: Entity<ListState<FilePicker>>,
    picker_open: bool,
    /// The context breakdown popover is open.
    context_open: bool,
    /// Inline `@file` completion for the mention being typed.
    mention: Option<Mention>,
    /// The `/` list: its highlighted row, and whether Esc closed it.
    suggestions: SuggestionList,
    suggestions_scroll: ScrollHandle,
    /// ↑ / ↓ through earlier prompts.
    history: History,
    /// Dictation: microphone, local recognizer and the live transcript.
    speech: Entity<SpeechState>,
    /// While dictating: the draft as it was when dictation began, and the transcript so far.
    dictation: Option<Dictation>,
    /// Focus for the dictation view (it replaces the input while dictating).
    dictation_focus: FocusHandle,
    /// The dictation shortcut between its press and release (hold to talk, tap to toggle).
    push_to_talk: crate::stt::shortcut::PushToTalk,
    /// Deep search's effort, as the index of an [`Effort`].
    effort: Entity<SliderState>,
    /// The effort is being changed: what the session says is older.
    effort_moving: bool,
    _subscriptions: Vec<Subscription>,
}

/// A dictation in progress.
struct Dictation {
    draft: String,
    transcript: SharedString,
}

/// The `@` mention at the cursor and the files that match it.
struct Mention {
    /// Byte offset of the `@` in the input.
    start: usize,
    matches: Vec<String>,
    selected: usize,
}

/// Files offered for an `@` mention at once.
const MENTION_RESULTS: usize = 8;
/// The distance between two stops of the effort slider.
const EFFORT_STOP: f32 = 28.;
/// The `/` list's height: about 8 rows, then it scrolls.
const SUGGESTIONS_HEIGHT: Rems = rems(16.5);

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(session: Entity<PiSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = match session.read(cx).mode() {
            Mode::Chat => "Message pi…  (/ for commands)",
            Mode::Code => "Message pi…  (/ for commands, ! to run a shell command)",
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .submit_on_enter(true)
                .placeholder(placeholder)
        });
        let speech = cx.new(|cx| {
            let state = SpeechState::new(cx)
                .recognizer(crate::stt::engine::LocalRecognizer {
                    config: Box::new(crate::stt::session_config),
                })
                // Only the local model: the system recognizers may send audio to a service.
                .system_fallback(false)
                .stop_timeout(Duration::from_secs(15));
            match crate::stt::engine::WavInput::from_env() {
                Some(input) => state.input(input),
                None => state,
            }
        });
        let mut subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { shift: false, .. } => {
                            if this.mention.is_some() {
                                this.accept_mention(window, cx);
                            } else if !this.enter_suggestion(window, cx) {
                                this.submit(false, window, cx);
                            }
                        }
                        InputEvent::Change => this.on_input_changed(cx),
                        _ => {}
                    }
                }),
                cx.observe(&session, |_, _, cx| cx.notify()),
                // Repaint the device button's status dot.
                cx.observe(&crate::hosts::hosts(cx), |_, _, cx| cx.notify()),
            ];
        // Repaint the sandbox badge.
        let security = crate::security::security(cx);
        subscriptions.push(cx.observe(&security, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe_in(
            &speech,
            window,
            |this, _, event: &SpeechEvent, window, cx| this.on_speech(event, window, cx),
        ));
        subscriptions.push(cx.observe(&speech, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&crate::stt::models::models(cx), |_, _, cx| cx.notify()));
        let effort = effort_value(session.read(cx).web_effort());
        let effort = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(effort_value(Effort::Max))
                .step(1.)
                .default_value(effort)
        });
        subscriptions.push(cx.subscribe(&effort, Self::on_effort));
        subscriptions.push(
            cx.observe_in(&session, window, |this, session, window, cx| {
                let effort = session.read(cx).web_effort();
                let value = effort_value(effort);
                if !this.effort_moving && effort_at(this.effort.read(cx).value().end()) != effort {
                    this.effort
                        .update(cx, |effort, cx| effort.set_value(value, window, cx));
                }
            }),
        );
        let weak = cx.weak_entity();
        let root = session.read(cx).cwd().to_path_buf();
        let location = match session.read(cx).host().cloned() {
            Some(host) => crate::hosts::hosts(cx).update(cx, |hosts, _| hosts.location(&host)),
            None => workspace::Location::Local,
        };
        let picker = cx.new(|cx| {
            let delegate = FilePicker::new(root, location, move |pick, window, cx| {
                let _ = weak.update(cx, |this, cx| this.on_pick(pick, window, cx));
            });
            ListState::new(delegate, window, cx).searchable(true)
        });
        // Index the folder in the background (never for the welcome screen's placeholder).
        if session.read(cx).has_folder_tools() && !session.read(cx).is_idle() {
            file_picker::refresh_index(&picker, cx);
        }
        Self {
            session,
            input,
            attachments: Vec::new(),
            loading: Vec::new(),
            next_attachment: 0,
            spinning: None,
            picker,
            picker_open: false,
            context_open: false,
            mention: None,
            suggestions: SuggestionList::default(),
            suggestions_scroll: ScrollHandle::new(),
            history: History::default(),
            speech,
            dictation: None,
            dictation_focus: cx.focus_handle(),
            push_to_talk: crate::stt::shortcut::PushToTalk::default(),
            effort,
            effort_moving: false,
            _subscriptions: subscriptions,
        }
    }

    /// The microphone button: starts or stops dictation; the first time, offers to
    /// download a model.
    pub fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.speech.read(cx).status().is_active() || self.dictation_ready(window, cx) {
            self.speech.update(cx, |speech, cx| speech.toggle(cx));
        }
    }

    /// Whether a speech model is installed. If not, says why dictation cannot start: the
    /// download in progress, or the dialog that offers one (the button and the shortcut
    /// behave the same).
    fn dictation_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if crate::stt::session_config(cx).is_some() {
            return true;
        }
        if let Some(download) = crate::stt::models::models(cx).read(cx).download() {
            use gpui_kit::component::notification::Notification;
            let message = format!(
                "{} is still downloading ({:.0}%). You can keep typing meanwhile.",
                download.model.name,
                download.fraction() * 100.
            );
            window.push_notification(Notification::info(message), cx);
        } else {
            crate::stt::ui::open_download_dialog(window, cx);
        }
        false
    }

    /// The dictation shortcut went down (or repeats while held).
    fn on_dictation_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !is_dictation_shortcut(&event.keystroke) {
            return;
        }
        // The input must not see it (Shift+Space would type a space).
        cx.stop_propagation();
        use crate::stt::shortcut::Command;
        let active = self.speech.read(cx).status().is_active();
        match self
            .push_to_talk
            .key_down(Instant::now(), event.is_held, active)
        {
            Command::Start => {
                if self.dictation_ready(window, cx) {
                    self.speech.update(cx, |speech, cx| speech.start(cx));
                } else {
                    self.push_to_talk.reset();
                }
            }
            Command::Stop => self.speech.update(cx, |speech, cx| speech.stop(cx)),
            Command::Nothing => {}
        }
    }

    /// The shortcut was released: after a hold, this ends dictation and keeps the text.
    ///
    /// Releasing Space or one of the shortcut's modifiers both count, whichever comes first:
    /// macOS does not deliver the key-up of Space while ⌘ is still down.
    fn on_dictation_release(&mut self, cx: &mut Context<Self>) {
        use crate::stt::shortcut::Command;
        let active = self.speech.read(cx).status().is_active();
        if self.push_to_talk.key_up(Instant::now(), active) == Command::Stop {
            self.speech.update(cx, |speech, cx| speech.stop(cx));
        }
    }

    fn on_speech(&mut self, event: &SpeechEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SpeechEvent::Started => {
                self.mention = None;
                self.dictation = Some(Dictation {
                    draft: self.input.read(cx).value().to_string(),
                    transcript: SharedString::default(),
                });
                self.dictation_focus.focus(window, cx);
            }
            SpeechEvent::Partial(text) => {
                if let Some(dictation) = self.dictation.as_mut() {
                    dictation.transcript = text.clone();
                }
            }
            SpeechEvent::Final(text) => {
                if let Some(dictation) = self.dictation.take() {
                    let text = append_dictation(&dictation.draft, text);
                    self.set_text(&text, window, cx);
                }
            }
            SpeechEvent::Cancelled => {
                if self.dictation.take().is_some() {
                    self.focus_handle(cx).focus(window, cx);
                }
            }
            SpeechEvent::Error(error) => {
                use gpui_kit::component::notification::Notification;
                if self.dictation.take().is_some() {
                    self.focus_handle(cx).focus(window, cx);
                }
                window.push_notification(
                    Notification::error(format!("Dictation stopped: {error}")),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// The microphone (or stop) button at the right of the input.
    fn render_mic(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let status = self.speech.read(cx).status();
        let active = status.is_active();
        let downloading = crate::stt::models::models(cx).read(cx).download().is_some()
            && crate::stt::session_config(cx).is_none();
        let tooltip: String = match status {
            SpeechStatus::Connecting => "Loading the speech model…".into(),
            _ if active => "Stop dictation (Enter)".into(),
            _ if downloading => "Speech model is downloading…".into(),
            _ => format!(
                "Dictate (hold {} to talk, tap to toggle)",
                crate::stt::shortcut::LABEL
            ),
        };
        Button::new("dictate")
            .icon(if active {
                IconName::Square
            } else {
                IconName::Mic
            })
            .small()
            .when(active, |button| button.primary())
            // The toolbar's quiet style: a visible hover and press on the composer surface.
            .when(!active, |button| button.quiet(cx))
            .loading(status == SpeechStatus::Connecting)
            .tooltip(tooltip)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_dictation(window, cx)))
    }

    /// While dictating: the draft as it was, then the live transcript greyed out.
    fn render_dictation(&self, dictation: &Dictation, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let status = self.speech.read(cx).status();
        let separator = if dictation.draft.is_empty()
            || dictation.draft.ends_with(char::is_whitespace)
            || dictation.transcript.is_empty()
        {
            ""
        } else {
            " "
        };
        let placeholder = match status {
            SpeechStatus::Connecting => crate::stt::models::active_model(cx)
                .map(|model| format!("Loading {}…", model.name))
                .unwrap_or_default(),
            SpeechStatus::Stopping => "Finishing…".to_owned(),
            _ => "Listening… speak now".to_owned(),
        };
        let shown_transcript = if dictation.transcript.is_empty() {
            placeholder
        } else {
            dictation.transcript.to_string()
        };
        let text = format!("{}{separator}{shown_transcript}", dictation.draft);
        let grey = dictation.draft.len() + separator.len()..text.len();
        let italic = dictation.transcript.is_empty();
        v_flex()
            .id("dictation")
            .track_focus(&self.dictation_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.speech.update(cx, |speech, cx| speech.stop(cx)),
                    "escape" => this.speech.update(cx, |speech, cx| speech.cancel(cx)),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .w_full()
            .min_h(px(24.))
            // The textarea's own padding (medium size), so the text does not move when the
            // transcript becomes the draft.
            .px(px(10.))
            .py(px(8.))
            .gap_1()
            .child(
                div()
                    .w_full()
                    .child(StyledText::new(text).with_highlights([(
                        grey,
                        HighlightStyle {
                            color: Some(muted),
                            font_style: italic.then_some(FontStyle::Italic),
                            ..Default::default()
                        },
                    )])),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(SpeechWaveform::new(&self.speech).w(px(96.)).h(px(16.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Enter to finish · Esc to discard"),
                    ),
            )
            .into_any_element()
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    /// Sends `text` as if typed and sent with Enter (`PISTACHIO_OPEN=send:<text>`, for
    /// scripted end-to-end checks).
    pub fn send_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text(text, window, cx);
        if self.loading.is_empty() {
            self.submit(false, window, cx);
            return;
        }
        // Once what is attached is read.
        cx.spawn_in(window, async move |this, cx| {
            let wait = Duration::from_millis(100);
            while let Ok(true) = this.read_with(cx, |this, _| !this.loading.is_empty()) {
                cx.background_executor().timer(wait).await;
            }
            let _ = this.update_in(cx, |this, window, cx| this.submit(false, window, cx));
        })
        .detach();
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
    }

    /// Puts messages given back by the session into the composer: before the current draft
    /// (or instead of it with `replace`), with their files and images as attachments.
    pub fn restore(
        &mut self,
        text: &str,
        images: Vec<ImageContent>,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self.input.read(cx).value().to_string();
        let prompt = prompt::split(text);
        let merged = merge_restored(prompt.text, &draft, replace);
        self.input
            .update(cx, |input, cx| input.set_value(merged, window, cx));
        if replace {
            self.attachments.clear();
        }
        let dir = cx.global::<AppConfig>().documents_dir.clone();
        for (file, block) in prompt.files {
            let preview = match &file.id {
                Some(id) => remembered_preview(&dir, id),
                None => Preview::None,
            };
            self.attach(Attachment {
                order: 0,
                path: PathBuf::new(),
                name: file.name.into(),
                preview,
                image: None,
                inline: Some(InlineText::Sent(block.to_owned())),
                id: file.id,
            });
        }
        for (index, image) in images.into_iter().enumerate() {
            self.attach(Attachment {
                order: 0,
                path: PathBuf::new(),
                name: format!("Image {}", index + 1).into(),
                preview: images::from_base64(&image.data, &image.mime_type)
                    .map_or(Preview::None, Preview::Image),
                image: Some(image),
                inline: None,
                id: None,
            });
        }
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    /// Whether anything is typed or attached but not sent.
    pub fn has_draft(&self, cx: &App) -> bool {
        !self.input.read(cx).value().trim().is_empty()
            || !self.attachments.is_empty()
            || !self.loading.is_empty()
            || self
                .dictation
                .as_ref()
                .is_some_and(|dictation| !dictation.transcript.trim().is_empty())
    }

    #[cfg(test)]
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Takes what is typed and attached but not sent, leaving the composer empty: its chat is
    /// let go, or the draft moves to another one. A dictation ends, and what it heard so far
    /// is part of the draft.
    pub fn take_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Draft {
        let mut text = self.input.read(cx).value().to_string();
        if let Some(dictation) = self.dictation.take() {
            text = append_dictation(&text, &dictation.transcript);
        }
        self.speech.update(cx, |speech, cx| speech.cancel(cx));
        let draft = Draft {
            text,
            attachments: std::mem::take(&mut self.attachments),
            unread: std::mem::take(&mut self.loading)
                .into_iter()
                .map(|loading| loading.path)
                .collect(),
            host: self.session.read(cx).host().cloned(),
        };
        self.clear(window, cx);
        draft
    }

    /// Puts a draft taken from another composer into this one, before anything typed here.
    /// Attached files are read or referenced the way this session needs them; files on the
    /// draft's SSH host cannot come along to another machine.
    pub fn set_draft(&mut self, draft: Draft, window: &mut Window, cx: &mut Context<Self>) {
        if draft.is_empty() {
            return;
        }
        let typed = self.input.read(cx).value().to_string();
        let text = if typed.trim().is_empty() {
            draft.text
        } else {
            merge_restored(&draft.text, &typed, false)
        };
        self.input.update(cx, |input, cx| {
            let end = text.len();
            input.set_value(text, window, cx);
            input.set_selected_range(end..end, cx);
        });
        let (inline, moved) = {
            let session = self.session.read(cx);
            (
                !session.is_local_code(),
                draft.host.as_ref() != session.host(),
            )
        };
        let mut local = draft.unread;
        let mut left_behind = Vec::new();
        for attachment in draft.attachments {
            let on_host = draft.host.is_some() && attachment.inline.is_none();
            let sent = matches!(attachment.inline, Some(InlineText::Sent(_)));
            if attachment.image.is_some() || sent {
                self.attach(attachment);
            } else if on_host && moved {
                left_behind.push(attachment.name);
            } else if on_host || attachment.inline.is_some() == inline {
                self.attach(attachment);
            } else {
                local.push(attachment.path);
            }
        }
        if !local.is_empty() {
            self.attach_paths(local, cx);
        }
        if !left_behind.is_empty() {
            window.push_notification(
                format!(
                    "Not attached here, because they are on the SSH host: {}",
                    left_behind.join(", ")
                ),
                cx,
            );
        }
        cx.notify();
    }

    /// Inserts `text` at the cursor (e.g. an `@path` mention from the Files tab) and focuses
    /// the input.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.insert(text.to_owned(), window, cx));
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
    }

    fn submit(&mut self, follow_up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }

        if self.attachments.is_empty() {
            match commands::handle(&text, &self.session, window, cx) {
                CommandOutcome::Handled => {
                    self.clear(window, cx);
                    return;
                }
                CommandOutcome::Keep => return,
                CommandOutcome::NotACommand => {}
            }
        }

        // Still being read: the message waits here for it.
        if let Some(loading) = self.loading.first() {
            let name = attachment_name(&loading.path);
            window.push_notification(
                format!("Not sent yet: {name} is still being read. Wait for it, or remove it."),
                cx,
            );
            return;
        }

        // pi has stopped: keep the message (and attachments) here instead of queueing it
        // where it would never be sent.
        if let Lifecycle::Exited { .. } = self.session.read(cx).lifecycle() {
            window.push_notification(
                "pi is not running, so the message was kept. Reconnect or start a new session.",
                cx,
            );
            return;
        }

        let cwd = self.session.read(cx).cwd().to_owned();
        let mut blocks = Vec::new();
        let mut references = Vec::new();
        let mut images = Vec::new();
        let mut room = self.room(cx);
        for attachment in self.attachments.drain(..) {
            let block = |contents: &str| {
                prompt::block(&attachment.name, attachment.id.as_deref(), contents)
            };
            match (attachment.image, attachment.inline) {
                (Some(image), _) => images.push(image),
                (None, Some(InlineText::Document(document))) => {
                    let contents = document.contents(&text, &room);
                    room.take(contents.len());
                    blocks.push(block(&contents));
                }
                (None, Some(InlineText::Sent(sent))) => blocks.push(sent),
                (None, Some(InlineText::Skipped(reason))) => {
                    references.push(format!(
                        "(Attachment `{}` was not included: {reason}.)",
                        attachment.name
                    ));
                }
                (None, None) => {
                    let path = attachment
                        .path
                        .strip_prefix(&cwd)
                        .unwrap_or(&attachment.path);
                    references.push(mention::file_reference(&path.display().to_string()));
                }
            }
        }
        let message = if references.is_empty() {
            text
        } else {
            // What is said of files (chat mode) gets paragraphs; `@path`s share a line.
            let separator = if self.session.read(cx).is_local_code() {
                " "
            } else {
                "\n\n"
            };
            format!("{}\n\n{text}", references.join(separator))
                .trim()
                .to_owned()
        };

        let message = prompt::join(blocks.iter().map(String::as_str), &message);

        self.session.update(cx, |session, cx| {
            session.submit(message, images, follow_up, cx);
        });
        self.clear(window, cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attachments.clear();
        self.history.reset();
        self.mention = None;
        self.suggestions = SuggestionList::default();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn send_follow_up(&mut self, _: &SendFollowUp, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(true, window, cx);
    }

    fn interrupt(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.mention.take().is_some() {
            cx.notify();
            return;
        }
        // Also while a prompt is on its way or waits to be sent, not only during a run.
        if self.session.read(cx).is_busy() {
            self.session.update(cx, |session, cx| session.interrupt(cx));
        } else {
            cx.propagate();
        }
    }

    fn dequeue(&mut self, _: &Dequeue, _: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.dequeue(cx));
    }

    fn cycle_thinking(&mut self, _: &CycleThinking, _: &mut Window, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.cycle_thinking_level(cx));
    }

    pub fn open_context_breakdown(&mut self, cx: &mut Context<Self>) {
        self.context_open = true;
        cx.notify();
    }

    pub fn close_context_breakdown(&mut self, cx: &mut Context<Self>) {
        self.context_open = false;
        cx.notify();
    }

    /// The attach picker, whose index lists the folder's files.
    pub(crate) fn file_picker(&self) -> Entity<ListState<FilePicker>> {
        self.picker.clone()
    }

    /// Lists the folder's files again (e.g. after a turn that may have made some).
    pub fn refresh_file_index(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        if session.has_folder_tools() && !session.is_idle() {
            file_picker::refresh_index(&self.picker, cx);
        }
    }

    pub fn open_attach_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_picker_open(true, window, cx);
    }

    fn set_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open == open {
            return;
        }
        self.picker_open = open;
        if open {
            // Show the cached index at once; pick up new files in the background.
            file_picker::refresh_index(&self.picker, cx);
            self.picker.update(cx, |picker, cx| {
                picker.set_selected_index(Some(IndexPath::default()), window, cx);
                picker.focus(window, cx);
            });
        } else {
            let focus = self.focus_handle(cx);
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Called from inside the picker's own update, so it must not touch the picker.
    fn on_pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_open = false;
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        match pick {
            // A file on the SSH host: referenced as `@path` (pi reads it there).
            Pick::File(path) if self.session.read(cx).host().is_some() => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                self.attach(Attachment {
                    order: 0,
                    path,
                    name: name.into(),
                    preview: Preview::None,
                    image: None,
                    inline: None,
                    id: None,
                });
            }
            Pick::File(path) => self.attach_paths(vec![path], cx),
            Pick::Browse => self.pick_attachments(cx),
            Pick::Dismiss => {}
        }
        cx.notify();
    }

    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.attach_paths(paths, cx));
        })
        .detach();
    }

    /// Images and copied files on the clipboard become attachments (like pi's
    /// Ctrl+V); returns `false` to let plain text paste normally.
    fn paste(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        let mut paths = Vec::new();
        let mut consumed = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let mime = match image.format {
                        ImageFormat::Png => "image/png",
                        ImageFormat::Jpeg => "image/jpeg",
                        ImageFormat::Webp => "image/webp",
                        ImageFormat::Gif => "image/gif",
                        _ => continue,
                    };
                    let name = format!(
                        "pasted-image-{}.{}",
                        self.attachments.len() + 1,
                        mime.trim_start_matches("image/")
                    );
                    self.attach(Attachment {
                        order: 0,
                        path: PathBuf::from(&name),
                        name: name.into(),
                        preview: Preview::Image(Arc::new(image.clone())),
                        image: Some(ImageContent::new(
                            base64::engine::general_purpose::STANDARD.encode(&image.bytes),
                            mime,
                        )),
                        inline: None,
                        id: None,
                    });
                    consumed = true;
                }
                ClipboardEntry::ExternalPaths(external) => {
                    paths.extend(external.paths().iter().cloned());
                    consumed = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if !paths.is_empty() {
            self.attach_paths(paths, cx);
        }
        cx.notify();
        consumed
    }

    pub fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Without local file access (chat, or pi on another machine) the text of files goes along.
        let inline = !self.session.read(cx).is_local_code();
        // For how much of a text goes along.
        crate::document_parsers::document_parsers(cx);
        let dir = cx.global::<AppConfig>().documents_dir.clone();
        let svg = cx.svg_renderer();
        for path in paths {
            if inline && AttachedDocument::is_one(&path) {
                self.read_document(path, cx);
                continue;
            }
            let order = self.start_loading(path.clone(), None, cx);
            let (dir, svg) = (dir.clone(), svg.clone());
            cx.spawn(async move |this, cx| {
                let attachment = cx
                    .background_executor()
                    .spawn(async move { load_attachment(path, inline, &dir, &svg) })
                    .await;
                let _ = this.update(cx, |this, cx| this.loaded(order, attachment, cx));
            })
            .detach();
        }
    }

    /// Adds `attachment` after the others.
    fn attach(&mut self, attachment: Attachment) {
        let order = self.next_order();
        self.attachments.push(Attachment {
            order,
            ..attachment
        });
    }

    fn next_order(&mut self) -> u64 {
        self.next_attachment += 1;
        self.next_attachment
    }

    fn start_loading(
        &mut self,
        path: PathBuf,
        document: Option<Reading>,
        cx: &mut Context<Self>,
    ) -> u64 {
        let order = self.next_order();
        self.loading.push(Loading {
            order,
            path,
            preview: Preview::None,
            _document: document,
        });
        self.spin(cx);
        cx.notify();
        order
    }

    /// Repaints while something is being read.
    fn spin(&mut self, cx: &mut Context<Self>) {
        if self.spinning.is_some() || cx.reduce_motion() {
            return;
        }
        self.spinning = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(spinner::FRAME).await;
                let spinning = this.update(cx, |this, cx| {
                    let reading = this.is_reading();
                    if reading {
                        cx.notify();
                    } else {
                        this.spinning = None;
                    }
                    reading
                });
                if !spinning.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    /// What is loading as `order` was read; it keeps its place.
    fn loaded(&mut self, order: u64, attachment: Attachment, cx: &mut Context<Self>) {
        // Removed, or taken with the draft meanwhile: it is read again where it went.
        let Some(index) = self.loading.iter().position(|l| l.order == order) else {
            return;
        };
        self.loading.remove(index);
        let index = self.attachments.partition_point(|a| a.order < order);
        self.attachments.insert(
            index,
            Attachment {
                order,
                ..attachment
            },
        );
        cx.notify();
    }

    /// Reads a document for a chat with the parser in the settings.
    fn read_document(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (reading, stopped) = Reading::new();
        let order = self.start_loading(path.clone(), Some(reading), cx);
        let (exe, dir) = {
            let config = cx.global::<AppConfig>();
            (config.exe.clone(), config.documents_dir.clone())
        };
        // Its first page shows while it is read.
        let remembered = cx.background_executor().spawn({
            let (dir, path) = (dir.clone(), path.clone());
            async move {
                let id = attached::remember(&dir, &path);
                let preview = match &id {
                    Some(id) => remembered_preview(&dir, id),
                    None => Preview::None,
                };
                (id, preview)
            }
        });
        let (tx, rx) = async_channel::bounded(1);
        // A thread of its own: it can take minutes.
        let spawned = std::thread::Builder::new()
            .name("read-document".into())
            .spawn({
                let path = path.clone();
                move || {
                    let read = AttachedDocument::read(exe.as_ref(), &dir, &path, &stopped);
                    let _ = tx.send_blocking(read);
                }
            });
        cx.spawn(async move |this, cx| {
            let (id, preview) = remembered.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(loading) = this.loading.iter_mut().find(|l| l.order == order) {
                    loading.preview = preview.clone();
                    cx.notify();
                }
            });
            let read = match spawned {
                Ok(_) => rx.recv().await.unwrap_or_else(|e| Err(e.to_string())),
                Err(error) => Err(error.to_string()),
            };
            // What has no picture shows its start.
            let preview = match (&read, preview) {
                (Ok(document), Preview::None) => attached::start(document.text())
                    .map_or(Preview::None, |start| Preview::Text(start.into())),
                (_, preview) => preview,
            };
            let attachment = Attachment {
                order,
                name: attachment_name(&path),
                path,
                preview,
                image: None,
                inline: Some(match read {
                    Ok(document) => InlineText::Document(document),
                    Err(reason) => InlineText::Skipped(reason),
                }),
                id,
            };
            let _ = this.update(cx, |this, cx| this.loaded(order, attachment, cx));
        })
        .detach();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose working folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(folder) = paths.pop()
            {
                let _ = this.update(cx, |this, cx| this.change_folder(folder, cx));
            }
        })
        .detach();
    }

    fn change_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if folder != self.session.read(cx).cwd() {
            cx.emit(ComposerEvent::ChangeFolder(folder));
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session.read(cx);
        let folder = session.cwd().file_name().map_or_else(
            || session.cwd().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let folder_locked = !session.is_empty();
        let cwd = session.cwd().to_path_buf();

        if session.mode() == Mode::Chat {
            let web_mode = session.web_search_available().then(|| session.web_mode());
            // No project folder: attach straight from the system dialog, no folder picker.
            let left = h_flex()
                .gap_1()
                .child(
                    Button::new("attach")
                        .quiet(cx)
                        .small()
                        .icon(Icon::from(AppIcon::Paperclip))
                        .accessibility_label("Attach files or images")
                        .tooltip("Attach files or images")
                        .on_click(cx.listener(|this, _, _, cx| this.pick_attachments(cx))),
                )
                .when_some(web_mode, |left, mode| {
                    left.child(self.render_web_mode(mode, cx))
                        .when(mode == WebMode::Deep, |left| {
                            left.child(self.render_effort(cx))
                        })
                });
            return self.toolbar_layout(left, cx).into_any_element();
        }

        if let Some(host) = session.host().cloned() {
            // pi runs on another machine: the picker lists files there (referenced as
            // `@path`), "Browse other files…" pastes local ones in, and the folder button
            // browses folders on that machine.
            let left = h_flex()
                .gap_1()
                .child(self.render_attach_popover(cx))
                .child(self.render_device(Some(&host), cx))
                .child(shrinking(
                    "composer-folder",
                    FOLDER_SHRINK,
                    Button::new("remote-folder")
                        .quiet(cx)
                        .small()
                        .icon(IconName::Folder)
                        .child(FadingText::new(folder.clone()))
                        .accessibility_label(folder)
                        .tooltip(format!(
                            "{} (choosing another starts a new session)",
                            cwd.display()
                        ))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ComposerEvent::BrowseRemote(host.clone()));
                        })),
                ));
            return self.toolbar_layout(left, cx).into_any_element();
        }

        let left = h_flex()
            .gap_1()
            .child(self.render_attach_popover(cx))
            .child(self.render_device(None, cx))
            .child(shrinking(
                "composer-folder",
                FOLDER_SHRINK,
                Button::new(menu_id(
                    "folder",
                    &(&cwd, cx.try_global::<RecentFolders>().map(|r| r.0.clone())),
                ))
                .quiet(cx)
                .small()
                .icon(IconName::Folder)
                .child(FadingText::new(folder.clone()))
                .accessibility_label(folder)
                .tooltip(if folder_locked {
                    format!("{} (choosing another starts a new chat)", cwd.display())
                } else {
                    format!("{} (choose another working folder)", cwd.display())
                })
                .dropdown_menu_with_anchor(
                    Anchor::BottomLeft,
                    folder_menu(cx.weak_entity(), cwd.clone()),
                ),
            ))
            .when(sandbox::SUPPORTED, |left| {
                left.child(self.render_sandbox(&cwd, cx))
            });

        self.toolbar_layout(left, cx).into_any_element()
    }

    /// The globe (chats only): a menu to choose how the model uses the web; labelled and
    /// highlighted while search or deep search is on.
    fn render_web_mode(
        &self,
        current: WebMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let button = Button::new(menu_id("web-mode", &current.as_str()))
            .small()
            .accessibility_label("Web search")
            .icon(Icon::from(AppIcon::Globe))
            .toggled(current != WebMode::Off);
        let button = match current {
            WebMode::Off => button
                .quiet(cx)
                .tooltip("Web: off. Choose Search for quick lookups, Deep search for research"),
            WebMode::Search => button
                .label(web_mode_label(current))
                .tooltip("Search: the model looks things up on the web when needed"),
            WebMode::Deep => button.label(web_mode_label(current)).tooltip(
                "Deep search: the model researches thoroughly on the web before reporting back",
            ),
        };
        let handle = self.session.clone();
        crate::demo::mark(
            "web-mode",
            button.dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for mode in [WebMode::Off, WebMode::Search, WebMode::Deep] {
                    let handle = handle.clone();
                    menu = menu.item(
                        crate::demo::menu_item(
                            web_mode_label(mode),
                            format!("web:{}", mode.as_str()),
                        )
                        .checked(current == mode)
                        .on_click(move |_, _, cx| {
                            handle.update(cx, |session, cx| session.set_web_mode(mode, cx));
                        }),
                    );
                }
                menu
            }),
        )
    }

    fn on_effort(&mut self, _: Entity<SliderState>, event: &SliderEvent, cx: &mut Context<Self>) {
        match event {
            SliderEvent::Change(_) => self.effort_moving = true,
            SliderEvent::Release(value) => {
                self.effort_moving = false;
                let effort = effort_at(value.end());
                self.session
                    .update(cx, |session, cx| session.set_web_effort(effort, cx));
            }
        }
        cx.notify();
    }

    /// Deep search's effort: a slider with the name of the level it is at.
    fn render_effort(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let effort = effort_at(self.effort.read(cx).value().end());
        // The slider gives its first and last stop half as much room as those between: the rest
        // of theirs is beside it.
        let beside = |id: &'static str, end: Effort, cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_none()
                .w(px(EFFORT_STOP / 2.))
                .h_6()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.effort.update(cx, |effort, cx| {
                        effort.set_value(effort_value(end), window, cx);
                    });
                    this.session
                        .update(cx, |session, cx| session.set_web_effort(end, cx));
                }))
        };
        let before = beside("web-effort-first", Effort::Light, cx);
        let after = beside("web-effort-last", Effort::Max, cx);
        h_flex()
            .id("web-effort")
            .debug_selector(|| "web-effort".into())
            .flex_none()
            .tooltip(move |window, cx| {
                let text = format!("Effort: {}. {}", effort.label(), effort.summary());
                Tooltip::new(text).build(window, cx)
            })
            .child(before)
            .child(
                div()
                    .debug_selector(|| "web-effort-track".into())
                    .relative()
                    .flex_none()
                    .w(px(EFFORT_STOP * effort_value(Effort::Max)))
                    .child(effort_stops(effort, cx))
                    .child(
                        // Only its knob: the stops are the track.
                        crate::ui::knob::outlined(
                            Slider::new(&self.effort)
                                .bg(transparent_black())
                                .accessibility_label(format!(
                                    "Deep search effort: {}",
                                    effort.label()
                                )),
                            &self.effort,
                            cx,
                        ),
                    ),
            )
            .child(after)
            .child(
                // As wide as the longest name, so nothing moves with the slider, and clear of the
                // knob at its end. The letters' middle, not the line's, is level with the track.
                div()
                    .relative()
                    .top(px(-1.5))
                    .w(rems(4.5))
                    .pl_2()
                    .whitespace_nowrap()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(effort.label()),
            )
    }

    /// The attach button with its searchable list of the working folder's files.
    fn render_attach_popover(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let picker = self.picker.clone();
        let weak = cx.weak_entity();
        Popover::new("attach-popover")
            .anchor(Anchor::BottomLeft)
            .open(self.picker_open)
            .on_open_change(move |open, window, cx| {
                let _ = weak.update(cx, |this, cx| this.set_picker_open(*open, window, cx));
            })
            .trigger(
                Button::new("attach")
                    .quiet(cx)
                    .small()
                    .icon(Icon::from(AppIcon::Paperclip))
                    .accessibility_label("Attach files")
                    .tooltip("Attach files"),
            )
            .p_0()
            .content(move |_, _, _| List::new(&picker).w(px(460.)).h(px(320.)))
    }

    /// "This computer" / an SSH host, with its status dot; the menu switches machines.
    fn render_device(
        &self,
        current: Option<&HostId>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let hosts = crate::hosts::hosts(cx);
        let hosts = hosts.read(cx);
        let (label, status) = match current {
            Some(id) => (
                hosts
                    .get(id)
                    .map_or_else(|| id.0.clone(), |host| host.label.clone()),
                Some(hosts.status(id)),
            ),
            None => ("This computer".to_owned(), None),
        };
        let tooltip: SharedString = match &status {
            Some(status) => format!("{label}: {}", crate::remote_ui::status_text(status)).into(),
            None => "Runs on this computer".into(),
        };
        // Key the menu by hosts and statuses so its dots are never stale.
        let key: Vec<(String, String)> = hosts
            .hosts()
            .iter()
            .map(|host| (host.id.0.clone(), format!("{:?}", hosts.status(&host.id))))
            .collect();
        let button = Button::new(menu_id("device", &(current.map(|id| id.0.clone()), key)))
            .quiet(cx)
            .small()
            .child(
                h_flex()
                    .gap_1p5()
                    .child(crate::remote_ui::status_dot(status.as_ref(), cx))
                    .child(FadingText::new(label)),
            )
            .accessibility_label(tooltip.clone())
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(
                Anchor::BottomLeft,
                device_menu(cx.weak_entity(), current.cloned()),
            );
        shrinking("composer-device", 1., button)
    }

    /// The sandbox badge of a local Code session: a shield while its commands are sandboxed
    /// (with "No network" when that is off), a crossed-out one with "No sandbox" when they are
    /// not; its menu turns the sandbox off or on for the project and opens Settings → Security.
    fn render_sandbox(&self, cwd: &Path, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use crate::security::SessionSandbox;
        let security = crate::security::security(cx);
        let state = security.read(cx);
        let session = state.session(cwd);
        let all_off = !state.settings.enabled;
        let (icon, label, tooltip): (AppIcon, Option<&str>, &str) = match session {
            SessionSandbox::Sandboxed { network: true } | SessionSandbox::Checking => (
                AppIcon::ShieldCheck,
                None,
                "Sandboxed: commands can change only this project and can't read your keys",
            ),
            SessionSandbox::Sandboxed { network: false } => (
                AppIcon::ShieldCheck,
                Some("No network"),
                "Sandboxed: commands can change only this project, can't read your keys and \
                 have no network",
            ),
            SessionSandbox::Off => (
                AppIcon::ShieldOff,
                Some("No sandbox"),
                "Commands run as you, without a sandbox",
            ),
            SessionSandbox::Unavailable => (
                AppIcon::ShieldOff,
                Some("No sandbox"),
                "This computer has no sandbox the app can use (see Settings → Security)",
            ),
        };
        let warning = matches!(session, SessionSandbox::Off | SessionSandbox::Unavailable);
        let color = if warning {
            palette(cx).warning
        } else {
            cx.theme().muted_foreground
        };
        let project = cwd.to_path_buf();
        let button = Button::new(menu_id("sandbox", &(format!("{session:?}"), all_off)))
            .quiet(cx)
            .small()
            .child(
                h_flex()
                    .gap_1()
                    .text_color(color)
                    .child(Icon::from(icon).small())
                    .when_some(label, |row, label| row.child(label)),
            )
            .accessibility_label(tooltip)
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, cx| {
                let security = crate::security::security(cx);
                let toggle = match session {
                    SessionSandbox::Off if all_off => Some(("Turn the sandbox on", None)),
                    SessionSandbox::Off => Some(("Use the sandbox in this project", Some(false))),
                    SessionSandbox::Sandboxed { .. } | SessionSandbox::Checking => {
                        Some(("Run this project without a sandbox", Some(true)))
                    }
                    SessionSandbox::Unavailable => None,
                };
                if let Some((label, off)) = toggle {
                    let project = project.clone();
                    let security = security.clone();
                    menu = menu
                        .item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let project = project.clone();
                            security.update(cx, |security, cx| match off {
                                Some(off) => security.set_project_unsandboxed(&project, off, cx),
                                None => security.update(cx, |settings| settings.enabled = true),
                            });
                        }))
                        .separator();
                }
                menu.item(
                    PopupMenuItem::new("Security settings…").on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::OpenSecuritySettings), cx);
                    }),
                )
            });
        // Small and always whole: the folder and device give way instead.
        shrinking("composer-sandbox", 0., button)
    }

    /// The composer's bottom bar: `left` controls, then model, thinking, context and send.
    fn toolbar_layout(&self, left: Div, cx: &mut Context<Self>) -> Div {
        let session = self.session.read(cx);
        let running = session.is_running();
        let stop_label = session.can_stop().then_some(if running {
            "Interrupt (esc)"
        } else if session.is_compacting() {
            "Cancel compaction (esc)"
        } else {
            "Stop the command (esc)"
        });
        let ready = matches!(session.lifecycle(), Lifecycle::Ready | Lifecycle::Starting);
        let has_input =
            !self.input.read(cx).value().trim().is_empty() || !self.attachments.is_empty();
        let reading = self.is_reading();
        let send_label = if reading {
            "Wait until the attached files are read"
        } else if running {
            "Steer (enter) · follow-up (alt+enter)"
        } else {
            "Send (enter)"
        };
        let right = h_flex()
            .gap_1()
            .child(self.render_model_picker(cx))
            .child(self.render_thinking_picker(cx))
            .child(self.render_context_ring(cx))
            .child(crate::demo::mark(
                "send",
                if let Some(label) = stop_label.filter(|_| !has_input) {
                    Button::new("stop")
                        .primary()
                        .small()
                        .icon(IconName::Square)
                        .rounded(cx.theme().radius_full())
                        .accessibility_label(label)
                        .tooltip(label)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.interrupt(&Interrupt, window, cx);
                        }))
                } else {
                    Button::new("send")
                        .primary()
                        .small()
                        .icon(IconName::ArrowUp)
                        .rounded(cx.theme().radius_full())
                        .disabled(!has_input || !ready || reading)
                        .accessibility_label(send_label)
                        .tooltip(send_label)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(false, window, cx)))
                },
            ));

        // In a narrow column the left side gives way: the folder's name fades out, then the
        // device's, then they are clipped; the model, thinking level and send / stop always stay visible.
        h_flex()
            .w_full()
            .justify_between()
            .gap_1()
            .px_2()
            .pb_2()
            .child(
                left.debug_selector(|| "composer-left".into())
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden(),
            )
            .child(right.flex_none())
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let models = session.models().to_vec();
        let current = session.model().map(pi_rpc::protocol::Model::qualified_id);
        let label = session.model().map_or_else(
            || "Model".to_owned(),
            |model| model.display_name().to_owned(),
        );
        // On a host whose pi could not use this computer's models, say why.
        let empty = session
            .host()
            .and_then(|host| {
                let error = crate::hosts::hosts(cx).read(cx).link_error(host)?;
                Some(SharedString::from(format!(
                    "No models on the host. This computer's models can't be used there: {error}"
                )))
            })
            .unwrap_or_else(|| "No models available yet".into());
        let handle = self.session.clone();
        let composer = cx.weak_entity();
        let picker = Button::new(menu_id("model-picker", &(&current, models.len())))
            .quiet(cx)
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                let composer = composer.clone();
                let change = PopupMenuItem::new("Add or change models…")
                    .icon(IconName::Plus)
                    .on_click(move |_, _, cx| {
                        let _ = composer.update(cx, |_, cx| cx.emit(ComposerEvent::Models));
                    });
                // A provider can have hundreds.
                let long = models.len() > LONG_MODEL_MENU;
                let change = if long {
                    menu = menu.scrollable(true).item(change).separator();
                    None
                } else {
                    Some(change)
                };
                if models.is_empty() {
                    menu = menu.label(empty.clone());
                }
                for model in &models {
                    let (handle, model) = (handle.clone(), model.clone());
                    let checked = current.as_deref() == Some(model.qualified_id().as_str());
                    menu = menu.item(
                        described_item(
                            format!("model:{}", model.id),
                            model.display_name().to_owned(),
                            model.provider.clone(),
                        )
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            handle.update(cx, |session, cx| session.set_model(&model, cx));
                        }),
                    );
                }
                match change {
                    Some(change) => menu.separator().item(change),
                    None => menu,
                }
            });
        crate::demo::mark("model-picker", picker)
    }

    fn render_thinking_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let levels = session.thinking_levels();
        let current = session.thinking_level();
        let reasoning = session.model().is_some_and(|model| model.reasoning);
        let handle = self.session.clone();
        let picker = Button::new(menu_id("thinking-picker", &(&levels, current)))
            .quiet(cx)
            .small()
            .icon(Icon::from(AppIcon::Brain))
            .label(current.map_or_else(|| "off".to_owned(), |level| level.to_string()))
            .tooltip("Thinking level (shift+tab)")
            .disabled(!reasoning || levels.len() < 2)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                for level in &levels {
                    let (handle, level) = (handle.clone(), *level);
                    menu = menu.item(
                        crate::demo::menu_item(level.to_string(), format!("thinking:{level}"))
                            .checked(current == Some(level))
                            .on_click(move |_, _, cx| {
                                handle.update(cx, |session, cx| {
                                    session.set_thinking_level(level, cx);
                                });
                            }),
                    );
                }
                menu
            });
        crate::demo::mark("thinking-picker", picker)
    }

    fn render_context_ring(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let usage = session.context_usage();
        let cost = session.total_cost();
        #[allow(clippy::cast_precision_loss)]
        let percent = usage.map_or(0.0, |(used, window)| used as f64 / window as f64 * 100.0);
        let tooltip: SharedString = match usage {
            Some((used, window)) => {
                let mut text = format!(
                    "Context: {} / {} tokens ({percent:.1}%)",
                    transcript::format_tokens(used),
                    transcript::format_tokens(window)
                );
                if let Some(cost) = cost.filter(|cost| *cost > 0.0) {
                    let _ = write!(text, " · ${cost:.3}");
                }
                text.push_str(" · click for breakdown");
                text.into()
            }
            None => "Context usage unavailable".into(),
        };
        let colors = palette(cx);
        let color = if percent >= 90.0 {
            colors.error
        } else if percent >= 75.0 {
            colors.warning
        } else {
            colors.accent
        };
        let session = self.session.downgrade();
        let weak = cx.weak_entity();
        let popover = Popover::new("context-popover")
            .anchor(Anchor::BottomRight)
            .open(self.context_open)
            .on_open_change(move |open, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.context_open = *open;
                    cx.notify();
                });
            })
            .trigger(
                Button::new("context-ring")
                    .quiet(cx)
                    .small()
                    .compact()
                    .child(
                        #[allow(clippy::cast_possible_truncation)]
                        ProgressCircle::new("context-progress")
                            .value(percent as f32)
                            .color(color)
                            .size_4(),
                    )
                    // Instant: the numbers are the point of hovering.
                    .accessibility_label(tooltip.clone())
                    .tooltip(tooltip)
                    .tooltip_show_delay(Duration::ZERO),
            )
            .content(move |_, _, cx| match session.upgrade() {
                Some(session) => super::context_panel::render(session.read(cx), cx),
                None => div().into_any_element(),
            });
        crate::demo::mark("context-ring", popover)
    }

    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut cards: Vec<(u64, AnyElement)> = Vec::new();
        let mut room = self.room(cx);
        for attachment in &self.attachments {
            let order = attachment.order;
            let remove = cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.attachments.retain(|a| a.order != order);
                cx.notify();
            });
            let show = cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.show_attachment(order, window, cx);
            });
            let card = attachment.card(&mut room).render(remove, show, cx);
            cards.push((order, card.into_any_element()));
        }
        for loading in &self.loading {
            let order = loading.order;
            let remove = cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.loading.retain(|loading| loading.order != order);
                cx.notify();
            });
            let show = cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.show_attachment(order, window, cx);
            });
            let card = loading.card().render(remove, show, cx);
            cards.push((order, card.into_any_element()));
        }
        cards.sort_by_key(|(order, _)| *order);
        h_flex()
            .id("attachments")
            .flex_wrap()
            .gap_2()
            .px_3()
            .pt_3()
            .max_h(px(attachment_card::MAX_HEIGHT))
            .overflow_y_scroll()
            .children(cards.into_iter().map(|(_, card)| card))
    }

    /// How much text may go along with the next message.
    fn room(&self, cx: &App) -> Room {
        let limit = match crate::document_parsers::existing(cx) {
            Some(parsers) => parsers.whole_text(),
            None => usize::try_from(documents::config::WHOLE_TEXT_BYTES).ok(),
        };
        Room::new(limit, self.session.read(cx).context_usage())
    }

    /// Shows an attached picture, PDF or text file in its viewer.
    fn show_attachment(&self, order: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(loading) = self.loading.iter().find(|l| l.order == order) {
            pdf_viewer::open(loading.path.clone(), window, cx);
            return;
        }
        let Some(attachment) = self.attachments.iter().find(|a| a.order == order) else {
            return;
        };
        if let Some(image) = &attachment.image {
            let image = transcript::Image::new(&image.data, &image.mime_type);
            images::open_viewer(Arc::new([image]), 0, window, cx);
            return;
        }
        // Of a message that came back, only what is remembered is known.
        let dir = &cx.global::<AppConfig>().documents_dir;
        let remembered = || attached::original(dir, attachment.id.as_deref()?);
        let path = Some(attachment.path.clone())
            .filter(|path| path.is_file())
            .or_else(remembered);
        match path {
            Some(path) if attachment.is_plain() => text_viewer::open(path, window, cx),
            Some(path) => pdf_viewer::open(path, window, cx),
            None => {
                let name = &attachment.name;
                window.push_notification(format!("{name} is not on this computer."), cx);
            }
        }
    }

    /// Attachments are still being read.
    fn is_reading(&self) -> bool {
        !self.loading.is_empty()
    }

    /// The commands that complete what is typed, best first; none after Esc, or while the
    /// text is an earlier prompt or is being dictated.
    fn suggested(&self, cx: &App) -> Vec<Suggestion> {
        if self.dictation.is_some() || self.history.is_browsing() {
            return Vec::new();
        }
        let text = self.input.read(cx).value();
        self.suggestions
            .rows(&text, self.session.read(cx).commands())
    }

    /// The `/` list's rows. A fully typed command with no other match needs no list.
    fn suggestion_rows(&self, cx: &App) -> Vec<Suggestion> {
        let rows = self.suggested(cx);
        match rows.as_slice() {
            [only] if only.text() == *self.input.read(cx).value() => Vec::new(),
            _ => rows,
        }
    }

    fn move_suggestion(&mut self, rows: &[Suggestion], down: bool, cx: &mut Context<Self>) {
        let index = self.suggestions.move_selection(rows, down);
        self.suggestions_scroll.scroll_to_item(index);
        cx.notify();
    }

    /// Replaces the text with `suggestion`, ready for its arguments.
    fn accept_suggestion(
        &mut self,
        suggestion: &Suggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let completion = format!("{} ", suggestion.text());
        self.input.update(cx, |input, cx| {
            let end = input.value().len();
            input.set_selected_range(0..end, cx);
            input.replace(completion, window, cx);
        });
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    /// Enter with the `/` list open: completes the highlighted row, and runs it right away when
    /// it takes no arguments. Returns `false` when the text is to be sent as it is (no list, or
    /// the row is what is typed).
    fn enter_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.suggestion_rows(cx);
        let Some(row) = rows.get(self.suggestions.selected(&rows)) else {
            return false;
        };
        let typed = self.input.read(cx).value().to_string();
        if row.text() == typed {
            return false;
        }
        self.accept_suggestion(row, window, cx);
        // With attachments the text is a message, not a command.
        if self.attachments.is_empty() && row.runs_at_once(&typed) {
            self.submit(false, window, cx);
        }
        true
    }

    /// Esc: closes the `/` list. Returns whether it was open.
    fn close_suggestions(&mut self, cx: &mut Context<Self>) -> bool {
        if self.suggestion_rows(cx).is_empty() {
            return false;
        }
        let text = self.input.read(cx).value().to_string();
        self.suggestions.close(&text);
        cx.notify();
        true
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let found = self.suggestion_rows(cx);
        if found.is_empty() {
            return None;
        }
        let selected = self.suggestions.selected(&found);
        let theme = cx.theme();
        let (muted, hover, selected_bg) = (
            theme.muted_foreground,
            theme.secondary_hover,
            theme.secondary_active,
        );
        let rows = found.into_iter().enumerate().map(|(index, suggestion)| {
            let (name, description) = (suggestion.text(), suggestion.description.clone());
            h_flex()
                .id(("suggestion", index))
                .flex_none()
                .gap_3()
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .role(Role::ListBoxOption)
                .aria_label(SharedString::from(name.clone()))
                .aria_selected(index == selected)
                .when(index == selected, |this| this.bg(selected_bg))
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.accept_suggestion(&suggestion, window, cx);
                }))
                .child(div().flex_none().text_sm().child(name))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(description),
                )
        });
        Some(
            v_flex()
                .id("suggestion-list")
                .role(Role::ListBox)
                .aria_label("Commands")
                .max_h(SUGGESTIONS_HEIGHT)
                .overflow_y_scroll()
                .track_scroll(&self.suggestions_scroll)
                .px_1()
                .pt_1()
                .gap_0p5()
                .children(rows),
        )
    }

    /// Tab: the highlighted file or command.
    fn complete(&mut self, _: &Complete, window: &mut Window, cx: &mut Context<Self>) {
        if !self.complete_selected(window, cx) {
            cx.propagate();
        }
    }

    /// Accepts the highlighted row of the `@` or `/` list. Returns whether there was one.
    fn complete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mention.is_some() {
            self.accept_mention(window, cx);
            return true;
        }
        let rows = self.suggested(cx);
        let Some(row) = rows.get(self.suggestions.selected(&rows)) else {
            return false;
        };
        self.accept_suggestion(row, window, cx);
        true
    }

    /// The input changed: follow the `@` mention at the cursor and the `/` list, and stop
    /// browsing history once the shown entry is edited.
    fn on_input_changed(&mut self, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let input = self.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        if self.history.is_browsing() && !self.history_entries(cx).contains(&text) {
            self.history.reset();
        }
        let rows = commands::suggestions(&text, self.session.read(cx).commands());
        self.suggestions.text_changed(&text, &rows);
        self.suggestions_scroll
            .scroll_to_item(self.suggestions.selected(&rows));
        let query = self
            .session
            .read(cx)
            .has_folder_tools()
            .then(|| mention::mention_at(&text, cursor))
            .flatten();
        self.mention = query.and_then(|query| {
            let files = self.picker.read(cx).delegate().files();
            let matches: Vec<String> = file_picker::rank(&files, query.query, MENTION_RESULTS)
                .into_iter()
                .filter_map(|index| files.get(index).cloned())
                .collect();
            // Keep the selection on the same file while the list refines.
            let selected = self
                .mention
                .as_ref()
                .and_then(|old| old.matches.get(old.selected))
                .and_then(|file| matches.iter().position(|m| m == file))
                .unwrap_or(0);
            (!matches.is_empty()).then_some(Mention {
                start: query.start,
                matches,
                selected,
            })
        });
        cx.notify();
    }

    /// Replaces the mention being typed with the chosen file, as pi writes it.
    fn accept_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mention) = self.mention.take() else {
            return;
        };
        let Some(file) = mention.matches.get(mention.selected) else {
            return;
        };
        let reference = format!("{} ", mention::file_reference(file));
        self.input.update(cx, |input, cx| {
            let cursor = input.cursor();
            input.set_selected_range(mention.start..cursor, cx);
            input.replace(reference, window, cx);
        });
        self.mention = None;
        cx.notify();
    }

    fn move_mention(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(mention) = &mut self.mention {
            let last = mention.matches.len().saturating_sub(1);
            mention.selected = if down {
                (mention.selected + 1).min(last)
            } else {
                mention.selected.saturating_sub(1)
            };
            cx.notify();
        }
    }

    /// This session's earlier prompts, oldest first.
    fn history_entries(&self, cx: &App) -> Vec<String> {
        let session = self.session.read(cx);
        mention::history_entries(session.transcript().items().iter().filter_map(|item| {
            match &item.kind {
                transcript::ItemKind::User(user) => Some(user.typed()),
                _ => None,
            }
        }))
    }

    /// ↑ in the input: the `@` or `/` list, or an earlier prompt when the composer is empty (or
    /// shows one) and the cursor is on the first line. Otherwise the cursor moves as usual.
    fn on_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention.is_some() {
            self.move_mention(false, cx);
            cx.stop_propagation();
            return;
        }
        let rows = self.suggestion_rows(cx);
        if !rows.is_empty() {
            self.move_suggestion(&rows, false, cx);
            cx.stop_propagation();
            return;
        }
        let (text, cursor) = {
            let input = self.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        let on_first_line = !text[..cursor.min(text.len())].contains('\n');
        if !(text.trim().is_empty() || self.history.is_browsing()) || !on_first_line {
            return;
        }
        let entries = self.history_entries(cx);
        if let Some(entry) = self.history.previous(&entries, &text) {
            let entry = entry.to_owned();
            self.input
                .update(cx, |input, cx| input.set_value(entry, window, cx));
            cx.stop_propagation();
        }
    }

    /// ↓ in the input: the `@` or `/` list, or the next prompt while browsing history (the
    /// cursor on the last line).
    fn on_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention.is_some() {
            self.move_mention(true, cx);
            cx.stop_propagation();
            return;
        }
        let rows = self.suggestion_rows(cx);
        if !rows.is_empty() {
            self.move_suggestion(&rows, true, cx);
            cx.stop_propagation();
            return;
        }
        if !self.history.is_browsing() {
            return;
        }
        let (text, cursor) = {
            let input = self.input.read(cx);
            (input.value().to_string(), input.cursor())
        };
        if text.get(cursor..).is_some_and(|rest| rest.contains('\n')) {
            return;
        }
        let entries = self.history_entries(cx);
        if let Some(entry) = self.history.next(&entries) {
            self.input
                .update(cx, |input, cx| input.set_value(entry, window, cx));
            cx.stop_propagation();
        }
    }

    fn render_mention(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let mention = self.mention.as_ref()?;
        let theme = cx.theme();
        let (muted, hover, selected_bg) = (
            theme.muted_foreground,
            theme.secondary_hover,
            theme.secondary_active,
        );
        let rows = mention.matches.iter().enumerate().map(|(index, file)| {
            let (dir, name) = match file.rsplit_once('/') {
                Some((dir, name)) => (format!("{dir}/"), name.to_owned()),
                None => (String::new(), file.clone()),
            };
            h_flex()
                .id(("mention", index))
                .gap_1()
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .role(Role::ListBoxOption)
                .aria_label(SharedString::from(file.clone()))
                .aria_selected(index == mention.selected)
                .when(index == mention.selected, |this| this.bg(selected_bg))
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(mention) = &mut this.mention {
                        mention.selected = index;
                    }
                    this.accept_mention(window, cx);
                }))
                .child(div().flex_none().text_sm().child(name))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(dir),
                )
        });
        Some(
            v_flex()
                .id("mention-list")
                .role(Role::ListBox)
                .aria_label("Files")
                .px_1()
                .pt_1()
                .gap_0p5()
                .children(rows),
        )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        // Deep search's status drives its toolbar button and the "Working…" line instead.
        let statuses = self
            .session
            .read(cx)
            .extension_status()
            .iter()
            .filter(|(key, _)| {
                ![
                    deepsearch::STATUS_KEY,
                    deepsearch::EFFORT_KEY,
                    deepsearch::PROGRESS_KEY,
                ]
                .contains(&key.as_str())
            })
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>();
        if statuses.is_empty() {
            return None;
        }
        let text = statuses.join("  ·  ");
        Some(
            div()
                .px_3()
                .pb_1()
                .truncate()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let session = self.session.read(cx);
        let queue = session.transcript().queue();
        let widgets = session.widgets();
        let pending: Vec<(String, usize)> = session
            .pending_prompts()
            .map(|(text, images, _)| (text.to_owned(), images))
            .collect();
        if queue.is_empty() && widgets.is_empty() && pending.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
        // Typed but not handed to pi yet (it is starting, or busy with the last one).
        let pending_lines = pending
            .into_iter()
            .enumerate()
            .map(|(index, (text, images))| {
                let text = waiting_line(&text, images);
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .flex_none()
                            .text_color(muted)
                            .child("↳ Waiting to send:"),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(text))
                    .child(
                        Button::new(("cancel-pending", index))
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip("Cancel (back to the message box)")
                            .accessibility_label("Cancel waiting message")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.session
                                    .update(cx, |session, cx| session.cancel_pending(index, cx));
                            })),
                    )
            });
        let lines = queue
            .steering
            .iter()
            .map(|text| ("Steering", text.clone()))
            .chain(
                queue
                    .follow_up
                    .iter()
                    .map(|text| ("Follow-up", text.clone())),
            )
            .map(move |(kind, text)| {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().text_color(muted).child(format!("↳ {kind}:")))
                    .child(div().truncate().child(waiting_line(&text, 0)))
            });
        let widget_lines = widgets
            .values()
            .flatten()
            .cloned()
            .map(move |line| div().text_xs().text_color(muted).child(line));
        Some(
            v_flex()
                .px_3()
                .pt_2()
                .gap_0p5()
                .children(widget_lines)
                .children(pending_lines)
                .children(lines)
                .when(!queue.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("alt+↑ to edit queued messages"),
                    )
                }),
        )
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("composer");
        let colors = palette(cx);
        let queue = self.render_queue(cx);
        let suggestions = self.render_suggestions(cx);
        let mention = self.render_mention(cx);
        let status = self.render_status(cx);
        let has_attachments = !self.attachments.is_empty() || self.is_reading();
        v_flex()
            .key_context(COMPOSER_CONTEXT)
            .on_action(cx.listener(Self::send_follow_up))
            .on_action(cx.listener(Self::interrupt))
            .on_action(cx.listener(Self::dequeue))
            .on_action(cx.listener(Self::cycle_thinking))
            .on_action(cx.listener(Self::complete))
            .capture_key_down(cx.listener(Self::on_dictation_key_down))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if event.keystroke.key == "space" {
                    this.on_dictation_release(cx);
                }
            }))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                if !dictation_modifiers_held(event.modifiers) {
                    this.on_dictation_release(cx);
                }
            }))
            .capture_action(cx.listener(Self::on_move_up))
            .capture_action(cx.listener(Self::on_move_down))
            // The input binds Tab and Esc itself; with the `@` or `/` list open they act on it.
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.complete_selected(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.mention.take().is_some() {
                    cx.notify();
                    cx.stop_propagation();
                } else if this.close_suggestions(cx) {
                    cx.stop_propagation();
                }
            }))
            // Esc with the focus on one of the composer's buttons.
            .capture_action(cx.listener(|this, _: &Interrupt, _, cx| {
                if this.close_suggestions(cx) {
                    cx.stop_propagation();
                }
            }))
            // The input binds Esc to its own `Escape` action, so the `escape` → `Interrupt`
            // binding never matches while typing. What the input lets bubble up (it had no
            // selection or completion to dismiss) interrupts the agent.
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                this.interrupt(&Interrupt, window, cx);
            }))
            .w_full()
            .rounded_xl()
            .border_1()
            .border_color(cx.theme().input)
            .bg(colors.surface)
            .shadow_sm()
            .children(suggestions)
            .children(mention)
            .children(queue)
            .when(has_attachments, |this| {
                this.child(self.render_attachments(cx))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(paths.paths().to_vec(), cx);
            }))
            .child(
                div()
                    .relative()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(match self.dictation.as_ref() {
                                        Some(dictation) => self.render_dictation(dictation, cx),
                                        None => {
                                            let weak = cx.weak_entity();
                                            Textarea::new(&self.input)
                                                .aria_label("Message")
                                                .appearance(false)
                                                .on_paste(move |item, _, cx| {
                                                    weak.update(cx, |this, cx| this.paste(item, cx))
                                                        .unwrap_or(false)
                                                })
                                                .into_any_element()
                                        }
                                    }),
                            )
                            .child(self.render_mic(cx)),
                    )
                    .children(crate::demo::probe("composer-input")),
            )
            .child(self.render_toolbar(cx))
            .children(status)
    }
}

/// The parsed dictation shortcut.
fn dictation_shortcut() -> &'static Keystroke {
    static SHORTCUT: std::sync::OnceLock<Keystroke> = std::sync::OnceLock::new();
    SHORTCUT.get_or_init(|| {
        Keystroke::parse(crate::stt::shortcut::KEYS).expect("the dictation shortcut parses")
    })
}

/// `keystroke` is the dictation shortcut (exactly its modifiers, so ⌘⌥⇧Space is not).
fn is_dictation_shortcut(keystroke: &Keystroke) -> bool {
    let shortcut = dictation_shortcut();
    keystroke.key == shortcut.key && keystroke.modifiers == shortcut.modifiers
}

/// All of the shortcut's modifiers are still down.
fn dictation_modifiers_held(modifiers: Modifiers) -> bool {
    let wanted = dictation_shortcut().modifiers;
    (!wanted.platform || modifiers.platform)
        && (!wanted.control || modifiers.control)
        && (!wanted.shift || modifiers.shift)
        && (!wanted.alt || modifiers.alt)
}

/// The composer text after dictating `spoken` into a composer holding `draft`.
fn append_dictation(draft: &str, spoken: &str) -> String {
    let spoken = spoken.trim();
    if spoken.is_empty() {
        draft.to_owned()
    } else if draft.is_empty() || draft.ends_with(char::is_whitespace) {
        format!("{draft}{spoken}")
    } else {
        format!("{draft} {spoken}")
    }
}

/// The composer text after restoring `restored` into a composer holding `draft`.
/// A message that waits to be sent, in one line.
fn waiting_line(text: &str, images: usize) -> String {
    let prompt = prompt::split(text);
    let files = prompt.files.len();
    let with: Vec<_> = [(files, "file(s)"), (images, "image(s)")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect();
    match (prompt.text, with.join(", ")) {
        (text, with) if with.is_empty() => text.to_owned(),
        ("", with) => format!("[{with}]"),
        (text, with) => format!("{text} [+{with}]"),
    }
}

fn merge_restored(restored: &str, draft: &str, replace: bool) -> String {
    let (restored, draft) = (restored.trim_end(), draft.trim());
    if replace || draft.is_empty() {
        restored.to_owned()
    } else if restored.is_empty() {
        draft.to_owned()
    } else {
        format!("{restored}\n\n{draft}")
    }
}

impl Loading {
    fn card(&self) -> Card {
        let name = attachment_name(&self.path);
        Card {
            id: ElementId::NamedInteger("attachment".into(), self.order),
            extension: marker(&name, None),
            name,
            preview: self.preview.clone(),
            status: "Reading…".into(),
            why: None,
            left_out: false,
            reading: true,
            can_show: is_pdf(&self.path),
        }
    }
}

impl Attachment {
    /// Its card, after others took their part of `room`.
    fn card(&self, room: &mut Room) -> Card {
        let (remark, why) = self.remark(room);
        if let Some(InlineText::Document(document)) = &self.inline {
            room.take(document.text().len());
        }
        Card {
            id: ElementId::NamedInteger("attachment".into(), self.order),
            name: self.name.clone(),
            extension: marker(&self.name, self.image.as_ref()),
            preview: self.preview.clone(),
            status: remark.unwrap_or_else(|| self.kind()).into(),
            why,
            left_out: matches!(self.inline, Some(InlineText::Skipped(_))),
            reading: false,
            can_show: self.image.is_some()
                || self.is_plain()
                || is_pdf(Path::new(self.name.as_ref())),
        }
    }

    /// Whether it is plain text.
    fn is_plain(&self) -> bool {
        let read = matches!(self.inline, Some(InlineText::Document(_)));
        documents::Kind::of(Path::new(self.name.as_ref())).is_none()
            && self.image.is_none()
            && (read || matches!(self.preview, Preview::Text(_)))
    }

    fn kind(&self) -> &'static str {
        if self.image.is_some() {
            "Image"
        } else if let Some(kind) = documents::Kind::of(Path::new(self.name.as_ref())) {
            kind.label()
        } else if self.is_plain() {
            "Text"
        } else {
            "File"
        }
    }

    /// What its card says besides the name and the kind of file, and why.
    fn remark(&self, room: &Room) -> (Option<&'static str>, Option<SharedString>) {
        match &self.inline {
            Some(InlineText::Skipped(reason)) => (
                Some("Not included"),
                Some(format!("It is not included: {reason}.").into()),
            ),
            Some(InlineText::Document(document)) if document.is_cut(room) => (
                Some("Shortened"),
                Some(shortened(document.text().len()).into()),
            ),
            Some(InlineText::Document(document)) => match document.note() {
                Some(note) => (Some("Built-in reader"), Some(note.to_owned().into())),
                None => (None, None),
            },
            _ => (None, None),
        }
    }
}

/// The extension a card shows: pictures have that of their format, as once they are sent.
fn marker(name: &str, image: Option<&ImageContent>) -> Option<SharedString> {
    image
        .and_then(|image| transcript::image_extension(&image.mime_type))
        .map(SharedString::from)
        .or_else(|| prompt::extension(name).map(Into::into))
}

/// The small picture of the file that is remembered as `id`.
fn remembered_preview(dir: &Path, id: &str) -> Preview {
    match attached::picture(dir, id) {
        Some(page) => Preview::Page(page),
        None => attached::original(dir, id)
            .as_deref()
            .map_or(Preview::None, text_preview),
    }
}

fn shortened(bytes: usize) -> String {
    let size = if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        #[allow(clippy::cast_precision_loss)] // a size to show
        let mb = bytes as f64 / (1024. * 1024.);
        format!("{mb:.1} MB")
    };
    format!(
        "It is {size} of text, so the model gets its start and the parts that match your message. \
         Settings → Document parsing says how much goes whole."
    )
}

fn is_pdf(path: &Path) -> bool {
    documents::Kind::of(path) == Some(documents::Kind::Pdf)
}

fn text_preview(path: &Path) -> Preview {
    // Some PDFs start like a text.
    if documents::Kind::of(path).is_some() {
        return Preview::None;
    }
    attached::first_lines(path).map_or(Preview::None, |lines| Preview::Text(lines.into()))
}

fn attachment_name(path: &Path) -> SharedString {
    path.file_name()
        .map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into()
}

fn load_attachment(path: PathBuf, inline: bool, dir: &Path, svg: &SvgRenderer) -> Attachment {
    let name = attachment_name(&path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let as_it_is = match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    };
    let image = match as_it_is {
        Some(mime) => std::fs::read(&path).ok().map(|bytes| (bytes, mime)),
        // In Code sessions the source of a drawing is what matters.
        None if extension == "svg" && !inline => None,
        None => convert::to_png(&path, &extension, svg).map(|png| (png, "image/png")),
    };
    let image = image.map(|(bytes, mime)| {
        ImageContent::new(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            mime,
        )
    });
    let preview = match &image {
        Some(image) => {
            images::from_base64(&image.data, &image.mime_type).map_or(Preview::None, Preview::Image)
        }
        None => text_preview(&path),
    };
    let inline = (inline && image.is_none()).then(|| match AttachedDocument::read_plain(&path) {
        Ok(text) => InlineText::Document(text),
        Err(reason) => InlineText::skipped(reason),
    });
    let id = match &inline {
        Some(InlineText::Document(_)) => attached::remember(dir, &path),
        _ => None,
    };
    Attachment {
        order: 0,
        path,
        name,
        preview,
        image,
        inline,
        id,
    }
}

/// Builds the folder menu: recent working folders (instant, no disk access) and a fallback to
/// the system folder dialog, which macOS takes a noticeable moment to open.
/// The model menu scrolls when it has more models than this.
const LONG_MODEL_MENU: usize = 15;

const FOLDER_MENU_WIDTH: f32 = 440.;

/// The device menu: this computer, saved SSH hosts (with status dots) and "Add SSH connection…".
fn device_menu(
    composer: WeakEntity<Composer>,
    current: Option<HostId>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |mut menu, _, cx| {
        let hosts = crate::hosts::hosts(cx);
        let entries: Vec<(
            Option<HostId>,
            String,
            Option<crate::hosts::HostStatus>,
            String,
        )> = std::iter::once((None, "This computer".to_owned(), None, String::new()))
            .chain(hosts.read(cx).hosts().iter().map(|host| {
                let status = hosts.read(cx).status(&host.id);
                (
                    Some(host.id.clone()),
                    host.label.clone(),
                    Some(status),
                    host.target.display(),
                )
            }))
            .collect();
        menu = menu
            .min_w(px(240.))
            .max_w(px(FOLDER_MENU_WIDTH))
            .label("Run on");
        for (id, label, status, detail) in entries {
            let checked = id == current;
            let composer = composer.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    h_flex()
                        .min_w_0()
                        .max_w(px(FOLDER_MENU_WIDTH - 64.))
                        .gap_2()
                        .child(crate::remote_ui::status_dot(status.as_ref(), cx))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .truncate()
                                .child(label.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(detail.clone()),
                        )
                })
                .checked(checked)
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    let _ = composer.update(cx, |_, cx| cx.emit(ComposerEvent::ChooseHost(id)));
                }),
            );
        }
        let (add, manage) = (composer.clone(), composer.clone());
        menu.separator()
            .item(
                PopupMenuItem::new("Add SSH connection…")
                    .icon(IconName::Plus)
                    .on_click(move |_, _, cx| {
                        let _ = add.update(cx, |_, cx| cx.emit(ComposerEvent::AddHost));
                    }),
            )
            .item(
                PopupMenuItem::new("Manage SSH connections…")
                    .icon(IconName::Settings)
                    .on_click(move |_, _, cx| {
                        let _ = manage.update(cx, |_, cx| cx.emit(ComposerEvent::ManageHosts));
                    }),
            )
    }
}

/// `(folder name, shortened parent)` for a menu row; the home folder is just `~`.
pub(crate) fn folder_label(folder: &Path, home: Option<&Path>) -> (SharedString, SharedString) {
    let short = crate::ui::short_path(folder, home);
    match (folder.file_name(), folder.parent()) {
        _ if short == "~" => ("~".into(), SharedString::default()),
        (Some(name), Some(parent)) => (
            name.to_string_lossy().into_owned().into(),
            crate::ui::short_path(parent, home).into(),
        ),
        _ => (short.into(), SharedString::default()),
    }
}

/// Element id for a dropdown whose items depend on `key`. gpui-kit keeps a built menu per
/// trigger id and only rebuilds it after certain dismissals, so a stale menu could be shown
/// (e.g. the previous model's thinking levels); a content-derived id always gets a fresh one.
/// The effort slider's track: a line with a dot per level, filled up to `effort`.
fn effort_stops(effort: Effort, cx: &App) -> Div {
    const DOT: f32 = 8.;
    const LINE: f32 = 3.;
    let colors = palette(cx);
    let rest = cx.theme().muted_foreground.opacity(0.45);
    let line =
        |width: f32, color: Hsla| div().absolute().left_0().w(px(width)).h(px(LINE)).bg(color);
    let dots = Effort::ALL.map(|level| {
        div()
            .absolute()
            .left(px(EFFORT_STOP * effort_value(level) - DOT / 2.))
            .size(px(DOT))
            .rounded_full()
            .map(|dot| {
                if level <= effort {
                    dot.bg(colors.accent)
                } else {
                    // Hollow: the line does not show through.
                    dot.bg(colors.surface).border_1().border_color(rest)
                }
            })
    });
    h_flex()
        .absolute()
        .inset_0()
        .child(line(EFFORT_STOP * effort_value(Effort::Max), rest))
        .child(line(EFFORT_STOP * effort_value(effort), colors.accent))
        .children(dots)
}

#[allow(clippy::cast_precision_loss)] // One of four.
fn effort_value(effort: Effort) -> f32 {
    Effort::ALL
        .iter()
        .position(|other| *other == effort)
        .unwrap_or_default() as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // One of four.
fn effort_at(value: f32) -> Effort {
    Effort::ALL
        .get(value.round() as usize)
        .copied()
        .unwrap_or_default()
}

fn web_mode_label(mode: WebMode) -> &'static str {
    match mode {
        WebMode::Off => "Off",
        WebMode::Search => "Search",
        WebMode::Deep => "Deep search",
    }
}

fn menu_id(name: &'static str, key: &impl std::hash::Hash) -> ElementId {
    use std::hash::BuildHasher as _;

    ElementId::NamedInteger(
        name.into(),
        std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
            .hash_one(key),
    )
}

/// How much the folder gives way in a narrow toolbar, compared with the device's 1: it shrinks
/// as far as it can first.
const FOLDER_SHRINK: f32 = 1000.;

/// A toolbar button that gives way in a narrow column.
fn shrinking(name: &'static str, shrink: f32, button: impl IntoElement) -> Div {
    div()
        .debug_selector(|| name.into())
        .flex_shrink(shrink)
        .child(button)
}

fn folder_menu(
    composer: WeakEntity<Composer>,
    cwd: PathBuf,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |mut menu, _, cx| {
        let home = cx.global::<AppConfig>().home.clone();
        let recent = cx
            .try_global::<RecentFolders>()
            .map(|recent| recent.0.clone())
            .unwrap_or_default();
        let mut folders = vec![cwd.clone()];
        // Folders are checked when the menu opens (not at startup): checking a folder under
        // Desktop or Documents can make macOS ask for access.
        folders.extend(
            recent
                .into_iter()
                .filter(|folder| *folder != cwd && folder.is_dir())
                .take(crate::config::RECENT_FOLDERS),
        );
        menu = menu
            .min_w(px(260.))
            .max_w(px(FOLDER_MENU_WIDTH))
            .label("Working folder");
        for folder in folders {
            let (name, parent) = folder_label(&folder, home.as_deref());
            let checked = folder == cwd;
            let composer = composer.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    // Long paths are cut with an ellipsis instead of overflowing the menu.
                    h_flex()
                        .w_full()
                        .max_w(px(FOLDER_MENU_WIDTH - 64.))
                        .min_w_0()
                        .gap_2()
                        .child(Icon::new(IconName::Folder).small().text_color(muted))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .truncate()
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(parent.clone()),
                        )
                })
                .checked(checked)
                .on_click(move |_, _, cx| {
                    let folder = folder.clone();
                    let _ = composer.update(cx, |this, cx| this.change_folder(folder, cx));
                }),
            );
        }
        let composer = composer.clone();
        menu.separator()
            .item(
                PopupMenuItem::new("Choose another folder…").on_click(move |_, _, cx| {
                    let _ = composer.update(cx, |this, cx| this.pick_folder(cx));
                }),
            )
    }
}

#[cfg(test)]
mod inline_tests {
    #[test]
    fn every_effort_has_its_place_on_the_slider() {
        use super::{Effort, effort_at, effort_value};
        for effort in Effort::ALL {
            assert_eq!(effort_at(effort_value(effort)), effort);
        }
    }

    #[test]
    fn dictation_shortcut_matches_exactly() {
        use super::{dictation_modifiers_held, dictation_shortcut, is_dictation_shortcut};
        use gpui_kit::{Keystroke, Modifiers};
        let shortcut = dictation_shortcut().clone();
        assert!(is_dictation_shortcut(&shortcut));
        let plain_space = Keystroke::parse("space").unwrap();
        assert!(!is_dictation_shortcut(&plain_space));
        let shift_space = Keystroke::parse("shift-space").unwrap();
        assert!(!is_dictation_shortcut(&shift_space));
        let extra = Keystroke {
            modifiers: Modifiers {
                alt: true,
                ..shortcut.modifiers
            },
            ..shortcut.clone()
        };
        assert!(!is_dictation_shortcut(&extra));

        assert!(dictation_modifiers_held(shortcut.modifiers));
        // Letting go of either modifier is a release.
        let no_shift = Modifiers {
            shift: false,
            ..shortcut.modifiers
        };
        assert!(!dictation_modifiers_held(no_shift));
        assert!(!dictation_modifiers_held(Modifiers::default()));
    }

    #[test]
    fn dictation_joins_the_draft() {
        use super::append_dictation;
        assert_eq!(append_dictation("", " Hello there. "), "Hello there.");
        assert_eq!(append_dictation("Fix", "the tests"), "Fix the tests");
        assert_eq!(append_dictation("Fix\n", "the tests"), "Fix\nthe tests");
        assert_eq!(append_dictation("Keep", "  "), "Keep");
    }
}

#[cfg(test)]
mod suggestion_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::component::Root;
    use gpui_kit::component::input::Enter;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use pi_sessions::Mode;

    use super::Composer;
    use crate::chat::session::PiSession;

    /// A composer in a window, over a Code session without a pi.
    fn composer(cx: &mut TestAppContext) -> (Entity<Composer>, &mut VisualTestContext) {
        composer_in(Mode::Code, cx)
    }

    fn composer_in(
        mode: Mode,
        cx: &mut TestAppContext,
    ) -> (Entity<Composer>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            super::init(cx);
            cx.set_global(crate::config::AppConfig::for_tests("pi".into()));
            cx.set_global(crate::settings::AppSettings::default());
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
            crate::stt::models::init(cx);
        });
        let mut composer = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|_| PiSession::idle(std::env::temp_dir(), mode));
            let view = cx.new(|cx| Composer::new(session, window, cx));
            composer = Some(view.clone());
            Root::new(view, window, cx)
        });
        let composer = composer.unwrap();
        cx.update(|window, cx| {
            let focus = composer.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        });
        (composer, cx)
    }

    /// Enter in the input. (A simulated keystroke would also type a newline first: the test
    /// platform inserts the text of a key its action lets through.)
    fn press_enter(cx: &mut VisualTestContext) {
        cx.dispatch_action(Enter {
            secondary: false,
            shift: false,
        });
        cx.run_until_parked();
    }

    fn text(composer: &Entity<Composer>, cx: &VisualTestContext) -> String {
        cx.read(|cx| composer.read(cx).input.read(cx).value().to_string())
    }

    fn rows(composer: &Entity<Composer>, cx: &VisualTestContext) -> Vec<String> {
        cx.read(|cx| {
            let rows = composer.read(cx).suggestion_rows(cx);
            rows.into_iter().map(|row| row.name).collect()
        })
    }

    #[gpui_kit::test]
    fn arrows_and_enter_complete_the_highlighted_command(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_input("/co");
        assert_eq!(rows(&composer, cx), ["compact", "copy"]);
        cx.simulate_keystrokes("down down up");
        press_enter(cx);
        // Completed, not sent; the caret is after it, ready for arguments.
        assert_eq!(text(&composer, cx), "/compact ");
        assert_eq!(rows(&composer, cx), Vec::<String>::new());
        cx.simulate_input("keep it");
        assert_eq!(text(&composer, cx), "/compact keep it");
    }

    #[gpui_kit::test]
    fn tab_completes_and_esc_closes_the_list(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_input("/th");
        cx.simulate_keystrokes("tab");
        assert_eq!(text(&composer, cx), "/thinking ");
        cx.update(|window, cx| {
            composer.update(cx, |composer, cx| composer.clear(window, cx));
        });
        cx.simulate_input("/comp");
        cx.simulate_keystrokes("escape");
        assert_eq!(rows(&composer, cx), Vec::<String>::new());
        assert_eq!(text(&composer, cx), "/comp");
        // It opens again for other text.
        cx.simulate_input("a");
        assert_eq!(rows(&composer, cx), ["compact"]);
    }

    #[gpui_kit::test]
    fn enter_runs_a_named_command_and_one_without_arguments(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        let opened = Rc::new(Cell::new(0));
        cx.update(|_, cx| {
            let opened = opened.clone();
            cx.on_action(move |_: &crate::actions::NewChat, _| opened.set(opened.get() + 1));
        });
        // Fully typed: no list, Enter runs it.
        cx.simulate_input("/new");
        assert_eq!(rows(&composer, cx), Vec::<String>::new());
        press_enter(cx);
        assert_eq!((text(&composer, cx).as_str(), opened.get()), ("", 1));
        // Its highlighted row: Enter runs it as well.
        cx.simulate_input("/ne");
        press_enter(cx);
        assert_eq!((text(&composer, cx).as_str(), opened.get()), ("", 2));
    }

    #[gpui_kit::test]
    fn the_whole_list_is_reachable_and_stays_in_view(cx: &mut TestAppContext) {
        let (composer, cx) = composer(cx);
        cx.simulate_input("/");
        let all = rows(&composer, cx);
        assert_eq!(all.len(), crate::chat::commands::SUPPORTED.len());
        for _ in 0..all.len() + 3 {
            cx.simulate_keystrokes("down");
        }
        cx.run_until_parked();
        cx.read(|cx| {
            let composer = composer.read(cx);
            let rows = composer.suggestion_rows(cx);
            assert_eq!(composer.suggestions.selected(&rows), rows.len() - 1);
            let scroll = &composer.suggestions_scroll;
            let last = scroll.bounds_for_item(rows.len() - 1).unwrap();
            let list = scroll.bounds();
            assert!(list.size.height < gpui_kit::px(320.), "{list:?}");
            assert!(
                last.bottom() + scroll.offset().y <= list.bottom() + gpui_kit::px(1.),
                "{last:?} {list:?} {:?}",
                scroll.offset()
            );
        });
        press_enter(cx);
        assert_eq!(text(&composer, cx), "/quit ");
    }

    /// `/login` asks the window for a terminal in a Code session, and says what to do in a chat.
    #[gpui_kit::test]
    fn login_leads_somewhere(cx: &mut TestAppContext) {
        use crate::actions::OpenLogin;

        let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
        for mode in [Mode::Code, Mode::Chat] {
            let (composer, cx) = composer_in(mode, cx);
            cx.update(|_, cx| {
                let opened = opened.clone();
                cx.on_action(move |login: &OpenLogin, _| opened.borrow_mut().push(login.clone()));
            });
            cx.simulate_input("/login anthropic");
            press_enter(cx);
            assert_eq!(text(&composer, cx), "");
            let notices = cx.read(|cx| {
                let session = composer.read(cx).session.read(cx);
                session.transcript().items().len()
            });
            assert_eq!(notices, usize::from(mode == Mode::Chat));
        }
        let login = OpenLogin {
            logout: false,
            provider: Some("anthropic".into()),
        };
        assert_eq!(*opened.borrow(), [login]);
    }
}

#[cfg(test)]
mod toolbar_tests {
    use std::path::PathBuf;

    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, Bounds, Pixels, TestAppContext, VisualTestContext, px, size};
    use pi_sessions::Mode;

    use super::{Composer, PiSession};

    /// The device's and the folder's button, in a window of `width` with the folder `name`.
    fn buttons<'a>(
        name: &str,
        width: Pixels,
        cx: &'a mut TestAppContext,
    ) -> ([Bounds<Pixels>; 2], &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(crate::config::AppConfig::for_tests("pi".into()));
            cx.set_global(crate::settings::AppSettings::default());
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
            crate::stt::models::init(cx);
        });
        let folder = PathBuf::from("/code").join(name);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|_| PiSession::idle(folder, Mode::Code));
            let view = cx.new(|cx| Composer::new(session, window, cx));
            Root::new(view, window, cx)
        });
        (at(width, cx), cx)
    }

    fn at(width: Pixels, cx: &mut VisualTestContext) -> [Bounds<Pixels>; 2] {
        cx.simulate_resize(size(width, px(400.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        ["composer-device", "composer-folder"].map(|name| cx.debug_bounds(name).unwrap())
    }

    /// A long folder name gives way to the model, then the device's does. Neither is squeezed
    /// to nothing.
    #[gpui_kit::test]
    fn a_long_folder_name_gives_way(cx: &mut TestAppContext) {
        let name = "spin-adapted-decomposition-search.retired-2026-09-24";
        let ([device, whole], cx) = buttons(name, px(2000.), cx);
        assert!(whole.left() >= device.right());

        // Room for the model, a shortened folder and the sandbox badge at its widest ("No
        // sandbox": the tests' app has none).
        let [same, less] = at(px(680.), cx);
        assert_eq!(
            same.size.width, device.size.width,
            "the device gave way first"
        );
        assert!(less.size.width < whole.size.width);
        let left = cx.debug_bounds("composer-left").unwrap();
        assert!(less.right() <= left.right(), "the folder is clipped");

        let [smaller, least] = at(px(300.), cx);
        assert!(smaller.size.width < device.size.width);
        assert!(least.size.width > px(56.), "the name is gone");
    }

    /// The effort slider is there in deep search only, and follows the session.
    #[gpui_kit::test]
    fn deep_search_has_an_effort_slider(cx: &mut TestAppContext) {
        use super::{Effort, WebMode, effort_at};

        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(crate::config::AppConfig::for_tests("pi".into()));
            cx.set_global(crate::settings::AppSettings::default());
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
            crate::stt::models::init(cx);
        });
        let session = cx.new(|_| PiSession::idle(PathBuf::from("/chat"), Mode::Chat));
        let shown = session.clone();
        let mut composer = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Composer::new(shown, window, cx));
            composer = Some(view.clone());
            Root::new(view, window, cx)
        });
        let composer = composer.unwrap();
        let mut show = |mode, effort| {
            session.update(cx, |session, cx| {
                session.set_web_for_tests(mode, effort, cx);
            });
            cx.run_until_parked();
            let at = cx.read(|cx| effort_at(composer.read(cx).effort.read(cx).value().end()));
            (cx.debug_bounds("web-effort").is_some(), at)
        };
        assert_eq!(
            show(WebMode::Search, Effort::Standard),
            (false, Effort::Standard)
        );
        assert_eq!(show(WebMode::Deep, Effort::Max), (true, Effort::Max));
        assert_eq!(show(WebMode::Deep, Effort::Light), (true, Effort::Light));
        assert!(!show(WebMode::Off, Effort::Light).0);

        // Moving it is not undone by what the session says meanwhile.
        show(WebMode::Deep, Effort::Light);
        let track = cx.debug_bounds("web-effort-track").unwrap();
        let end = gpui_kit::point(track.right() - px(2.), track.center().y);
        cx.simulate_mouse_down(
            end,
            gpui_kit::MouseButton::Left,
            gpui_kit::Modifiers::default(),
        );
        session.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let effort = |cx: &VisualTestContext| {
            effort_at(cx.read(|cx| composer.read(cx).effort.read(cx).value().end()))
        };
        assert_eq!(effort(cx), Effort::Max);
        cx.simulate_mouse_up(
            end,
            gpui_kit::MouseButton::Left,
            gpui_kit::Modifiers::default(),
        );

        // Every stop has as much room as the next one, the ends' partly beside the track.
        let bounds = |name, cx: &mut VisualTestContext| cx.debug_bounds(name).unwrap();
        let (first, last) = (
            bounds("web-effort-first", cx),
            bounds("web-effort-last", cx),
        );
        assert_eq!((first.right(), last.left()), (track.left(), track.right()));
        let stop = track.size.width / 3.;
        for (index, expected) in Effort::ALL.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // One of four.
            let left = first.left() + stop * index as f32;
            for x in [left + px(3.), left + stop - px(3.)] {
                cx.simulate_click(
                    gpui_kit::point(x, track.center().y),
                    gpui_kit::Modifiers::default(),
                );
                cx.run_until_parked();
                assert_eq!(effort(cx), expected, "{x:?}");
            }
        }
    }

    /// A short name is as wide as it is, in any window.
    #[gpui_kit::test]
    fn a_short_folder_name_stays_whole(cx: &mut TestAppContext) {
        let ([_, whole], cx) = buttons("app", px(2000.), cx);
        assert!(whole.size.width < px(120.));
        for width in [900., 520., 300.] {
            assert_eq!(at(px(width), cx)[1].size.width, whole.size.width, "{width}");
        }
    }
}

#[cfg(test)]
mod draft_tests {
    use std::path::{Path, PathBuf};

    use gpui_kit::component::{Root, WindowExt as _};
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, Window};
    use pi_rpc::protocol::ImageContent;
    use pi_sessions::Mode;

    use super::{
        AttachedDocument, Attachment, Composer, Dictation, Draft, InlineText, PiSession, Preview,
        prompt, waiting_line,
    };
    use crate::config::AppConfig;
    use crate::hosts::{Hosts, HostsHandle};

    /// `N` composers (of sessions without a pi) in one window.
    fn composers<const N: usize>(
        sessions: [PiSession; N],
        cx: &mut TestAppContext,
    ) -> ([Entity<Composer>; N], &mut VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(AppConfig::for_tests("pi".into()));
            cx.set_global(crate::settings::AppSettings::default());
            crate::stt::init(cx);
            let hosts = cx.new(|_| Hosts::for_tests(Vec::new()));
            cx.set_global(HostsHandle(hosts));
        });
        let mut built = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let composers = sessions.map(|session| {
                let session = cx.new(|_| session);
                cx.new(|cx| Composer::new(session, window, cx))
            });
            let first = composers[0].clone();
            built = Some(composers);
            Root::new(first, window, cx)
        });
        (built.unwrap(), cx)
    }

    fn code() -> PiSession {
        PiSession::idle(std::env::temp_dir(), Mode::Code)
    }

    fn on_host() -> PiSession {
        PiSession::idle_on_host(remote::HostId("box".into()))
    }

    fn file(path: &Path, inline: Option<&str>) -> Attachment {
        Attachment {
            order: 0,
            path: path.to_path_buf(),
            name: path.to_string_lossy().into_owned().into(),
            preview: Preview::None,
            image: None,
            inline: inline
                .map(|text| InlineText::Document(AttachedDocument::plain(path, text.to_owned()))),
            id: None,
        }
    }

    fn names(composer: &Entity<Composer>, cx: &VisualTestContext) -> Vec<String> {
        cx.read(|cx| {
            let attachments = &composer.read(cx).attachments;
            attachments.iter().map(|a| a.name.to_string()).collect()
        })
    }

    fn act<R>(
        composer: &Entity<Composer>,
        cx: &mut VisualTestContext,
        f: impl FnOnce(&mut Composer, &mut Window, &mut gpui_kit::Context<Composer>) -> R,
    ) -> R {
        let result = cx.update(|window, cx| composer.update(cx, |this, cx| f(this, window, cx)));
        cx.run_until_parked();
        result
    }

    /// Lets files be read on the background threads.
    fn settle(composer: &Entity<Composer>, cx: &mut VisualTestContext) {
        for _ in 0..500 {
            cx.run_until_parked();
            if cx.read(|cx| composer.read(cx).loading.is_empty()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the attachments were not read");
    }

    #[gpui_kit::test]
    fn a_draft_moves_with_its_attachments(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "text").unwrap();
        let ([from, to], cx) = composers([code(), code()], cx);
        assert!(act(&from, cx, |from, window, cx| from.take_draft(window, cx)).is_empty());
        assert!(!cx.read(|cx| from.read(cx).has_draft(cx)));

        let draft = act(&from, cx, |from, window, cx| {
            from.set_text("fix this ", window, cx);
            from.attach(Attachment {
                order: 0,
                path: "pasted.png".into(),
                name: "pasted.png".into(),
                preview: Preview::None,
                image: Some(ImageContent::new("AAAA", "image/png")),
                inline: None,
                id: None,
            });
            // Still being read when the draft is taken.
            from.attach_paths(vec![path.clone()], cx);
            assert!(from.has_draft(cx));
            from.take_draft(window, cx)
        });
        settle(&from, cx);
        assert!(!cx.read(|cx| from.read(cx).has_draft(cx)));
        assert_eq!(names(&from, cx), [""; 0]);
        assert_eq!(draft.unread, [path]);

        act(&to, cx, |to, window, cx| to.set_draft(draft, window, cx));
        settle(&to, cx);
        assert_eq!(names(&to, cx), ["pasted.png", "notes.txt"]);
        cx.read(|cx| {
            let to = to.read(cx);
            assert_eq!(to.text(cx), "fix this ");
            assert_eq!(to.input.read(cx).cursor(), "fix this ".len());
            assert!(to.attachments[0].image.is_some());
        });
    }

    #[gpui_kit::test]
    fn a_draft_goes_before_what_is_typed_meanwhile(cx: &mut TestAppContext) {
        let ([composer], cx) = composers([code()], cx);
        act(&composer, cx, |composer, window, cx| {
            composer.set_text("later", window, cx);
            composer.set_draft(Draft::for_tests("earlier"), window, cx);
        });
        assert_eq!(cx.read(|cx| composer.read(cx).text(cx)), "earlier\n\nlater");
    }

    /// What was heard so far is part of the draft.
    #[gpui_kit::test]
    fn a_draft_includes_the_dictation(cx: &mut TestAppContext) {
        let ([composer], cx) = composers([code()], cx);
        let draft = act(&composer, cx, |composer, window, cx| {
            composer.set_text("Fix", window, cx);
            composer.dictation = Some(Dictation {
                draft: "Fix".into(),
                transcript: "the tests".into(),
            });
            composer.take_draft(window, cx)
        });
        assert_eq!(draft.text(), "Fix the tests");
        assert!(!cx.read(|cx| composer.read(cx).has_draft(cx)));
    }

    /// Files are referenced or read as the machine the draft moves to needs them.
    #[gpui_kit::test]
    fn attachments_follow_to_another_machine(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "text").unwrap();
        let ([local, host], cx) = composers([code(), on_host()], cx);
        let draft = act(&local, cx, |local, window, cx| {
            local.attach(file(&path, None));
            local.take_draft(window, cx)
        });
        act(&host, cx, |host, window, cx| {
            host.set_draft(draft, window, cx);
        });
        settle(&host, cx);
        cx.read(|cx| {
            let attachment = &host.read(cx).attachments[0];
            assert!(
                matches!(&attachment.inline, Some(InlineText::Document(text)) if text.text() == "text")
            );
        });

        // Back, with a file picked on the host, which stays behind.
        let draft = act(&host, cx, |host, window, cx| {
            host.attach(file(&PathBuf::from("/srv/remote.txt"), None));
            host.take_draft(window, cx)
        });
        act(&local, cx, |local, window, cx| {
            local.set_draft(draft, window, cx);
        });
        settle(&local, cx);
        assert_eq!(names(&local, cx), ["notes.txt"]);
        assert!(cx.read(|cx| local.read(cx).attachments[0].inline.is_none()));
        assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 1);
    }

    /// A chat whose documents are read by `script` (the app's helper: JSON on stdin and stdout).
    #[cfg(unix)]
    fn chat<'a>(
        dir: &Path,
        script: &str,
        cx: &'a mut TestAppContext,
    ) -> (Entity<Composer>, &'a mut VisualTestContext) {
        use std::os::unix::fs::PermissionsExt as _;

        let exe = dir.join("app");
        std::fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let session = PiSession::idle(std::env::temp_dir(), Mode::Chat);
        let ([composer], cx) = composers([session], cx);
        cx.update(|_, cx| cx.global_mut::<AppConfig>().exe = Some(exe));
        (composer, cx)
    }

    #[cfg(unix)]
    fn remark(composer: &Entity<Composer>, cx: &VisualTestContext) -> Option<&'static str> {
        cx.read(|cx| {
            let composer = composer.read(cx);
            composer.attachments[0].remark(&composer.room(cx)).0
        })
    }

    /// A card says that a document is read, and Enter waits for it.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn a_document_is_read_before_it_is_sent(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("report.pdf");
        std::fs::write(&pdf, "%PDF-1.4").unwrap();
        let go = dir.path().join("go");
        let reply =
            r##"{"ok":true,"markdown":"# Hi","parser":"builtin","note":"Docling failed."}"##;
        let script = format!(
            "read line; while ! test -e '{}'; do sleep 0.05; done; echo '{reply}'",
            go.display()
        );
        let (composer, cx) = chat(dir.path(), &script, cx);
        act(&composer, cx, |composer, window, cx| {
            composer.set_text("What is this?", window, cx);
            composer.attach_paths(vec![pdf], cx);
            assert!(composer.is_reading() && composer.has_draft(cx));
        });
        assert!(cx.debug_bounds("reading-attachment").is_some());
        act(&composer, cx, |composer, window, cx| {
            composer.submit(false, window, cx);
        });
        assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 1);
        assert_eq!(cx.read(|cx| composer.read(cx).text(cx)), "What is this?");

        std::fs::write(go, "").unwrap();
        settle(&composer, cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("reading-attachment").is_none());
        assert_eq!(names(&composer, cx), ["report.pdf"]);
        assert_eq!(remark(&composer, cx), Some("Built-in reader"));
        cx.read(|cx| {
            let inline = &composer.read(cx).attachments[0].inline;
            let Some(InlineText::Document(document)) = inline else {
                panic!("not read");
            };
            assert_eq!(document.note(), Some("Docling failed."));
            let room = composer.read(cx).room(cx);
            assert_eq!(document.contents("What is this?", &room), "# Hi");
        });
    }

    /// Cards stay in the order they were attached in, whatever is read first, and show what is in
    /// a file while it is read.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn cards_keep_their_places(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("report.PDF");
        std::fs::write(
            &pdf,
            include_bytes!("../../../documents/src/check.pdf").as_slice(),
        )
        .unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "two\nlines").unwrap();
        let image = dir.path().join("photo.jpeg");
        std::fs::write(&image, "not really").unwrap();
        let go = dir.path().join("go");
        let reply = r##"{"ok":true,"markdown":"# Hi","parser":"builtin"}"##;
        let script = format!(
            "read line; while ! test -e '{}'; do sleep 0.05; done; echo '{reply}'",
            go.display()
        );
        let (composer, cx) = chat(dir.path(), &script, cx);
        cx.update(|_, cx| {
            cx.global_mut::<AppConfig>().documents_dir = dir.path().join("documents");
        });
        act(&composer, cx, |composer, _, cx| {
            composer.attach_paths(vec![pdf, text, image], cx);
            let reading: Vec<_> = composer.loading.iter().map(|l| l.card().status).collect();
            assert_eq!(reading, ["Reading…"; 3]);
        });
        for _ in 0..500 {
            cx.run_until_parked();
            let shown = cx.read(|cx| {
                let composer = composer.read(cx);
                let page = matches!(
                    composer.loading.first().map(|l| &l.preview),
                    Some(Preview::Page(_))
                );
                composer.attachments.len() == 2 && (page || !cfg!(target_os = "macos"))
            });
            if shown {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(names(&composer, cx), ["notes.txt", "photo.jpeg"]);
        cx.read(|cx| {
            let reading = &composer.read(cx).loading[0];
            assert_eq!(reading.card().extension.as_deref(), Some(".pdf"));
            assert_eq!(
                matches!(reading.preview, Preview::Page(_)),
                cfg!(target_os = "macos")
            );
        });

        std::fs::write(go, "").unwrap();
        settle(&composer, cx);
        assert_eq!(
            names(&composer, cx),
            ["report.PDF", "notes.txt", "photo.jpeg"]
        );
        cx.read(|cx| {
            let mut room = composer.read(cx).room(cx);
            let attachments = &composer.read(cx).attachments;
            let cards: Vec<_> = attachments
                .iter()
                .map(|attachment| {
                    let card = attachment.card(&mut room);
                    (card.extension.unwrap().to_string(), card.status.to_string())
                })
                .collect();
            let cards: Vec<_> = cards
                .iter()
                .map(|(extension, status)| (extension.as_str(), status.as_str()))
                .collect();
            assert_eq!(
                cards,
                [(".pdf", "PDF"), (".txt", "Text"), (".jpg", "Image")]
            );
            let shown: Vec<_> = attachments
                .iter()
                .map(|a| a.card(&mut room).can_show)
                .collect();
            assert_eq!(shown, [true, true, true]);
            assert!(matches!(&attachments[1].preview, Preview::Text(text) if text == "two\nlines"));
        });
        // Clicking the picture of the text or the PDF, with nothing else after it, shows it.
        for left in [2, 1] {
            act(&composer, cx, |composer, window, cx| {
                window.close_all_dialogs(cx);
                composer.attachments.truncate(left);
                cx.notify();
            });
            assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
            let picture = cx.debug_bounds("attachment-picture").unwrap();
            cx.simulate_click(picture.center(), gpui_kit::Modifiers::none());
            assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        }
    }

    /// The text of attached files goes along with a message, and comes back as files with it.
    #[gpui_kit::test]
    fn attached_files_come_and_go_with_their_message(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "two\nlines").unwrap();
        let session = PiSession::idle(std::env::temp_dir(), Mode::Chat);
        let ([composer], cx) = composers([session], cx);
        cx.update(|_, cx| {
            cx.global_mut::<AppConfig>().documents_dir = dir.path().join("documents");
        });
        act(&composer, cx, |composer, _, cx| {
            composer.attach_paths(vec![path.clone()], cx);
        });
        settle(&composer, cx);
        let send = |cx: &mut VisualTestContext| {
            act(&composer, cx, |composer, window, cx| {
                composer.submit(false, window, cx);
                let session = composer.session.read(cx);
                let (text, _, _) = session.pending_prompts().last().unwrap();
                text.to_owned()
            })
        };
        act(&composer, cx, |composer, window, cx| {
            composer.set_text("What is this?", window, cx);
        });
        let sent = send(cx);
        assert_eq!(names(&composer, cx), [""; 0]);
        let prompt = prompt::split(&sent);
        assert_eq!(prompt.text, "What is this?");
        let (file, block) = &prompt.files[0];
        assert_eq!(file.name, "notes.txt");
        assert!(block.contains("\ntwo\nlines\n"), "{block}");
        assert!(!sent.contains("reader") && !sent.contains(&*path.to_string_lossy()));
        let id = file.id.as_deref().unwrap();
        let documents = dir.path().join("documents");
        assert_eq!(documents::attached::original(&documents, id), Some(path));
        assert_eq!(waiting_line(&sent, 0), "What is this? [+1 file(s)]");
        assert_eq!(waiting_line(block, 2), "[1 file(s), 2 image(s)]");
        assert_eq!(waiting_line("hi", 0), "hi");

        // Given back, with what pi adds about a picture, and sent again.
        act(&composer, cx, |composer, window, cx| {
            let noted = format!("{sent}\n\n[Image: original 9x9, displayed at 3x3. Multiply…]");
            composer.restore(&noted, Vec::new(), true, window, cx);
        });
        assert_eq!(cx.read(|cx| composer.read(cx).text(cx)), "What is this?");
        assert_eq!(names(&composer, cx), ["notes.txt"]);
        cx.read(|cx| {
            let preview = &composer.read(cx).attachments[0].preview;
            assert!(matches!(preview, Preview::Text(text) if text == "two\nlines"));
        });
        let draft = act(&composer, cx, |composer, window, cx| {
            composer.take_draft(window, cx)
        });
        act(&composer, cx, |composer, window, cx| {
            composer.set_draft(draft, window, cx);
        });
        assert_eq!(send(cx), sent);
    }

    /// A text that is too long says that it is shortened, and is, unless the settings allow more.
    #[gpui_kit::test]
    fn long_text_is_shortened(cx: &mut TestAppContext) {
        use std::fmt::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let mut log = String::new();
        for n in 0..20_000 {
            let _ = writeln!(log, "{n} something happened");
        }
        std::fs::write(&path, &log).unwrap();
        let session = PiSession::idle(std::env::temp_dir(), Mode::Chat);
        let ([composer], cx) = composers([session], cx);
        cx.update(|_, cx| {
            cx.global_mut::<AppConfig>().documents_dir = dir.path().join("documents");
        });
        for (limit, short) in [(Some(256 * 1024), true), (Some(1024 * 1024), false)] {
            act(&composer, cx, |composer, window, cx| {
                composer.set_text("What about 12345?", window, cx);
                composer.attach_paths(vec![path.clone()], cx);
                let parsers = crate::document_parsers::document_parsers(cx);
                parsers.update(cx, |parsers, cx| parsers.set_whole_text(limit, cx));
            });
            settle(&composer, cx);
            let status = cx.read(|cx| {
                let composer = composer.read(cx);
                composer.attachments[0].card(&mut composer.room(cx)).status
            });
            assert_eq!(status, if short { "Shortened" } else { "Text" });
            let sent = act(&composer, cx, |composer, window, cx| {
                composer.submit(false, window, cx);
                let session = composer.session.read(cx);
                session.pending_prompts().last().unwrap().0.to_owned()
            });
            assert!(sent.contains("\n12345 something happened\n"));
            assert_eq!(sent.len() > log.len(), !short, "{}", sent.len());
        }
    }

    /// Pictures that models do not take are converted; the source of a drawing is for Code.
    #[gpui_kit::test]
    fn other_pictures_are_converted(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let svg = dir.path().join("a.svg");
        let drawing = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"9\" height=\"9\"/>";
        std::fs::write(&svg, drawing).unwrap();
        let bmp = dir.path().join("b.bmp");
        image::RgbaImage::new(2, 2).save(&bmp).unwrap();
        let broken = dir.path().join("c.svg");
        std::fs::write(&broken, "<svg>not closed").unwrap();
        let chat = PiSession::idle(std::env::temp_dir(), Mode::Chat);
        let ([chat, code], cx) = composers([chat, code()], cx);
        let mime = |composer: &Entity<Composer>, cx: &mut VisualTestContext| {
            act(composer, cx, |composer, _, cx| {
                composer.attach_paths(vec![svg.clone(), bmp.clone(), broken.clone()], cx);
            });
            settle(composer, cx);
            cx.read(|cx| {
                let attachments = &composer.read(cx).attachments;
                let image = |a: &Attachment| a.image.as_ref().map(|i| i.mime_type.clone());
                attachments.iter().map(image).collect::<Vec<_>>()
            })
        };
        let png = || Some("image/png".to_owned());
        assert_eq!(mime(&chat, cx), [png(), png(), None]);
        assert_eq!(mime(&code, cx), [None, png(), None]);
        cx.read(|cx| {
            // What cannot be drawn is still text.
            let attachment = &chat.read(cx).attachments[2];
            assert!(matches!(&attachment.inline, Some(InlineText::Document(_))));
            assert!(code.read(cx).attachments[0].inline.is_none());
        });
    }

    /// Removing a document, or leaving the chat, stops reading it.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn reading_a_document_stops_with_its_card(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("report.pdf");
        std::fs::write(&pdf, "%PDF-1.4").unwrap();
        let stopped = dir.path().join("stopped");
        // Ends with its input, like the real one.
        let script = format!("cat > /dev/null; echo >> '{}'", stopped.display());
        let (composer, cx) = chat(dir.path(), &script, cx);
        for leave in [false, true] {
            act(&composer, cx, |composer, _, cx| {
                composer.attach_paths(vec![pdf.clone()], cx);
            });
            // Until the helper runs.
            std::thread::sleep(std::time::Duration::from_millis(300));
            act(&composer, cx, |composer, window, cx| {
                if leave {
                    assert_eq!(
                        composer.take_draft(window, cx).unread,
                        std::slice::from_ref(&pdf)
                    );
                } else {
                    composer.loading.clear();
                }
            });
            assert!(!cx.read(|cx| composer.read(cx).has_draft(cx)));
            for _ in 0..500 {
                let lines = std::fs::read_to_string(&stopped).unwrap_or_default();
                if lines.lines().count() > usize::from(leave) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let lines = std::fs::read_to_string(&stopped).unwrap();
            assert_eq!(lines.lines().count(), 1 + usize::from(leave));
            cx.run_until_parked();
            assert_eq!(names(&composer, cx), [""; 0]);
        }
    }

    /// A document that cannot be read says why, and one in a Code session is only named.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn documents_that_are_not_read(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let docx = dir.path().join("letter.docx");
        std::fs::write(&docx, "PK").unwrap();
        let script = r#"echo '{"ok":false,"error":"the file is odd"}'"#;
        let (composer, cx) = chat(dir.path(), script, cx);
        act(&composer, cx, |composer, _, cx| {
            composer.attach_paths(vec![docx], cx);
        });
        settle(&composer, cx);
        assert_eq!(remark(&composer, cx), Some("Not included"));
        cx.read(|cx| {
            let composer = composer.read(cx);
            let (_, why) = composer.attachments[0].remark(&composer.room(cx));
            assert_eq!(why.unwrap(), "It is not included: the file is odd.");
        });
    }

    #[gpui_kit::test]
    fn a_document_in_code_is_a_path(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("report.pdf");
        std::fs::write(&pdf, "%PDF-1.4").unwrap();
        let ([composer], cx) = composers([code()], cx);
        act(&composer, cx, |composer, _, cx| {
            composer.attach_paths(vec![pdf], cx);
            assert!(composer.loading[0]._document.is_none());
        });
        settle(&composer, cx);
        assert!(cx.read(|cx| composer.read(cx).attachments[0].inline.is_none()));
    }
}
