//! The context breakdown shown when clicking the composer's context wheel.
//!
//! The total and window come from pi; how the total splits into categories is estimated from
//! the transcript ([`ContextEstimate`]) and reconciled with that total here ([`Breakdown`]):
//! whatever the conversation does not explain is attributed to the system prompt and tool
//! definitions, which pi does not expose.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::SessionStats;
use transcript::{ContextEstimate, format_tokens};

use super::session::PiSession;

/// One row of the breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub label: &'static str,
    pub tokens: u64,
    pub color: u32,
}

/// The reconciled breakdown of `used` tokens out of `window`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown {
    pub used: u64,
    pub window: u64,
    /// Non-empty categories, in display order (system first).
    pub categories: Vec<Category>,
}

/// Category colors from the app's palette (readable on light and dark).
const SYSTEM: u32 = 0x008C_9C93;
const USER: u32 = 0x0024_A926;
const ASSISTANT: u32 = 0x00BF_FD50;
const THINKING: u32 = 0x005F_8F72;
const TOOL_CALLS: u32 = 0x00C6_D73B;
const TOOL_RESULTS: u32 = 0x0002_9102;
const SUMMARY: u32 = 0x00D3_DED7;

impl Breakdown {
    /// Splits `used` by the estimate. If the estimate overshoots (it is only approximate), the
    /// conversation categories are scaled down to fit and nothing is left for the system part.
    pub fn new(used: u64, window: u64, estimate: &ContextEstimate) -> Self {
        let parts = [
            ("Your messages", estimate.user, USER),
            ("Assistant replies", estimate.assistant, ASSISTANT),
            ("Thinking", estimate.thinking, THINKING),
            ("Tool calls", estimate.tool_calls, TOOL_CALLS),
            ("Tool results", estimate.tool_results, TOOL_RESULTS),
            ("Compaction summary", estimate.summary, SUMMARY),
        ];
        let estimated = estimate.total();
        let scale = |tokens: u64| {
            if estimated > used && estimated > 0 {
                u64::try_from(u128::from(tokens) * u128::from(used) / u128::from(estimated))
                    .unwrap_or(tokens)
            } else {
                tokens
            }
        };
        let mut categories = vec![Category {
            label: "System prompt & tools",
            tokens: used.saturating_sub(estimated),
            color: SYSTEM,
        }];
        categories.extend(parts.into_iter().map(|(label, tokens, color)| Category {
            label,
            tokens: scale(tokens),
            color,
        }));
        categories.retain(|category| category.tokens > 0);
        Self {
            used,
            window,
            categories,
        }
    }

    pub fn free(&self) -> u64 {
        self.window.saturating_sub(self.used)
    }

    /// Share of the window, 0.0..=1.0.
    pub fn fraction(&self, tokens: u64) -> f32 {
        if self.window == 0 {
            return 0.;
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let fraction = (tokens as f64 / self.window as f64).clamp(0., 1.) as f32;
        fraction
    }
}

/// Renders the breakdown card for `session`.
pub fn render(session: &PiSession, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (foreground, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
    let Some((used, window)) = session.context_usage() else {
        return card()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Context usage is unavailable until pi reports a model."),
            )
            .into_any_element();
    };
    let known = used > 0 || session.transcript().items().is_empty();
    let breakdown = Breakdown::new(
        used,
        window,
        &ContextEstimate::of(session.transcript().items()),
    );
    let percent = breakdown.fraction(used) * 100.;

    let bar = h_flex()
        .w_full()
        .h(px(8.))
        .rounded_full()
        .overflow_hidden()
        .bg(border)
        .children(breakdown.categories.iter().map(|category| {
            div()
                .h_full()
                .flex_none()
                .w(relative(breakdown.fraction(category.tokens)))
                .bg(rgb(category.color))
        }));

    let category_rows = breakdown
        .categories
        .iter()
        .map(|category| {
            row(
                Some(category.color),
                category.label,
                category.tokens,
                &breakdown,
                foreground,
                muted,
            )
        })
        .chain(std::iter::once(row(
            None,
            "Free space",
            breakdown.free(),
            &breakdown,
            foreground,
            muted,
        )));

    card()
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(foreground).child("Context window"))
                .child(div().text_xs().text_color(muted).child(format!("{percent:.1}% used"))),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{} of {} tokens", format_tokens(used), format_tokens(window))),
        )
        .child(bar)
        .when(!known, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("The current size is unknown until the next response (e.g. right after compaction)."),
            )
        })
        .child(v_flex().gap_1().children(category_rows))
        .when_some(session.stats(), |this, stats| this.child(div().h(px(1.)).bg(border)).child(session_totals(stats, foreground, muted)))
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child("Categories are estimated from the conversation (≈4 characters per token); totals come from pi."),
        )
        .into_any_element()
}

