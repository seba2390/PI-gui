//! The composer: multi-line input plus the bottom toolbar (attachments, working
//! folder, model, thinking level, context-window wheel, send/stop).
//!
//! Key behaviour mirrors pi's terminal: Enter submits (steers while the agent
//! works), Shift+Enter inserts a newline, Alt+Enter queues a follow-up, Esc
//! interrupts, Alt+Up restores queued messages, Shift+Tab cycles thinking.

use pi_sessions::Mode;
use remote::HostId;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::list::{List, ListState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::ImageContent;

use super::commands::{self, CommandOutcome};
use super::file_picker::{self, FilePicker, Pick};
use super::session::{Lifecycle, PiSession};
use crate::config::{AppConfig, RecentFolders};
use crate::theme::palette;
use crate::ui::AppIcon;
use crate::ui::QuietButton as _;

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
    /// Open the "Add model" dialog (on its form, or on the models.json editor).
    AddModel {
        edit_json: bool,
    },
}

/// A file attached to the next prompt.
struct Attachment {
    path: PathBuf,
    name: SharedString,
    /// Images are sent as image content; other files as `@path` references,
    /// which is how pi's terminal refers to files.
    image: Option<ImageContent>,
    /// In chat mode the model has no `read` tool, so text files are pasted into the prompt.
    inline: Option<InlineText>,
}

