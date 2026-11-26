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
/// Rows a list shows; below them the next one fades out, and the list scrolls a row at a time.
const LIST_ROWS_SHOWN: usize = 3;
/// How far to scroll for one row.
const LIST_ROW_SCROLL: f32 = 34.;

/// `site_input` is the allowlist's field (the view keeps it, with its text).
pub(super) fn page(site_input: Entity<InputState>) -> SettingPage {
    SettingPage::new("Sandboxing & Security")
        .icon(AppIcon::ShieldCheck)
        .description(
            "What the agent can do on this computer in Code sessions. Sessions on SSH hosts run \
             there, and chats have no tools.",
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
                        "The folders in your home folder where tools keep their programs, \
                         settings and downloads (~/.cargo, ~/.npm, ~/.nvm, ~/.gitconfig, …). \
                         pi's skills can always be read.",
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
/// rows with the next one fading out, scrolls a row at a time over them, and has a thin bar
/// saying where it is.
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
    let count = rows.len();
    let last = count.saturating_sub(LIST_ROWS_SHOWN);
    let top = read(cx, |s| s.list_tops.get(id).copied().unwrap_or(0)).min(last);
    let more_below = top + LIST_ROWS_SHOWN < count;
    let separator = theme.border.opacity(0.6);
    let row_box = |row: AnyElement, line: bool| {
        div()
            .w_full()
            .pl_3()
            .pr_4()
            .py_1p5()
            .when(line, |this| this.border_b_1().border_color(separator))
            .child(row)
    };
    let mut shown: Vec<AnyElement> = Vec::new();
    let mut peek = None;
    for (index, row) in rows.into_iter().enumerate().skip(top) {
        if index < top + LIST_ROWS_SHOWN {
            shown.push(row_box(row, index + 1 < count).into_any_element());
        } else {
            // The next row, cut and faded: there is more.
            peek = Some(
                div()
                    .h(px(18.))
                    .overflow_hidden()
                    .opacity(0.3)
                    .child(row_box(row, false))
                    .into_any_element(),
            );
            break;
        }
    }
    #[allow(clippy::cast_precision_loss)] // a short list
    let (thumb_height, thumb_top) = if count > 0 {
        (
            LIST_ROWS_SHOWN.min(count) as f32 / count as f32,
            top as f32 / count as f32,
        )
    } else {
        (1., 0.)
    };
    let scrollbar = (count > LIST_ROWS_SHOWN).then(|| {
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
                    .top(relative(thumb_top))
                    .h(relative(thumb_height))
                    .rounded_full()
                    .bg(theme.muted_foreground.opacity(0.6)),
            )
    });
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
                .w_full()
                .mt_1()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background.opacity(0.6))
                .overflow_hidden()
                .child(
                    div()
                        .id(id)
                        .relative()
                        .w_full()
                        .on_scroll(move |event, window, cx| {
                            let delta = event.delta.pixel_delta(window.line_height()).y;
                            let rows = (count, LIST_ROWS_SHOWN);
                            let moved = security(cx).update(cx, |s, cx| {
                                s.scroll_list(id, f32::from(delta), rows, LIST_ROW_SCROLL, cx)
                            });
                            if moved {
                                cx.stop_propagation();
                            }
                        })
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
                        .children(shown)
                        .children(peek)
                        .children(scrollbar),
                )
                .child(
                    div()
                        .w_full()
                        .when(!more_below || count == 0, |this| {
                            this.border_t_1().border_color(theme.border)
                        })
                        .px_3()
                        .py_2()
                        .child(footer),
                ),
        )
        .into_any_element()
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
