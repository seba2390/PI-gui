//! The main window: sidebar | chat (+ terminal below) | right panel.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, TitleBar, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use std::collections::HashMap;

use pi_sessions::Mode;

use crate::actions::{
    FocusComposer, NewChat, ShowChatMode, ShowCodeMode, ShowDiff, ShowFiles, ToggleRightPanel,
    ToggleSidebar, ToggleTerminal, WINDOW_CONTEXT,
};
use crate::chat::{ChatEvent, ChatView};
use crate::config::AppConfig;
use crate::panels::{RightPanel, RightPanelEvent, RightTab, TerminalPanel, TerminalPanelEvent};
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::ui::QuietButton as _;
use crate::ui::{AppIcon, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, WindowDrag};

pub fn open_main_window(cx: &mut App) {
    crate::chat::init(cx);
    crate::panels::terminal_panel::init(cx);
    let bounds = Bounds::centered(None, size(px(1320.), px(860.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(560.), px(420.))),
        app_id: Some("pi-gui".into()),
        ..TitleBar::window_options()
    };
    let opened = open_window(options, cx, |window, cx| {
        crate::theme::sync_with_window(window, cx);
        cx.new(|cx| MainWindow::new(window, cx))
    });
    if let Err(error) = opened {
        log::error!("failed to open the main window: {error:#}");
    }
}

/// Layout of a cached child view: views only re-render when they notify, so streaming
/// in the chat never re-renders the sidebar, panels or terminal (and vice versa).
fn full() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

/// Moves keyboard focus to `chat`'s composer.
fn focus_chat(chat: &Entity<ChatView>, window: &mut Window, cx: &mut App) {
    let focus = chat.read(cx).composer_focus(cx);
    focus.focus(window, cx);
}

