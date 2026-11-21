//! The main window: sidebar | chat (+ terminal below) | right panel.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use pi_sessions::Mode;

use crate::actions::{
    FocusComposer, NewChat, ShowChatMode, ShowCodeMode, ShowDiff, ShowFiles, ToggleRightPanel,
    ToggleSidebar, ToggleTerminal, WINDOW_CONTEXT,
};
use crate::chat::{ChatEvent, ChatView, ComposerEvent};
use crate::config::AppConfig;
use crate::hosts::HostsEvent;
use crate::panels::{RightPanel, RightPanelEvent, RightTab, TerminalPanel, TerminalPanelEvent};
use crate::remote_ui::{
    AddHost, AddHostEvent, ConnectFlow, ConnectFlowEvent, ManageHosts, ManageHostsEvent,
};
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::ui::QuietButton as _;
use crate::ui::{AppIcon, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, WindowDrag};
use crate::ui_state::{LastSession, UiState};
use remote::HostId;

pub fn open_main_window(cx: &mut App) {
    crate::chat::init(cx);
    crate::panels::terminal_panel::init(cx);
    crate::hosts::init(cx);
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

/// A restart of pi that waits for the agent to settle, and the model to select afterwards.
#[derive(Debug, Clone)]
struct PendingReload {
    select: Option<(String, String)>,
}

/// A session on an SSH host to open once the host is connected.
#[derive(Debug, Clone)]
struct RemoteTarget {
    host: HostId,
    folder: PathBuf,
    /// The session file to resume; `None` starts a new session.
    file: Option<PathBuf>,
}

#[allow(clippy::struct_excessive_bools)] // independent layout flags
pub struct MainWindow {
    sidebar: Entity<Sidebar>,
    chat: Entity<ChatView>,
    /// Chats switched away from while their agent was still working; kept alive
    /// until they settle so a run is never killed by navigation.
    background: Vec<(Entity<ChatView>, Subscription)>,
    right_panel: Entity<RightPanel>,
    terminal: Option<Entity<TerminalPanel>>,
    /// Events of the open SSH dialog (add host / connect flow / manage hosts).
    dialog_subscription: Option<Subscription>,
    /// The remote session to open when its host is ready: from the connect dialog's
    /// "connect, then open" mode, or restored at startup (`quiet`: no dialog).
    pending_remote: Option<(RemoteTarget, bool)>,
    /// `PI_GUI_OPEN` panels to open once a remote session is shown.
    after_remote: Vec<String>,
    /// `PI_GUI_OPEN=remote-latest:…`: open this host's newest session once it is listed.
    pending_latest: Option<HostId>,
    /// models.json changed while the agent worked: restart pi (and select this model) once
    /// it settles.
    models_reload: Option<PendingReload>,
    /// The Skills page, shown in the chat column instead of the chat while `skills_open`.
    skills: Option<(Entity<crate::skills::SkillsView>, Subscription)>,
    skills_open: bool,
    /// Set while pi restarts for the Skills page, so showing the new chat keeps the page.
    keep_skills: bool,
    /// The welcome screen is showing (no session open yet; `chat` is an idle placeholder).
    welcome: bool,
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
        // Start on the welcome screen: no pi process and no folder until the user picks one
        // (starting in the launch folder, often `~` or `/`, was surprising and made the file
        // index walk the whole home folder).
        let mode = sidebar.read(cx).mode();
        let cwd = cx
            .global::<AppConfig>()
            .home
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let chat = cx.new(|cx| ChatView::idle(cwd.clone(), mode, window, cx));
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
            cx.subscribe_in(
                &crate::hosts::hosts(cx),
                window,
                |this, hosts, event: &HostsEvent, window, cx| match event {
                    HostsEvent::Prompt => {
                        while let Some(prompt) = hosts.update(cx, |hosts, _| hosts.take_prompt()) {
                            crate::remote_ui::open_prompt(prompt, window, cx);
                        }
                    }
                    HostsEvent::Changed => this.check_pending_remote(window, cx),
                    HostsEvent::SessionsChanged(host) => {
                        this.check_pending_remote(window, cx);
                        if this.pending_latest.as_ref() == Some(host) {
                            let latest = hosts.read(cx).sessions(host).first().cloned();
                            if let Some(latest) = latest {
                                this.pending_latest = None;
                                let target = RemoteTarget {
                                    host: host.clone(),
                                    folder: latest.cwd,
                                    file: Some(latest.path),
                                };
                                this.open_remote(target, window, cx);
                            }
                        }
                    }
                },
            ),
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
                this.open_panels(&open_at_startup, window, cx);
            });
        });
        // A Code session on an SSH host comes back after a restart (once the host connected
        // quietly in the background; nothing happens if it cannot be reached).
        let restore = {
            let state = cx.default_global::<UiState>();
            (state.mode == Mode::Code)
                .then(|| state.last_sessions.get(&Mode::Code).cloned())
                .flatten()
                .and_then(|last| {
                    Some(RemoteTarget {
                        host: HostId(last.host?),
                        folder: last.folder?,
                        file: Some(last.file),
                    })
                })
                .filter(|target| crate::hosts::hosts(cx).read(cx).get(&target.host).is_some())
        };
        Self {
            sidebar,
            chat,
            background: Vec::new(),
            right_panel,
            terminal: None,
            dialog_subscription: None,
            pending_remote: restore.map(|target| (target, true)),
            after_remote: Vec::new(),
            pending_latest: None,
            welcome: true,
            models_reload: None,
            skills: None,
            skills_open: false,
            keep_skills: false,
            sidebar_open: true,
            right_open: false,
            terminal_open: false,
            drag: WindowDrag::default(),
            focus_handle,
            chat_subscription,
            _subscriptions: subscriptions,
        }
    }

    /// Opens panels and dialogs named in `PI_GUI_OPEN` (scripted screenshots and checks).
    fn open_panels(&mut self, panels: &[String], window: &mut Window, cx: &mut Context<Self>) {
        for (index, panel) in panels.iter().enumerate() {
            // Panels need a session: scripted runs open one in the launch folder.
            if self.welcome
                && matches!(
                    panel.as_str(),
                    "terminal" | "diff" | "files" | "attach" | "context"
                )
                && let Ok(folder) = std::env::current_dir()
            {
                self.new_chat_in(folder, window, cx);
            }
            match panel.as_str() {
                "terminal" => self.toggle_terminal(&ToggleTerminal, window, cx),
                "diff" => self.show_diff(&ShowDiff, window, cx),
                "files" => self.show_files(&ShowFiles, window, cx),
                "no-sidebar" => self.toggle_sidebar(&ToggleSidebar, window, cx),
                "chat" => self.switch_to_mode(Mode::Chat, window, cx),
                "add-host" => self.open_add_host(window, cx),
                "manage-hosts" => self.open_manage_hosts(window, cx),
                "skills" => self.toggle_skills(window, cx),
                other if other.starts_with("open-skill:") => {
                    let path = other.trim_start_matches("open-skill:").to_owned();
                    self.toggle_skills(window, cx);
                    if let Some((skills, _)) = &self.skills {
                        skills.update(cx, |skills, cx| skills.open_skill(path, window, cx));
                    }
                }
                "new-skill" => {
                    self.toggle_skills(window, cx);
                    if let Some((skills, _)) = &self.skills {
                        skills.update(cx, |skills, cx| skills.start_new(window, cx));
                    }
                }
                "add-model" => drop(self.open_models_dialog(false, window, cx)),
                "add-provider" => {
                    let dialog = self.open_models_dialog(false, window, cx);
                    dialog.update(cx, |dialog, cx| dialog.start_with_new_provider(cx));
                }
                "edit-models" => drop(self.open_models_dialog(true, window, cx)),
                other if other.starts_with("ssh:") => {
                    let alias = other.trim_start_matches("ssh:").to_owned();
                    let host = add_alias_host(alias, cx);
                    self.open_connect_flow(host, window, cx);
                }
                // `remote:<ssh alias>:<folder>`: a new session there once connected.
                // `remote:<ssh alias>:<folder>`: a new session there once connected; the
                // panels after it apply to that session.
                other if other.starts_with("remote:") => {
                    let rest = other.trim_start_matches("remote:");
                    if let Some((alias, folder)) = rest.split_once(':') {
                        let host = add_alias_host(alias.to_owned(), cx);
                        let target = RemoteTarget {
                            host,
                            folder: PathBuf::from(folder),
                            file: None,
                        };
                        self.after_remote = panels[index + 1..].to_vec();
                        self.open_remote(target, window, cx);
                        return;
                    }
                }
                // `remote-latest:<ssh alias>`: the newest session on that host, once listed.
                other if other.starts_with("remote-latest:") => {
                    let alias = other.trim_start_matches("remote-latest:").to_owned();
                    let host = add_alias_host(alias, cx);
                    crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.connect(&host, false, cx));
                    self.after_remote = panels[index + 1..].to_vec();
                    self.pending_latest = Some(host);
                    return;
                }
                "code" => self.switch_to_mode(Mode::Code, window, cx),
                "latest" => {
                    let dir = cx.global::<AppConfig>().sessions_dir.clone();
                    if let Some(latest) =
                        dir.and_then(|dir| pi_sessions::scan(&dir).into_iter().next())
                    {
                        drop(self.open_session(None, &latest, window, cx));
                    }
                }
                "context" => {
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.open_context_breakdown(cx));
                }
                "attach" => {
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.open_attach_picker(window, cx));
                }
                other => log::warn!("PI_GUI_OPEN: unknown panel {other:?}"),
            }
        }
    }

    fn cwd(&self, cx: &App) -> PathBuf {
        self.chat.read(cx).session().read(cx).cwd().to_owned()
    }

    /// The visible conversation is a Code session on this computer.
    fn local_code(&self, cx: &App) -> bool {
        !self.welcome && self.chat.read(cx).session().read(cx).is_local_code()
    }

    /// The visible conversation has a project folder (here or on an SSH host), so the
    /// terminal and the diff / files panels apply.
    fn folder_tools(&self, cx: &App) -> bool {
        !self.welcome && self.chat.read(cx).session().read(cx).has_folder_tools()
    }

    /// The SSH host of the visible conversation.
    fn host(&self, cx: &App) -> Option<HostId> {
        self.chat.read(cx).session().read(cx).host().cloned()
    }

    /// The mode of the visible conversation.
    fn mode(&self, cx: &App) -> Mode {
        self.chat.read(cx).session().read(cx).mode()
    }

    /// Where a new conversation in `mode` starts: the chat folder, or for Code the current
    /// project, else the most recent one, else the launch folder.
    /// The chat folder, or for Code the current project, else the most recent one. `None`
    /// when there is no project yet: the user picks one (never silently the launch folder).
    fn default_folder(&self, mode: Mode, cx: &App) -> Option<PathBuf> {
        let config = cx.global::<AppConfig>();
        match mode {
            Mode::Chat => Some(config.chat_dir.clone()),
            Mode::Code if self.local_code(cx) => Some(self.cwd(cx)),
            Mode::Code => self.sidebar.read(cx).latest_code_folder(cx),
        }
    }

    /// A new conversation in `mode`'s default folder; asks for a folder when there is none.
    fn new_in_default_folder(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        match self.default_folder(mode, cx) {
            Some(folder) => self.new_chat_in(folder, window, cx),
            None => self.choose_folder(window, cx),
        }
    }

    /// The system folder dialog; a new Code session starts in the chosen folder.
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(folder) = paths.pop()
            {
                let _ = this.update_in(cx, |this, window, cx| this.new_chat_in(folder, window, cx));
            }
        })
        .detach();
    }

    /// Shows the welcome screen for `mode` (nothing running until the user starts something).
    fn show_welcome(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cx
            .global::<AppConfig>()
            .home
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let chat = cx.new(|cx| ChatView::idle(cwd, mode, window, cx));
        self.show_chat(chat, window, cx);
        self.welcome = true;
        self.terminal_open = false;
        if self.right_open {
            self.set_right_open(false, cx);
        }
        cx.notify();
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
            if self.skills_open {
                self.close_skills(cx);
            }
        }
        let last = cx
            .default_global::<UiState>()
            .last_sessions
            .get(&mode)
            .cloned();
        match last {
            // A remote session resumes on its host.
            Some(LastSession {
                file,
                host: Some(host),
                folder: Some(folder),
            }) if crate::hosts::hosts(cx)
                .read(cx)
                .get(&HostId(host.clone()))
                .is_some() =>
            {
                let target = RemoteTarget {
                    host: HostId(host),
                    folder,
                    file: Some(file),
                };
                self.open_remote(target, window, cx);
            }
            Some(LastSession {
                file, host: None, ..
            }) if let Some(summary) = self.sidebar.read(cx).summary(&file) => {
                drop(self.open_session(None, &summary, window, cx));
            }
            // Nothing to come back to: the welcome screen for that mode.
            _ => self.show_welcome(mode, window, cx),
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
        self.welcome = false;
        remember(&previous, cx);
        remember(&chat, cx);
        if self.skills_open {
            if self.keep_skills {
                let session = chat.read(cx).session().clone();
                if let Some((skills, _)) = &self.skills {
                    skills.update(cx, |skills, cx| skills.set_session(session, cx));
                }
            } else {
                self.close_skills(cx);
            }
        }
        // Opening a conversation of the other mode (e.g. at startup) switches the sidebar too.
        let mode = chat.read(cx).session().read(cx).mode();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_mode(mode, window, cx));
        if !chat.read(cx).session().read(cx).has_folder_tools() {
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
        let host = self.host(cx);
        let session_file = chat.read(cx).session().read(cx).session_file();
        let location = self.location(cx);
        self.right_panel
            .update(cx, |panel, cx| panel.set_cwd(cwd.clone(), location, cx));
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_active(session_file, host.clone(), cx);
        });
        // A terminal for another folder or machine is replaced when it is next shown.
        if self.terminal_open
            && !self.terminal.as_ref().is_some_and(|terminal| {
                terminal.read(cx).cwd() == cwd && terminal.read(cx).host() == host.as_ref()
            })
        {
            self.terminal_open = false;
        }
        focus_chat(&chat, window, cx);
        cx.notify();
    }

    /// Where the visible conversation's folder lives.
    fn location(&self, cx: &mut Context<Self>) -> workspace::Location {
        match self.host(cx) {
            Some(host) => crate::hosts::hosts(cx).update(cx, |hosts, _| hosts.location(&host)),
            None => workspace::Location::Local,
        }
    }

    /// Drops background chats whose agent has finished.
    fn prune_background(&mut self, cx: &mut Context<Self>) {
        self.background
            .retain(|(chat, _)| chat.read(cx).session().read(cx).is_running());
    }

    fn new_chat_in(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let chat = cx.new(|cx| ChatView::new(cwd, None, None, window, cx));
        self.show_chat(chat, window, cx);
    }

    /// Opens a session (on `host`, or this computer), reusing the visible or a background
    /// chat that already has it.
    fn open_session(
        &mut self,
        host: Option<HostId>,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ChatView> {
        let target = Some(summary.path.clone());
        let same = |chat: &Entity<ChatView>, cx: &App| {
            let session = chat.read(cx).session().read(cx);
            session.session_file() == target && session.host() == host.as_ref()
        };
        if same(&self.chat, cx) {
            return self.chat.clone();
        }
        let existing = self
            .background
            .iter()
            .find(|(chat, _)| same(chat, cx))
            .map(|(chat, _)| chat.clone());
        let chat = existing.unwrap_or_else(|| {
            cx.new(|cx| {
                ChatView::new(
                    summary.cwd.clone(),
                    Some(summary.path.clone()),
                    host.clone(),
                    window,
                    cx,
                )
            })
        });
        self.show_chat(chat.clone(), window, cx);
        chat
    }

    /// Opens a session on an SSH host: right away when the host is ready, else through the
    /// connect dialog (trust prompt, password, pi install), which opens it once connected.
    fn open_remote(&mut self, target: RemoteTarget, window: &mut Window, cx: &mut Context<Self>) {
        let ready = crate::hosts::hosts(cx)
            .read(cx)
            .status(&target.host)
            .is_ready();
        if ready {
            self.open_remote_now(target, window, cx);
            return;
        }
        let host = target.host.clone();
        self.pending_remote = Some((target, false));
        self.open_connect_dialog(host, true, window, cx);
    }

    fn open_remote_now(
        &mut self,
        target: RemoteTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RemoteTarget { host, folder, file } = target;
        match file {
            Some(file) => {
                let summary = crate::hosts::hosts(cx)
                    .read(cx)
                    .sessions(&host)
                    .iter()
                    .find(|session| session.path == file)
                    .cloned()
                    .unwrap_or_else(|| SessionSummary {
                        path: file,
                        id: String::new(),
                        cwd: folder,
                        name: None,
                        first_prompt: None,
                        modified: std::time::SystemTime::UNIX_EPOCH,
                    });
                drop(self.open_session(Some(host), &summary, window, cx));
            }
            None => {
                let folder = folder.to_string_lossy().into_owned();
                self.new_remote_chat(host, folder, window, cx);
            }
        }
        let after = std::mem::take(&mut self.after_remote);
        if !after.is_empty() {
            self.open_panels(&after, window, cx);
        }
    }

    /// Opens the session restored at startup once its host connected quietly and listed its
    /// sessions, if the session still exists there (dropped if the host cannot be used without
    /// asking, or the session is gone).
    fn check_pending_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, true)) = self.pending_remote.clone() else {
            return;
        };
        let (status, loaded, exists) = {
            let hosts = crate::hosts::hosts(cx);
            let hosts = hosts.read(cx);
            let exists = hosts
                .sessions(&target.host)
                .iter()
                .any(|session| Some(&session.path) == target.file.as_ref());
            (
                hosts.status(&target.host),
                hosts.sessions_loaded(&target.host),
                exists,
            )
        };
        if status.is_ready() && loaded {
            self.pending_remote = None;
            // Only if the user has not started something else meanwhile.
            if exists && self.chat.read(cx).session().read(cx).is_empty() {
                self.open_remote_now(target, window, cx);
            }
        } else if !status.is_ready()
            && !status.is_busy()
            && status != crate::hosts::HostStatus::Idle
        {
            self.pending_remote = None;
        }
    }

    /// Connects to the visible session's host again and resumes the session.
    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (host, folder, file) = {
            let session = self.chat.read(cx).session().read(cx);
            let Some(host) = session.host().cloned() else {
                return;
            };
            (host, session.cwd().to_path_buf(), session.session_file())
        };
        self.pending_remote = Some((
            RemoteTarget {
                host: host.clone(),
                folder,
                file,
            },
            false,
        ));
        self.open_connect_dialog(host, true, window, cx);
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::Open(None, summary) => drop(self.open_session(None, summary, window, cx)),
            SidebarEvent::Open(Some(host), summary) => {
                let target = RemoteTarget {
                    host: host.clone(),
                    folder: summary.cwd.clone(),
                    file: Some(summary.path.clone()),
                };
                self.open_remote(target, window, cx);
            }
            SidebarEvent::NewChat => self.new_chat(&NewChat, window, cx),
            SidebarEvent::NewInFolder(None, folder) => self.new_chat_in(folder.clone(), window, cx),
            SidebarEvent::NewInFolder(Some(host), folder) => {
                let target = RemoteTarget {
                    host: host.clone(),
                    folder: folder.clone(),
                    file: None,
                };
                self.open_remote(target, window, cx);
            }
            SidebarEvent::ModeChanged(mode) => self.switch_to_mode(*mode, window, cx),
            SidebarEvent::Collapse => self.toggle_sidebar(&ToggleSidebar, window, cx),
            SidebarEvent::Delete(host, summary) => {
                self.confirm_delete(host.clone(), summary.clone(), window, cx);
            }
            SidebarEvent::Rename(host, summary) => {
                self.prompt_rename(host.clone(), summary.clone(), window, cx);
            }
            SidebarEvent::ConnectHost(host) => self.open_connect_flow(host.clone(), window, cx),
            SidebarEvent::ShowSkills => self.toggle_skills(window, cx),
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
            ChatEvent::SessionChanged
                if self.models_reload.is_some()
                    && !self.chat.read(cx).session().read(cx).is_running() =>
            {
                let select = self.models_reload.take().and_then(|reload| reload.select);
                self.reload_models(select, window, cx);
            }
            ChatEvent::SessionChanged => {
                let session_file = self.chat.read(cx).session().read(cx).session_file();
                let host = self.host(cx);
                remember(&self.chat, cx);
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_active(session_file, host.clone(), cx);
                    sidebar.refresh(cx);
                });
                if let Some(host) = host {
                    crate::hosts::hosts(cx)
                        .update(cx, |hosts, cx| hosts.refresh_sessions(&host, cx));
                }
                self.right_panel.update(cx, |panel, cx| panel.refresh(cx));
                cx.notify();
            }
            ChatEvent::Reconnect => self.reconnect(window, cx),
            ChatEvent::Composer(event) => match event {
                ComposerEvent::ChangeFolder(folder) => self.new_chat_in(folder.clone(), window, cx),
                ComposerEvent::ChooseHost(None) => {
                    if self.chat.read(cx).session().read(cx).host().is_some() {
                        self.new_in_default_folder(Mode::Code, window, cx);
                    }
                }
                ComposerEvent::ChooseHost(Some(host)) | ComposerEvent::BrowseRemote(host) => {
                    self.open_connect_flow(host.clone(), window, cx);
                }
                ComposerEvent::AddHost => self.open_add_host(window, cx),
                ComposerEvent::ManageHosts => self.open_manage_hosts(window, cx),
                ComposerEvent::AddModel { edit_json } => {
                    drop(self.open_models_dialog(*edit_json, window, cx));
                }
            },
        }
    }

    /// "Add SSH connection": on success continues straight to connecting.
    fn open_add_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let add = cx.new(AddHost::new);
        self.dialog_subscription =
            Some(
                cx.subscribe_in(&add, window, |this, _, event: &AddHostEvent, window, cx| {
                    let AddHostEvent::Added(host) = event;
                    window.close_dialog(cx);
                    this.open_connect_flow(host.clone(), window, cx);
                }),
            );
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Add SSH connection")
                .w(px(480.))
                .child(add.clone())
        });
    }

    /// Connects to `host` (trust prompt, password, pi install) and lets the user pick a
    /// folder there.
    fn open_connect_flow(&mut self, host: HostId, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_remote = None;
        self.open_connect_dialog(host, false, window, cx);
    }

    /// The connect dialog; with `auto_open` it closes itself once the host is ready and
    /// opens [`Self::pending_remote`].
    fn open_connect_dialog(
        &mut self,
        host: HostId,
        auto_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = crate::hosts::hosts(cx).read(cx).label(&host);
        let title = if auto_open {
            format!("Connecting to {label}")
        } else {
            format!("Open a folder on {label}")
        };
        let flow = cx.new(|cx| ConnectFlow::new(host, auto_open, window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &flow,
            window,
            |this, _, event: &ConnectFlowEvent, window, cx| match event {
                ConnectFlowEvent::Open { host, folder } => {
                    window.close_dialog(cx);
                    this.new_remote_chat(host.clone(), folder.clone(), window, cx);
                }
                ConnectFlowEvent::OpenTerminal { host } => {
                    window.close_dialog(cx);
                    this.open_remote_terminal(host.clone(), PathBuf::from("~"), window, cx);
                }
                ConnectFlowEvent::Ready { host } => {
                    window.close_dialog(cx);
                    if let Some((target, _)) = this.pending_remote.take()
                        && target.host == *host
                    {
                        this.open_remote_now(target, window, cx);
                    }
                }
            },
        ));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title(title.clone()).w(px(520.)).child(flow.clone())
        });
    }

    /// "Add model" / "Edit models.json" for the pi the visible session runs (here or on its
    /// SSH host). After saving, pi restarts on the same session to load the file.
    fn open_models_dialog(
        &mut self,
        edit_json: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<crate::chat::models_dialog::ModelsDialog> {
        use crate::chat::models_dialog::{ModelsDialog, ModelsDialogEvent, ModelsTab};
        let host = self.host(cx);
        let title = match &host {
            Some(host) => format!("Models on {}", crate::hosts::hosts(cx).read(cx).label(host)),
            None => "Models".to_owned(),
        };
        let tab = if edit_json {
            ModelsTab::Json
        } else {
            ModelsTab::Form
        };
        let dialog = cx.new(|cx| ModelsDialog::new(host, tab, window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &ModelsDialogEvent, window, cx| {
                let ModelsDialogEvent::Saved { select } = event;
                window.close_dialog(cx);
                this.reload_models(select.clone(), window, cx);
            },
        ));
        let view = dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, _| {
            dialog_view
                .title(title.clone())
                .w(px(600.))
                .child(view.clone())
        });
        dialog
    }

    /// pi reads models.json only when it starts: restart the visible session's pi (resuming
    /// its session file) and select `select`. A working agent is not interrupted; the
    /// restart waits until it settles.
    fn reload_models(
        &mut self,
        select: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat.read(cx).session().read(cx).is_running() {
            self.models_reload = Some(PendingReload { select });
            window.push_notification(
                "Saved models.json. pi reloads it when the current turn finishes.",
                cx,
            );
            return;
        }
        let (cwd, file, host) = {
            let session = self.chat.read(cx).session().read(cx);
            (
                session.cwd().to_path_buf(),
                session.session_file(),
                session.host().cloned(),
            )
        };
        let chat = cx.new(|cx| ChatView::new(cwd, file, host, window, cx));
        if let Some((provider, id)) = select {
            let session = chat.read(cx).session().clone();
            session.update(cx, |session, cx| {
                session.select_model_when_ready(provider, id, cx);
            });
        }
        // The old pi is idle; let it go rather than keep it in the background.
        let previous = self.chat.clone();
        self.keep_skills = self.skills_open;
        self.show_chat(chat, window, cx);
        self.keep_skills = false;
        self.background
            .retain(|(chat, _)| chat.entity_id() != previous.entity_id());
    }

    /// Shows (or hides) the Skills page for the visible Code session.
    fn toggle_skills(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.skills_open {
            self.close_skills(cx);
            focus_chat(&self.chat, window, cx);
            cx.notify();
            return;
        }
        if self.mode(cx) != Mode::Code {
            return;
        }
        let session = self.chat.read(cx).session().clone();
        let reuse = self.skills.as_ref().is_some_and(|(skills, _)| {
            let current = skills.read(cx).session_entity();
            let (a, b) = (current.read(cx), session.read(cx));
            a.cwd() == b.cwd() && a.host() == b.host()
        });
        if reuse {
            if let Some((skills, _)) = &self.skills {
                skills.update(cx, |skills, cx| skills.set_session(session, cx));
            }
        } else {
            let skills = cx.new(|cx| crate::skills::SkillsView::new(session, window, cx));
            let subscription = cx.subscribe_in(
                &skills,
                window,
                |this, _, event: &crate::skills::SkillsEvent, window, cx| match event {
                    // pi reads skills (and project trust) only when it starts.
                    crate::skills::SkillsEvent::Changed => this.reload_models(None, window, cx),
                },
            );
            self.skills = Some((skills, subscription));
        }
        self.skills_open = true;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_skills_active(true, cx));
        cx.notify();
    }

    fn close_skills(&mut self, cx: &mut Context<Self>) {
        self.skills_open = false;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_skills_active(false, cx));
    }

    /// "Manage SSH connections".
    fn open_manage_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let manage = cx.new(|cx| ManageHosts::new(window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &manage,
            window,
            |this, _, event: &ManageHostsEvent, window, cx| match event {
                ManageHostsEvent::Add => {
                    window.close_dialog(cx);
                    this.open_add_host(window, cx);
                }
            },
        ));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("SSH connections")
                .w(px(560.))
                .child(manage.clone())
        });
    }

    fn new_remote_chat(
        &mut self,
        host: HostId,
        folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.touch_folder(&host, &folder, cx));
        let chat = cx.new(|cx| ChatView::new(PathBuf::from(folder), None, Some(host), window, cx));
        self.show_chat(chat, window, cx);
    }

    fn confirm_delete(
        &mut self,
        host: Option<HostId>,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        let title = summary.title().to_owned();
        let description = match &host {
            Some(host) => format!(
                "The session file is moved to the trash on {} and can be restored from there.",
                crate::hosts::hosts(cx).read(cx).label(host)
            ),
            None => {
                "The session file is moved to the trash and can be restored from there.".to_owned()
            }
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, summary, host) = (weak.clone(), summary.clone(), host.clone());
            alert
                .title(format!("Move “{title}” to the trash?"))
                .description(description.clone())
                .confirm()
                .ok_text("Move to trash")
                .ok_variant(component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let host = host.clone();
                    let _ = weak.update(cx, |this, cx| {
                        this.delete_session(host, &summary, window, cx);
                    });
                    true
                })
        });
    }

    fn delete_session(
        &mut self,
        host: Option<HostId>,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matches = |chat: &Entity<ChatView>, cx: &App| {
            let session = chat.read(cx).session().read(cx);
            session.session_file().as_deref() == Some(summary.path.as_path())
                && session.host() == host.as_ref()
        };
        if matches(&self.chat, cx) {
            match &host {
                Some(host) => {
                    let folder = summary.cwd.to_string_lossy().into_owned();
                    self.new_remote_chat(host.clone(), folder, window, cx);
                }
                None => self.new_chat_in(summary.cwd.clone(), window, cx),
            }
        }
        self.background.retain(|(chat, _)| !matches(chat, cx));
        if let Some(host) = host {
            let task = crate::hosts::hosts(cx).update(cx, |hosts, cx| {
                hosts.trash_session(&host, summary.path.clone(), cx)
            });
            cx.spawn_in(window, async move |_, cx| {
                if let Err(error) = task.await {
                    let _ = cx.update(|window, cx| {
                        window.push_notification(
                            format!("Could not move the session to the trash: {error}"),
                            cx,
                        );
                    });
                }
            })
            .detach();
            return;
        }
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
        host: Option<HostId>,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(summary.title().to_owned()));
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let (weak, summary, input_value) = (weak.clone(), summary.clone(), input.clone());
            let host = host.clone();
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
                                    let host = host.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        // Renames go through pi so the session file stays pi's
                                        // (on the host, for remote sessions).
                                        let chat = this.open_session(host, &summary, window, cx);
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
        if self.welcome {
            self.new_in_default_folder(mode, window, cx);
            return;
        }
        if self.mode(cx) == mode && self.chat.read(cx).session().read(cx).is_empty() {
            focus_chat(&self.chat, window, cx);
            return;
        }
        // On an SSH host, a new session stays on that host and folder.
        let remote = {
            let session = self.chat.read(cx).session().read(cx);
            session
                .host()
                .cloned()
                .filter(|_| mode == Mode::Code)
                .map(|host| (host, session.cwd().to_string_lossy().into_owned()))
        };
        if let Some((host, folder)) = remote {
            self.new_remote_chat(host, folder, window, cx);
            return;
        }
        self.new_in_default_folder(mode, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn toggle_right_panel(&mut self, _: &ToggleRightPanel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_tools(cx) {
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
        if !self.folder_tools(cx) {
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
        if !self.folder_tools(cx) {
            return;
        }
        if self.terminal_open {
            self.terminal_open = false;
            focus_chat(&self.chat, window, cx);
            cx.notify();
            return;
        }
        let cwd = self.cwd(cx);
        match self.host(cx) {
            Some(host) => self.open_remote_terminal(host, cwd, window, cx),
            None => {
                let reuse = self.terminal.as_ref().is_some_and(|terminal| {
                    terminal.read(cx).cwd() == cwd && terminal.read(cx).host().is_none()
                });
                if !reuse {
                    let terminal = cx.new(|cx| TerminalPanel::new(cwd, window, cx));
                    self.set_terminal(terminal, window, cx);
                }
                self.show_terminal(window, cx);
            }
        }
    }

    /// Shows a terminal on `host` in `cwd` there (reusing an open one for the same place).
    fn open_remote_terminal(
        &mut self,
        host: HostId,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reuse = self.terminal.as_ref().is_some_and(|terminal| {
            terminal.read(cx).cwd() == cwd && terminal.read(cx).host() == Some(&host)
        });
        if !reuse {
            let (label, ssh) = {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                (hosts.label(&host), hosts.ssh(&host))
            };
            let Some(ssh) = ssh else { return };
            let terminal =
                cx.new(|cx| TerminalPanel::remote(host, label, &ssh, cwd, None, window, cx));
            self.set_terminal(terminal, window, cx);
        }
        self.show_terminal(window, cx);
    }

    fn set_terminal(
        &mut self,
        terminal: Entity<TerminalPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions.push(cx.subscribe_in(
            &terminal,
            window,
            |this, terminal, event, window, cx| match event {
                TerminalPanelEvent::Closed => {
                    // Only the current terminal; a replaced one may still report its exit.
                    if this.terminal.as_ref() == Some(terminal) {
                        this.terminal_open = false;
                        this.terminal = None;
                        focus_chat(&this.chat, window, cx);
                        cx.notify();
                    }
                }
            },
        ));
        self.terminal = Some(terminal);
    }

    fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_open = true;
        if let Some(terminal) = &self.terminal {
            let focus = terminal.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn focus_composer(&mut self, _: &FocusComposer, window: &mut Window, cx: &mut Context<Self>) {
        focus_chat(&self.chat, window, cx);
    }

    // ---- rendering -----------------------------------------------------------

    fn render_chat_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.chat.read(cx).session().read(cx);
        let title = if self.skills_open {
            "Skills".to_owned()
        } else if self.welcome {
            String::new()
        } else {
            session.display_title().unwrap_or_else(|| {
                match session.mode() {
                    Mode::Chat => "New chat",
                    Mode::Code => "New session",
                }
                .to_owned()
            })
        };
        let home = cx.global::<AppConfig>().home.clone();
        let folder = crate::ui::short_path(session.cwd(), home.as_deref());
        // Code sessions (here or on an SSH host) have folder tools (terminal, diff, files);
        // chats have no folder, and remote folders are shown as `host:path`.
        let code = session.has_folder_tools() && !self.skills_open && !self.welcome;
        let folder = match session.host() {
            Some(host) => {
                let label = crate::hosts::hosts(cx)
                    .read(cx)
                    .get(host)
                    .map_or_else(|| host.0.clone(), |h| h.label.clone());
                format!("{label}:{}", session.cwd().display())
            }
            None => folder,
        };
        let show_folder = session.mode() == Mode::Code && !self.welcome;
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
                        .when(show_folder, |this| {
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
                .when(self.skills_open, |this| {
                    this.child(
                        Button::new("close-skills")
                            .quiet(cx)
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label("Back to the session")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_skills(window, cx);
                            })),
                    )
                })
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

/// Saves `chat`'s session as the one to come back to in its mode (also across restarts).
fn remember(chat: &Entity<ChatView>, cx: &mut App) {
    let (mode, last) = {
        let session = chat.read(cx).session().read(cx);
        let Some(file) = session.session_file() else {
            return;
        };
        (
            session.mode(),
            LastSession {
                file,
                host: session.host().map(|host| host.0.clone()),
                folder: session.host().map(|_| session.cwd().to_path_buf()),
            },
        )
    };
    UiState::update(cx, |state| {
        state.last_sessions.insert(mode, last);
    });
}

/// Saves (or finds) the host for an ssh config alias.
fn add_alias_host(alias: String, cx: &mut App) -> HostId {
    let target = remote::Target::Alias {
        alias: alias.clone(),
    };
    crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.add(alias, target, cx))
}

impl MainWindow {
    /// The start screen: logo and a button that starts a session (Code: in a recent folder,
    /// a chosen folder or on an SSH host; Chat: a new chat).
    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mode = self.sidebar.read(cx).mode();
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let accent = crate::theme::palette(cx).accent;
        let (headline, detail, label) = match mode {
            Mode::Code => (
                "Start a coding session",
                "Open a folder on this computer or an SSH host, or pick up a session from the \
                 sidebar.",
                "New session",
            ),
            Mode::Chat => (
                "Start a chat",
                "A plain conversation, without access to your files.",
                "New chat",
            ),
        };
        let weak = cx.weak_entity();
        let button = Button::new("welcome-new")
            .primary()
            .large()
            .icon(component::Icon::from(AppIcon::SquarePen))
            .label(label);
        let button = match mode {
            Mode::Chat => button
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_in_default_folder(Mode::Chat, window, cx);
                }))
                .into_any_element(),
            Mode::Code => button
                .dropdown_caret(true)
                .dropdown_menu_with_anchor(Anchor::TopCenter, move |menu, _, cx| {
                    welcome_menu(menu, weak.clone(), cx)
                })
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                component::Icon::from(AppIcon::PiLogo)
                    .size_12()
                    .text_color(accent),
            )
            .child(div().text_xl().text_color(foreground).child(headline))
            .child(
                div()
                    .max_w(px(420.))
                    .text_sm()
                    .text_center()
                    .text_color(muted)
                    .child(detail),
            )
            .child(div().pt_2().child(button))
    }
}

