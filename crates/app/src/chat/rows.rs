//! Rendering of transcript rows: user prompts, assistant blocks (thinking, text,
//! tool calls), notices, and per-turn "Worked for" summaries.

use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::explore::{Group, RowRole};
use transcript::search::Field;
use transcript::tool::{self, DiffKind, PathContext, ToolKind};
use transcript::{
    AssistantItem, BlockKind, ItemKind, NestedEntry, NoticeKind, RunStatus, ThinkingBlock,
    ToolBlock, ToolStatus, TranscriptItem, format_duration,
};

use super::images;
use super::session::{AfterFork, PiSession};
use super::{BlockKey, CONTENT_MAX_WIDTH, ChatView, markdown_view};
use crate::code_highlight::{self, LineStyles, Side};
use crate::config::AppConfig;
use crate::theme::{Palette, palette};
use crate::ui::{AppIcon, CopyButton, QuietButton as _};

/// Text size of tool calls, thinking and "Explored" rows: a step below the model's prose,
/// so its answers read as the main voice.
const ACTIVITY_TEXT: Rems = rems(0.8125);
/// How far a run of activity sits right of its rail line.
const RAIL_INDENT: Pixels = px(14.);
/// Extra space where the model's text meets a run of activity (rows inside a run sit
/// close together).
const SECTION_GAP: Pixels = px(8.);

/// Collapsed tool output shows at most this many lines.
const PREVIEW_LINES: usize = 3;
/// Expanded tool output is capped to keep rows cheap (at `search::MAX_OUTPUT_LINES`, which is
/// what find searches); the rest is summarized.
use transcript::search::{self, MAX_OUTPUT_LINES};

/// Everything a row needs besides the item itself.
struct RowContext<'a> {
    chat: &'a ChatView,
    items: &'a [TranscriptItem],
    session: Entity<PiSession>,
    /// Nothing is in flight, so the conversation can be branched.
    idle: bool,
    weak: WeakEntity<ChatView>,
    row: usize,
    palette: Palette,
    paths: PathContext<'a>,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    mono: SharedString,
    /// Syntax colors (the user's Code colors) for edits and files shown in tool rows.
    highlight: Arc<HighlightTheme>,
    app: &'a App,
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
    let items = session.transcript().items();
    let context = RowContext {
        chat,
        items,
        session: chat.session.clone(),
        idle: !session.is_busy(),
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
        highlight: theme.highlight_theme.clone(),
        app: cx,
    };

    let content = match (items.get(row), chat.role(row)) {
        // Folded into the "Explored" group of an earlier row.
        (Some(_), Some(RowRole::Hidden)) => return div().into_any_element(),
        (Some(_), Some(RowRole::Group(group))) => render_group(group, &context),
        (Some(item), _) => render_item(item, &context, theme.muted, cx),
        (None, _) => render_working(session.transcript().status(), &context),
    };
    // Rows of only activity (and "Explored" groups) sit on one continuous rail; text gets more
    // room where it meets them.
    let activity = |row: usize| match (items.get(row), chat.role(row)) {
        (Some(_), Some(RowRole::Group(_))) => true,
        (Some(item), _) => is_activity(item),
        (None, _) => false,
    };
    let on_rail = activity(row);
    // The row above that shows something (folded rows render nothing).
    let previous = (0..row)
        .rev()
        .find(|ix| !matches!(chat.role(*ix), Some(RowRole::Hidden)));
    let section_change = previous.is_some_and(|previous| {
        activity(previous) != on_rail
            && matches!(
                items.get(previous).map(|item| &item.kind),
                Some(ItemKind::Assistant(_))
            )
    });
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
                .when(section_change, |this| this.mt(SECTION_GAP))
                .pt(top)
                .pb(bottom)
                // The rail's line spans the row's padding, so consecutive rows join up.
                .when(on_rail, |this| {
                    this.pl(RAIL_INDENT)
                        .border_l_1()
                        .border_color(context.palette.subtle.opacity(0.35))
                })
                .child(content),
        )
        .into_any_element()
}

