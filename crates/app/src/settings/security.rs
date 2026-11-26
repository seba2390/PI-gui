//! Settings → Sandboxing & Security: the sandbox the agent's tools run in (local Code sessions),
//! what it may use (folders, the network) and which project files pi loads (its project trust).
//! The state is [`crate::security::Security`].

use std::path::Path;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sandbox::{Backend, Network};

use crate::security::{Security, TrustDefault, security, tilde};
use crate::theme::palette;
use crate::ui::AppIcon;

/// The issue about a sandbox on Windows.
const WINDOWS_ISSUE: &str = "https://github.com/seba2390/pistachio/issues/47";
/// Rows a list shows; below them half of the next one fades out, and the list scrolls.
const LIST_ROWS_SHOWN: usize = 3;
/// The height of a row in a list.
const LIST_ROW: f32 = 38.;

/// `site_input` is the allowlist's field (the view keeps it, with its text).
pub(super) fn page(site_input: Entity<InputState>) -> SettingPage {
    SettingPage::new("Sandboxing & Security")
        .icon(AppIcon::ShieldCheck)
        .description(
            "What the agent can do on this computer in Code sessions. Sessions on SSH hosts run \
             there. Chats can't reach your files: their only tools are web search and Python in \
             a sealed interpreter (no files, network or programs).",
        )
        .group(
            SettingGroup::new()
                .title("Sandbox")
                .item(
                    SettingItem::render(|_, _, cx| status(cx))
                        .keywords(["sandbox", "Seatbelt", "bubblewrap", "Landlock"]),
                )
                .item(
                    SettingItem::new(
                        "Sandbox Code sessions",
                        super::view::hover_switch(
                            "sandbox-enabled",
                            |cx| read(cx, |s| s.settings.enabled),
                            |on, cx| change(cx, |settings| settings.enabled = on),
                        ),
                    )
                    .description(
                        "The agent's commands run in a sandbox, and its file tools keep to the same \
                         rules. Changes apply from the next command, also in running sessions. \
                         pi itself, its logins and model requests, MCP servers and the commands you \
                         run yourself with ! are not sandboxed.",
                    ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Files")
                .description(
                    "Your home folder is hidden from the agent, except the session's folder and \
                     what you allow here. Programs and system files outside it stay readable, \
                     since commands need them. Changes are allowed only in the session's \
                     folder, temporary files and the git folder its commits need, plus what you \
                     allow to change.",
                )
                .item(
                    SettingItem::new(
                        "Let tools use their folders",
                        super::view::hover_switch(
                            "sandbox-tool-folders",
                            |cx| read(cx, |s| s.settings.tool_folders),
                            |on, cx| change(cx, |settings| settings.tool_folders = on),
                        ),
                    )
                    .description(
                        "Commands may use the folders where tools like cargo, npm and git keep \
                         their programs, settings and caches.",
                    ),
                )
                .item(SettingItem::render(|_, _, cx| tool_folders(cx)).keywords([
                    "tools", "cargo", "npm", "nvm", "cache",
                ]))
                .item(SettingItem::render(|_, _, cx| allowed_folders(cx)).keywords([
                    "allow", "folder", "read", "write", "hidden", "home",
                ])),
        )
        .group(
            SettingGroup::new()
                .title("Internet")
                .item(
                    SettingItem::render(|_, _, cx| network_choice(cx))
                        .keywords(["network", "internet", "sites", "proxy"]),
                )
                .item(
                    SettingItem::render(move |_, window, cx| sites(&site_input, window, cx))
                        .keywords(["allowlist", "sites", "domains", "url"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Git and GitHub")
                .description(
                    "Your git and GitHub logins never enter the sandbox. When the agent needs them \
                     (push, pull, clone, gh, signing commits), you're asked, and it's done outside \
                     the sandbox; the agent sees only what it prints.",
                )
                .item(SettingItem::render(|_, _, cx| gate_diagram(cx)))
                .item(SettingItem::render(|_, _, cx| key_uses(cx)).keywords([
                    "git", "push", "pull", "clone", "github", "gh", "sign", "keys", "approve",
                ])),
        )
        .group(
            SettingGroup::new()
                .title("Project files")
                .description(
                    "A project's .pi folder can bring its own extensions (code that runs inside \
                     pi), MCP servers, skills and settings. pi loads them only for projects you \
                     trust (on the Skills page, or /trust in the terminal). AGENTS.md and \
                     CLAUDE.md are always read. Changes apply to sessions started afterwards.",
                )
                .item(
                    SettingItem::new(
                        "Without a decision",
                        SettingField::dropdown(
                            TrustDefault::ALL
                                .into_iter()
                                .map(|value| (value.id().into(), value.label().into()))
                                .collect(),
                            |cx| read(cx, |s| s.trust_default).id().into(),
                            |value, cx| {
                                let value = TrustDefault::from_id(value.as_ref());
                                security(cx).update(cx, |s, cx| s.set_trust_default(value, cx));
                            },
                        ),
                    )
                    .description("pi's defaultProjectTrust setting.")
                    .keywords(["trust", "project", "defaultProjectTrust", "extensions"]),
                )
                .item(SettingItem::render(|_, _, cx| trusted(cx)).keywords(["trusted"])),
        )
}

/// Reads the state (made when the Settings page opened).
fn read<T: Default>(cx: &App, f: impl FnOnce(&Security) -> T) -> T {
    crate::security::existing(cx).map_or_else(T::default, |state| f(state.read(cx)))
}

fn change(cx: &mut App, f: impl FnOnce(&mut sandbox::Settings)) {
    security(cx).update(cx, |s, cx| s.update(cx, f));
}

/// A titled box holding a list and, under it, how to add to it. A long list shows its first
/// rows with half of the next one fading out, scrolls smoothly over them, and has a thin bar
/// saying where it is. Under the pointer, where it takes scrolling, its border lights up.
fn list_card(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    rows: Vec<AnyElement>,
    empty: &'static str,
    footer: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    // Under the pointer: grey-white, not the accent.
    let lit = theme.foreground;
    let count = rows.len();
    let scrolls = count > LIST_ROWS_SHOWN;
    #[allow(clippy::cast_precision_loss)] // a short list
    let total = count as f32 * LIST_ROW;
    #[allow(clippy::cast_precision_loss)]
    let viewport = (LIST_ROWS_SHOWN as f32 + 0.5) * LIST_ROW;
    let max = (total - viewport).max(0.);
    let offset = read(cx, |s| s.list_offsets.get(id).copied().unwrap_or(0.)).clamp(0., max);
    let separator = theme.border.opacity(0.6);
    let rows = rows.into_iter().enumerate().map(|(index, row)| {
        h_flex()
            .w_full()
            .h(px(LIST_ROW))
            .flex_none()
            .pl_3()
            .pr_4()
            .overflow_hidden()
            .when(index + 1 < count, |this| {
                this.border_b_1().border_color(separator)
            })
            .child(row)
    });
    let group = SharedString::from(format!("{id}-card"));
    let scrollbar = scrolls.then(|| {
        div()
            .absolute()
            .top(px(6.))
            .bottom(px(6.))
            .right(px(4.))
            .w(px(3.))
            .rounded_full()
            .bg(theme.border.opacity(0.35))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w_full()
                    .top(relative(offset / total))
                    .h(relative(viewport / total))
                    .rounded_full()
                    .bg(theme.muted_foreground.opacity(0.5))
                    .group_hover(group.clone(), move |style| style.bg(lit.opacity(0.7))),
            )
    });
    // Fades where rows go on out of sight.
    let fade = |top: bool| {
        let (solid, clear) = (theme.background.opacity(0.92), theme.background.opacity(0.));
        let (from, to) = if top { (solid, clear) } else { (clear, solid) };
        div()
            .absolute()
            .left_0()
            .right_0()
            .h(px(LIST_ROW * 0.6))
            .map(|this| if top { this.top_0() } else { this.bottom_0() })
            .bg(linear_gradient(
                180.,
                linear_color_stop(from, 0.),
                linear_color_stop(to, 1.),
            ))
    };
    let list = div()
        .relative()
        .w_full()
        .when(count == 0, |this| {
            this.child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .py_4()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(empty),
            )
        })
        .map(|this| {
            if scrolls {
                this.h(px(viewport)).overflow_hidden().child(
                    v_flex()
                        .absolute()
                        .left_0()
                        .w_full()
                        .top(px(-offset))
                        .children(rows),
                )
            } else {
                this.children(rows)
            }
        })
        .when(scrolls && offset > 0.5, |this| this.child(fade(true)))
        .when(scrolls && offset < max - 0.5, |this| {
            this.child(fade(false))
        })
        .children(scrollbar)
        .when(scrolls, |this| this.child(scroll_catcher(id, max)));
    v_flex()
        .w_full()
        .gap_1p5()
        .py_1()
        .debug_selector(move || id.into())
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .child(
            v_flex()
                .id(group.clone())
                .group(group)
                .w_full()
                .mt_1()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background.opacity(0.6))
                .overflow_hidden()
                .hover(move |style| {
                    style.border_color(lit.opacity(0.4)).shadow(vec![BoxShadow {
                        color: lit.opacity(0.08),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(10.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                })
                .child(list)
                .child(
                    div()
                        .w_full()
                        .border_t_1()
                        .border_color(theme.border)
                        .px_3()
                        .py_2()
                        .child(footer),
                ),
        )
        .into_any_element()
}

/// Takes scrolling over a list before the Settings page does (in the capture phase: the page's
/// own scrolling would get it first otherwise), so the list moves; at its ends the page scrolls
/// as usual.
fn scroll_catcher(id: &'static str, max: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            // Only where the list can be seen (the page clips it while scrolling).
            let visible = window.content_mask().bounds.intersect(&bounds);
            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !visible.contains(&event.position) {
                    return;
                }
                let delta = event.delta.pixel_delta(window.line_height()).y;
                let took =
                    security(cx).update(cx, |s, cx| s.scroll_list(id, f32::from(delta), max, cx));
                if took {
                    cx.stop_propagation();
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The round "+" in the middle of a list's footer.
fn add_button(id: &'static str, tooltip: &'static str) -> Button {
    Button::new(id)
        .icon(IconName::Plus)
        .small()
        .outline()
        .rounded(px(999.))
        .tooltip(tooltip)
        .accessibility_label(tooltip)
}

/// The red trash can that removes a row.
fn remove_button(id: SharedString, what: String, cx: &App) -> Stateful<Div> {
    let red = palette(cx).error;
    div()
        .id(id)
        .flex_none()
        .p_1()
        .rounded_md()
        .cursor_pointer()
        .hover(move |style| style.bg(red.opacity(0.14)))
        .child(Icon::from(AppIcon::Trash).small().text_color(red))
        .tooltip(move |window, cx| {
            component::tooltip::Tooltip::new(format!("Remove {what}")).build(window, cx)
        })
}

/// A choice that is on or off: an icon and a word, green while on.
fn chip(id: SharedString, icon: Icon, label: &'static str, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (green, muted, hover) = (
        palette(cx).accent,
        theme.muted_foreground,
        theme.secondary_hover,
    );
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .text_xs()
        .cursor_pointer()
        .map(|this| {
            if on {
                this.bg(green.opacity(0.16))
                    .border_color(green.opacity(0.55))
                    .text_color(green)
            } else {
                this.border_color(transparent_black())
                    .text_color(muted)
                    .hover(move |style| style.bg(hover))
            }
        })
        .child(icon.xsmall())
        .child(label)
}

/// What a folder may be used for: an icon and its word, greyed out while off. A click turns it
/// on or off.
fn access(id: SharedString, icon: Icon, label: &'static str, on: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (foreground, muted, hover) = (
        theme.foreground,
        theme.muted_foreground,
        theme.secondary_hover,
    );
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .hover(move |style| style.bg(hover))
        .map(|this| {
            if on {
                this.text_color(foreground)
            } else {
                this.text_color(muted).opacity(0.45)
            }
        })
        .child(icon.small())
        .child(label)
        .tooltip(move |window, cx| {
            let tip = if on {
                format!("{label}: on (click to turn off)")
            } else {
                format!("{label}: off (click to turn on)")
            };
            component::tooltip::Tooltip::new(tip).build(window, cx)
        })
}

/// Which list of folders a row belongs to.
#[derive(Clone, Copy)]
enum Folders {
    /// The tool folders (until changed, what this computer has).
    Tools,
    Allowed,
}

impl Folders {
    fn key(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Allowed => "allowed",
        }
    }

    /// The list as shown.
    fn entries(self, cx: &App) -> Vec<sandbox::Allowed> {
        read(cx, |s| match self {
            Self::Tools => s.settings.tool_list(&sandbox::Context::current(), true),
            Self::Allowed => s.settings.allowed.clone(),
        })
    }

    /// Changes the list (the tool folders become the user's own from the first change).
    fn change(self, cx: &mut App, f: impl FnOnce(&mut Vec<sandbox::Allowed>)) {
        change(cx, |settings| match self {
            Self::Tools => {
                let mut list = settings.tool_list(&sandbox::Context::current(), true);
                f(&mut list);
                settings.tools = Some(list);
            }
            Self::Allowed => f(&mut settings.allowed),
        });
    }
}

/// The rows of a list of folders: the path, Read and Write to turn on and off (no reading means no
/// writing; writing needs reading) and the trash can.
fn folder_rows(list: Folders, cx: &App) -> Vec<AnyElement> {
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    list.entries(cx)
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let id = |what: &str| SharedString::from(format!("{}-{index}-{what}", list.key()));
            let read_on = entry.read;
            let write_on = entry.write && entry.read;
            let read = access(id("read"), Icon::new(IconName::Eye), "Read", read_on, cx).on_click(
                move |_, _, cx| {
                    list.change(cx, |entries| {
                        if let Some(entry) = entries.get_mut(index) {
                            entry.read = !read_on;
                            // No reading, no writing.
                            if read_on {
                                entry.write = false;
                            }
                        }
                    });
                },
            );
            let write = access(
                id("write"),
                Icon::from(AppIcon::Pencil),
                "Write",
                write_on,
                cx,
            )
            .on_click(move |_, _, cx| {
                list.change(cx, |entries| {
                    if let Some(entry) = entries.get_mut(index) {
                        entry.write = !write_on;
                        // Writing needs reading.
                        if !write_on {
                            entry.read = true;
                        }
                    }
                });
            });
            let remove =
                remove_button(id("remove"), entry.path.clone(), cx).on_click(move |_, _, cx| {
                    list.change(cx, |entries| {
                        if index < entries.len() {
                            entries.remove(index);
                        }
                    });
                });
            h_flex()
                .w_full()
                .gap_2()
                .child(Icon::new(IconName::Folder).small().text_color(muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .when(!read_on, |this| this.text_color(muted))
                        .child(entry.path.clone()),
                )
                .child(h_flex().flex_none().gap_1().child(read).child(write))
                .child(remove)
                .into_any_element()
        })
        .collect()
}

/// The centred "+" that asks for folders to add to `list` (readable; Write is a click away).
fn add_folders(list: Folders, id: &'static str) -> AnyElement {
    let add = add_button(id, "Add a folder").on_click(move |_, _, cx| {
        pick(false, "Add", cx, move |paths, cx| {
            list.change(cx, |entries| {
                for path in paths {
                    crate::security::add_to(entries, sandbox::Allowed::new(path, false));
                }
            });
        });
    });
    h_flex()
        .w_full()
        .justify_center()
        .child(add)
        .into_any_element()
}

/// The folders where tools keep their programs, settings and caches, while that is on.
fn tool_folders(cx: &mut App) -> AnyElement {
    if !read(cx, |s| s.settings.tool_folders) {
        return div().into_any_element();
    }
    let rows = folder_rows(Folders::Tools, cx);
    list_card(
        "sandbox-tools",
        "Tool folders",
        "Found on this computer. Caches can be written, so builds and installs work; programs and \
         settings only read.",
        rows,
        "No tool folders",
        add_folders(Folders::Tools, "tools-add"),
        cx,
    )
}

/// The folders outside the project the agent may use.
fn allowed_folders(cx: &mut App) -> AnyElement {
    let rows = folder_rows(Folders::Allowed, cx);
    list_card(
        "sandbox-allowed",
        "Allowed folders",
        "Folders outside the project the agent may read, or also write. Keys and logins \
         (~/.ssh, ~/.aws, ~/.gnupg, pi's own and others) stay hidden even inside an allowed \
         folder.",
        rows,
        "No folders yet",
        add_folders(Folders::Allowed, "allowed-add"),
        cx,
    )
}

/// None, All or Custom, side by side.
fn network_choice(cx: &mut App) -> AnyElement {
    let (network, blockable) = read(cx, |s| {
        (
            s.settings.network,
            s.status.as_ref().is_none_or(|status| status.network),
        )
    });
    let theme = cx.theme();
    let warning = palette(cx).warning;
    let option = |value: Network, icon: Icon, label: &'static str| {
        chip(
            SharedString::from(format!("network-{label}")),
            icon,
            label,
            network == value,
            cx,
        )
        .px_2p5()
        .py_1()
        .text_sm()
        .on_click(move |_, _, cx| change(cx, |settings| settings.network = value))
    };
    let description = match network {
        Network::Off => "Commands can't reach any website: installs and downloads fail.",
        Network::On => "Commands can reach every website.",
        Network::Custom => {
            "Commands reach only the websites below. Tools that don't use a proxy get none \
             (most do: curl, git, npm, pip, cargo)."
        }
    };
    v_flex()
        .w_full()
        .gap_1p5()
        .py_1()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Allowed websites"),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .child(option(Network::Off, Icon::from(AppIcon::GlobeOff), "None"))
                        .child(option(Network::On, Icon::from(AppIcon::Globe), "All"))
                        .child(option(
                            Network::Custom,
                            Icon::from(AppIcon::GlobeLock),
                            "Custom",
                        )),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(description),
        )
        .when(network != Network::On && !blockable, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(warning)
                    .child("This system can't limit the network (Landlock needs Linux 6.7)."),
            )
        })
        .into_any_element()
}

