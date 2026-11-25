//! Settings → Web search: the sections few people change, behind a title that opens them.

use std::ops::RangeInclusive;
use std::rc::Rc;

use deepsearch::config::{Check, Config as WebConfig};
use deepsearch::effort::{DeepBudget, Effort};
use gpui_kit::component::button::Button;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::view::{SettingsView, update_web, web_config};
use crate::theme::palette;
use crate::ui::QuietButton as _;

const SLIDER_WIDTH: Pixels = px(190.);

/// Which sections are open.
#[derive(Debug, Default)]
pub struct Advanced {
    search: bool,
    deep: bool,
    /// One was open: the page stays long enough that closing it moves nothing.
    opened: bool,
    /// The effort level whose settings are shown.
    level: Effort,
}

impl Advanced {
    pub fn open_all(&mut self) {
        *self = Self {
            search: true,
            deep: true,
            opened: true,
            level: self.level,
        };
    }

    /// Room below the page's end, so that a section that closes while the page is scrolled to its
    /// end keeps its title in place.
    pub fn room(&self, window: &Window) -> Pixels {
        if self.opened {
            window.viewport_size().height * 0.7
        } else {
            px(0.)
        }
    }
}

pub fn search_group(advanced: &Advanced, weak: &WeakEntity<SettingsView>) -> SettingGroup {
    let open = advanced.search;
    let weak = weak.clone();
    let title = title(
        "web-search-options",
        "Advanced search options",
        "What a quick search may do, and how much of each page the model gets.",
        open,
        move |cx| toggle(&weak, |advanced| &mut advanced.search, cx),
    )
    // Found by the settings search also while closed.
    .keywords([
        "Advanced search options",
        "Searches",
        "Pages read",
        "Text per page",
    ]);
    let group = SettingGroup::new().item(title);
    if !open {
        return group;
    }
    group
        .item(number(
            "Searches",
            "At most, per message. Then the model answers with what it found.",
            Number::new("search-searches", 1..=50, |config| {
                config.modes.search.max_searches
            })
            .set(|config, value| config.modes.search.max_searches = value),
        ))
        .item(number(
            "Pages read",
            "At most, per message.",
            Number::new("search-pages", 0..=50, |config| {
                config.modes.search.max_fetches
            })
            .set(|config, value| config.modes.search.max_fetches = value),
        ))
        .item(number(
            "Text per page",
            "Characters, about four to a token. Lower it for models with a small context window.",
            Number::new("text-per-page", 1..=100, |config| {
                u32::try_from(config.fetch.max_chars / 1000).unwrap_or(u32::MAX)
            })
            .set(|config, value| config.fetch.max_chars = value as usize * 1000)
            .shown(|value| format!("{value},000")),
        ))
}