/// The welcome screen's "New session" menu: recent folders (checked now, not at startup),
/// "Choose a folder…", and SSH hosts.
fn welcome_menu(
    mut menu: component::menu::PopupMenu,
    window_view: WeakEntity<MainWindow>,
    cx: &mut Context<component::menu::PopupMenu>,
) -> component::menu::PopupMenu {
    let home = cx.global::<AppConfig>().home.clone();
    let recent: Vec<PathBuf> = cx
        .try_global::<crate::config::RecentFolders>()
        .map(|recent| recent.0.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|folder| folder.is_dir())
        .take(crate::config::RECENT_FOLDERS)
        .collect();
    menu = menu.min_w(px(300.)).max_w(px(460.));
    if !recent.is_empty() {
        menu = menu.label("Recent folders");
        for folder in recent {
            let short = crate::ui::short_path(&folder, home.as_deref());
            let view = window_view.clone();
            menu = menu.item(
                component::menu::PopupMenuItem::new(short)
                    .icon(IconName::Folder)
                    .on_click(move |_, window, cx| {
                        let folder = folder.clone();
                        let _ = view.update(cx, |this, cx| this.new_chat_in(folder, window, cx));
                    }),
            );
        }
        menu = menu.separator();
    }
    let view = window_view.clone();
    menu = menu.item(
        component::menu::PopupMenuItem::new("Choose a folder…")
            .icon(IconName::FolderOpen)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.choose_folder(window, cx));
            }),
    );
    let hosts: Vec<(HostId, String, crate::hosts::HostStatus)> = {
        let hosts = crate::hosts::hosts(cx);
        let hosts = hosts.read(cx);
        hosts
            .hosts()
            .iter()
            .map(|host| (host.id.clone(), host.label.clone(), hosts.status(&host.id)))
            .collect()
    };
    menu = menu.separator().label("On an SSH host");
    for (id, label, status) in hosts {
        let view = window_view.clone();
        menu = menu.item(
            component::menu::PopupMenuItem::element(move |_, cx| {
                h_flex()
                    .gap_2()
                    .child(crate::remote_ui::status_dot(Some(&status), cx))
                    .child(label.clone())
            })
            .on_click(move |_, window, cx| {
                let id = id.clone();
                let _ = view.update(cx, |this, cx| this.open_connect_flow(id, window, cx));
            }),
        );
    }
    let view = window_view;
    menu.item(
        component::menu::PopupMenuItem::new("Add SSH connection…")
            .icon(IconName::Plus)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.open_add_host(window, cx));
            }),
    )
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
        let skills = self
            .skills
            .as_ref()
            .filter(|_| self.skills_open)
            .map(|(skills, _)| skills.clone());
        let chat_column = v_flex()
            .size_full()
            .min_w_0()
            .child(self.render_chat_header(cx))
            .when_some(skills, |this, skills| {
                this.child(div().flex_1().min_h_0().child(skills.cached(full())))
            })
            .when(self.welcome && !self.skills_open, |this| {
                this.child(self.render_welcome(cx))
            })
            .when(!self.skills_open && !self.welcome, |this| {
                this.child(
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
                )
            });

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