/// With a limited network: the allowed sites, and a field that checks an address as it is typed.
fn sites(input: &Entity<InputState>, window: &mut Window, cx: &mut App) -> AnyElement {
    let (network, sites, check) = read(cx, |s| {
        (
            s.settings.network,
            s.settings.sites.clone(),
            s.site_check.clone(),
        )
    });
    if network != Network::Custom {
        return div().into_any_element();
    }
    let theme = cx.theme();
    let (muted, mono, border) = (
        theme.muted_foreground,
        theme.mono_font_family.clone(),
        theme.border,
    );
    let (success, warning, error) = (palette(cx).success, palette(cx).warning, palette(cx).error);
    let rows = sites
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let (host, subdomains) = match label.strip_suffix("/*") {
                Some(host) => (host.to_owned(), true),
                None => (label.clone(), false),
            };
            h_flex()
                .w_full()
                .gap_2()
                .child(Icon::new(IconName::Globe).small().text_color(muted))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .child(host),
                )
                .when(subdomains, |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .text_xs()
                            .text_color(muted)
                            .child("and subdomains"),
                    )
                })
                .child(div().flex_1())
                .child(
                    remove_button(
                        SharedString::from(format!("site-remove-{index}")),
                        label.clone(),
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        change(cx, |settings| {
                            if index < settings.sites.len() {
                                settings.sites.remove(index);
                            }
                        });
                    }),
                )
                .into_any_element()
        })
        .collect();

    // What the field holds, checked as it is typed.
    let text = input.read(cx).value().to_string();
    let parsed = (!text.trim().is_empty()).then(|| sandbox::sites::parse(&text));
    let verdict: Option<(IconName, Hsla, String)> = match &parsed {
        None => None,
        Some(Err(why)) => Some((IconName::CircleX, error, why.clone())),
        Some(Ok(parsed)) => {
            let allows = if parsed.site.subdomains {
                format!("Allows {} and its subdomains", parsed.site.host)
            } else {
                format!("Allows {}", parsed.site.host)
            };
            let found = check
                .as_ref()
                .filter(|check| check.host == parsed.site.host)
                .and_then(|check| check.found);
            Some(match (found, &parsed.note) {
                (Some(false), _) => (
                    IconName::TriangleAlert,
                    warning,
                    format!("No site by that name answers (typo?). {allows}."),
                ),
                (_, Some(note)) => (IconName::Info, warning, format!("{note}.")),
                (None, None) => (
                    IconName::LoaderCircle,
                    muted,
                    format!("{allows} · checking…"),
                ),
                (Some(true), None) => (IconName::CircleCheck, success, format!("{allows}.")),
            })
        }
    };
    let valid = matches!(parsed, Some(Ok(_)));
    let field = input.clone();
    let add = add_button("site-add", "Add the site")
        .disabled(!valid)
        .on_click(move |_, window, cx| add_site(&field, window, cx));
    let _ = window;
    let footer = v_flex()
        .w_full()
        .gap_1p5()
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(div().flex_1().child(Input::new(input).small()))
                .child(add),
        )
        .when_some(verdict, |this, (icon, color, text)| {
            this.child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(color)
                    .debug_selector(|| "site-verdict".into())
                    .child(Icon::new(icon).xsmall())
                    .child(text),
            )
        })
        .into_any_element();
    list_card(
        "sandbox-sites",
        "Websites",
        "A website is allowed as a whole: pages can't be told apart over HTTPS. Add /* for its \
         subdomains too, like github.com/* for api.github.com.",
        rows,
        "No websites yet: commands reach none",
        footer,
        cx,
    )
}

