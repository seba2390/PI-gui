//! A `request_user_help` request (see [`sandbox::help`]): the model hands the user a step the
//! sandbox blocks. It docks above the composer like a question ([`super::question`]), as a pill
//! that says "Needs your help: <title>" with Copy command, Dismiss and Done. Expanded, it shows
//! what the sandbox blocked (the command, its folder and its error) apart from what the model
//! says (its reason and the command it suggests).
//!
//! Only Done and Dismiss answer; copying doesn't. Like a question, nothing is chosen until the
//! user moves to a button or clicks one. Esc dismisses.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::protocol::{ExtensionUiAnswer, ExtensionUiMethod, ExtensionUiRequest};
use sandbox::help::{DISMISS, DONE, Request};

use super::question::OnAnswer;
use crate::ui::AppIcon;

/// How long "Copied" shows after copying.
const COPIED_FOR: Duration = Duration::from_secs(2);

/// The pill's buttons, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Copy,
    Dismiss,
    Done,
}

pub struct Help {
    id: SharedString,
    request: Request,
    focus: FocusHandle,
    /// The buttons shown, in order, each with its focus.
    actions: Vec<(Action, FocusHandle)>,
    expanded: bool,
    copied: bool,
    on_answer: OnAnswer,
}

impl Help {
    /// The help request in `request`, if it is one.
    pub fn request(request: &ExtensionUiRequest) -> Option<Request> {
        match &request.method {
            ExtensionUiMethod::Select { title, .. } => Request::parse(title),
            _ => None,
        }
    }

    pub fn new(id: &str, request: Request, on_answer: OnAnswer, cx: &mut Context<Self>) -> Self {
        let actions = [Action::Copy, Action::Dismiss, Action::Done]
            .into_iter()
            .filter(|action| *action != Action::Copy || request.command_to_copy().is_some())
            .map(|action| (action, cx.focus_handle().tab_stop(true)))
            .collect();
        Self {
            id: id.to_owned().into(),
            request,
            focus: cx.focus_handle(),
            actions,
            expanded: false,
            copied: false,
            on_answer,
        }
    }

    pub fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Hands the answer over once this card's own update is done (the chat then drops it).
    fn answer(&self, value: &str, window: &mut Window, cx: &mut App) {
        let (on_answer, id) = (self.on_answer.clone(), self.id.clone());
        let answer = ExtensionUiAnswer::Value(value.to_owned());
        window.defer(cx, move |window, cx| on_answer(&id, &answer, window, cx));
    }

    fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Done => self.answer(DONE, window, cx),
            Action::Dismiss => self.answer(DISMISS, window, cx),
            Action::Copy => {
                let Some(command) = self.request.command_to_copy() else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(command.to_owned()));
                self.copied = true;
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    let _ = this.update(cx, |this, cx| {
                        this.copied = false;
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.set_expanded(!self.expanded, cx);
    }

    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .actions
            .iter()
            .position(|(_, focus)| focus.is_focused(window));
        let last = self.actions.len().saturating_sub(1);
        let next = match (event.keystroke.key.as_str(), at) {
            ("escape", _) => {
                cx.stop_propagation();
                self.answer(DISMISS, window, cx);
                return;
            }
            ("down" | "right", None) => 0,
            ("down" | "right", Some(index)) => last.min(index + 1),
            ("up" | "left", None) => last,
            ("up" | "left", Some(index)) => index.saturating_sub(1),
            _ => return,
        };
        if let Some((_, focus)) = self.actions.get(next) {
            focus.focus(window, cx);
            cx.stop_propagation();
        }
    }

    fn render_action(&self, index: usize, cx: &Context<Self>) -> Button {
        let (action, focus) = &self.actions[index];
        let action = *action;
        let button = Button::new(("help-action", index))
            .small()
            .track_focus(focus)
            .on_click(cx.listener(move |this, _, window, cx| this.act(action, window, cx)));
        match action {
            Action::Done => button.primary().icon(IconName::Check).label("Done"),
            Action::Dismiss => button
                .ghost()
                .icon(IconName::Close)
                .label("Dismiss")
                .tooltip("Dismiss (Esc)"),
            Action::Copy if self.copied => button
                .outline()
                .icon(Icon::new(IconName::Check).text_color(crate::theme::palette(cx).success))
                .label("Copied"),
            Action::Copy => {
                let command = self.request.command_to_copy().unwrap_or_default();
                let button = button
                    .outline()
                    .icon(IconName::Copy)
                    .tooltip(format!("Copy command: {command}"));
                if self.expanded {
                    button.label("Copy command")
                } else {
                    button.accessibility_label("Copy command")
                }
            }
        }
    }

    /// The small round button on the pill's top edge that shows or hides the details.
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
                div().rounded_full().bg(surface).child(
                    Button::new("help-expand")
                        .outline()
                        .xsmall()
                        .rounded(ButtonRounded::Size(px(11.)))
                        .icon(icon)
                        .tooltip(tip)
                        .accessibility_label(tip)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                ),
            )
    }

    /// What the expanded pill shows above its buttons: what the model says, then what the
    /// sandbox blocked.
    fn render_details(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let heading = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(text)
        };
        let code = |text: String| {
            div()
                .w_full()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.muted)
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .text_color(theme.foreground)
                .whitespace_normal()
                .child(text)
        };
        let field = |label: &'static str, value: AnyElement| {
            h_flex()
                .w_full()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(64.))
                        .pt_1()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let request = &self.request;
        let model = v_flex()
            .gap_1()
            .child(heading("What the model says"))
            .when(!request.reason.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(request.reason.clone()),
                )
            })
            .when_some(request.suggested_command.clone(), |this, command| {
                this.child(field("Suggests", code(command).into_any_element()))
            });
        let blocked = request.blocked.as_ref().map(|blocked| {
            v_flex()
                .gap_1()
                .child(heading("What the sandbox blocked"))
                .child(field(
                    "Command",
                    code(blocked.command.clone()).into_any_element(),
                ))
                .when(!blocked.cwd.is_empty(), |this| {
                    this.child(field(
                        "Folder",
                        code(blocked.cwd.clone()).into_any_element(),
                    ))
                })
                .when(!blocked.error.is_empty(), |this| {
                    this.child(field(
                        "Error",
                        div()
                            .id("help-error")
                            .max_h(px(120.))
                            .overflow_y_scroll()
                            .child(code(blocked.error.clone()).text_color(muted))
                            .into_any_element(),
                    ))
                })
        });
        v_flex()
            .pl_6()
            .pr_2()
            .gap_2p5()
            .child(model)
            .children(blocked)
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Nothing runs for you: do it yourself if you agree, then choose Done."),
            )
    }
}