fn render_item(item: &TranscriptItem, cx: &RowContext<'_>, bubble: Hsla, app: &App) -> AnyElement {
    match &item.kind {
        ItemKind::User(user) => v_flex()
            .w_full()
            .items_end()
            .gap_1()
            .group("user-message")
            // Attached pictures, above the bubble; they open in the viewer.
            .children(images::thumbnails(
                ("prompt-images", item.id.0),
                &user.images,
                images::PROMPT_THUMBNAILS,
                size(px(72.), px(54.)),
                app,
            ))
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
                    ),
            )
            .child(user_actions(item.id.0, &user.text, cx, app))
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
                this.child(output_box(
                    ("bash-out", item.id.0),
                    output,
                    None,
                    BlockKey {
                        item: item.id,
                        block: 0,
                    },
                    cx,
                ))
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
                .w_full()
                .justify_between()
                .group("turn-end")
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .text_color(cx.palette.subtle)
                        .child(crate::ui::spinner::resting_pi(px(11.), cx.palette.accent))
                        .child(label),
                )
                .child(turn_actions(item.id.0, cx, app))
                .into_any_element()
        }
    }
}

/// Hover actions under a prompt: copy it, or edit it (the conversation continues in a new
/// branch from just before it).
fn user_actions(id: u64, text: &str, cx: &RowContext<'_>, app: &App) -> impl IntoElement {
    let from_end = users_after(cx.items, cx.row);
    let edit_text = text.to_owned();
    h_flex()
        .gap_0p5()
        .invisible()
        .group_hover("user-message", |style| style.visible())
        .child(CopyButton::new(("copy-prompt", id), text.to_owned()).tooltip("Copy"))
        .when(cx.idle, |this| {
            this.child(action_button(
                ("edit-prompt", id),
                AppIcon::SquarePen,
                "Edit (continues in a new branch)",
                &cx.session,
                move |session, cx| {
                    session.fork_at(from_end, edit_text.clone(), AfterFork::Edit, cx);
                },
                app,
            ))
        })
}

/// Hover actions after a turn: copy the reply, retry the last prompt (the newest turn), or
/// branch from here (a new session that ends with this turn).
fn turn_actions(id: u64, cx: &RowContext<'_>, app: &App) -> impl IntoElement {
    let row = cx.row;
    let prompt = cx.items[..row]
        .iter()
        .rposition(|item| matches!(item.kind, ItemKind::User(_)));
    let next_prompt = cx.items[row + 1..]
        .iter()
        .position(|item| matches!(item.kind, ItemKind::User(_)))
        .map(|offset| row + 1 + offset);
    let reply = turn_text(&cx.items[prompt.map_or(0, |index| index + 1)..row]);
    let retry = match (prompt, next_prompt) {
        (Some(prompt), None) => match &cx.items[prompt].kind {
            ItemKind::User(user) => Some((users_after(cx.items, prompt), user.text.clone())),
            _ => None,
        },
        _ => None,
    };
    let branch = next_prompt.map(|next| match &cx.items[next].kind {
        ItemKind::User(user) => (users_after(cx.items, next), user.text.clone()),
        _ => (0, String::new()),
    });
    h_flex()
        .gap_0p5()
        .invisible()
        .group_hover("turn-end", |style| style.visible())
        .when(!reply.is_empty(), |this| {
            this.child(CopyButton::new(("copy-reply", id), reply).tooltip("Copy reply"))
        })
        .when(cx.idle, |this| {
            this.when_some(retry, |this, (from_end, text)| {
                this.child(action_button(
                    ("retry", id),
                    AppIcon::RotateCcw,
                    "Retry (the new answer continues in a new branch)",
                    &cx.session,
                    move |session, cx| {
                        session.fork_at(from_end, text.clone(), AfterFork::Resend, cx);
                    },
                    app,
                ))
            })
            .child(action_button(
                ("branch", id),
                AppIcon::GitBranch,
                "Branch from here (a new session up to this point)",
                &cx.session,
                move |session, cx| match &branch {
                    Some((from_end, text)) => {
                        session.fork_at(*from_end, text.clone(), AfterFork::Keep, cx);
                    }
                    None => session.clone_session(cx),
                },
                app,
            ))
        })
}