pub fn deep_group(advanced: &Advanced, weak: &WeakEntity<SettingsView>) -> SettingGroup {
    let open = advanced.deep;
    let level = advanced.level;
    let toggle_weak = weak.clone();
    let title = title(
        "deep-search-options",
        "Advanced deep search settings",
        "How much work each effort level does, and how its report is checked.",
        open,
        move |cx| toggle(&toggle_weak, |advanced| &mut advanced.deep, cx),
    )
    .keywords([
        "Advanced deep search settings",
        "Effort levels",
        "Sub-questions",
        "Different sites",
        "Claims checked",
        "Revisions",
        "Time per question",
    ]);
    let group = SettingGroup::new().item(title);
    if !open {
        return group;
    }
    let weak = weak.clone();
    let deep =
        |id: &'static str, range, get: fn(&DeepBudget) -> u32, set: fn(&mut DeepBudget, u32)| {
            Number::new(id, range, move |config| get(&config.deep.budget(level)))
                .set(move |config, value| set(config.deep.budget_mut(level), value))
                .keyed(level.as_str())
        };
    let claims = |budget: &DeepBudget| u32::try_from(budget.max_claims).unwrap_or(u32::MAX);
    group
        .item(levels(level, weak))
        .item(heading("At most"))
        .item(number(
            "Searches",
            "Per message.",
            deep("deep-searches", 1..=300, |b| b.max_searches, |b, v| b.max_searches = v),
        ))
        .item(number(
            "Pages read",
            "Per message.",
            deep("deep-pages", 1..=300, |b| b.max_fetches, |b, v| b.max_fetches = v),
        ))
        .item(number(
            "Results per search",
            "More are more to choose from, and more to read through.",
            deep(
                "deep-results",
                1..=20,
                |b| u32::try_from(b.max_results).unwrap_or(u32::MAX),
                |b, v| b.max_results = v as usize,
            ),
        ))
        .item(heading("At least"))
        .item(number(
            "Sub-questions",
            "A broad question is split into at most this many. Each asks for the searches and pages below.",
            deep("deep-questions", 1..=20, |b| b.max_questions, |b, v| b.max_questions = v),
        ))
        .item(number(
            "Searches per sub-question",
            "With less the model is sent back to its research.",
            deep(
                "deep-question-searches",
                0..=8,
                |b| b.searches_per_question,
                |b, v| b.searches_per_question = v,
            ),
        ))
        .item(number(
            "Pages per sub-question",
            "The same for pages it read.",
            deep(
                "deep-question-pages",
                0..=8,
                |b| b.pages_per_question,
                |b, v| b.pages_per_question = v,
            ),
        ))
        .item(number(
            "Different sites",
            "So that not all of it comes from one.",
            deep("deep-sites", 1..=30, |b| b.min_sites, |b, v| b.min_sites = v),
        ))
        .item(number(
            "Sent back",
            "How often the model is sent back to its research. Then its report is taken as it is.",
            deep("deep-rounds", 0..=20, |b| b.max_rounds, |b, v| b.max_rounds = v),
        ))
        .item(heading("Checking the report"))
        .item(number(
            "Claims checked",
            "Against their sources; those with numbers first. With 0, only links are checked.",
            deep("deep-claims", 0..=200, claims, |b, v| b.max_claims = v as usize),
        ))
        .item(number(
            "Claims per question",
            "How many are judged at once. Fewer is slower and more careful.",
            deep(
                "deep-claims-per-check",
                1..=10,
                |b| u32::try_from(b.claims_per_check).unwrap_or(u32::MAX),
                |b, v| b.claims_per_check = v as usize,
            ),
        ))
        .item(number(
            "Revisions",
            "How often the report is written again because of what the check found.",
            deep("deep-revisions", 0..=6, |b| b.max_revisions, |b, v| b.max_revisions = v),
        ))
        .item(heading("For every level"))
        .item(number(
            "Time per question",
            "How long the model may take to judge a claim before the check gives up.",
            Number::new("check-timeout", 1..=20, |config| secs(config.check.timeout_secs, 30))
                .set(|config, value| config.check.timeout_secs = u64::from(value) * 30)
                .shown(|value| duration(value * 30)),
        ))
        .item(number(
            "Time per check",
            "How long checking one report may take.",
            Number::new("check-total", 1..=60, |config| secs(config.check.total_secs, 60))
                .set(|config, value| config.check.total_secs = u64::from(value) * 60)
                .shown(|value| duration(value * 60)),
        ))
        .item(number(
            "Answer length",
            "Tokens the model may write for one question, its thinking included.",
            Number::new("check-tokens", 1..=32, |config| config.check.max_tokens / 1024)
                .set(|config, value| config.check.max_tokens = value * 1024)
                .shown(|value| format!("{}", value * 1024)),
        ))
}

fn secs(value: u64, step: u64) -> u32 {
    u32::try_from(value / step).unwrap_or(u32::MAX)
}

fn duration(secs: u32) -> String {
    match (secs / 60, secs % 60) {
        (0, secs) => format!("{secs} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, secs) => format!("{minutes} min {secs} s"),
    }
}

fn toggle(weak: &WeakEntity<SettingsView>, section: fn(&mut Advanced) -> &mut bool, cx: &mut App) {
    let _ = weak.update(cx, |view, cx| {
        let open = section(&mut view.advanced);
        *open = !*open;
        view.advanced.opened = true;
        cx.notify();
    });
}

/// A section's title: a click on it opens or closes what is below. It is as tall in both states.
fn title(
    id: &'static str,
    title: &'static str,
    about: &'static str,
    open: bool,
    on_click: impl Fn(&mut App) + 'static,
) -> SettingItem {
    let on_click = Rc::new(on_click);
    SettingItem::render(move |_, _, cx| {
        let theme = cx.theme();
        let on_click = on_click.clone();
        h_flex()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(title)
            .aria_expanded(open)
            .w_full()
            .gap_3()
            .cursor_pointer()
            .text_color(theme.muted_foreground)
            .hover(|this| this.text_color(cx.theme().foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    // Like the titles of the other groups.
                    .child(div().line_height(relative(1.25)).child(title))
                    .child(div().text_sm().opacity(0.8).child(about)),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .small(),
            )
            .on_click(move |_, _, cx| on_click(cx))
    })
}

fn heading(text: &'static str) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        h_flex()
            .w_full()
            .pt_2()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(text),
            )
            .child(div().flex_1().h(px(1.)).bg(cx.theme().border))
    })
}

/// The effort levels, and putting the shown one back as it comes.
fn levels(level: Effort, weak: WeakEntity<SettingsView>) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let changed = web_config(cx).deep.budget(level) != DeepBudget::of(level)
            || web_config(cx).check != Check::default();
        let weak = weak.clone();
        let selected = Effort::ALL.iter().position(|other| *other == level);
        h_flex()
            .w_full()
            .justify_between()
            .gap_3()
            .child(
                TabBar::new("deep-levels")
                    .segmented()
                    .small()
                    .selected_index(selected.unwrap_or_default())
                    .on_click(move |index, _, cx| {
                        let level = Effort::ALL.get(*index).copied().unwrap_or_default();
                        let _ = weak.update(cx, |view, cx| {
                            view.advanced.level = level;
                            cx.notify();
                        });
                    })
                    .children(Effort::ALL.map(|level| Tab::new().label(level.label()))),
            )
            // Keeps its room, so nothing moves.
            .child(
                div().when(!changed, |this| this.invisible()).child(
                    Button::new("deep-reset")
                        .quiet(cx)
                        .small()
                        .icon(Icon::from(crate::ui::AppIcon::RotateCcw))
                        .label("Reset")
                        .tooltip(SharedString::from(format!(
                            "{} and the settings for every level, as they come",
                            level.label()
                        )))
                        .on_click(move |_, _, cx| {
                            update_web(cx, |config| {
                                *config.deep.budget_mut(level) = DeepBudget::of(level);
                                config.check = Check::default();
                            });
                        }),
                ),
            )
    })
}