fn card() -> Div {
    v_flex().w(px(340.)).p_3().gap_2()
}

fn row(
    color: Option<u32>,
    label: &'static str,
    tokens: u64,
    breakdown: &Breakdown,
    foreground: Hsla,
    muted: Hsla,
) -> Div {
    let swatch = div().size(px(8.)).flex_none().rounded_sm();
    let swatch = match color {
        Some(color) => swatch.bg(rgb(color)),
        None => swatch.border_1().border_color(muted),
    };
    h_flex()
        .gap_2()
        .text_xs()
        .child(swatch)
        .child(div().flex_1().text_color(foreground).child(label))
        .child(div().text_color(foreground).child(format!(
            "{}{}",
            if color.is_some() { "≈ " } else { "" },
            format_tokens(tokens)
        )))
        .child(
            div()
                .w(px(44.))
                .flex_none()
                .text_right()
                .text_color(muted)
                .child(percent_label(tokens, breakdown)),
        )
}

/// `12.3%`, or `<0.1%` for small but non-zero shares (common with million-token windows).
fn percent_label(tokens: u64, breakdown: &Breakdown) -> String {
    let percent = breakdown.fraction(tokens) * 100.;
    if tokens > 0 && percent < 0.1 {
        "<0.1%".to_owned()
    } else {
        format!("{percent:.1}%")
    }
}

fn session_totals(stats: &SessionStats, foreground: Hsla, muted: Hsla) -> Div {
    let tokens = &stats.tokens;
    let line = |label: &'static str, value: String| {
        h_flex()
            .justify_between()
            .text_xs()
            .child(div().text_color(muted).child(label))
            .child(div().text_color(foreground).child(value))
    };
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child("This session"),
        )
        .child(line("Input", format_tokens(tokens.input)))
        .child(line("Output", format_tokens(tokens.output)))
        .when(tokens.cache_read > 0 || tokens.cache_write > 0, |this| {
            this.child(line(
                "Cache read / write",
                format!(
                    "{} / {}",
                    format_tokens(tokens.cache_read),
                    format_tokens(tokens.cache_write)
                ),
            ))
        })
        .when(stats.cost > 0., |this| {
            this.child(line("Cost", format!("${:.4}", stats.cost)))
        })
        .child(line(
            "Messages · tool calls",
            format!(
                "{} · {}",
                stats.user_messages + stats.assistant_messages,
                stats.tool_calls
            ),
        ))
}

#[cfg(test)]
mod tests {
    use transcript::ContextEstimate;

    use super::Breakdown;

    fn labels(breakdown: &Breakdown) -> Vec<(&'static str, u64)> {
        breakdown
            .categories
            .iter()
            .map(|category| (category.label, category.tokens))
            .collect()
    }

    #[test]
    fn remainder_is_attributed_to_system_prompt_and_tools() {
        let estimate = ContextEstimate {
            user: 100,
            assistant: 300,
            tool_results: 600,
            ..Default::default()
        };
        let breakdown = Breakdown::new(5_000, 100_000, &estimate);
        assert_eq!(
            labels(&breakdown),
            [
                ("System prompt & tools", 4_000),
                ("Your messages", 100),
                ("Assistant replies", 300),
                ("Tool results", 600)
            ]
        );
        assert_eq!(breakdown.free(), 95_000);
        assert!((breakdown.fraction(5_000) - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn overshooting_estimates_are_scaled_to_the_reported_total() {
        let estimate = ContextEstimate {
            user: 1_000,
            assistant: 3_000,
            ..Default::default()
        };
        let breakdown = Breakdown::new(2_000, 10_000, &estimate);
        assert_eq!(
            labels(&breakdown),
            [("Your messages", 500), ("Assistant replies", 1_500)]
        );
        assert_eq!(Breakdown::new(0, 0, &estimate).fraction(10), 0.);
    }
}
