//! The left sidebar: the Chat | Code switch, new chat, and previous sessions.
//!
//! The list comes from pi's session files (read-only, via `pi-sessions`) and is refreshed by
//! a filesystem watcher, so sessions created by the terminal `pi` appear here too. Chat mode
//! lists chats by date; Code mode groups sessions under their working folder, with groups that
//! can be collapsed (remembered in [`UiState`]). Below them, Code mode lists each saved SSH
//! host's sessions (fetched over the host's shared connection by [`crate::hosts`]), grouped by
//! host and then folder, with the host's status dot on its header.

mod rows;
mod typing;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::{Mode, SessionSummary};
use remote::HostId;

use crate::config::{AppConfig, RECENT_FOLDERS, RecentFolders};
use crate::theme::palette;
use crate::ui::QuietButton as _;
use crate::ui::{AppIcon, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, WindowDrag};
use crate::ui_state::UiState;

/// Session events carry the SSH host the session lives on (`None`: this computer).
#[derive(Debug, Clone)]
pub enum SidebarEvent {
    Open(Option<HostId>, SessionSummary),
    /// A new conversation in the current mode.
    NewChat,
    /// A new Code session in this folder.
    NewInFolder(Option<HostId>, PathBuf),
    /// The user switched between Chat and Code.
    ModeChanged(Mode),
    Collapse,
    Delete(Option<HostId>, SessionSummary),
    Rename(Option<HostId>, SessionSummary),
    /// Connect to a host / start a session there (its header's button).
    ConnectHost(HostId),
    /// Show the Skills page (Code mode).
    ShowSkills,
    /// Show the Settings page.
    ShowSettings,
    /// Move every session of this folder (on this host) to the trash.
    DeleteFolder(Option<HostId>, PathBuf),
}

/// What a session is doing, as shown next to its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// The agent is working (spinner).
    Working,
    /// An extension asks something and waits for the user (amber dot).
    NeedsYou,
    /// Finished in the background and not looked at since (green dot).
    Done,
}

/// A conversation's new automatic name being typed out in its row (see [`typing`]).
#[derive(Debug, Clone)]
struct TypedName {
    title: String,
    /// What the row showed when typing started, erased first (empty for a row that was not
    /// listed before).
    from: String,
    /// When typing started; `None` until the session's row is listed.
    started: Option<Instant>,
    /// The name the session listing gave when typing started. While it still gives that
    /// (the file was not re-read yet), the row keeps showing the new name.
    listed_as: String,
    /// When the name arrived (names whose row never appears are dropped after a while).
    arrived: Instant,
}

impl TypedName {
    /// The animation has played to the end.
    fn finished(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|started| now >= started + typing::duration(&self.from, &self.title))
    }
}

/// What a session row shows as its name.
pub(super) enum RowTitle {
    Plain(String),
    /// Mid-animation: the text so far, and whether the caret shows.
    Typing {
        text: String,
        caret: bool,
    },
}

