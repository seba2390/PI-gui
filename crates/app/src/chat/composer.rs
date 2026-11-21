//! The composer: multi-line input plus the bottom toolbar (attachments, working
//! folder, model, thinking level, context-window wheel, send/stop).
//!
//! Key behaviour mirrors pi's terminal: Enter submits (steers while the agent
//! works), Shift+Enter inserts a newline, Alt+Enter queues a follow-up, Esc
//! interrupts, Alt+Up restores queued messages, Shift+Tab cycles thinking.

use std::fmt::Write as _;
use std::path::PathBuf;

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::ImageContent;

use super::commands::{self, CommandOutcome};
use super::session::{Lifecycle, PiSession};
use crate::theme::palette;
use crate::ui::AppIcon;

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
}

/// A file attached to the next prompt.
struct Attachment {
    path: PathBuf,
    name: SharedString,
    /// Images are sent as image content; other files as `@path` references,
    /// which is how pi's terminal refers to files.
    image: Option<ImageContent>,
}

pub struct Composer {
    session: Entity<PiSession>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(session: Entity<PiSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .submit_on_enter(true)
                .placeholder("Message pi…  (/ for commands, ! to run a shell command)")
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(false, window, cx);
                }
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
        ];
        Self {
            session,
            input,
            attachments: Vec::new(),
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
            match attachment.image {
                Some(image) => images.push(image),
                None => {
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
            format!("{}\n\n{text}", references.join(" "))
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
        cx.spawn(async move |this, cx| {
            let attachments = cx
                .background_executor()
                .spawn(async move { paths.into_iter().map(load_attachment).collect::<Vec<_>>() })
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
                let _ = this.update(cx, |_, cx| cx.emit(ComposerEvent::ChangeFolder(folder)));
            }
        })
        .detach();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let running = session.is_running();
        let ready = matches!(session.lifecycle(), Lifecycle::Ready | Lifecycle::Starting);
        let has_input =
            !self.input.read(cx).value().trim().is_empty() || !self.attachments.is_empty();
        let folder = session.cwd().file_name().map_or_else(
            || session.cwd().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let folder_locked = !session.is_empty();

        let left = h_flex()
            .gap_1()
            .child(
                Button::new("attach")
                    .ghost()
                    .small()
                    .icon(Icon::from(AppIcon::Paperclip))
                    .tooltip("Attach files")
                    .on_click(cx.listener(|this, _, _, cx| this.pick_attachments(cx))),
            )
            .child(
                Button::new("folder")
                    .ghost()
                    .small()
                    .icon(IconName::Folder)
                    .label(folder)
                    .tooltip(if folder_locked {
                        "Working folder (choosing another starts a new chat)"
                    } else {
                        "Choose working folder"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
            );

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
        Button::new("model-picker")
            .ghost()
            .small()
            .label(label)
            .dropdown_caret(true)
            .disabled(models.is_empty())
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
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
                menu
            })
    }

    fn render_thinking_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let levels = session.thinking_levels().to_vec();
        let current = session.thinking_level();
        let reasoning = session.model().is_some_and(|model| model.reasoning);
        let handle = self.session.clone();
        Button::new("thinking-picker")
            .ghost()
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
        div()
            .id("context-ring")
            .px_1()
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(
                #[allow(clippy::cast_possible_truncation)]
                ProgressCircle::new("context-progress")
                    .value(percent as f32)
                    .color(color)
                    .size_4(),
            )
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
                            .ghost()
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

fn load_attachment(path: PathBuf) -> Attachment {
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
    Attachment { path, name, image }
}
