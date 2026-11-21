//! Sidebar rows: one-line session rows and collapsible Code folder headers.

use std::path::Path;
use std::time::SystemTime;

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use super::{Sidebar, SidebarEvent};
use crate::config::AppConfig;
use crate::ui::QuietButton as _;
use crate::ui::{AppIcon, relative_time};

/// A session: title on the left, age on the right; rename / trash replace the age on hover.
/// `indented` rows sit under a Code folder header.
pub fn session_row(
    sidebar: &Sidebar,
    session: &SessionSummary,
    indented: bool,
    now: SystemTime,
    cx: &mut Context<Sidebar>,
) -> impl IntoElement + use<> {
    let active = sidebar.active.as_deref() == Some(session.path.as_path());
    let theme = cx.theme();
    let (hover_bg, active_bg) = (theme.secondary_hover, theme.accent);
    let (open, delete, rename) = (session.clone(), session.clone(), session.clone());
    let id = SharedString::from(session.path.to_string_lossy().into_owned());
    h_flex()
        .id(ElementId::Name(id.clone()))
        .group("session-row")
        .relative()
        .w_full()
        .h(px(30.))
        .gap_2()
        .pr_2()
        .map(|this| {
            if indented {
                this.pl(px(30.))
            } else {
                this.pl_2()
            }
        })
        .rounded_md()
        .cursor_pointer()
        .when(active, |this| this.bg(active_bg))
        .hover(|style| style.bg(hover_bg))
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SidebarEvent::Open(open.clone()))))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .truncate()
                .text_color(theme.foreground)
                .child(session.title().to_owned()),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(relative_time(session.modified, now)),
        )
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
                        .tooltip("Rename")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(SidebarEvent::Rename(rename.clone()));
                        })),
                )
                .child(
                    Button::new(ElementId::Name(format!("delete:{id}").into()))
                        .quiet(cx)
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

/// A Code folder header: chevron, folder name, session count, and "+" (new session here) on
/// hover. Clicking it collapses or expands the folder.
pub fn folder_row(
    folder: &Path,
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
    let full_path: SharedString = crate::ui::short_path(folder, home.as_deref()).into();
    let (toggle, new_in) = (folder.to_path_buf(), folder.to_path_buf());
    let id = SharedString::from(format!("folder:{}", folder.display()));
    h_flex()
        .id(ElementId::Name(id.clone()))
        .group("folder-row")
        .relative()
        .w_full()
        .h(px(30.))
        .mt_1()
        .gap_1p5()
        .px_2()
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
                        .tooltip("New session in this folder")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.emit(SidebarEvent::NewInFolder(new_in.clone()));
                        })),
                ),
        )
}
