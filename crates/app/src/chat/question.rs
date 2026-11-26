//! An extension's question with a few answers (`ctx.ui.select` and `ctx.ui.confirm`, such as the
//! sandbox gate's "Allow / Always for this project / Don't allow"), shown as a card that slides up
//! from behind the composer and stays docked just above it until it is answered.
//!
//! The card takes the keyboard when it opens, but nothing is chosen until the user moves to an
//! answer (←/→, ↑/↓ or Tab) or clicks one: an Enter meant for the message box never answers it.
//! Esc declines.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};

use crate::ui::AppIcon;

/// How far the card reaches under the composer, so it looks tucked behind it.
const TUCK: f32 = 14.;

/// What an answer does, for its icon and weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Allow,
    Always,
    Decline,
    Other,
}

impl Kind {
    fn of(label: &str) -> Self {
        let lower = label.to_lowercase();
        if lower.starts_with("always") {
            Self::Always
        } else if lower.starts_with("allow") || matches!(lower.as_str(), "ok" | "yes") {
            Self::Allow
        } else if ["don't", "do not", "deny", "decline", "cancel", "no"]
            .iter()
            .any(|word| lower == *word || lower.starts_with(&format!("{word} ")))
        {
            Self::Decline
        } else {
            Self::Other
        }
    }
}

struct Choice {
    label: SharedString,
    answer: ExtensionUiAnswer,
    kind: Kind,
    focus: FocusHandle,
}

/// A line under the question: what it is about, or what needs care.
enum Line {
    About(SharedString),
    Warning(SharedString),
}

pub type OnAnswer = Rc<dyn Fn(&str, &ExtensionUiAnswer, &mut Window, &mut App)>;

/// A question as the chat keeps it: what it needs without reading the card, which may be in the
/// middle of handling a click when the chat looks.
pub struct Docked {
    pub id: String,
    pub focus: FocusHandle,
    /// When pi stops waiting for the answer.
    pub expires: Option<std::time::Instant>,
    pub card: Entity<Question>,
}

impl Docked {
    pub fn new(request: &ExtensionUiRequest, on_answer: OnAnswer, cx: &mut App) -> Self {
        let card = cx.new(|cx| Question::new(request, on_answer, cx));
        let focus = card.read(cx).focus.clone();
        Self {
            id: request.id.clone(),
            focus,
            expires: request
                .timeout_ms()
                .map(|ms| std::time::Instant::now() + Duration::from_millis(ms)),
            card,
        }
    }

    pub fn expired(&self) -> bool {
        self.expires
            .is_some_and(|at| std::time::Instant::now() >= at)
    }
}

pub struct Question {
    id: SharedString,
    heading: SharedString,
    lines: Vec<Line>,
    choices: Vec<Choice>,
    focus: FocusHandle,
    on_answer: OnAnswer,
}

impl Question {
    /// Whether `request` is shown as a card (a select or a confirm).
    pub fn fits(request: &ExtensionUiRequest) -> bool {
        matches!(
            request.method,
            ExtensionUiMethod::Select { .. } | ExtensionUiMethod::Confirm { .. }
        )
    }

