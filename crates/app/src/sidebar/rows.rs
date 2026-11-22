//! Sidebar rows: one-line session rows and collapsible Code folder headers.

use std::path::Path;
use std::time::SystemTime;

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use remote::HostId;

use super::{Activity, Sidebar, SidebarEvent};
use crate::config::AppConfig;
use crate::ui::QuietButton as _;
use crate::ui::{AppIcon, relative_time};

/// A session: title on the left, age on the right; rename / trash replace the age on hover.
/// `indent` is the nesting level (1 under a Code folder, 2 under a host's folder); `host` is
/// the SSH host the session lives on.
pub fn session_row(
    sidebar: &Sidebar,
    session: &SessionSummary,
    indent: u8,
    host: Option<&HostId>,
    now: SystemTime,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement + use<> {
    let active = sidebar.is_active(host, &session.path);
    let activity = sidebar.activity(host, &session.path);
    let colors = crate::theme::palette(cx);
    let spinner = crate::chat::SPINNER[sidebar.spinner_frame % crate::chat::SPINNER.len()];
    let theme = cx.theme();
    let (hover_bg, active_bg) = (theme.secondary_hover, theme.accent);
    let host = host.cloned();
    let open = SidebarEvent::Open(host.clone(), session.clone());
    let delete = SidebarEvent::Delete(host.clone(), session.clone());
    let rename = SidebarEvent::Rename(host.clone(), session.clone());
    let id = SharedString::from(format!(
        "{}{}",
        host.as_ref()
            .map_or_else(String::new, |host| format!("ssh://{}", host.0)),
        session.path.to_string_lossy()
    ));
    let status = match activity {
        Some(Activity::Working) => ", working",
        Some(Activity::NeedsYou) => ", needs your input",
        Some(Activity::Done) => ", finished",
        None => "",
    };
    let accessible_name = SharedString::from(format!("{}{status}", session.title()));
    h_flex()
        .id(ElementId::Name(id.clone()))
        .role(Role::Button)
        .aria_label(accessible_name)
        .aria_selected(active)
        .group("session-row")
        .relative()
        .w_full()
        .h(px(30.))
        .gap_2()
        .pr_2()
        .map(|this| match indent {
            0 => this.pl_2(),
            1 => this.pl(px(30.)),
            _ => this.pl(px(44.)),
        })
        .rounded_md()
        .cursor_pointer()
        .when(active, |this| this.bg(active_bg))
        .hover(|style| style.bg(hover_bg))
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(open.clone())))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .truncate()
                .text_color(theme.foreground)
                .child(session.title().to_owned()),
        )
        .child(match activity {
            Some(Activity::Working) => div()
                .flex_none()
                .w(px(14.))
                .text_sm()
                .text_center()
                .text_color(colors.accent)
                .child(spinner)
                .into_any_element(),
            other => h_flex()
                .flex_none()
                .gap_1p5()
                .when_some(other, |this, activity| {
                    let (color, tip) = match activity {
                        Activity::NeedsYou => (colors.warning, "Waiting for your answer"),
                        _ => (colors.accent, "Finished"),
                    };
                    this.child(
                        div()
                            .id(ElementId::Name(format!("dot:{id}").into()))
                            .size(px(7.))
                            .rounded_full()
                            .bg(color)
                            .tooltip(move |window, cx| {
                                component::tooltip::Tooltip::new(tip).build(window, cx)
                            }),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(relative_time(session.modified, now)),
                )
                .into_any_element(),
        })
        .child(
            h_flex()
                .absolute()
                .top_0()
                .bottom_0()
                .right_1()
                .items_center()
                .pl_2()
                .bg(if active { active_bg } else { hover_bg })
                .invisible()
                .group_hover("session-row", |style| style.visible())
                .child(
                    Button::new(ElementId::Name(format!("rename:{id}").into()))
                        .quiet(cx)
                        .xsmall()
                        .icon(Icon::from(AppIcon::SquarePen))
                        .accessibility_label("Rename")
                        .tooltip("Rename")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(rename.clone());
                        })),
                )
                .child(
                    Button::new(ElementId::Name(format!("delete:{id}").into()))
                        .quiet(cx)
                        .xsmall()
                        .icon(IconName::Delete)
                        .accessibility_label("Move to trash")
                        .tooltip("Move to trash")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(delete.clone());
                        })),
                ),
        )
}

/// A full-width action at the top of the sidebar ("Skills", "New chat"): icon and label
/// aligned with the session rows below.
pub fn action_row(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    selected: bool,
    event: SidebarEvent,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (hover_bg, active_bg) = (theme.secondary_hover, theme.accent);
    h_flex()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .aria_selected(selected)
        .w_full()
        .h(px(30.))
        .gap_2()
        .pl_2()
        .pr_2()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(theme.foreground)
        .when(selected, |this| this.bg(active_bg))
        .hover(|style| style.bg(hover_bg))
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(event.clone())))
        .child(icon.small().text_color(theme.muted_foreground))
        .child(label)
}

