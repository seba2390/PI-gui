//! What deep search found when it checked its report: a symbol below the report, and the dialog
//! it opens.

use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::{AboutReport, CheckItem, ClaimCounts, Problem, ProblemKind};

use crate::theme::palette;
use crate::ui::QuietButton as _;

const WIDTH: Pixels = px(640.);
/// Of the window's height, at most.
const HEIGHT: f32 = 0.7;

/// Opens what is known about a report.
pub fn symbol(id: u64, check: &CheckItem, cx: &App) -> impl IntoElement + use<> {
    let (icon, label) = if check.checked {
        (IconName::Info, "How this report was checked")
    } else {
        (IconName::TriangleAlert, "This report was not checked")
    };
    let color = if check.checked {
        cx.theme().muted_foreground
    } else {
        palette(cx).warning
    };
    let check = check.clone();
    Button::new(("report-check", id))
        .quiet(cx)
        .xsmall()
        .icon(Icon::new(icon).text_color(color))
        .accessibility_label(label)
        .tooltip(label)
        .on_click(move |_, window, cx| open(check.clone(), window, cx))
}

pub fn open(check: CheckItem, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, window, cx| {
        let title = if check.checked {
            "How this report was checked"
        } else {
            "This report was not checked"
        };
        dialog.title(title).w(WIDTH).child(
            // As tall as what is in it, up to where it scrolls.
            v_flex()
                .debug_selector(|| "report-check".into())
                .max_h(window.viewport_size().height * HEIGHT)
                .overflow_y_scrollbar()
                .child(match &check.about {
                    Some(about) => render_about(about, cx).into_any_element(),
                    None => div()
                        .pb_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(check.text.clone())
                        .into_any_element(),
                }),
        )
    });
}

fn render_about(about: &AboutReport, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let effort = deepsearch::effort::Effort::parse(&about.effort)
        .map_or_else(|| about.effort.clone(), |effort| effort.label().to_owned());
    let count = |value: u32, label: &str| (value.to_string(), label.to_owned());
    let tiles = [
        (effort, "effort".to_owned()),
        count(about.searches, "searches"),
        count(about.pages, "pages read"),
        count(about.sites, "different sites"),
    ]
    .map(|(value, label)| {
        v_flex()
            .flex_1()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .bg(theme.muted.opacity(0.5))
            .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(value))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
    });
    let groups = ProblemKind::ALL
        .into_iter()
        .map(|kind| {
            let of_kind = about.problems.iter().filter(|problem| problem.kind == kind);
            (kind, of_kind.collect::<Vec<_>>())
        })
        .filter(|(_, problems)| !problems.is_empty())
        .enumerate()
        .map(|(group, (kind, problems))| render_group(group, kind, &problems, cx))
        .collect::<Vec<_>>();
    v_flex()
        .w_full()
        .gap_4()
        .pb_2()
        .pr_3()
        .child(h_flex().w_full().gap_2().children(tiles))
        .children(about.notes.iter().map(|note| {
            h_flex()
                .items_start()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(div().mt(px(2.)).child(Icon::new(IconName::Info).small()))
                .child(div().flex_1().min_w_0().child(note.clone()))
        }))
        .when(about.links.total > 0, |this| {
            let links = &about.links;
            let state = match (links.broken, links.unread) {
                (0, 0) => "all of them read".to_owned(),
                (broken, 0) => format!("{broken} broken"),
                (0, unread) => format!("{unread} not read"),
                (broken, unread) => format!("{broken} broken, {unread} not read"),
            };
            this.child(heading("Links", format!("{}, {state}", links.total), cx))
        })
        .children(
            about
                .claims
                .as_ref()
                .map(|claims| render_claims(claims, cx)),
        )
        .children(groups)
}

fn heading(title: &str, detail: String, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .items_baseline()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(detail),
        )
}

/// How many claims are backed: a bar, and what its colors are.
fn render_claims(claims: &ClaimCounts, cx: &App) -> impl IntoElement + use<> {
    let colors = palette(cx);
    let parts: Vec<(u32, &str, Hsla)> = [
        (claims.backed, "backed by their source", colors.accent),
        (claims.partly, "only partly", colors.warning),
        (claims.unbacked, "not backed", colors.error),
        (
            claims.unsourced,
            "without a source",
            colors.error.opacity(0.5),
        ),
        (
            claims.unchecked,
            "not checked",
            cx.theme().muted_foreground.opacity(0.4),
        ),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .collect();
    let bar = parts.iter().map(|(count, _, color)| {
        // Wide enough to be seen.
        div()
            .h_full()
            .flex_grow(f32::from(u16::try_from(*count).unwrap_or(u16::MAX)))
            .flex_basis(px(0.))
            .min_w(px(3.))
            .bg(*color)
    });
    let legend = parts.iter().map(|(count, label, color)| {
        h_flex()
            .gap_1p5()
            .items_center()
            .text_sm()
            .child(div().size(px(8.)).flex_none().rounded_full().bg(*color))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(count.to_string()),
            )
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child((*label).to_owned()),
            )
    });
    v_flex()
        .gap_2()
        .child(heading("Claims", claims.total.to_string(), cx))
        .when(!parts.is_empty(), |this| {
            this.child(
                h_flex()
                    .w_full()
                    .h(px(8.))
                    .gap(px(2.))
                    .rounded_full()
                    .overflow_hidden()
                    .children(bar),
            )
            .child(h_flex().flex_wrap().gap_x_4().gap_y_1().children(legend))
        })
}

fn render_group(
    group: usize,
    kind: ProblemKind,
    problems: &[&Problem],
    cx: &App,
) -> impl IntoElement + use<> {
    let border = cx.theme().border;
    let rows = problems.iter().enumerate().map(|(index, problem)| {
        // For a link, the link itself says it.
        let is_link = problem.urls.first() == Some(&problem.text);
        v_flex()
            .w_full()
            .gap_1()
            .px_3()
            .py_2()
            .when(index > 0, |this| this.border_t_1().border_color(border))
            .when(!is_link, |this| {
                this.child(div().text_sm().child(problem.text.clone()))
            })
            .when(!problem.why.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(problem.why.clone()),
                )
            })
            .when(!problem.urls.is_empty(), |this| {
                this.child(h_flex().flex_wrap().gap_x_3().gap_y_0p5().children(
                    problem.urls.iter().enumerate().map(|(link, url)| {
                        let id = group * 1_000_000 + index * 1_000 + link;
                        page_link(id, url, is_link, cx)
                    }),
                ))
            })
    });
    v_flex()
        .w_full()
        .gap_1p5()
        .child(heading(kind.label(), problems.len().to_string(), cx))
        .child(
            v_flex()
                .w_full()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(border)
                .children(rows),
        )
}

/// A link to a page: by its site, or (`whole`) its address.
fn page_link(id: usize, url: &str, whole: bool, cx: &App) -> impl IntoElement + use<> {
    let short = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let label = if whole {
        short
    } else {
        short.split('/').next().unwrap_or(short)
    };
    let url = url.to_owned();
    div()
        .id(("report-link", id))
        .text_xs()
        .text_color(palette(cx).accent)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(label.to_owned())
        .tooltip({
            let url = url.clone();
            move |window, cx| Tooltip::new(url.clone()).build(window, cx)
        })
        .on_click(move |_, _, cx| cx.open_url(&url))
}