/// Adds what the allowlist's field holds and empties it.
pub(super) fn add_site(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    let text = input.read(cx).value().to_string();
    let added = security(cx).update(cx, |s, cx| s.add_site(&text, cx));
    if added.is_ok() {
        input.update(cx, |input, cx| input.set_value("", window, cx));
    }
}

/// How the gate works, drawn: the agent's `git push` in the sandbox goes to Pistachio's gate,
/// which asks the user and does it with their keys (which never leave it) at the remote.
fn gate_diagram(cx: &mut App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let (sandbox, gate, remote) = (palette(cx).warning, palette(cx).accent, muted);
    let node = |title, about, color| diagram_node(title, about, color, muted);
    let region = |title, about, color| diagram_region(title, about, color, muted);
    let arrow = |icon: IconName| Icon::new(icon).small().text_color(muted);
    h_flex()
        .w_full()
        .justify_center()
        .items_center()
        .gap_2()
        .py_2()
        .debug_selector(|| "gate-diagram".into())
        .child(
            region("Sandbox", "The agent's commands", sandbox)
                .child(node("git push", "Handed over", sandbox))
                .child(node("No keys here", "Gets the result only", sandbox)),
        )
        .child(arrow(IconName::ArrowRight))
        .child(
            region("Pistachio", "Outside the sandbox", gate)
                .child(node("Git gate", "Checks it, asks you", gate))
                .child(arrow(IconName::ArrowUp))
                .child(node("Your keys", "Never leave here", gate)),
        )
        .child(arrow(IconName::ArrowRight))
        .child(node("github.com", "Your remote", remote))
        .into_any_element()
}