/// A Code folder header: chevron, folder name, session count, and "+" (new session here) on
/// hover. Clicking it collapses or expands the folder. `host` is the SSH host it is on
/// (indented under the host's header).
pub fn folder_row(
    folder: &Path,
    host: Option<&HostId>,
    label: super::FolderLabel,
    count: usize,
    open: bool,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (foreground, muted, hover_bg) = (
        theme.foreground,
        theme.muted_foreground,
        theme.secondary_hover,
    );
    let home = cx.global::<AppConfig>().home.clone();
    let full_path: SharedString = match host {
        Some(_) => folder.display().to_string().into(),
        None => crate::ui::short_path(folder, home.as_deref()).into(),
    };
    let toggle = match host {
        Some(host) => crate::ui_state::remote_key(&host.0, Some(folder)),
        None => folder.to_path_buf(),
    };
    let new_in = SidebarEvent::NewInFolder(host.cloned(), folder.to_path_buf());
    let delete = SidebarEvent::DeleteFolder(host.cloned(), folder.to_path_buf());
    let indented = host.is_some();
    let id = SharedString::from(format!("folder:{}", toggle.display()));
    let accessible_name = SharedString::from(format!(
        "Folder {}, {count} session{}",
        label.name,
        if count == 1 { "" } else { "s" }
    ));
    h_flex()
        .id(ElementId::Name(id.clone()))
        .role(Role::Button)
        .aria_label(accessible_name)
        .aria_expanded(open)
        .group("folder-row")
        .relative()
        .w_full()
        .h(px(30.))
        .mt_1()
        .gap_1p5()
        .pr_2()
        .map(|this| {
            if indented {
                this.pl(px(22.))
            } else {
                this.pl_2()
            }
        })
        .rounded_md()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg))
        .tooltip(move |window, cx| {
            component::tooltip::Tooltip::new(full_path.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(&toggle, cx)))
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(muted),
        )
        .child(Icon::new(IconName::Folder).small().text_color(muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground)
                .child(label.name),
        )
        .when_some(label.hint, |this, hint| {
            this.child(
                div()
                    .flex_none()
                    .max_w(px(120.))
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(hint),
            )
        })
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(muted)
                .child(count.to_string()),
        )
        .child(
            h_flex()
                .absolute()
                .top_0()
                .bottom_0()
                .right_1()
                .items_center()
                .pl_2()
                .bg(hover_bg)
                .invisible()
                .group_hover("folder-row", |style| style.visible())
                .child(
                    Button::new(ElementId::Name(format!("new-in:{id}").into()))
                        .quiet(cx)
                        .xsmall()
                        .icon(IconName::Plus)
                        .accessibility_label("New session in this folder")
                        .tooltip("New session in this folder")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(new_in.clone());
                        })),
                )
                .child(
                    Button::new(ElementId::Name(format!("delete-folder:{id}").into()))
                        .quiet(cx)
                        .xsmall()
                        .icon(IconName::Delete)
                        .accessibility_label("Move this folder's sessions to the trash")
                        .tooltip("Move this folder's sessions to the trash")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(delete.clone());
                        })),
                ),
        )
}

/// An SSH host's header in Code mode: chevron, status dot, name, session count; on hover
/// "+" (new session on the host) and refresh, or "Connect" when it is not connected.
pub fn host_row(
    host: &HostId,
    label: String,
    status: &crate::hosts::HostStatus,
    count: usize,
    open: bool,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (foreground, muted, hover_bg) = (
        theme.foreground,
        theme.muted_foreground,
        theme.secondary_hover,
    );
    let key = host.0.clone();
    let tooltip: SharedString =
        format!("{label}: {}", crate::remote_ui::status_text(status)).into();
    let (toggle, new_on, refresh) = (host.clone(), host.clone(), host.clone());
    let connected = status.is_connected();
    h_flex()
        .id(ElementId::Name(format!("host:{key}").into()))
        .role(Role::Button)
        .aria_label(tooltip.clone())
        .aria_expanded(open)
        .group("host-row")
        .relative()
        .w_full()
        .h(px(30.))
        .mt_2()
        .gap_1p5()
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .hover(|style| style.bg(hover_bg))
        .tooltip(move |window, cx| {
            component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_host(&toggle, cx)))
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(muted),
        )
        .child(Icon::new(IconName::Globe).small().text_color(muted))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground)
                .child(label),
        )
        .child(crate::remote_ui::status_dot(Some(status), cx))
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(muted)
                .child(count.to_string()),
        )
        .child(
            h_flex()
                .absolute()
                .top_0()
                .bottom_0()
                .right_1()
                .items_center()
                .pl_2()
                .bg(hover_bg)
                .invisible()
                .group_hover("host-row", |style| style.visible())
                .when(connected, |this| {
                    this.child(
                        Button::new(ElementId::Name(format!("refresh-host:{key}").into()))
                            .quiet(cx)
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh sessions")
                            .tooltip("Refresh sessions")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                let host = refresh.clone();
                                crate::hosts::hosts(cx)
                                    .update(cx, |hosts, cx| hosts.refresh_sessions(&host, cx));
                            })),
                    )
                })
                .child(
                    Button::new(ElementId::Name(format!("new-on:{key}").into()))
                        .quiet(cx)
                        .xsmall()
                        .icon(if connected {
                            IconName::Plus
                        } else {
                            IconName::Globe
                        })
                        .accessibility_label(if connected {
                            "New session on this host"
                        } else {
                            "Connect"
                        })
                        .tooltip(if connected {
                            "New session on this host"
                        } else {
                            "Connect"
                        })
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(SidebarEvent::ConnectHost(new_on.clone()));
                        })),
                ),
        )
}