pub struct MainWindow {
    sidebar: Entity<Sidebar>,
    chat: Entity<ChatView>,
    /// Chats switched away from while their agent was still working; kept alive
    /// until they settle so a run is never killed by navigation.
    background: Vec<(Entity<ChatView>, Subscription)>,
    right_panel: Entity<RightPanel>,
    terminal: Option<Entity<TerminalPanel>>,
    /// The conversation last shown in each mode, reopened when switching back to it.
    last_session: HashMap<Mode, PathBuf>,
    sidebar_open: bool,
    right_open: bool,
    terminal_open: bool,
    drag: WindowDrag,
    focus_handle: FocusHandle,
    chat_subscription: Subscription,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let cwd = {
            let config = cx.global::<AppConfig>();
            match sidebar.read(cx).mode() {
                Mode::Chat => config.chat_dir.clone(),
                Mode::Code => config.default_cwd.clone(),
            }
        };
        let chat = cx.new(|cx| ChatView::new(cwd.clone(), None, window, cx));
        let right_panel = cx.new(|cx| RightPanel::new(cwd, window, cx));
        let chat_subscription = cx.subscribe_in(&chat, window, Self::on_chat_event);
        let subscriptions = vec![
            cx.subscribe_in(&sidebar, window, Self::on_sidebar_event),
            cx.subscribe_in(
                &right_panel,
                window,
                |this, _, event: &RightPanelEvent, window, cx| {
                    let RightPanelEvent::InsertPath(path) = event;
                    let mention = format!("@{} ", path.display());
                    let chat = this.chat.clone();
                    chat.read(cx).composer().clone().update(cx, |composer, cx| {
                        composer.insert_text(&mention, window, cx);
                    });
                },
            ),
            cx.observe_window_appearance(window, |_, window, cx| {
                crate::theme::sync_with_window(window, cx);
            }),
        ];
        let focus_handle = cx.focus_handle();
        let chat_for_focus = chat.clone();
        let open_at_startup = cx.global::<AppConfig>().open_at_startup.clone();
        window.defer(cx, move |window, cx| {
            focus_chat(&chat_for_focus, window, cx);
        });
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = this.update(cx, |this, cx| {
                for panel in &open_at_startup {
                    match panel.as_str() {
                        "terminal" => this.toggle_terminal(&ToggleTerminal, window, cx),
                        "diff" => this.show_diff(&ShowDiff, window, cx),
                        "files" => this.show_files(&ShowFiles, window, cx),
                        "no-sidebar" => this.toggle_sidebar(&ToggleSidebar, window, cx),
                        "chat" => this.switch_to_mode(Mode::Chat, window, cx),
                        "code" => this.switch_to_mode(Mode::Code, window, cx),
                        "latest" => {
                            let dir = cx.global::<AppConfig>().sessions_dir.clone();
                            if let Some(latest) =
                                dir.and_then(|dir| pi_sessions::scan(&dir).into_iter().next())
                            {
                                drop(this.open_session(&latest, window, cx));
                            }
                        }
                        "context" => {
                            let composer = this.chat.read(cx).composer().clone();
                            composer.update(cx, |composer, cx| composer.open_context_breakdown(cx));
                        }
                        "attach" => {
                            let composer = this.chat.read(cx).composer().clone();
                            composer
                                .update(cx, |composer, cx| composer.open_attach_picker(window, cx));
                        }
                        other => log::warn!("PI_GUI_OPEN: unknown panel {other:?}"),
                    }
                }
            });
        });
        Self {
            sidebar,
            chat,
            background: Vec::new(),
            right_panel,
            terminal: None,
            last_session: HashMap::new(),
            sidebar_open: true,
            right_open: false,
            terminal_open: false,
            drag: WindowDrag::default(),
            focus_handle,
            chat_subscription,
            _subscriptions: subscriptions,
        }
    }

    fn cwd(&self, cx: &App) -> PathBuf {
        self.chat.read(cx).session().read(cx).cwd().to_owned()
    }

    /// The mode of the visible conversation.
    fn mode(&self, cx: &App) -> Mode {
        self.chat.read(cx).session().read(cx).mode()
    }

    /// Where a new conversation in `mode` starts: the chat folder, or for Code the current
    /// project, else the most recent one, else the launch folder.
    fn default_folder(&self, mode: Mode, cx: &App) -> PathBuf {
        let config = cx.global::<AppConfig>();
        match mode {
            Mode::Chat => config.chat_dir.clone(),
            Mode::Code if self.mode(cx) == Mode::Code => self.cwd(cx),
            Mode::Code => self
                .sidebar
                .read(cx)
                .latest_code_folder(cx)
                .unwrap_or_else(|| config.default_cwd.clone()),
        }
    }

    /// Shows `mode`: the conversation last open in it, or a new one. Chat has no project
    /// folder, so the terminal and side panel close.
    fn switch_to_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_mode(mode, window, cx));
        if self.mode(cx) == mode {
            return;
        }
        if mode == Mode::Chat {
            self.terminal_open = false;
            self.set_right_open(false, cx);
        }
        let last = self
            .last_session
            .get(&mode)
            .and_then(|path| self.sidebar.read(cx).summary(path));
        match last {
            Some(summary) => drop(self.open_session(&summary, window, cx)),
            None => {
                let folder = self.default_folder(mode, cx);
                self.new_chat_in(folder, window, cx);
            }
        }
    }

    fn show_chat_mode(&mut self, _: &ShowChatMode, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_mode(Mode::Chat, window, cx);
    }

    fn show_code_mode(&mut self, _: &ShowCodeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_mode(Mode::Code, window, cx);
    }

    /// Replaces the visible chat, keeping the old one alive if it is still working.
    fn show_chat(&mut self, chat: Entity<ChatView>, window: &mut Window, cx: &mut Context<Self>) {
        let previous = std::mem::replace(&mut self.chat, chat.clone());
        {
            let session = previous.read(cx).session().read(cx);
            if let Some(file) = session.session_file() {
                self.last_session.insert(session.mode(), file);
            }
        }
        // Opening a conversation of the other mode (e.g. at startup) switches the sidebar too.
        let mode = chat.read(cx).session().read(cx).mode();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_mode(mode, window, cx));
        if mode == Mode::Chat {
            self.terminal_open = false;
            if self.right_open {
                self.set_right_open(false, cx);
            }
        }
        if previous.read(cx).session().read(cx).is_running() {
            // Keep it alive until it settles, then let it (and its pi process) go.
            let subscription = cx.subscribe(&previous, |this, _, event: &ChatEvent, cx| {
                if matches!(event, ChatEvent::SessionChanged) {
                    this.prune_background(cx);
                    this.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
                }
            });
            self.background.push((previous, subscription));
        }
        self.background
            .retain(|(background, _)| background.entity_id() != chat.entity_id());
        self.prune_background(cx);
        self.chat_subscription = cx.subscribe_in(&chat, window, Self::on_chat_event);

        let cwd = self.cwd(cx);
        let session_file = chat.read(cx).session().read(cx).session_file();
        self.right_panel
            .update(cx, |panel, cx| panel.set_cwd(cwd, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_active(session_file, cx));
        focus_chat(&chat, window, cx);
        cx.notify();
    }

    /// Drops background chats whose agent has finished.
    fn prune_background(&mut self, cx: &mut Context<Self>) {
        self.background
            .retain(|(chat, _)| chat.read(cx).session().read(cx).is_running());
    }

    fn new_chat_in(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let chat = cx.new(|cx| ChatView::new(cwd, None, window, cx));
        self.show_chat(chat, window, cx);
    }

    fn open_session(
        &mut self,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ChatView> {
        let target = Some(summary.path.clone());
        if self.chat.read(cx).session().read(cx).session_file() == target {
            return self.chat.clone();
        }
        let existing = self
            .background
            .iter()
            .find(|(chat, _)| chat.read(cx).session().read(cx).session_file() == target)
            .map(|(chat, _)| chat.clone());
        let chat = existing.unwrap_or_else(|| {
            cx.new(|cx| ChatView::new(summary.cwd.clone(), Some(summary.path.clone()), window, cx))
        });
        self.show_chat(chat.clone(), window, cx);
        chat
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::Open(summary) => drop(self.open_session(summary, window, cx)),
            SidebarEvent::NewChat => self.new_chat(&NewChat, window, cx),
            SidebarEvent::NewInFolder(folder) => self.new_chat_in(folder.clone(), window, cx),
            SidebarEvent::ModeChanged(mode) => self.switch_to_mode(*mode, window, cx),
            SidebarEvent::Collapse => self.toggle_sidebar(&ToggleSidebar, window, cx),
            SidebarEvent::Delete(summary) => self.confirm_delete(summary.clone(), window, cx),
            SidebarEvent::Rename(summary) => self.prompt_rename(summary.clone(), window, cx),
        }
    }

    fn on_chat_event(
        &mut self,
        _: &Entity<ChatView>,
        event: &ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChatEvent::SessionChanged => {
                let session_file = self.chat.read(cx).session().read(cx).session_file();
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_active(session_file, cx);
                    sidebar.refresh(cx);
                });
                self.right_panel.update(cx, |panel, cx| panel.refresh(cx));
                cx.notify();
            }
            ChatEvent::ChangeFolder(folder) => self.new_chat_in(folder.clone(), window, cx),
        }
    }

    fn confirm_delete(
        &mut self,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        let title = summary.title().to_owned();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, summary) = (weak.clone(), summary.clone());
            alert
                .title(format!("Move “{title}” to the trash?"))
                .description(
                    "The session file is moved to the trash and can be restored from there.",
                )
                .confirm()
                .ok_text("Move to trash")
                .ok_variant(component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.delete_session(&summary, window, cx));
                    true
                })
        });
    }

    fn delete_session(
        &mut self,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_active = self
            .chat
            .read(cx)
            .session()
            .read(cx)
            .session_file()
            .as_deref()
            == Some(summary.path.as_path());
        if is_active {
            let cwd = summary.cwd.clone();
            self.new_chat_in(cwd, window, cx);
        }
        self.background.retain(|(chat, _)| {
            chat.read(cx).session().read(cx).session_file().as_deref()
                != Some(summary.path.as_path())
        });
        if let Err(error) = trash::delete(&summary.path) {
            log::warn!("could not trash {}: {error}", summary.path.display());
            window.push_notification(
                format!("Could not move the session to the trash: {error}"),
                cx,
            );
        }
        self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
    }

    fn prompt_rename(
        &mut self,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(summary.title().to_owned()));
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (weak, summary, input_value) = (weak.clone(), summary.clone(), input.clone());
            dialog
                .title("Rename chat")
                .w(px(420.))
                .child(Input::new(&input))
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rename-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("rename-ok").primary().label("Rename").on_click(
                            move |_, window, cx| {
                                let name = input_value.read(cx).value().trim().to_owned();
                                if !name.is_empty() {
                                    let summary = summary.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        // Renames go through pi so the session file stays pi's.
                                        let chat = this.open_session(&summary, window, cx);
                                        chat.read(cx)
                                            .session()
                                            .clone()
                                            .update(cx, |session, cx| session.rename(name, cx));
                                    });
                                }
                                window.close_dialog(cx);
                            },
                        )),
                )
        });
    }

    // ---- actions -------------------------------------------------------------

    fn new_chat(&mut self, _: &NewChat, window: &mut Window, cx: &mut Context<Self>) {
        // An untouched conversation of the sidebar's mode is already "new": just focus it.
        let mode = self.sidebar.read(cx).mode();
        if self.mode(cx) == mode && self.chat.read(cx).session().read(cx).is_empty() {
            focus_chat(&self.chat, window, cx);
            return;
        }
        let folder = self.default_folder(mode, cx);
        self.new_chat_in(folder, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn toggle_right_panel(&mut self, _: &ToggleRightPanel, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode(cx) == Mode::Chat {
            return;
        }
        self.set_right_open(!self.right_open, cx);
    }

    /// Opens or closes the right panel; a hidden panel stops watching the folder.
    fn set_right_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.right_open = open;
        self.right_panel
            .update(cx, |panel, cx| panel.set_visible(open, cx));
        cx.notify();
    }

    fn show_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.mode(cx) == Mode::Chat {
            return;
        }
        let current = self.right_panel.read(cx).tab();
        if self.right_open && current == tab {
            self.set_right_open(false, cx);
        } else {
            self.right_panel
                .update(cx, |panel, cx| panel.set_tab(tab, cx));
            self.set_right_open(true, cx);
        }
    }

    fn show_diff(&mut self, _: &ShowDiff, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(RightTab::Diff, cx);
    }

    fn show_files(&mut self, _: &ShowFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(RightTab::Files, cx);
    }

    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode(cx) == Mode::Chat {
            return;
        }
        if self.terminal_open {
            self.terminal_open = false;
            focus_chat(&self.chat, window, cx);
        } else {
            let cwd = self.cwd(cx);
            let reuse = self
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.read(cx).cwd() == cwd);
            if !reuse {
                let terminal = cx.new(|cx| TerminalPanel::new(cwd, window, cx));
                self._subscriptions.push(cx.subscribe_in(
                    &terminal,
                    window,
                    |this, _, event, window, cx| match event {
                        TerminalPanelEvent::Closed => {
                            this.terminal_open = false;
                            this.terminal = None;
                            focus_chat(&this.chat, window, cx);
                            cx.notify();
                        }
                    },
                ));
                self.terminal = Some(terminal);
            }
            self.terminal_open = true;
            if let Some(terminal) = &self.terminal {
                let focus = terminal.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    fn focus_composer(&mut self, _: &FocusComposer, window: &mut Window, cx: &mut Context<Self>) {
        focus_chat(&self.chat, window, cx);
    }

    // ---- rendering -----------------------------------------------------------

    fn render_chat_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.chat.read(cx).session().read(cx);
        let title = session.display_title().unwrap_or_else(|| {
            match session.mode() {
                Mode::Chat => "New chat",
                Mode::Code => "New session",
            }
            .to_owned()
        });
        let home = cx.global::<AppConfig>().home.clone();
        let folder = crate::ui::short_path(session.cwd(), home.as_deref());
        // Chat has no project folder: no path, terminal, diff or files.
        let code = session.mode() == Mode::Code;
        let tab = self.right_panel.read(cx).tab();
        let theme = cx.theme();

        self.drag.apply(
            h_flex()
                .id("chat-top-bar")
                .h(TOP_BAR_HEIGHT)
                .flex_none()
                .w_full()
                .pr_2()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .map(|this| {
                    if self.sidebar_open {
                        this.pl_4()
                    } else {
                        this.pl(TRAFFIC_LIGHT_INSET)
                    }
                })
                .when(!self.sidebar_open, |this| {
                    this.child(
                        Button::new("open-sidebar")
                            .quiet(cx)
                            .compact()
                            .child(crate::ui::top_bar_icon(IconName::PanelLeftOpen))
                            .tooltip("Show sidebar (⌘B)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx);
                            })),
                    )
                    .child(
                        Button::new("header-new-chat")
                            .quiet(cx)
                            .compact()
                            .child(crate::ui::top_bar_icon(component::Icon::from(
                                AppIcon::SquarePen,
                            )))
                            .tooltip("New chat (⌘N)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_chat(&NewChat, window, cx);
                            })),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                // The title keeps its width (up to a cap); the folder truncates first.
                                .flex_shrink_0()
                                .max_w(px(360.))
                                .truncate()
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when(code, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(folder),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .when(!code, |this| this.hidden())
                        .child(
                            Button::new("toggle-terminal")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(IconName::SquareTerminal))
                                .selected(self.terminal_open)
                                .tooltip("Terminal (ctrl+`)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_terminal(&ToggleTerminal, window, cx);
                                })),
                        )
                        .child(
                            Button::new("show-diff")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(component::Icon::from(
                                    AppIcon::GitCompare,
                                )))
                                .selected(self.right_open && tab == RightTab::Diff)
                                .tooltip("Changes (⌘⇧D)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_diff(&ShowDiff, window, cx);
                                })),
                        )
                        .child(
                            Button::new("show-files")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(component::Icon::from(
                                    AppIcon::Files,
                                )))
                                .selected(self.right_open && tab == RightTab::Files)
                                .tooltip("Files (⌘⇧E)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_files(&ShowFiles, window, cx);
                                })),
                        )
                        .child(
                            Button::new("toggle-right")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(if self.right_open {
                                    IconName::PanelRightClose
                                } else {
                                    IconName::PanelRightOpen
                                }))
                                .tooltip("Toggle side panel (⌘⌥B)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_right_panel(&ToggleRightPanel, window, cx);
                                })),
                        ),
                ),
        )
    }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (background, foreground, font_family) = (
            cx.theme().background,
            cx.theme().foreground,
            cx.theme().font_family.clone(),
        );
        let chat_column = v_flex()
            .size_full()
            .min_w_0()
            .child(self.render_chat_header(cx))
            .child(
                v_resizable("chat-and-terminal")
                    .child(resizable_panel().child(self.chat.clone().cached(full())))
                    .child(
                        resizable_panel()
                            .visible(self.terminal_open)
                            .size(px(280.))
                            .size_range(px(120.)..px(2000.))
                            .children(
                                self.terminal
                                    .clone()
                                    .map(|terminal| terminal.cached(full())),
                            ),
                    ),
            );

        div()
            .key_context(WINDOW_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_chat))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_right_panel))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::show_diff))
            .on_action(cx.listener(Self::show_files))
            .on_action(cx.listener(Self::focus_composer))
            .on_action(cx.listener(Self::show_chat_mode))
            .on_action(cx.listener(Self::show_code_mode))
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(font_family)
            .child(
                h_resizable("main-columns")
                    .child(
                        resizable_panel()
                            .visible(self.sidebar_open)
                            .size(px(272.))
                            .size_range(px(200.)..px(440.))
                            .child(self.sidebar.clone().cached(full())),
                    )
                    .child(resizable_panel().child(chat_column))
                    .child(
                        resizable_panel()
                            .visible(self.right_open)
                            .size(px(460.))
                            .size_range(px(300.)..px(1200.))
                            .child(self.right_panel.clone().cached(full())),
                    ),
            )
    }
}