/// A box in the page's diagrams: what it is, and a few words about it.
fn diagram_node(title: &'static str, about: &'static str, color: Hsla, muted: Hsla) -> Div {
    v_flex()
        .w(px(158.))
        .flex_none()
        .items_center()
        .gap_0p5()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(color.opacity(0.55))
        .bg(color.opacity(0.1))
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().text_xs().text_color(muted).child(about))
}

/// A dashed area around boxes in the page's diagrams, named at the top.
fn diagram_region(title: &'static str, about: &'static str, color: Hsla, muted: Hsla) -> Div {
    v_flex()
        .flex_none()
        .items_center()
        .gap_2()
        .p_2()
        .rounded_xl()
        .border_1()
        .border_dashed()
        .border_color(color.opacity(0.45))
        .child(
            v_flex()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .child(title),
                )
                .child(div().text_xs().text_color(muted).child(about)),
        )
}

/// What the user allowed to be done with their keys without asking: per project and remote, the
/// kinds (push, fetch, …); each can be removed.
fn key_uses(cx: &mut App) -> AnyElement {
    let rules = read(cx, |s| s.settings.gate.clone());
    let theme = cx.theme();
    let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
    let rows = rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let kinds: Vec<&str> = rule.allow.iter().map(|class| class.label()).collect();
            h_flex()
                .w_full()
                .gap_2()
                .child(Icon::from(AppIcon::GitBranch).small().text_color(muted))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(260.))
                        .truncate()
                        .text_sm()
                        .font_family(mono.clone())
                        .child(rule.target.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(match &rule.account {
                            Some(account) => {
                                format!("{} as {account} · in {}", kinds.join(", "), rule.project)
                            }
                            None => format!("{} · in {}", kinds.join(", "), rule.project),
                        }),
                )
                .child(
                    remove_button(
                        SharedString::from(format!("gate-remove-{index}")),
                        format!("what {} may do without asking", rule.target),
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        change(cx, |settings| {
                            if index < settings.gate.len() {
                                settings.gate.remove(index);
                            }
                        });
                    }),
                )
                .into_any_element()
        })
        .collect();
    let footer = div()
        .w_full()
        .text_xs()
        .text_color(muted)
        .child(
            "Asked the first time; \u{201c}Always for this project\u{201d} adds it here. Force \
             pushes, the default branch and signing are asked about separately.",
        )
        .into_any_element();
    list_card(
        "sandbox-key-uses",
        "Allowed with your keys",
        "What the agent may do with your git and GitHub logins without asking, per project and \
         remote.",
        rows,
        "Nothing yet: the agent asks the first time it needs your keys",
        footer,
        cx,
    )
}