    /// The card for `request` (one that [`Self::fits`]).
    pub fn new(request: &ExtensionUiRequest, on_answer: OnAnswer, cx: &mut Context<Self>) -> Self {
        let (title, answers): (String, Vec<(String, ExtensionUiAnswer)>) = match &request.method {
            ExtensionUiMethod::Select { title, options, .. } => (
                title.clone(),
                options
                    .iter()
                    .map(|option| (option.clone(), ExtensionUiAnswer::Value(option.clone())))
                    .collect(),
            ),
            ExtensionUiMethod::Confirm { title, message, .. } => (
                if message.is_empty() {
                    title.clone()
                } else {
                    format!("{title}\n{message}")
                },
                vec![
                    ("OK".to_owned(), ExtensionUiAnswer::Confirmed(true)),
                    ("Cancel".to_owned(), ExtensionUiAnswer::Confirmed(false)),
                ],
            ),
            _ => (String::new(), Vec::new()),
        };
        // The first line is the question; the rest what it is about (the gate writes "· …" for
        // the commits and addresses, "⚠ …" for what needs care).
        let mut text = title.lines();
        let heading = text.next().unwrap_or_default().to_owned().into();
        let lines = text
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                if let Some(rest) = line.strip_prefix('⚠') {
                    Line::Warning(rest.trim().to_owned().into())
                } else {
                    Line::About(line.trim_start_matches('·').trim().to_owned().into())
                }
            })
            .collect();
        let choices = answers
            .into_iter()
            .map(|(label, answer)| Choice {
                kind: Kind::of(&label),
                label: label.into(),
                answer,
                focus: cx.focus_handle().tab_stop(true),
            })
            .collect();
        Self {
            id: request.id.clone().into(),
            heading,
            lines,
            choices,
            focus: cx.focus_handle(),
            on_answer,
        }
    }

    /// The answer Esc gives: the declining one, else "cancelled".
    fn declined(&self) -> ExtensionUiAnswer {
        self.choices
            .iter()
            .find(|choice| choice.kind == Kind::Decline)
            .map_or(ExtensionUiAnswer::Cancelled, |choice| choice.answer.clone())
    }

    /// Hands the answer over once this card's own update is done (the chat then drops the card).
    fn answer(&self, answer: &ExtensionUiAnswer, window: &mut Window, cx: &mut App) {
        let (on_answer, id, answer) = (self.on_answer.clone(), self.id.clone(), answer.clone());
        window.defer(cx, move |window, cx| on_answer(&id, &answer, window, cx));
    }

    fn focused(&self, window: &Window) -> Option<usize> {
        self.choices
            .iter()
            .position(|choice| choice.focus.is_focused(window))
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.choices.len().saturating_sub(1);
        let next = match (event.keystroke.key.as_str(), self.focused(window)) {
            ("escape", _) => {
                cx.stop_propagation();
                let declined = self.declined();
                self.answer(&declined, window, cx);
                return;
            }
            ("down" | "right", None) => 0,
            ("down" | "right", Some(index)) => last.min(index + 1),
            ("up" | "left", None) => last,
            ("up" | "left", Some(index)) => index.saturating_sub(1),
            _ => return,
        };
        if let Some(choice) = self.choices.get(next) {
            choice.focus.focus(window, cx);
            cx.stop_propagation();
        }
    }

    fn render_choice(&self, index: usize, primary: bool, cx: &Context<Self>) -> Button {
        let choice = &self.choices[index];
        let answer = choice.answer.clone();
        let button = Button::new(("question-answer", index))
            .small()
            .track_focus(&choice.focus)
            .label(choice.label.clone())
            .on_click(cx.listener(move |this, _, window, cx| this.answer(&answer, window, cx)));
        match choice.kind {
            Kind::Allow if primary => button.primary().icon(IconName::Check),
            Kind::Allow => button.outline().icon(IconName::Check),
            Kind::Always => button.outline().icon(Icon::from(AppIcon::CheckCheck)),
            Kind::Decline => button.ghost().icon(IconName::Ban).tooltip("Esc"),
            Kind::Other => button.outline(),
        }
    }
}

