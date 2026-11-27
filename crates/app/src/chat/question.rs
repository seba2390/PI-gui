//! An extension's question with a few answers (`ctx.ui.select` and `ctx.ui.confirm`, such as the
//! sandbox gate's "Allow / Always for this project / Don't allow"), shown as a card that slides up
//! from behind the composer and stays docked just above it until it is answered.
//!
//! The card takes the keyboard when it opens, but nothing is chosen until the user moves to an
//! answer (←/→, ↑/↓ or Tab) or clicks one: an Enter meant for the message box never answers it.
//! Esc declines.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};

use crate::ui::AppIcon;

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

/// What is docked: a question, or a request for help from `request_user_help` (a select too,
/// see [`super::help`]).
pub enum Card {
    Question(Entity<Question>),
    Help(Entity<super::help::Help>),
}

impl Card {
    pub fn view(&self) -> AnyView {
        match self {
            Self::Question(card) => card.clone().into(),
            Self::Help(card) => card.clone().into(),
        }
    }
}

/// A question as the chat keeps it: what it needs without reading the card, which may be in the
/// middle of handling a click when the chat looks.
pub struct Docked {
    pub id: String,
    pub focus: FocusHandle,
    /// When pi stops waiting for the answer.
    pub expires: Option<std::time::Instant>,
    pub card: Card,
}

impl Docked {
    pub fn new(request: &ExtensionUiRequest, on_answer: OnAnswer, cx: &mut App) -> Self {
        let (card, focus) = match super::help::Help::request(request) {
            Some(help) => {
                let card = cx.new(|cx| super::help::Help::new(&request.id, help, on_answer, cx));
                let focus = card.read(cx).focus().clone();
                (Card::Help(card), focus)
            }
            None => {
                let card = cx.new(|cx| Question::new(request, on_answer, cx));
                let focus = card.read(cx).focus.clone();
                (Card::Question(card), focus)
            }
        };
        Self {
            id: request.id.clone(),
            focus,
            expires: request
                .timeout_ms()
                .map(|ms| std::time::Instant::now() + Duration::from_millis(ms)),
            card,
        }
    }

