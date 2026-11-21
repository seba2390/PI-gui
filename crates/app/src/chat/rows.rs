//! Rendering of transcript rows: user prompts, assistant blocks (thinking, text,
//! tool calls), notices, and per-turn "Worked for" summaries.

use std::fmt::Write as _;
use std::time::Instant;

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::tool::{self, DiffKind, PathContext};
use transcript::{
    AssistantItem, BlockKind, ItemKind, NoticeKind, RunStatus, ThinkingBlock, ToolBlock,
    ToolStatus, TranscriptItem, format_duration,
};

use super::{BlockKey, CONTENT_MAX_WIDTH, ChatView, markdown_view};
use crate::config::AppConfig;
use crate::theme::{Palette, palette};
use crate::ui::AppIcon;

/// Collapsed tool output shows at most this many lines.
const PREVIEW_LINES: usize = 3;
/// Expanded tool output is capped to keep rows cheap; the rest is summarized.
const MAX_OUTPUT_LINES: usize = 400;
/// Spinner glyphs, played forwards then backwards so the star "breathes"; one per chat tick.
const SPINNER: [&str; 10] = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

/// Everything a row needs besides the item itself.
struct RowContext<'a> {
    chat: &'a ChatView,
    weak: WeakEntity<ChatView>,
    row: usize,
    palette: Palette,
    paths: PathContext<'a>,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    mono: SharedString,
}

pub fn render_row(
    weak: &WeakEntity<ChatView>,
    row: usize,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(chat) = weak.upgrade() else {
        return div().into_any_element();
    };
    let chat = chat.read(cx);
    let session = chat.session.read(cx);
    let config = cx.global::<AppConfig>();
    let theme = cx.theme();
    let context = RowContext {
        chat,
        weak: weak.clone(),
        row,
        palette: palette(cx),
        paths: PathContext {
            cwd: Some(session.cwd()),
            home: config.home.as_deref(),
        },
        foreground: theme.foreground,
        muted: theme.muted_foreground,
        border: theme.border,
        mono: theme.mono_font_family.clone(),
    };

    let items = session.transcript().items();
    let content = match items.get(row) {
        Some(item) => render_item(item, &context, theme.muted),
        None => render_working(session.transcript().status(), &context),
    };
    // Tight spacing inside a turn, more air around prompts and turn ends.
    let is_user = matches!(
        items.get(row).map(|item| &item.kind),
        Some(ItemKind::User(_))
    );
    let after_summary = row > 0
        && matches!(
            items.get(row - 1).map(|item| &item.kind),
            Some(ItemKind::TurnSummary(_))
        );
    let (top, bottom) = match (row, is_user) {
        (0, _) => (px(24.), px(8.)),
        (_, true) if after_summary => (px(20.), px(8.)),
        (_, true) => (px(12.), px(8.)),
        _ => (px(6.), px(6.)),
    };

    h_flex()
        .w_full()
        .justify_center()
        .px_4()
        .child(
            div()
                .w_full()
                .max_w(CONTENT_MAX_WIDTH)
                .pt(top)
                .pb(bottom)
                .child(content),
        )
        .into_any_element()
}