fn action_button(
    id: impl Into<ElementId>,
    icon: AppIcon,
    tooltip: &'static str,
    session: &Entity<PiSession>,
    action: impl Fn(&mut PiSession, &mut Context<PiSession>) + 'static,
    app: &App,
) -> impl IntoElement {
    let session = session.clone();
    component::button::Button::new(id)
        .quiet(app)
        .xsmall()
        .icon(Icon::from(icon))
        .accessibility_label(tooltip)
        .tooltip(tooltip)
        .on_click(move |_, _, cx| session.update(cx, |session, cx| action(session, cx)))
}

/// How many prompts come after `row` (0 for the last one): pi's fork list is matched from
/// its end.
fn users_after(items: &[TranscriptItem], row: usize) -> usize {
    items[row + 1..]
        .iter()
        .filter(|item| matches!(item.kind, ItemKind::User(_)))
        .count()
}

/// The answer text of a turn's assistant items.
fn turn_text(items: &[TranscriptItem]) -> String {
    let mut parts = Vec::new();
    for item in items {
        if let ItemKind::Assistant(assistant) = &item.kind {
            for block in &assistant.blocks {
                if let BlockKind::Text(text) = &block.kind
                    && !text.text.trim().is_empty()
                {
                    parts.push(text.text.trim());
                }
            }
        }
    }
    parts.join("\n\n")
}

fn render_assistant(
    item: &TranscriptItem,
    assistant: &AssistantItem,
    cx: &RowContext<'_>,
) -> AnyElement {
    // A row of only activity is put on the rail as a whole (see `render_row`); a row that
    // mixes text and steps puts each run of steps on its own rail here.
    let rail_here = !is_activity(item);
    let mut column = v_flex().w_full().gap_3();
    let mut run: Vec<AnyElement> = Vec::new();
    let flush = |column: Div, run: &mut Vec<AnyElement>| {
        if run.is_empty() {
            return column;
        }
        let steps = std::mem::take(run);
        if rail_here {
            column.child(rail(v_flex().w_full().gap_3().children(steps), cx))
        } else {
            column.children(steps)
        }
    };
    for (index, block) in assistant.blocks.iter().enumerate() {
        let key = BlockKey {
            item: item.id,
            block: index,
        };
        match &block.kind {
            BlockKind::Thinking(thinking) => run.push(render_thinking(key, thinking, cx)),
            BlockKind::Tool(tool) => run.push(render_tool(key, tool, cx)),
            BlockKind::Text(text) => {
                column = flush(column, &mut run);
                column = column.child(match cx.chat.markdown(key) {
                    Some(state) => div()
                        .w_full()
                        .text_color(cx.foreground)
                        .child(markdown_view(state))
                        .into_any_element(),
                    None => div().child(text.text.clone()).into_any_element(),
                });
            }
        }
    }
    column = flush(column, &mut run);
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

/// Puts a run of activity (tool calls, thinking) behind a thin line on its left, indented, so
/// it reads as one unit apart from the model's text.
fn rail(content: impl IntoElement, cx: &RowContext<'_>) -> Div {
    div()
        .w_full()
        .pl(RAIL_INDENT)
        .border_l_1()
        .border_color(cx.palette.subtle.opacity(0.35))
        .child(content)
}

/// Whether a transcript item shows only activity: an assistant message with tool calls or
/// thinking but no text (and no error). Such rows join one continuous rail.
fn is_activity(item: &TranscriptItem) -> bool {
    match &item.kind {
        ItemKind::Assistant(assistant) => {
            assistant.error.is_none()
                && !assistant.blocks.is_empty()
                && assistant.blocks.iter().all(|block| match &block.kind {
                    BlockKind::Text(text) => text.text.trim().is_empty(),
                    BlockKind::Thinking(_) | BlockKind::Tool(_) => true,
                })
        }
        _ => false,
    }
}

fn element_id(prefix: &'static str, key: BlockKey) -> ElementId {
    // Wrapping: a whole group's key uses `block: usize::MAX`.
    ElementId::NamedInteger(
        prefix.into(),
        key.item
            .0
            .wrapping_mul(1_000)
            .wrapping_add(key.block as u64),
    )
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
                .role(Role::Button)
                .aria_label(SharedString::from(label.clone()))
                .aria_expanded(expanded)
                .gap_1p5()
                .text_size(ACTIVITY_TEXT)
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
                    .child(
                        StyledText::new(thinking.text.trim().to_owned())
                            .with_highlights(cx.chat.find_highlights(key, Field::Thinking, cx.app)),
                    ),
            )
        })
        .into_any_element()
}