    /// A request for help: it waits on a tool call, so it goes when the agent stops.
    pub fn is_help(&self) -> bool {
        matches!(self.card, Card::Help(_))
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
    /// The details (commits, addresses, warnings) shown; at first only when something needs care.
    expanded: bool,
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
        let lines: Vec<Line> = text
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
        let expanded = lines.iter().any(|line| matches!(line, Line::Warning(_)));
        Self {
            id: request.id.clone().into(),
            heading,
            expanded,
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
        let order = self.shown_order();
        let at = self
            .focused(window)
            .and_then(|focused| order.iter().position(|&index| index == focused));
        let last = order.len().saturating_sub(1);
        let next = match (event.keystroke.key.as_str(), at) {
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
        if let Some(choice) = order.get(next).and_then(|&index| self.choices.get(index)) {
            choice.focus.focus(window, cx);
            cx.stop_propagation();
        }
    }

    /// The answers in the order they are shown, left to right: declining first, then the others,
    /// the main "allow" last (on the right). The arrow keys go along this order.
    fn shown_order(&self) -> Vec<usize> {
        let main = self.main_allow();
        let mut order: Vec<usize> = (0..self.choices.len())
            .filter(|&index| self.choices[index].kind == Kind::Decline)
            .collect();
        order.extend(
            (0..self.choices.len())
                .filter(|&index| self.choices[index].kind != Kind::Decline && Some(index) != main),
        );
        order.extend(main);
        order
    }

    /// The first "allow": the main answer.
    fn main_allow(&self) -> Option<usize> {
        self.choices
            .iter()
            .position(|choice| choice.kind == Kind::Allow)
    }

    fn render_choice(&self, index: usize, cx: &Context<Self>) -> Button {
        let choice = &self.choices[index];
        let answer = choice.answer.clone();
        let label = choice.label.clone();
        let button = Button::new(("question-answer", index))
            .small()
            .track_focus(&choice.focus)
            .on_click(cx.listener(move |this, _, window, cx| this.answer(&answer, window, cx)));
        // Declining and "always" are icons (named in their tooltips) when there is one of each;
        // with an answer per account, they keep their words.
        let alone = |kind: Kind| {
            self.choices
                .iter()
                .filter(|choice| choice.kind == kind)
                .count()
                == 1
        };
        match choice.kind {
            Kind::Allow if Some(index) == self.main_allow() => {
                button.primary().icon(IconName::Check).label(label)
            }
            Kind::Allow => button.outline().icon(IconName::Check).label(label),
            Kind::Always if alone(Kind::Always) => button
                .outline()
                .icon(Icon::from(AppIcon::CheckCheck))
                .tooltip(label.clone())
                .accessibility_label(label),
            Kind::Always => button
                .outline()
                .icon(Icon::from(AppIcon::CheckCheck))
                .label(label),
            Kind::Decline if alone(Kind::Decline) => button
                .ghost()
                .icon(IconName::Close)
                .tooltip(format!("{label} (Esc)"))
                .accessibility_label(label),
            Kind::Decline => button.ghost().icon(IconName::Close).label(label),
            Kind::Other => button.outline().label(label),
        }
    }

    /// The small round button on the card's top edge that shows or hides the details.
    fn render_expand(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let surface = crate::theme::palette(cx).surface;
        let (icon, tip) = if self.expanded {
            (IconName::ChevronDown, "Fewer details")
        } else {
            (IconName::ChevronUp, "More details")
        };
        h_flex()
            .absolute()
            .top(px(-11.))
            .left_0()
            .right_0()
            .justify_center()
            .child(
                // Opaque, so the card's border doesn't run through it.
                div().rounded_full().bg(surface).child(
                    Button::new("question-expand")
                        .outline()
                        .xsmall()
                        .rounded(ButtonRounded::Size(px(11.)))
                        .icon(icon)
                        .tooltip(tip)
                        .accessibility_label(tip)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.expanded = !this.expanded;
                            cx.notify();
                        })),
                ),
            )
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
        let (muted, warning, border) = (theme.muted_foreground, theme.warning, theme.border);
        let palette = crate::theme::palette(cx);
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
            Icon::from(AppIcon::ShieldCheck).text_color(palette.accent)
        } else {
            Icon::new(IconName::Info).text_color(muted)
        };
        let expanded = self.expanded && !self.lines.is_empty();
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
        let buttons: Vec<Button> = self
            .shown_order()
            .into_iter()
            .map(|index| self.render_choice(index, cx))
            .collect();
        v_flex()
            .id(ElementId::Name(format!("question-{}", self.id).into()))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .mx_2()
            .mb_2()
            .pl_3()
            .pr_1p5()
            .py_1p5()
            .gap_1p5()
            .when(expanded, |this| this.pt_4())
            .rounded(px(if expanded { 16. } else { 22. }))
            .border_1()
            .border_color(if careful {
                warning.opacity(0.55)
            } else {
                border
            })
            // See-through: the conversation shows behind it.
            .bg(palette.surface.opacity(0.72))
            .when(expanded, |this| {
                this.child(v_flex().pl_6().pr_2().gap_0p5().text_xs().children(lines))
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_none().child(icon.small()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .when(!expanded, |this| this.truncate())
                            .child(self.heading.clone()),
                    )
                    .child(h_flex().flex_none().gap_1().children(buttons)),
            )
            .when(!self.lines.is_empty(), |this| {
                this.child(self.render_expand(cx))
            })
            // Up from behind the composer.
            .with_animation(
                ElementId::Name(format!("question-in-{}", self.id).into()),
                Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).top(px(28. * (1. - delta))),
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

    use super::{Card, Docked, Kind, OnAnswer};
    use crate::ui::dialog::testing::{draw, press};

    /// Holds a docked question the way the chat does, and like the chat looks at it again when
    /// the answer comes (which crashed while the card was still handling the click).
    struct Holder {
        docked: Option<Docked>,
    }

    impl Render for Holder {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // Room above for the button on the card's top edge.
            div()
                .size_full()
                .pt(gpui_kit::px(40.))
                .children(self.docked.as_ref().map(|docked| docked.card.view()))
        }
    }

    type Answers = Rc<RefCell<Vec<ExtensionUiAnswer>>>;

    thread_local! {
        /// The card [`gate_question`] made last.
        static CARD: RefCell<Option<gpui_kit::Entity<super::Question>>> = const { RefCell::new(None) };
    }

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
                        if let Card::Question(card) = &docked.card {
                            let _ = card.read(cx).heading.clone();
                        }
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
                if let Card::Question(question) = &docked.card {
                    CARD.with(|card| *card.borrow_mut() = Some(question.clone()));
                }
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

    /// A question with something that needs care opens with its details; the button on the
    /// card's edge folds them away and back.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn the_details_fold_and_unfold(cx: &mut TestAppContext) {
        let (cx, answers) = gate_question(cx);
        let card = CARD.with(|card| card.borrow().clone()).expect("card");
        assert!(cx.update(|_, cx| card.read(cx).expanded));
        for open in [false, true] {
            cx.update(|window, cx| window.click("question-expand", cx));
            draw(cx);
            assert_eq!(cx.update(|_, cx| card.read(cx).expanded), open);
        }
        assert_eq!(*answers.borrow(), []);
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