fn render_item(item: &TranscriptItem, cx: &RowContext<'_>, bubble: Hsla) -> AnyElement {
    match &item.kind {
        ItemKind::User(user) => h_flex()
            .w_full()
            .justify_end()
            .child(
                v_flex()
                    .min_w_0()
                    .max_w(relative(0.85))
                    .px_4()
                    .py_2p5()
                    .rounded_xl()
                    .bg(bubble)
                    .text_color(cx.foreground)
                    .gap_1()
                    .child(
                        match cx.chat.markdown(BlockKey {
                            item: item.id,
                            block: 0,
                        }) {
                            Some(state) => markdown_view(state).into_any_element(),
                            None => div().child(user.text.clone()).into_any_element(),
                        },
                    )
                    .when(user.image_count > 0, |this| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.muted)
                                .child(Icon::from(AppIcon::Image).xsmall())
                                .child(plural(user.image_count, "image")),
                        )
                    }),
            )
            .into_any_element(),
        ItemKind::Assistant(assistant) => render_assistant(item, assistant, cx),
        ItemKind::Bash {
            command,
            output,
            exit_code,
        } => v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .font_family(cx.mono.clone())
                    .text_sm()
                    .child(div().text_color(cx.palette.accent).child("$"))
                    .child(div().text_color(cx.foreground).child(command.clone()))
                    .when_some(exit_code.filter(|code| *code != 0), |this, code| {
                        this.child(
                            div()
                                .text_color(cx.palette.error)
                                .child(format!("exit {code}")),
                        )
                    }),
            )
            .when(!output.trim().is_empty(), |this| {
                this.child(output_box(("bash-out", item.id.0), output, cx))
            })
            .into_any_element(),
        ItemKind::Notice(notice) => {
            let (icon, color): (Icon, Hsla) = match notice.kind {
                NoticeKind::Error => (IconName::CircleX.into(), cx.palette.error),
                NoticeKind::Warning => (IconName::TriangleAlert.into(), cx.palette.warning),
                NoticeKind::Compaction => (IconName::Info.into(), cx.muted),
                NoticeKind::BranchSummary => (AppIcon::GitBranch.into(), cx.muted),
                NoticeKind::Info => (IconName::Info.into(), cx.muted),
            };
            h_flex()
                .items_start()
                .gap_2()
                .text_sm()
                .text_color(color)
                .child(div().mt(px(2.)).child(icon.small()))
                .child(div().flex_1().min_w_0().child(notice.text.clone()))
                .into_any_element()
        }
        ItemKind::TurnSummary(summary) => {
            let label = if summary.aborted {
                format!("Interrupted after {}", format_duration(summary.duration))
            } else {
                format!("Worked for {}", format_duration(summary.duration))
            };
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(cx.palette.subtle)
                .child(div().text_color(cx.palette.accent).child("✻"))
                .child(label)
                .into_any_element()
        }
    }
}

fn render_assistant(
    item: &TranscriptItem,
    assistant: &AssistantItem,
    cx: &RowContext<'_>,
) -> AnyElement {
    let mut column = v_flex().w_full().gap_3();
    for (index, block) in assistant.blocks.iter().enumerate() {
        let key = BlockKey {
            item: item.id,
            block: index,
        };
        column = column.child(match &block.kind {
            BlockKind::Thinking(thinking) => render_thinking(key, thinking, cx),
            BlockKind::Text(text) => match cx.chat.markdown(key) {
                Some(state) => div()
                    .w_full()
                    .text_color(cx.foreground)
                    .child(markdown_view(state))
                    .into_any_element(),
                None => div().child(text.text.clone()).into_any_element(),
            },
            BlockKind::Tool(tool) => render_tool(key, tool, cx),
        });
    }
    if let Some(error) = &assistant.error
        && assistant.stop_reason != pi_rpc::protocol::StopReason::Aborted
    {
        column = column.child(
            h_flex()
                .gap_2()
                .text_sm()
                .text_color(cx.palette.error)
                .child(Icon::new(IconName::CircleX).small())
                .child(error.clone()),
        );
    }
    column.into_any_element()
}

fn element_id(prefix: &'static str, key: BlockKey) -> ElementId {
    ElementId::NamedInteger(prefix.into(), key.item.0 * 1_000 + key.block as u64)
}

fn toggle_handler(
    cx: &RowContext<'_>,
    key: BlockKey,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let (weak, row) = (cx.weak.clone(), cx.row);
    move |_, _, app| {
        let _ = weak.update(app, |chat, cx| chat.toggle_block(key, row, cx));
    }
}