/// A run of exploration calls as one row: "Explored · 5 steps" and a line per call;
/// expanded, every thought and call of the run as usual.
fn render_group(group: &Group, cx: &RowContext<'_>) -> AnyElement {
    let first = &cx.items[cx.row];
    let key = BlockKey {
        item: first.id,
        block: usize::MAX,
    };
    let expanded = cx.chat.is_expanded(key, false);
    let tools: Vec<&ToolBlock> = group
        .tools
        .iter()
        .filter_map(|&(item, block)| match &cx.items[item].kind {
            ItemKind::Assistant(assistant) => match &assistant.blocks.get(block)?.kind {
                BlockKind::Tool(tool) => Some(tool),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let active = tools.iter().any(|tool| tool.status.is_active());
    let failed = tools
        .iter()
        .filter(|tool| tool.status == ToolStatus::Failed)
        .count();
    let marker_status = if active {
        ToolStatus::Running
    } else if failed > 0 {
        ToolStatus::Failed
    } else {
        ToolStatus::Done
    };
    let mut detail = plural(tools.len(), "step");
    if failed > 0 {
        let _ = write!(detail, ", {failed} failed");
    }

    let header = h_flex()
        .id(element_id("explored", key))
        .role(Role::Button)
        .aria_label(SharedString::from(format!("Explored, {detail}")))
        .aria_expanded(expanded)
        .items_start()
        .gap_2()
        .cursor_pointer()
        .on_click(toggle_handler(cx, key))
        .child(tool_marker(ToolKind::Search, marker_status, px(13.), cx).mt(px(2.)))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .text_size(ACTIVITY_TEXT)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.muted)
                        .child(if active { "Exploring" } else { "Explored" }),
                )
                .child(div().text_color(cx.muted).child(detail))
                .child(chevron(expanded).text_color(cx.muted)),
        );

    let body = if expanded {
        let rows = (cx.row..=group.last).filter_map(|index| {
            let item = &cx.items[index];
            match &item.kind {
                ItemKind::Assistant(assistant) => Some(render_assistant(item, assistant, cx)),
                _ => None,
            }
        });
        v_flex()
            .pl(px(15.))
            .gap_3()
            .children(rows)
            .into_any_element()
    } else {
        let lines = tools.iter().enumerate().map(|(index, tool)| {
            let header = tool::header(tool, cx.paths);
            let color = match tool.status {
                ToolStatus::Failed => cx.palette.error,
                _ => cx.muted,
            };
            h_flex()
                .gap_2()
                .text_size(ACTIVITY_TEXT)
                .child(
                    div()
                        .flex_none()
                        .w(px(10.))
                        .text_color(cx.palette.subtle)
                        .child(if index == 0 { "⎿" } else { "" }),
                )
                .child(div().flex_none().text_color(color).child(header.title))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(cx.mono.clone())
                        .text_xs()
                        .text_color(color)
                        .child(header.detail),
                )
        });
        v_flex()
            .pl(px(15.))
            .gap_0p5()
            .children(lines)
            .into_any_element()
    };
    // Collapsed, the group shows the pictures its calls returned next to its lines; expanded,
    // each call shows its own.
    let pictures: Vec<transcript::Image> = if expanded {
        Vec::new()
    } else {
        tools
            .iter()
            .flat_map(|tool| tool.images.iter().cloned())
            .collect()
    };
    let thumbs = images::thumbnails(
        element_id("explored-images", key),
        &pictures,
        images::TOOL_THUMBNAILS,
        size(px(48.), px(34.)),
        cx.app,
    );
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(header)
                .child(body),
        )
        .children(thumbs)
        .into_any_element()
}