impl Focusable for Help {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Help {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::palette(cx);
        let theme = cx.theme();
        let expanded = self.expanded;
        let buttons: Vec<Button> = (0..self.actions.len())
            .map(|index| self.render_action(index, cx))
            .collect();
        let heading = h_flex()
            .id("help-heading")
            .flex_1()
            .min_w_0()
            .gap_1()
            .text_sm()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Needs your help:"),
            )
            .child(
                div()
                    .min_w_0()
                    .when(!expanded, |this| this.truncate())
                    .child(self.request.title.clone()),
            );
        v_flex()
            .id(ElementId::Name(format!("help-{}", self.id).into()))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .mx_2()
            .mb_2()
            .pl_3()
            .pr_1p5()
            .py_1p5()
            .gap_2()
            .when(expanded, |this| this.pt_4())
            .rounded(px(if expanded { 16. } else { 22. }))
            .border_1()
            .border_color(palette.accent.opacity(0.5))
            .bg(palette.surface.opacity(0.72))
            .when(expanded, |this| this.child(self.render_details(cx)))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_none().child(
                            Icon::from(AppIcon::HandHelping)
                                .text_color(palette.accent)
                                .small(),
                        ),
                    )
                    .child(heading)
                    .child(h_flex().flex_none().gap_1().children(buttons)),
            )
            .child(self.render_expand(cx))
            .text_color(theme.foreground)
            .with_animation(
                ElementId::Name(format!("help-in-{}", self.id).into()),
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

    use super::super::question::{Card, Docked, OnAnswer};
    use crate::ui::dialog::testing::{draw, press};

    struct Holder {
        docked: Option<Docked>,
    }

    impl Render for Holder {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .pt(gpui_kit::px(40.))
                .children(self.docked.as_ref().map(|docked| docked.card.view()))
        }
    }

    type Answers = Rc<RefCell<Vec<ExtensionUiAnswer>>>;

    thread_local! {
        /// The card [`help_request`] made last.
        static CARD: RefCell<Option<gpui_kit::Entity<super::Help>>> = const { RefCell::new(None) };
    }

    /// A help request docked the way the chat docks it, and the answers it gave.
    fn help_request(cx: &mut TestAppContext) -> (&mut VisualTestContext, Answers) {
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
                        assert_eq!(this.docked.take().expect("asked").id, id);
                        sent.borrow_mut().push(answer.clone());
                        cx.notify();
                    });
                });
                let request = ExtensionUiRequest {
                    id: "h1".into(),
                    method: ExtensionUiMethod::Select {
                        title: format!(
                            r#"{}{{"title":"Install jq","reason":"brew is blocked.","blocked":{{"command":"brew install jq","cwd":"/p","error":"Operation not permitted"}}}}"#,
                            sandbox::help::MARK
                        ),
                        options: vec!["Done".into(), "Dismiss".into()],
                        timeout: None,
                    },
                };
                let docked = Docked::new(&request, on_answer, cx);
                let Card::Help(help) = &docked.card else {
                    panic!("a help card");
                };
                CARD.with(|card| *card.borrow_mut() = Some(help.clone()));
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

    /// Copy command copies the blocked command (none was suggested) and answers nothing; Done
    /// answers.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn copying_answers_nothing_and_done_answers(cx: &mut TestAppContext) {
        let (cx, answers) = help_request(cx);
        cx.update(|window, cx| window.click(("help-action", 0usize), cx));
        draw(cx);
        assert_eq!(*answers.borrow(), []);
        let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(copied.as_deref(), Some("brew install jq"));
        cx.update(|window, cx| window.click(("help-action", 2usize), cx));
        draw(cx);
        assert_eq!(*answers.borrow(), [ExtensionUiAnswer::Value("Done".into())]);
    }

    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn enter_never_answers_and_esc_dismisses(cx: &mut TestAppContext) {
        let (cx, answers) = help_request(cx);
        press("enter space", cx);
        assert_eq!(*answers.borrow(), []);
        press("escape", cx);
        assert_eq!(
            *answers.borrow(),
            [ExtensionUiAnswer::Value("Dismiss".into())]
        );
    }

    /// Folded at first; the button on the edge opens and folds the details.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)]
    async fn the_details_open_and_fold(cx: &mut TestAppContext) {
        let (cx, answers) = help_request(cx);
        let card = CARD.with(|card| card.borrow().clone()).expect("card");
        let expanded = |cx: &mut VisualTestContext| cx.update(|_, cx| card.read(cx).expanded);
        assert!(!expanded(cx));
        cx.update(|window, cx| window.click("help-expand", cx));
        draw(cx);
        assert!(expanded(cx));
        cx.update(|window, cx| window.click("help-expand", cx));
        draw(cx);
        assert!(!expanded(cx));
        assert_eq!(*answers.borrow(), []);
    }
}