/// A text attachment's contents for chat mode, or why it could not be included.
enum InlineText {
    Text(String),
    Skipped(&'static str),
}

/// Text attachments larger than this are not pasted into a chat prompt.
const MAX_INLINE_BYTES: u64 = 256 * 1024;

pub struct Composer {
    session: Entity<PiSession>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    /// The attach popover's file list (searchable, indexed in the background).
    picker: Entity<ListState<FilePicker>>,
    picker_open: bool,
    /// The context breakdown popover is open.
    context_open: bool,
    _subscriptions: Vec<Subscription>,
}

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
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(false, window, cx);
                }
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
            // Repaint the device button's status dot.
            cx.observe(&crate::hosts::hosts(cx), |_, _, cx| cx.notify()),
        ];
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
            picker,
            picker_open: false,
            context_open: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
        let focus = self.focus_handle(cx);
        focus.focus(window, cx);
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

        let cwd = self.session.read(cx).cwd().to_owned();
        let mut references = Vec::new();
        let mut images = Vec::new();
        for attachment in self.attachments.drain(..) {
            match (attachment.image, attachment.inline) {
                (Some(image), _) => images.push(image),
                (None, Some(InlineText::Text(text))) => {
                    references.push(inline_file_block(&attachment.name, &text));
                }
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
                    references.push(format!("@{}", path.display()));
                }
            }
        }
        let message = if references.is_empty() {
            text
        } else {
            // Pasted files (chat mode) each get their own paragraph; `@path`s share a line.
            let separator = if self.session.read(cx).is_local_code() {
                " "
            } else {
                "\n\n"
            };
            format!("{}\n\n{text}", references.join(separator))
                .trim()
                .to_owned()
        };

        self.session.update(cx, |session, cx| {
            session.submit(message, images, follow_up, cx);
        });
        self.clear(window, cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.attachments.clear();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn send_follow_up(&mut self, _: &SendFollowUp, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(true, window, cx);
    }

    fn interrupt(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).is_running() {
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
                self.attachments.push(Attachment {
                    path,
                    name: name.into(),
                    image: None,
                    inline: None,
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
                    self.attachments.push(Attachment {
                        path: PathBuf::from(&name),
                        name: name.into(),
                        image: Some(ImageContent::new(
                            base64::engine::general_purpose::STANDARD.encode(&image.bytes),
                            mime,
                        )),
                        inline: None,
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

    fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Without local file access (chat, or pi on another machine) text files are pasted in.
        let inline = !self.session.read(cx).is_local_code();
        cx.spawn(async move |this, cx| {
            let attachments = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| load_attachment(path, inline))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.attachments.extend(attachments);
                cx.notify();
            });
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
            // No project folder: attach straight from the system dialog, no folder picker.
            let left = h_flex().gap_1().child(
                Button::new("attach")
                    .quiet(cx)
                    .small()
                    .icon(Icon::from(AppIcon::Paperclip))
                    .tooltip("Attach files or images")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_attachments(cx))),
            );
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
                .child(
                    Button::new("remote-folder")
                        .quiet(cx)
                        .small()
                        .icon(IconName::Folder)
                        .label(folder)
                        .tooltip(format!(
                            "{} (choosing another starts a new session)",
                            cwd.display()
                        ))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ComposerEvent::BrowseRemote(host.clone()));
                        })),
                );
            return self.toolbar_layout(left, cx).into_any_element();
        }

        let left = h_flex()
            .gap_1()
            .child(self.render_attach_popover(cx))
            .child(self.render_device(None, cx))
            .child(
                Button::new(menu_id(
                    "folder",
                    &(&cwd, cx.try_global::<RecentFolders>().map(|r| r.0.clone())),
                ))
                .quiet(cx)
                .small()
                .icon(IconName::Folder)
                .label(folder)
                .tooltip(if folder_locked {
                    "Working folder (choosing another starts a new chat)"
                } else {
                    "Choose working folder"
                })
                .dropdown_menu_with_anchor(Anchor::BottomLeft, folder_menu(cx.weak_entity(), cwd)),
            );

        self.toolbar_layout(left, cx).into_any_element()
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
        Button::new(menu_id("device", &(current.map(|id| id.0.clone()), key)))
            .quiet(cx)
            .small()
            .child(
                h_flex()
                    .gap_1p5()
                    .child(crate::remote_ui::status_dot(status.as_ref(), cx))
                    .child(label),
            )
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(
                Anchor::BottomLeft,
                device_menu(cx.weak_entity(), current.cloned()),
            )
    }

    /// The composer's bottom bar: `left` controls, then model, thinking, context and send.
    fn toolbar_layout(&self, left: Div, cx: &mut Context<Self>) -> Div {
        let session = self.session.read(cx);
        let running = session.is_running();
        let ready = matches!(session.lifecycle(), Lifecycle::Ready | Lifecycle::Starting);
        let has_input =
            !self.input.read(cx).value().trim().is_empty() || !self.attachments.is_empty();
        let right = h_flex()
            .gap_1()
            .child(self.render_model_picker(cx))
            .child(self.render_thinking_picker(cx))
            .child(self.render_context_ring(cx))
            .child(if running && !has_input {
                Button::new("stop")
                    .primary()
                    .small()
                    .icon(IconName::Square)
                    .rounded(cx.theme().radius_full())
                    .tooltip("Interrupt (esc)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.interrupt(&Interrupt, window, cx)),
                    )
            } else {
                Button::new("send")
                    .primary()
                    .small()
                    .icon(IconName::ArrowUp)
                    .rounded(cx.theme().radius_full())
                    .disabled(!has_input || !ready)
                    .tooltip(if running {
                        "Steer (enter) · follow-up (alt+enter)"
                    } else {
                        "Send (enter)"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.submit(false, window, cx)))
            });

        h_flex()
            .w_full()
            .justify_between()
            .px_2()
            .pb_2()
            .child(left)
            .child(right)
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let models = session.models().to_vec();
        let current = session.model().map(pi_rpc::protocol::Model::qualified_id);
        let label = session.model().map_or_else(
            || "Model".to_owned(),
            |model| model.display_name().to_owned(),
        );
        let handle = self.session.clone();
        let composer = cx.weak_entity();
        Button::new(menu_id("model-picker", &(&current, models.len())))
            .quiet(cx)
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                if models.is_empty() {
                    menu = menu.label("No models available yet");
                }
                for model in &models {
                    let (handle, model) = (handle.clone(), model.clone());
                    let checked = current.as_deref() == Some(model.qualified_id().as_str());
                    menu = menu.item(
                        PopupMenuItem::new(format!(
                            "{}  ·  {}",
                            model.display_name(),
                            model.provider
                        ))
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            handle.update(cx, |session, cx| session.set_model(&model, cx));
                        }),
                    );
                }
                let (add, edit) = (composer.clone(), composer.clone());
                menu.separator()
                    .item(
                        PopupMenuItem::new("Add model…")
                            .icon(IconName::Plus)
                            .on_click(move |_, _, cx| {
                                let _ = add.update(cx, |_, cx| {
                                    cx.emit(ComposerEvent::AddModel { edit_json: false });
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Edit models.json…")
                            .icon(IconName::File)
                            .on_click(move |_, _, cx| {
                                let _ = edit.update(cx, |_, cx| {
                                    cx.emit(ComposerEvent::AddModel { edit_json: true });
                                });
                            }),
                    )
            })
    }

    fn render_thinking_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let levels = session.thinking_levels().to_vec();
        let current = session.thinking_level();
        let reasoning = session.model().is_some_and(|model| model.reasoning);
        let handle = self.session.clone();
        Button::new(menu_id("thinking-picker", &(&levels, current)))
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
                        PopupMenuItem::new(level.to_string())
                            .checked(current == Some(level))
                            .on_click(move |_, _, cx| {
                                handle.update(cx, |session, cx| {
                                    session.set_thinking_level(level, cx);
                                });
                            }),
                    );
                }
                menu
            })
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
        Popover::new("context-popover")
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
                    .tooltip(tooltip)
                    .tooltip_show_delay(Duration::ZERO),
            )
            .content(move |_, _, cx| match session.upgrade() {
                Some(session) => super::context_panel::render(session.read(cx), cx),
                None => div().into_any_element(),
            })
    }

    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let chips = self
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                h_flex()
                    .gap_1()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .text_xs()
                    .child(if attachment.image.is_some() {
                        Icon::from(AppIcon::Image).xsmall()
                    } else {
                        Icon::new(IconName::File).xsmall()
                    })
                    .child(attachment.name.clone())
                    .child(
                        Button::new(("remove-attachment", index))
                            .quiet(cx)
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if index < this.attachments.len() {
                                    this.attachments.remove(index);
                                    cx.notify();
                                }
                            })),
                    )
            });
        h_flex().flex_wrap().gap_1().px_3().pt_2().children(chips)
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let text = self.input.read(cx).value().to_string();
        let found = commands::suggestions(&text, self.session.read(cx).commands());
        if found.is_empty()
            || found
                .iter()
                .any(|suggestion| text == format!("/{}", suggestion.name))
        {
            return None;
        }
        let (muted, hover) = (cx.theme().muted_foreground, cx.theme().secondary_hover);
        let rows = found.into_iter().enumerate().map(|(index, suggestion)| {
            let completion = format!("/{} ", suggestion.name);
            h_flex()
                .id(("suggestion", index))
                .gap_3()
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_text(&completion, window, cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_sm()
                        .child(format!("/{}", suggestion.name)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(suggestion.description),
                )
        });
        Some(v_flex().px_1().pt_1().gap_0p5().children(rows))
    }

    fn complete_first(&mut self, _: &Complete, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        match commands::suggestions(&text, self.session.read(cx).commands()).first() {
            Some(first) => {
                let completion = format!("/{} ", first.name);
                self.set_text(&completion, window, cx);
            }
            None => cx.propagate(),
        }
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let statuses = self.session.read(cx).extension_status();
        if statuses.is_empty() {
            return None;
        }
        let text = statuses.values().cloned().collect::<Vec<_>>().join("  ·  ");
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
        if queue.is_empty() && widgets.is_empty() {
            return None;
        }
        let muted = cx.theme().muted_foreground;
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
                    .child(div().truncate().child(text))
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
        let colors = palette(cx);
        let queue = self.render_queue(cx);
        let suggestions = self.render_suggestions(cx);
        let status = self.render_status(cx);
        let has_attachments = !self.attachments.is_empty();
        v_flex()
            .key_context(COMPOSER_CONTEXT)
            .on_action(cx.listener(Self::send_follow_up))
            .on_action(cx.listener(Self::interrupt))
            .on_action(cx.listener(Self::dequeue))
            .on_action(cx.listener(Self::cycle_thinking))
            .on_action(cx.listener(Self::complete_first))
            .w_full()
            .rounded_xl()
            .border_1()
            .border_color(cx.theme().input)
            .bg(colors.surface)
            .shadow_sm()
            .children(suggestions)
            .children(queue)
            .when(has_attachments, |this| {
                this.child(self.render_attachments(cx))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(paths.paths().to_vec(), cx);
            }))
            .child(div().px_3().pt_3().pb_1().child({
                let weak = cx.weak_entity();
                Textarea::new(&self.input)
                    .appearance(false)
                    .on_paste(move |item, _, cx| {
                        weak.update(cx, |this, cx| this.paste(item, cx))
                            .unwrap_or(false)
                    })
            }))
            .child(self.render_toolbar(cx))
            .children(status)
    }
}

