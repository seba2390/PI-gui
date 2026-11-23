//! The saved connections in Settings: a switch per host that connects and disconnects it, a
//! "⋮" menu (rename, change, remove) and a "+" to add one.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::HostId;

use crate::hosts::hosts;
use crate::ui::{AppIcon, QuietButton as _, hover_switch};

/// What the user asked for in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedHostAction {
    Add,
    Connect(HostId),
    Disconnect(HostId),
    Rename(HostId),
    Change(HostId),
    Remove(HostId),
}

type OnAction = Rc<dyn Fn(SavedHostAction, &mut Window, &mut App)>;

pub fn saved_hosts(
    on_action: impl Fn(SavedHostAction, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let on_action: OnAction = Rc::new(on_action);
    let muted = cx.theme().muted_foreground;
    let hosts = hosts(cx);
    let hosts = hosts.read(cx);
    let rows: Vec<AnyElement> = hosts
        .hosts()
        .iter()
        .map(|host| {
            let status = hosts.status(&host.id);
            host_row(&host.id, &host.label, &status, &on_action, cx)
        })
        .collect();
    v_flex()
        .w_full()
        .gap_1()
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No saved connections yet."),
            )
        })
        .children(rows)
        .child(
            h_flex().w_full().justify_center().pt_1().child(
                div().debug_selector(|| "add-host".into()).child(
                    Button::new("settings-add-host")
                        .quiet(cx)
                        .icon(IconName::Plus)
                        .accessibility_label("Add SSH connection")
                        .tooltip("Add SSH connection")
                        .on_click(move |_, window, cx| {
                            on_action(SavedHostAction::Add, window, cx);
                        }),
                ),
            ),
        )
        .into_any_element()
}

fn host_row(
    id: &HostId,
    label: &str,
    status: &crate::hosts::HostStatus,
    on_action: &OnAction,
    cx: &App,
) -> AnyElement {
    // On while it connects too, so an attempt can be stopped.
    let on = status.is_connected() || status.is_busy();
    let (toggle, toggle_id) = (on_action.clone(), id.clone());
    let (menu, menu_id) = (on_action.clone(), id.clone());
    h_flex()
        .w_full()
        .gap_2()
        .py_1()
        .child(super::status_dot(Some(status), cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().truncate().child(label.to_owned()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(super::status_text(status)),
                ),
        )
        .child(
            hover_switch(
                ElementId::Name(format!("host-switch:{}", id.0).into()),
                on,
                move |on, window, cx| {
                    let id = toggle_id.clone();
                    let action = if on {
                        SavedHostAction::Connect(id)
                    } else {
                        SavedHostAction::Disconnect(id)
                    };
                    toggle(action, window, cx);
                },
                Some(format!("Connect {label}").into()),
                cx,
            )
            .flex_none()
            .debug_selector(|| format!("host-switch:{label}")),
        )
        .child(
            div()
                .flex_none()
                .debug_selector(|| format!("host-menu:{label}"))
                .child(
                    Button::new(ElementId::Name(format!("host-menu:{}", id.0).into()))
                        .quiet(cx)
                        .small()
                        .icon(IconName::EllipsisVertical)
                        .accessibility_label(format!("More actions for {label}"))
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, _, _| {
                            let item = |label: &'static str, action: SavedHostAction| {
                                let on_action = menu.clone();
                                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                    on_action(action.clone(), window, cx);
                                })
                            };
                            let id = || menu_id.clone();
                            popup
                                .item(
                                    item("Rename…", SavedHostAction::Rename(id()))
                                        .icon(AppIcon::Pencil),
                                )
                                .item(
                                    item("Change…", SavedHostAction::Change(id()))
                                        .icon(AppIcon::Wrench),
                                )
                                .separator()
                                .item(
                                    item("Remove…", SavedHostAction::Remove(id()))
                                        .icon(IconName::Delete),
                                )
                        }),
                ),
        )
        .into_any_element()
}

/// Asks for a new name for `host`.
pub fn open_rename_host(host: &HostId, window: &mut Window, cx: &mut App) {
    let label = hosts(cx).read(cx).label(host);
    let input = cx.new(|cx| InputState::new(window, cx).default_value(label));
    let host = host.clone();
    open_input_dialog(
        "Rename connection".into(),
        None,
        input,
        move |name, cx| {
            if name.trim().is_empty() {
                return false;
            }
            hosts(cx).update(cx, |hosts, cx| hosts.rename(&host, name, cx));
            true
        },
        window,
        cx,
    );
}

/// Asks for the command that starts pi on `host`.
pub fn open_change_host(host: &HostId, window: &mut Window, cx: &mut App) {
    let (label, command, address) = {
        let hosts = hosts(cx);
        let hosts = hosts.read(cx);
        let Some(saved) = hosts.get(host) else {
            return;
        };
        (
            saved.label.clone(),
            saved.pi_command.clone().unwrap_or_default(),
            saved.target.display(),
        )
    };
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("pi (installed automatically when missing)")
            .default_value(command)
    });
    let host = host.clone();
    open_input_dialog(
        format!("Change {label}").into(),
        Some(format!("Command that starts pi on {address}").into()),
        input,
        move |command, cx| {
            hosts(cx).update(cx, |hosts, cx| hosts.set_pi_command(&host, command, cx));
            true
        },
        window,
        cx,
    );
}