fn render_tool(key: BlockKey, tool: &ToolBlock, cx: &RowContext<'_>) -> AnyElement {
    let expanded = cx.chat.is_expanded(key, false);
    let header = tool::header(tool, cx.paths);
    // A labelled bash call shows its command when expanded.
    let labelled = tool.name == "bash" && header.title != "Bash";
    let diff = tool::diff_lines(tool);

    let nested_steps = tool
        .nested
        .iter()
        .filter(|entry| matches!(entry, NestedEntry::Tool(_)))
        .count();
    // The latest step inside a running call (a subagent's or code mode's own tools).
    let live_step = tool.nested.iter().rev().find_map(|entry| match entry {
        NestedEntry::Tool(inner) => {
            let header = tool::header(inner, cx.paths);
            Some(
                format!("{} {}", header.title, header.detail)
                    .trim()
                    .to_owned(),
            )
        }
        NestedEntry::Text(_) => None,
    });
    // The collapsed preview of a file the tool read is colored like the expanded output.
    let preview_language = tool_language(tool).filter(|_| tool.name == "read");
    let summary: Option<SharedString> = match tool.status {
        ToolStatus::Streaming | ToolStatus::Pending => Some("Preparing…".into()),
        ToolStatus::Running if !expanded && live_step.is_some() => Some(
            format!(
                "{} · {}",
                plural(nested_steps, "step"),
                live_step.unwrap_or_default()
            )
            .into(),
        ),
        ToolStatus::Running if tool.output.is_empty() => Some("Running…".into()),
        ToolStatus::Cancelled => Some("Interrupted".into()),
        _ if expanded => None,
        // A labelled call reads as what it did; its output shows only when expanded.
        _ if labelled => None,
        _ if nested_steps > 0 => Some(plural(nested_steps, "step").into()),
        _ => tool::result_summary(tool).map(Into::into).or_else(|| {
            (!tool.output.trim().is_empty() && diff.is_none()).then(|| preview(&tool.output).into())
        }),
    };

    // Pictures the call returned (`read` on a screenshot), to the right of its lines.
    let thumbs = images::thumbnails(
        element_id("tool-images", key),
        &tool.images,
        images::TOOL_THUMBNAILS,
        size(px(48.), px(34.)),
        cx.app,
    );
    let top = v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            h_flex()
                .id(element_id("tool", key))
                .role(Role::Button)
                .aria_label(SharedString::from(
                    format!("{} {}", header.title, header.detail)
                        .trim()
                        .to_owned(),
                ))
                .aria_expanded(expanded)
                .items_start()
                .gap_2()
                .cursor_pointer()
                .on_click(toggle_handler(cx, key))
                .child(tool_marker(tool::kind(tool), tool.status, px(13.), cx).mt(px(2.)))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1p5()
                        .text_size(ACTIVITY_TEXT)
                        .items_start()
                        // Quieter than the model's text: activity is context, prose is the
                        // answer.
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(cx.muted)
                                .child(header.title),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(cx.mono.clone())
                                .text_color(cx.palette.subtle)
                                .child(header.detail),
                        )
                        .when(labelled, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .mt(px(3.))
                                    .text_color(cx.palette.subtle)
                                    .child(chevron(expanded)),
                            )
                        }),
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
                            .child(match preview_language {
                                Some(language) if summary.as_ref() == preview(&tool.output) => {
                                    let styles = file_styles(language, &tool.output, cx);
                                    let highlights = code_highlight::join_lines(
                                        &styles,
                                        &tool.output,
                                        PREVIEW_LINES,
                                    );
                                    StyledText::new(summary)
                                        .with_highlights(highlights)
                                        .into_any_element()
                                }
                                _ => summary.into_any_element(),
                            }),
                    ),
            )
        });
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .child(top)
                .children(thumbs),
        )
        .when(expanded, |this| {
            let command = (tool.name == "bash")
                .then(|| tool.args.get("command").and_then(serde_json::Value::as_str))
                .flatten()
                .filter(|_| labelled);
            let this = this.when_some(command, |this, command| {
                this.child(
                    div()
                        .pl(px(15.))
                        .font_family(cx.mono.clone())
                        .text_xs()
                        .text_color(cx.muted)
                        .child(format!("$ {}", command.trim())),
                )
            });
            let language = tool_language(tool);
            let body = match &diff {
                Some(lines) => diff_box(
                    ("tool-diff", key.item.0 * 1_000 + key.block as u64),
                    lines,
                    language.map(|language| diff_styles(language, lines, cx)),
                    key,
                    cx,
                ),
                None if !tool.output.trim().is_empty() => output_box(
                    ("tool-out", key.item.0 * 1_000 + key.block as u64),
                    &tool.output,
                    language
                        .filter(|_| tool.name == "read")
                        .map(|language| file_styles(language, &tool.output, cx)),
                    key,
                    cx,
                ),
                None => div()
                    .pl(px(15.))
                    .text_xs()
                    .text_color(cx.muted)
                    .child("(no output)")
                    .into_any_element(),
            };
            this.when(!tool.nested.is_empty(), |this| {
                this.child(render_nested(&tool.nested, cx))
            })
            .child(div().pl(px(15.)).child(body))
        })
        .into_any_element()
}