/// Which sandbox this computer has, and what to do when it has none.
fn status(cx: &mut App) -> AnyElement {
    let state = security(cx).read(cx);
    let theme = cx.theme();
    let (warning, success) = (palette(cx).warning, palette(cx).success);
    let enabled = state.settings.enabled;
    let (line, color, problem, fix) = match &state.status {
        None => (
            "Checking the sandbox…".to_owned(),
            theme.muted_foreground,
            None,
            None,
        ),
        Some(status) => match &status.backend {
            Some(backend) if enabled => (
                format!("Commands run in a sandbox ({}).", backend.label()),
                success,
                match backend {
                    // Why not bubblewrap.
                    Backend::Landlock => status.problem.clone(),
                    _ => None,
                },
                status.fix.clone(),
            ),
            Some(backend) => (
                format!("The sandbox is off. This computer has {}.", backend.label()),
                warning,
                None,
                None,
            ),
            None => (
                "Commands are not sandboxed on this computer.".to_owned(),
                warning,
                status
                    .problem
                    .clone()
                    .map(|problem| capitalize(&problem) + "."),
                status.fix.clone(),
            ),
        },
    };
    let windows = !sandbox::SUPPORTED;
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .child(
            h_flex()
                .w_full()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color)
                        .debug_selector(|| "sandbox-status".into())
                        .child(line),
                )
                .when(!windows, |this| {
                    this.child(
                        Button::new("sandbox-check-again")
                            .label("Check again")
                            .xsmall()
                            .ghost()
                            .disabled(state.status.is_none())
                            .on_click(|_, _, cx| security(cx).update(cx, Security::check)),
                    )
                }),
        )
        .when_some(problem, |this, problem| {
            this.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(problem),
            )
        })
        .when_some(fix, |this, fix| {
            this.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("To use bubblewrap:")
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.foreground)
                            .child(fix.clone()),
                    )
                    .child(
                        Button::new("sandbox-copy-fix")
                            .label("Copy")
                            .xsmall()
                            .ghost()
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(fix.clone()));
                            }),
                    ),
            )
        })
        .when(windows, |this| {
            this.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Commands run as you. A sandbox for Windows is planned.")
                    .child(
                        Button::new("sandbox-windows-issue")
                            .label("Follow it on GitHub")
                            .xsmall()
                            .ghost()
                            .on_click(|_, _, cx| cx.open_url(WINDOWS_ISSUE)),
                    ),
            )
        })
        .into_any_element()
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Asks for folders (or files) and passes them on.
fn pick(
    files: bool,
    prompt: &'static str,
    cx: &mut App,
    chosen: impl FnOnce(Vec<String>, &mut App) + 'static,
) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files,
        directories: true,
        multiple: true,
        prompt: Some(prompt.into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await {
            let paths = paths.iter().map(|path| tilde(path)).collect();
            cx.update(|cx| chosen(paths, cx));
        }
    })
    .detach();
}