type Set = dyn Fn(u32, &mut App);

/// A whole number of the settings, chosen with a slider.
#[derive(Clone)]
pub(super) struct Number {
    id: SharedString,
    range: RangeInclusive<u32>,
    get: Rc<dyn Fn(&App) -> u32>,
    set: Rc<Set>,
    shown: fn(u32) -> String,
}

impl Number {
    /// One that is not a setting of web search.
    pub(super) fn of(
        id: &'static str,
        range: RangeInclusive<u32>,
        get: impl Fn(&App) -> u32 + 'static,
        set: impl Fn(u32, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            range,
            get: Rc::new(get),
            set: Rc::new(set),
            shown: |value| value.to_string(),
        }
    }

    fn new(
        id: &'static str,
        range: RangeInclusive<u32>,
        get: impl Fn(&WebConfig) -> u32 + 'static,
    ) -> Self {
        Self::of(id, range, move |cx| get(web_config(cx)), |_, _| {})
    }

    fn set(mut self, set: impl Fn(&mut WebConfig, u32) + 'static) -> Self {
        self.set = Rc::new(move |value, cx| update_web(cx, |config| set(config, value)));
        self
    }

    pub(super) fn shown(mut self, shown: fn(u32) -> String) -> Self {
        self.shown = shown;
        self
    }

    /// Only the tests of the documents settings use it, and those need macOS or Linux.
    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    pub(super) fn choose(&self, value: u32, cx: &mut App) {
        (self.set)(value, cx);
    }

    /// Different for each `key`.
    fn keyed(mut self, key: &str) -> Self {
        self.id = format!("{}-{key}", self.id).into();
        self
    }
}

pub(super) fn number(title: &'static str, about: &'static str, number: Number) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::render(move |_, window, cx| slider(title, &number, window, cx)),
    )
    .description(about)
}

/// A slider and the setting it was last in step with.
struct SliderField {
    slider: Entity<SliderState>,
    saved: u32,
    _subscription: Subscription,
}

#[allow(clippy::cast_precision_loss)] // Small numbers.
fn to_f32(value: u32) -> f32 {
    value as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Small numbers from 0 up.
fn to_number(value: f32) -> u32 {
    value.round().max(0.) as u32
}

fn slider(
    title: &'static str,
    number: &Number,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let value = (number.get)(cx);
    let (min, max) = (*number.range.start(), *number.range.end());
    let field = window.use_keyed_state(number.id.clone(), cx, |_, cx| {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(to_f32(min))
                .max(to_f32(max))
                .step(1.)
                .default_value(to_f32(value.clamp(min, max)))
        });
        let set = number.set.clone();
        let subscription = cx.subscribe(
            &slider,
            move |this: &mut SliderField, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(_) => cx.notify(),
                // Saved once it is let go, not for each step on the way.
                SliderEvent::Release(value) => {
                    let value = to_number(value.end());
                    this.saved = value;
                    set(value, cx);
                }
            },
        );
        SliderField {
            slider,
            saved: value,
            _subscription: subscription,
        }
    });
    // Changed in another way (Reset, or the file).
    if field.read(cx).saved != value {
        let slider = field.read(cx).slider.clone();
        slider.update(cx, |slider, cx| {
            slider.set_value(to_f32(value.clamp(min, max)), window, cx);
        });
        field.update(cx, |field, _| field.saved = value);
    }
    let slider = field.read(cx).slider.clone();
    let at = to_number(slider.read(cx).value().end());
    // A value beyond the slider's ends, set in the file, shows as it is until it is moved.
    let label = if at == value.clamp(min, max) {
        value
    } else {
        at
    };
    h_flex()
        .debug_selector({
            let id = number.id.clone();
            move || id.to_string()
        })
        .flex_none()
        .gap_3()
        .child(
            div().w(SLIDER_WIDTH).child(crate::ui::knob::outlined(
                Slider::new(&slider)
                    .bg(palette(cx).accent)
                    .accessibility_label(format!("{title}: {}", (number.shown)(label))),
                &slider,
                cx,
            )),
        )
        .child(
            // As wide as the longest value, so the sliders line up.
            div()
                .w(rems(5.))
                .text_sm()
                .text_right()
                .whitespace_nowrap()
                .child((number.shown)(label)),
        )
}

#[cfg(test)]
mod tests {
    use super::duration;

    #[test]
    fn durations_are_short() {
        assert_eq!(
            [30, 60, 90, 1200].map(duration),
            ["30 s", "1 min", "1 min 30 s", "20 min"]
        );
    }
}