fn load_attachment(path: PathBuf, inline: bool) -> Attachment {
    let name: SharedString = path
        .file_name()
        .map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into();
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    };
    let image = mime.and_then(|mime| {
        let bytes = std::fs::read(&path).ok()?;
        Some(ImageContent::new(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            mime,
        ))
    });
    let inline = (inline && image.is_none()).then(|| read_inline_text(&path));
    Attachment {
        path,
        name,
        image,
        inline,
    }
}

fn read_inline_text(path: &std::path::Path) -> InlineText {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_INLINE_BYTES => InlineText::Skipped("larger than 256 KB"),
        Ok(_) => match std::fs::read(path).map(String::from_utf8) {
            Ok(Ok(text)) => InlineText::Text(text),
            Ok(Err(_)) => InlineText::Skipped("not a text file"),
            Err(_) => InlineText::Skipped("could not be read"),
        },
        Err(_) => InlineText::Skipped("could not be read"),
    }
}

/// A file pasted into a chat prompt: its name, then its contents in a fenced block whose
/// fence is longer than any backtick run inside, so the contents can't close it early.
fn inline_file_block(name: &str, contents: &str) -> String {
    let longest = contents
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let body = contents.strip_suffix('\n').unwrap_or(contents);
    format!("`{name}`:\n{fence}\n{body}\n{fence}")
}

/// Builds the folder menu: recent working folders (instant, no disk access) and a fallback to
/// the system folder dialog, which macOS takes a noticeable moment to open.
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
fn folder_label(
    folder: &std::path::Path,
    home: Option<&std::path::Path>,
) -> (SharedString, SharedString) {
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
fn menu_id(name: &'static str, key: &impl std::hash::Hash) -> ElementId {
    use std::hash::BuildHasher as _;

    ElementId::NamedInteger(
        name.into(),
        std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
            .hash_one(key),
    )
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
    use super::inline_file_block;

    #[test]
    fn inlined_files_use_a_fence_longer_than_their_backticks() {
        assert_eq!(inline_file_block("a.txt", "hi\n"), "`a.txt`:\n```\nhi\n```");
        assert_eq!(
            inline_file_block("b.md", "```rust\nx\n```"),
            "`b.md`:\n````\n```rust\nx\n```\n````"
        );
    }
}