/// The icon at the start of a tool row: what kind of step it is (a document for a read, a
/// pencil for a change, a terminal for a command…), coloured only when that means something:
/// muted when done, red when it failed, the accent and breathing while it runs, faded when it
/// was interrupted. (Green is left for "connected" and "running".)
fn tool_marker(kind: ToolKind, status: ToolStatus, size: Pixels, cx: &RowContext<'_>) -> Div {
    let icon: Icon = match kind {
        ToolKind::Read => IconName::FileText.into(),
        ToolKind::Write => AppIcon::Pencil.into(),
        ToolKind::List => IconName::Folder.into(),
        ToolKind::Search => IconName::Search.into(),
        ToolKind::Web => AppIcon::Globe.into(),
        ToolKind::Install => AppIcon::Package.into(),
        ToolKind::Git => AppIcon::GitBranch.into(),
        ToolKind::Shell => IconName::SquareTerminal.into(),
        ToolKind::Other => AppIcon::Wrench.into(),
    };
    let color = match status {
        ToolStatus::Done => cx.muted,
        ToolStatus::Failed => cx.palette.error,
        ToolStatus::Cancelled => cx.palette.subtle.opacity(0.5),
        ToolStatus::Streaming | ToolStatus::Pending | ToolStatus::Running => cx
            .palette
            .accent
            .opacity(crate::ui::spinner::running_opacity()),
    };
    div()
        .flex_none()
        .size(size)
        .child(icon.size(size).text_color(color))
}

/// The steps inside an expanded tool call: each nested call as a compact row, and what a
/// nested assistant replied.
fn render_nested(nested: &[NestedEntry], cx: &RowContext<'_>) -> AnyElement {
    let rows = nested.iter().map(|entry| match entry {
        NestedEntry::Tool(inner) => {
            let header = tool::header(inner, cx.paths);
            h_flex()
                .gap_1p5()
                .items_center()
                .text_xs()
                .child(tool_marker(tool::kind(inner), inner.status, px(11.), cx))
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
                )
                .into_any_element()
        }
        NestedEntry::Text(text) => div()
            .text_xs()
            .text_color(cx.muted)
            .child(text.clone())
            .into_any_element(),
    });
    v_flex()
        .pl(px(15.))
        .ml(px(3.))
        .border_l_1()
        .border_color(cx.palette.subtle.opacity(0.4))
        .gap_1()
        .child(v_flex().pl_2().gap_1().children(rows))
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

/// The highlighter language of the file a tool call works on, from its path argument.
fn tool_language(tool: &ToolBlock) -> Option<&'static str> {
    let path = ["path", "file_path"]
        .iter()
        .find_map(|key| tool.args.get(*key).and_then(serde_json::Value::as_str))?;
    code_highlight::language_for_path(std::path::Path::new(path))
}

/// Syntax colors for each line of an edit's diff, from its rebuilt old and new text.
fn diff_styles(
    language: &'static str,
    lines: &[tool::DiffLine],
    cx: &RowContext<'_>,
) -> Rc<Vec<LineStyles>> {
    let side = |kind: DiffKind| match kind {
        DiffKind::Added => Side::New,
        DiffKind::Removed => Side::Old,
        DiffKind::Context => Side::Both,
        DiffKind::Gap => Side::Neither,
    };
    let key = (
        "diff",
        language,
        lines
            .iter()
            .map(|line| (line.kind as u8, line.text.as_str()))
            .collect::<Vec<_>>(),
    );
    cx.chat
        .highlights
        .borrow_mut()
        .get_or_insert(&cx.highlight, key, |theme| {
            code_highlight::highlight_diff(
                language,
                lines
                    .iter()
                    .map(|line| (side(line.kind), line.text.as_str())),
                theme,
            )
        })
}