fn chevron(expanded: bool) -> Icon {
    Icon::new(if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
    .xsmall()
}

fn render_thinking(key: BlockKey, thinking: &ThinkingBlock, cx: &RowContext<'_>) -> AnyElement {
    let expanded = cx.chat.is_expanded(key, true);
    let label = if !thinking.done {
        let elapsed = thinking.started_at.map(|start| start.elapsed());
        match elapsed {
            Some(elapsed) if elapsed.as_secs() > 0 => {
                format!("Thinking… {}", format_duration(elapsed))
            }
            _ => "Thinking…".to_owned(),
        }
    } else if thinking.redacted {
        "Thinking (redacted)".to_owned()
    } else if let Some(duration) = thinking.duration {
        format!("Thought for {}", format_duration(duration))
    } else {
        "Thought".to_owned()
    };
    let color = if thinking.done {
        cx.muted
    } else {
        cx.palette.accent
    };
    v_flex()
        .gap_1p5()
        .child(
            h_flex()
                .id(element_id("thinking", key))
                .gap_1p5()
                .text_sm()
                .text_color(color)
                .cursor_pointer()
                .hover(|style| style.text_color(cx.foreground))
                .on_click(toggle_handler(cx, key))
                .child(Icon::from(AppIcon::Brain).small())
                .child(label)
                .child(chevron(expanded)),
        )
        .when(expanded && !thinking.text.trim().is_empty(), |this| {
            this.child(
                div()
                    .ml(px(7.))
                    .pl_3()
                    .border_l_1()
                    .border_color(cx.border)
                    .text_sm()
                    .text_color(cx.muted)
                    .child(thinking.text.trim().to_owned()),
            )
        })
        .into_any_element()
}

fn render_tool(key: BlockKey, tool: &ToolBlock, cx: &RowContext<'_>) -> AnyElement {
    let expanded = cx.chat.is_expanded(key, false);
    let header = tool::header(tool, cx.paths);
    let dot = match tool.status {
        ToolStatus::Done => cx.palette.success,
        ToolStatus::Failed => cx.palette.error,
        ToolStatus::Cancelled => cx.palette.subtle,
        ToolStatus::Streaming | ToolStatus::Pending | ToolStatus::Running => cx.palette.accent,
    };
    let diff = tool::diff_lines(tool);

    let summary: Option<SharedString> = match tool.status {
        ToolStatus::Streaming | ToolStatus::Pending => Some("Preparing…".into()),
        ToolStatus::Running if tool.output.is_empty() => Some("Running…".into()),
        ToolStatus::Cancelled => Some("Interrupted".into()),
        _ if expanded => None,
        _ => tool::result_summary(tool).map(Into::into).or_else(|| {
            (!tool.output.trim().is_empty() && diff.is_none()).then(|| preview(&tool.output).into())
        }),
    };

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .id(element_id("tool", key))
                .items_start()
                .gap_2()
                .cursor_pointer()
                .on_click(toggle_handler(cx, key))
                .child(
                    div()
                        .mt(px(7.))
                        .size(px(7.))
                        .flex_none()
                        .rounded_full()
                        .bg(dot),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .text_sm()
                        .items_start()
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.foreground)
                                .child(header.title),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(cx.mono.clone())
                                .text_color(cx.muted)
                                .child(header.detail),
                        ),
                )
                .when_some(tool.duration_ms.filter(|ms| *ms >= 1_000), |this, ms| {
                    this.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.palette.subtle)
                            .child(format_duration(std::time::Duration::from_millis(ms))),
                    )
                }),
        )
        .when_some(summary, |this, summary| {
            let color = if tool.status == ToolStatus::Failed {
                cx.palette.error
            } else {
                cx.muted
            };
            this.child(
                h_flex()
                    .items_start()
                    .pl(px(15.))
                    .gap_2()
                    .text_sm()
                    .text_color(color)
                    .child(div().text_color(cx.palette.subtle).child("⎿"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(cx.mono.clone())
                            .text_xs()
                            .child(summary),
                    ),
            )
        })
        .when(expanded, |this| {
            let body = match &diff {
                Some(lines) => diff_box(
                    ("tool-diff", key.item.0 * 1_000 + key.block as u64),
                    lines,
                    cx,
                ),
                None if !tool.output.trim().is_empty() => output_box(
                    ("tool-out", key.item.0 * 1_000 + key.block as u64),
                    &tool.output,
                    cx,
                ),
                None => div()
                    .pl(px(15.))
                    .text_xs()
                    .text_color(cx.muted)
                    .child("(no output)")
                    .into_any_element(),
            };
            this.child(div().pl(px(15.)).child(body))
        })
        .into_any_element()
}