#[allow(clippy::struct_excessive_bools)] // independent view flags
pub struct Sidebar {
    mode: Mode,
    /// Sessions that are working, waiting for an answer or finished unseen, by host and file.
    activity: HashMap<(Option<HostId>, PathBuf), Activity>,
    spinner: Option<Task<()>>,
    /// New automatic names being typed out, by host and file.
    typed: HashMap<(Option<HostId>, PathBuf), TypedName>,
    /// The typing animation's repaint loop is running (only while a name is being typed).
    typing_ticking: bool,
    sessions: Vec<SessionSummary>,
    /// Session files being moved to the trash: hidden from the list right away, also from
    /// scans that finish before the trash does.
    trashing: HashSet<PathBuf>,
    /// The open session: its host (`None`: this computer) and file.
    active: Option<(Option<HostId>, PathBuf)>,
    /// The session open in the side column, if any.
    side_active: Option<(Option<HostId>, PathBuf)>,
    /// The Skills page is showing (its button is highlighted).
    skills_active: bool,
    /// The Settings page is open (its footer row is highlighted).
    settings_active: bool,
    filter: Entity<InputState>,
    loaded: bool,
    drag: WindowDrag,
    scroll: ScrollHandle,
    _watcher: Option<workspace::watch::Watcher>,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = match cx
            .default_global::<crate::settings::AppSettings>()
            .start_mode
        {
            crate::settings::StartMode::Last => cx.default_global::<UiState>().mode,
            crate::settings::StartMode::Chat => Mode::Chat,
            crate::settings::StartMode::Code => Mode::Code,
        };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(search_placeholder(mode)));
        let subscriptions = vec![
            cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            // Remote session lists and host status dots.
            cx.observe(&crate::hosts::hosts(cx), |this, _, cx| {
                this.start_typing(cx);
                cx.notify();
            }),
        ];
        let mut subscriptions = subscriptions;
        if let Some(names) = crate::naming::names(cx) {
            subscriptions.push(cx.subscribe(
                &names,
                |this, _, named: &crate::naming::Named, cx| {
                    this.type_name(named, cx);
                },
            ));
        }
        let mut this = Self {
            mode,
            activity: HashMap::new(),
            spinner: None,
            typed: HashMap::new(),
            typing_ticking: false,
            sessions: Vec::new(),
            trashing: HashSet::new(),
            active: None,
            side_active: None,
            skills_active: false,
            settings_active: false,
            filter,
            loaded: false,
            drag: WindowDrag::default(),
            scroll: ScrollHandle::new(),
            _watcher: None,
            _refresh: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.watch(cx);
        this.refresh(cx);
        // Relative times ("5m") age without any file change; the view is cached, so
        // repaint it once a minute.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        this
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switches the list (and the remembered mode) without emitting an event; the window
    /// calls this when it opens a session of the other mode itself.
    pub fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        UiState::update(cx, |state| state.mode = mode);
        self.filter.update(cx, |filter, cx| {
            filter.set_placeholder(search_placeholder(mode), window, cx);
        });
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn switch_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.set_mode(mode, window, cx);
            cx.emit(SidebarEvent::ModeChanged(mode));
        }
    }

    pub fn set_active(
        &mut self,
        path: Option<PathBuf>,
        host: Option<HostId>,
        cx: &mut Context<Self>,
    ) {
        let active = path.map(|path| (host, path));
        if self.active != active {
            self.active = active;
            cx.notify();
        }
    }

    /// Marks the session open in the side column (`None`: no side chat).
    pub fn set_side_active(
        &mut self,
        side: Option<(Option<HostId>, PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        if self.side_active != side {
            self.side_active = side;
            cx.notify();
        }
    }

    /// Updates the working / needs-you / done marks; the spinner ticks only while a session
    /// works.
    pub fn set_activity(
        &mut self,
        activity: HashMap<(Option<HostId>, PathBuf), Activity>,
        cx: &mut Context<Self>,
    ) {
        if self.activity == activity {
            return;
        }
        let working = activity.values().any(|state| *state == Activity::Working);
        self.activity = activity;
        if !working {
            self.spinner = None;
        } else if self.spinner.is_none() {
            self.spinner = Some(cx.spawn(async move |this, cx| {
                loop {
                    // The working π pulses; the sidebar is cheap to repaint, and only while something
                    // works.
                    cx.background_executor()
                        .timer(crate::ui::spinner::FRAME)
                        .await;
                    let alive = this.update(cx, |_, cx| {
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// A conversation got an automatic name: type it out in its row, now if the row is
    /// listed, else as soon as it is (a new session's file appears after its first answer).
    fn type_name(&mut self, named: &crate::naming::Named, cx: &mut Context<Self>) {
        self.typed.insert(
            (named.host.clone(), named.path.clone()),
            TypedName {
                title: named.title.clone(),
                from: String::new(),
                started: None,
                listed_as: String::new(),
                arrived: Instant::now(),
            },
        );
        self.start_typing(cx);
    }

    /// The name a listed session's row shows now, or `None` when it is not listed.
    fn listed_title(&self, host: Option<&HostId>, path: &Path, cx: &App) -> Option<String> {
        match host {
            None => self
                .sessions
                .iter()
                .find(|session| session.path == path)
                .map(|session| session.title().to_owned()),
            Some(host) => crate::hosts::hosts(cx)
                .read(cx)
                .sessions(host)
                .iter()
                .find(|session| session.path == path)
                .map(|session| session.title().to_owned()),
        }
    }

    /// Starts the animations whose rows are now listed, forgets finished and stale ones,
    /// and runs the repaint loop while any name is being typed.
    fn start_typing(&mut self, cx: &mut Context<Self>) {
        if self.typed.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut listed = HashMap::new();
        for (host, path) in self.typed.keys() {
            if let Some(title) = self.listed_title(host.as_ref(), path, cx) {
                listed.insert((host.clone(), path.clone()), title);
            }
        }
        self.typed.retain(|key, typed| {
            if typed.started.is_none() {
                let Some(shown) = listed.get(key) else {
                    // The row never appeared (the conversation was left before pi wrote it).
                    return now.duration_since(typed.arrived) < Duration::from_secs(120);
                };
                typed.from = if *shown == typed.title {
                    String::new()
                } else {
                    shown.clone()
                };
                typed.listed_as.clone_from(shown);
                typed.started = Some(now);
            }
            if !typed.finished(now) {
                return true;
            }
            // Played: kept only while the listing is stale (still what it was at the start,
            // not the new name nor a name the user gave since), and not for long.
            let stale = listed
                .get(key)
                .is_some_and(|shown| *shown == typed.listed_as)
                && typed.listed_as != typed.title;
            stale && now.duration_since(typed.arrived) < Duration::from_secs(30)
        });
        let typing = self
            .typed
            .values()
            .any(|typed| typed.started.is_some() && !typed.finished(now));
        if typing && !self.typing_ticking {
            self.typing_ticking = true;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(crate::ui::spinner::FRAME)
                        .await;
                    let more = this.update(cx, |this, cx| {
                        let now = Instant::now();
                        let more = this
                            .typed
                            .values()
                            .any(|typed| typed.started.is_some() && !typed.finished(now));
                        this.typing_ticking = more;
                        if !more {
                            this.start_typing(cx);
                        }
                        cx.notify();
                        more
                    });
                    if !matches!(more, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// The automatic name being typed out for the session at `path` (on `host`), if any.
    fn typed_name(&self, host: Option<&HostId>, path: &Path) -> Option<&str> {
        self.typed
            .get(&(host.cloned(), path.to_path_buf()))
            .map(|typed| typed.title.as_str())
    }

    /// What the row of `session` (on `host`) shows as its name.
    fn row_title(&self, host: Option<&HostId>, session: &SessionSummary) -> RowTitle {
        let key = (host.cloned(), session.path.clone());
        let now = Instant::now();
        match self.typed.get(&key) {
            Some(typed) => match typed.started {
                Some(started) if !typed.finished(now) => {
                    let frame = typing::frame(&typed.from, &typed.title, now - started);
                    RowTitle::Typing {
                        text: frame.text.to_owned(),
                        caret: frame.caret,
                    }
                }
                // Played to the end (but not forgotten yet): the name, even if the file
                // listing has not caught up.
                Some(_) => RowTitle::Plain(typed.title.clone()),
                None => RowTitle::Plain(session.title().to_owned()),
            },
            None => RowTitle::Plain(session.title().to_owned()),
        }
    }

    fn activity(&self, host: Option<&HostId>, path: &Path) -> Option<Activity> {
        self.activity
            .get(&(host.cloned(), path.to_path_buf()))
            .copied()
    }

    pub fn set_settings_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.settings_active != active {
            self.settings_active = active;
            cx.notify();
        }
    }

    pub fn set_skills_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.skills_active != active {
            self.skills_active = active;
            cx.notify();
        }
    }

    fn is_active(&self, host: Option<&HostId>, path: &Path) -> bool {
        if self.skills_active || self.settings_active {
            return false;
        }
        [&self.active, &self.side_active]
            .into_iter()
            .flatten()
            .any(|(active_host, active)| active_host.as_ref() == host && active == path)
    }

    /// Whether the first scan of pi's session folder has finished.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Every listed local session, newest first.
    pub fn listed(&self) -> &[SessionSummary] {
        &self.sessions
    }

    /// The summary of a listed session, if known.
    pub fn summary(&self, path: &Path) -> Option<SessionSummary> {
        self.sessions
            .iter()
            .find(|session| session.path == path)
            .cloned()
    }

    /// The listed sessions whose working folder is `folder` (on this computer).
    pub fn sessions_in(&self, folder: &Path) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .filter(|session| session.cwd == folder)
            .cloned()
            .collect()
    }

    /// Rescans pi's session directory in the background.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let config = cx.global::<AppConfig>();
        let Some(dir) = config.sessions_dir.clone() else {
            self.loaded = true;
            return;
        };
        let chat_dir = config.chat_dir.clone();
        let previous = self.sessions.clone();
        self._refresh = cx.spawn(async move |this, cx| {
            let (mut sessions, folders) = cx
                .background_executor()
                .spawn(async move {
                    let sessions = pi_sessions::scan_reusing(&dir, &previous);
                    // Over-fetch so deleted folders can be dropped without shrinking the menu.
                    // Menus check that folders still exist when they open: touching folders
                    // under Desktop or Documents here would make macOS ask for access at
                    // startup.
                    let code = sessions.iter().filter(|session| session.cwd != chat_dir);
                    let folders = pi_sessions::recent_folders(
                        &code.cloned().collect::<Vec<_>>(),
                        RECENT_FOLDERS * 2,
                    );
                    (sessions, folders)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                cx.set_global(RecentFolders(folders));
                sessions.retain(|session| !this.trashing.contains(&session.path));
                this.sessions = sessions;
                if !this.loaded {
                    crate::perf::phase("sessions_listed");
                }
                this.loaded = true;
                this.start_typing(cx);
                cx.notify();
            });
        });
    }

    /// Moves local session files to the trash. Their rows go at once; the trash itself (a
    /// Finder request on macOS, which can take a second) runs in the background. On failure
    /// the list is scanned again, so files that are still there come back.
    pub fn trash_sessions(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.trashing.extend(paths.iter().cloned());
        self.sessions
            .retain(|session| !paths.contains(&session.path));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let paths = paths.clone();
                    async move { trash::delete_all(&paths).map_err(|error| error.to_string()) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for path in &paths {
                    this.trashing.remove(path);
                }
                if result.is_err() {
                    this.refresh(cx);
                }
            });
            result
        })
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = cx.global::<AppConfig>().sessions_dir.clone() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let (sender, receiver) = async_channel::bounded::<()>(1);
        match workspace::watch::Watcher::new(&dir, Duration::from_millis(400), move |_| {
            let _ = sender.try_send(());
        }) {
            Ok(watcher) => self._watcher = Some(watcher),
            Err(error) => {
                log::warn!("cannot watch {}: {error}", dir.display());
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Sessions of the current mode matching the search, newest first.
    fn visible_sessions(&self, cx: &App) -> Vec<&SessionSummary> {
        let config = cx.global::<AppConfig>();
        let query = self.filter.read(cx).value().to_lowercase();
        self.sessions
            .iter()
            .filter(|session| config.mode_of(&session.cwd) == self.mode)
            .filter(|session| {
                query.is_empty()
                    || session.title().to_lowercase().contains(&query)
                    || (self.mode == Mode::Code
                        && session
                            .cwd
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&query))
            })
            .collect()
    }

    fn toggle_host(&mut self, host: &HostId, cx: &mut Context<Self>) {
        let key = crate::ui_state::remote_key(&host.0, None);
        self.toggle_folder(&key, cx);
    }

    /// The saved hosts' session groups for Code mode (hosts with nothing to show and no
    /// connection are left out), as `(host, label, status, sessions matching the search)`.
    fn remote_groups(
        &self,
        cx: &App,
    ) -> Vec<(
        HostId,
        String,
        crate::hosts::HostStatus,
        Vec<SessionSummary>,
    )> {
        let query = self.filter.read(cx).value().to_lowercase();
        let hosts = crate::hosts::hosts(cx);
        let hosts = hosts.read(cx);
        hosts
            .hosts()
            .iter()
            .filter_map(|host| {
                let status = hosts.status(&host.id);
                let all = hosts.sessions(&host.id);
                if all.is_empty() && !status.is_connected() {
                    return None;
                }
                let matching: Vec<SessionSummary> = all
                    .iter()
                    .filter(|session| {
                        query.is_empty()
                            || session.title().to_lowercase().contains(&query)
                            || session
                                .cwd
                                .to_string_lossy()
                                .to_lowercase()
                                .contains(&query)
                            || host.label.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect();
                if !query.is_empty() && matching.is_empty() {
                    return None;
                }
                Some((host.id.clone(), host.label.clone(), status, matching))
            })
            .collect()
    }

    fn toggle_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let folder = folder.to_path_buf();
        UiState::update(cx, |state| {
            if !state.collapsed.remove(&folder) {
                state.collapsed.insert(folder);
            }
        });
        cx.notify();
    }

    fn render_mode_switch(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted_bg, active_bg) = (theme.muted, theme.background);
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let tab = |mode: Mode,
                   label: &'static str,
                   icon: AppIcon,
                   shortcut: &'static str,
                   cx: &mut Context<Self>| {
            let selected = self.mode == mode;
            div()
                .id(label)
                .role(Role::Tab)
                .aria_label(label)
                .aria_selected(selected)
                .aria_keyshortcuts(shortcut)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .gap_1p5()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .map(|this| {
                    if selected {
                        this.bg(active_bg)
                            .shadow_xs()
                            .text_color(foreground)
                            .font_weight(FontWeight::MEDIUM)
                    } else {
                        this.text_color(muted)
                            .hover(|style| style.text_color(foreground))
                    }
                })
                .tooltip(move |window, cx| {
                    component::tooltip::Tooltip::new(format!("{label} ({shortcut})"))
                        .build(window, cx)
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.switch_mode(mode, window, cx)),
                )
                .child(Icon::from(icon).small())
                .child(label)
                .relative()
                .children(crate::demo::probe(format!("mode:{}", label.to_lowercase())))
        };
        h_flex()
            .id("mode-switch")
            .role(Role::TabList)
            .aria_label("Mode")
            .mx_2()
            .mb_2()
            .p_0p5()
            .gap_0p5()
            .rounded_lg()
            .bg(muted_bg)
            .child(tab(
                Mode::Chat,
                "Chat",
                AppIcon::MessageSquare,
                crate::actions::CHAT_MODE_KEYS,
                cx,
            ))
            .child(tab(
                Mode::Code,
                "Code",
                AppIcon::CodeXml,
                crate::actions::CODE_MODE_KEYS,
                cx,
            ))
    }

    fn render_list(&self, cx: &mut Context<Self>) -> Div {
        let now = SystemTime::now();
        let muted = cx.theme().muted_foreground;
        let searching = !self.filter.read(cx).value().is_empty();
        let sessions = self.visible_sessions(cx);
        let mut list = v_flex().w_full().gap_0p5().px_2().pb_3();
        let remote = if self.mode == Mode::Code {
            self.remote_groups(cx)
        } else {
            Vec::new()
        };

        if sessions.is_empty() && remote.is_empty() {
            if self.loaded {
                let text = match (searching, self.mode) {
                    (true, _) => "No matches",
                    (false, Mode::Chat) => "No chats yet",
                    (false, Mode::Code) => "No coding sessions yet",
                };
                list = list.child(div().px_2().pt_4().text_sm().text_color(muted).child(text));
            }
            return list;
        }

        match self.mode {
            Mode::Chat => {
                let mut current = "";
                for row in pi_sessions::side_chat::nest(sessions) {
                    let session = row.session;
                    let group = bucket(session.modified, now);
                    // Side chats stay under their conversation, whatever their age.
                    if row.depth == 0 && group != current {
                        current = group;
                        list = list.child(
                            div()
                                .px_2()
                                .pt_3()
                                .pb_1()
                                .text_xs()
                                .text_color(muted)
                                .child(group),
                        );
                    }
                    list = list.child(rows::session_row(self, &row, 0, None, now, cx));
                }
            }
            Mode::Code => {
                let collapsed = cx.default_global::<UiState>().collapsed.clone();
                let home = cx.global::<AppConfig>().home.clone();
                let groups = pi_sessions::group_by_folder(sessions);
                let labels = folder_labels(groups.iter().map(|group| group.cwd), home.as_deref());
                for (group, label) in groups.into_iter().zip(labels) {
                    // While searching, show every match regardless of collapsed folders.
                    let open = searching || !collapsed.contains(group.cwd);
                    list = list.child(rows::folder_row(
                        group.cwd,
                        None,
                        label,
                        group.sessions.len(),
                        open,
                        cx,
                    ));
                    if open {
                        for row in pi_sessions::side_chat::nest(group.sessions) {
                            list = list.child(rows::session_row(self, &row, 1, None, now, cx));
                        }
                    }
                }
                list = self.render_remote(list, remote, searching, now, cx);
            }
        }
        list
    }

    /// Appends each SSH host's header, its folders and their sessions.
    fn render_remote(
        &self,
        mut list: Div,
        remote: Vec<(
            HostId,
            String,
            crate::hosts::HostStatus,
            Vec<SessionSummary>,
        )>,
        searching: bool,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let collapsed = cx.default_global::<UiState>().collapsed.clone();
        for (host, label, status, sessions) in remote {
            let host_open =
                searching || !collapsed.contains(&crate::ui_state::remote_key(&host.0, None));
            list = list.child(rows::host_row(
                &host,
                label,
                &status,
                sessions.len(),
                host_open,
                cx,
            ));
            if !host_open {
                continue;
            }
            if sessions.is_empty() {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                let text = match (hosts.sessions_loaded(&host), hosts.sessions_error(&host)) {
                    (_, Some(error)) => format!("Could not list sessions: {error}"),
                    (false, None) if status.is_connected() => "Loading sessions…".to_owned(),
                    _ => "No sessions on this host yet".to_owned(),
                };
                list = list.child(
                    div()
                        .pl(px(30.))
                        .py_1()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(text),
                );
                continue;
            }
            let groups = pi_sessions::group_by_folder(sessions.iter());
            // Remote paths are never shortened with this computer's home folder.
            let labels = folder_labels(groups.iter().map(|group| group.cwd), None);
            for (group, folder_label) in groups.into_iter().zip(labels) {
                let key = crate::ui_state::remote_key(&host.0, Some(group.cwd));
                let open = searching || !collapsed.contains(&key);
                list = list.child(rows::folder_row(
                    group.cwd,
                    Some(&host),
                    folder_label,
                    group.sessions.len(),
                    open,
                    cx,
                ));
                if open {
                    for row in pi_sessions::side_chat::nest(group.sessions) {
                        list = list.child(rows::session_row(self, &row, 2, Some(&host), now, cx));
                    }
                }
            }
        }
        list
    }
}

/// A Code folder's label: its name (`~` for home), plus its shortened parent when another
/// listed folder has the same name, so `~/a/demo` and `/tmp/demo` can be told apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderLabel {
    pub name: String,
    pub hint: Option<String>,
}

fn folder_labels<'a>(
    folders: impl Iterator<Item = &'a Path>,
    home: Option<&Path>,
) -> Vec<FolderLabel> {
    let folders: Vec<&Path> = folders.collect();
    let name = |folder: &Path| {
        if home == Some(folder) {
            "~".to_owned()
        } else {
            folder.file_name().map_or_else(
                || crate::ui::short_path(folder, home),
                |name| name.to_string_lossy().into_owned(),
            )
        }
    };
    let names: Vec<String> = folders.iter().map(|folder| name(folder)).collect();
    folders
        .iter()
        .zip(&names)
        .map(|(folder, name)| FolderLabel {
            name: name.clone(),
            hint: (names.iter().filter(|other| *other == name).count() > 1)
                .then(|| {
                    folder
                        .parent()
                        .map(|parent| crate::ui::short_path(parent, home))
                })
                .flatten(),
        })
        .collect()
}

fn search_placeholder(mode: Mode) -> &'static str {
    match mode {
        Mode::Chat => "Search chats",
        Mode::Code => "Search sessions and folders",
    }
}

/// Buckets like other chat apps: Today, Yesterday, Previous 7 days, Older.
fn bucket(modified: SystemTime, now: SystemTime) -> &'static str {
    let age = now
        .duration_since(modified)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match age {
        0..86_400 => "Today",
        86_400..172_800 => "Yesterday",
        172_800..604_800 => "Previous 7 days",
        604_800..2_592_000 => "Previous 30 days",
        _ => "Older",
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("sidebar");
        let colors = palette(cx);
        let new_label = match self.mode {
            Mode::Chat => "New chat",
            Mode::Code => "New session",
        };
        let list = self.render_list(cx);

        v_flex()
            .size_full()
            .bg(colors.sidebar)
            .child(
                self.drag.apply(
                    h_flex()
                        .id("sidebar-top-bar")
                        .h(TOP_BAR_HEIGHT)
                        .flex_none()
                        .pl(TRAFFIC_LIGHT_INSET)
                        .pr_2()
                        .justify_end()
                        .gap_1()
                        .child(
                            Button::new("collapse-sidebar")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(IconName::PanelLeftClose))
                                .accessibility_label("Hide sidebar (⌘B)")
                                .tooltip("Hide sidebar (⌘B)")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::Collapse)),
                                ),
                        ),
                ),
            )
            .child(self.render_mode_switch(cx))
            .when(self.mode == Mode::Code, |this| {
                this.child(div().px_2().child(rows::action_row(
                    "show-skills",
                    Icon::from(AppIcon::Sparkles),
                    "Skills",
                    self.skills_active,
                    SidebarEvent::ShowSkills,
                    cx,
                )))
            })
            .child(div().px_2().pb_2().child(crate::demo::mark(
                "new-chat",
                rows::action_row(
                    "new-chat",
                    Icon::from(AppIcon::SquarePen),
                    new_label,
                    false,
                    SidebarEvent::NewChat,
                    cx,
                ),
            )))
            .child(
                div()
                    .px_2()
                    .pb_1()
                    // The same field as the Settings page's search: default size, search icon.
                    .child(
                        Input::new(&self.filter)
                            .prefix(Icon::new(IconName::Search))
                            .cleanable(true),
                    ),
            )
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(list),
            )
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(crate::demo::mark(
                        "show-settings",
                        rows::action_row(
                            "show-settings",
                            Icon::new(IconName::Settings),
                            "Settings",
                            self.settings_active,
                            SidebarEvent::ShowSettings,
                            cx,
                        ),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{FolderLabel, bucket, folder_labels};

    #[test]
    fn folder_labels_disambiguate_and_name_home() {
        let home = Path::new("/home/u");
        let folders = [
            Path::new("/home/u/a/demo"),
            Path::new("/tmp/demo"),
            Path::new("/home/u"),
            Path::new("/home/u/app"),
        ];
        let label = |name: &str, hint: Option<&str>| FolderLabel {
            name: name.to_owned(),
            hint: hint.map(str::to_owned),
        };
        assert_eq!(
            folder_labels(folders.into_iter(), Some(home)),
            vec![
                label("demo", Some("~/a")),
                label("demo", Some("/tmp")),
                label("~", None),
                label("app", None)
            ]
        );
    }

    use std::time::{Duration, SystemTime};

    #[test]
    fn buckets_by_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_hours(2400);
        let ago = |days: u64| now - Duration::from_secs(days * 86_400 + 60);
        assert_eq!(bucket(now, now), "Today");
        assert_eq!(bucket(ago(1), now), "Yesterday");
        assert_eq!(bucket(ago(3), now), "Previous 7 days");
        assert_eq!(bucket(ago(10), now), "Previous 30 days");
        assert_eq!(bucket(ago(60), now), "Older");
    }
}
