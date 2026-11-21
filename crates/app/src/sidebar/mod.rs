//! The left sidebar: new chat, and previous sessions grouped by recency.
//!
//! The list comes from pi's session files (read-only, via `pi-sessions`) and is
//! refreshed by a filesystem watcher, so sessions created by the terminal `pi`
//! appear here too.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use crate::config::AppConfig;
use crate::theme::palette;
use crate::ui::{AppIcon, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, WindowDrag, relative_time};

#[derive(Debug, Clone)]
pub enum SidebarEvent {
    Open(SessionSummary),
    NewChat,
    Collapse,
    Delete(SessionSummary),
    Rename(SessionSummary),
}

pub struct Sidebar {
    sessions: Vec<SessionSummary>,
    active: Option<PathBuf>,
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
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search chats"));
        let subscriptions = vec![cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        let mut this = Self {
            sessions: Vec::new(),
            active: None,
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

    pub fn set_active(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.active != path {
            self.active = path;
            cx.notify();
        }
    }

    /// Rescans pi's session directory in the background.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = cx.global::<AppConfig>().sessions_dir.clone() else {
            self.loaded = true;
            return;
        };
        let previous = self.sessions.clone();
        self._refresh = cx.spawn(async move |this, cx| {
            let sessions = cx
                .background_executor()
                .spawn(async move { pi_sessions::scan_reusing(&dir, &previous) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sessions = sessions;
                this.loaded = true;
                cx.notify();
            });
        });
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

    fn visible_sessions(&self, cx: &App) -> Vec<&SessionSummary> {
        let query = self.filter.read(cx).value().to_lowercase();
        self.sessions
            .iter()
            .filter(|session| {
                query.is_empty()
                    || session.title().to_lowercase().contains(&query)
                    || session
                        .cwd
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }

    fn render_row(
        &self,
        session: &SessionSummary,
        now: SystemTime,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active.as_deref() == Some(session.path.as_path());
        let theme = cx.theme();
        let (open, delete, rename) = (session.clone(), session.clone(), session.clone());
        let id = SharedString::from(session.path.to_string_lossy().into_owned());
        h_flex()
            .id(ElementId::Name(id.clone()))
            .group("session-row")
            .w_full()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .when(active, |this| this.bg(theme.accent))
            .hover(|style| style.bg(theme.secondary_hover))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(SidebarEvent::Open(open.clone()))))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(session.title().to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · {}",
                                session.folder_name(),
                                relative_time(session.modified, now)
                            )),
                    ),
            )
            .child(
                h_flex()
                    .invisible()
                    .group_hover("session-row", |style| style.visible())
                    .child(
                        Button::new(ElementId::Name(format!("rename:{id}").into()))
                            .ghost()
                            .xsmall()
                            .icon(Icon::from(AppIcon::SquarePen))
                            .tooltip("Rename")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.emit(SidebarEvent::Rename(rename.clone()));
                            })),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("delete:{id}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Move to trash")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.emit(SidebarEvent::Delete(delete.clone()));
                            })),
                    ),
            )
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
        let now = SystemTime::now();
        let colors = palette(cx);
        let muted = cx.theme().muted_foreground;
        let sessions = self.visible_sessions(cx);
        let is_empty = sessions.is_empty();

        let mut list = v_flex().w_full().gap_0p5().px_2().pb_3();
        let mut current = "";
        for session in sessions {
            let group = bucket(session.modified, now);
            if group != current {
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
            list = list.child(self.render_row(session, now, cx));
        }
        if is_empty && self.loaded {
            list = list.child(
                div()
                    .px_2()
                    .pt_4()
                    .text_sm()
                    .text_color(muted)
                    .child("No chats yet"),
            );
        }

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
                                .ghost()
                                .small()
                                .icon(IconName::PanelLeftClose)
                                .tooltip("Hide sidebar (⌘B)")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::Collapse)),
                                ),
                        ),
                ),
            )
            .child(
                div().px_2().pb_2().child(
                    Button::new("new-chat")
                        .ghost()
                        .w_full()
                        .justify_start()
                        .icon(Icon::from(AppIcon::SquarePen))
                        .label("New chat")
                        .tooltip("New chat (⌘N)")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::NewChat))),
                ),
            )
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .child(Input::new(&self.filter).small().cleanable(true)),
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
    }
}

#[cfg(test)]
mod tests {
    use super::bucket;
    use std::time::{Duration, SystemTime};

    #[test]
    fn buckets_by_age() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400);
        let ago = |days: u64| now - Duration::from_secs(days * 86_400 + 60);
        assert_eq!(bucket(now, now), "Today");
        assert_eq!(bucket(ago(1), now), "Yesterday");
        assert_eq!(bucket(ago(3), now), "Previous 7 days");
        assert_eq!(bucket(ago(10), now), "Previous 30 days");
        assert_eq!(bucket(ago(60), now), "Older");
    }
}