/// First lines of `output` plus a "+N lines" note.
fn preview(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let shown = lines
        .iter()
        .take(PREVIEW_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > PREVIEW_LINES {
        format!(
            "{shown}\n… +{} lines (ctrl+o to expand)",
            lines.len() - PREVIEW_LINES
        )
    } else {
        shown
    }
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn output_box(id: impl Into<ElementId>, output: &str, cx: &RowContext<'_>) -> AnyElement {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let mut text = lines
        .iter()
        .take(MAX_OUTPUT_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > MAX_OUTPUT_LINES {
        let _ = write!(text, "\n… {} more lines", lines.len() - MAX_OUTPUT_LINES);
    }
    div()
        .id(id)
        .w_full()
        .max_h(px(320.))
        .overflow_y_scroll()
        .p_2()
        .rounded_md()
        .bg(cx.palette.code_bg)
        .border_1()
        .border_color(cx.border)
        .font_family(cx.mono.clone())
        .text_xs()
        .text_color(cx.foreground)
        .child(text)
        .into_any_element()
}

fn diff_box(id: impl Into<ElementId>, lines: &[tool::DiffLine], cx: &RowContext<'_>) -> AnyElement {
    let gutter = lines
        .iter()
        .filter_map(|line| line.line_number)
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    let rows =
        lines.iter().take(MAX_OUTPUT_LINES).map(|line| {
            let (sign, fg, bg) = match line.kind {
                DiffKind::Added => ("+", cx.palette.added, Some(cx.palette.added_bg)),
                DiffKind::Removed => ("-", cx.palette.removed, Some(cx.palette.removed_bg)),
                DiffKind::Context => (" ", cx.foreground, None),
                DiffKind::Gap => ("", cx.palette.subtle, None),
            };
            let number = line.line_number.map_or_else(String::new, |n| n.to_string());
            h_flex()
                .w_full()
                .px_2()
                .when_some(bg, |this, bg| this.bg(bg))
                .child(
                    div()
                        .w(px(8.
                            * f32::from(u16::try_from(gutter).unwrap_or(u16::MAX))
                            + 8.))
                        .flex_none()
                        .text_color(cx.palette.subtle)
                        .child(format!("{number:>gutter$}")),
                )
                .child(div().w(px(14.)).flex_none().text_color(fg).child(sign))
                .child(div().flex_1().min_w_0().text_color(fg).child(
                    if line.kind == DiffKind::Gap {
                        "⋯".to_owned()
                    } else {
                        line.text.clone()
                    },
                ))
        });
    div()
        .id(id)
        .w_full()
        .max_h(px(360.))
        .overflow_y_scroll()
        .py_1()
        .rounded_md()
        .bg(cx.palette.code_bg)
        .border_1()
        .border_color(cx.border)
        .font_family(cx.mono.clone())
        .text_xs()
        .children(rows)
        .into_any_element()
}

fn render_working(status: &RunStatus, cx: &RowContext<'_>) -> AnyElement {
    let (label, since): (String, Option<Instant>) = match status {
        RunStatus::Working { since } => ("Working…".to_owned(), Some(*since)),
        RunStatus::Compacting => ("Compacting conversation…".to_owned(), None),
        RunStatus::Retrying {
            attempt,
            max_attempts,
            ..
        } => (
            format!("Retrying (attempt {attempt}/{max_attempts})…"),
            None,
        ),
        RunStatus::Idle => ("Working…".to_owned(), None),
    };
    let elapsed = since.map(|since| since.elapsed());
    let frame = cx.chat.spinner_frame();
    let glyph = SPINNER[frame % SPINNER.len()];
    // The label gently pulses (about one breath per 2 s) alongside the spinner.
    #[allow(clippy::cast_precision_loss)]
    let phase = (frame % 25) as f32 / 25.0 * std::f32::consts::TAU;
    let label_color = cx.palette.accent.opacity(0.7 + 0.3 * phase.cos());
    h_flex()
        .gap_2()
        .text_sm()
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .text_color(cx.palette.accent)
                .child(glyph),
        )
        .child(div().text_color(label_color).child(label))
        .when_some(elapsed, |this, elapsed| {
            this.child(
                div()
                    .text_color(cx.muted)
                    .child(format!("{} · esc to interrupt", format_duration(elapsed))),
            )
        })
        .into_any_element()
}