/// The saved trust decisions, each with Remove.
fn trusted(cx: &mut App) -> AnyElement {
    let (rows, error) = read(cx, |s| (s.trusted.clone(), s.error.clone()));
    let theme = cx.theme();
    let warning = palette(cx).warning;
    let list = rows
        .into_iter()
        .enumerate()
        .map(|(index, (folder, trusted))| {
            let forget = folder.clone();
            h_flex()
                .w_full()
                .gap_2()
                .py_0p5()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .child(tilde(Path::new(&folder))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if trusted { "Trusted" } else { "Not trusted" }),
                )
                .child(
                    Button::new(SharedString::from(format!("trust-forget-{index}")))
                        .label("Remove")
                        .accessibility_label(format!("Remove the decision for {folder}"))
                        .xsmall()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            let folder = forget.clone();
                            security(cx).update(cx, |s, cx| s.forget_trust(&folder, cx));
                        }),
                )
        });
    let list: Vec<_> = list.collect();
    v_flex()
        .w_full()
        .gap_1()
        .py_1()
        .debug_selector(|| "trust-decisions".into())
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child("Decisions"),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Saved in pi's trust.json; a decision covers the folder's subfolders."),
        )
        .when(list.is_empty(), |this| {
            this.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .child("None yet."),
            )
        })
        .children(list)
        .when_some(error, |this, error| {
            this.child(div().text_xs().text_color(warning).child(error))
        })
        .into_any_element()
}