/// Syntax colors for each line of a file the agent read.
fn file_styles(language: &'static str, text: &str, cx: &RowContext<'_>) -> Rc<Vec<LineStyles>> {
    cx.chat.highlights.borrow_mut().get_or_insert(
        &cx.highlight,
        ("file", language, text),
        |theme| code_highlight::highlight_lines(language, text, theme),
    )
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Tool or shell output in a scrolling box, with syntax colors for a file that was read
/// and find highlights. It shows [`search::shown_output`], the text find searches.
fn output_box(
    id: impl Into<ElementId>,
    output: &str,
    styles: Option<Rc<Vec<LineStyles>>>,
    key: BlockKey,
    cx: &RowContext<'_>,
) -> AnyElement {
    let shown = search::shown_output(output);
    let total = output.trim_end().lines().count();
    let syntax = styles
        .map(|styles| code_highlight::join_lines(&styles, output, MAX_OUTPUT_LINES))
        .unwrap_or_default();
    let find = cx.chat.find_highlights(key, Field::Output, cx.app);
    let highlights = code_highlight::overlay(&syntax, &find);
    let mut text = shown.into_owned();
    if total > MAX_OUTPUT_LINES {
        let _ = write!(text, "\n… {} more lines", total - MAX_OUTPUT_LINES);
    }
    // With the current find match inside, the box shows everything so the match is visible.
    let full = cx.chat.find_current_in(key);
    div()
        .id(id)
        .w_full()
        .when(!full, |this| this.max_h(px(320.)).overflow_y_scroll())
        .p_2()
        .rounded_md()
        .bg(cx.palette.code_bg)
        .border_1()
        .border_color(cx.border)
        .font_family(cx.mono.clone())
        .text_xs()
        .text_color(cx.foreground)
        .child(StyledText::new(text).with_highlights(highlights))
        .into_any_element()
}

fn diff_box(
    id: impl Into<ElementId>,
    lines: &[tool::DiffLine],
    styles: Option<Rc<Vec<LineStyles>>>,
    key: BlockKey,
    cx: &RowContext<'_>,
) -> AnyElement {
    let full = cx.chat.find_current_in(key);
    let gutter = lines
        .iter()
        .filter_map(|line| line.line_number)
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    let rows = lines
        .iter()
        .enumerate()
        .take(MAX_OUTPUT_LINES)
        .map(|(ix, line)| {
            let syntax = styles
                .as_ref()
                .and_then(|styles| styles.get(ix))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let find = cx.chat.find_highlights(key, Field::DiffLine(ix), cx.app);
            let line_styles = (!syntax.is_empty() || !find.is_empty())
                .then(|| code_highlight::overlay(syntax, &find));
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
                .child(match line_styles {
                    // Code colors on top of the added/removed background; the sign keeps
                    // the red/green.
                    Some(line_styles) if line.kind != DiffKind::Gap => div()
                        .flex_1()
                        .min_w_0()
                        .text_color(cx.foreground)
                        .child(StyledText::new(line.text.clone()).with_highlights(line_styles)),
                    _ => div().flex_1().min_w_0().text_color(fg).child(
                        if line.kind == DiffKind::Gap {
                            "⋯".to_owned()
                        } else {
                            line.text.clone()
                        },
                    ),
                })
        });
    div()
        .id(id)
        .w_full()
        .when(!full, |this| this.max_h(px(360.)).overflow_y_scroll())
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
    let style = cx
        .app
        .global::<crate::settings::AppSettings>()
        .working_indicator;
    h_flex()
        .w_full()
        .justify_center()
        .items_center()
        .gap_1p5()
        .text_sm()
        .child(crate::ui::spinner::working_indicator(
            style,
            px(13.),
            cx.palette.accent,
        ))
        .child(crate::ui::spinner::working_label(
            style,
            label.into(),
            cx.palette.accent,
        ))
        .when_some(elapsed, |this, elapsed| {
            this.child(
                div()
                    .text_color(cx.muted)
                    .child(format!("{} · esc to interrupt", format_duration(elapsed))),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{BlockKey, element_id};
    use transcript::ItemId;

    #[test]
    fn a_whole_group_has_an_element_id() {
        let key = |block| BlockKey {
            item: ItemId(7),
            block,
        };
        assert_ne!(
            element_id("explored", key(usize::MAX)),
            element_id("explored", key(0))
        );
    }
}