impl Focusable for Question {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Question {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, warning, primary) = (theme.muted_foreground, theme.warning, theme.primary);
        let (border, background) = (theme.border, theme.secondary);
        let careful = self
            .lines
            .iter()
            .any(|line| matches!(line, Line::Warning(_)));
        let permission = self
            .choices
            .iter()
            .any(|choice| matches!(choice.kind, Kind::Allow | Kind::Always));
        let icon = if careful {
            Icon::new(IconName::TriangleAlert).text_color(warning)
        } else if permission {
            Icon::from(AppIcon::ShieldCheck).text_color(primary)
        } else {
            Icon::new(IconName::Info).text_color(muted)
        };
        let lines = self.lines.iter().map(|line| match line {
            Line::About(text) => h_flex()
                .items_start()
                .gap_2()
                .text_color(muted)
                .child(div().flex_none().child("·"))
                .child(div().min_w_0().child(text.clone())),
            Line::Warning(text) => h_flex()
                .items_center()
                .gap_1p5()
                .text_color(warning)
                .child(Icon::new(IconName::TriangleAlert).xsmall())
                .child(div().min_w_0().child(text.clone())),
        });
        // The first "allow" is the main answer; declining sits apart on the right.
        let first_allow = self
            .choices
            .iter()
            .position(|choice| choice.kind == Kind::Allow);
        let (declines, answers): (Vec<usize>, Vec<usize>) =
            (0..self.choices.len()).partition(|&index| self.choices[index].kind == Kind::Decline);
        let answers: Vec<Button> = answers
            .into_iter()
            .map(|index| self.render_choice(index, Some(index) == first_allow, cx))
            .collect();
        let declines: Vec<Button> = declines
            .into_iter()
            .map(|index| self.render_choice(index, false, cx))
            .collect();
        v_flex()
            .id(ElementId::Name(format!("question-{}", self.id).into()))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .mx_3()
            .mb(px(-TUCK))
            .px_4()
            .pt_3()
            .pb(px(TUCK + 10.))
            .gap_2()
            .rounded_t(px(14.))
            .border_t_1()
            .border_l_1()
            .border_r_1()
            .border_color(if careful {
                warning.opacity(0.6)
            } else {
                border
            })
            .bg(background)
            .shadow_sm()
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_none().pt(px(2.)).child(icon.small()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.heading.clone()),
                    ),
            )
            .when(!self.lines.is_empty(), |this| {
                this.child(v_flex().pl_6().gap_0p5().text_xs().children(lines))
            })
            .child(
                h_flex()
                    .pl_6()
                    .pt_1()
                    .gap_2()
                    .flex_wrap()
                    .children(answers)
                    .child(div().flex_1())
                    .children(declines),
            )
            // Up from behind the composer.
            .with_animation(
                ElementId::Name(format!("question-in-{}", self.id).into()),
                Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).relative().top(px(36. * (1. - delta))),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, Window, div,
    };
    use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};
    use pretty_assertions::assert_eq;

    use super::{Docked, Kind, OnAnswer};
    use crate::ui::dialog::testing::{draw, press};

    /// Holds a docked question the way the chat does, and like the chat looks at it again when
    /// the answer comes (which crashed while the card was still handling the click).
    struct Holder {
        docked: Option<Docked>,
    }

    impl Render for Holder {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .children(self.docked.as_ref().map(|docked| docked.card.clone()))
        }
    }

    type Answers = Rc<RefCell<Vec<ExtensionUiAnswer>>>;

    fn gate_question(cx: &mut TestAppContext) -> (&mut VisualTestContext, Answers) {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
        });
        let answers = Answers::default();
        let sent = answers.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let holder = cx.new(|cx| {
                let weak = cx.weak_entity();
                let on_answer: OnAnswer = Rc::new(move |id, answer, _, cx| {
                    let sent = sent.clone();
                    let _ = weak.update(cx, |this: &mut Holder, cx| {
                        let docked = this.docked.take().expect("asked");
                        assert_eq!(docked.id, id);
                        let _ = docked.card.read(cx).heading.clone();
                        sent.borrow_mut().push(answer.clone());
                        cx.notify();
                    });
                });
                let request = ExtensionUiRequest {
                    id: "q1".into(),
                    method: ExtensionUiMethod::Select {
                        title: "Read issues on github.com/a/b\n· one\n⚠ careful".into(),
                        options: ["Allow", "Always for this project", "Don't allow"]
                            .map(str::to_owned)
                            .to_vec(),
                        timeout: None,
                    },
                };
                let docked = Docked::new(&request, on_answer, cx);
                window.focus(&docked.focus, cx);
                Holder {
                    docked: Some(docked),
                }
            });
            Root::new(holder, window, cx)
        });
        draw(cx);
        (cx, answers)
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn clicking_an_answer_sends_it(cx: &mut TestAppContext) {
        let (cx, answers) = gate_question(cx);
        cx.update(|window, cx| window.click(("question-answer", 0usize), cx));
        draw(cx);
        assert_eq!(
            *answers.borrow(),
            [ExtensionUiAnswer::Value("Allow".into())]
        );
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn the_keyboard_answers_and_esc_declines(cx: &mut TestAppContext) {
        let (cx, answers) = gate_question(cx);
        // Nothing is chosen by an Enter meant for something else.
        press("enter space", cx);
        assert_eq!(*answers.borrow(), []);
        press("escape", cx);
        assert_eq!(
            *answers.borrow(),
            [ExtensionUiAnswer::Value("Don't allow".into())]
        );
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn the_arrows_move_between_answers(cx: &mut TestAppContext) {
        let (cx, answers) = gate_question(cx);
        press("right right enter", cx);
        assert_eq!(
            *answers.borrow(),
            [ExtensionUiAnswer::Value("Always for this project".into())]
        );
    }

    #[test]
    fn answers_are_told_apart_by_their_words() {
        for (label, kind) in [
            ("Allow", Kind::Allow),
            ("Allow as seba2390", Kind::Allow),
            ("Always for this project", Kind::Always),
            ("Always for this project as me-at-work", Kind::Always),
            ("Don't allow", Kind::Decline),
            ("Cancel", Kind::Decline),
            ("OK", Kind::Allow),
            ("Nothing", Kind::Other),
            ("option 1", Kind::Other),
        ] {
            assert_eq!(Kind::of(label), kind, "{label}");
        }
    }
}