/// Asks before removing `host` from the saved connections.
pub fn confirm_remove_host(host: &HostId, window: &mut Window, cx: &mut App) {
    let label = hosts(cx).read(cx).label(host);
    let host = host.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let host = host.clone();
        alert
            .title(format!("Remove “{label}”?"))
            .description("Its connection is closed. Sessions on the host are kept.")
            .confirm()
            .ok_text("Remove")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                hosts(cx).update(cx, |hosts, cx| hosts.remove(&host, cx));
                true
            })
    });
}

/// A dialog with one text field; Save and Enter close it when `save` takes the text.
fn open_input_dialog(
    title: SharedString,
    field_label: Option<SharedString>,
    input: Entity<InputState>,
    save: impl Fn(String, &mut App) -> bool + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let submit = {
        let input = input.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let value = input.read(cx).value().to_string();
            if save(value, cx) {
                window.close_dialog(cx);
            }
        })
    };
    let handle = window.window_handle();
    {
        let submit = submit.clone();
        cx.subscribe(&input, move |_, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let submit = submit.clone();
                let _ = handle.update(cx, move |_, window, cx| submit(window, cx));
            }
        })
        .detach();
    }
    input.update(cx, |input, cx| input.focus(window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let submit = submit.clone();
        dialog
            .title(title.clone())
            .w(px(420.))
            .child(
                v_flex()
                    .gap_1()
                    .when_some(field_label.clone(), |this, label| {
                        this.child(div().text_xs().text_color(muted).child(label))
                    })
                    .child(Input::new(&input)),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("host-edit-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("host-edit-save")
                            .primary()
                            .label("Save")
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        AppContext as _, Context, IntoElement, Modifiers, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, Window, div, px,
    };
    use remote::{HostStore, Target};

    use super::SavedHostAction;
    use crate::hosts::{HostStatus, Hosts, HostsHandle};

    struct List(Rc<RefCell<Vec<SavedHostAction>>>);

    impl Render for List {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let actions = self.0.clone();
            div().w(px(600.)).child(super::saved_hosts(
                move |action, _, _| actions.borrow_mut().push(action),
                cx,
            ))
        }
    }

    /// The switch connects and disconnects, "⋮" (at the right edge) offers rename, change and
    /// remove, and "+" is centered under the last host.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn rows_have_a_switch_and_a_menu(cx: &mut TestAppContext) {
        let mut store = HostStore::default();
        let ids = ["one", "two"].map(|alias| {
            let alias = alias.to_owned();
            store.add(alias.clone(), Target::Alias { alias })
        });
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            let hosts = cx.new(|_| Hosts::for_tests(store.hosts));
            hosts.update(cx, |hosts, cx| {
                hosts.set_status_for_tests(&ids[1], HostStatus::Connected, cx);
            });
            cx.set_global(HostsHandle(hosts));
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let shown = actions.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let list = cx.new(|_| List(shown));
            gpui_kit::component::Root::new(list, window, cx)
        });
        let draw = |cx: &mut VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        let take = || std::mem::take(&mut *actions.borrow_mut());
        draw(cx);

        for (label, expected) in [
            ("one", SavedHostAction::Connect(ids[0].clone())),
            ("two", SavedHostAction::Disconnect(ids[1].clone())),
        ] {
            let switch = cx
                .debug_bounds(Box::leak(format!("host-switch:{label}").into_boxed_str()))
                .unwrap();
            cx.simulate_click(switch.center(), Modifiers::default());
            assert_eq!(take(), [expected]);
            let menu = cx
                .debug_bounds(Box::leak(format!("host-menu:{label}").into_boxed_str()))
                .unwrap();
            assert!(menu.left() >= switch.right());
            assert_eq!(menu.right(), px(600.));
        }

        let add = cx.debug_bounds("add-host").unwrap();
        assert!((add.center().x - px(300.)).abs() < px(1.));
        let last = cx.debug_bounds("host-menu:two").unwrap();
        assert!(add.top() >= last.bottom());
        cx.simulate_click(add.center(), Modifiers::default());
        assert_eq!(take(), [SavedHostAction::Add]);

        for (downs, expected) in [
            (1, SavedHostAction::Rename(ids[1].clone())),
            (2, SavedHostAction::Change(ids[1].clone())),
            (3, SavedHostAction::Remove(ids[1].clone())),
        ] {
            cx.simulate_click(last.center(), Modifiers::default());
            draw(cx);
            cx.simulate_keystrokes(&vec!["down"; downs].join(" "));
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(take(), [expected]);
        }
    }
}
